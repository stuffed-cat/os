//! Minimal shell support for bare-metal boot while the full shell is feature-gated to `std` builds.

use alloc::string::String;
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

impl<Io: ShellIo> BareShell<Io> {
    /// Creates a new shell instance with the given IO backend.
    pub fn new(mut io: Io) -> Self {
        io.write_str("bare shell ready> ");
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
        let command = core::mem::take(&mut self.input);
        self.print("\r\n");
        if !command.trim().is_empty() {
            self.history.push(command.clone());
            match command.trim() {
                "help" => self.println("Commands: help, history"),
                "history" => {
                    let entries: Vec<String> = self.history.iter().cloned().collect();
                    for entry in entries {
                        self.println(&entry);
                    }
                }
                _ => self.println("command not found"),
            }
        }
        self.print("bare shell ready> ");
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
