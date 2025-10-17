//! Minimal POSIX-style userland process stub.
//!
//! In the final system this binary would be compiled as an ELF application
//! that links against a C runtime shim and communicates with the kernel via
//! the syscall ABI defined in `kernel::syscall`. For now we simulate the
//! behavior in pure Rust to exercise architecture concepts at a high level.

use std::io::{self, Write};

fn main() {
    println!("os hybrid kernel userland prototype");

    // Simulate writing to stdout via a POSIX write syscall.
    if let Err(err) = write_stdout(b"hello from userland\n") {
        eprintln!("write failed: {err}");
    }
}

fn write_stdout(buf: &[u8]) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    stdout.write_all(buf)?;
    stdout.flush()
}