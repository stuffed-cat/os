//! Shell bootstrapper that spawns the initial user-mode process and bridges
//! console input between interrupts and the POSIX layer.

use crate::arch::x86_64::serial;
use crate::core::{KernelContext, Subsystem, SubsystemId};
use crate::error::SubsystemError;
use crate::memory::MemoryManager;
use crate::process::ProcessTable;
#[cfg(all(not(feature = "std"), not(feature = "hardware")))]
use crate::scheduler::SchedulingClass;
use crate::scheduler::{RunQueueEntry, Scheduler, ThreadPriority, ThreadStatus};
use crate::session;
use crate::syscall::SyscallDispatcher;
use crate::users;
use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::string::String;
#[cfg(all(not(feature = "std"), not(feature = "hardware")))]
use log::{error, warn};
use log::{info, trace};
use pc_keyboard::layouts::Us104Key;
use pc_keyboard::{DecodedKey, HandleControl, Keyboard, ScancodeSet1};
use spin::{Lazy, Mutex};

const SCANCODE_QUEUE_CAPACITY: usize = 256;
const CONSOLE_BUFFER_CAPACITY: usize = 4096;

struct ConsoleState {
    keyboard: Keyboard<Us104Key, ScancodeSet1>,
    scancodes: VecDeque<u8>,
    buffer: VecDeque<u8>,
}

impl ConsoleState {
    fn new() -> Self {
        Self {
            keyboard: Keyboard::new(ScancodeSet1::new(), Us104Key, HandleControl::Ignore),
            scancodes: VecDeque::with_capacity(SCANCODE_QUEUE_CAPACITY),
            buffer: VecDeque::with_capacity(CONSOLE_BUFFER_CAPACITY),
        }
    }

    fn push_scancode(&mut self, scancode: u8) {
        if self.scancodes.len() >= SCANCODE_QUEUE_CAPACITY {
            self.scancodes.pop_front();
        }
        self.scancodes.push_back(scancode);
        self.process_scancodes();
    }

    fn process_scancodes(&mut self) {
        while let Some(code) = self.scancodes.pop_front() {
            if let Ok(Some(event)) = self.keyboard.add_byte(code) {
                if let Some(decoded) = self.keyboard.process_keyevent(event) {
                    if let DecodedKey::Unicode(ch) = decoded {
                        self.queue_char(ch);
                    }
                }
            }
        }
    }

    fn queue_char(&mut self, ch: char) {
        match ch {
            '\r' => {
                self.push_byte(b'\n');
                serial::write_str("\r\n");
            }
            '\n' => {
                self.push_byte(b'\n');
                serial::write_str("\r\n");
            }
            '\u{8}' | '\u{7f}' => {
                if self.buffer.pop_back().is_some() {
                    serial::write_str("\u{8} \u{20} \u{8}");
                }
            }
            other => {
                let mut buf = [0u8; 4];
                let encoded = other.encode_utf8(&mut buf);
                for byte in encoded.as_bytes() {
                    self.push_byte(*byte);
                }
                serial::write_str(encoded);
            }
        }
    }

    fn push_byte(&mut self, byte: u8) {
        if self.buffer.len() >= CONSOLE_BUFFER_CAPACITY {
            self.buffer.pop_front();
        }
        self.buffer.push_back(byte);
    }

    fn read(&mut self, out: &mut [u8]) -> usize {
        if out.is_empty() {
            return 0;
        }
        self.process_scancodes();
        let mut count = 0;
        while count < out.len() {
            match self.buffer.pop_front() {
                Some(byte) => {
                    out[count] = byte;
                    count += 1;
                    if byte == b'\n' {
                        break;
                    }
                }
                None => break,
            }
        }
        count
    }

    fn has_data(&mut self) -> bool {
        self.process_scancodes();
        !self.buffer.is_empty()
    }
}

static CONSOLE: Lazy<Mutex<ConsoleState>> = Lazy::new(|| Mutex::new(ConsoleState::new()));

