//! Prototype `cd` command that serializes a chdir syscall request.

use std::env;

use userland::syscall::{to_hex, Runtime, SyscallRequest};

fn main() {
    let mut args = env::args().skip(1);
    let target = args.next().unwrap_or_else(|| String::from("/"));
    if args.next().is_some() {
        eprintln!("cd: too many arguments");
        std::process::exit(1);
    }

    let runtime = Runtime::default();
    let request = SyscallRequest::Chdir { path: target };
    let bytes = runtime.invoke(request);
    println!("{}", to_hex(&bytes));
}
