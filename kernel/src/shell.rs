//! Userland shell coordinator running inside the kernel event loop.

use crate::arch::x86_64::serial;
use crate::core::{KernelContext, Subsystem, SubsystemId};
use crate::error::SubsystemError;
use crate::fs::{self, EntryKind as FsEntryKind, FsError as KernelFsError};
use alloc::collections::VecDeque;
use alloc::vec::Vec;
use log::info;
use spin::Mutex;
use userland::{
    BareShell, DirEntry, EntryKind, FsError as ShellFsError, ShellFs, ShellIo, ShellSystem,
    SystemError,
};

const SCANCODE_QUEUE_CAPACITY: usize = 256;

static SCANCODE_QUEUE: Mutex<VecDeque<u8>> = Mutex::new(VecDeque::new());

/// Shell subsystem bridging keyboard interrupts and the userland shell loop.
pub struct ShellSubsystem {
    shell: BareShell<SerialShellIo, KernelShellFs, KernelShellSystem>,
}

impl ShellSubsystem {
    /// Creates a new shell subsystem instance.
    pub fn new() -> Self {
        Self {
            shell: BareShell::new(SerialShellIo, KernelShellFs, KernelShellSystem),
        }
    }

    fn poll_shell(&mut self) {
        self.shell.poll();
    }
}

impl Subsystem for ShellSubsystem {
    fn id(&self) -> SubsystemId {
        SubsystemId("shell")
    }

    fn init(&mut self, _ctx: &KernelContext) -> Result<(), SubsystemError> {
        info!("userland shell initialized");
        Ok(())
    }

    fn tick(&mut self, _ctx: &KernelContext) -> Result<(), SubsystemError> {
        self.poll_shell();
        Ok(())
    }
}

/// Serial-backed shell IO implementation.
struct SerialShellIo;

impl ShellIo for SerialShellIo {
    fn next_scancode(&mut self) -> Option<u8> {
        let mut queue = SCANCODE_QUEUE.lock();
        queue.pop_front()
    }

    fn write_str(&mut self, s: &str) {
        serial::write_str(s);
    }
}

struct KernelShellFs;

struct KernelShellSystem;

impl ShellFs for KernelShellFs {
    fn list_dir(&self, path: &str) -> Result<Vec<DirEntry>, ShellFsError> {
        match fs::list_dir(path) {
            Ok(entries) => Ok(entries
                .into_iter()
                .map(|entry| DirEntry {
                    name: entry.name,
                    kind: match entry.kind {
                        FsEntryKind::Directory => EntryKind::Directory,
                        FsEntryKind::File => EntryKind::File,
                    },
                    size: entry.size,
                    mode: entry.mode,
                    uid: entry.uid,
                    gid: entry.gid,
                    inode: entry.inode,
                })
                .collect()),
            Err(KernelFsError::NotInitialized | KernelFsError::Unsupported) => {
                Err(ShellFsError::Unavailable)
            }
            Err(KernelFsError::InvalidImage) => Err(ShellFsError::Corrupt),
            Err(KernelFsError::AlreadyExists) => Err(ShellFsError::Corrupt),
            Err(KernelFsError::NotFound) => Err(ShellFsError::NotFound),
            Err(KernelFsError::PermissionDenied) => Err(ShellFsError::PermissionDenied),
            Err(KernelFsError::NotDirectory) => Err(ShellFsError::NotDirectory),
            Err(KernelFsError::NotFile) => Err(ShellFsError::NotFile),
        }
    }

    fn read_file(&self, path: &str) -> Result<Vec<u8>, ShellFsError> {
        match fs::read_file(path) {
            Ok(data) => Ok(data),
            Err(KernelFsError::NotInitialized | KernelFsError::Unsupported) => {
                Err(ShellFsError::Unavailable)
            }
            Err(KernelFsError::InvalidImage) => Err(ShellFsError::Corrupt),
            Err(KernelFsError::AlreadyExists) => Err(ShellFsError::Corrupt),
            Err(KernelFsError::NotFound) => Err(ShellFsError::NotFound),
            Err(KernelFsError::PermissionDenied) => Err(ShellFsError::PermissionDenied),
            Err(KernelFsError::NotDirectory) => Err(ShellFsError::NotDirectory),
            Err(KernelFsError::NotFile) => Err(ShellFsError::NotFile),
        }
    }

    fn create_file(&self, path: &str, mode: u16) -> Result<(), ShellFsError> {
        match fs::create_file(path, mode) {
            Ok(_) => Ok(()),
            Err(KernelFsError::NotInitialized | KernelFsError::Unsupported) => {
                Err(ShellFsError::Unavailable)
            }
            Err(KernelFsError::InvalidImage) => Err(ShellFsError::Corrupt),
            Err(KernelFsError::AlreadyExists) => Err(ShellFsError::AlreadyExists),
            Err(KernelFsError::NotFound) => Err(ShellFsError::NotFound),
            Err(KernelFsError::PermissionDenied) => Err(ShellFsError::PermissionDenied),
            Err(KernelFsError::NotDirectory) => Err(ShellFsError::NotDirectory),
            Err(KernelFsError::NotFile) => Err(ShellFsError::NotFile),
        }
    }

