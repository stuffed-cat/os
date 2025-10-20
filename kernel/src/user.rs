//! User mode scaffolding: address space layout, stack management, and trap context.

use alloc::vec;
use alloc::{format, string::String, vec::Vec};
use core::fmt;

use crate::{
    elf::{ExecutableImage, ExecutableSegment, SegmentFlags},
    error::SubsystemError,
};

const PAGE_SIZE: u64 = 4096;
const DEFAULT_STACK_TOP: u64 = 0x0000_7FFF_F000;
const DEFAULT_STACK_SIZE: usize = 128 * 1024;
const USER_RFLAGS: u64 = 0x0000_0000_0000_0202;

const AUXV_AT_NULL: u64 = 0;
const AUXV_AT_PHDR: u64 = 3;
const AUXV_AT_PHENT: u64 = 4;
const AUXV_AT_PHNUM: u64 = 5;
const AUXV_AT_PAGESZ: u64 = 6;
const AUXV_AT_BASE: u64 = 7;
const AUXV_AT_ENTRY: u64 = 9;
const AUXV_AT_UID: u64 = 11;
const AUXV_AT_EUID: u64 = 12;
const AUXV_AT_GID: u64 = 13;
const AUXV_AT_EGID: u64 = 14;
const AUXV_AT_CLKTCK: u64 = 17;
const AUXV_AT_SECURE: u64 = 23;
const AUXV_AT_RANDOM: u64 = 25;
const AUXV_AT_EXECFN: u64 = 31;

const DEFAULT_PROGRAM_BASE: u64 = 0x0040_0000;
const INTERPRETER_BASE_START: u64 = 0x0200_0000;
const LOAD_GAP: u64 = 0x0020_0000;

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
    initial_sp: u64,
    image: Option<StackImage>,
}

/// Serialized payload describing bytes to copy into a stack mapping.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StackImage {
    offset: usize,
    data: Vec<u8>,
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
            initial_sp: aligned_top,
            image: None,
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

    /// Returns the initial stack pointer configured for the process.
    pub fn initial_sp(&self) -> u64 {
        self.initial_sp
    }

    /// Returns the prepared stack image, if any.
    pub fn image(&self) -> Option<&StackImage> {
        self.image.as_ref()
    }

    /// Updates the initial stack contents and starting pointer.
    pub fn set_initial_state(&mut self, sp: u64, image: Option<StackImage>) {
        let mapped_top = self.base + self.size as u64;
        assert!(
            sp >= self.base && sp <= mapped_top,
            "stack pointer out of range"
        );
        if let Some(ref img) = image {
            assert!(img.offset <= self.size, "stack image offset outside range");
            assert!(
                img.offset + img.data.len() <= self.size,
                "stack image exceeds stack bounds"
            );
        }
        self.initial_sp = sp;
        self.image = image;
    }
}

impl StackImage {
    /// Starting offset from the stack base where data should be copied.
    pub fn offset(&self) -> usize {
        self.offset
    }

    /// Returns the raw bytes that should be copied into the stack.
    pub fn data(&self) -> &[u8] {
        &self.data
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

/// Creates a TLS segment mapping from a TLS template.
fn create_tls_segment(tls: &crate::elf::TlsTemplate, base_addr: u64) -> SegmentMapping {
    // Calculate total TLS size with proper alignment
    let align = tls.align.max(16); // At least 16-byte alignment
    let aligned_base = (base_addr + align - 1) & !(align - 1);
    
    // TLS layout: [initialized data] [zero-filled bss (memsz - data.len())]
    let total_size = tls.mem_size;
    
    // Round up to page boundary
    let segment_size = ((total_size as u64 + PAGE_SIZE - 1) / PAGE_SIZE) * PAGE_SIZE;
    
    // Prepare payload: copy initial data and zero-extend to mem_size
    let mut payload = tls.data.clone();
    payload.resize(total_size, 0);
    
    // TLS is read-write, not executable
    let flags = MemoryFlags::READ | MemoryFlags::WRITE | MemoryFlags::USER;
    
    SegmentMapping {
        base: aligned_base,
        length: segment_size as usize,
        permissions: flags,
        payload,
    }
}

/// Address space description built from an executable image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddressSpace {
    entry_point: u64,
    segments: Vec<SegmentMapping>,
    stack: Stack,
    tls_template: Option<crate::elf::TlsTemplate>,
    tls_base: Option<u64>,
}

impl AddressSpace {
    /// Builds an address space from an ELF image and stack configuration.
    pub fn from_executable(image: &ExecutableImage, stack_config: StackConfig) -> Self {
        Self::build(image, None, stack_config).space
    }

