//! System call dispatch table bridging userland and kernel services.

use log::debug;
use spin::Once;

use crate::{
    error::{KernelError, SubsystemError},
    posix::{Errno, PosixLayer},
    process::{Pid, ProcessTable},
    user::TrapFrame,
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
    /// POSIX `close` syscall.
    Close,
    /// POSIX `fork` syscall.
    Fork,
    /// POSIX `execve` syscall.
    Exec,
    /// POSIX `exit` syscall.
    Exit,
    /// POSIX `waitpid` syscall.
    WaitPid,
    /// POSIX `getpid` syscall.
    GetPid,
    /// POSIX `pipe` syscall.
    Pipe,
    /// POSIX `dup` syscall.
    Dup,
    /// POSIX `dup2` syscall.
    Dup2,
    /// POSIX `chdir` syscall.
    Chdir,
    /// POSIX `getcwd` syscall.
    GetCwd,
    /// POSIX `nanosleep` (millisecond placeholder) syscall.
    Sleep,
    /// Placeholder for future syscalls.
    Unknown(u64),
}

impl From<u64> for SyscallId {
    fn from(value: u64) -> Self {
        match value {
            0 => SyscallId::Read,
            1 => SyscallId::Write,
            2 => SyscallId::Open,
            3 => SyscallId::Close,
            57 => SyscallId::Fork,
            59 => SyscallId::Exec,
            60 => SyscallId::Exit,
            61 => SyscallId::WaitPid,
            39 => SyscallId::GetPid,
            22 => SyscallId::Pipe,
            32 => SyscallId::Dup,
            33 => SyscallId::Dup2,
            80 => SyscallId::Chdir,
            79 => SyscallId::GetCwd,
            35 => SyscallId::Sleep,
            other => SyscallId::Unknown(other),
        }
    }
}

/// Dispatcher bridging userland traps to POSIX layer.
pub struct SyscallDispatcher<'a> {
    posix: PosixLayer<'a>,
}

static GLOBAL_DISPATCHER: Once<&'static SyscallDispatcher<'static>> = Once::new();

impl<'a> SyscallDispatcher<'a> {
    /// Creates a dispatcher.
    pub fn new(process_table: &'a ProcessTable) -> Self {
        Self {
            posix: PosixLayer::new(process_table),
        }
    }

    /// Handles a syscall from a user process.
    pub fn handle(&self, pid: Pid, id: SyscallId, args: &[u64]) -> Result<u64, KernelError> {
        debug!("Syscall {:?} from {:?}", id, pid);
        match self.posix.dispatch(pid, id, args) {
            Ok(ret) => Ok(ret),
            Err(errno) => Err(map_errno(errno, id)),
        }
    }

    /// Handles a syscall triggered through a trap frame, mirroring the x86-64 Linux ABI.
    pub fn handle_trap(&self, pid: Pid, frame: &mut TrapFrame) -> Result<(), KernelError> {
        let id = SyscallId::from(frame.rax);
        let args = [
            frame.rdi, frame.rsi, frame.rdx, frame.r10, frame.r8, frame.r9,
        ];
        let result = self.handle(pid, id, &args)?;
        frame.set_return_value(result);
        Ok(())
    }

    /// Returns the underlying POSIX layer for explicit operations.
    pub fn posix(&self) -> &PosixLayer<'a> {
        &self.posix
    }
}

impl SyscallDispatcher<'static> {
    /// Registers a global syscall dispatcher instance.
    pub fn register_global(&'static self) {
        GLOBAL_DISPATCHER.call_once(|| self);
    }

    /// Returns the registered global dispatcher, if any.
    pub fn global() -> Option<&'static SyscallDispatcher<'static>> {
        GLOBAL_DISPATCHER.get().copied()
    }
}

fn map_errno(errno: Errno, id: SyscallId) -> KernelError {
    match errno {
        Errno::Success => KernelError::Unimplemented("unexpected success"),
        Errno::Perm => KernelError::Subsystem {
            id: "posix",
            source: SubsystemError::Runtime("permission denied"),
        },
        Errno::NoEnt => KernelError::Subsystem {
            id: "posix",
            source: SubsystemError::Runtime("not found"),
        },
        Errno::Intr => KernelError::Subsystem {
            id: "posix",
            source: SubsystemError::Runtime("interrupted"),
        },
        Errno::Again => KernelError::Subsystem {
            id: "posix",
            source: SubsystemError::Runtime("try again"),
        },
        Errno::Badf => KernelError::Subsystem {
            id: "posix",
            source: SubsystemError::Runtime("bad file descriptor"),
        },
        Errno::Child => KernelError::Subsystem {
            id: "posix",
            source: SubsystemError::Runtime("no child processes"),
        },
        Errno::NoMem => KernelError::Memory("ENOMEM"),
        Errno::Inval => KernelError::Subsystem {
            id: "posix",
            source: SubsystemError::Runtime("invalid argument"),
        },
        Errno::NotDir => KernelError::Subsystem {
            id: "posix",
            source: SubsystemError::Runtime("not a directory"),
        },
        Errno::NoImpl => match id {
            SyscallId::Unknown(_) => KernelError::Unimplemented("unknown syscall"),
            _ => KernelError::Unimplemented("syscall not implemented"),
        },
    }
}
