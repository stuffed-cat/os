//! Process management and hybrid capability tracking.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, Ordering};
use spin::RwLock;
use x86_64::structures::paging::PhysFrame;

use crate::{
    elf::{ElfError, ExecutableImage},
    error::SubsystemError,
    fs::{self, Credentials, FsError},
    memory::{Capability, MemoryManager, PageTableHandle},
    scheduler::{ThreadState, ThreadStatus},
    user::{AddressSpace, StackConfig, UserContext},
};
use log::trace;

/// Process identifier type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Pid(u64);

impl Pid {
    /// Creates a PID from raw value.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns raw PID value.
    pub const fn as_u64(self) -> u64 {
        self.0
    }
}

/// Thread identifier type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Tid(u64);

impl Tid {
    /// Creates a thread identifier from raw value.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the raw TID value.
    pub const fn as_u64(self) -> u64 {
        self.0
    }
}

/// Process control block with capability list.
pub struct Process {
    pid: Pid,
    threads: RwLock<BTreeMap<Tid, ThreadState>>, // monolithic fast path for scheduling
    capabilities: RwLock<Vec<Capability>>,       // microkernel style capability list
    exit_status: AtomicI32,
    terminated: AtomicBool,
    parent: RwLock<Option<Pid>>,
    program: RwLock<Option<String>>,
    executable: RwLock<Option<ExecutableImage>>,
    address_space: RwLock<Option<AddressSpace>>,
    page_table: RwLock<Option<PageTableHandle>>,
    user_context: RwLock<Option<UserContext>>,
    fds: RwLock<BTreeMap<u64, String>>,
    fd_offsets: RwLock<BTreeMap<u64, usize>>,
    next_fd: AtomicU64,
    cwd: RwLock<String>,
    env: RwLock<BTreeMap<String, String>>,
    pipe_seed: AtomicU64,
    uid: AtomicU32,
    gid: AtomicU32,
    groups: RwLock<Vec<u32>>,
    next_tid: AtomicU64,
}

impl Process {
    /// Allocates a new process.
    pub fn new(pid: Pid) -> Arc<Self> {
        let process = Arc::new(Self {
            pid,
            threads: RwLock::new(BTreeMap::new()),
            capabilities: RwLock::new(Vec::new()),
            exit_status: AtomicI32::new(0),
            terminated: AtomicBool::new(false),
            parent: RwLock::new(None),
            program: RwLock::new(None),
            executable: RwLock::new(None),
            address_space: RwLock::new(None),
            page_table: RwLock::new(None),
            user_context: RwLock::new(None),
            fds: RwLock::new(BTreeMap::new()),
            fd_offsets: RwLock::new(BTreeMap::new()),
            next_fd: AtomicU64::new(3),
            cwd: RwLock::new(String::from("/")),
            env: RwLock::new(BTreeMap::new()),
            pipe_seed: AtomicU64::new(1),
            uid: AtomicU32::new(0),
            gid: AtomicU32::new(0),
            groups: RwLock::new(vec![0]),
            next_tid: AtomicU64::new(1),
        });

        process.set_fd(0, String::from("tty:stdin"));
        process.set_fd(1, String::from("tty:stdout"));
        process.set_fd(2, String::from("tty:stderr"));

        process
    }

    /// Returns the process identifier.
    pub fn pid(&self) -> Pid {
        self.pid
    }

    /// Stores exit status.
    pub fn set_exit_status(&self, status: i32) {
        self.exit_status.store(status, Ordering::SeqCst);
    }

    /// Returns the stored exit status.
    pub fn exit_status(&self) -> i32 {
        self.exit_status.load(Ordering::SeqCst)
    }

    /// Marks the process as terminated.
    pub fn mark_terminated(&self, status: i32) {
        self.exit_status.store(status, Ordering::SeqCst);
        self.terminated.store(true, Ordering::SeqCst);
        let mut threads = self.threads.write();
        for state in threads.values_mut() {
            state.set_status(ThreadStatus::Dead);
        }
    }

    /// Returns whether the process has terminated.
    pub fn is_terminated(&self) -> bool {
        self.terminated.load(Ordering::SeqCst)
    }

    /// Registers a capability token.
    pub fn add_capability(&self, cap: Capability) {
        self.capabilities.write().push(cap);
    }

    /// Adds a thread to the process.
    pub fn add_thread(&self, tid: Tid, state: ThreadState) {
        self.threads.write().insert(tid, state);
    }