    /// Builds an address space that maps both the primary executable and its interpreter.
    pub fn from_executable_pair(
        program: &ExecutableImage,
        interpreter: &ExecutableImage,
        stack_config: StackConfig,
    ) -> Self {
        Self::build(program, Some(interpreter), stack_config).space
    }

    /// Program entry point virtual address.
    pub fn entry_point(&self) -> u64 {
        self.entry_point
    }

    /// Returns the stack metadata.
    pub fn stack(&self) -> &Stack {
        &self.stack
    }

    /// Returns a mutable reference to the stack metadata.
    pub fn stack_mut(&mut self) -> &mut Stack {
        &mut self.stack
    }

    /// Returns an iterator over the segment mappings.
    pub fn segments(&self) -> &[SegmentMapping] {
        &self.segments
    }

    /// Returns the TLS template if present.
    pub fn tls_template(&self) -> Option<&crate::elf::TlsTemplate> {
        self.tls_template.as_ref()
    }

    /// Returns the TLS base address if allocated.
    pub fn tls_base(&self) -> Option<u64> {
        self.tls_base
    }

    /// Sets TLS information for this address space.
    pub fn set_tls(&mut self, template: crate::elf::TlsTemplate, base: u64) {
        self.tls_template = Some(template);
        self.tls_base = Some(base);
    }

    /// Builds an address space and returns both the mapping and layout metadata.
    pub fn build(
        program: &ExecutableImage,
        interpreter: Option<&ExecutableImage>,
        stack_config: StackConfig,
    ) -> AddressSpaceBuild {
        let stack_size = stack_config.size();
        let program_placement = layout_image(program, DEFAULT_PROGRAM_BASE);
        let program_layout = ProgramLayoutInfo::from_image(program, &program_placement, stack_size);
        let program_entry = program_placement.entry;
        let program_end = program_placement.end;
        let program_segments = program_placement.segments.clone();

        // Choose TLS from interpreter if present, otherwise from program
        let tls_image = interpreter
            .and_then(|i| i.tls_template())
            .or_else(|| program.tls_template());

        match interpreter {
            Some(interp) => {
                let combined_flags =
                    merge_segment_flags(program.stack_flags(), interp.stack_flags());
                let preferred_interpreter_start =
                    core::cmp::max(program_end.saturating_add(LOAD_GAP), INTERPRETER_BASE_START);
                let interpreter_placement = layout_image(interp, preferred_interpreter_start);
                let interpreter_layout =
                    ProgramLayoutInfo::from_image(interp, &interpreter_placement, stack_size);
                let interpreter_entry = interpreter_placement.entry;
                let mut segments = program_segments.clone();
                segments.extend(interpreter_placement.segments.clone());
                
                // Add TLS segment if present
                let tls_base = if let Some(tls) = tls_image {
                    let tls_base = interpreter_placement.end.saturating_add(PAGE_SIZE);
                    let tls_segment = create_tls_segment(tls, tls_base);
                    segments.push(tls_segment);
                    Some(tls_base)
                } else {
                    None
                };
                
                segments.sort_by_key(|segment| segment.base());
                let mut space = Self::finish_address_space(
                    interpreter_entry,
                    segments,
                    combined_flags,
                    stack_config,
                );
                
                // Set TLS information
                if let (Some(tls), Some(base)) = (tls_image, tls_base) {
                    space.set_tls(tls.clone(), base);
                }
                
                AddressSpaceBuild {
                    space,
                    program: program_layout,
                    interpreter: Some(interpreter_layout),
                }
            }
            None => {
                let mut segments = program_segments;
                
                // Add TLS segment if present
                let tls_base = if let Some(tls) = tls_image {
                    let tls_base = program_end.saturating_add(PAGE_SIZE);
                    let tls_segment = create_tls_segment(tls, tls_base);
                    segments.push(tls_segment);
                    Some(tls_base)
                } else {
                    None
                };
                
                segments.sort_by_key(|segment| segment.base());
                let mut space = Self::finish_address_space(
                    program_entry,
                    segments,
                    program.stack_flags(),
                    stack_config,
                );
                
                // Set TLS information
                if let (Some(tls), Some(base)) = (tls_image, tls_base) {
                    space.set_tls(tls.clone(), base);
                }
                
                AddressSpaceBuild {
                    space,
                    program: program_layout,
                    interpreter: None,
                }
            }
        }
    }
}

/// Describes key layout attributes for a loaded executable image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProgramLayoutInfo {
    base_address: u64,
    entry_point: u64,
    stack_size: usize,
    program_headers: Option<ProgramHeaderTable>,
}

