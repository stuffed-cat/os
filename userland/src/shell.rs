//! Simplified interactive shell inspired by early Bash releases.
//!
//! The goal is not to provide full POSIX compliance, but to establish a
//! userland component that can run basic commands, handle a few built-ins, and
//! serve as a convenient harness while the kernel-side infrastructure matures.

use std::env;
use std::fs::File;
use std::io::{self, BufRead, BufReader, IsTerminal, Write};
use std::path::Path;
use std::process::Command;

/// Minimal shell facade that keeps track of the prompt and last exit status.
#[derive(Debug)]
pub struct Shell {
    prompt: String,
    last_status: i32,
}

impl Default for Shell {
    fn default() -> Self {
        Self::new()
    }
}

impl Shell {
    /// Creates a shell instance using the default prompt (`"osh$ "`).
    pub fn new() -> Self {
        Self {
            prompt: "osh$ ".into(),
            last_status: 0,
        }
    }

    /// Creates a shell instance with a custom prompt string.
    pub fn with_prompt<S: Into<String>>(prompt: S) -> Self {
        Self {
            prompt: prompt.into(),
            last_status: 0,
        }
    }

    /// Returns the exit status of the most recently executed command.
    pub fn last_status(&self) -> i32 {
        self.last_status
    }

    /// Runs the shell interactively using stdin/stdout.
    pub fn run_interactive(&mut self) -> i32 {
        let stdin = io::stdin();
        let interactive = stdin.is_terminal();
        let mut handle = stdin.lock();
        let mut buffer = String::new();

        loop {
            buffer.clear();

            // Print prompt when stdin is a terminal (best effort—ignore errors).
            if interactive {
                print!("{}", self.prompt);
                let _ = io::stdout().flush();
            }

            match handle.read_line(&mut buffer) {
                Ok(0) => break, // EOF
                Ok(_) => {
                    let line = buffer.trim_end_matches(['\n', '\r']);
                    if let Err(err) = self.dispatch_line(line) {
                        eprintln!("bash: {err}");
                        self.last_status = 1;
                    }
                    if is_exit_trigger(self.last_status) {
                        break;
                    }
                }
                Err(err) => {
                    eprintln!("bash: failed to read line: {err}");
                    self.last_status = 1;
                    break;
                }
            }
        }

        let final_status = normalize_status(self.last_status);
        self.last_status = final_status;
        final_status
    }

    /// Executes a single command string, returning the resulting exit status.
    pub fn run_command(&mut self, line: &str) -> Result<i32, ShellError> {
        self.dispatch_line(line)?;
        let status = normalize_status(self.last_status);
        self.last_status = status;
        Ok(status)
    }

    /// Executes commands from a script file until completion or `exit`.
    pub fn run_script<P: AsRef<Path>>(&mut self, path: P) -> Result<i32, ShellError> {
        let file = File::open(path.as_ref())?;
        let mut reader = BufReader::new(file);
        let mut line = String::new();
        while {
            line.clear();
            reader.read_line(&mut line)?
        } > 0
        {
            let trimmed = line.trim_end_matches(['\n', '\r']);
            if let Err(err) = self.dispatch_line(trimmed) {
                self.last_status = 1;
                return Err(err);
            }
            if is_exit_trigger(self.last_status) {
                break;
            }
        }
        let status = normalize_status(self.last_status);
        self.last_status = status;
        Ok(status)
    }

    fn dispatch_line(&mut self, line: &str) -> Result<(), ShellError> {
        let tokens = parse_tokens(line)?;
        if tokens.is_empty() {
            return Ok(());
        }

        match tokens[0].as_str() {
            "#" => Ok(()),
            "cd" => match self.builtin_cd(&tokens[1..]) {
                Ok(status) => {
                    self.last_status = status;
                    Ok(())
                }
                Err(err) => {
                    self.last_status = 1;
                    Err(err)
                }
            },
            "pwd" => match self.builtin_pwd() {
                Ok(status) => {
                    self.last_status = status;
                    Ok(())
                }
                Err(err) => {
                    self.last_status = 1;
                    Err(err)
                }
            },
            "echo" => {
                self.last_status = self.builtin_echo(&tokens[1..]);
                Ok(())
            }
            "exit" => {
                self.last_status = self.builtin_exit(&tokens[1..]);
                Ok(())
            }
            "help" => {
                self.last_status = self.builtin_help();
                Ok(())
            }
            _ => match self.invoke_external(&tokens) {
                Ok(status) => {
                    self.last_status = status;
                    Ok(())
                }
                Err(err) => {
                    self.last_status = 1;
                    Err(err)
                }
            },
        }
    }

    fn builtin_cd(&self, args: &[String]) -> Result<i32, ShellError> {
        let target = if let Some(path) = args.first() {
            path.clone()
        } else {
            env::var("HOME").unwrap_or_else(|_| String::from("/"))
        };

        env::set_current_dir(&target).map_err(|err| ShellError::Builtin {
            name: "cd",
            message: format!("{}: {err}", target),
        })?;
        Ok(0)
    }

    fn builtin_pwd(&self) -> Result<i32, ShellError> {
        let cwd = env::current_dir().map_err(|err| ShellError::Builtin {
            name: "pwd",
            message: err.to_string(),
        })?;
        println!("{}", cwd.display());
        Ok(0)
    }

