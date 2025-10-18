//! Prototype ls command that emits syscall request payloads.

use std::env;

use userland::syscall::{to_hex, Runtime, SyscallRequest};

const READ_SLICE: u64 = 4096;

fn main() {
    let mut args = env::args().skip(1);
    let target = args.next().unwrap_or_else(|| String::from("."));

    let runtime = Runtime::default();
    let mut sequence = Vec::new();

    sequence.push(SyscallRequest::Open {
        path: target.clone(),
        flags: 0,
        mode: 0,
    });
    sequence.push(SyscallRequest::Read {
        fd: 3,
        len: READ_SLICE,
    });
    for request in sequence {
        let bytes = runtime.invoke(request);
        println!("{}", to_hex(&bytes));
    }
}