    /// Returns a snapshot of the thread state for the provided TID.
    pub fn thread_state(&self, tid: Tid) -> Option<ThreadState> {
        self.threads.read().get(&tid).cloned()
    }

    /// Updates the lifecycle status for the given thread, returning whether it existed.
    pub fn set_thread_status(&self, tid: Tid, status: ThreadStatus) -> bool {
        let mut threads = self.threads.write();
        if let Some(state) = threads.get_mut(&tid) {
            state.set_status(status);
            true
        } else {
            false
        }
    }

    /// Returns the first registered thread for this process, if any.
    pub fn main_thread(&self) -> Option<(Tid, ThreadState)> {
        self.threads
            .read()
            .iter()
            .next()
            .map(|(tid, state)| (*tid, state.clone()))
    }

    /// Allocates a fresh thread identifier.
    pub fn allocate_tid(&self) -> Tid {
        let id = self.next_tid.fetch_add(1, Ordering::SeqCst);
        Tid::new(id)
    }

    /// Resets thread bookkeeping, removing existing entries and rewinding TID allocation.
    fn reset_threads(&self) {
        self.next_tid.store(1, Ordering::SeqCst);
        self.threads.write().clear();
    }

    /// Assigns the parent PID for this process.
    pub fn set_parent(&self, parent: Pid) {
        *self.parent.write() = Some(parent);
    }

    /// Returns the parent PID if one exists.
    pub fn parent(&self) -> Option<Pid> {
        *self.parent.read()
    }

    /// Records the currently executing program.
    pub fn set_program_image(
        &self,
        program: String,
        image: ExecutableImage,
        memory: Option<&MemoryManager>,
    ) -> Result<(), SubsystemError> {
        let address_space = AddressSpace::from_executable(&image, StackConfig::default());
        let stack_top = address_space.stack().top();
        let context = UserContext::for_entry(address_space.entry_point(), stack_top);

        if let Some(manager) = memory {
            let handle = manager.map_address_space(&address_space)?;
            *self.page_table.write() = Some(handle);
        } else {
            self.page_table.write().take();
        }

        *self.program.write() = Some(program);
        *self.executable.write() = Some(image);
        *self.address_space.write() = Some(address_space.clone());
        *self.user_context.write() = Some(context.clone());

        // Refresh thread table with an initial user thread representing the exec'd image.
        self.reset_threads();
        if let Some(root) = self.page_table_root() {
            let tid = self.allocate_tid();
            let thread_state = ThreadState::new_user(context, root);
            self.add_thread(tid, thread_state);
        }
        Ok(())
    }

    /// Retrieves the program string.
    pub fn program(&self) -> Option<String> {
        self.program.read().clone()
    }

    /// Retrieves the parsed executable image for the process, if any.
    pub fn executable_image(&self) -> Option<ExecutableImage> {
        self.executable.read().clone()
    }

    /// Returns the synthesized user address space for this process, if available.
    pub fn address_space(&self) -> Option<AddressSpace> {
        self.address_space.read().clone()
    }

    /// Returns the user context (register snapshot) if one has been built for the process.
    pub fn user_context(&self) -> Option<UserContext> {
        self.user_context.read().clone()
    }

    /// Returns the physical frame for the process's root page table, if mapped.
    pub fn page_table_root(&self) -> Option<PhysFrame> {
        self.page_table.read().as_ref().map(|handle| handle.root())
    }

    /// Replaces the stored user context.
    pub fn set_user_context(&self, context: UserContext) {
        *self.user_context.write() = Some(context);
    }

    /// Returns the next free file descriptor.
    pub fn next_fd(&self) -> u64 {
        self.next_fd.fetch_add(1, Ordering::SeqCst)
    }

    /// Inserts a descriptor binding.
    pub fn insert_fd(&self, fd: u64, path: String) {
        let is_path = Self::descriptor_is_path(&path);
        self.fds.write().insert(fd, path);
        if is_path {
            self.reset_fd_offset(fd);
        } else {
            self.remove_fd_offset(fd);
        }
        self.ensure_next_fd(fd.saturating_add(1));
    }

    /// Retrieves a descriptor binding.
    pub fn get_fd(&self, fd: u64) -> Option<String> {
        self.fds.read().get(&fd).cloned()
    }