/// Tracks the thread currently executing in user mode.
static CURRENT_THREAD: Lazy<Mutex<Option<RunQueueEntry>>> = Lazy::new(|| Mutex::new(None));

/// Records the most recent user thread that faulted so the scheduler can log it.
static FAILED_THREAD: Lazy<Mutex<Option<RunQueueEntry>>> = Lazy::new(|| Mutex::new(None));

#[cfg(all(not(feature = "std"), not(feature = "hardware")))]
fn set_current_thread(entry: RunQueueEntry) {
    let mut current = CURRENT_THREAD.lock();
    *current = Some(entry);
}

#[cfg(all(not(feature = "std"), not(feature = "hardware")))]
fn clear_current_thread() {
    CURRENT_THREAD.lock().take();
}

/// Enqueues a raw keyboard scancode produced by the hardware interrupt handler.
pub fn enqueue_scancode(scancode: u8) {
    let mut console = CONSOLE.lock();
    console.push_scancode(scancode);
}

/// Called by exception handlers to signal that the current process should be skipped
pub fn mark_current_process_failed() {
    let current = if let Some(scheduler) = Scheduler::global() {
        scheduler.current_thread()
    } else {
        None
    };

    let fallback = {
        let mut guard = CURRENT_THREAD.lock();
        guard.take()
    };

    if let Some(entry) = current.or(fallback) {
        if let Some(table) = ProcessTable::global() {
            if let Some(process) = table.lookup(entry.pid) {
                process.set_thread_status(entry.tid, ThreadStatus::Dead);
                process.mark_terminated(-1);
            }
        }

        if let Some(scheduler) = Scheduler::global() {
            scheduler.complete_current(ThreadStatus::Dead);
        }

        let mut failed = FAILED_THREAD.lock();
        *failed = Some(entry);
    }
}

/// Reads buffered console input into the provided slice, returning the number of bytes copied.
pub fn read_console(buffer: &mut [u8]) -> usize {
    let mut console = CONSOLE.lock();
    console.read(buffer)
}

/// Returns whether buffered console input is currently available.
pub fn console_has_data() -> bool {
    let mut console = CONSOLE.lock();
    console.has_data()
}

/// Shell subsystem responsible for launching the initial user process.
pub struct ShellSubsystem {
    process_table: &'static ProcessTable,
    scheduler: &'static Scheduler,
    init_spawned: bool,
}

impl ShellSubsystem {
    /// Creates a new shell subsystem instance.
    pub fn new() -> Self {
        let process_table = Box::leak(Box::new(ProcessTable::new()));
        process_table.register_global();
        let scheduler = Box::leak(Box::new(Scheduler::new()));
        // Register scheduler globally but DON'T start preemption yet
        // Timer will be enabled in init() after all subsystems are ready
        scheduler.register_global_only();
        crate::arch::x86_64::serial::write_str("shell: scheduler registered (timer deferred)\r\n");
        let dispatcher = Box::leak(Box::new(SyscallDispatcher::new(process_table)));
        dispatcher.register_global();
        crate::arch::x86_64::serial::write_str("shell: syscall dispatcher registered\r\n");

        Self {
            process_table,
            scheduler,
            init_spawned: false,
        }
    }

