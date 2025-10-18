//! Prototype `cp` command.

use std::env;

fn main() {
    let mut args = env::args().skip(1);
    let Some(src) = args.next() else {
        eprintln!("cp: missing source operand");
        std::process::exit(1);
    };
    let Some(dest) = args.next() else {
        eprintln!("cp: missing destination operand");
        std::process::exit(1);
    };

    if args.next().is_some() {
        eprintln!("cp: multiple sources and options are not supported yet");
        std::process::exit(1);
    }

    println!(
        "cp: would open `{}` for reading and `{}` for writing (syscall wiring pending)",
        src, dest
    );
}
