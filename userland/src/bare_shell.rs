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
    io: Io,
    fs: Fs,
}

const PROMPT: &str = "bare shell ready> ";
const HELP_ENTRIES: &[(&str, &str)] = &[
    ("help", "Show this help message"),
    ("history", "Display previously executed commands"),
    ("ls", "List entries from the mounted filesystem"),
];

/// Filesystem abstraction exposed to the bare shell.
pub trait ShellFs {
    /// Lists directory entries for the provided absolute path.
    fn list_dir(&self, path: &str) -> Result<Vec<DirEntry>, FsError>;
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
    pub fn new(mut io: Io, fs: Fs) -> Self {
        io.write_str(PROMPT);
        Self {
            keyboard: Keyboard::new(ScancodeSet1::new(), Us104Key, HandleControl::Ignore),
            input: String::new(),
            history: Vec::new(),
            io,
            fs,
        }
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
                    _ => self.println("command not found"),
                }
            }
        }
        self.print(PROMPT);
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
        let path = args.first().copied().unwrap_or("/");
        match self.fs.list_dir(path) {
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
            Err(FsError::Unavailable) => {
                self.println("ls: filesystem unavailable");
            }
            Err(FsError::NotFound) => {
                self.print("ls: not found: ");
                self.println(path);
            }
            Err(FsError::NotDirectory) => {
                self.print("ls: not a directory: ");
                self.println(path);
            }
            Err(FsError::Corrupt) => {
                self.println("ls: filesystem corrupt");
            }
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    Directory,
    File,
}