impl ProgramLayoutInfo {
    fn from_image(image: &ExecutableImage, placement: &ImagePlacement, stack_size: usize) -> Self {
        let base_address = placement
            .segments
            .iter()
            .map(SegmentMapping::base)
            .min()
            .unwrap_or(0);
        let program_headers = image
            .program_header_virtual_address(placement.bias)
            .map(|address| ProgramHeaderTable {
                address,
                entry_size: image.program_header_entry_size(),
                count: image.program_header_count(),
            });
        Self {
            base_address,
            entry_point: placement.entry,
            stack_size,
            program_headers,
        }
    }

    /// Base virtual address at which the executable was loaded.
    pub fn base_address(&self) -> u64 {
        self.base_address
    }

    /// Entry point virtual address after relocation.
    pub fn entry_point(&self) -> u64 {
        self.entry_point
    }

    /// Size of the stack reserved for this image.
    pub fn stack_size(&self) -> usize {
        self.stack_size
    }

    /// Program header metadata if available.
    pub fn program_headers(&self) -> Option<&ProgramHeaderTable> {
        self.program_headers.as_ref()
    }
}

/// Captures the program header table location exposed to user space.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProgramHeaderTable {
    /// Virtual address of the first program header entry.
    pub address: u64,
    /// Size in bytes of each program header entry.
    pub entry_size: u16,
    /// Number of entries present in the table.
    pub count: u16,
}

/// Result of building an address space, including metadata useful for process setup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddressSpaceBuild {
    /// Fully prepared address space mapping.
    pub space: AddressSpace,
    /// Layout information for the primary executable.
    pub program: ProgramLayoutInfo,
    /// Layout information for the interpreter, if present.
    pub interpreter: Option<ProgramLayoutInfo>,
}

/// Effective user and group identifiers exposed to user space via auxv entries.
#[derive(Clone, Copy, Debug)]
pub struct StackUserIds {
    /// Real user identifier.
    pub uid: u32,
    /// Real group identifier.
    pub gid: u32,
    /// Effective user identifier.
    pub euid: u32,
    /// Effective group identifier.
    pub egid: u32,
}

/// Result of preparing the initial stack image for a freshly exec'd process.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StackInitialization {
    /// Stack pointer where user execution should begin.
    pub stack_pointer: u64,
    /// Optional byte payload that must be copied into the stack mapping.
    pub image: Option<StackImage>,
}

