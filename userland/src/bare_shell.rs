//! Minimal shell support for bare-metal boot while the full shell is feature-gated to `std` builds.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use pc_keyboard::layouts::Us104Key;
use pc_keyboard::{DecodedKey, HandleControl, Keyboard, ScancodeSet1};

mod commands;

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
    current_user: UserIdentity,
    hostname: String,
    io: Io,
    fs: Fs,
    sys: Sys,
}

const HOSTNAME_DEFAULT: &str = "nexa-os";
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
    ("chmod", "Update file permissions: chmod 644 path"),
    (
        "chown",
        "Change file owner: chown user[:group] path (root only)",
    ),
    ("whoami", "Print the active username"),
    ("id", "Show uid/gid and group membership"),
    ("users", "List registered users"),
    ("su", "Switch user: su name password"),
    (
        "useradd",
        "Create a user: useradd name password [--home PATH]",
    ),
    ("passwd", "Update password: passwd [name] newpassword"),
    ("setsid", "Run a command in a new session (placeholder)"),
    (
        "cttyhack",
        "Attach to controlling tty before running command (placeholder)",
    ),
    ("touch", "Create empty files"),
    ("mkdir", "Create directories"),
    ("rmdir", "Remove empty directories"),
    ("rm", "Remove files"),
    ("cp", "Copy a file"),
    ("mv", "Move or rename files"),
    ("reboot", "Reboot the system"),
    ("shutdown", "Power off the system"),
];

/// Filesystem abstraction exposed to the bare shell.
pub trait ShellFs {
    /// Lists directory entries for the provided absolute path.
    fn list_dir(&self, path: &str) -> Result<Vec<DirEntry>, FsError>;
    /// Reads a regular file from the provided absolute path.
    fn read_file(&self, path: &str) -> Result<Vec<u8>, FsError>;
    /// Creates a regular file with the requested permissions.
    fn create_file(&self, path: &str, mode: u16) -> Result<(), FsError>;
    /// Creates a directory with the requested permissions.
    fn create_dir(&self, path: &str, mode: u16) -> Result<(), FsError>;
    /// Removes a filesystem node.
    fn remove_file(&self, path: &str) -> Result<(), FsError>;
    /// Removes an empty directory at the provided absolute path.
    fn remove_dir(&self, path: &str) -> Result<(), FsError>;
    /// Writes bytes to a path at the given offset, optionally truncating first.
    fn write_file(
        &self,
        path: &str,
        offset: usize,
        data: &[u8],
        truncate: bool,
    ) -> Result<usize, FsError>;
    /// Updates an inode's permission bits.
    fn chmod(&self, path: &str, mode: u16) -> Result<(), FsError>;
    /// Updates an inode's owner/group identifiers.
    fn chown(&self, path: &str, uid: u32, gid: u32) -> Result<(), FsError>;
}

/// Platform control hooks exposed to the shell.
pub trait ShellSystem {
    /// Requests a system reboot.
    fn reboot(&self) -> Result<(), SystemError>;
    /// Requests a system shutdown/power off.
    fn shutdown(&self) -> Result<(), SystemError>;
    /// Launches an external executable with the provided arguments in the current working directory.
    fn exec(
        &self,
        path: &str,
        args: &[&str],
        cwd: &str,
        env: &[(&str, &str)],
    ) -> Result<ExecResult, SystemError>;
    /// Returns the current session user information.
    fn current_user(&self) -> UserIdentity;
    /// Authenticates a user by username and password.
    fn authenticate(&self, username: &str, password: &str) -> Result<UserIdentity, AuthError>;
    /// Updates the active session to the provided user profile.
    fn set_session(&self, user: &UserIdentity) -> Result<UserIdentity, AuthError>;
    /// Creates a new user account.
    fn create_user(
        &self,
        username: &str,
        password: &str,
        home: Option<&str>,
    ) -> Result<UserIdentity, UserAdminError>;
    /// Updates the password for an existing user.
    fn set_password(&self, username: &str, password: &str) -> Result<(), UserAdminError>;
    /// Returns all registered user identities.
    fn list_users(&self) -> Result<Vec<UserIdentity>, UserAdminError>;
    /// Looks up a single user by name.
    fn lookup_user(&self, username: &str) -> Option<UserIdentity>;
    /// Returns the hostname displayed by the prompt.
    fn hostname(&self) -> &str;
}

/// Errors returned by platform control hooks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemError {
    /// Operation is not supported by the current backend.
    Unsupported,
    /// Operation failed unexpectedly.
    Failed,
    /// Requested executable was not found.
    NotFound,
    /// Filesystem service is currently unavailable.
    FilesystemUnavailable,
    /// Access to the executable was denied.
    PermissionDenied,
    /// Executable image was malformed or unsupported.
    InvalidExecutable,
    /// Executable referenced an interpreter that was unavailable.
    MissingInterpreter,
}

/// Result returned from launching an external executable via [`ShellSystem::exec`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExecResult {
    /// Process identifier assigned to the spawned program.
    pub pid: u64,
}

