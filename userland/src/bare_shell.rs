//! Minimal shell support for bare-metal boot while the full shell is feature-gated to `std` builds.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use pc_keyboard::layouts::Us104Key;
use pc_keyboard::{DecodedKey, HandleControl, Keyboard, ScancodeSet1};

/// IO abstraction that platform glue must implement for the bare shell.
pub trait ShellIo {
    /// Returns the next raw scancode if available.
    fn next_scancode(&mut self) -> Option<u8>;
    /// Writes a UTF-8 string to the console/backing output.
    fn write_str(&mut self, s: &str);
}

/// Bare minimal shell loop used during bring-up on bare-metal targets.
pub struct BareShell<Io, Fs, Sys> {
    keyboard: Keyboard<Us104Key, ScancodeSet1>,
    input: String,
    history: Vec<String>,
    current_dir: String,
    io: Io,
    fs: Fs,
    sys: Sys,
}

const PROMPT_PREFIX: &str = "bare shell";
const COLOR_BLUE: &str = "\x1b[1;34m";
const COLOR_RESET: &str = "\x1b[0m";
const HELP_ENTRIES: &[(&str, &str)] = &[
    ("help", "Show this help message"),
    ("history", "Display previously executed commands"),
    ("sh", "Launch a nested shell session"),
    ("ls", "List entries from the mounted filesystem"),
    ("pwd", "Print the current working directory"),
    ("cd", "Change the current working directory"),
    ("cat", "Display file contents"),
    ("echo", "Print arguments back to the console"),
    (
        "touch",
        "Create an empty file (not supported on read-only FS)",
    ),
    (
        "mkdir",
        "Create a directory (not supported on read-only FS)",
    ),
    (
        "rmdir",
        "Remove a directory (not supported on read-only FS)",
    ),
    ("rm", "Remove a file (not supported on read-only FS)"),
    ("cp", "Copy a file (not supported on read-only FS)"),
    (
        "mv",
        "Move or rename a file (not supported on read-only FS)",
    ),
    ("reboot", "Reboot the system"),
    ("shutdown", "Power off the system"),
];

/// Filesystem abstraction exposed to the bare shell.
pub trait ShellFs {
    /// Lists directory entries for the provided absolute path.
    fn list_dir(&self, path: &str) -> Result<Vec<DirEntry>, FsError>;
    /// Reads a regular file from the provided absolute path.
    fn read_file(&self, path: &str) -> Result<Vec<u8>, FsError>;
}

/// Platform control hooks exposed to the shell.
pub trait ShellSystem {
    /// Requests a system reboot.
    fn reboot(&self) -> Result<(), SystemError>;
    /// Requests a system shutdown/power off.
    fn shutdown(&self) -> Result<(), SystemError>;
}

/// Errors returned by platform control hooks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemError {
    /// Operation is not supported by the current backend.
    Unsupported,
    /// Operation failed unexpectedly.
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ColorMode {
    Auto,
    Always,
    Never,
}

