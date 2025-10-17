//! POSIX compatibility primitives enabling a Unix-like userland.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use spin::RwLock;

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
    /// Not enough memory.
    NoMem = 12,
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
    program_handles: RwLock<BTreeMap<u64, String>>,
    path_handles: RwLock<BTreeMap<u64, String>>,
}

impl<'a> PosixLayer<'a> {
    /// Wraps the process table.
    pub fn new(process_table: &'a ProcessTable) -> Self {
        Self { process_table, program_handles: RwLock::new(BTreeMap::new()), path_handles: RwLock::new(BTreeMap::new()) }
    }

    /// Dispatches a POSIX syscall.
    pub fn dispatch(&self, pid: Pid, syscall: SyscallId, args: &[u64]) -> Result<u64, Errno> {
        match syscall {
            SyscallId::Fork => self.fork(pid).map(|child| child.as_u64()),
            SyscallId::Exec => {
                let handle = args.get(0).copied().unwrap_or_default();
                let program = self.lookup_program_handle(handle)?;
                self.exec(pid, program).map(|_| 0)
            }
            SyscallId::Write => {
                let fd = args.get(0).copied().unwrap_or_default();
                let buf = args.get(1).copied().unwrap_or_default();
                let len = args.get(2).copied().unwrap_or_default();
                self.write(pid, fd, buf, len).map_err(|_| Errno::NoImpl)
            }
            SyscallId::Read => {
                let fd = args.get(0).copied().unwrap_or_default();
                let len = args.get(1).copied().unwrap_or_default();
                self.read(pid, fd, len)
            }
            SyscallId::Open => {
                let handle = args.get(0).copied().unwrap_or_default();
                let flags = args.get(1).copied().unwrap_or_default();
                let path = self.lookup_path_handle(handle)?;
                self.open(pid, &path, flags)
            }
            SyscallId::Exit => {
                self.exit(pid, args.get(0).copied().unwrap_or_default() as i32);
                Ok(0)
            }
            _ => Err(Errno::NoImpl),
        }
    }

    /// Associates a numeric handle with a program path for exec calls.
    pub fn register_program_handle(&self, handle: u64, path: String) {
        self.program_handles.write().insert(handle, path);
    }

    /// Associates a numeric handle with a filesystem path for open.
    pub fn register_path_handle(&self, handle: u64, path: String) {
        self.path_handles.write().insert(handle, path);
    }

    fn write(&self, _pid: Pid, _fd: u64, _buf: u64, _len: u64) -> Result<u64, SubsystemError> {
        // We would copy from userland buffer and push to a pseudo TTY device service.
        Ok(0)
    }

    fn read(&self, pid: Pid, fd: u64, len: u64) -> Result<u64, Errno> {
        let proc = self.process_table.lookup(pid).ok_or(Errno::NoEnt)?;
        if proc.get_fd(fd).is_none() {
            return Err(Errno::Inval);
        }
        Ok(len)
    }

    fn open(&self, pid: Pid, path: &str, _flags: u64) -> Result<u64, Errno> {
        let normalized = Self::normalize_path(path);
        self
            .process_table
            .open(pid, normalized)
            .map_err(Errno::from_subsystem)
    }

    fn exit(&self, pid: Pid, status: i32) {
        if let Some(proc) = self.process_table.lookup(pid) {
            proc.set_exit_status(status);
        }
    }

    fn fork(&self, pid: Pid) -> Result<Pid, Errno> {
        self.process_table.fork(pid).map_err(Errno::from_subsystem)
    }

    fn exec(&self, pid: Pid, program: String) -> Result<(), Errno> {
        self.process_table.exec(pid, program).map_err(Errno::from_subsystem)
    }

    fn lookup_program_handle(&self, handle: u64) -> Result<String, Errno> {
        self.program_handles
            .read()
            .get(&handle)
            .cloned()
            .ok_or(Errno::NoEnt)
    }

    fn lookup_path_handle(&self, handle: u64) -> Result<String, Errno> {
        self.path_handles
            .read()
            .get(&handle)
            .cloned()
            .ok_or(Errno::NoEnt)
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

impl Errno {
    fn from_subsystem(err: SubsystemError) -> Self {
        match err {
            SubsystemError::Init(_) => Errno::NoImpl,
            SubsystemError::Runtime(_) => Errno::NoEnt,
            SubsystemError::Resource(_) => Errno::NoMem,
        }
    }
}
