//! POSIX compatibility primitives enabling a Unix-like userland.

use alloc::collections::{BTreeMap, VecDeque};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::slice;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::RwLock;

use crate::{
    arch::x86_64::serial,
    error::SubsystemError,
    fs::{self, FsError},
    process::{Pid, ProcessTable, WaitError},
    syscall::SyscallId,
};

/// POSIX errno values we expose to userland.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Errno {
    /// Operation succeeded.
    Success = 0,
    /// Operation not permitted.
    Perm = 1,
    /// No such file or directory.
    NoEnt = 2,
    /// Interrupted function call.
    Intr = 4,
    /// Try again.
    Again = 11,
    /// Bad file descriptor.
    Badf = 9,
    /// No child processes.
    Child = 10,
    /// Not enough memory.
    NoMem = 12,
    /// Invalid argument.
    Inval = 22,
    /// Function not implemented.
    NoImpl = 38,
    /// Not a directory.
    NotDir = 20,
}

const O_RDONLY: u64 = 0;
const O_WRONLY: u64 = 1;
const O_RDWR: u64 = 2;
const O_ACCMODE: u64 = 0b11;
const O_APPEND: u64 = 0o2000;
const O_TRUNC: u64 = 0o1000;

impl Errno {
    /// Converts to i32.
    pub fn as_raw(self) -> i32 {
        self as i32
    }
}

/// POSIX signal representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    /// Termination signal.
    Term = 15,
    /// Kill signal.
    Kill = 9,
    /// Interrupt signal.
    Int = 2,
}

/// POSIX compliance shim bridging syscalls and process table.
pub struct PosixLayer<'a> {
    process_table: &'a ProcessTable,
    program_handles: RwLock<BTreeMap<u64, String>>,
    path_handles: RwLock<BTreeMap<u64, String>>,
    next_handle: AtomicU64,
    pipes: RwLock<BTreeMap<u64, PipeState>>,
}

impl<'a> PosixLayer<'a> {
    /// Wraps the process table.
    pub fn new(process_table: &'a ProcessTable) -> Self {
        Self {
            process_table,
            program_handles: RwLock::new(BTreeMap::new()),
            path_handles: RwLock::new(BTreeMap::new()),
            next_handle: AtomicU64::new(1_000),
            pipes: RwLock::new(BTreeMap::new()),
        }
    }

    /// Dispatches a POSIX syscall.
    pub fn dispatch(&self, pid: Pid, syscall: SyscallId, args: &[u64]) -> Result<u64, Errno> {
        match syscall {
            SyscallId::Fork => self.fork(pid).map(|child| child.as_u64()),
            SyscallId::Exec => {
                let handle = args.get(0).copied().unwrap_or_default();
                let program = self.lookup_program_handle(handle)?;
                self.exec(pid, program).map(|_| 0)
            }
            SyscallId::Write => {
                let fd = args.get(0).copied().unwrap_or_default();
                let buf = args.get(1).copied().unwrap_or_default();
                let len = args.get(2).copied().unwrap_or_default();
                self.write(pid, fd, buf, len)
            }
            SyscallId::Read => {
                let fd = args.get(0).copied().unwrap_or_default();
                let buf = args.get(1).copied().unwrap_or_default();
                let len = args.get(2).copied().unwrap_or_default();
                self.read(pid, fd, buf, len)
            }
            SyscallId::Open => {
                let handle = args.get(0).copied().unwrap_or_default();
                let flags = args.get(1).copied().unwrap_or_default();
                let path = self.lookup_path_handle(handle)?;
                self.open(pid, &path, flags)
            }
            SyscallId::Close => {
                let fd = args.get(0).copied().unwrap_or_default();
                self.close(pid, fd).map(|_| 0)
            }
            SyscallId::Exit => {
                self.exit(pid, args.get(0).copied().unwrap_or_default() as i32);
                Ok(0)
            }
            SyscallId::GetPid => Ok(pid.as_u64()),
            SyscallId::WaitPid => {
                let target = args.get(0).copied().unwrap_or(u64::MAX) as i64;
                let options = args.get(2).copied().unwrap_or_default();
                self.waitpid(pid, target, options)
            }
            SyscallId::Dup => {
                let fd = args.get(0).copied().unwrap_or_default();
                self.dup(pid, fd)
            }
            SyscallId::Dup2 => {
                let fd = args.get(0).copied().unwrap_or_default();
                let new_fd = args.get(1).copied().unwrap_or_default();
                self.dup2(pid, fd, new_fd)
            }
            SyscallId::Pipe => self.pipe(pid),
            SyscallId::Chdir => {
                let handle = args.get(0).copied().unwrap_or_default();
                let path = self.lookup_path_handle(handle)?;
                self.chdir(pid, path)
            }
            SyscallId::GetCwd => self.getcwd(pid),
            SyscallId::Sleep => {
                let millis = args.get(0).copied().unwrap_or_default();
                self.sleep(millis)
            }
            _ => Err(Errno::NoImpl),
        }
    }