    fn launch_initial_user(&self) -> Result<(), SubsystemError> {
        session::reset_to_root();
        let profile = users::root_profile();
        let shell_path = if profile.shell.is_empty() {
            String::from("/bin/sh")
        } else {
            profile.shell.clone()
        };

        trace!("shell: spawning initial user process");
        let process = self.process_table.spawn();
        trace!("shell: spawn completed with pid={}", process.pid().as_u64());
        process.set_credentials(profile.uid, profile.gid, profile.groups.clone());
        process.set_cwd(profile.home.clone());
        process.set_env(String::from("HOME"), profile.home.clone());
        process.set_env(String::from("PWD"), profile.home.clone());
        process.set_env(String::from("USER"), profile.username.clone());
        process.set_env(String::from("SHELL"), shell_path.clone());
        process.set_env(
            String::from("PATH"),
            String::from("/bin:/usr/bin:/usr/local/bin"),
        );
        process.set_env(String::from("TERM"), String::from("nexa-console"));

        trace!("shell: executing {}", shell_path);
        self.process_table.exec(process.pid(), shell_path)?;
        trace!("shell: exec completed");

        let (tid, _) = process
            .main_thread()
            .ok_or(SubsystemError::Runtime("exec produced no runnable thread"))?;
        let user_context = process.user_context();
        trace!(
            "shell: user context present? {}",
            user_context.is_some()
        );
        match user_context {
            Some(context) => {
                let frame = context.frame();
                trace!(
                    "shell: initial user context rip={:#x} rsp={:#x}",
                    frame.rip,
                    frame.rsp
                );
            }
            None => {
                trace!("shell: initial user context missing");
            }
        }
        process.set_thread_status(tid, ThreadStatus::Ready);
        let priority = process
            .thread_state(tid)
            .map(|state| state.priority())
            .unwrap_or(ThreadPriority::Normal);
        trace!("shell: enqueueing process for scheduling");
        self.scheduler
            .enqueue(RunQueueEntry::user(process.pid(), tid, priority));
        Ok(())
    }

    #[cfg(all(not(feature = "std"), not(feature = "hardware")))]
    fn run_ready_threads(&mut self) {
        use core::task::Poll;

        loop {
            if let Some(failed) = FAILED_THREAD.lock().take() {
                error!(
                    "scheduler: terminated user process pid={} tid={} after exception",
                    failed.pid.as_u64(),
                    failed.tid.as_u64()
                );
                continue;
            }

            match self.scheduler.tick() {
                Poll::Ready(entry) => {
                    set_current_thread(entry);

                    match entry.class {
                        SchedulingClass::User => {
                            if let Some(process) = self.process_table.lookup(entry.pid) {
                                if let Some((context, root)) =
                                    process.take_thread_runtime(entry.tid)
                                {
                                    process.set_thread_status(entry.tid, ThreadStatus::Running);
                                    unsafe {
                                        crate::arch::x86_64::context::enter_user_mode(
                                            &context, root,
                                        );
                                    }
                                } else {
                                    clear_current_thread();
                                    warn!(
                                        "scheduler: missing user context for pid={} tid={}",
                                        entry.pid.as_u64(),
                                        entry.tid.as_u64()
                                    );
                                }
                            } else {
                                clear_current_thread();
                                warn!("scheduler: missing process for pid={}", entry.pid.as_u64());
                            }
                        }
                        SchedulingClass::Kernel => {
                            clear_current_thread();
                            warn!(
                                "scheduler: kernel thread scheduling not implemented (pid={} tid={})",
                                entry.pid.as_u64(),
                                entry.tid.as_u64()
                            );
                        }
                    }
                }
                Poll::Pending => {
                    // No ready processes
                    break;
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

        // NOW enable timer after all initialization is done
        Scheduler::enable_timer_global();

        if !self.init_spawned {
            // Disable interrupts during initial user launch to avoid deadlock with scheduler lock
            x86_64::instructions::interrupts::disable();
            let result = self.launch_initial_user();
            x86_64::instructions::interrupts::enable();

            match result {
                Ok(()) => {
                    self.init_spawned = true;
                    info!("initial user shell spawned");
                }
                Err(e) => {
                    log::error!("failed to launch initial user: {:?}", e);
                    return Err(e);
                }
            }
        }

        Ok(())
    }

    fn tick(&mut self, _ctx: &KernelContext) -> Result<(), SubsystemError> {
        #[cfg(all(not(feature = "std"), not(feature = "hardware")))]
        self.run_ready_threads();
        Ok(())
    }
}