impl ColorMode {
    fn uses_color(self) -> bool {
        matches!(self, ColorMode::Auto | ColorMode::Always)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LsOptions {
    show_hidden: bool,
    color_mode: ColorMode,
    help: bool,
    long: bool,
}

impl Default for LsOptions {
    fn default() -> Self {
        Self {
            show_hidden: false,
            color_mode: ColorMode::Auto,
            help: false,
            long: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BuiltinCommand {
    Shell,
    Help,
    History,
    Ls,
    Pwd,
    Cd,
    Cat,
    Echo,
    Touch,
    Mkdir,
    Rmdir,
    Rm,
    Cp,
    Mv,
    Reboot,
    Shutdown,
}

struct CommandBinary {
    builtin: BuiltinCommand,
}

enum CommandExecutable {
    Builtin(BuiltinCommand),
    Binary(CommandBinary),
}

enum CommandResolutionError {
    NotFound,
    InvalidFormat,
    Filesystem(FsError),
}

/// Shell-friendly filesystem error type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsError {
    /// Filesystem service not available yet.
    Unavailable,
    /// Requested path was not found.
    NotFound,
    /// Requested path exists but is not a directory.
    NotDirectory,
    /// Requested path exists but is not a regular file.
    NotFile,
    /// Filesystem image is corrupt or unreadable.
    Corrupt,
}

/// Directory entry returned by [`ShellFs`].
#[derive(Clone)]
pub struct DirEntry {
    /// UTF-8 filename without path separators.
    pub name: String,
    /// Entry kind.
    pub kind: EntryKind,
    /// File size in bytes.
    pub size: u64,
    /// Raw inode mode bits.
    pub mode: u16,
    /// Inode number associated with the entry.
    pub inode: u32,
}

impl<Io, Fs, Sys> BareShell<Io, Fs, Sys>
where
    Io: ShellIo,
    Fs: ShellFs,
    Sys: ShellSystem,
{
    /// Creates a new shell instance with the given IO backend.
    pub fn new(io: Io, fs: Fs, sys: Sys) -> Self {
        let mut shell = Self {
            keyboard: Keyboard::new(ScancodeSet1::new(), Us104Key, HandleControl::Ignore),
            input: String::new(),
            history: Vec::new(),
            current_dir: "/".to_string(),
            io,
            fs,
            sys,
        };
        shell.print_prompt();
        shell
    }

    /// Polls the underlying IO for pending scancodes and processes them.
    pub fn poll(&mut self) {
        while let Some(scancode) = self.io.next_scancode() {
            if let Ok(Some(event)) = self.keyboard.add_byte(scancode) {
                if let Some(decoded) = self.keyboard.process_keyevent(event) {
                    if let DecodedKey::Unicode(ch) = decoded {
                        self.handle_char(ch);
                    }
                }
            }
        }
    }

    fn handle_char(&mut self, c: char) {
        match c {
            '\n' => self.execute_command(),
            '\u{8}' | '\u{7f}' => {
                if !self.input.is_empty() {
                    self.input.pop();
                    self.print("\u{8} \u{8}");
                }
            }
            _ => {
                self.input.push(c);
                self.print_char(c);
            }
        }
    }

    fn execute_command(&mut self) {
        let command_line = core::mem::take(&mut self.input);
        self.print("\r\n");
        let trimmed = command_line.trim();
        if !trimmed.is_empty() {
            self.history.push(trimmed.to_string());
            let mut parts = trimmed.split_whitespace();
            if let Some(cmd) = parts.next() {
                let args: Vec<&str> = parts.collect();
                match self.resolve_command(cmd) {
                    Ok(CommandExecutable::Builtin(builtin)) => self.run_builtin(builtin, &args),
                    Ok(CommandExecutable::Binary(binary)) => self.run_binary(&binary, &args),
                    Err(CommandResolutionError::InvalidFormat) => {
                        self.print("unsupported executable format: ");
                        self.println(cmd);
                    }
                    Err(CommandResolutionError::Filesystem(FsError::Unavailable)) => {
                        self.println("filesystem unavailable")
                    }
                    Err(CommandResolutionError::Filesystem(FsError::Corrupt)) => {
                        self.println("filesystem corrupt")
                    }
                    Err(CommandResolutionError::Filesystem(_)) => self.println("command not found"),
                    Err(CommandResolutionError::NotFound) => self.println("command not found"),
                }
            }
        }
        self.print_prompt();
    }

    fn print_help(&mut self) {
        self.println("Built-in commands:");
        for (command, description) in HELP_ENTRIES {
            self.print("  ");
            self.print(command);
            self.print(": ");
            self.println(description);
        }
    }

    fn print_history(&mut self) {
        let entries: Vec<String> = self.history.iter().cloned().collect();
        for entry in entries {
            self.println(&entry);
        }
    }

    fn command_ls(&mut self, args: &[&str]) {
        let (options, paths) = match self.parse_ls_args(args) {
            Ok(result) => result,
            Err(()) => return,
        };

        if options.help {
            self.print_ls_help();
            return;
        }

        let mut targets = Vec::new();
        if paths.is_empty() {
            targets.push((String::from("."), self.current_dir.clone()));
        } else {
            for path in paths {
                let absolute = self.make_absolute_path(Some(path));
                targets.push((path.to_string(), absolute));
            }
        }

        let multiple = targets.len() > 1;
        let mut first_section = true;

        for (display, absolute) in targets {
            match self.fs.list_dir(&absolute) {
                Ok(entries) => {
                    if multiple {
                        if !first_section {
                            self.print("\r\n");
                        }
                        self.print(&display);
                        self.print(":\r\n");
                    }
                    first_section = false;
                    self.print_directory_entries(entries, options.color_mode, options.show_hidden);
                }
                Err(FsError::Unavailable) => self.println("ls: filesystem unavailable"),
                Err(FsError::NotFound) => {
                    self.print("ls: not found: ");
                    self.println(&display);
                }
                Err(FsError::NotDirectory | FsError::NotFile) => {
                    self.print("ls: not a directory: ");
                    self.println(&display);
                }
                Err(FsError::Corrupt) => self.println("ls: filesystem corrupt"),
            }
        }
    }

    fn command_pwd(&mut self) {
        let current = self.current_dir.clone();
        self.println(&current);
    }

    fn command_cd(&mut self, args: &[&str]) {
        if args.len() > 1 {
            self.println("cd: too many arguments");
            return;
        }
        let target = if let Some(first) = args.first() {
            self.make_absolute_path(Some(first))
        } else {
            "/".to_string()
        };
        match self.fs.list_dir(&target) {
            Ok(_) => {
                self.current_dir = target;
            }
            Err(FsError::Unavailable) => self.println("cd: filesystem unavailable"),
            Err(FsError::NotFound) => {
                self.print("cd: no such file or directory: ");
                if let Some(arg) = args.first() {
                    self.println(arg);
                } else {
                    self.println("/");
                }
            }
            Err(FsError::NotDirectory | FsError::NotFile) => {
                self.print("cd: not a directory: ");
                if let Some(arg) = args.first() {
                    self.println(arg);
                } else {
                    self.println("/");
                }
            }
            Err(FsError::Corrupt) => self.println("cd: filesystem corrupt"),
        }
    }

    fn command_cat(&mut self, args: &[&str]) {
        if args.is_empty() {
            self.println("cat: missing operand");
            return;
        }
        for &arg in args {
            let path = self.make_absolute_path(Some(arg));
            match self.fs.read_file(&path) {
                Ok(data) => self.print_file_bytes(&data),
                Err(FsError::Unavailable) => self.println("cat: filesystem unavailable"),
                Err(FsError::NotFound) => {
                    self.print("cat: no such file: ");
                    self.println(arg);
                }
                Err(FsError::NotDirectory) => {
                    self.print("cat: path is a directory: ");
                    self.println(arg);
                }
                Err(FsError::NotFile) => {
                    self.print("cat: path is not a regular file: ");
                    self.println(arg);
                }
                Err(FsError::Corrupt) => self.println("cat: filesystem corrupt"),
            }
        }
    }

    fn command_echo(&mut self, args: &[&str]) {
        if args.is_empty() {
            self.print("\r\n");
            return;
        }
        let mut first = true;
        for arg in args {
            if !first {
                self.print(" ");
            }
            first = false;
            self.print(arg);
        }
        self.print("\r\n");
    }

    fn command_read_only(&mut self, cmd: &str) {
        self.print(cmd);
        self.println(": filesystem is read-only");
    }

    fn command_sh(&mut self, args: &[&str]) {
        if !args.is_empty() {
            self.println("sh: arguments are not supported in the bare shell");
            return;
        }
        self.println("already running bare shell; nested shells are not implemented");
    }

    fn resolve_command(&mut self, name: &str) -> Result<CommandExecutable, CommandResolutionError> {
        let path = format!("/bin/{name}");
        match self.fs.read_file(&path) {
            Ok(data) => self
                .parse_command_binary(&data)
                .map(CommandExecutable::Binary),
            Err(FsError::NotFound) | Err(FsError::NotDirectory) | Err(FsError::NotFile) => {
                Self::builtin_from_str(name)
                    .map(CommandExecutable::Builtin)
                    .ok_or(CommandResolutionError::NotFound)
            }
            Err(FsError::Unavailable) => {
                Err(CommandResolutionError::Filesystem(FsError::Unavailable))
            }
            Err(FsError::Corrupt) => Err(CommandResolutionError::Filesystem(FsError::Corrupt)),
        }
    }

    fn parse_command_binary(&self, data: &[u8]) -> Result<CommandBinary, CommandResolutionError> {
        if data.len() >= 4 && &data[0..4] == b"\x7FELF" {
            return Self::parse_elf_command(data);
        }
        Self::parse_legacy_command(data)
    }

    fn parse_legacy_command(data: &[u8]) -> Result<CommandBinary, CommandResolutionError> {
        const COMMAND_MAGIC: [u8; 4] = [0x7F, b'B', b'C', b'M'];
        const COMMAND_VERSION: u8 = 1;
        const HEADER_SIZE: usize = 8;

        if data.len() < HEADER_SIZE {
            return Err(CommandResolutionError::InvalidFormat);
        }

        if data[0..4] != COMMAND_MAGIC {
            return Err(CommandResolutionError::InvalidFormat);
        }

        if data[4] != COMMAND_VERSION {
            return Err(CommandResolutionError::InvalidFormat);
        }

        let builtin_id = data[5];
        let name_len = u16::from_le_bytes([data[6], data[7]]) as usize;
        if data.len() < HEADER_SIZE + name_len {
            return Err(CommandResolutionError::InvalidFormat);
        }

        if name_len > 0 {
            if core::str::from_utf8(&data[HEADER_SIZE..HEADER_SIZE + name_len]).is_err() {
                return Err(CommandResolutionError::InvalidFormat);
            }
        }

        let builtin =
            Self::builtin_from_id(builtin_id).ok_or(CommandResolutionError::InvalidFormat)?;

        Ok(CommandBinary { builtin })
    }

    fn parse_elf_command(data: &[u8]) -> Result<CommandBinary, CommandResolutionError> {
        const ELF_HEADER_SIZE: usize = 64;
        const SHT_NOTE: u32 = 7;
        const COMMAND_NOTE_TYPE: u32 = 0x4D43_4221; // '!BCB' magic type
        const COMMAND_NOTE_MAGIC: u32 = 0x214D_4342; // '!BCM' descriptor magic

        if data.len() < ELF_HEADER_SIZE {
            return Err(CommandResolutionError::InvalidFormat);
        }

        if data[4] != 2 || data[5] != 1 || data[6] != 1 {
            return Err(CommandResolutionError::InvalidFormat);
        }

        let e_shoff = Self::read_u64(data, 40)? as usize;
        let e_shentsize = Self::read_u16(data, 58)? as usize;
        let e_shnum = Self::read_u16(data, 60)? as usize;
        let e_shstrndx = Self::read_u16(data, 62)? as usize;

        if e_shentsize == 0 || e_shnum == 0 {
            return Err(CommandResolutionError::InvalidFormat);
        }

        let sh_table_size = e_shentsize
            .checked_mul(e_shnum)
            .ok_or(CommandResolutionError::InvalidFormat)?;
        if e_shoff
            .checked_add(sh_table_size)
            .filter(|&end| end <= data.len())
            .is_none()
        {
            return Err(CommandResolutionError::InvalidFormat);
        }

        if e_shstrndx >= e_shnum {
            return Err(CommandResolutionError::InvalidFormat);
        }

        let shstr_header_offset = e_shoff + e_shstrndx * e_shentsize;
        let shstr_offset = Self::read_u64(data, shstr_header_offset + 24)? as usize;
        let shstr_size = Self::read_u64(data, shstr_header_offset + 32)? as usize;
        if shstr_offset
            .checked_add(shstr_size)
            .filter(|&end| end <= data.len())
            .is_none()
        {
            return Err(CommandResolutionError::InvalidFormat);
        }
        let shstrtab = &data[shstr_offset..shstr_offset + shstr_size];

        for index in 1..e_shnum {
            let header_offset = e_shoff + index * e_shentsize;
            let sh_type = Self::read_u32(data, header_offset + 4)?;
            if sh_type != SHT_NOTE {
                continue;
            }

            let name_offset = Self::read_u32(data, header_offset)? as usize;
            let section_name = Self::read_c_string(shstrtab, name_offset)
                .ok_or(CommandResolutionError::InvalidFormat)?;
            if section_name != ".note.bcm" {
                continue;
            }

            let note_offset = Self::read_u64(data, header_offset + 24)? as usize;
            let note_size = Self::read_u64(data, header_offset + 32)? as usize;
            if note_offset
                .checked_add(note_size)
                .filter(|&end| end <= data.len())
                .is_none()
            {
                return Err(CommandResolutionError::InvalidFormat);
            }
            let mut cursor = 0usize;
            let note = &data[note_offset..note_offset + note_size];
            while cursor + 12 <= note.len() {
                let namesz =
                    u32::from_le_bytes(note[cursor..cursor + 4].try_into().unwrap()) as usize;
                let descsz =
                    u32::from_le_bytes(note[cursor + 4..cursor + 8].try_into().unwrap()) as usize;
                let note_type =
                    u32::from_le_bytes(note[cursor + 8..cursor + 12].try_into().unwrap());
                cursor += 12;

                let name_end = Self::align_to(cursor + namesz, 4);
                if name_end > note.len() {
                    break;
                }
                let desc_start = name_end;
                let desc_end = Self::align_to(desc_start + descsz, 4);
                if desc_end > note.len() {
                    break;
                }

                let descriptor = &note[desc_start..desc_start + descsz];
                if note_type == COMMAND_NOTE_TYPE && descriptor.len() >= 12 {
                    let magic = u32::from_le_bytes(descriptor[0..4].try_into().unwrap());
                    let version = u32::from_le_bytes(descriptor[4..8].try_into().unwrap());
                    let command_id = u32::from_le_bytes(descriptor[8..12].try_into().unwrap());
                    if magic == COMMAND_NOTE_MAGIC && version == 1 {
                        if let Some(builtin) = Self::builtin_from_id(command_id as u8) {
                            return Ok(CommandBinary { builtin });
                        }
                        return Err(CommandResolutionError::InvalidFormat);
                    }
                }

                cursor = desc_end;
            }
        }

        Err(CommandResolutionError::InvalidFormat)
    }

    fn read_u16(data: &[u8], offset: usize) -> Result<u16, CommandResolutionError> {
        if offset + 2 > data.len() {
            return Err(CommandResolutionError::InvalidFormat);
        }
        Ok(u16::from_le_bytes([data[offset], data[offset + 1]]))
    }

    fn read_u32(data: &[u8], offset: usize) -> Result<u32, CommandResolutionError> {
        if offset + 4 > data.len() {
            return Err(CommandResolutionError::InvalidFormat);
        }
        Ok(u32::from_le_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ]))
    }

    fn read_u64(data: &[u8], offset: usize) -> Result<u64, CommandResolutionError> {
        if offset + 8 > data.len() {
            return Err(CommandResolutionError::InvalidFormat);
        }
        Ok(u64::from_le_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
            data[offset + 4],
            data[offset + 5],
            data[offset + 6],
            data[offset + 7],
        ]))
    }

