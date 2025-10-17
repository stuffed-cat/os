//! Minimal POSIX-style userland runtime that serializes syscall intents.
//!
//! The real system would transmit the encoded payloads to the kernel via a
//! shared memory channel or direct syscall traps. Here we showcase the
//! message formats that future IPC layers can consume.

use userland::{to_hex, Runtime, SyscallRequest};

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

    let dup_packet = runtime.invoke(SyscallRequest::Dup { fd: 1 });
    println!("dup => {}", to_hex(&dup_packet));

    let pipe_packet = runtime.invoke(SyscallRequest::Pipe);
    println!("pipe => {}", to_hex(&pipe_packet));

    let c_string = b"libc-lite\0";
    let length = unsafe { libc_lite::strlen(c_string.as_ptr()) };
    println!("strlen(c string) => {length}");
}