    /// Associates a numeric handle with a program path for exec calls.
    pub fn register_program_handle(&self, handle: u64, path: String) {
        self.program_handles.write().insert(handle, path);
    }

    /// Associates a numeric handle with a filesystem path for open.
    pub fn register_path_handle(&self, handle: u64, path: String) {
        self.path_handles.write().insert(handle, path);
    }

    /// Allocates a new handle id for either program or path associations.
    pub fn allocate_handle(&self) -> u64 {
        self.next_handle.fetch_add(1, Ordering::SeqCst)
    }

    fn write(&self, pid: Pid, fd: u64, buf: u64, len: u64) -> Result<u64, Errno> {
        if len == 0 {
            return Ok(0);
        }

        if buf == 0 {
            return Err(Errno::Inval);
        }

        let proc = self.process_table.lookup(pid).ok_or(Errno::NoEnt)?;
        let descriptor = proc.get_fd(fd).ok_or(Errno::Badf)?;
        let data = unsafe { slice::from_raw_parts(buf as *const u8, len as usize) };

        match self.classify_descriptor(&descriptor) {
            Descriptor::StdOut | Descriptor::StdErr => {
                serial::write_bytes(data);
                Ok(len)
            }
            Descriptor::Pipe {
                id,
                end: PipeEnd::Write,
            } => {
                let mut pipes = self.pipes.write();
                let state = pipes.get_mut(&id).ok_or(Errno::Inval)?;
                Ok(state.write(data) as u64)
            }
            Descriptor::Pipe { .. } => Err(Errno::Badf),
            Descriptor::Path => {
                let creds = proc.credentials();
                let offset = proc.fd_offset(fd).unwrap_or(0);
                match fs::write_file_with_credentials(&descriptor, &creds, offset, data, false) {
                    Ok(written) => {
                        proc.advance_fd_offset(fd, written);
                        Ok(written as u64)
                    }
                    Err(FsError::PermissionDenied) => Err(Errno::Perm),
                    Err(FsError::NotFound) => Err(Errno::NoEnt),
                    Err(FsError::NotDirectory) => Err(Errno::NotDir),
                    Err(FsError::NotFile) => Err(Errno::Inval),
                    Err(FsError::Unsupported) => Err(Errno::NoImpl),
                    Err(_) => Err(Errno::NoImpl),
                }
            }
            Descriptor::Unknown => Err(Errno::NoImpl),
            Descriptor::StdIn => Err(Errno::Badf),
        }
    }

    fn read(&self, pid: Pid, fd: u64, buf: u64, len: u64) -> Result<u64, Errno> {
        let proc = self.process_table.lookup(pid).ok_or(Errno::NoEnt)?;
        if len == 0 {
            return Ok(0);
        }

        if buf == 0 {
            return Err(Errno::Inval);
        }

        let descriptor = proc.get_fd(fd).ok_or(Errno::Badf)?;
        let buffer = unsafe { slice::from_raw_parts_mut(buf as *mut u8, len as usize) };

        match self.classify_descriptor(&descriptor) {
            Descriptor::StdIn => Ok(0),
            Descriptor::Pipe {
                id,
                end: PipeEnd::Read,
            } => {
                let mut pipes = self.pipes.write();
                if let Some(state) = pipes.get_mut(&id) {
                    Ok(state.read(buffer) as u64)
                } else {
                    Err(Errno::Inval)
                }
            }
            Descriptor::Pipe { .. } => Err(Errno::Badf),
            Descriptor::StdOut | Descriptor::StdErr => Err(Errno::Badf),
            Descriptor::Path => {
                let creds = proc.credentials();
                match fs::read_file_with_credentials(&descriptor, &creds) {
                    Ok(data) => {
                        let offset = proc.fd_offset(fd).unwrap_or(0);
                        if offset >= data.len() {
                            return Ok(0);
                        }
                        let available = &data[offset..];
                        let to_copy = core::cmp::min(available.len(), buffer.len());
                        buffer[..to_copy].copy_from_slice(&available[..to_copy]);
                        proc.advance_fd_offset(fd, to_copy);
                        Ok(to_copy as u64)
                    }
                    Err(FsError::PermissionDenied) => Err(Errno::Perm),
                    Err(FsError::NotFound) => Err(Errno::NoEnt),
                    Err(FsError::NotDirectory) => Err(Errno::NotDir),
                    Err(FsError::NotFile) => Err(Errno::Inval),
                    Err(_) => Err(Errno::NoImpl),
                }
            }
            Descriptor::Unknown => Err(Errno::NoImpl),
        }
    }

