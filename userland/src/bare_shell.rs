//! Minimal shell support for bare-metal boot while the full shell is feature-gated to `std` builds.

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
pub struct BareShell<Io, Fs> {
    keyboard: Keyboard<Us104Key, ScancodeSet1>,
    input: String,
    history: Vec<String>,
    current_dir: String,
    io: Io,
    fs: Fs,
}

const PROMPT_PREFIX: &str = "bare shell";
const HELP_ENTRIES: &[(&str, &str)] = &[
    ("help", "Show this help message"),
    ("history", "Display previously executed commands"),
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
];

/// Filesystem abstraction exposed to the bare shell.
pub trait ShellFs {
    /// Lists directory entries for the provided absolute path.
    fn list_dir(&self, path: &str) -> Result<Vec<DirEntry>, FsError>;
    /// Reads a regular file from the provided absolute path.
    fn read_file(&self, path: &str) -> Result<Vec<u8>, FsError>;
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
}

impl<Io, Fs> BareShell<Io, Fs>
where
    Io: ShellIo,
    Fs: ShellFs,
{
    /// Creates a new shell instance with the given IO backend.
    pub fn new(io: Io, fs: Fs) -> Self {
        let mut shell = Self {
            keyboard: Keyboard::new(ScancodeSet1::new(), Us104Key, HandleControl::Ignore),
            input: String::new(),
            history: Vec::new(),
            current_dir: "/".to_string(),
            io,
            fs,
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
                match cmd {
                    "help" => self.print_help(),
                    "history" => self.print_history(),
                    "ls" => self.command_ls(&args),
                    "pwd" => self.command_pwd(),
                    "cd" => self.command_cd(&args),
                    "cat" => self.command_cat(&args),
                    "echo" => self.command_echo(&args),
                    "touch" => self.command_read_only("touch"),
                    "mkdir" => self.command_read_only("mkdir"),
                    "rmdir" => self.command_read_only("rmdir"),
                    "rm" => self.command_read_only("rm"),
                    "cp" => self.command_read_only("cp"),
                    "mv" => self.command_read_only("mv"),
                    _ => self.println("command not found"),
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
        let target = self.make_absolute_path(args.first().copied());
        let display = args.first().copied().unwrap_or(".");
        match self.fs.list_dir(&target) {
            Ok(entries) => {
                if entries.is_empty() {
                    return;
                }
                let mut first = true;
                for entry in entries {
                    if !first {
                        self.print("  ");
                    }
                    first = false;
                    self.print(&entry.name);
                    if matches!(entry.kind, EntryKind::Directory) {
                        self.print("/");
                    }
                }
                self.print("\r\n");
            }
            Err(FsError::Unavailable) => self.println("ls: filesystem unavailable"),
            Err(FsError::NotFound) => {
                self.print("ls: not found: ");
                self.println(display);
            }
            Err(FsError::NotDirectory | FsError::NotFile) => {
                self.print("ls: not a directory: ");
                self.println(display);
            }
            Err(FsError::Corrupt) => self.println("ls: filesystem corrupt"),
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
