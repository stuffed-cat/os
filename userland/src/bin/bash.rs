//! A petite Bash-inspired shell for the hybrid kernel userland experiments.

use std::env;
use std::path::Path;
use std::process;

use userland::Shell;

fn main() {
    let mut shell = Shell::new();
    let args: Vec<String> = env::args().collect();

    let exit_code = match parse_mode(&args) {
        Mode::Interactive => shell.run_interactive(),
        Mode::Command(command) => match shell.run_command(&command) {
            Ok(code) => code,
            Err(err) => {
                eprintln!("bash: {err}");
                1
            }
        },
        Mode::Script(path) => match shell.run_script(&path) {
            Ok(code) => code,
            Err(err) => {
                eprintln!("bash: {err}");
                1
            }
        },
    };

    process::exit(exit_code);
}

enum Mode {
    Interactive,
    Command(String),
    Script(String),
}

fn parse_mode(args: &[String]) -> Mode {
    match args.len() {
        0 | 1 => Mode::Interactive,
        _ => match args[1].as_str() {
            "-c" if args.len() >= 3 => Mode::Command(args[2..].join(" ")),
            "-c" => Mode::Command(String::new()),
            path => {
                if Path::new(path).is_file() {
                    Mode::Script(path.into())
                } else {
                    // Fall back to running the argument list as a single command.
                    Mode::Command(args[1..].join(" "))
                }
            }
        },
    }
}
