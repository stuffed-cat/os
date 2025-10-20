//! Shell bootstrapper that spawns the initial user-mode process and bridges
//! console input between interrupts and the POSIX layer.

use crate::arch::x86_64::serial;
use crate::core::{KernelContext, Subsystem, SubsystemId};
use crate::error::SubsystemError;
use crate::fs::{self, FsError as KernelFsError};
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
use alloc::vec::Vec;
#[cfg(all(not(feature = "std"), not(feature = "hardware")))]
use log::{error, warn};
use log::trace;
use pc_keyboard::layouts::Us104Key;
use pc_keyboard::{DecodedKey, HandleControl, Keyboard, ScancodeSet1};
use spin::{Lazy, Mutex};
use userland::bare_shell::{
    AuthError as BareAuthError, BareShell, DirEntry as BareDirEntry, EntryKind as BareEntryKind,
    ExecResult as BareExecResult, FsError as BareFsError, ShellFs as BareShellFs,
    ShellIo as BareShellIo, ShellSystem as BareShellSystem, SystemError as BareSystemError,
    UserAdminError as BareUserAdminError, UserIdentity as BareUserIdentity,
};

const SCANCODE_QUEUE_CAPACITY: usize = 256;
const CONSOLE_BUFFER_CAPACITY: usize = 4096;
const ENABLE_USER_PROCESSES: bool = false;
const DEFAULT_HOSTNAME: &str = "nexaos";

type KernelBareShell = BareShell<KernelShellIo, KernelShellFs, KernelShellSystem>;

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
static BARE_SHELL_SCANCODES: Lazy<Mutex<VecDeque<u8>>> =
    Lazy::new(|| Mutex::new(VecDeque::with_capacity(SCANCODE_QUEUE_CAPACITY)));

/// Tracks the thread currently executing in user mode.
static CURRENT_THREAD: Lazy<Mutex<Option<RunQueueEntry>>> = Lazy::new(|| Mutex::new(None));

/// Records the most recent user thread that faulted so the scheduler can log it.
static FAILED_THREAD: Lazy<Mutex<Option<RunQueueEntry>>> = Lazy::new(|| Mutex::new(None));

/// Records details about the most recent user space fault.
static LAST_USER_FAULT: Lazy<Mutex<Option<UserFaultRecord>>> =
    Lazy::new(|| Mutex::new(None));

fn push_bare_shell_scancode(scancode: u8) {
    let mut queue = BARE_SHELL_SCANCODES.lock();
    if queue.len() >= SCANCODE_QUEUE_CAPACITY {
        queue.pop_front();
    }
    queue.push_back(scancode);
}

fn serial_write(msg: &str) {
    serial::write_str(msg);
}

struct KernelShellIo;

impl BareShellIo for KernelShellIo {
    fn next_scancode(&mut self) -> Option<u8> {
        BARE_SHELL_SCANCODES.lock().pop_front()
    }

    fn write_str(&mut self, s: &str) {
        serial_write(s);
    }
}

struct KernelShellFs;

impl KernelShellFs {
    fn creds() -> crate::fs::Credentials {
        session::current_credentials()
    }

    fn map_error(err: KernelFsError) -> BareFsError {
        match err {
            KernelFsError::NotInitialized | KernelFsError::Unsupported => BareFsError::Unavailable,
            KernelFsError::InvalidImage => BareFsError::Corrupt,
            KernelFsError::NotFound => BareFsError::NotFound,
            KernelFsError::NotDirectory => BareFsError::NotDirectory,
            KernelFsError::NotFile => BareFsError::NotFile,
            KernelFsError::PermissionDenied => BareFsError::PermissionDenied,
            KernelFsError::AlreadyExists => BareFsError::AlreadyExists,
            KernelFsError::DirectoryNotEmpty => BareFsError::DirectoryNotEmpty,
        }
    }

