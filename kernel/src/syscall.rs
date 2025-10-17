//! System call dispatch table bridging userland and kernel services.

use log::debug;

use crate::{
    error::{KernelError, SubsystemError},
    posix::{Errno, PosixLayer},
    process::{Pid, ProcessTable},
};

/// Identifiers for supported syscalls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SyscallId {
    /// POSIX `read` syscall.
    Read,
    /// POSIX `write` syscall.
    Write,
    /// POSIX `open` syscall.
    Open,
    /// POSIX `fork` syscall.
    Fork,
    /// POSIX `execve` syscall.
    Exec,
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
            2 => SyscallId::Open,
            57 => SyscallId::Fork,
            59 => SyscallId::Exec,
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
            Err(errno) => Err(map_errno(errno, id)),
        }
    }

    /// Returns the underlying POSIX layer for explicit operations.
    pub fn posix(&self) -> &PosixLayer<'a> {
        &self.posix
    }
}

fn map_errno(errno: Errno, id: SyscallId) -> KernelError {
    match errno {
        Errno::Success => KernelError::Unimplemented("unexpected success"),
        Errno::Perm => KernelError::Subsystem { id: "posix", source: SubsystemError::Runtime("permission denied") },
        Errno::NoEnt => KernelError::Subsystem { id: "posix", source: SubsystemError::Runtime("not found") },
        Errno::Intr => KernelError::Subsystem { id: "posix", source: SubsystemError::Runtime("interrupted") },
        Errno::Again => KernelError::Subsystem { id: "posix", source: SubsystemError::Runtime("try again") },
        Errno::NoMem => KernelError::Memory("ENOMEM"),
        Errno::Inval => KernelError::Subsystem { id: "posix", source: SubsystemError::Runtime("invalid argument") },
        Errno::NoImpl => match id {
            SyscallId::Unknown(_) => KernelError::Unimplemented("unknown syscall"),
            _ => KernelError::Unimplemented("syscall not implemented"),
        },
    }
}
