//! Minimal POSIX-style userland runtime that serializes syscall intents.
//!
//! The real system would transmit the encoded payloads to the kernel via a
//! shared memory channel or direct syscall traps. Here we showcase the
//! message formats that future IPC layers can consume.

use serde::{Deserialize, Serialize};
use std::fmt::Write as _;

fn main() {
    println!("hybrid kernel userland runtime prototype");

    let runtime = Runtime::default();

    let fork_packet = runtime.invoke(SyscallRequest::Fork);
    println!("fork => {}", to_hex(&fork_packet));

    let exec_packet = runtime.invoke(SyscallRequest::Exec {
        path: "/bin/init".into(),
        argv: vec!["/bin/init".into(), "--shell".into()],
    });
    println!("exec => {}", to_hex(&exec_packet));

    let open_packet = runtime.invoke(SyscallRequest::Open {
        path: "/tmp/data".into(),
        flags: 0o644,
    });
    println!("open => {}", to_hex(&open_packet));

    let read_packet = runtime.invoke(SyscallRequest::Read { fd: 4, len: 128 });
    println!("read => {}", to_hex(&read_packet));

    let write_packet = runtime.invoke(SyscallRequest::Write {
        fd: 1,
        data: b"hello from userland\n".to_vec(),
    });
    println!("write => {}", to_hex(&write_packet));

    let exit_packet = runtime.invoke(SyscallRequest::Exit { status: 0 });
    println!("exit => {}", to_hex(&exit_packet));
}

#[derive(Debug, Serialize, Deserialize)]
enum SyscallRequest {
    Fork,
    Exec { path: String, argv: Vec<String> },
    Open { path: String, flags: u32 },
    Read { fd: u64, len: u64 },
    Write { fd: u64, data: Vec<u8> },
    Exit { status: i32 },
}

#[derive(Default)]
struct Runtime;

impl Runtime {
    fn invoke(&self, request: SyscallRequest) -> Vec<u8> {
        bincode::serialize(&request).expect("serialize syscall request")
    }
}

fn to_hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut output, "{:02x}", byte).expect("write hex");
    }
    output
}