    /// Removes a descriptor binding, returning whether it existed.
    pub fn remove_fd(&self, fd: u64) -> bool {
        let removed = self.fds.write().remove(&fd).is_some();
        if removed {
            self.remove_fd_offset(fd);
        }
        removed
    }

    /// Forcefully assigns a descriptor binding, replacing any existing entry.
    pub fn set_fd(&self, fd: u64, descriptor: String) {
        let is_path = Self::descriptor_is_path(&descriptor);
        self.fds.write().insert(fd, descriptor);
        if is_path {
            self.reset_fd_offset(fd);
        } else {
            self.remove_fd_offset(fd);
        }
        self.ensure_next_fd(fd.saturating_add(1));
    }

    /// Returns the current working directory string.
    pub fn cwd(&self) -> String {
        self.cwd.read().clone()
    }

    /// Updates the working directory.
    pub fn set_cwd(&self, path: String) {
        *self.cwd.write() = path;
    }

    /// Retrieves an environment variable.
    pub fn get_env(&self, key: &str) -> Option<String> {
        self.env.read().get(key).cloned()
    }

    /// Sets an environment variable.
    pub fn set_env(&self, key: String, value: String) {
        self.env.write().insert(key, value);
    }

    fn descriptor_is_path(descriptor: &str) -> bool {
        descriptor.starts_with('/')
    }

    fn reset_fd_offset(&self, fd: u64) {
        self.fd_offsets.write().insert(fd, 0);
    }

    fn remove_fd_offset(&self, fd: u64) {
        self.fd_offsets.write().remove(&fd);
    }

    /// Returns the current offset for the provided descriptor, if tracked.
    pub fn fd_offset(&self, fd: u64) -> Option<usize> {
        self.fd_offsets.read().get(&fd).copied()
    }

    /// Overrides the offset for an existing descriptor.
    pub fn set_fd_offset(&self, fd: u64, offset: usize) {
        self.fd_offsets.write().insert(fd, offset);
    }

    /// Advances the tracked offset for a descriptor.
    pub fn advance_fd_offset(&self, fd: u64, amount: usize) {
        let mut offsets = self.fd_offsets.write();
        let entry = offsets.entry(fd).or_insert(0);
        *entry = entry.saturating_add(amount);
    }

    /// Copies the offset from one descriptor to another, if present.
    pub fn copy_fd_offset(&self, source: u64, target: u64) {
        if let Some(offset) = self.fd_offset(source) {
            self.set_fd_offset(target, offset);
        } else {
            self.remove_fd_offset(target);
        }
    }

    /// Returns an iterator snapshot of all environment variables.
    pub fn env_snapshot(&self) -> BTreeMap<String, String> {
        self.env.read().clone()
    }

    /// Generates a unique identifier for pipe bookkeeping.
    pub fn next_pipe_id(&self) -> u64 {
        self.pipe_seed.fetch_add(1, Ordering::SeqCst)
    }

    /// Returns the user identifier associated with this process.
    pub fn uid(&self) -> u32 {
        self.uid.load(Ordering::SeqCst)
    }

    /// Returns the primary group identifier associated with this process.
    pub fn gid(&self) -> u32 {
        self.gid.load(Ordering::SeqCst)
    }

    /// Updates the user identifier for this process.
    pub fn set_uid(&self, uid: u32) {
        self.uid.store(uid, Ordering::SeqCst);
    }

    /// Updates the primary group identifier for this process.
    pub fn set_gid(&self, gid: u32) {
        self.gid.store(gid, Ordering::SeqCst);
    }

    /// Returns the supplemental groups for this process.
    pub fn supplemental_groups(&self) -> Vec<u32> {
        self.groups.read().clone()
    }

    /// Replaces the supplemental groups for this process.
    pub fn set_groups(&self, groups: Vec<u32>) {
        *self.groups.write() = groups;
    }

    /// Replaces all credential information at once.
    pub fn set_credentials(&self, uid: u32, gid: u32, groups: Vec<u32>) {
        self.set_uid(uid);
        self.set_gid(gid);
        self.set_groups(groups);
    }

    /// Returns a credential snapshot for filesystem access checks.
    pub fn credentials(&self) -> Credentials {
        let mut groups = self.groups.read().clone();
        let primary_gid = self.gid();
        if !groups.iter().any(|&g| g == primary_gid) {
            groups.push(primary_gid);
        }
        Credentials::new(self.uid(), primary_gid, groups)
    }