/// Public identity information shared between kernel and shell subsystems.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserIdentity {
    /// Login/username.
    pub username: String,
    /// Primary numeric user identifier.
    pub uid: u32,
    /// Primary group identifier.
    pub gid: u32,
    /// Supplemental group identifiers.
    pub groups: Vec<u32>,
    /// Preferred home directory path.
    pub home: String,
    /// Preferred login shell path.
    pub shell: String,
}

/// Authentication outcomes returned by [`ShellSystem::authenticate`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthError {
    /// Referenced user does not exist.
    NotFound,
    /// Password mismatch.
    InvalidPassword,
    /// Authentication backend unavailable.
    Unsupported,
}

/// Errors surfaced when managing user accounts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserAdminError {
    /// Username already exists.
    AlreadyExists,
    /// User not found.
    NotFound,
    /// Password rejected (e.g., too short).
    InvalidPassword,
    /// Caller lacks required privileges.
    PermissionDenied,
    /// Operation not supported by backend.
    Unsupported,
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
    Chmod,
    Chown,
    Whoami,
    Id,
    Users,
    Su,
    UserAdd,
    Passwd,
    SetSid,
    CttyHack,
}

struct CommandBinary {
    builtin: BuiltinCommand,
}

enum CommandExecutable {
    Builtin(BuiltinCommand),
    External { path: String },
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
    /// Target already exists.
    AlreadyExists,
    /// Directory could not be removed because it still contains entries.
    DirectoryNotEmpty,
    /// Filesystem image is corrupt or unreadable.
    Corrupt,
    /// Operation denied due to insufficient permissions.
    PermissionDenied,
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
    /// Owning user identifier.
    pub uid: u32,
    /// Owning group identifier.
    pub gid: u32,
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
        let session = sys.current_user();
        let hostname = {
            let reported = sys.hostname();
            if reported.is_empty() {
                HOSTNAME_DEFAULT.to_string()
            } else {
                reported.to_string()
            }
        };
        let starting_dir = if session.home.is_empty() {
            String::from("/")
        } else {
            session.home.clone()
        };
        let mut shell = Self {
            keyboard: Keyboard::new(ScancodeSet1::new(), Us104Key, HandleControl::Ignore),
            input: String::new(),
            history: Vec::new(),
            current_dir: starting_dir,
            current_user: session,
            hostname,
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
                self.dispatch_command(cmd, &args);
            }
        }
        self.print_prompt();
    }

    fn dispatch_command(&mut self, cmd: &str, args: &[&str]) {
        match self.resolve_command(cmd) {
            Ok(CommandExecutable::Builtin(builtin)) => self.run_builtin(builtin, args),
            Ok(CommandExecutable::External { path }) => self.run_external(&path, args),
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
            Err(CommandResolutionError::Filesystem(FsError::PermissionDenied)) => {
                self.println("permission denied")
            }
            Err(CommandResolutionError::Filesystem(_)) => self.println("command not found"),
            Err(CommandResolutionError::NotFound) => self.println("command not found"),
        }
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
                    self.print_directory_entries(
                        entries,
                        options.color_mode,
                        options.show_hidden,
                        options.long,
                    );
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
                Err(FsError::PermissionDenied) => self.println("ls: permission denied"),
                Err(FsError::Corrupt) => self.println("ls: filesystem corrupt"),
                Err(_) => self.println("ls: filesystem error"),
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
            if self.current_user.home.is_empty() {
                "/".to_string()
            } else {
                self.current_user.home.clone()
            }
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
            Err(FsError::PermissionDenied) => self.println("cd: permission denied"),
            Err(FsError::Corrupt) => self.println("cd: filesystem corrupt"),
            Err(_) => self.println("cd: filesystem error"),
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
                Err(FsError::PermissionDenied) => self.println("cat: permission denied"),
                Err(FsError::Corrupt) => self.println("cat: filesystem corrupt"),
                Err(_) => self.println("cat: filesystem error"),
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

    fn command_touch(&mut self, args: &[&str]) {
        commands::touch(self, args);
    }

    fn command_mkdir(&mut self, args: &[&str]) {
        commands::mkdir(self, args);
    }

    fn command_rmdir(&mut self, args: &[&str]) {
        commands::rmdir(self, args);
    }

    fn command_rm(&mut self, args: &[&str]) {
        commands::rm(self, args);
    }

    fn command_cp(&mut self, args: &[&str]) {
        commands::cp(self, args);
    }

    fn command_mv(&mut self, args: &[&str]) {
        commands::mv(self, args);
    }

    fn command_chmod(&mut self, args: &[&str]) {
        if args.len() != 2 {
            self.println("usage: chmod MODE PATH");
            return;
        }
        let mode_str = args[0];
        let mode = match u16::from_str_radix(mode_str, 8) {
            Ok(value) => value,
            Err(_) => {
                self.print("chmod: invalid mode: ");
                self.println(mode_str);
                return;
            }
        };
        let path = self.make_absolute_path(Some(args[1]));
        match self.fs.chmod(&path, mode) {
            Ok(()) => {}
            Err(FsError::Unavailable) => self.println("chmod: filesystem unavailable"),
            Err(FsError::NotFound) => {
                self.print("chmod: no such file or directory: ");
                self.println(&path);
            }
            Err(FsError::NotDirectory) | Err(FsError::NotFile) => {
                self.print("chmod: invalid target: ");
                self.println(&path);
            }
            Err(FsError::PermissionDenied) => self.println("chmod: permission denied"),
            Err(FsError::Corrupt) => self.println("chmod: filesystem corrupt"),
            Err(FsError::AlreadyExists | FsError::DirectoryNotEmpty) => {}
        }
    }

    fn command_chown(&mut self, args: &[&str]) {
        if !self.require_root("chown") {
            return;
        }
        if args.len() != 2 {
            self.println("usage: chown USER[:GROUP] PATH");
            return;
        }
        let spec = args[0];
        let path = self.make_absolute_path(Some(args[1]));
        let (uid, gid) = match self.parse_owner_spec(spec) {
            Ok(pair) => pair,
            Err(message) => {
                self.print("chown: ");
                self.println(&message);
                return;
            }
        };

        match self.fs.chown(&path, uid, gid) {
            Ok(()) => {}
            Err(FsError::Unavailable) => self.println("chown: filesystem unavailable"),
            Err(FsError::NotFound) => {
                self.print("chown: no such file or directory: ");
                self.println(&path);
            }
            Err(FsError::NotDirectory) | Err(FsError::NotFile) => {
                self.print("chown: invalid target: ");
                self.println(&path);
            }
            Err(FsError::PermissionDenied) => self.println("chown: permission denied"),
            Err(FsError::Corrupt) => self.println("chown: filesystem corrupt"),
            Err(FsError::AlreadyExists | FsError::DirectoryNotEmpty) => {}
        }
    }

    fn command_whoami(&mut self) {
        let username = self.current_user.username.clone();
        self.println(&username);
    }

    fn command_id(&mut self) {
        let mut line = format!(
            "uid={}({}) gid={}({})",
            self.current_user.uid,
            self.current_user.username,
            self.current_user.gid,
            self.current_user.gid
        );
        if !self.current_user.groups.is_empty() {
            let groups = self
                .current_user
                .groups
                .iter()
                .map(|gid| gid.to_string())
                .collect::<Vec<String>>()
                .join(",");
            line.push_str(&format!(" groups={groups}"));
        }
        self.println(&line);
    }

    fn command_users(&mut self) {
        match self.sys.list_users() {
            Ok(users) => {
                let mut first = true;
                for user in users {
                    if !first {
                        self.print(" ");
                    }
                    first = false;
                    self.print(&user.username);
                }
                self.print("\r\n");
            }
            Err(UserAdminError::Unsupported) => self.println("users: user database not available"),
            Err(UserAdminError::PermissionDenied) => self.println("users: permission denied"),
            Err(_) => self.println("users: failed to retrieve user list"),
        }
    }

    fn command_su(&mut self, args: &[&str]) {
        if args.is_empty() || args.len() > 2 {
            self.println("usage: su USERNAME [PASSWORD]");
            return;
        }

        let username = args[0];
        let target_identity = if self.is_root() && args.len() == 1 {
            match self.sys.lookup_user(username) {
                Some(user) => user,
                None => {
                    self.print("su: unknown user ");
                    self.println(username);
                    return;
                }
            }
        } else {
            let Some(password) = args.get(1) else {
                self.println("su: password required");
                return;
            };
            match self.sys.authenticate(username, password) {
                Ok(user) => user,
                Err(AuthError::NotFound) => {
                    self.print("su: unknown user ");
                    self.println(username);
                    return;
                }
                Err(AuthError::InvalidPassword) => {
                    self.println("su: authentication failure");
                    return;
                }
                Err(AuthError::Unsupported) => {
                    self.println("su: authentication backend unavailable");
                    return;
                }
            }
        };

        match self.sys.set_session(&target_identity) {
            Ok(updated) => {
                self.current_user = updated;
                if !self.current_user.home.is_empty() {
                    self.current_dir = self.current_user.home.clone();
                }
                self.println(&format!("now logged in as {}", self.current_user.username));
            }
            Err(AuthError::Unsupported) => self.println("su: session switching unsupported"),
            Err(AuthError::NotFound) => self.println("su: user not found"),
            Err(AuthError::InvalidPassword) => self.println("su: authentication failure"),
        }
    }

    fn command_useradd(&mut self, args: &[&str]) {
        if !self.require_root("useradd") {
            return;
        }
        if args.len() < 2 {
            self.println("usage: useradd USER PASSWORD [--home PATH]");
            return;
        }

        let username = args[0];
        let password = args[1];
        let mut home: Option<String> = None;
        let mut idx = 2;
        while idx < args.len() {
            match args[idx] {
                "--home" => {
                    if idx + 1 >= args.len() {
                        self.println("useradd: missing argument for --home");
                        return;
                    }
                    let value = args[idx + 1];
                    let resolved = if value.starts_with('/') {
                        value.to_string()
                    } else {
                        self.make_absolute_path(Some(value))
                    };
                    home = Some(resolved);
                    idx += 2;
                }
                other => {
                    self.print("useradd: unknown option ");
                    self.println(other);
                    return;
                }
            }
        }

        match self.sys.create_user(username, password, home.as_deref()) {
            Ok(identity) => {
                self.println(&format!(
                    "user {} created (uid={})",
                    identity.username, identity.uid
                ));
            }
            Err(UserAdminError::AlreadyExists) => self.println("useradd: user already exists"),
            Err(UserAdminError::InvalidPassword) => self.println("useradd: password rejected"),
            Err(UserAdminError::PermissionDenied) => self.println("useradd: permission denied"),
            Err(UserAdminError::Unsupported) => self.println("useradd: operation unsupported"),
            Err(UserAdminError::NotFound) => self.println("useradd: backend missing prerequisite"),
        }
    }

    fn command_passwd(&mut self, args: &[&str]) {
        if args.is_empty() || args.len() > 2 {
            self.println("usage: passwd [USER] NEWPASSWORD");
            return;
        }

        let (username, new_password) = if args.len() == 1 {
            (self.current_user.username.as_str(), args[0])
        } else {
            let target = args[0];
            if !self.is_root() && target != self.current_user.username {
                self.println("passwd: permission denied");
                return;
            }
            (target, args[1])
        };

        match self.sys.set_password(username, new_password) {
            Ok(()) => self.println("password updated"),
            Err(UserAdminError::NotFound) => self.println("passwd: user not found"),
            Err(UserAdminError::InvalidPassword) => self.println("passwd: password rejected"),
            Err(UserAdminError::PermissionDenied) => self.println("passwd: permission denied"),
            Err(UserAdminError::Unsupported) => self.println("passwd: operation unsupported"),
            Err(UserAdminError::AlreadyExists) => {}
        }
    }

    fn command_setsid(&mut self, args: &[&str]) {
        if args.is_empty() {
            self.println("usage: setsid COMMAND [ARGS...]");
            return;
        }
        self.dispatch_command(args[0], &args[1..]);
    }

    fn command_cttyhack(&mut self, args: &[&str]) {
        if args.is_empty() {
            self.println("usage: cttyhack COMMAND [ARGS...]");
            return;
        }
        self.dispatch_command(args[0], &args[1..]);
    }

    fn parse_owner_spec(&mut self, spec: &str) -> Result<(u32, u32), String> {
        let (user_part, group_part) = match spec.split_once(':') {
            Some((user, group)) if !user.is_empty() => (user, Some(group)),
            _ => (spec, None),
        };

        let (uid, default_gid) = self.parse_user_token(user_part)?;
        let gid = match group_part {
            Some(group) if !group.is_empty() => self.parse_group_token(group)?,
            _ => default_gid.unwrap_or(uid),
        };
        Ok((uid, gid))
    }

    fn parse_user_token(&mut self, token: &str) -> Result<(u32, Option<u32>), String> {
        if let Ok(id) = token.parse::<u32>() {
            Ok((id, None))
        } else if let Some(user) = self.sys.lookup_user(token) {
            Ok((user.uid, Some(user.gid)))
        } else {
            Err(format!("unknown user '{token}'"))
        }
    }

    fn parse_group_token(&mut self, token: &str) -> Result<u32, String> {
        if let Ok(id) = token.parse::<u32>() {
            Ok(id)
        } else if let Some(user) = self.sys.lookup_user(token) {
            Ok(user.gid)
        } else {
            Err(format!("unknown group '{token}'"))
        }
    }

    fn require_root(&mut self, command: &str) -> bool {
        if self.is_root() {
            true
        } else {
            self.print(command);
            self.println(": permission denied (requires root)");
            false
        }
    }

    fn is_root(&self) -> bool {
        self.current_user.uid == 0
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
            Ok(data) => match self.parse_command_binary(&data) {
                Ok(binary) => Ok(CommandExecutable::Builtin(binary.builtin)),
                Err(CommandResolutionError::InvalidFormat) => {
                    if let Some(builtin) = Self::builtin_from_str(name) {
                        Ok(CommandExecutable::Builtin(builtin))
                    } else {
                        Ok(CommandExecutable::External { path })
                    }
                }
                Err(other) => Err(other),
            },
            Err(FsError::NotFound) | Err(FsError::NotDirectory) | Err(FsError::NotFile) => {
                if let Some(builtin) = Self::builtin_from_str(name) {
                    Ok(CommandExecutable::Builtin(builtin))
                } else {
                    Err(CommandResolutionError::NotFound)
                }
            }
            Err(FsError::Unavailable) => {
                Err(CommandResolutionError::Filesystem(FsError::Unavailable))
            }
            Err(FsError::PermissionDenied) => Err(CommandResolutionError::Filesystem(
                FsError::PermissionDenied,
            )),
            Err(FsError::Corrupt) => Err(CommandResolutionError::Filesystem(FsError::Corrupt)),
            Err(other) => Err(CommandResolutionError::Filesystem(other)),
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

    fn run_external(&mut self, path: &str, args: &[&str]) {
        let display = path.rsplit('/').next().unwrap_or(path);
        let mut owned_env: Vec<(String, String)> = Vec::new();
        if !self.history.is_empty() {
            owned_env.push(("SHELL_HISTORY".to_string(), self.history.join("\n")));
            owned_env.push((
                "SHELL_HISTORY_COUNT".to_string(),
                self.history.len().to_string(),
            ));
        }

        owned_env.push(("USER".to_string(), self.current_user.username.clone()));
        owned_env.push(("HOME".to_string(), self.current_user.home.clone()));
        owned_env.push(("UID".to_string(), self.current_user.uid.to_string()));
        owned_env.push(("GID".to_string(), self.current_user.gid.to_string()));
        owned_env.push(("HOSTNAME".to_string(), self.hostname.clone()));
        owned_env.push(("PWD".to_string(), self.current_dir.clone()));
        if !self.current_user.groups.is_empty() {
            let groups = self
                .current_user
                .groups
                .iter()
                .map(|gid| gid.to_string())
                .collect::<Vec<String>>()
                .join(",");
            owned_env.push(("GROUPS".to_string(), groups));
        }

        let env_pairs: Vec<(&str, &str)> = owned_env
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();

        match self
            .sys
            .exec(path, args, &self.current_dir, env_pairs.as_slice())
        {
            Ok(result) => {
                self.print("launched process pid ");
                self.println(&result.pid.to_string());
            }
            Err(SystemError::Unsupported) => {
                self.println("exec: operation not supported on this build")
            }
            Err(SystemError::FilesystemUnavailable) => self.println("exec: filesystem unavailable"),
            Err(SystemError::NotFound) => {
                self.print("exec: command not found: ");
                self.println(display);
            }
            Err(SystemError::MissingInterpreter) => {
                self.print("exec: interpreter not found for ");
                self.println(display);
            }
            Err(SystemError::PermissionDenied) => {
                self.print("exec: permission denied: ");
                self.println(display);
            }
            Err(SystemError::InvalidExecutable) => {
                self.print("exec: invalid executable: ");
                self.println(display);
            }
            Err(SystemError::Failed) => {
                self.print("exec: failed to launch ");
                self.println(display);
            }
        }
    }

    fn command_reboot(&mut self) {
        self.println("reboot: attempting reboot...");
        if let Err(err) = self.sys.reboot() {
            match err {
                SystemError::Unsupported => {
                    self.println("reboot: operation unsupported on this build")
                }
                SystemError::Failed => self.println("reboot: hardware did not respond"),
                _ => self.println("reboot: unexpected system error"),
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
                _ => self.println("shutdown: unexpected system error"),
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
            BuiltinCommand::Touch => self.command_touch(args),
            BuiltinCommand::Mkdir => self.command_mkdir(args),
            BuiltinCommand::Rmdir => self.command_rmdir(args),
            BuiltinCommand::Rm => self.command_rm(args),
            BuiltinCommand::Cp => self.command_cp(args),
            BuiltinCommand::Mv => self.command_mv(args),
            BuiltinCommand::Reboot => self.command_reboot(),
            BuiltinCommand::Shutdown => self.command_shutdown(),
            BuiltinCommand::Chmod => self.command_chmod(args),
            BuiltinCommand::Chown => self.command_chown(args),
            BuiltinCommand::Whoami => self.command_whoami(),
            BuiltinCommand::Id => self.command_id(),
            BuiltinCommand::Users => self.command_users(),
            BuiltinCommand::Su => self.command_su(args),
            BuiltinCommand::UserAdd => self.command_useradd(args),
            BuiltinCommand::Passwd => self.command_passwd(args),
            BuiltinCommand::SetSid => self.command_setsid(args),
            BuiltinCommand::CttyHack => self.command_cttyhack(args),
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
            "chmod" => Some(BuiltinCommand::Chmod),
            "chown" => Some(BuiltinCommand::Chown),
            "whoami" => Some(BuiltinCommand::Whoami),
            "id" => Some(BuiltinCommand::Id),
            "users" => Some(BuiltinCommand::Users),
            "su" => Some(BuiltinCommand::Su),
            "useradd" => Some(BuiltinCommand::UserAdd),
            "passwd" => Some(BuiltinCommand::Passwd),
            "setsid" => Some(BuiltinCommand::SetSid),
            "cttyhack" => Some(BuiltinCommand::CttyHack),
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
            16 => Some(BuiltinCommand::Chmod),
            17 => Some(BuiltinCommand::Chown),
            18 => Some(BuiltinCommand::Whoami),
            19 => Some(BuiltinCommand::Id),
            20 => Some(BuiltinCommand::Users),
            21 => Some(BuiltinCommand::Su),
            22 => Some(BuiltinCommand::UserAdd),
            23 => Some(BuiltinCommand::Passwd),
            24 => Some(BuiltinCommand::SetSid),
            25 => Some(BuiltinCommand::CttyHack),
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
                    "--long" => options.long = true,
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
                        'l' => options.long = true,
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
        self.println("  -l, --long          以长格式显示权限、所有者和大小");
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
        long_format: bool,
    ) {
        let mut printed_any = false;

        for entry in entries {
            if !show_hidden && entry.name.starts_with('.') {
                continue;
            }

            if long_format {
                if printed_any {
                    self.print("\r\n");
                }
                let mode_str = Self::format_mode_string(entry.kind, entry.mode);
                let uid = entry.uid;
                let gid = entry.gid;
                let size = entry.size;
                let mut display_name = entry.name.clone();
                if matches!(entry.kind, EntryKind::Directory) {
                    display_name.push('/');
                }
                let colored = self.apply_color(&display_name, entry.kind, color_mode);
                self.print(&format!(
                    "{} {:>5} {:>5} {:>10} {}",
                    mode_str, uid, gid, size, colored
                ));
            } else {
                if printed_any {
                    self.print("  ");
                }
                let mut display_name = entry.name.clone();
                if matches!(entry.kind, EntryKind::Directory) {
                    display_name.push('/');
                }
                let colored = self.apply_color(&display_name, entry.kind, color_mode);
                self.print(&colored);
            }

            printed_any = true;
        }

        if printed_any {
            self.print("\r\n");
        }
    }

    fn format_mode_string(kind: EntryKind, mode: u16) -> String {
        let mut result = String::with_capacity(10);
        result.push(match kind {
            EntryKind::Directory => 'd',
            EntryKind::File => '-',
        });
        result.push(Self::permission_char(mode, 0o400, 'r'));
        result.push(Self::permission_char(mode, 0o200, 'w'));
        result.push(Self::permission_char(mode, 0o100, 'x'));
        result.push(Self::permission_char(mode, 0o040, 'r'));
        result.push(Self::permission_char(mode, 0o020, 'w'));
        result.push(Self::permission_char(mode, 0o010, 'x'));
        result.push(Self::permission_char(mode, 0o004, 'r'));
        result.push(Self::permission_char(mode, 0o002, 'w'));
        result.push(Self::permission_char(mode, 0o001, 'x'));
        result
    }

    fn permission_char(mode: u16, bit: u16, ch: char) -> char {
        if (mode & bit) != 0 {
            ch
        } else {
            '-'
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
        let mut expanded = if path == "~" {
            if self.current_user.home.is_empty() {
                "/".to_string()
            } else {
                self.current_user.home.clone()
            }
        } else if let Some(rest) = path.strip_prefix("~/") {
            let mut base = if self.current_user.home.is_empty() {
                "/".to_string()
            } else {
                self.current_user.home.clone()
            };
            if !rest.is_empty() && !base.ends_with('/') {
                base.push('/');
            }
            base.push_str(rest);
            base
        } else {
            path.to_string()
        };

        if expanded.starts_with('~') {
            expanded = path.to_string();
        }

        let effective = expanded.as_str();

        let mut stack: Vec<String> = if effective.starts_with('/') {
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

        for component in effective.split('/') {
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
        let current = if self.current_dir.is_empty() {
            "/".to_string()
        } else {
            self.current_dir.clone()
        };
        let prompt = format!(
            "{}@{}:{}$ ",
            self.current_user.username, self.hostname, current
        );
        self.print(&prompt);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    Directory,
    File,
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::collections::{BTreeMap, BTreeSet};
    use alloc::rc::Rc;
    use core::cell::RefCell;

    #[derive(Default)]
    struct TestIo {
        output: String,
    }

    impl ShellIo for TestIo {
        fn next_scancode(&mut self) -> Option<u8> {
            None
        }

        fn write_str(&mut self, s: &str) {
            self.output.push_str(s);
        }
    }

    struct TestSystem;

    impl TestSystem {
        fn root_identity() -> UserIdentity {
            UserIdentity {
                username: "root".to_string(),
                uid: 0,
                gid: 0,
                groups: Vec::new(),
                home: "/".to_string(),
                shell: "/bin/sh".to_string(),
            }
        }
    }

    impl ShellSystem for TestSystem {
        fn reboot(&self) -> Result<(), SystemError> {
            Err(SystemError::Unsupported)
        }

        fn shutdown(&self) -> Result<(), SystemError> {
            Err(SystemError::Unsupported)
        }

        fn exec(
            &self,
            _path: &str,
            _args: &[&str],
            _cwd: &str,
            _env: &[(&str, &str)],
        ) -> Result<ExecResult, SystemError> {
            Err(SystemError::Unsupported)
        }

        fn current_user(&self) -> UserIdentity {
            Self::root_identity()
        }

        fn authenticate(&self, username: &str, password: &str) -> Result<UserIdentity, AuthError> {
            if username == "root" && password == "root" {
                Ok(Self::root_identity())
            } else {
                Err(AuthError::InvalidPassword)
            }
        }

        fn set_session(&self, user: &UserIdentity) -> Result<UserIdentity, AuthError> {
            Ok(user.clone())
        }

        fn create_user(
            &self,
            _username: &str,
            _password: &str,
            _home: Option<&str>,
        ) -> Result<UserIdentity, UserAdminError> {
            Err(UserAdminError::Unsupported)
        }

        fn set_password(&self, _username: &str, _password: &str) -> Result<(), UserAdminError> {
            Err(UserAdminError::Unsupported)
        }

        fn list_users(&self) -> Result<Vec<UserIdentity>, UserAdminError> {
            Ok(vec![Self::root_identity()])
        }

        fn lookup_user(&self, username: &str) -> Option<UserIdentity> {
            if username == "root" {
                Some(Self::root_identity())
            } else {
                None
            }
        }

        fn hostname(&self) -> &str {
            "test-host"
        }
    }

    #[derive(Clone)]
    struct TestFs {
        state: Rc<RefCell<TestFsState>>,
    }

    #[derive(Default)]
    struct TestFsState {
        dirs: BTreeSet<String>,
        files: BTreeMap<String, Vec<u8>>,
    }

    impl TestFs {
        fn new() -> Self {
            let mut dirs = BTreeSet::new();
            dirs.insert("/".to_string());
            Self {
                state: Rc::new(RefCell::new(TestFsState {
                    dirs,
                    files: BTreeMap::new(),
                })),
            }
        }

        fn add_dir(&self, path: &str) {
            self.state
                .borrow_mut()
                .dirs
                .insert(normalize_test_path(path));
        }

        fn add_file(&self, path: &str, data: Vec<u8>) {
            self.state
                .borrow_mut()
                .files
                .insert(normalize_test_path(path), data);
        }

        fn file_contents(&self, path: &str) -> Option<Vec<u8>> {
            self.state
                .borrow()
                .files
                .get(&normalize_test_path(path))
                .cloned()
        }

        fn has_dir(&self, path: &str) -> bool {
            self.state
                .borrow()
                .dirs
                .contains(&normalize_test_path(path))
        }
    }

    impl ShellFs for TestFs {
        fn list_dir(&self, path: &str) -> Result<Vec<DirEntry>, FsError> {
            let path = normalize_test_path(path);
            let state = self.state.borrow();
            if state.dirs.contains(&path) {
                let mut entries = Vec::new();
                for file_path in state.files.keys() {
                    if test_parent_of(file_path.as_str()) == Some(path.clone()) {
                        if let Some(name) = test_file_name(file_path) {
                            entries.push(DirEntry {
                                name,
                                kind: EntryKind::File,
                                size: state
                                    .files
                                    .get(file_path)
                                    .map(|data| data.len() as u64)
                                    .unwrap_or(0),
                                mode: 0o100644,
                                uid: 0,
                                gid: 0,
                                inode: 0,
                            });
                        }
                    }
                }
                for dir_path in state.dirs.iter() {
                    if dir_path == &path {
                        continue;
                    }
                    if test_parent_of(dir_path.as_str()) == Some(path.clone()) {
                        if let Some(name) = test_file_name(dir_path) {
                            entries.push(DirEntry {
                                name,
                                kind: EntryKind::Directory,
                                size: 0,
                                mode: 0o040755,
                                uid: 0,
                                gid: 0,
                                inode: 0,
                            });
                        }
                    }
                }
                Ok(entries)
            } else if state.files.contains_key(&path) {
                Err(FsError::NotDirectory)
            } else {
                Err(FsError::NotFound)
            }
        }

        fn read_file(&self, path: &str) -> Result<Vec<u8>, FsError> {
            let path = normalize_test_path(path);
            let state = self.state.borrow();
            if let Some(data) = state.files.get(&path) {
                Ok(data.clone())
            } else if state.dirs.contains(&path) {
                Err(FsError::NotFile)
            } else {
                Err(FsError::NotFound)
            }
        }

        fn create_file(&self, path: &str, _mode: u16) -> Result<(), FsError> {
            let path = normalize_test_path(path);
            let mut state = self.state.borrow_mut();
            if state.dirs.contains(&path) {
                return Err(FsError::NotFile);
            }
            if state.files.contains_key(&path) {
                return Err(FsError::AlreadyExists);
            }
            let Some(parent) = test_parent_of(&path) else {
                return Err(FsError::NotDirectory);
            };
            if !state.dirs.contains(&parent) {
                return Err(FsError::NotFound);
            }
            state.files.insert(path, Vec::new());
            Ok(())
        }

        fn create_dir(&self, path: &str, _mode: u16) -> Result<(), FsError> {
            let path = normalize_test_path(path);
            if path == "/" {
                return Err(FsError::AlreadyExists);
            }
            let mut state = self.state.borrow_mut();
            if state.dirs.contains(&path) {
                return Err(FsError::AlreadyExists);
            }
            if state.files.contains_key(&path) {
                return Err(FsError::NotFile);
            }
            let Some(parent) = test_parent_of(&path) else {
                return Err(FsError::NotDirectory);
            };
            if !state.dirs.contains(&parent) {
                return Err(FsError::NotFound);
            }
            state.dirs.insert(path);
            Ok(())
        }

        fn remove_file(&self, path: &str) -> Result<(), FsError> {
            let path = normalize_test_path(path);
            let mut state = self.state.borrow_mut();
            if state.files.remove(&path).is_some() {
                Ok(())
            } else if state.dirs.contains(&path) {
                Err(FsError::NotFile)
            } else {
                Err(FsError::NotFound)
            }
        }

        fn remove_dir(&self, path: &str) -> Result<(), FsError> {
            let path = normalize_test_path(path);
            if path == "/" {
                return Err(FsError::DirectoryNotEmpty);
            }
            let mut state = self.state.borrow_mut();
            if !state.dirs.contains(&path) {
                if state.files.contains_key(&path) {
                    return Err(FsError::NotDirectory);
                }
                return Err(FsError::NotFound);
            }

            let mut has_children = false;
            for file_path in state.files.keys() {
                if test_parent_of(file_path.as_str()) == Some(path.clone()) {
                    has_children = true;
                    break;
                }
            }
            if !has_children {
                for dir_path in state.dirs.iter() {
                    if dir_path == &path {
                        continue;
                    }
                    if test_parent_of(dir_path.as_str()) == Some(path.clone()) {
                        has_children = true;
                        break;
                    }
                }
            }

            if has_children {
                return Err(FsError::DirectoryNotEmpty);
            }

            state.dirs.remove(&path);
            Ok(())
        }

        fn write_file(
            &self,
            path: &str,
            offset: usize,
            data: &[u8],
            truncate: bool,
        ) -> Result<usize, FsError> {
            let path = normalize_test_path(path);
            let mut state = self.state.borrow_mut();
            if let Some(buffer) = state.files.get_mut(&path) {
                if truncate {
                    buffer.clear();
                }
                if offset > buffer.len() {
                    buffer.resize(offset, 0);
                }
                if offset + data.len() > buffer.len() {
                    buffer.resize(offset + data.len(), 0);
                }
                buffer[offset..offset + data.len()].copy_from_slice(data);
                Ok(data.len())
            } else if state.dirs.contains(&path) {
                Err(FsError::NotFile)
            } else {
                Err(FsError::NotFound)
            }
        }

        fn chmod(&self, _path: &str, _mode: u16) -> Result<(), FsError> {
            Ok(())
        }

        fn chown(&self, _path: &str, _uid: u32, _gid: u32) -> Result<(), FsError> {
            Ok(())
        }
    }

    fn normalize_test_path(path: &str) -> String {
        if path.is_empty() {
            return "/".to_string();
        }
        if !path.starts_with('/') {
            panic!("test paths must be absolute: {}", path);
        }
        let mut parts = Vec::new();
        for component in path.split('/') {
            if component.is_empty() || component == "." {
                continue;
            }
            if component == ".." {
                parts.pop();
            } else {
                parts.push(component);
            }
        }
        if parts.is_empty() {
            "/".to_string()
        } else {
            format!("/{}", parts.join("/"))
        }
    }

    fn test_parent_of(path: &str) -> Option<String> {
        if path == "/" {
            return None;
        }
        let normalized = normalize_test_path(path);
        if normalized == "/" {
            return None;
        }
        if let Some(pos) = normalized.rfind('/') {
            if pos == 0 {
                Some("/".to_string())
            } else {
                Some(normalized[..pos].to_string())
            }
        } else {
            Some("/".to_string())
        }
    }

    fn test_file_name(path: &str) -> Option<String> {
        let normalized = normalize_test_path(path);
        normalized
            .rsplit('/')
            .find(|component| !component.is_empty())
            .map(|component| component.to_string())
    }

    #[test]
    fn cp_copies_into_new_path() {
        let fs = TestFs::new();
        fs.add_file("/src.txt", b"hello".to_vec());

        let mut shell = BareShell::new(TestIo::default(), fs, TestSystem);
        shell.command_cp(&["/src.txt", "/copy.txt"]);

        assert_eq!(shell.fs.file_contents("/copy.txt"), Some(b"hello".to_vec()));
    }

    #[test]
    fn cp_copies_into_directory_destination() {
        let fs = TestFs::new();
        fs.add_dir("/dest");
        fs.add_file("/src.bin", b"payload".to_vec());

        let mut shell = BareShell::new(TestIo::default(), fs, TestSystem);
        shell.command_cp(&["/src.bin", "/dest"]);

        assert_eq!(
            shell.fs.file_contents("/dest/src.bin"),
            Some(b"payload".to_vec())
        );
    }

    #[test]
    fn touch_creates_files() {
        let fs = TestFs::new();
        let mut shell = BareShell::new(TestIo::default(), fs, TestSystem);

        shell.command_touch(&["/new.txt"]);

        assert_eq!(shell.fs.file_contents("/new.txt"), Some(Vec::new()));
    }

    #[test]
    fn mkdir_creates_directories() {
        let fs = TestFs::new();
        let mut shell = BareShell::new(TestIo::default(), fs, TestSystem);

        shell.command_mkdir(&["/newdir"]);

        assert!(shell.fs.has_dir("/newdir"));
    }

    #[test]
    fn rmdir_removes_empty_directories() {
        let fs = TestFs::new();
        let mut shell = BareShell::new(TestIo::default(), fs, TestSystem);
        shell.fs.add_dir("/toremove");

        shell.command_rmdir(&["/toremove"]);

        assert!(!shell.fs.has_dir("/toremove"));
    }

    #[test]
    fn rm_removes_files() {
        let fs = TestFs::new();
        let mut shell = BareShell::new(TestIo::default(), fs, TestSystem);
        shell.fs.add_file("/temp", b"data".to_vec());

        shell.command_rm(&["/temp"]);

        assert!(shell.fs.file_contents("/temp").is_none());
    }

    #[test]
    fn mv_moves_file() {
        let fs = TestFs::new();
        let mut shell = BareShell::new(TestIo::default(), fs, TestSystem);
        shell.fs.add_file("/src", b"payload".to_vec());

        shell.command_mv(&["/src", "/dest"]);

        assert_eq!(shell.fs.file_contents("/dest"), Some(b"payload".to_vec()));
        assert!(shell.fs.file_contents("/src").is_none());
    }
}
