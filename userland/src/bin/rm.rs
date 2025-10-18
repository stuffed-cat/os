//! Prototype `rm` command.

use std::env;

fn main() {
    let mut args = env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("rm: missing operand");
        std::process::exit(1);
    };

    if args.next().is_some() {
        eprintln!("rm: multiple operands are not supported in this prototype");
        std::process::exit(1);
    }

    println!("rm: would remove `{}` (syscall plumbing pending)", path);
}
