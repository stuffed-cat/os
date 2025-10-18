//! Helpers shared across userland command binaries when running with the `std` feature.

#[cfg(feature = "std")]
use crate::syscall::{to_hex, Runtime, SyscallRequest};

/// POSIX-style open flag representing read-only access (matches `O_RDONLY`).
pub const O_RDONLY: u32 = 0;
/// POSIX-style open flag representing write-only access (matches `O_WRONLY`).
pub const O_WRONLY: u32 = 1;
/// POSIX-style open flag representing read/write access (matches `O_RDWR`).
pub const O_RDWR: u32 = 2;
/// Flag enabling file creation when combined with write access (matches `O_CREAT`).
pub const O_CREAT: u32 = 0o100;
/// Flag truncating a file upon opening (matches `O_TRUNC`).
pub const O_TRUNC: u32 = 0o1000;
/// Flag appending writes at the end of a file (matches `O_APPEND`).
pub const O_APPEND: u32 = 0o2000;

/// Serializes the provided syscall requests and prints them as hexadecimal payloads.
#[cfg(feature = "std")]
pub fn print_requests<I>(requests: I)
where
    I: IntoIterator<Item = SyscallRequest>,
{
    let runtime = Runtime::default();
    for request in requests {
        let bytes = runtime.invoke(request);
        println!("{}", to_hex(&bytes));
    }
}
