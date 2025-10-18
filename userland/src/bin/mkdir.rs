//! Prototype `mkdir` command.

use std::env;

fn main() {
    let mut args = env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("mkdir: missing operand");
        std::process::exit(1);
    };

    if args.next().is_some() {
        eprintln!("mkdir: only single directory creation is supported in this prototype");
        std::process::exit(1);
    }

    println!(
        "mkdir: would create directory `{}` (syscall plumbing pending)",
        path
    );
}