/// Builds argc/argv/envp/auxv layout for a new process stack.
pub fn prepare_initial_stack(
    stack: &Stack,
    argv: &[String],
    env: &[(String, String)],
    program_layout: &ProgramLayoutInfo,
    interpreter_layout: Option<&ProgramLayoutInfo>,
    entry_point: u64,
    ids: StackUserIds,
    exec_path: &str,
) -> Result<StackInitialization, SubsystemError> {
    let mut builder = StackInitializer::new(stack);

    let mut argv_pointers = Vec::with_capacity(argv.len());
    for arg in argv {
        let ptr = builder.push_cstring(arg).map_err(SubsystemError::from)?;
        argv_pointers.push(ptr);
    }

    let mut env_pointers = Vec::with_capacity(env.len());
    for (key, value) in env {
        let entry = format!("{}={}", key, value);
        let ptr = builder.push_cstring(&entry).map_err(SubsystemError::from)?;
        env_pointers.push(ptr);
    }

    let execfn_ptr = builder
        .push_cstring(exec_path)
        .map_err(SubsystemError::from)?;

    let random_seed = entry_point ^ stack.top();
    let random_bytes = pseudo_random_bytes(random_seed);
    let random_ptr = builder
        .push_bytes(&random_bytes, 16)
        .map_err(SubsystemError::from)?;

    builder.align_down(16).map_err(SubsystemError::from)?;

    let (phdr, phent, phnum) = if let Some(headers) = program_layout.program_headers.as_ref() {
        (
            headers.address,
            u64::from(headers.entry_size),
            u64::from(headers.count),
        )
    } else {
        (0, 0, 0)
    };

    let mut auxv = Vec::new();
    auxv.push((AUXV_AT_PHDR, phdr));
    auxv.push((AUXV_AT_PHENT, phent));
    auxv.push((AUXV_AT_PHNUM, phnum));
    auxv.push((AUXV_AT_PAGESZ, PAGE_SIZE));
    if let Some(layout) = interpreter_layout {
        auxv.push((AUXV_AT_BASE, layout.base_address));
    }
    auxv.push((AUXV_AT_ENTRY, entry_point));
    auxv.push((AUXV_AT_UID, ids.uid as u64));
    auxv.push((AUXV_AT_EUID, ids.euid as u64));
    auxv.push((AUXV_AT_GID, ids.gid as u64));
    auxv.push((AUXV_AT_EGID, ids.egid as u64));
    auxv.push((AUXV_AT_SECURE, 0));
    auxv.push((AUXV_AT_CLKTCK, 100));
    auxv.push((AUXV_AT_EXECFN, execfn_ptr));
    auxv.push((AUXV_AT_RANDOM, random_ptr));
    auxv.push((AUXV_AT_NULL, 0));

    for (key, value) in auxv.iter().rev() {
        builder.push_u64(*value).map_err(SubsystemError::from)?;
        builder.push_u64(*key).map_err(SubsystemError::from)?;
    }

    let mut env_entries = env_pointers;
    env_entries.push(0);
    for ptr in env_entries.iter().rev() {
        builder.push_u64(*ptr).map_err(SubsystemError::from)?;
    }

    let mut argv_entries = argv_pointers;
    argv_entries.push(0);
    for ptr in argv_entries.iter().rev() {
        builder.push_u64(*ptr).map_err(SubsystemError::from)?;
    }

    // x86-64 ABI requires RSP to be 16-byte aligned at process entry (_start)
    // After pushing argc, RSP should be misaligned by 8 (as if a call just happened)
    // But we need to account for the fact that we're entering _start directly
    // without a call, so we need RSP%16==0 BEFORE push argc
    builder.align_down(16).map_err(SubsystemError::from)?;
    
    // Push a dummy return address to simulate a call to _start
    // This makes RSP%16==8, which is what we want before push argc
    builder.push_u64(0).map_err(SubsystemError::from)?;

    debug_assert_eq!(builder.current_sp() & 0xF, 8);

    let argc = (argv_entries.len() - 1) as u64;
    builder.push_u64(argc).map_err(SubsystemError::from)?;

    debug_assert_eq!(builder.current_sp() & 0xF, 0);

    let result = builder.finalize().map_err(SubsystemError::from)?;
    Ok(StackInitialization {
        stack_pointer: result.sp,
        image: result.image,
    })
}

