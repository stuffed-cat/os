//! Hybrid scheduler combining monolithic fast path with capability isolation.

use alloc::collections::VecDeque;
use core::task::Poll;
use log::trace;
use spin::Mutex;

use crate::process::Tid;

/// Possible states of a thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadState {
    /// Runnable and ready to be scheduled.
    Ready,
    /// Blocked waiting for an event.
    Blocked,
    /// Terminated.
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

/// Run queue entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunQueueEntry {
    /// Thread identifier.
    pub tid: Tid,
    /// Scheduling class.
    pub class: SchedulingClass,
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
        trace!("Enqueue thread {:?}", entry);
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
