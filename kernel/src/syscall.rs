//! System call dispatch table bridging userland and kernel services.

use log::debug;

use crate::{
    error::KernelError,
    posix::PosixLayer,
    process::{Pid, ProcessTable},
};

/// Identifiers for supported syscalls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SyscallId {
    /// POSIX `read` syscall placeholder.
    Read,
    /// POSIX `write` syscall.
    Write,
    /// POSIX `exit` syscall.
    Exit,
    /// Placeholder for future syscalls.
    Unknown(u64),
}

impl From<u64> for SyscallId {
    fn from(value: u64) -> Self {
        match value {
            0 => SyscallId::Read,
            1 => SyscallId::Write,
            60 => SyscallId::Exit,
            other => SyscallId::Unknown(other),
        }
    }
}

/// Dispatcher bridging userland traps to POSIX layer.
pub struct SyscallDispatcher<'a> {
    posix: PosixLayer<'a>,
}

impl<'a> SyscallDispatcher<'a> {
    /// Creates a dispatcher.
    pub fn new(process_table: &'a ProcessTable) -> Self {
        Self { posix: PosixLayer::new(process_table) }
    }

    /// Handles a syscall from a user process.
    pub fn handle(&self, pid: Pid, id: SyscallId, args: &[u64]) -> Result<u64, KernelError> {
        debug!("Syscall {:?} from {:?}", id, pid);
        match self.posix.dispatch(pid, id, args) {
            Ok(ret) => Ok(ret),
            Err(_errno) => Err(KernelError::Unimplemented(match id {
                SyscallId::Unknown(_) => "unknown syscall",
                _ => "syscall not implemented",
            })),
        }
    }
}
