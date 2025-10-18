//! Prototype `rm` command that emits unlink syscall payloads.

use std::env;

use userland::{print_requests, SyscallRequest};

fn main() {
    let mut args = env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("rm: missing operand");
        std::process::exit(1);
    };

    if args.next().is_some() {
        eprintln!("rm: multiple operands are not supported yet");
        std::process::exit(1);
    }

    let requests = [SyscallRequest::Unlink { path }];
    print_requests(requests);
}