    fn remove_file(&self, path: &str) -> Result<(), ShellFsError> {
        match fs::remove_file(path) {
            Ok(_) => Ok(()),
            Err(KernelFsError::NotInitialized | KernelFsError::Unsupported) => {
                Err(ShellFsError::Unavailable)
            }
            Err(KernelFsError::InvalidImage) => Err(ShellFsError::Corrupt),
            Err(KernelFsError::AlreadyExists) => Err(ShellFsError::Corrupt),
            Err(KernelFsError::NotFound) => Err(ShellFsError::NotFound),
            Err(KernelFsError::PermissionDenied) => Err(ShellFsError::PermissionDenied),
            Err(KernelFsError::NotDirectory) => Err(ShellFsError::NotDirectory),
            Err(KernelFsError::NotFile) => Err(ShellFsError::NotFile),
        }
    }

    fn write_file(
        &self,
        path: &str,
        offset: usize,
        data: &[u8],
        truncate: bool,
    ) -> Result<usize, ShellFsError> {
        match fs::write_file(path, offset, data, truncate) {
            Ok(written) => Ok(written),
            Err(KernelFsError::NotInitialized | KernelFsError::Unsupported) => {
                Err(ShellFsError::Unavailable)
            }
            Err(KernelFsError::InvalidImage) => Err(ShellFsError::Corrupt),
            Err(KernelFsError::AlreadyExists) => Err(ShellFsError::Corrupt),
            Err(KernelFsError::NotFound) => Err(ShellFsError::NotFound),
            Err(KernelFsError::PermissionDenied) => Err(ShellFsError::PermissionDenied),
            Err(KernelFsError::NotDirectory) => Err(ShellFsError::NotDirectory),
            Err(KernelFsError::NotFile) => Err(ShellFsError::NotFile),
        }
    }

    fn chmod(&self, path: &str, mode: u16) -> Result<(), ShellFsError> {
        match fs::chmod(path, mode) {
            Ok(_) => Ok(()),
            Err(KernelFsError::NotInitialized | KernelFsError::Unsupported) => {
                Err(ShellFsError::Unavailable)
            }
            Err(KernelFsError::InvalidImage) => Err(ShellFsError::Corrupt),
            Err(KernelFsError::AlreadyExists) => Err(ShellFsError::Corrupt),
            Err(KernelFsError::NotFound) => Err(ShellFsError::NotFound),
            Err(KernelFsError::PermissionDenied) => Err(ShellFsError::PermissionDenied),
            Err(KernelFsError::NotDirectory) => Err(ShellFsError::NotDirectory),
            Err(KernelFsError::NotFile) => Err(ShellFsError::NotFile),
        }
    }

    fn chown(&self, path: &str, uid: u32, gid: u32) -> Result<(), ShellFsError> {
        match fs::chown(path, uid, gid) {
            Ok(_) => Ok(()),
            Err(KernelFsError::NotInitialized | KernelFsError::Unsupported) => {
                Err(ShellFsError::Unavailable)
            }
            Err(KernelFsError::InvalidImage) => Err(ShellFsError::Corrupt),
            Err(KernelFsError::AlreadyExists) => Err(ShellFsError::Corrupt),
            Err(KernelFsError::NotFound) => Err(ShellFsError::NotFound),
            Err(KernelFsError::PermissionDenied) => Err(ShellFsError::PermissionDenied),
            Err(KernelFsError::NotDirectory) => Err(ShellFsError::NotDirectory),
            Err(KernelFsError::NotFile) => Err(ShellFsError::NotFile),
        }
    }
}

impl ShellSystem for KernelShellSystem {
    fn reboot(&self) -> Result<(), SystemError> {
        #[cfg(feature = "hardware")]
        {
            crate::arch::x86_64::power::reboot();
            return Ok(());
        }

        #[cfg(not(feature = "hardware"))]
        {
            Err(SystemError::Unsupported)
        }
    }

    fn shutdown(&self) -> Result<(), SystemError> {
        #[cfg(feature = "hardware")]
        {
            crate::arch::x86_64::power::shutdown();
            return Ok(());
        }

        #[cfg(not(feature = "hardware"))]
        {
            Err(SystemError::Unsupported)
        }
    }
}

/// Enqueues a raw keyboard scancode for shell processing.
pub fn enqueue_scancode(scancode: u8) {
    let mut queue = SCANCODE_QUEUE.lock();
    if queue.len() >= SCANCODE_QUEUE_CAPACITY {
        queue.pop_front();
    }
    queue.push_back(scancode);
}
