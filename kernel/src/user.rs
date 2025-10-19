//! User mode scaffolding: address space layout, stack management, and trap context.

use alloc::vec::Vec;
use core::fmt;

use crate::elf::{ExecutableImage, ExecutableSegment, SegmentFlags};

const PAGE_SIZE: u64 = 4096;
const DEFAULT_STACK_TOP: u64 = 0x0000_7FFF_F000;
const DEFAULT_STACK_SIZE: usize = 128 * 1024;
const USER_RFLAGS: u64 = 0x0000_0000_0000_0202;

bitflags::bitflags! {
    /// Access flags for user mappings.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct MemoryFlags: u8 {
        /// Mapping is readable from user mode.
        const READ = 1 << 0;
        /// Mapping is writable from user mode.
        const WRITE = 1 << 1;
        /// Mapping is executable from user mode.
        const EXEC = 1 << 2;
        /// Mapping is user accessible (as opposed to kernel only).
        const USER = 1 << 3;
    }
}

impl MemoryFlags {
    fn from_segment(flags: SegmentFlags) -> Self {
        let mut result = MemoryFlags::USER;
        if flags.readable {
            result |= MemoryFlags::READ;
        }
        if flags.writable {
            result |= MemoryFlags::WRITE;
        }
        if flags.executable {
            result |= MemoryFlags::EXEC;
        }
        result
    }
}

/// A single ELF segment prepared for mapping into an address space.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SegmentMapping {
    base: u64,
    length: usize,
    permissions: MemoryFlags,
    payload: Vec<u8>,
}

impl SegmentMapping {
    /// Virtual start address of the segment.
    pub fn base(&self) -> u64 {
        self.base
    }

    /// Length of the segment in bytes (already zero-extended to memsz).
    pub fn length(&self) -> usize {
        self.length
    }

    /// Access permissions for the segment.
    pub fn permissions(&self) -> MemoryFlags {
        self.permissions
    }

    /// Raw payload that should be copied into the mapped region.
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }
}

/// Stack layout for a user process.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stack {
    base: u64,
    size: usize,
    permissions: MemoryFlags,
}

impl Stack {
    /// Creates a new stack with the provided top address and size.
    pub fn new(top: u64, size: usize) -> Self {
        Self::with_permissions(
            top,
            size,
            MemoryFlags::READ | MemoryFlags::WRITE | MemoryFlags::USER,
        )
    }

    /// Creates a new stack with custom permissions.
    pub fn with_permissions(top: u64, size: usize, permissions: MemoryFlags) -> Self {
        assert!(size > 0, "stack size must be non-zero");
        let aligned_top = align_up(top, PAGE_SIZE);
        let aligned_size = align_up(size as u64, PAGE_SIZE) as usize;
        assert!(
            aligned_top >= aligned_size as u64,
            "stack top must exceed size"
        );
        let base = aligned_top - aligned_size as u64;
        Self {
            base,
            size: aligned_size,
            permissions,
        }
    }

    /// Stack base address (lowest canonical address).
    pub fn base(&self) -> u64 {
        self.base
    }

    /// Stack size in bytes.
    pub fn size(&self) -> usize {
        self.size
    }

    /// Address where the stack pointer should start.
    pub fn top(&self) -> u64 {
        self.base + self.size as u64
    }

    /// Permissions configured for the stack mapping.
    pub fn permissions(&self) -> MemoryFlags {
        self.permissions
    }
}

/// Configuration used when constructing an address space for a process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StackConfig {
    top: u64,
    size: usize,
    permissions: MemoryFlags,
}

impl StackConfig {
    /// Creates a new stack configuration.
    pub fn new(top: u64, size: usize) -> Self {
        Self {
            top,
            size,
            permissions: MemoryFlags::READ | MemoryFlags::WRITE | MemoryFlags::USER,
        }
    }

    /// Returns the stack top address.
    pub fn top(self) -> u64 {
        self.top
    }

    /// Returns the stack size in bytes.
    pub fn size(self) -> usize {
        self.size
    }

    /// Returns the configured stack permissions.
    pub fn permissions(self) -> MemoryFlags {
        self.permissions
    }

    /// Overrides the default stack permissions.
    pub fn with_permissions(mut self, permissions: MemoryFlags) -> Self {
        self.permissions = permissions;
        self
    }
}

impl Default for StackConfig {
    fn default() -> Self {
        Self::new(DEFAULT_STACK_TOP, DEFAULT_STACK_SIZE)
    }
}

/// Address space description built from an executable image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddressSpace {
    entry_point: u64,
    segments: Vec<SegmentMapping>,
    stack: Stack,
}

impl AddressSpace {
    /// Builds an address space from an ELF image and stack configuration.
    pub fn from_executable(image: &ExecutableImage, stack_config: StackConfig) -> Self {
        let mut segments: Vec<SegmentMapping> = image
            .segments()
            .iter()
            .map(|segment| segment_mapping(segment))
            .collect();
        segments.sort_by_key(|segment| segment.base);
        let stack_permissions =
            stack_config.permissions() | MemoryFlags::from_segment(image.stack_flags());
        let stack =
            Stack::with_permissions(stack_config.top(), stack_config.size(), stack_permissions);
        Self {
            entry_point: image.entry_point(),
            segments,
            stack,
        }
    }

