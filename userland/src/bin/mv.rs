//! Prototype `mv` command that emits a rename syscall payload.

use std::env;

use userland::{print_requests, SyscallRequest};

fn main() {
    let mut args = env::args().skip(1);
    let Some(src) = args.next() else {
        eprintln!("mv: missing source operand");
        std::process::exit(1);
    };
    let Some(dest) = args.next() else {
        eprintln!("mv: missing destination operand");
        std::process::exit(1);
    };

    if args.next().is_some() {
        eprintln!("mv: multiple operands are not supported yet");
        std::process::exit(1);
    }

    let requests = [SyscallRequest::Rename {
        old_path: src,
        new_path: dest,
    }];

    print_requests(requests);
}
