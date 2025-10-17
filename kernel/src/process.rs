//! Process management and hybrid capability tracking.

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicI32, AtomicU64, Ordering};
use spin::RwLock;

use crate::{memory::Capability, scheduler::ThreadState};
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

/// Process control block with capability list.
pub struct Process {
    pid: Pid,
    threads: RwLock<BTreeMap<Tid, ThreadState>>, // monolithic fast path for scheduling
    capabilities: RwLock<alloc::vec::Vec<Capability>>, // microkernel style capability list
    exit_status: AtomicI32,
}

impl Process {
    /// Allocates a new process.
    pub fn new(pid: Pid) -> Arc<Self> {
        Arc::new(Self {
            pid,
            threads: RwLock::new(BTreeMap::new()),
            capabilities: RwLock::new(alloc::vec::Vec::new()),
            exit_status: AtomicI32::new(0),
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

    /// Registers a capability token.
    pub fn add_capability(&self, cap: Capability) {
        self.capabilities.write().push(cap);
    }

    /// Adds a thread to the process.
    pub fn add_thread(&self, tid: Tid, state: ThreadState) {
        self.threads.write().insert(tid, state);
    }
}

/// Global process table.
pub struct ProcessTable {
    next_pid: AtomicU64,
    processes: RwLock<BTreeMap<Pid, Arc<Process>>>,
}

impl Default for ProcessTable {
    fn default() -> Self {
        Self { next_pid: AtomicU64::new(100), processes: RwLock::new(BTreeMap::new()) }
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
}
