//! Process management and hybrid capability tracking.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use spin::RwLock;

use crate::{error::SubsystemError, memory::Capability, scheduler::ThreadState};
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
    capabilities: RwLock<alloc::vec::Vec<Capability>>, // microkernel style capability list
    exit_status: AtomicI32,
    terminated: AtomicBool,
    parent: RwLock<Option<Pid>>,
    program: RwLock<Option<String>>,
    fds: RwLock<BTreeMap<u64, String>>,
    next_fd: AtomicU64,
}

impl Process {
    /// Allocates a new process.
    pub fn new(pid: Pid) -> Arc<Self> {
        Arc::new(Self {
            pid,
            threads: RwLock::new(BTreeMap::new()),
            capabilities: RwLock::new(alloc::vec::Vec::new()),
            exit_status: AtomicI32::new(0),
            terminated: AtomicBool::new(false),
            parent: RwLock::new(None),
            program: RwLock::new(None),
            fds: RwLock::new(BTreeMap::new()),
            next_fd: AtomicU64::new(4),
        })
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

    /// Assigns the parent PID for this process.
    pub fn set_parent(&self, parent: Pid) {
        *self.parent.write() = Some(parent);
    }

    /// Returns the parent PID if one exists.
    pub fn parent(&self) -> Option<Pid> {
        *self.parent.read()
    }

    /// Records the currently executing program.
    pub fn set_program(&self, program: String) {
        *self.program.write() = Some(program);
    }

    /// Retrieves the program string.
    pub fn program(&self) -> Option<String> {
        self.program.read().clone()
    }

    /// Returns the next free file descriptor.
    pub fn next_fd(&self) -> u64 {
        self.next_fd.fetch_add(1, Ordering::SeqCst)
    }

    /// Inserts a descriptor binding.
    pub fn insert_fd(&self, fd: u64, path: String) {
        self.fds.write().insert(fd, path);
    }

    /// Retrieves a descriptor binding.
    pub fn get_fd(&self, fd: u64) -> Option<String> {
        self.fds.read().get(&fd).cloned()
    }

    /// Removes a descriptor binding, returning whether it existed.
    pub fn remove_fd(&self, fd: u64) -> bool {
        self.fds.write().remove(&fd).is_some()
    }

    /// Clones mutable state into a target process (used by fork).
    pub fn clone_state_into(&self, target: &Process) {
        *target.capabilities.write() = self.capabilities.read().clone();
        *target.program.write() = self.program.read().clone();
        *target.fds.write() = self.fds.read().clone();
        target
            .next_fd
            .store(self.next_fd.load(Ordering::SeqCst), Ordering::SeqCst);
    }
}

/// Global process table.
pub struct ProcessTable {
    next_pid: AtomicU64,
    processes: RwLock<BTreeMap<Pid, Arc<Process>>>,
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

    /// Spawns a new process.
    pub fn spawn(&self) -> Arc<Process> {
        let pid = self.allocate_pid();
        let proc = Process::new(pid);
        self.processes.write().insert(pid, proc.clone());
        trace!("Spawned process pid={}", proc.pid().as_u64());
        proc
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
        proc.set_program(program);
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
