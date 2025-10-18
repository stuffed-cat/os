//! Prototype `rmdir` command.

use std::env;

fn main() {
    let mut args = env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("rmdir: missing operand");
        std::process::exit(1);
    };

    if args.next().is_some() {
        eprintln!("rmdir: this prototype only supports removing a single directory");
        std::process::exit(1);
    }

    println!(
        "rmdir: would remove directory `{}` (syscall plumbing pending)",
        path
    );
}
