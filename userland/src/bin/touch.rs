//! Prototype `touch` command.

use std::env;

fn main() {
    let mut args = env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("touch: missing file operand");
        std::process::exit(1);
    };

    if args.next().is_some() {
        eprintln!("touch: multiple operands are not supported yet");
        std::process::exit(1);
    }

    println!(
        "touch: would create or update `{}` (syscall wiring pending)",
        path
    );
}
