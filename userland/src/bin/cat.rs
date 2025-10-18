//! Prototype cat command that emits syscall request payloads for each input path.

use std::env;

use userland::syscall::{to_hex, Runtime, SyscallRequest};

const CHUNK: u64 = 4096;

fn main() {
    let mut args = env::args().skip(1).collect::<Vec<_>>();
    if args.is_empty() {
        args.push(String::from("-"));
    }

    let runtime = Runtime::default();

    for (index, path) in args.iter().enumerate() {
        if path == "-" {
            // Emit a read on stdin (fd 0) for pipelines.
            let bytes = runtime.invoke(SyscallRequest::Read { fd: 0, len: CHUNK });
            println!("{}", to_hex(&bytes));
            continue;
        }

        println!("# file {}", index + 1);
        let open_bytes = runtime.invoke(SyscallRequest::Open {
            path: path.clone(),
            flags: 0,
            mode: 0,
        });
        println!("{}", to_hex(&open_bytes));

        // Use fd=3 as the first user-allocated descriptor (matching kernel defaults).
        let read_bytes = runtime.invoke(SyscallRequest::Read { fd: 3, len: CHUNK });
        println!("{}", to_hex(&read_bytes));
    }
}
