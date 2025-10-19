//! Hybrid scheduler implementing preemptive, priority-aware time slicing.

use alloc::collections::{BTreeMap, BinaryHeap};
use core::cmp::{Ordering, Reverse};
#[cfg(feature = "std")]
use core::sync::atomic::AtomicBool;
use core::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use core::task::Poll;
use core::time::Duration;
use log::trace;
use spin::{Mutex, Once};

use crate::{
    arch::x86_64::context as arch_context,
    process::{KernelContext, Pid, Tid},
    user::UserContext,
};
use x86_64::structures::paging::PhysFrame;

const DEFAULT_TIMER_FREQUENCY_HZ: u64 = 250;

static GLOBAL_SCHEDULER: Once<&'static Scheduler> = Once::new();

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

/// Priority hints that influence time-slice length and fairness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ThreadPriority {
    /// Real-time or kernel work that needs short slices.
    High,
    /// Default priority for user processes.
    Normal,
    /// Background work with reduced CPU share.
    Low,
}

impl Default for ThreadPriority {
    fn default() -> Self {
        ThreadPriority::Normal
    }
}

impl ThreadPriority {
    fn weight(self) -> u64 {
        match self {
            ThreadPriority::High => 1,
            ThreadPriority::Normal => 2,
            ThreadPriority::Low => 4,
        }
    }

    fn order(self) -> u8 {
        match self {
            ThreadPriority::High => 0,
            ThreadPriority::Normal => 1,
            ThreadPriority::Low => 2,
        }
    }

    fn time_slice_ticks(self) -> u32 {
        match self {
            ThreadPriority::High => 2,
            ThreadPriority::Normal => 4,
            ThreadPriority::Low => 8,
        }
    }
}

/// Per-thread control block stored alongside the process metadata.
#[derive(Debug, Clone)]
pub struct ThreadState {
    status: ThreadStatus,
    class: SchedulingClass,
    context: Option<UserContext>,
    page_table_root: Option<PhysFrame>,
    priority: ThreadPriority,
    kernel_context: Option<KernelContext>,
}

impl ThreadState {
    /// Creates a brand-new kernel thread slot.
    pub fn new_kernel() -> Self {
        Self {
            status: ThreadStatus::Ready,
            class: SchedulingClass::Kernel,
            context: None,
            page_table_root: None,
            priority: ThreadPriority::High,
            kernel_context: None,
        }
    }