    fn ensure_next_fd(&self, candidate: u64) {
        let mut current = self.next_fd.load(Ordering::SeqCst);
        while current < candidate {
            match self.next_fd.compare_exchange(
                current,
                candidate,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => break,
                Err(actual) => current = actual,
            }
        }
    }

    /// Clones mutable state into a target process (used by fork).
    pub fn clone_state_into(&self, target: &Process) {
        *target.capabilities.write() = self.capabilities.read().clone();
        *target.program.write() = self.program.read().clone();
        *target.executable.write() = self.executable.read().clone();
        *target.address_space.write() = self.address_space.read().clone();
        target.page_table.write().take();
        *target.user_context.write() = self.user_context.read().clone();
        target
            .next_tid
            .store(self.next_tid.load(Ordering::SeqCst), Ordering::SeqCst);
        *target.threads.write() = self.threads.read().clone();
        *target.fds.write() = self.fds.read().clone();
        *target.fd_offsets.write() = self.fd_offsets.read().clone();
        target
            .next_fd
            .store(self.next_fd.load(Ordering::SeqCst), Ordering::SeqCst);
        target.set_cwd(self.cwd());
        *target.env.write() = self.env.read().clone();
        target
            .pipe_seed
            .store(self.pipe_seed.load(Ordering::SeqCst), Ordering::SeqCst);
        target
            .uid
            .store(self.uid.load(Ordering::SeqCst), Ordering::SeqCst);
        target
            .gid
            .store(self.gid.load(Ordering::SeqCst), Ordering::SeqCst);
        *target.groups.write() = self.groups.read().clone();
    }
}

/// Global process table.
pub struct ProcessTable {
    next_pid: AtomicU64,
    processes: RwLock<BTreeMap<Pid, Arc<Process>>>,
    exec_overrides: RwLock<BTreeMap<String, ExecutableImage>>,
    memory: RwLock<Option<NonNull<MemoryManager>>>,
}

/// Errors returned when waiting on child processes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitError {
    /// The given PID is not a child of the caller.
    NotChild,
    /// The child exists but has not exited yet.
    ChildRunning,
    /// The caller has no children that can be waited on.
    NoChildren,
}

impl Default for ProcessTable {
    fn default() -> Self {
        Self {
            next_pid: AtomicU64::new(100),
            processes: RwLock::new(BTreeMap::new()),
            exec_overrides: RwLock::new(BTreeMap::new()),
            memory: RwLock::new(None),
        }
    }
}

impl ProcessTable {
    /// Creates a new table.
    pub fn new() -> Self {
        Self::default()
    }

    fn allocate_pid(&self) -> Pid {
        Pid(self.next_pid.fetch_add(1, Ordering::SeqCst))
    }

    /// Associates the kernel memory manager with the process table.
    pub fn bind_memory_manager(&self, manager: &'static MemoryManager) {
        *self.memory.write() = Some(NonNull::from(manager));
    }