    fn builtin_echo(&self, args: &[String]) -> i32 {
        println!("{}", args.join(" "));
        0
    }

    fn builtin_exit(&self, args: &[String]) -> i32 {
        let code = args
            .first()
            .and_then(|raw| raw.parse::<i32>().ok())
            .unwrap_or(0);
        EXIT_SIGNAL | (code & 0xff)
    }

    fn builtin_help(&self) -> i32 {
        println!(
            "bash prototype built-ins:\n  cd [path]\n  pwd\n  echo [args...]\n  exit [code]\n  help"
        );
        0
    }

    fn invoke_external(&self, tokens: &[String]) -> Result<i32, ShellError> {
        let mut command = Command::new(&tokens[0]);
        if tokens.len() > 1 {
            command.args(&tokens[1..]);
        }

        match command.status() {
            Ok(status) => Ok(normalize_status(exit_status_to_code(status))),
            Err(err) => Err(ShellError::Command {
                program: tokens[0].clone(),
                source: err,
            }),
        }
    }
}

/// Sentinel bit used to signal an `exit` invocation internally.
const EXIT_SIGNAL: i32 = 1 << 30;

fn is_exit_trigger(status: i32) -> bool {
    status & EXIT_SIGNAL != 0
}

fn normalize_status(status: i32) -> i32 {
    status & 0xff
}

fn exit_status_to_code(status: std::process::ExitStatus) -> i32 {
    status.code().unwrap_or(128)
}

#[derive(Debug)]
pub enum ShellError {
    /// Line parsing failed.
    Parse(ParseError),
    /// Built-in execution failed.
    Builtin { name: &'static str, message: String },
    /// External command failed to start.
    Command { program: String, source: io::Error },
    /// Generic I/O failure (e.g., when reading scripts).
    Io(io::Error),
}

impl std::fmt::Display for ShellError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShellError::Parse(err) => write!(f, "{err}"),
            ShellError::Builtin { name, message } => {
                write!(f, "{name}: {message}")
            }
            ShellError::Command { program, source } => {
                write!(f, "{program}: {source}")
            }
            ShellError::Io(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for ShellError {}

impl From<io::Error> for ShellError {
    fn from(value: io::Error) -> Self {
        ShellError::Io(value)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum ParseError {
    /// Reached end-of-line while a quote was still open.
    UnclosedQuote,
    /// Line ended with a dangling escape (e.g. trailing backslash).
    DanglingEscape,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::UnclosedQuote => write!(f, "unterminated quoted string"),
            ParseError::DanglingEscape => write!(f, "dangling escape sequence"),
        }
    }
}

impl std::error::Error for ParseError {}

enum QuoteState {
    None,
    Single,
    Double,
}

fn parse_tokens(line: &str) -> Result<Vec<String>, ShellError> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut state = QuoteState::None;
    let mut escape = false;

    for ch in line.chars() {
        if escape {
            current.push(match ch {
                'n' => '\n',
                't' => '\t',
                other => other,
            });
            escape = false;
            continue;
        }

        match state {
            QuoteState::None => match ch {
                '\\' => {
                    escape = true;
                }
                '"' => {
                    state = QuoteState::Double;
                }
                '\'' => {
                    state = QuoteState::Single;
                }
                ch if ch.is_whitespace() => {
                    if !current.is_empty() {
                        tokens.push(current.clone());
                        current.clear();
                    }
                }
                '#' => {
                    if current.is_empty() {
                        break; // comment to end of line
                    } else {
                        current.push('#');
                    }
                }
                other => current.push(other),
            },
            QuoteState::Single => match ch {
                '\'' => {
                    state = QuoteState::None;
                }
                other => current.push(other),
            },
            QuoteState::Double => match ch {
                '"' => {
                    state = QuoteState::None;
                }
                '\\' => {
                    escape = true;
                }
                other => current.push(other),
            },
        }
    }

    if escape {
        return Err(ShellError::Parse(ParseError::DanglingEscape));
    }

    if !matches!(state, QuoteState::None) {
        return Err(ShellError::Parse(ParseError::UnclosedQuote));
    }

    if !current.is_empty() {
        tokens.push(current);
    }

    Ok(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_basic_tokens() {
        let tokens = parse_tokens("echo hello world").unwrap();
        assert_eq!(tokens, vec!["echo", "hello", "world"]);
    }

    #[test]
    fn parse_quotes_and_escapes() {
        let tokens = parse_tokens("echo \"hello world\" foo\\ bar").unwrap();
        assert_eq!(tokens, vec!["echo", "hello world", "foo bar"]);
    }

    #[test]
    fn parse_single_quotes() {
        let tokens = parse_tokens("echo 'a b c'").unwrap();
        assert_eq!(tokens, vec!["echo", "a b c"]);
    }

    #[test]
    fn parse_comments() {
        let tokens = parse_tokens("ls -l # list files").unwrap();
        assert_eq!(tokens, vec!["ls", "-l"]);
    }

    #[test]
    fn detect_unclosed_quote() {
        let err = parse_tokens("echo \"unterminated").unwrap_err();
        assert!(matches!(err, ShellError::Parse(ParseError::UnclosedQuote)));
    }

    #[test]
    fn detect_dangling_escape() {
        let err = parse_tokens("echo foo\\").unwrap_err();
        assert!(matches!(err, ShellError::Parse(ParseError::DanglingEscape)));
    }
}