struct StackInitializer {
    base: u64,
    size: usize,
    sp: u64,
    writes: Vec<(u64, Vec<u8>)>,
}

impl StackInitializer {
    fn new(stack: &Stack) -> Self {
        Self {
            base: stack.base,
            size: stack.size,
            sp: stack.top(),
            writes: Vec::new(),
        }
    }

    fn push_bytes(&mut self, bytes: &[u8], align: usize) -> Result<u64, StackBuilderError> {
        let len = bytes.len() as u64;
        let mut new_sp = self
            .sp
            .checked_sub(len)
            .ok_or(StackBuilderError::Overflow)?;
        if align > 1 {
            let mask = (align as u64) - 1;
            new_sp &= !mask;
        }
        if new_sp < self.base {
            return Err(StackBuilderError::Overflow);
        }
        self.sp = new_sp;
        let addr = self.sp;
        self.writes.push((addr, bytes.to_vec()));
        Ok(addr)
    }

    fn push_cstring(&mut self, value: &str) -> Result<u64, StackBuilderError> {
        let mut bytes = Vec::with_capacity(value.len() + 1);
        bytes.extend_from_slice(value.as_bytes());
        bytes.push(0);
        self.push_bytes(&bytes, 1)
    }

    fn push_u64(&mut self, value: u64) -> Result<u64, StackBuilderError> {
        self.push_bytes(&value.to_le_bytes(), core::mem::size_of::<u64>())
    }

    fn align_down(&mut self, align: usize) -> Result<(), StackBuilderError> {
        if align <= 1 {
            return Ok(());
        }
        let mask = (align as u64) - 1;
        let aligned = self.sp & !mask;
        if aligned < self.base {
            return Err(StackBuilderError::Overflow);
        }
        self.sp = aligned;
        Ok(())
    }

    fn current_sp(&self) -> u64 {
        self.sp
    }

    fn finalize(self) -> Result<StackBuildResult, StackBuilderError> {
        let top = self.base + self.size as u64;
        let used = top
            .checked_sub(self.sp)
            .ok_or(StackBuilderError::Overflow)? as usize;
        if used == 0 {
            return Ok(StackBuildResult {
                sp: self.sp,
                image: None,
            });
        }
        let mut data = vec![0u8; used];
        for (addr, bytes) in self.writes {
            let start = addr
                .checked_sub(self.sp)
                .ok_or(StackBuilderError::Overflow)? as usize;
            let end = start + bytes.len();
            if end > data.len() {
                return Err(StackBuilderError::Overflow);
            }
            data[start..end].copy_from_slice(&bytes);
        }
        Ok(StackBuildResult {
            sp: self.sp,
            image: Some(StackImage {
                offset: (self.sp - self.base) as usize,
                data,
            }),
        })
    }
}

struct StackBuildResult {
    sp: u64,
    image: Option<StackImage>,
}

#[derive(Debug)]
enum StackBuilderError {
    Overflow,
}

impl From<StackBuilderError> for SubsystemError {
    fn from(_: StackBuilderError) -> Self {
        SubsystemError::Runtime("initial stack construction overflow")
    }
}