    fn open(&self, pid: Pid, path: &str, flags: u64) -> Result<u64, Errno> {
        let normalized = Self::normalize_path(path);
        let proc = self.process_table.lookup(pid).ok_or(Errno::NoEnt)?;
        let creds = proc.credentials();

        let accmode = flags & O_ACCMODE;
        let require_read = accmode == O_RDONLY || accmode == O_RDWR;
        let require_write = accmode == O_WRONLY || accmode == O_RDWR;

        let mut info = match fs::file_info_with_credentials(
            &normalized,
            &creds,
            require_read,
            require_write,
        ) {
            Ok(info) => info,
            Err(FsError::PermissionDenied) => return Err(Errno::Perm),
            Err(FsError::NotFound) => return Err(Errno::NoEnt),
            Err(FsError::NotDirectory) => return Err(Errno::NotDir),
            Err(FsError::NotFile) => return Err(Errno::Inval),
            Err(FsError::Unsupported) => return Err(Errno::NoImpl),
            Err(_) => return Err(Errno::NoImpl),
        };

        if (flags & O_TRUNC) != 0 {
            match fs::truncate_file_with_credentials(&normalized, &creds) {
                Ok(()) => info.size = 0,
                Err(FsError::PermissionDenied) => return Err(Errno::Perm),
                Err(FsError::NotFound) => return Err(Errno::NoEnt),
                Err(FsError::NotFile) => return Err(Errno::Inval),
                Err(FsError::Unsupported) => return Err(Errno::NoImpl),
                Err(_) => return Err(Errno::NoImpl),
            }
        }

        let fd = self
            .process_table
            .open(pid, normalized.clone())
            .map_err(Errno::from_subsystem)?;

        if (flags & O_APPEND) != 0 {
            let capped = core::cmp::min(info.size, usize::MAX as u64) as usize;
            proc.set_fd_offset(fd, capped);
        } else {
            proc.set_fd_offset(fd, 0);
        }

        Ok(fd)
    }

    fn close(&self, pid: Pid, fd: u64) -> Result<(), Errno> {
        let descriptor = self.descriptor_for(pid, fd).ok_or(Errno::Badf)?;
        self.process_table.close(pid, fd).map_err(|err| match err {
            SubsystemError::Runtime("fd not found") => Errno::Badf,
            other => Errno::from_subsystem(other),
        })?;
        self.release_descriptor(&descriptor);
        Ok(())
    }

    fn exit(&self, pid: Pid, status: i32) {
        self.process_table.mark_exit(pid, status);
    }

    fn fork(&self, pid: Pid) -> Result<Pid, Errno> {
        self.process_table.fork(pid).map_err(Errno::from_subsystem)
    }

    fn exec(&self, pid: Pid, program: String) -> Result<(), Errno> {
        self.process_table
            .exec(pid, program)
            .map_err(Errno::from_subsystem)
    }

    fn lookup_program_handle(&self, handle: u64) -> Result<String, Errno> {
        self.program_handles
            .read()
            .get(&handle)
            .cloned()
            .ok_or(Errno::NoEnt)
    }

    fn lookup_path_handle(&self, handle: u64) -> Result<String, Errno> {
        self.path_handles
            .read()
            .get(&handle)
            .cloned()
            .ok_or(Errno::NoEnt)
    }

    fn waitpid(&self, parent: Pid, child_raw: i64, _options: u64) -> Result<u64, Errno> {
        let target = if child_raw <= 0 {
            None
        } else {
            Some(Pid::new(child_raw as u64))
        };
        match self.process_table.wait_pid(parent, target) {
            Ok((pid, status)) => {
                // For now, we return the child PID and encode the status in the upper bits.
                Ok(((status as u64) << 32) | pid.as_u64())
            }
            Err(WaitError::NoChildren) => Err(Errno::Child),
            Err(WaitError::NotChild) => Err(Errno::Child),
            Err(WaitError::ChildRunning) => Err(Errno::Again),
        }
    }

    fn dup(&self, pid: Pid, fd: u64) -> Result<u64, Errno> {
        let descriptor = self.descriptor_for(pid, fd).ok_or(Errno::Badf)?;
        let new_fd = self
            .process_table
            .dup(pid, fd)
            .map_err(Errno::from_subsystem)? as u64;
        self.retain_descriptor(&descriptor);
        Ok(new_fd)
    }