    /// Program entry point virtual address.
    pub fn entry_point(&self) -> u64 {
        self.entry_point
    }

    /// Returns the stack metadata.
    pub fn stack(&self) -> &Stack {
        &self.stack
    }

    /// Returns an iterator over the segment mappings.
    pub fn segments(&self) -> &[SegmentMapping] {
        &self.segments
    }
}

fn segment_mapping(segment: &ExecutableSegment) -> SegmentMapping {
    let payload = segment.data.clone();
    SegmentMapping {
        base: segment.virtual_addr,
        length: segment.mem_size,
        permissions: MemoryFlags::from_segment(segment.flags),
        payload,
    }
}

fn align_up(value: u64, align: u64) -> u64 {
    if value % align == 0 {
        value
    } else {
        value + (align - (value % align))
    }
}

/// Snapshot of general-purpose registers captured during a user->kernel transition.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrapFrame {
    /// System call number / return value.
    pub rax: u64,
    /// Callee-saved buffer registers.
    pub rbx: u64,
    /// Register used for fourth syscall argument.
    pub rcx: u64,
    /// Register used for third syscall argument.
    pub rdx: u64,
    /// Register used for second syscall argument.
    pub rsi: u64,
    /// Register used for first syscall argument.
    pub rdi: u64,
    /// Register used for fifth syscall argument.
    pub r8: u64,
    /// Register used for sixth syscall argument.
    pub r9: u64,
    /// Register used for fourth syscall argument (per x86-64 ABI).
    pub r10: u64,
    /// Register used for clobbered syscall scratch state.
    pub r11: u64,
    /// Callee-saved register snapshot.
    pub r12: u64,
    /// Callee-saved register snapshot.
    pub r13: u64,
    /// Callee-saved register snapshot.
    pub r14: u64,
    /// Callee-saved register snapshot.
    pub r15: u64,
    /// Base pointer.
    pub rbp: u64,
    /// Stack pointer at trap time.
    pub rsp: u64,
    /// Instruction pointer where execution will resume.
    pub rip: u64,
    /// Saved RFLAGS value.
    pub rflags: u64,
}

impl TrapFrame {
    /// Sets the return value that will be observed by user space.
    pub fn set_return_value(&mut self, value: u64) {
        self.rax = value;
    }
}

/// User context tracking the trap frame and metadata needed to resume execution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserContext {
    frame: TrapFrame,
}

impl UserContext {
    /// Creates a fresh context for a new process pointing at the entry point and stack top.
    pub fn for_entry(entry_point: u64, stack_top: u64) -> Self {
        let mut frame = TrapFrame::default();
        frame.rip = entry_point;
        frame.rsp = stack_top;
        frame.rflags = USER_RFLAGS;
        Self { frame }
    }

    /// Builds a user context from a captured trap frame snapshot.
    pub fn from_trap_frame(frame: TrapFrame) -> Self {
        Self { frame }
    }

    /// Returns the trap frame snapshot.
    pub fn frame(&self) -> &TrapFrame {
        &self.frame
    }

    /// Returns a mutable reference to the trap frame snapshot.
    pub fn frame_mut(&mut self) -> &mut TrapFrame {
        &mut self.frame
    }
}

impl fmt::Display for UserContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "UserContext[rip={:#x}, rsp={:#x}]",
            self.frame.rip, self.frame.rsp
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn address_space_from_executable_maps_segments() {
        let segments = vec![ExecutableSegment {
            virtual_addr: 0x400000,
            mem_size: 16,
            file_size: 16,
            align: 0x1000,
            data: vec![0x90; 16],
            flags: SegmentFlags {
                readable: true,
                writable: false,
                executable: true,
            },
        }];
        let image = ExecutableImage::from_parts(0x401000, segments).expect("image valid");
        let layout = AddressSpace::from_executable(&image, StackConfig::default());
        assert_eq!(layout.entry_point(), 0x401000);
        assert_eq!(layout.segments().len(), 1);
        let segment = &layout.segments()[0];
        assert_eq!(segment.base(), 0x400000);
        assert_eq!(segment.length(), 16);
        assert!(segment.permissions().contains(MemoryFlags::READ));
        assert!(segment.permissions().contains(MemoryFlags::EXEC));
        assert!(!segment.permissions().contains(MemoryFlags::WRITE));
        assert_eq!(
            layout.stack().top(),
            Stack::new(DEFAULT_STACK_TOP, DEFAULT_STACK_SIZE).top()
        );
        let stack_perms = layout.stack().permissions();
        assert!(stack_perms.contains(MemoryFlags::READ));
        assert!(stack_perms.contains(MemoryFlags::WRITE));
        assert!(!stack_perms.contains(MemoryFlags::EXEC));
    }

    #[test]
    fn user_context_initializes_frame() {
        let context = UserContext::for_entry(0xdead_beef, 0x7fff_ffff_ff00);
        let frame = context.frame();
        assert_eq!(frame.rip, 0xdead_beef);
        assert_eq!(frame.rsp, 0x7fff_ffff_ff00);
        assert_eq!(frame.rflags, USER_RFLAGS);
    }

    #[test]
    fn trap_frame_return_value_updates_rax() {
        let mut frame = TrapFrame::default();
        frame.set_return_value(42);
        assert_eq!(frame.rax, 42);
    }
}