    fn convert_entry(entry: fs::DirEntry) -> BareDirEntry {
        let kind = match entry.kind {
            fs::EntryKind::Directory => BareEntryKind::Directory,
            fs::EntryKind::File => BareEntryKind::File,
        };
        BareDirEntry {
            name: entry.name,
            kind,
            size: entry.size,
            mode: entry.mode,
            uid: entry.uid,
            gid: entry.gid,
            inode: entry.inode,
        }
    }
}

impl BareShellFs for KernelShellFs {
    fn list_dir(&self, path: &str) -> Result<Vec<BareDirEntry>, BareFsError> {
        let creds = Self::creds();
        fs::list_dir_with_credentials(path, &creds)
            .map(|entries| entries.into_iter().map(Self::convert_entry).collect())
            .map_err(Self::map_error)
    }

    fn read_file(&self, path: &str) -> Result<Vec<u8>, BareFsError> {
        let creds = Self::creds();
        fs::read_file_with_credentials(path, &creds).map_err(Self::map_error)
    }

    fn create_file(&self, path: &str, mode: u16) -> Result<(), BareFsError> {
        let creds = Self::creds();
        fs::create_file_with_credentials(path, &creds, mode).map_err(Self::map_error)
    }

    fn create_dir(&self, path: &str, mode: u16) -> Result<(), BareFsError> {
        let creds = Self::creds();
        fs::create_dir_with_credentials(path, &creds, mode).map_err(Self::map_error)
    }

    fn remove_file(&self, path: &str) -> Result<(), BareFsError> {
        let creds = Self::creds();
        fs::remove_file_with_credentials(path, &creds).map_err(Self::map_error)
    }

    fn remove_dir(&self, path: &str) -> Result<(), BareFsError> {
        let creds = Self::creds();
        fs::remove_dir_with_credentials(path, &creds).map_err(Self::map_error)
    }

    fn write_file(
        &self,
        path: &str,
        offset: usize,
        data: &[u8],
        truncate: bool,
    ) -> Result<usize, BareFsError> {
        let creds = Self::creds();
        fs::write_file_with_credentials(path, &creds, offset, data, truncate)
            .map_err(Self::map_error)
    }

    fn chmod(&self, path: &str, mode: u16) -> Result<(), BareFsError> {
        let creds = Self::creds();
        fs::chmod_with_credentials(path, &creds, mode).map_err(Self::map_error)
    }

    fn chown(&self, path: &str, uid: u32, gid: u32) -> Result<(), BareFsError> {
        let creds = Self::creds();
        fs::chown_with_credentials(path, &creds, uid, gid).map_err(Self::map_error)
    }
}

struct KernelShellSystem {
    hostname: &'static str,
}

impl KernelShellSystem {
    const fn new(hostname: &'static str) -> Self {
        Self { hostname }
    }

    fn to_identity(profile: users::UserProfile) -> BareUserIdentity {
        BareUserIdentity {
            username: profile.username,
            uid: profile.uid,
            gid: profile.gid,
            groups: profile.groups,
            home: profile.home,
            shell: profile.shell,
        }
    }

    fn current_identity() -> BareUserIdentity {
        let session = session::current_session();
        BareUserIdentity {
            username: session.username,
            uid: session.uid,
            gid: session.gid,
            groups: session.groups,
            home: session.home,
            shell: session.shell,
        }
    }

    fn require_root() -> Result<(), BareUserAdminError> {
        let session = session::current_session();
        if session.uid == 0 {
            Ok(())
        } else {
            Err(BareUserAdminError::PermissionDenied)
        }
    }

    fn map_user_error_admin(err: users::UserError) -> BareUserAdminError {
        match err {
            users::UserError::AlreadyExists => BareUserAdminError::AlreadyExists,
            users::UserError::NotFound => BareUserAdminError::NotFound,
            users::UserError::InvalidPassword => BareUserAdminError::InvalidPassword,
            users::UserError::AuthenticationFailed => BareUserAdminError::PermissionDenied,
        }
    }

    fn map_user_error_auth(err: users::UserError) -> BareAuthError {
        match err {
            users::UserError::NotFound => BareAuthError::NotFound,
            users::UserError::InvalidPassword | users::UserError::AuthenticationFailed => {
                BareAuthError::InvalidPassword
            }
            users::UserError::AlreadyExists => BareAuthError::Unsupported,
        }
    }
}