    fn dup2(&self, pid: Pid, fd: u64, new_fd: u64) -> Result<u64, Errno> {
        if fd == new_fd {
            return Ok(new_fd);
        }

        let source = self.descriptor_for(pid, fd).ok_or(Errno::Badf)?;
        let existing = self.descriptor_for(pid, new_fd);

        let result = self
            .process_table
            .dup2(pid, fd, new_fd)
            .map_err(Errno::from_subsystem)? as u64;

        if let Some(old) = existing {
            self.release_descriptor(&old);
        }
        self.retain_descriptor(&source);
        Ok(result)
    }

    fn pipe(&self, pid: Pid) -> Result<u64, Errno> {
        let (read_fd, write_fd, pipe_id) = self
            .process_table
            .pipe(pid)
            .map_err(Errno::from_subsystem)?;
        self.pipes.write().insert(pipe_id, PipeState::new());
        Ok(((write_fd as u64) << 32) | (read_fd as u64))
    }

    fn chdir(&self, pid: Pid, path: String) -> Result<u64, Errno> {
        let normalized = Self::normalize_path(&path);
        let proc = self.process_table.lookup(pid).ok_or(Errno::NoEnt)?;
        let creds = proc.credentials();
        match fs::list_dir_with_credentials(&normalized, &creds) {
            Ok(_) => self
                .process_table
                .chdir(pid, normalized)
                .map(|_| 0)
                .map_err(Errno::from_subsystem),
            Err(FsError::PermissionDenied) => Err(Errno::Perm),
            Err(FsError::NotFound) => Err(Errno::NoEnt),
            Err(FsError::NotDirectory) => Err(Errno::NotDir),
            Err(_) => Err(Errno::NoImpl),
        }
    }

    fn getcwd(&self, pid: Pid) -> Result<u64, Errno> {
        let cwd = self
            .process_table
            .getcwd(pid)
            .map_err(Errno::from_subsystem)?;
        let handle = self.allocate_handle();
        self.register_path_handle(handle, cwd);
        Ok(handle)
    }

    fn sleep(&self, millis: u64) -> Result<u64, Errno> {
        // Placeholder: real implementation would integrate with a timer scheduler.
        if millis == 0 {
            return Ok(0);
        }
        for _ in 0..millis {
            core::hint::spin_loop();
        }
        Ok(0)
    }

    /// Normalizes POSIX path.
    pub fn normalize_path(path: &str) -> String {
        let mut parts = Vec::new();
        for part in path.split('/') {
            match part {
                "" | "." => {}
                ".." => {
                    parts.pop();
                }
                other => parts.push(other),
            }
        }
        format!("/{}", parts.join("/"))
    }
}