    /// Constructs a user thread bound to a user context and top-level page table.
    pub fn new_user(context: UserContext, root: PhysFrame) -> Self {
        Self {
            status: ThreadStatus::Ready,
            class: SchedulingClass::User,
            context: Some(context),
            page_table_root: Some(root),
            priority: ThreadPriority::default(),
            kernel_context: None,
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

    /// Returns the configured scheduling priority.
    pub fn priority(&self) -> ThreadPriority {
        self.priority
    }

    /// Updates the priority hint for the thread.
    pub fn set_priority(&mut self, priority: ThreadPriority) {
        self.priority = priority;
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

    /// Updates the CR3 root frame backing this thread's address space.
    pub fn set_page_table_root(&mut self, root: PhysFrame) {
        self.page_table_root = Some(root);
    }

    /// Clears the stored page table root for this thread.
    pub fn clear_page_table_root(&mut self) {
        self.page_table_root = None;
    }

    /// Consumes the stored user context and returns it together with the page table root.
    pub fn take_runtime_state(&mut self) -> Option<(UserContext, PhysFrame)> {
        let context = self.context.take()?;
        let root = self.page_table_root?;
        Some((context, root))
    }

    /// Replaces the saved user context snapshot.
    pub fn store_context(&mut self, context: UserContext) {
        self.context = Some(context);
    }

    /// Returns the stored kernel context snapshot, if any.
    pub fn kernel_context(&self) -> Option<&KernelContext> {
        self.kernel_context.as_ref()
    }

    /// Replaces the saved kernel context snapshot.
    pub fn store_kernel_context(&mut self, context: KernelContext) {
        self.kernel_context = Some(context);
    }

    /// Removes and returns the stored kernel context snapshot.
    pub fn take_kernel_context(&mut self) -> Option<KernelContext> {
        self.kernel_context.take()
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
    /// Scheduling priority hint.
    pub priority: ThreadPriority,
}

impl RunQueueEntry {
    /// Creates a new run-queue entry for a kernel thread.
    pub fn kernel(pid: Pid, tid: Tid) -> Self {
        Self {
            pid,
            tid,
            class: SchedulingClass::Kernel,
            priority: ThreadPriority::High,
        }
    }

    /// Creates a new run-queue entry for a user thread.
    pub fn user(pid: Pid, tid: Tid, priority: ThreadPriority) -> Self {
        Self {
            pid,
            tid,
            class: SchedulingClass::User,
            priority,
        }
    }

    /// Returns the key representation of this entry.
    fn key(&self) -> ThreadKey {
        ThreadKey::new(self.pid, self.tid)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct ThreadKey {
    pid: Pid,
    tid: Tid,
}

impl ThreadKey {
    const fn new(pid: Pid, tid: Tid) -> Self {
        Self { pid, tid }
    }
}

#[derive(Debug, Clone)]
struct ThreadInfo {
    entry: RunQueueEntry,
    priority: ThreadPriority,
    vruntime: u64,
    status: ThreadStatus,
    queued: bool,
}

impl ThreadInfo {
    fn new(entry: RunQueueEntry) -> Self {
        Self {
            priority: entry.priority,
            entry,
            vruntime: 0,
            status: ThreadStatus::Ready,
            queued: false,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct RunningThread {
    key: ThreadKey,
    slice_remaining: u32,
}

#[derive(Debug, Clone, Copy)]
struct SchedItem {
    key: ThreadKey,
    vruntime: u64,
    priority: ThreadPriority,
}

impl PartialEq for SchedItem {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key && self.vruntime == other.vruntime
    }
}

impl Eq for SchedItem {}

impl PartialOrd for SchedItem {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SchedItem {
    fn cmp(&self, other: &Self) -> Ordering {
        match self.vruntime.cmp(&other.vruntime) {
            Ordering::Equal => match self.priority.order().cmp(&other.priority.order()) {
                Ordering::Equal => self.key.cmp(&other.key),
                order => order,
            },
            order => order,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct SleepEntry {
    wake_tick: u64,
    key: ThreadKey,
}

impl PartialEq for SleepEntry {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key && self.wake_tick == other.wake_tick
    }
}

impl Eq for SleepEntry {}

impl PartialOrd for SleepEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SleepEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        match self.wake_tick.cmp(&other.wake_tick) {
            Ordering::Equal => self.key.cmp(&other.key),
            order => order,
        }
    }
}

#[derive(Default)]
struct SchedulerInner {
    run_queue: BinaryHeap<Reverse<SchedItem>>,
    sleeping: BinaryHeap<Reverse<SleepEntry>>,
    threads: BTreeMap<ThreadKey, ThreadInfo>,
    current: Option<RunningThread>,
    tick: u64,
    need_resched: bool,
}

impl SchedulerInner {
    fn push_run_queue(&mut self, key: ThreadKey) {
        if let Some(info) = self.threads.get_mut(&key) {
            if info.status == ThreadStatus::Ready && !info.queued {
                info.queued = true;
                self.run_queue.push(Reverse(SchedItem {
                    key,
                    vruntime: info.vruntime,
                    priority: info.priority,
                }));
            }
        }
    }

    fn pop_next_ready(&mut self) -> Option<(ThreadKey, RunQueueEntry, ThreadPriority)> {
        while let Some(Reverse(item)) = self.run_queue.pop() {
            if let Some(info) = self.threads.get_mut(&item.key) {
                if info.status == ThreadStatus::Ready {
                    info.queued = false;
                    return Some((item.key, info.entry, info.priority));
                }
            }
        }
        None
    }
}

/// Core scheduling entity.
pub struct Scheduler {
    inner: Mutex<SchedulerInner>,
    tick_frequency: AtomicU64,
    #[cfg(feature = "std")]
    ticker_spawned: AtomicBool,
}

#[derive(Clone, Copy, Debug, Default)]
/// Summary of scheduling decisions produced by a hardware timer tick.
pub struct TimerTickOutcome {
    /// Thread that exhausted its time slice and was re-queued, if any.
    pub preempted: Option<RunQueueEntry>,
    /// The thread selected to run next, if the scheduler chose one.
    pub next: Option<RunQueueEntry>,
}

impl Default for Scheduler {
    fn default() -> Self {
        Self {
            inner: Mutex::new(SchedulerInner::default()),
            tick_frequency: AtomicU64::new(DEFAULT_TIMER_FREQUENCY_HZ),
            #[cfg(feature = "std")]
            ticker_spawned: AtomicBool::new(false),
        }
    }
}

impl Scheduler {
    /// Creates a scheduler instance.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers this scheduler as the global instance only (without enabling timer).
    /// Timer will be enabled later in init() phase.
    pub fn register_global_only(&'static self) {
        GLOBAL_SCHEDULER.call_once(|| self);
    }

    /// Registers this scheduler as the global instance and configures timer interrupts.
    pub fn start_preemption(&'static self) {
        GLOBAL_SCHEDULER.call_once(|| self);
        self.configure_timer(DEFAULT_TIMER_FREQUENCY_HZ);
    }

    /// Enables the timer (should be called from init() after all subsystems ready).
    /// This is a static method that accesses the global scheduler.
    pub fn enable_timer_global() {
        if let Some(scheduler) = Self::global() {
            scheduler.configure_timer(DEFAULT_TIMER_FREQUENCY_HZ);
        }
        crate::arch::x86_64::serial::write_str("shell: timer enabled in init\r\n");
    }

    /// Configures the timer frequency driving preemption.
    pub fn configure_timer(&'static self, frequency_hz: u64) {
        let hz = frequency_hz.max(1);
        self.tick_frequency.store(hz, AtomicOrdering::SeqCst);
        trace!("scheduler: configuring timer at {} Hz", hz);
        #[cfg(any(feature = "hardware", feature = "boot"))]
        {
            trace!("scheduler: initializing PIT timer for hardware");
            // Add explicit serial prints to aid debugging on minimal boot builds
            // so we can tell whether PIT init returns.
            crate::arch::x86_64::serial::write_str("scheduler: about to init PIT\r\n");
            crate::arch::x86_64::timer::init(hz as u32);
            crate::arch::x86_64::serial::write_str("scheduler: PIT init returned\r\n");
            trace!("scheduler: PIT timer initialized");
        }
        #[cfg(feature = "std")]
        self.spawn_host_timer(hz);
    }

    /// Returns the registered global scheduler, if any.
    pub fn global() -> Option<&'static Scheduler> {
        GLOBAL_SCHEDULER.get().copied()
    }

    /// Enqueues a runnable thread, updating its scheduling metadata.
    pub fn enqueue(&self, entry: RunQueueEntry) {
        trace!(
            "scheduler: enqueue pid={} tid={} priority={:?}",
            entry.pid.as_u64(),
            entry.tid.as_u64(),
            entry.priority
        );

        let mut inner = self.inner.lock();
        let key = entry.key();
        let info = inner
            .threads
            .entry(key)
            .or_insert_with(|| ThreadInfo::new(entry));
        info.entry = entry;
        info.priority = entry.priority;
        if info.status == ThreadStatus::Dead {
            return;
        }
        info.status = ThreadStatus::Ready;
        inner.push_run_queue(key);
    }

    /// Marks the current running thread as completed with the provided status.
    pub fn complete_current(&self, status: ThreadStatus) {
        let mut inner = self.inner.lock();
        if let Some(running) = inner.current.take() {
            if let Some(info) = inner.threads.get_mut(&running.key) {
                info.status = status;
                info.queued = false;
                if status == ThreadStatus::Ready {
                    inner.push_run_queue(running.key);
                }
            }
        }
        inner.need_resched = true;
    }

    /// Places the current thread into a timed sleep queue.
    pub fn sleep_current(&self, duration: Duration) {
        if duration.is_zero() {
            self.yield_current();
            return;
        }

        let ticks = self.duration_to_ticks(duration).max(1);
        let mut inner = self.inner.lock();
        if let Some(running) = inner.current.take() {
            let blocked = inner
                .threads
                .get_mut(&running.key)
                .map(|info| {
                    info.status = ThreadStatus::Blocked;
                    info.queued = false;
                })
                .is_some();

            if blocked {
                let wake_tick = inner.tick.saturating_add(ticks);
                inner.sleeping.push(Reverse(SleepEntry {
                    wake_tick,
                    key: running.key,
                }));
                inner.need_resched = true;
            }
        }
    }

    /// Gives up the remainder of the time slice voluntarily.
    pub fn yield_current(&self) {
        let mut inner = self.inner.lock();
        if let Some(running) = inner.current.take() {
            if let Some(info) = inner.threads.get_mut(&running.key) {
                info.status = ThreadStatus::Ready;
                info.queued = false;
                inner.push_run_queue(running.key);
            }
            inner.need_resched = true;
        }
    }

    /// Simulates a scheduling tick returning readiness state.
    pub fn tick(&self) -> Poll<RunQueueEntry> {
        let mut inner = self.inner.lock();
        // If there's a current thread running and no reschedule needed, keep it running
        if inner.current.is_some() && !inner.need_resched {
            return Poll::Pending;
        }

        // If no thread is currently running, or reschedule is needed, get the next one
        if let Some((key, entry, priority)) = inner.pop_next_ready() {
            if let Some(info) = inner.threads.get_mut(&key) {
                info.status = ThreadStatus::Running;
                let slice = priority.time_slice_ticks().max(1);
                inner.current = Some(RunningThread {
                    key,
                    slice_remaining: slice,
                });
                inner.need_resched = false;
                return Poll::Ready(entry);
            }
        }

        Poll::Pending
    }

    /// Picks the next runnable thread if one is immediately available.
    pub fn pick_next(&self) -> Option<RunQueueEntry> {
        match self.tick() {
            Poll::Ready(entry) => Some(entry),
            Poll::Pending => None,
        }
    }

    /// Handles a periodic timer interrupt, updating runtime accounting and wakeups.
    pub fn handle_timer_tick(&self) {
        let mut inner = self.inner.lock();
        inner.tick = inner.tick.saturating_add(1);
        Self::wake_sleepers(&mut inner);
        let _ = Self::advance_current(&mut inner);
    }

    /// Advances the scheduler by a single hardware timer tick and reports preemption events.
    pub fn evaluate_timer_tick(&self) -> TimerTickOutcome {
        let mut inner = self.inner.lock();
        inner.tick = inner.tick.saturating_add(1);
        // Disable verbose tick logging for cleaner output
        // trace!("scheduler: evaluate_timer_tick #{}", inner.tick);
        Self::wake_sleepers(&mut inner);
        let preempted = Self::advance_current(&mut inner);
        let next = if inner.need_resched {
            Self::select_next(&mut inner)
        } else {
            None
        };

        TimerTickOutcome { preempted, next }
    }

    /// Returns the total number of scheduler ticks observed.
    pub fn current_tick(&self) -> u64 {
        self.inner.lock().tick
    }

    /// Returns the currently running thread, if one is scheduled.
    pub fn current_thread(&self) -> Option<RunQueueEntry> {
        let inner = self.inner.lock();
        let running = match inner.current {
            Some(running) => running,
            None => return None,
        };
        inner.threads.get(&running.key).map(|info| info.entry)
    }

    fn duration_to_ticks(&self, duration: Duration) -> u64 {
        let hz = self.tick_frequency.load(AtomicOrdering::SeqCst);
        if hz == 0 {
            return 0;
        }
        let nanos = duration.as_nanos();
        let tick_ns = 1_000_000_000u128 / hz as u128;
        ((nanos + tick_ns - 1) / tick_ns) as u64
    }

    #[cfg(feature = "std")]
    fn spawn_host_timer(&'static self, frequency_hz: u64) {
        use std::thread;
        use std::time::Duration as StdDuration;

        if self.ticker_spawned.swap(true, AtomicOrdering::SeqCst) {
            return;
        }

        let interval = StdDuration::from_nanos((1_000_000_000u64 / frequency_hz).max(1));
        thread::Builder::new()
            .name("scheduler-timer".into())
            .spawn(move || loop {
                thread::sleep(interval);
                if let Some(scheduler) = Scheduler::global() {
                    scheduler.handle_timer_tick();
                }
            })
            .expect("failed to spawn scheduler timer thread");
    }
}

impl Scheduler {
    fn wake_sleepers(inner: &mut SchedulerInner) {
        while let Some(Reverse(entry)) = inner.sleeping.peek() {
            if entry.wake_tick > inner.tick {
                break;
            }
            let Reverse(expired) = inner.sleeping.pop().expect("sleeping entry vanished");
            if let Some(info) = inner.threads.get_mut(&expired.key) {
                if info.status == ThreadStatus::Blocked {
                    info.status = ThreadStatus::Ready;
                    inner.push_run_queue(expired.key);
                }
            }
        }
    }

    fn advance_current(inner: &mut SchedulerInner) -> Option<RunQueueEntry> {
        if let Some(mut running) = inner.current.take() {
            let mut requeue_entry = None;
            if let Some(info) = inner.threads.get_mut(&running.key) {
                info.vruntime = info.vruntime.saturating_add(info.priority.weight());
                if running.slice_remaining > 1 {
                    running.slice_remaining -= 1;
                    inner.current = Some(running);
                    return None;
                } else {
                    info.status = ThreadStatus::Ready;
                    info.queued = false;
                    inner.need_resched = true;
                    requeue_entry = Some(info.entry);
                }
            } else {
                inner.need_resched = true;
            }
            if let Some(entry) = requeue_entry {
                inner.push_run_queue(running.key);
                return Some(entry);
            }
        }
        None
    }

    fn select_next(inner: &mut SchedulerInner) -> Option<RunQueueEntry> {
        if let Some((key, entry, priority)) = inner.pop_next_ready() {
            if let Some(info) = inner.threads.get_mut(&key) {
                info.status = ThreadStatus::Running;
            }
            let slice = priority.time_slice_ticks().max(1);
            inner.current = Some(RunningThread {
                key,
                slice_remaining: slice,
            });
            inner.need_resched = false;
            Some(entry)
        } else {
            inner.need_resched = false;
            None
        }
    }
}