impl BareShellSystem for KernelShellSystem {
    fn reboot(&self) -> Result<(), BareSystemError> {
        Err(BareSystemError::Unsupported)
    }

    fn shutdown(&self) -> Result<(), BareSystemError> {
        Err(BareSystemError::Unsupported)
    }

    fn exec(
        &self,
        _path: &str,
        _args: &[&str],
        _cwd: &str,
        _env: &[(&str, &str)],
    ) -> Result<BareExecResult, BareSystemError> {
        Err(BareSystemError::Unsupported)
    }

    fn current_user(&self) -> BareUserIdentity {
        Self::current_identity()
    }

    fn authenticate(
        &self,
        username: &str,
        password: &str,
    ) -> Result<BareUserIdentity, BareAuthError> {
        users::authenticate(username, password)
            .map(Self::to_identity)
            .map_err(Self::map_user_error_auth)
    }

    fn set_session(&self, user: &BareUserIdentity) -> Result<BareUserIdentity, BareAuthError> {
        if let Some(profile) = users::get_user(&user.username) {
            session::set_session(&profile);
            Ok(Self::to_identity(profile))
        } else {
            Err(BareAuthError::NotFound)
        }
    }

    fn create_user(
        &self,
        username: &str,
        password: &str,
        home: Option<&str>,
    ) -> Result<BareUserIdentity, BareUserAdminError> {
        Self::require_root()?;
        users::add_user(username, password, home, None)
            .map(Self::to_identity)
            .map_err(Self::map_user_error_admin)
    }

    fn set_password(&self, username: &str, password: &str) -> Result<(), BareUserAdminError> {
        let session = session::current_session();
        if session.uid != 0 && session.username != username {
            return Err(BareUserAdminError::PermissionDenied);
        }
        users::set_password(username, password).map_err(Self::map_user_error_admin)
    }

    fn list_users(&self) -> Result<Vec<BareUserIdentity>, BareUserAdminError> {
        Self::require_root()?;
        Ok(users::list_users()
            .into_iter()
            .map(Self::to_identity)
            .collect())
    }

    fn lookup_user(&self, username: &str) -> Option<BareUserIdentity> {
        users::get_user(username).map(Self::to_identity)
    }

    fn hostname(&self) -> &str {
        self.hostname
    }
}

/// User space fault classification for diagnostics.
#[derive(Clone, Copy, Debug)]
pub enum UserFaultKind {
    /// Page fault triggered from user mode.
    PageFault,
    /// General protection fault triggered from user mode.
    GeneralProtection,
    /// Invalid opcode fault triggered from user mode.
    InvalidOpcode,
    /// Double fault triggered from user mode.
    DoubleFault,
}

#[derive(Clone, Copy, Debug)]
struct UserFaultRecord {
    kind: UserFaultKind,
    rip: u64,
    address: u64,
    code: u64,
}

/// Stores information about the most recent user fault for later diagnostics.
pub fn record_user_fault(kind: UserFaultKind, rip: u64, address: u64, code: u64) {
    let mut guard = LAST_USER_FAULT.lock();
    *guard = Some(UserFaultRecord {
        kind,
        rip,
        address,
        code,
    });
}

