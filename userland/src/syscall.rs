//! Minimal POSIX-style syscall codec used by the prototype userland runtime.
//!
//! The real system is expected to encode syscall payloads and exchange them
//! with the kernel over a shared memory channel or similar IPC mechanism. The
//! utilities here let other binaries within the `userland` crate serialize such
//! payloads for experimentation.

use serde::{Deserialize, Serialize};
use std::fmt::Write as _;

/// Serializable representation of syscall intents used by the kernel IPC shim.
#[derive(Debug, Serialize, Deserialize)]
pub enum SyscallRequest {
    /// POSIX `fork` primitive.
    Fork,
    /// POSIX `execve` primitive.
    Exec { path: String, argv: Vec<String> },
    /// POSIX `open` primitive.
    Open { path: String, flags: u32 },
    /// POSIX `read` primitive.
    Read { fd: u64, len: u64 },
    /// POSIX `write` primitive.
    Write { fd: u64, data: Vec<u8> },
    /// POSIX `exit` primitive.
    Exit { status: i32 },
    /// POSIX `dup` primitive.
    Dup { fd: u64 },
    /// POSIX `dup2` primitive.
    Dup2 { fd: u64, new_fd: u64 },
    /// POSIX `pipe` primitive.
    Pipe,
    /// POSIX `chdir` primitive.
    Chdir { path: String },
    /// POSIX `getcwd` primitive.
    GetCwd,
    /// POSIX `sleep` primitive (milliseconds placeholder).
    Sleep { millis: u64 },
}

/// Runtime helper capable of turning high-level requests into binary payloads.
#[derive(Default)]
pub struct Runtime;

impl Runtime {
    /// Serializes the request into a byte buffer suitable for IPC.
    pub fn invoke(&self, request: SyscallRequest) -> Vec<u8> {
        bincode::serialize(&request).expect("serialize syscall request")
    }
}

/// Converts a byte slice to its hexadecimal representation for debug output.
pub fn to_hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut output, "{:02x}", byte).expect("write hex");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_and_decode_roundtrip() {
        let runtime = Runtime::default();
        let bytes = runtime.invoke(SyscallRequest::Exec {
            path: "/bin/init".into(),
            argv: vec!["/bin/init".into(), "--shell".into()],
        });

        let decoded: SyscallRequest = bincode::deserialize(&bytes).unwrap();
        match decoded {
            SyscallRequest::Exec { path, argv } => {
                assert_eq!(path, "/bin/init");
                assert_eq!(argv, vec!["/bin/init", "--shell"]);
            }
            other => panic!("unexpected variant: {other:?}"),
        }

        let pipe_bytes = runtime.invoke(SyscallRequest::Pipe);
        assert!(matches!(
            bincode::deserialize::<SyscallRequest>(&pipe_bytes),
            Ok(SyscallRequest::Pipe)
        ));
    }

    #[test]
    fn to_hex_formatting() {
        let bytes = [0xde, 0xad, 0xbe, 0xef];
        assert_eq!(to_hex(&bytes), "deadbeef");
    }
}
