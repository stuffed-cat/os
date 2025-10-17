//! POSIX compatibility primitives enabling a Unix-like userland.

use alloc::string::String;
use alloc::vec::Vec;

use crate::{
    error::SubsystemError,
    process::{Pid, ProcessTable},
    syscall::SyscallId,
};

/// POSIX errno values we expose to userland.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Errno {
    /// Operation succeeded.
    Success = 0,
    /// Operation not permitted.
    Perm = 1,
    /// No such file or directory.
    NoEnt = 2,
    /// Interrupted function call.
    Intr = 4,
    /// Try again.
    Again = 11,
    /// Invalid argument.
    Inval = 22,
    /// Function not implemented.
    NoImpl = 38,
}

impl Errno {
    /// Converts to i32.
    pub fn as_raw(self) -> i32 {
        self as i32
    }
}

/// POSIX signal representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    /// Termination signal.
    Term = 15,
    /// Kill signal.
    Kill = 9,
    /// Interrupt signal.
    Int = 2,
}

/// POSIX compliance shim bridging syscalls and process table.
pub struct PosixLayer<'a> {
    process_table: &'a ProcessTable,
}

impl<'a> PosixLayer<'a> {
    /// Wraps the process table.
    pub fn new(process_table: &'a ProcessTable) -> Self {
        Self { process_table }
    }

    /// Dispatches a POSIX syscall.
    pub fn dispatch(&self, pid: Pid, syscall: SyscallId, args: &[u64]) -> Result<u64, Errno> {
        match syscall {
            SyscallId::Write => {
                let fd = args.get(0).copied().unwrap_or_default();
                let buf = args.get(1).copied().unwrap_or_default();
                let len = args.get(2).copied().unwrap_or_default();
                self.write(pid, fd, buf, len).map_err(|_| Errno::NoImpl)
            }
            SyscallId::Exit => {
                self.exit(pid, args.get(0).copied().unwrap_or_default() as i32);
                Ok(0)
            }
            _ => Err(Errno::NoImpl),
        }
    }

    fn write(&self, _pid: Pid, _fd: u64, _buf: u64, _len: u64) -> Result<u64, SubsystemError> {
        // We would copy from userland buffer and push to a pseudo TTY device service.
        Ok(0)
    }

    fn exit(&self, pid: Pid, status: i32) {
        if let Some(proc) = self.process_table.lookup(pid) {
            proc.set_exit_status(status);
        }
    }

    /// Normalizes POSIX path.
    pub fn normalize_path(path: &str) -> String {
        let mut parts = Vec::new();
        for part in path.split('/') {
            match part {
                "" | "." => {}
                ".." => {
                    parts.pop();
                }
                other => parts.push(other),
            }
        }
        format!("/{}", parts.join("/"))
    }
}