    fn read_c_string<'a>(data: &'a [u8], offset: usize) -> Option<&'a str> {
        if offset >= data.len() {
            return None;
        }
        let end = data[offset..]
            .iter()
            .position(|&b| b == 0)
            .map(|idx| offset + idx)?;
        core::str::from_utf8(&data[offset..end]).ok()
    }

    fn align_to(value: usize, align: usize) -> usize {
        if align == 0 {
            return value;
        }
        (value + align - 1) & !(align - 1)
    }

    fn run_binary(&mut self, binary: &CommandBinary, args: &[&str]) {
        self.run_builtin(binary.builtin, args);
    }

    fn command_reboot(&mut self) {
        self.println("reboot: attempting reboot...");
        if let Err(err) = self.sys.reboot() {
            match err {
                SystemError::Unsupported => {
                    self.println("reboot: operation unsupported on this build")
                }
                SystemError::Failed => self.println("reboot: hardware did not respond"),
            }
        }
    }

    fn command_shutdown(&mut self) {
        self.println("shutdown: attempting power off...");
        if let Err(err) = self.sys.shutdown() {
            match err {
                SystemError::Unsupported => {
                    self.println("shutdown: operation unsupported on this build")
                }
                SystemError::Failed => self.println("shutdown: hardware did not respond"),
            }
        }
    }

    fn run_builtin(&mut self, builtin: BuiltinCommand, args: &[&str]) {
        match builtin {
            BuiltinCommand::Shell => self.command_sh(args),
            BuiltinCommand::Help => self.print_help(),
            BuiltinCommand::History => self.print_history(),
            BuiltinCommand::Ls => self.command_ls(args),
            BuiltinCommand::Pwd => self.command_pwd(),
            BuiltinCommand::Cd => self.command_cd(args),
            BuiltinCommand::Cat => self.command_cat(args),
            BuiltinCommand::Echo => self.command_echo(args),
            BuiltinCommand::Touch => self.command_read_only("touch"),
            BuiltinCommand::Mkdir => self.command_read_only("mkdir"),
            BuiltinCommand::Rmdir => self.command_read_only("rmdir"),
            BuiltinCommand::Rm => self.command_read_only("rm"),
            BuiltinCommand::Cp => self.command_read_only("cp"),
            BuiltinCommand::Mv => self.command_read_only("mv"),
            BuiltinCommand::Reboot => self.command_reboot(),
            BuiltinCommand::Shutdown => self.command_shutdown(),
        }
    }

    fn builtin_from_str(name: &str) -> Option<BuiltinCommand> {
        match name {
            "sh" => Some(BuiltinCommand::Shell),
            "help" => Some(BuiltinCommand::Help),
            "history" => Some(BuiltinCommand::History),
            "ls" => Some(BuiltinCommand::Ls),
            "pwd" => Some(BuiltinCommand::Pwd),
            "cd" => Some(BuiltinCommand::Cd),
            "cat" => Some(BuiltinCommand::Cat),
            "echo" => Some(BuiltinCommand::Echo),
            "touch" => Some(BuiltinCommand::Touch),
            "mkdir" => Some(BuiltinCommand::Mkdir),
            "rmdir" => Some(BuiltinCommand::Rmdir),
            "rm" => Some(BuiltinCommand::Rm),
            "cp" => Some(BuiltinCommand::Cp),
            "mv" => Some(BuiltinCommand::Mv),
            "reboot" => Some(BuiltinCommand::Reboot),
            "shutdown" => Some(BuiltinCommand::Shutdown),
            _ => None,
        }
    }

    fn builtin_from_id(id: u8) -> Option<BuiltinCommand> {
        match id {
            0 => Some(BuiltinCommand::Help),
            1 => Some(BuiltinCommand::History),
            2 => Some(BuiltinCommand::Ls),
            3 => Some(BuiltinCommand::Pwd),
            4 => Some(BuiltinCommand::Cd),
            5 => Some(BuiltinCommand::Cat),
            6 => Some(BuiltinCommand::Echo),
            7 => Some(BuiltinCommand::Touch),
            8 => Some(BuiltinCommand::Mkdir),
            9 => Some(BuiltinCommand::Rmdir),
            10 => Some(BuiltinCommand::Rm),
            11 => Some(BuiltinCommand::Cp),
            12 => Some(BuiltinCommand::Mv),
            13 => Some(BuiltinCommand::Reboot),
            14 => Some(BuiltinCommand::Shutdown),
            15 => Some(BuiltinCommand::Shell),
            _ => None,
        }
    }

    fn parse_ls_args<'a>(&mut self, args: &[&'a str]) -> Result<(LsOptions, Vec<&'a str>), ()> {
        let mut options = LsOptions::default();
        let mut paths = Vec::new();
        let mut end_of_options = false;

        for &arg in args {
            if !end_of_options && arg == "--" {
                end_of_options = true;
                continue;
            }

            if !end_of_options && arg.starts_with("--") {
                match arg {
                    "--help" => options.help = true,
                    "--all" | "--almost-all" => options.show_hidden = true,
                    "--color" => options.color_mode = ColorMode::Always,
                    _ => {
                        if let Some(value) = arg.strip_prefix("--color=") {
                            match value {
                                "auto" => options.color_mode = ColorMode::Auto,
                                "always" => options.color_mode = ColorMode::Always,
                                "never" => options.color_mode = ColorMode::Never,
                                _ => {
                                    self.print("ls: invalid value for --color: ");
                                    self.println(value);
                                    return Err(());
                                }
                            }
                        } else {
                            self.print("ls: unsupported option: ");
                            self.println(arg);
                            return Err(());
                        }
                    }
                }
                continue;
            }

            if !end_of_options && arg.starts_with('-') && arg.len() > 1 && arg != "-" {
                for ch in arg.chars().skip(1) {
                    match ch {
                        'a' | 'A' => options.show_hidden = true,
                        'h' => options.help = true,
                        _ => {
                            self.println(&format!("ls: unsupported flag -{ch}"));
                            return Err(());
                        }
                    }
                }
                continue;
            }

            paths.push(arg);
        }

        Ok((options, paths))
    }

    fn print_ls_help(&mut self) {
        self.println("用法: ls [选项]... [路径]");
        self.println("简化版 ls 支持以下选项:");
        self.println("  -a, --all           显示以 '.' 开头的文件");
        self.println("  -h, --help          显示本帮助信息");
        self.println("      --color[=WHEN]  启用彩色输出，WHEN=auto|always|never");
        self.println("");
        self.println("若未指定路径，则默认列出当前工作目录。");
    }

    fn print_directory_entries(
        &mut self,
        entries: Vec<DirEntry>,
        color_mode: ColorMode,
        show_hidden: bool,
    ) {
        let mut first = true;
        for entry in entries {
            if !show_hidden && entry.name.starts_with('.') {
                continue;
            }
            if !first {
                self.print("  ");
            }
            first = false;

            let mut display_name = entry.name.clone();
            if matches!(entry.kind, EntryKind::Directory) {
                display_name.push('/');
            }
            let colored = self.apply_color(&display_name, entry.kind, color_mode);
            self.print(&colored);
        }

        if !first {
            self.print("\r\n");
        }
    }

    fn apply_color(&self, name: &str, kind: EntryKind, mode: ColorMode) -> String {
        if !mode.uses_color() {
            return name.to_string();
        }

        match kind {
            EntryKind::Directory => format!("{COLOR_BLUE}{name}{COLOR_RESET}"),
            EntryKind::File => name.to_string(),
        }
    }

    fn make_absolute_path(&self, arg: Option<&str>) -> String {
        match arg {
            Some(path) => self.normalize_path(path),
            None => self.current_dir.clone(),
        }
    }

    fn normalize_path(&self, path: &str) -> String {
        let mut stack: Vec<String> = if path.starts_with('/') {
            Vec::new()
        } else if self.current_dir == "/" {
            Vec::new()
        } else {
            self.current_dir
                .trim_start_matches('/')
                .split('/')
                .filter(|component| !component.is_empty())
                .map(|component| component.to_string())
                .collect()
        };

        for component in path.split('/') {
            match component {
                "" | "." => {}
                ".." => {
                    stack.pop();
                }
                _ => stack.push(component.to_string()),
            }
        }

        if stack.is_empty() {
            "/".to_string()
        } else {
            let mut result = String::from("/");
            result.push_str(&stack.join("/"));
            result
        }
    }

    fn print_file_bytes(&mut self, data: &[u8]) {
        let rendered = String::from_utf8_lossy(data);
        for segment in rendered.split_inclusive('\n') {
            if let Some(stripped) = segment.strip_suffix('\n') {
                if !stripped.is_empty() {
                    self.print(stripped);
                }
                self.print("\r\n");
            } else {
                self.print(segment);
            }
        }
        if !data.ends_with(&[b'\n']) {
            self.print("\r\n");
        }
    }

    fn print(&mut self, msg: &str) {
        self.io.write_str(msg);
    }

    fn print_char(&mut self, c: char) {
        let mut buf = [0u8; 4];
        let slice = c.encode_utf8(&mut buf);
        self.print(slice);
    }

    fn println(&mut self, msg: &str) {
        self.print(msg);
        self.print("\r\n");
    }

    fn print_prompt(&mut self) {
        let current = self.current_dir.clone();
        self.print(PROMPT_PREFIX);
        self.print(":");
        self.print(&current);
        if current != "/" {
            self.print(" ");
        }
        self.print("> ");
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    Directory,
    File,
}
