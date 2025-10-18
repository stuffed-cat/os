//! Prototype `rmdir` command that serializes the directory removal syscall.

use std::env;

use userland::{print_requests, SyscallRequest};

fn main() {
    let mut args = env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("rmdir: missing operand");
        std::process::exit(1);
    };

    if args.next().is_some() {
        eprintln!("rmdir: only single directory removal is supported");
        std::process::exit(1);
    }

    let requests = [SyscallRequest::Rmdir { path }];
    print_requests(requests);
}
