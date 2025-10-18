//! Hybrid scheduler combining monolithic fast path with capability isolation.

use alloc::collections::VecDeque;
use core::task::Poll;
use log::trace;
use spin::Mutex;

use crate::{
    arch::x86_64::context as arch_context,
    process::{Pid, Tid},
    user::UserContext,
};
use x86_64::structures::paging::PhysFrame;

/// Lifecycle state tracked for each thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadStatus {
    /// Runnable and ready to be scheduled.
    Ready,
    /// Currently executing on a CPU.
    Running,
    /// Blocked waiting for an event or resource.
    Blocked,
    /// Terminated and awaiting cleanup.
    Dead,
}

/// Scheduling class differentiating kernel and user threads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedulingClass {
    /// Kernel workers executing privileged tasks.
    Kernel,
    /// Userland threads governed by POSIX policies.
    User,
}

/// Per-thread control block stored alongside the process metadata.
#[derive(Debug, Clone)]
pub struct ThreadState {
    status: ThreadStatus,
    class: SchedulingClass,
    context: Option<UserContext>,
    page_table_root: Option<PhysFrame>,
}

impl ThreadState {
    /// Creates a brand-new kernel thread slot.
    pub fn new_kernel() -> Self {
        Self {
            status: ThreadStatus::Ready,
            class: SchedulingClass::Kernel,
            context: None,
            page_table_root: None,
        }
    }

    /// Constructs a user thread bound to a user context and top-level page table.
    pub fn new_user(context: UserContext, root: PhysFrame) -> Self {
        Self {
            status: ThreadStatus::Ready,
            class: SchedulingClass::User,
            context: Some(context),
            page_table_root: Some(root),
        }
    }

    /// Returns the current lifecycle status for the thread.
    pub fn status(&self) -> ThreadStatus {
        self.status
    }

    /// Updates the lifecycle status.
    pub fn set_status(&mut self, status: ThreadStatus) {
        self.status = status;
    }

    /// Returns the scheduling class for this thread.
    pub fn class(&self) -> SchedulingClass {
        self.class
    }

    /// Returns an immutable reference to the stored user context, if any.
    pub fn context(&self) -> Option<&UserContext> {
        self.context.as_ref()
    }

    /// Provides mutable access to the user context where callers can stage register updates.
    pub fn context_mut(&mut self) -> Option<&mut UserContext> {
        self.context.as_mut()
    }

    /// Returns the CR3 root frame backing this thread's address space.
    pub fn page_table_root(&self) -> Option<PhysFrame> {
        self.page_table_root
    }

    /// Performs a user-mode transition using the stored context and address space.
    ///
    /// # Safety
    ///
    /// This function never returns when successful and assumes the caller has performed
    /// all necessary validation on the contained register snapshot.
    pub unsafe fn enter_user_mode(&self) -> ! {
        let context = self
            .context
            .as_ref()
            .expect("user thread missing context")
            .clone();
        let root = self
            .page_table_root
            .expect("user thread missing page table root");
        arch_context::enter_user_mode(&context, root)
    }
}

/// Run queue entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunQueueEntry {
    /// Process identifier owning the thread.
    pub pid: Pid,
    /// Thread identifier.
    pub tid: Tid,
    /// Scheduling class.
    pub class: SchedulingClass,
}

impl RunQueueEntry {
    /// Creates a new run-queue entry for a kernel thread.
    pub fn kernel(pid: Pid, tid: Tid) -> Self {
        Self {
            pid,
            tid,
            class: SchedulingClass::Kernel,
        }
    }

    /// Creates a new run-queue entry for a user thread.
    pub fn user(pid: Pid, tid: Tid) -> Self {
        Self {
            pid,
            tid,
            class: SchedulingClass::User,
        }
    }
}

/// Core scheduling entity.
pub struct Scheduler {
    run_queue: Mutex<VecDeque<RunQueueEntry>>,
}

impl Default for Scheduler {
    fn default() -> Self {
        Self {
            run_queue: Mutex::new(VecDeque::new()),
        }
    }
}

impl Scheduler {
    /// Creates a scheduler instance.
    pub fn new() -> Self {
        Self::default()
    }

    /// Enqueues a runnable thread.
    pub fn enqueue(&self, entry: RunQueueEntry) {
        trace!(
            "enqueue thread pid={} tid={}",
            entry.pid.as_u64(),
            entry.tid.as_u64()
        );
        self.run_queue.lock().push_back(entry);
    }

    /// Picks the next runnable thread.
    pub fn pick_next(&self) -> Option<RunQueueEntry> {
        self.run_queue.lock().pop_front()
    }

    /// Simulates a scheduling tick returning readiness state.
    pub fn tick(&self) -> Poll<RunQueueEntry> {
        if let Some(entry) = self.pick_next() {
            Poll::Ready(entry)
        } else {
            Poll::Pending
        }
    }
}
