//! Minimal `cttyhack` command placeholder that forwards execution to the requested program.

use std::env;
use std::process;

use userland::{print_requests, SyscallRequest};

fn main() {
    let mut args = env::args().skip(1).collect::<Vec<_>>();
    if args.is_empty() {
        eprintln!("usage: cttyhack COMMAND [ARGS...]");
        process::exit(1);
    }

    let path = args.remove(0);
    let mut argv = Vec::with_capacity(args.len() + 1);
    argv.push(path.clone());
    argv.append(&mut args);

    print_requests([SyscallRequest::Exec { path, argv }]);
}