impl Errno {
    fn from_subsystem(err: SubsystemError) -> Self {
        match err {
            SubsystemError::Init(_) => Errno::NoImpl,
            SubsystemError::Runtime(_) => Errno::Inval,
            SubsystemError::Resource(_) => Errno::NoMem,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PipeEnd {
    Read,
    Write,
}

#[derive(Debug)]
enum Descriptor {
    StdIn,
    StdOut,
    StdErr,
    Pipe { id: u64, end: PipeEnd },
    Path,
    Unknown,
}

#[derive(Default)]
struct PipeState {
    buffer: VecDeque<u8>,
    readers: usize,
    writers: usize,
}

impl PipeState {
    fn new() -> Self {
        Self {
            buffer: VecDeque::new(),
            readers: 1,
            writers: 1,
        }
    }

    fn write(&mut self, data: &[u8]) -> usize {
        for byte in data {
            self.buffer.push_back(*byte);
        }
        data.len()
    }

    fn read(&mut self, out: &mut [u8]) -> usize {
        let mut count = 0;
        while count < out.len() {
            match self.buffer.pop_front() {
                Some(byte) => {
                    out[count] = byte;
                    count += 1;
                }
                None => break,
            }
        }
        count
    }

    fn retain(&mut self, end: PipeEnd) {
        match end {
            PipeEnd::Read => self.readers += 1,
            PipeEnd::Write => self.writers += 1,
        }
    }

    fn release(&mut self, end: PipeEnd) {
        match end {
            PipeEnd::Read => {
                self.readers = self.readers.saturating_sub(1);
            }
            PipeEnd::Write => {
                self.writers = self.writers.saturating_sub(1);
            }
        }
    }

    fn is_orphaned(&self) -> bool {
        self.readers == 0 && self.writers == 0
    }
}

impl<'a> PosixLayer<'a> {
    fn descriptor_for(&self, pid: Pid, fd: u64) -> Option<String> {
        self.process_table
            .lookup(pid)
            .and_then(|proc| proc.get_fd(fd))
    }

    fn classify_descriptor(&self, descriptor: &str) -> Descriptor {
        match descriptor {
            "tty:stdin" => Descriptor::StdIn,
            "tty:stdout" => Descriptor::StdOut,
            "tty:stderr" => Descriptor::StdErr,
            other => {
                if let Some(pipe) = other.strip_prefix("pipe:") {
                    let mut parts = pipe.split(':');
                    if let (Some(id), Some(end)) = (parts.next(), parts.next()) {
                        if let Ok(pipe_id) = id.parse::<u64>() {
                            return match end {
                                "r" => Descriptor::Pipe {
                                    id: pipe_id,
                                    end: PipeEnd::Read,
                                },
                                "w" => Descriptor::Pipe {
                                    id: pipe_id,
                                    end: PipeEnd::Write,
                                },
                                _ => Descriptor::Unknown,
                            };
                        }
                    }
                }
                if descriptor.is_empty() {
                    Descriptor::Unknown
                } else {
                    Descriptor::Path
                }
            }
        }
    }

    fn retain_descriptor(&self, descriptor: &str) {
        if let Descriptor::Pipe { id, end } = self.classify_descriptor(descriptor) {
            if let Some(state) = self.pipes.write().get_mut(&id) {
                state.retain(end);
            }
        }
    }

    fn release_descriptor(&self, descriptor: &str) {
        if let Descriptor::Pipe { id, end } = self.classify_descriptor(descriptor) {
            let mut pipes = self.pipes.write();
            if let Some(state) = pipes.get_mut(&id) {
                state.release(end);
                if state.is_orphaned() {
                    pipes.remove(&id);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::{Pid, ProcessTable};
    use alloc::boxed::Box;

    fn setup_layer() -> (&'static ProcessTable, PosixLayer<'static>, Pid) {
        let table = ProcessTable::new();
        let pid = table.spawn().pid();
        // SAFETY: leak the table for the duration of the test process.
        let static_table: &'static ProcessTable = Box::leak(Box::new(table));
        let layer = PosixLayer::new(static_table);
        (static_table, layer, pid)
    }

    #[test]
    fn dup_and_dup2_roundtrip() {
        let (table, layer, pid) = setup_layer();
        let fd = table.open(pid, "/tmp/file".into()).unwrap();
        let dup_fd = layer.dup(pid, fd).unwrap();
        assert_ne!(dup_fd, fd);
        let target_fd = fd + 5;
        let target = layer.dup2(pid, fd, target_fd).unwrap();
        assert_eq!(target, target_fd);
    }

    #[test]
    fn pipe_returns_two_descriptors() {
        let (_, layer, pid) = setup_layer();
        let packed = layer.pipe(pid).unwrap();
        let read_fd = (packed & 0xffff_ffff) as u32;
        let write_fd = (packed >> 32) as u32;
        assert_ne!(read_fd, write_fd);
        assert!(read_fd >= 3);
        assert!(write_fd > read_fd);
    }

    #[test]
    fn getcwd_registers_handle() {
        let (_, layer, pid) = setup_layer();
        let initial = layer.getcwd(pid).unwrap();
        let path = layer.lookup_path_handle(initial).unwrap();
        assert_eq!(path, "/");
    }

    #[test]
    fn pipe_write_then_read_roundtrip() {
        let (_, layer, pid) = setup_layer();
        let packed = layer.pipe(pid).unwrap();
        let read_fd = (packed & 0xffff_ffff) as u64;
        let write_fd = (packed >> 32) as u64;

        let payload = b"abc";
        let written = layer
            .write(pid, write_fd, payload.as_ptr() as u64, payload.len() as u64)
            .unwrap();
        assert_eq!(written, payload.len() as u64);

        let mut buffer = [0u8; 8];
        let read = layer
            .read(
                pid,
                read_fd,
                buffer.as_mut_ptr() as u64,
                payload.len() as u64,
            )
            .unwrap();
        assert_eq!(read, payload.len() as u64);
        assert_eq!(&buffer[..payload.len()], payload);
    }

    #[test]
    fn write_to_stdout_accepts_data() {
        let (_, layer, pid) = setup_layer();
        let message = b"hi";
        let written = layer
            .write(pid, 1, message.as_ptr() as u64, message.len() as u64)
            .unwrap();
        assert_eq!(written, message.len() as u64);
    }
}