fn pseudo_random_bytes(seed: u64) -> [u8; 16] {
    let mut state = seed ^ 0x9E37_79B9_7F4A_7C15;
    let mut output = [0u8; 16];
    for chunk in output.chunks_mut(8) {
        state ^= state << 7;
        state ^= state >> 9;
        state ^= state << 8;
        let bytes = state.to_le_bytes();
        let len = chunk.len();
        chunk.copy_from_slice(&bytes[..len]);
    }
    output
}
impl AddressSpace {
    fn from_segment_mappings(
        entry_point: u64,
        segments: impl IntoIterator<Item = SegmentMapping>,
        stack_flags: SegmentFlags,
        stack_config: StackConfig,
    ) -> Self {
        let mut segments: Vec<SegmentMapping> = segments.into_iter().collect();
        segments.sort_by_key(|segment| segment.base());
        Self::finish_address_space(entry_point, segments, stack_flags, stack_config)
    }

    fn finish_address_space(
        entry_point: u64,
        segments: Vec<SegmentMapping>,
        stack_flags: SegmentFlags,
        stack_config: StackConfig,
    ) -> Self {
        let stack_permissions = stack_config.permissions() | MemoryFlags::from_segment(stack_flags);
        let stack =
            Stack::with_permissions(stack_config.top(), stack_config.size(), stack_permissions);
        Self {
            entry_point,
            segments,
            stack,
            tls_template: None,
            tls_base: None,
        }
    }
}

#[derive(Debug)]
struct ImagePlacement {
    segments: Vec<SegmentMapping>,
    entry: u64,
    end: u64,
    bias: u64,
}

fn layout_image(image: &ExecutableImage, preferred_start: u64) -> ImagePlacement {
    let min_addr = image.min_virtual_address();
    let max_addr = image.max_virtual_address();
    let bias = if image.is_position_independent() {
        let align = image.max_alignment().max(PAGE_SIZE);
        let preferred = core::cmp::max(preferred_start, min_addr);
        let aligned_start = align_up(preferred, align);
        aligned_start - min_addr
    } else {
        0
    };

    // Create a mutable copy of segments for relocation
    let mut exec_segments: Vec<ExecutableSegment> = image.segments().to_vec();
    
    // Apply relocations if the image has dynamic info
    if image.is_position_independent() && bias != 0 {
        log::trace!("user: applying relocations with bias=0x{:x}", bias);
        crate::elf::apply_relocations(&mut exec_segments, image.dynamic_info(), bias);
        log::trace!("user: relocations applied");
    } else {
        log::trace!("user: no relocations needed (PIE={} bias=0x{:x})", 
            image.is_position_independent(), bias);
    }

    let mut segments: Vec<SegmentMapping> = exec_segments
        .iter()
        .map(|segment| segment_mapping(segment, bias))
        .collect();
    segments.sort_by_key(|segment| segment.base());

    let entry = image
        .entry_point()
        .checked_add(bias)
        .expect("entry point overflow while applying load bias");
    let end = max_addr
        .checked_add(bias)
        .expect("address space overflow while applying load bias");

    ImagePlacement {
        segments,
        entry,
        end,
        bias,
    }
}

fn segment_mapping(segment: &ExecutableSegment, bias: u64) -> SegmentMapping {
    let payload = segment.data.clone();
    SegmentMapping {
        base: segment.virtual_addr + bias,
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

fn merge_segment_flags(a: SegmentFlags, b: SegmentFlags) -> SegmentFlags {
    SegmentFlags {
        readable: a.readable || b.readable,
        writable: a.writable || b.writable,
        executable: a.executable || b.executable,
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
        Self::for_entry_with_tls(entry_point, stack_top, None)
    }

    /// Creates a fresh context with optional TLS base address.
    pub fn for_entry_with_tls(entry_point: u64, stack_top: u64, tls_base: Option<u64>) -> Self {
        let mut frame = TrapFrame::default();
        frame.rip = entry_point;
        frame.rsp = stack_top;
        frame.rflags = USER_RFLAGS;
        
        // On x86_64, set FS_BASE MSR for TLS pointer
        #[cfg(target_arch = "x86_64")]
        if let Some(tls) = tls_base {
            use x86_64::registers::model_specific::FsBase;
            FsBase::write(x86_64::VirtAddr::new(tls));
        }
        
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
            file_offset: 0,
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
