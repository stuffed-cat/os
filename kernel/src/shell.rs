//! Userland shell coordinator running inside the kernel event loop.

use crate::arch::x86_64::serial;
use crate::core::{KernelContext, Subsystem, SubsystemId};
use crate::error::SubsystemError;
use crate::fs::{self, EntryKind as FsEntryKind, FsError as KernelFsError};
use crate::memory::MemoryManager;
use crate::process::ProcessTable;
#[cfg(not(feature = "std"))]
use crate::scheduler::SchedulingClass;
use crate::scheduler::{RunQueueEntry, Scheduler, ThreadStatus};
use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::format;
use alloc::string::ToString;
use alloc::vec::Vec;
#[cfg(not(feature = "std"))]
use core::task::Poll;
use log::info;
#[cfg(not(feature = "std"))]
use log::warn;
use spin::Mutex;
use userland::{
    BareShell, DirEntry, EntryKind, ExecResult, FsError as ShellFsError, ShellFs, ShellIo,
    ShellSystem, SystemError,
};

const SCANCODE_QUEUE_CAPACITY: usize = 256;

static SCANCODE_QUEUE: Mutex<VecDeque<u8>> = Mutex::new(VecDeque::new());

/// Shell subsystem bridging keyboard interrupts and the userland shell loop.
pub struct ShellSubsystem {
    shell: BareShell<SerialShellIo, KernelShellFs, KernelShellSystem>,
    process_table: &'static ProcessTable,
    #[cfg_attr(feature = "std", allow(dead_code))]
    scheduler: &'static Scheduler,
}

impl ShellSubsystem {
    /// Creates a new shell subsystem instance.
    pub fn new() -> Self {
        let process_table = Box::leak(Box::new(ProcessTable::new()));
        let scheduler = Box::leak(Box::new(Scheduler::new()));
        let system = KernelShellSystem::new(process_table, scheduler);

        Self {
            shell: BareShell::new(SerialShellIo, KernelShellFs, system),
            process_table,
            scheduler,
        }
    }

    fn poll_shell(&mut self) {
        self.shell.poll();
    }

    #[cfg(not(feature = "std"))]
    fn run_ready_threads(&mut self) {
        while let Poll::Ready(entry) = self.scheduler.tick() {
            match entry.class {
                SchedulingClass::User => {
                    if let Some(process) = self.process_table.lookup(entry.pid) {
                        if let Some(thread) = process.thread_state(entry.tid) {
                            process.set_thread_status(entry.tid, ThreadStatus::Running);
                            unsafe {
                                thread.enter_user_mode();
                            }
                        } else {
                            warn!(
                                "scheduler: missing thread state for pid={} tid={}",
                                entry.pid.as_u64(),
                                entry.tid.as_u64()
                            );
                        }
                    } else {
                        warn!("scheduler: missing process for pid={}", entry.pid.as_u64());
                    }
                }
                SchedulingClass::Kernel => {
                    warn!(
                        "scheduler: kernel thread scheduling not implemented (pid={} tid={})",
                        entry.pid.as_u64(),
                        entry.tid.as_u64()
                    );
                }
            }
        }
    }
}

impl Subsystem for ShellSubsystem {
    fn id(&self) -> SubsystemId {
        SubsystemId("shell")
    }

    fn init(&mut self, ctx: &KernelContext) -> Result<(), SubsystemError> {
        if let Some(hal) = ctx.hal() {
            let manager: &'static MemoryManager =
                unsafe { &*(hal.memory() as *const MemoryManager) };
            self.process_table.bind_memory_manager(manager);
        }
        info!("userland shell initialized");
        Ok(())
    }

    fn tick(&mut self, _ctx: &KernelContext) -> Result<(), SubsystemError> {
        self.poll_shell();
        #[cfg(not(feature = "std"))]
        self.run_ready_threads();
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

#[derive(Clone, Copy)]
struct KernelShellSystem {
    process_table: &'static ProcessTable,
    scheduler: &'static Scheduler,
}

impl KernelShellSystem {
    fn new(process_table: &'static ProcessTable, scheduler: &'static Scheduler) -> Self {
        Self {
            process_table,
            scheduler,
        }
    }
}

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

    fn exec(
        &self,
        path: &str,
        args: &[&str],
        cwd: &str,
        env: &[(&str, &str)],
    ) -> Result<ExecResult, SystemError> {
        let process = self.process_table.spawn();
        let pid = process.pid();

        process.set_cwd(cwd.to_string());
        process.set_env("PWD".to_string(), cwd.to_string());

        let display = path.rsplit('/').next().unwrap_or(path);
        process.set_env("ARGV0".to_string(), display.to_string());
        process.set_env("ARGC".to_string(), (args.len() + 1).to_string());
        for (index, arg) in args.iter().enumerate() {
            process.set_env(format!("ARG{}", index + 1), (*arg).to_string());
        }

        for &(key, value) in env {
            process.set_env(key.to_string(), value.to_string());
        }

        let program = path.to_string();
        if let Err(err) = self.process_table.exec(pid, program) {
            return Err(map_exec_error(err));
        }

        let (tid, _) = process
            .main_thread()
            .ok_or(SystemError::InvalidExecutable)?;
        process.set_thread_status(tid, ThreadStatus::Ready);
        self.scheduler.enqueue(RunQueueEntry::user(pid, tid));

        Ok(ExecResult { pid: pid.as_u64() })
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

fn map_exec_error(err: SubsystemError) -> SystemError {
    match err {
        SubsystemError::Runtime(msg) => match msg {
            "executable not found" => SystemError::NotFound,
            "filesystem unavailable" => SystemError::FilesystemUnavailable,
            "permission denied" => SystemError::PermissionDenied,
            "invalid executable magic"
            | "unsupported elf class"
            | "unsupported elf endian"
            | "unsupported elf type"
            | "unsupported elf arch"
            | "corrupt program header"
            | "corrupt segment"
            | "executable truncated"
            | "executable missing segments"
            | "interpreter truncated"
            | "interpreter invalid magic"
            | "interpreter unsupported class"
            | "interpreter unsupported endian"
            | "interpreter unsupported type"
            | "interpreter unsupported arch"
            | "interpreter corrupt program header"
            | "interpreter corrupt segment"
            | "interpreter missing segments"
            | "interpreter recursion detected" => SystemError::InvalidExecutable,
            "interpreter not found" => SystemError::MissingInterpreter,
            "interpreter read failure" => SystemError::Failed,
            _ => SystemError::Failed,
        },
        SubsystemError::Init(_) | SubsystemError::Resource(_) => SystemError::Failed,
    }
}