fn take_user_fault() -> Option<UserFaultRecord> {
    LAST_USER_FAULT.lock().take()
}

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
    push_bare_shell_scancode(scancode);
    if ENABLE_USER_PROCESSES {
        let mut console = CONSOLE.lock();
        console.push_scancode(scancode);
    }
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
        let fault = take_user_fault();

        if let Some(info) = fault {
            #[cfg(feature = "hardware")]
            serial::write_fmt(format_args!(
                "user fault: kind={:?} pid={} tid={} rip={:#x} addr={:#x} code={:#x}\r\n",
                info.kind,
                entry.pid.as_u64(),
                entry.tid.as_u64(),
                info.rip,
                info.address,
                info.code
            ));

            log::error!(
                "user fault: kind={:?} pid={} tid={} rip={:#x} addr={:#x} code={:#x}",
                info.kind,
                entry.pid.as_u64(),
                entry.tid.as_u64(),
                info.rip,
                info.address,
                info.code
            );
        } else {
            #[cfg(feature = "hardware")]
            serial::write_fmt(format_args!(
                "user fault: pid={} tid={} (details unavailable)\r\n",
                entry.pid.as_u64(),
                entry.tid.as_u64()
            ));

            log::error!(
                "user fault: pid={} tid={} (details unavailable)",
                entry.pid.as_u64(),
                entry.tid.as_u64()
            );
        }

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
    bare_shell: KernelBareShell,
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

        session::reset_to_root();
        let bare_shell = BareShell::new(
            KernelShellIo,
            KernelShellFs,
            KernelShellSystem::new(DEFAULT_HOSTNAME),
        );
        crate::arch::x86_64::serial::write_str("shell: bare shell initialized\r\n");

        Self {
            process_table,
            scheduler,
            init_spawned: false,
            bare_shell,
        }
    }

    fn launch_initial_user(&self) -> Result<(), SubsystemError> {
        serial::write_str("shell: launch_initial_user starting\r\n");
        
        session::reset_to_root();
        serial::write_str("shell: session reset\r\n");
        
        let profile = users::root_profile();
        serial::write_str("shell: got root profile\r\n");
        
        let shell_path = if profile.shell.is_empty() {
            String::from("/bin/sh")
        } else {
            profile.shell.clone()
        };

        serial::write_fmt(format_args!("shell: spawning process for {}\r\n", shell_path));
        trace!("shell: spawning initial user process");
        let process = self.process_table.spawn();
        serial::write_fmt(format_args!("shell: spawn completed pid={}\r\n", process.pid().as_u64()));
        trace!("shell: spawn completed with pid={}", process.pid().as_u64());
        serial::write_str("shell: setting credentials and env\r\n");
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

        serial::write_fmt(format_args!("shell: executing {}\r\n", shell_path));
        trace!("shell: executing {}", shell_path);
        self.process_table.exec(process.pid(), shell_path)?;
        serial::write_str("shell: exec completed\r\n");
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

    fn poll_bare_shell(&mut self) {
        self.bare_shell.poll();
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

        // Disable interrupts before enabling timer to avoid deadlock with serial/scheduler locks
        let interrupts_were_enabled = x86_64::instructions::interrupts::are_enabled();
        x86_64::instructions::interrupts::disable();

        // NOW enable timer after all initialization is done
        Scheduler::enable_timer_global();
        serial::write_str("shell: timer enabled in init\r\n");

        if ENABLE_USER_PROCESSES && !self.init_spawned {
            serial::write_str("shell: launching initial user\r\n");
            
            let result = self.launch_initial_user();
            serial::write_str("shell: launch_initial_user returned\r\n");

            match result {
                Ok(()) => {
                    self.init_spawned = true;
                    serial::write_str("shell: initial user spawned successfully\r\n");
                }
                Err(e) => {
                    serial::write_fmt(format_args!("shell: failed to launch initial user: {:?}\r\n", e));
                    // Re-enable interrupts if they were enabled before
                    if interrupts_were_enabled {
                        x86_64::instructions::interrupts::enable();
                    }
                    return Err(e);
                }
            }
        } else if !ENABLE_USER_PROCESSES && !self.init_spawned {
            serial::write_str("shell: userspace launch disabled; running bare shell only\r\n");
            self.init_spawned = true;
        }

        // Always re-enable interrupts if they were enabled before
        if interrupts_were_enabled {
            x86_64::instructions::interrupts::enable();
        }
        serial::write_str("shell: init complete\r\n");

        Ok(())
    }

    fn tick(&mut self, _ctx: &KernelContext) -> Result<(), SubsystemError> {
        #[cfg(all(not(feature = "std"), not(feature = "hardware")))]
        self.run_ready_threads();
        self.poll_bare_shell();
        Ok(())
    }
}
