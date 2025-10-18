//! Prototype `sh` command that serializes an `execve` request for `/bin/sh`.

use std::env;

use userland::{print_requests, SyscallRequest};

fn main() {
    let mut args = env::args().skip(1).collect::<Vec<_>>();
    let mut argv = Vec::with_capacity(args.len() + 1);
    argv.push(String::from("/bin/sh"));
    argv.append(&mut args);

    let requests = [SyscallRequest::Exec {
        path: String::from("/bin/sh"),
        argv,
    }];

    print_requests(requests);
}
