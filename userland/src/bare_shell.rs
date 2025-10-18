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
pub struct BareShell<Io> {
    keyboard: Keyboard<Us104Key, ScancodeSet1>,
    input: String,
    history: Vec<String>,
    io: Io,
}

const PROMPT: &str = "bare shell ready> ";
const HELP_ENTRIES: &[(&str, &str)] = &[
    ("help", "Show this help message"),
    ("history", "Display previously executed commands"),
    ("ls", "List built-in pseudo filesystem entries"),
];

const ROOT_ENTRIES: &[(&str, EntryKind)] = &[
    ("bin", EntryKind::Directory),
    ("dev", EntryKind::Directory),
    ("tmp", EntryKind::Directory),
    ("README", EntryKind::File),
];

impl<Io: ShellIo> BareShell<Io> {
    /// Creates a new shell instance with the given IO backend.
    pub fn new(mut io: Io) -> Self {
        io.write_str(PROMPT);
        Self {
            keyboard: Keyboard::new(ScancodeSet1::new(), Us104Key, HandleControl::Ignore),
            input: String::new(),
            history: Vec::new(),
            io,
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
        if let Some(arg) = args.first() {
            if *arg != "/" {
                self.print("ls: unsupported path: ");
                self.println(arg);
                return;
            }
        }

        let mut first = true;
        for (name, kind) in ROOT_ENTRIES {
            if !first {
                self.print("  ");
            }
            first = false;
            self.print(name);
            if matches!(kind, EntryKind::Directory) {
                self.print("/");
            }
        }

        if !ROOT_ENTRIES.is_empty() {
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
}

#[derive(Clone, Copy)]
enum EntryKind {
    Directory,
    File,
}