    fn memory_manager(&self) -> Option<&'static MemoryManager> {
        self.memory
            .read()
            .as_ref()
            .map(|ptr| unsafe { ptr.as_ref() })
    }

    /// Spawns a new process.
    pub fn spawn(&self) -> Arc<Process> {
        let pid = self.allocate_pid();
        let proc = Process::new(pid);
        self.processes.write().insert(pid, proc.clone());
        trace!("Spawned process pid={}", proc.pid().as_u64());
        proc
    }

    /// Registers an in-memory executable image that bypasses filesystem lookup.
    pub fn register_exec_override(&self, path: String, image: ExecutableImage) {
        self.exec_overrides.write().insert(path, image);
    }

    /// Looks up a process by PID.
    pub fn lookup(&self, pid: Pid) -> Option<Arc<Process>> {
        self.processes.read().get(&pid).cloned()
    }

    /// Forks the given parent process, returning the child's PID.
    pub fn fork(&self, parent: Pid) -> Result<Pid, SubsystemError> {
        let parent_proc = self
            .lookup(parent)
            .ok_or(SubsystemError::Runtime("parent not found"))?;
        let child = self.spawn();
        child.set_parent(parent);
        parent_proc.clone_state_into(&child);
        Ok(child.pid())
    }

    /// Replaces the program image for a process.
    pub fn exec(&self, pid: Pid, program: String) -> Result<(), SubsystemError> {
        let proc = self
            .lookup(pid)
            .ok_or(SubsystemError::Runtime("process not found"))?;

        if let Some(image) = self.exec_overrides.read().get(&program).cloned() {
            proc.set_program_image(program, image, self.memory_manager())?;
            return Ok(());
        }

        let creds = proc.credentials();
        let data = match fs::read_file_with_credentials(&program, &creds) {
            Ok(bytes) => bytes,
            Err(FsError::NotFound) => {
                return Err(SubsystemError::Runtime("executable not found"));
            }
            Err(FsError::NotInitialized) => {
                return Err(SubsystemError::Runtime("filesystem unavailable"));
            }
            Err(FsError::PermissionDenied) => {
                return Err(SubsystemError::Runtime("permission denied"));
            }
            Err(_) => return Err(SubsystemError::Runtime("exec read failure")),
        };

        let mut image = ExecutableImage::parse(&data).map_err(|err| match err {
            ElfError::Truncated => SubsystemError::Runtime("executable truncated"),
            ElfError::BadMagic => SubsystemError::Runtime("invalid executable magic"),
            ElfError::UnsupportedClass => SubsystemError::Runtime("unsupported elf class"),
            ElfError::UnsupportedEndian => SubsystemError::Runtime("unsupported elf endian"),
            ElfError::UnsupportedType => SubsystemError::Runtime("unsupported elf type"),
            ElfError::UnsupportedArch => SubsystemError::Runtime("unsupported elf arch"),
            ElfError::BadProgramHeaderBounds => SubsystemError::Runtime("corrupt program header"),
            ElfError::BadSegmentBounds => SubsystemError::Runtime("corrupt segment"),
            ElfError::NoLoadSegments => SubsystemError::Runtime("executable missing segments"),
        })?;

    if let Some(interpreter_path) = image.interpreter().map(|s| String::from(s)) {
            if interpreter_path == program {
                return Err(SubsystemError::Runtime("interpreter recursion detected"));
            }
            let interp_bytes = match fs::read_file_with_credentials(&interpreter_path, &creds) {
                Ok(bytes) => bytes,
                Err(FsError::NotFound) => {
                    return Err(SubsystemError::Runtime("interpreter not found"));
                }
                Err(FsError::NotInitialized) => {
                    return Err(SubsystemError::Runtime("filesystem unavailable"));
                }
                Err(FsError::PermissionDenied) => {
                    return Err(SubsystemError::Runtime("permission denied"));
                }
                Err(_) => return Err(SubsystemError::Runtime("interpreter read failure")),
            };

            let interpreter_image = ExecutableImage::parse(&interp_bytes).map_err(|err| match err {
                ElfError::Truncated => SubsystemError::Runtime("interpreter truncated"),
                ElfError::BadMagic => SubsystemError::Runtime("interpreter invalid magic"),
                ElfError::UnsupportedClass => SubsystemError::Runtime("interpreter unsupported class"),
                ElfError::UnsupportedEndian => SubsystemError::Runtime("interpreter unsupported endian"),
                ElfError::UnsupportedType => SubsystemError::Runtime("interpreter unsupported type"),
                ElfError::UnsupportedArch => SubsystemError::Runtime("interpreter unsupported arch"),
                ElfError::BadProgramHeaderBounds => SubsystemError::Runtime("interpreter corrupt program header"),
                ElfError::BadSegmentBounds => SubsystemError::Runtime("interpreter corrupt segment"),
                ElfError::NoLoadSegments => SubsystemError::Runtime("interpreter missing segments"),
            })?;

            proc.set_env(String::from("INTERPRETEE"), program.clone());
            proc.set_env(String::from("INTERPRETER"), interpreter_path.clone());
            image = interpreter_image;
            proc.set_program_image(interpreter_path, image, self.memory_manager())?;
        } else {
            proc.set_program_image(program, image, self.memory_manager())?;
        }
        Ok(())
    }

    /// Registers an open file descriptor for the process.
    pub fn open(&self, pid: Pid, path: String) -> Result<u64, SubsystemError> {
        let proc = self
            .lookup(pid)
            .ok_or(SubsystemError::Runtime("process not found"))?;
        let fd = proc.next_fd();
        proc.insert_fd(fd, path);
        Ok(fd)
    }

    /// Closes a file descriptor for the process.
    pub fn close(&self, pid: Pid, fd: u64) -> Result<(), SubsystemError> {
        let proc = self
            .lookup(pid)
            .ok_or(SubsystemError::Runtime("process not found"))?;
        if proc.remove_fd(fd) {
            Ok(())
        } else {
            Err(SubsystemError::Runtime("fd not found"))
        }
    }

    /// Duplicates an existing file descriptor, returning the new descriptor number.
    pub fn dup(&self, pid: Pid, fd: u64) -> Result<u64, SubsystemError> {
        let proc = self
            .lookup(pid)
            .ok_or(SubsystemError::Runtime("process not found"))?;
        let descriptor = proc
            .get_fd(fd)
            .ok_or(SubsystemError::Runtime("fd not found"))?;
        let new_fd = proc.next_fd();
        proc.insert_fd(new_fd, descriptor);
        proc.copy_fd_offset(fd, new_fd);
        Ok(new_fd)
    }

    /// Duplicates a descriptor into a specific target number.
    pub fn dup2(&self, pid: Pid, fd: u64, target_fd: u64) -> Result<u64, SubsystemError> {
        let proc = self
            .lookup(pid)
            .ok_or(SubsystemError::Runtime("process not found"))?;
        let descriptor = proc
            .get_fd(fd)
            .ok_or(SubsystemError::Runtime("fd not found"))?;
        proc.set_fd(target_fd, descriptor);
        proc.copy_fd_offset(fd, target_fd);
        Ok(target_fd)
    }

    /// Creates a simple pipe-like pair of descriptors for the process.
    pub fn pipe(&self, pid: Pid) -> Result<(u64, u64, u64), SubsystemError> {
        let proc = self
            .lookup(pid)
            .ok_or(SubsystemError::Runtime("process not found"))?;
        let pipe_id = proc.next_pipe_id();
        let read_fd = proc.next_fd();
        let write_fd = proc.next_fd();
        proc.insert_fd(read_fd, format!("pipe:{}:r", pipe_id));
        proc.insert_fd(write_fd, format!("pipe:{}:w", pipe_id));
        Ok((read_fd, write_fd, pipe_id))
    }

    /// Changes the working directory for the process.
    pub fn chdir(&self, pid: Pid, path: String) -> Result<(), SubsystemError> {
        let proc = self
            .lookup(pid)
            .ok_or(SubsystemError::Runtime("process not found"))?;
        proc.set_cwd(path);
        Ok(())
    }

    /// Returns the current working directory for the process.
    pub fn getcwd(&self, pid: Pid) -> Result<String, SubsystemError> {
        let proc = self
            .lookup(pid)
            .ok_or(SubsystemError::Runtime("process not found"))?;
        Ok(proc.cwd())
    }

    /// Reads all environment variables for the process as a snapshot.
    pub fn env_snapshot(&self, pid: Pid) -> Result<BTreeMap<String, String>, SubsystemError> {
        let proc = self
            .lookup(pid)
            .ok_or(SubsystemError::Runtime("process not found"))?;
        Ok(proc.env_snapshot())
    }

    /// Sets an environment variable for the given process.
    pub fn set_env(&self, pid: Pid, key: String, value: String) -> Result<(), SubsystemError> {
        let proc = self
            .lookup(pid)
            .ok_or(SubsystemError::Runtime("process not found"))?;
        proc.set_env(key, value);
        Ok(())
    }

    /// Marks the process as terminated with the given status.
    pub fn mark_exit(&self, pid: Pid, status: i32) {
        if let Some(proc) = self.lookup(pid) {
            proc.mark_terminated(status);
        }
    }

    /// Waits for a child process to exit.
    pub fn wait_pid(&self, parent: Pid, child: Option<Pid>) -> Result<(Pid, i32), WaitError> {
        let mut table = self.processes.write();

        if let Some(target_pid) = child {
            let proc = table.get(&target_pid).ok_or(WaitError::NoChildren)?;
            if proc.parent() != Some(parent) {
                return Err(WaitError::NotChild);
            }
            if !proc.is_terminated() {
                return Err(WaitError::ChildRunning);
            }
            let status = proc.exit_status();
            table.remove(&target_pid);
            return Ok((target_pid, status));
        }

        let mut found: Option<(Pid, i32)> = None;
        for (pid, proc) in table.iter() {
            if proc.parent() == Some(parent) && proc.is_terminated() {
                found = Some((*pid, proc.exit_status()));
                break;
            }
        }

        match found {
            Some((pid, status)) => {
                table.remove(&pid);
                Ok((pid, status))
            }
            None => Err(WaitError::NoChildren),
        }
    }
}

unsafe impl Send for ProcessTable {}
unsafe impl Sync for ProcessTable {}
