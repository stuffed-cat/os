//! Prototype `mkdir` command that emits the syscall payload to create a directory.

use std::env;

use userland::{print_requests, SyscallRequest};

const DEFAULT_MODE: u32 = 0o755;

fn main() {
    let mut args = env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("mkdir: missing operand");
        std::process::exit(1);
    };

    if args.next().is_some() {
        eprintln!("mkdir: only single directory creation is supported");
        std::process::exit(1);
    }

    let requests = [SyscallRequest::Mkdir {
        path,
        mode: DEFAULT_MODE,
    }];

    print_requests(requests);
}
