//! Minimal ELF64 loader for userland executables.
//!
//! The implementation purposely keeps the scope small: it validates 64-bit
//! little-endian ELF binaries for the x86-64 architecture and extracts the
//! loadable segments so that the process layer can build an address space.

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::convert::TryFrom;
use core::fmt;

/// Constants defined by the ELF specification.
const ELF_MAGIC: [u8; 4] = [0x7F, b'E', b'L', b'F'];
const ELF_CLASS_64: u8 = 2;
const ELF_DATA_LITTLE: u8 = 1;
const ELF_VERSION_CURRENT: u8 = 1;
const ELF_OSABI_SYSV: u8 = 0;
const ELF_OSABI_LINUX: u8 = 3;
const ELF_HDR_LEN: usize = 64;
const PHDR_LEN: usize = 56;
const USER_CANONICAL_LIMIT: u64 = 0x0000_8000_0000_0000;

const EI_VERSION: usize = 6;
const EI_OSABI: usize = 7;

const ET_EXEC: u16 = 2;
const ET_DYN: u16 = 3;
const EM_X86_64: u16 = 0x3E;

const PT_LOAD: u32 = 1;
const PT_DYNAMIC: u32 = 2;
const PT_INTERP: u32 = 3;
const PT_TLS: u32 = 7;
const PT_GNU_STACK: u32 = 0x6474_E551;
const PF_X: u32 = 1;
const PF_W: u32 = 2;
const PF_R: u32 = 4;

// Dynamic section tags
const DT_NULL: u64 = 0;
const DT_RELA: u64 = 7;
const DT_RELASZ: u64 = 8;
const DT_RELAENT: u64 = 9;
const DT_REL: u64 = 17;
const DT_RELSZ: u64 = 18;
const DT_RELENT: u64 = 19;
const DT_RELR: u64 = 36; // RELR relative relocations (compressed)
const DT_RELRSZ: u64 = 35; // Size of RELR table
const DT_RELRENT: u64 = 37; // Size of RELR entry

// x86_64 relocation types
const R_X86_64_NONE: u32 = 0;
const R_X86_64_64: u32 = 1;
const R_X86_64_GLOB_DAT: u32 = 6;
const R_X86_64_JUMP_SLOT: u32 = 7;
const R_X86_64_RELATIVE: u32 = 8;

/// Classification of an executable image based on its ELF type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageKind {
    /// A fixed-address executable (ET_EXEC).
    Static,
    /// A position-independent image that requires a load bias (ET_DYN).
    PositionIndependent,
}

impl ImageKind {
    fn is_position_independent(self) -> bool {
        matches!(self, ImageKind::PositionIndependent)
    }
}

const DEFAULT_STACK_FLAGS: SegmentFlags = SegmentFlags {
    readable: true,
    writable: true,
    executable: false,
};

/// Parsing errors raised when decoding an ELF binary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElfError {
    /// File too small to contain the ELF header.
    Truncated,
    /// Magic bytes do not match the ELF signature.
    BadMagic,
    /// Unsupported ELF class (only 64-bit is accepted).
    UnsupportedClass,
    /// Unsupported endianness (only little-endian is accepted).
    UnsupportedEndian,
    /// Unsupported binary type.
    UnsupportedType,
    /// Unsupported target architecture.
    UnsupportedArch,
    /// Unsupported or unknown OS ABI.
    UnsupportedAbi,
    /// Unsupported ELF version.
    UnsupportedVersion,
    /// Program header table is out of bounds.
    BadProgramHeaderBounds,
    /// Program header entries have an unexpected size.
    BadProgramHeaderSize,
    /// Program header entry has invalid bounds.
    BadSegmentBounds,
    /// Program segment does not respect alignment requirements.
    BadSegmentAlignment,
    /// Program segment has an invalid virtual address range.
    BadSegmentAddress,
    /// Program segments overlap in virtual memory.
    SegmentOverlap,
    /// No loadable segments were found.
    NoLoadSegments,
    /// Entry point does not fall within a loadable segment.
    EntryNotLoadable,
}

impl fmt::Display for ElfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use ElfError::*;
        match self {
            Truncated => write!(f, "truncated ELF binary"),
            BadMagic => write!(f, "bad ELF magic"),
            UnsupportedClass => write!(f, "unsupported ELF class"),
            UnsupportedEndian => write!(f, "unsupported ELF endianness"),
            UnsupportedType => write!(f, "unsupported ELF type"),
            UnsupportedArch => write!(f, "unsupported ELF architecture"),
            UnsupportedAbi => write!(f, "unsupported ELF OS ABI"),
            UnsupportedVersion => write!(f, "unsupported ELF version"),
            BadProgramHeaderBounds => write!(f, "program header table out of bounds"),
            BadProgramHeaderSize => write!(f, "unexpected program header entry size"),
            BadSegmentBounds => write!(f, "segment bounds invalid"),
            BadSegmentAlignment => write!(f, "segment alignment invalid"),
            BadSegmentAddress => write!(f, "segment virtual address invalid"),
            SegmentOverlap => write!(f, "segments overlap in virtual memory"),
            NoLoadSegments => write!(f, "ELF contains no loadable segments"),
            EntryNotLoadable => write!(f, "entry point not present in loadable segment"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for ElfError {}

/// Executable image extracted from an ELF binary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutableImage {
    entry_point: u64,
    segments: Vec<ExecutableSegment>,
    interpreter: Option<String>,
    stack_flags: SegmentFlags,
    tls: Option<TlsTemplate>,
    dynamic: Option<DynamicInfo>,
    kind: ImageKind,
    program_header_offset: u64,
    program_header_entry_size: u16,
    program_header_count: u16,
}

impl ExecutableImage {
    /// Parses an ELF64 binary from the provided byte slice.
    pub fn parse(data: &[u8]) -> Result<Self, ElfError> {
        if data.len() < ELF_HDR_LEN {
            return Err(ElfError::Truncated);
        }

        if data[0..4] != ELF_MAGIC {
            return Err(ElfError::BadMagic);
        }

        let class = data[4];
        if class != ELF_CLASS_64 {
            return Err(ElfError::UnsupportedClass);
        }

        let endian = data[5];
        if endian != ELF_DATA_LITTLE {
            return Err(ElfError::UnsupportedEndian);
        }

        if data[EI_VERSION] != ELF_VERSION_CURRENT {
            return Err(ElfError::UnsupportedVersion);
        }

        let os_abi = data[EI_OSABI];
        if os_abi != ELF_OSABI_SYSV && os_abi != ELF_OSABI_LINUX {
            return Err(ElfError::UnsupportedAbi);
        }

        let e_type = read_u16(data, 16);
        if e_type != ET_EXEC && e_type != ET_DYN {
            return Err(ElfError::UnsupportedType);
        }

        let kind = if e_type == ET_DYN {
            ImageKind::PositionIndependent
        } else {
            ImageKind::Static
        };

        let e_machine = read_u16(data, 18);
        if e_machine != EM_X86_64 {
            return Err(ElfError::UnsupportedArch);
        }

        let e_entry = read_u64(data, 24);
        let e_phoff_raw = read_u64(data, 32);
        let e_phoff = usize::try_from(e_phoff_raw).map_err(|_| ElfError::BadProgramHeaderBounds)?;
        let e_phentsize_raw = read_u16(data, 54);
        let e_phentsize = e_phentsize_raw as usize;
        let e_phnum_raw = read_u16(data, 56);
        let e_phnum = e_phnum_raw as usize;

        if e_phentsize == 0 {
            return Err(ElfError::BadProgramHeaderBounds);
        }

        if e_phentsize != PHDR_LEN {
            return Err(ElfError::BadProgramHeaderSize);
        }

        if e_phnum == 0 {
            return Err(ElfError::BadProgramHeaderBounds);
        }

        let header_table_len = e_phentsize
            .checked_mul(e_phnum)
            .ok_or(ElfError::BadProgramHeaderBounds)?;
        if e_phoff
            .checked_add(header_table_len)
            .map_or(true, |end| end > data.len())
        {
            return Err(ElfError::BadProgramHeaderBounds);
        }

        let mut segments = Vec::new();
        let mut interpreter = None;
        let mut stack_flags = DEFAULT_STACK_FLAGS;
        let mut tls = None;
        let mut dynamic = None;
        for index in 0..e_phnum {
            let offset = e_phoff + index * e_phentsize;
            let header = &data[offset..offset + e_phentsize];
            let p_type = read_u32(header, 0);
            match p_type {
                PT_DYNAMIC => {
                    let p_offset = read_u64(header, 8);
                    let p_filesz = read_u64(header, 32);
                    if p_filesz > 0 {
                        let offset =
                            usize::try_from(p_offset).map_err(|_| ElfError::BadSegmentBounds)?;
                        let size =
                            usize::try_from(p_filesz).map_err(|_| ElfError::BadSegmentBounds)?;
                        if offset
                            .checked_add(size)
                            .map_or(true, |end| end > data.len())
                        {
                            return Err(ElfError::BadSegmentBounds);
                        }
                        dynamic = Some(parse_dynamic_section(&data[offset..offset + size])?);
                    }
                }
                PT_INTERP => {
                    let p_offset = read_u64(header, 8);
                    let p_filesz = read_u64(header, 32);
                    if p_filesz == 0 {
                        continue;
                    }
                    let offset =
                        usize::try_from(p_offset).map_err(|_| ElfError::BadSegmentBounds)?;
                    let size = usize::try_from(p_filesz).map_err(|_| ElfError::BadSegmentBounds)?;
                    if offset
                        .checked_add(size)
                        .map_or(true, |end| end > data.len())
                    {
                        return Err(ElfError::BadSegmentBounds);
                    }
                    let raw = &data[offset..offset + size];
                    let terminator = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
                    let bytes = &raw[..terminator];
                    if let Ok(path) = core::str::from_utf8(bytes) {
                        interpreter = Some(String::from(path));
                    }
                }
                PT_GNU_STACK => {
                    let p_flags = read_u32(header, 4);
                    stack_flags = SegmentFlags::from_program_flags(p_flags);
                }
                PT_TLS => {
                    let p_offset = read_u64(header, 8);
                    let p_filesz = read_u64(header, 32);
                    let p_memsz = read_u64(header, 40);
                    let p_align = read_u64(header, 48);
                    if p_memsz < p_filesz {
                        return Err(ElfError::BadSegmentBounds);
                    }
                    let offset =
                        usize::try_from(p_offset).map_err(|_| ElfError::BadSegmentBounds)?;
                    let filesz =
                        usize::try_from(p_filesz).map_err(|_| ElfError::BadSegmentBounds)?;
                    let memsz = usize::try_from(p_memsz).map_err(|_| ElfError::BadSegmentBounds)?;
                    if offset
                        .checked_add(filesz)
                        .map_or(true, |end| end > data.len())
                    {
                        return Err(ElfError::BadSegmentBounds);
                    }
                    let mut data_buf = vec![0u8; memsz];
                    if filesz > 0 {
                        data_buf[..filesz].copy_from_slice(&data[offset..offset + filesz]);
                    }
                    let align = normalize_alignment(p_align)?;
                    tls = Some(TlsTemplate {
                        data: data_buf,
                        mem_size: memsz,
                        align,
                    });
                }
                PT_LOAD => {
                    let p_flags = read_u32(header, 4);
                    let p_offset = read_u64(header, 8);
                    let p_vaddr = read_u64(header, 16);
                    let p_filesz = read_u64(header, 32);
                    let p_memsz = read_u64(header, 40);
                    let p_align = read_u64(header, 48);

                    if p_memsz < p_filesz {
                        return Err(ElfError::BadSegmentBounds);
                    }

                    let offset =
                        usize::try_from(p_offset).map_err(|_| ElfError::BadSegmentBounds)?;
                    let file_size =
                        usize::try_from(p_filesz).map_err(|_| ElfError::BadSegmentBounds)?;
                    let mem_size =
                        usize::try_from(p_memsz).map_err(|_| ElfError::BadSegmentBounds)?;

                    if offset
                        .checked_add(file_size)
                        .map_or(true, |end| end > data.len())
                    {
                        return Err(ElfError::BadSegmentBounds);
                    }

                    let align = normalize_alignment(p_align)?;
                    if (p_offset % align) != (p_vaddr % align) {
                        return Err(ElfError::BadSegmentAlignment);
                    }

                    let seg_end = p_vaddr
                        .checked_add(p_memsz)
                        .ok_or(ElfError::BadSegmentAddress)?;
                    if !is_user_canonical(p_vaddr) || seg_end > USER_CANONICAL_LIMIT {
                        return Err(ElfError::BadSegmentAddress);
                    }

                    let mut segment_data = Vec::with_capacity(mem_size);
                    segment_data.extend_from_slice(&data[offset..offset + file_size]);
                    if mem_size > file_size {
                        segment_data.resize(mem_size, 0);
                    }

                    let flags = SegmentFlags::from_program_flags(p_flags);

                    segments.push(ExecutableSegment {
                        virtual_addr: p_vaddr,
                        mem_size,
                        file_size,
                        align,
                        data: segment_data,
                        flags,
                        file_offset: p_offset,
                    });
                }
                _ => {}
            }
        }

        if segments.is_empty() {
            return Err(ElfError::NoLoadSegments);
        }

        segments.sort_by_key(|segment| segment.virtual_addr);

        for window in segments.windows(2) {
            let prev_end = window[0]
                .virtual_addr
                .checked_add(window[0].mem_size as u64)
                .ok_or(ElfError::BadSegmentAddress)?;
            if prev_end > window[1].virtual_addr {
                return Err(ElfError::SegmentOverlap);
            }
        }

        if !segments
            .iter()
            .any(|segment| segment_contains(segment, e_entry))
        {
            return Err(ElfError::EntryNotLoadable);
        }

        Ok(Self {
            entry_point: e_entry,
            segments,
            interpreter,
            stack_flags,
            tls,
            kind,
            program_header_offset: e_phoff_raw,
            program_header_entry_size: e_phentsize_raw,
            program_header_count: e_phnum_raw,
            dynamic,
        })
    }

    /// Builds an executable image from already prepared segments.
    pub fn from_parts(
        entry_point: u64,
        segments: Vec<ExecutableSegment>,
    ) -> Result<Self, ElfError> {
        if segments.is_empty() {
            return Err(ElfError::NoLoadSegments);
        }
        Ok(Self {
            entry_point,
            segments,
            interpreter: None,
            stack_flags: DEFAULT_STACK_FLAGS,
            tls: None,
            kind: ImageKind::Static,
            program_header_offset: 0,
            program_header_entry_size: 0,
            program_header_count: 0,
            dynamic: None,
        })
    }

    /// Entry point address specified by the ELF header.
    pub fn entry_point(&self) -> u64 {
        self.entry_point
    }

    /// Loadable segments extracted from the ELF binary.
    pub fn segments(&self) -> &[ExecutableSegment] {
        &self.segments
    }

    /// Returns the underlying ELF image kind.
    pub fn kind(&self) -> ImageKind {
        self.kind
    }

    /// Returns true when the image is position-independent and requires a load bias.
    pub fn is_position_independent(&self) -> bool {
        self.kind.is_position_independent()
    }

    /// Lowest virtual address referenced by the image's segments.
    pub fn min_virtual_address(&self) -> u64 {
        self.segments
            .iter()
            .map(|segment| segment.virtual_addr)
            .min()
            .unwrap_or(0)
    }

    /// Exclusive upper bound of the image's virtual address space.
    pub fn max_virtual_address(&self) -> u64 {
        self.segments
            .iter()
            .map(|segment| segment.virtual_addr + segment.mem_size as u64)
            .max()
            .unwrap_or(0)
    }

    /// Maximum alignment requested by any loadable segment.
    pub fn max_alignment(&self) -> u64 {
        self.segments
            .iter()
            .map(|segment| segment.align.max(1))
            .max()
            .unwrap_or(1)
    }

    /// Optional interpreter path extracted from the program headers.
    pub fn interpreter(&self) -> Option<&str> {
        self.interpreter.as_deref()
    }

    /// Stack permissions requested by the ELF binary (from `PT_GNU_STACK`).
    pub fn stack_flags(&self) -> SegmentFlags {
        self.stack_flags
    }

    /// Optional TLS template describing thread-local initial data.
    pub fn tls_template(&self) -> Option<&TlsTemplate> {
        self.tls.as_ref()
    }

    /// Optional dynamic section information for relocations.
    pub fn dynamic_info(&self) -> Option<&DynamicInfo> {
        self.dynamic.as_ref()
    }

    /// Returns the file offset of the program header table.
    pub fn program_header_offset(&self) -> u64 {
        self.program_header_offset
    }

    /// Returns the size in bytes of each program header entry.
    pub fn program_header_entry_size(&self) -> u16 {
        self.program_header_entry_size
    }

    /// Returns the number of program header entries in the table.
    pub fn program_header_count(&self) -> u16 {
        self.program_header_count
    }

    /// Returns the virtual address of the program header table after applying the provided load bias.
    pub fn program_header_virtual_address(&self, load_bias: u64) -> Option<u64> {
        if self.program_header_count == 0 {
            return None;
        }
        let offset = self.program_header_offset;
        for segment in &self.segments {
            let start = segment.file_offset;
            let end = start + segment.file_size as u64;
            if offset >= start && offset < end {
                let delta = offset - start;
                return Some(segment.virtual_addr + delta + load_bias);
            }
        }
        None
    }
}

/// A single loadable segment from the executable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutableSegment {
    /// Virtual address where the segment should be mapped.
    pub virtual_addr: u64,
    /// Size of the segment in memory (`p_memsz`).
    pub mem_size: usize,
    /// Size of the initialized portion of the segment (`p_filesz`).
    pub file_size: usize,
    /// Requested alignment for the segment (`p_align`).
    pub align: u64,
    /// Data payload for the segment (already zero-extended to memsz).
    pub data: Vec<u8>,
    /// Access permissions extracted from the program header.
    pub flags: SegmentFlags,
    /// File offset (`p_offset`) where the segment payload starts.
    pub file_offset: u64,
}

/// Template describing a thread-local storage image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TlsTemplate {
    /// Initial thread-local data copied into TLS blocks.
    pub data: Vec<u8>,
    /// Total TLS memory requirement (`p_memsz`).
    pub mem_size: usize,
    /// Alignment constraint for TLS (`p_align`).
    pub align: u64,
}

/// Dynamic section information for relocation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DynamicInfo {
    /// Address of RELA relocation table.
    pub rela_addr: Option<u64>,
    /// Size of RELA relocation table.
    pub rela_size: Option<u64>,
    /// Size of each RELA entry.
    pub rela_ent: Option<u64>,
    /// Address of REL relocation table.
    pub rel_addr: Option<u64>,
    /// Size of REL relocation table.
    pub rel_size: Option<u64>,
    /// Size of each REL entry.
    pub rel_ent: Option<u64>,
    /// Address of RELR relocation table (compressed relative relocations).
    pub relr_addr: Option<u64>,
    /// Size of RELR relocation table.
    pub relr_size: Option<u64>,
    /// Size of each RELR entry.
    pub relr_ent: Option<u64>,
}

/// A single relocation entry (RELA format).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RelocationEntry {
    /// Offset where to apply the relocation.
    pub offset: u64,
    /// Relocation type.
    pub r_type: u32,
    /// Symbol index.
    pub symbol: u32,
    /// Addend for the relocation.
    pub addend: i64,
}

/// Access flags for a segment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SegmentFlags {
    /// Segment is readable.
    pub readable: bool,
    /// Segment is writable.
    pub writable: bool,
    /// Segment is executable.
    pub executable: bool,
}

impl SegmentFlags {
    fn from_program_flags(flags: u32) -> Self {
        Self {
            readable: (flags & PF_R) != 0,
            writable: (flags & PF_W) != 0,
            executable: (flags & PF_X) != 0,
        }
    }
}

fn read_u16(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([data[offset], data[offset + 1]])
}

fn read_u32(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

/// Parse the PT_DYNAMIC segment to extract relocation table addresses.
fn parse_dynamic_section(data: &[u8]) -> Result<DynamicInfo, ElfError> {
    const DYNAMIC_ENTRY_SIZE: usize = 16;
    let mut info = DynamicInfo::default();

    let entry_count = data.len() / DYNAMIC_ENTRY_SIZE;
    for i in 0..entry_count {
        let offset = i * DYNAMIC_ENTRY_SIZE;
        if offset + DYNAMIC_ENTRY_SIZE > data.len() {
            break;
        }

        let d_tag = read_u64(data, offset);
        let d_val = read_u64(data, offset + 8);

        match d_tag {
            DT_NULL => break,
            DT_RELA => info.rela_addr = Some(d_val),
            DT_RELASZ => info.rela_size = Some(d_val),
            DT_RELAENT => info.rela_ent = Some(d_val),
            DT_REL => info.rel_addr = Some(d_val),
            DT_RELSZ => info.rel_size = Some(d_val),
            DT_RELENT => info.rel_ent = Some(d_val),
            DT_RELRSZ => info.relr_size = Some(d_val), // Note: size comes before address in DT_ numbers
            DT_RELR => info.relr_addr = Some(d_val),
            DT_RELRENT => info.relr_ent = Some(d_val),
            _ => {}
        }
    }

    Ok(info)
}

/// Extract RELA relocation entries from the dynamic info and segment data.
fn extract_relocations(
    segments: &[ExecutableSegment],
    dynamic: &DynamicInfo,
    _load_bias: u64,
) -> Vec<RelocationEntry> {
    let mut relocations = Vec::new();

    // Try RELA first (most common for x86_64)
    if let (Some(rela_addr), Some(rela_size)) = (dynamic.rela_addr, dynamic.rela_size) {
        const RELA_ENTRY_SIZE: usize = 24; // sizeof(Elf64_Rela)

        log::trace!(
            "elf: searching for RELA table at vaddr=0x{:x} size={}",
            rela_addr,
            rela_size
        );

        // Find which segment contains the RELA table
        for segment in segments {
            let seg_start = segment.virtual_addr;
            let seg_end = seg_start + segment.data.len() as u64;

            log::trace!("elf: checking segment [0x{:x}, 0x{:x})", seg_start, seg_end);

            if rela_addr >= seg_start && rela_addr < seg_end {
                let table_offset = (rela_addr - seg_start) as usize;
                let table_size = rela_size as usize;

                if table_offset + table_size <= segment.data.len() {
                    let table_data = &segment.data[table_offset..table_offset + table_size];
                    let entry_count = table_size / RELA_ENTRY_SIZE;

                    for i in 0..entry_count {
                        let entry_offset = i * RELA_ENTRY_SIZE;
                        if entry_offset + RELA_ENTRY_SIZE <= table_data.len() {
                            let r_offset = read_u64(table_data, entry_offset);
                            let r_info = read_u64(table_data, entry_offset + 8);
                            let r_addend = read_i64(table_data, entry_offset + 16);

                            let r_type = (r_info & 0xffffffff) as u32;
                            let r_sym = (r_info >> 32) as u32;

                            relocations.push(RelocationEntry {
                                offset: r_offset,
                                r_type,
                                symbol: r_sym,
                                addend: r_addend,
                            });
                        }
                    }
                }
                break;
            }
        }
    }

    // Now try RELR (compressed relative relocations)
    if let (Some(relr_addr), Some(relr_size)) = (dynamic.relr_addr, dynamic.relr_size) {
        log::trace!(
            "elf: searching for RELR table at vaddr=0x{:x} size={}",
            relr_addr,
            relr_size
        );

        // Find which segment contains the RELR table
        for segment in segments {
            let seg_start = segment.virtual_addr;
            let seg_end = seg_start + segment.data.len() as u64;

            if relr_addr >= seg_start && relr_addr < seg_end {
                let table_offset = (relr_addr - seg_start) as usize;
                let table_size = relr_size as usize;

                if table_offset + table_size <= segment.data.len() {
                    let table_data = &segment.data[table_offset..table_offset + table_size];
                    let entry_count = table_size / 8; // Each RELR entry is 8 bytes

                    let mut base_addr = 0u64;
                    for i in 0..entry_count {
                        let entry_offset = i * 8;
                        if entry_offset + 8 <= table_data.len() {
                            let entry = read_u64(table_data, entry_offset);

                            if entry & 1 == 0 {
                                // LSB = 0: This is a base address
                                base_addr = entry;
                                relocations.push(RelocationEntry {
                                    offset: base_addr,
                                    r_type: R_X86_64_RELATIVE,
                                    symbol: 0,
                                    addend: 0, // RELR doesn't have addend, will use load_bias
                                });
                                base_addr += 8; // Move to next potential location
                            } else {
                                // LSB = 1: This is a bitmap
                                let bitmap = entry >> 1; // Remove LSB
                                for bit in 0..63 {
                                    if bitmap & (1 << bit) != 0 {
                                        let offset = base_addr + (bit * 8);
                                        relocations.push(RelocationEntry {
                                            offset,
                                            r_type: R_X86_64_RELATIVE,
                                            symbol: 0,
                                            addend: 0,
                                        });
                                    }
                                }
                                base_addr += 63 * 8; // Move past the 63 positions
                            }
                        }
                    }
                    log::trace!("elf: decoded {} RELR relocations", entry_count);
                }
                break;
            }
        }
    }

    relocations
}

/// Apply R_X86_64_RELATIVE relocations to segment data.
pub fn apply_relocations(
    segments: &mut [ExecutableSegment],
    dynamic: Option<&DynamicInfo>,
    load_bias: u64,
) {
    let Some(dynamic_info) = dynamic else {
        return;
    };

    let relocations = extract_relocations(segments, dynamic_info, load_bias);

    log::trace!(
        "elf: found {} relocations, load_bias=0x{:x}",
        relocations.len(),
        load_bias
    );

    let mut applied_count = 0;
    let mut code_segment_modified = 0;

    for reloc in relocations {
        // Only handle R_X86_64_RELATIVE for now (most critical for PIE)
        if reloc.r_type != R_X86_64_RELATIVE {
            continue;
        }

        let target_addr = reloc.offset;

        // Find which segment contains the target address
        for segment in segments.iter_mut() {
            let seg_start = segment.virtual_addr;
            let seg_end = seg_start + segment.data.len() as u64;

            if target_addr >= seg_start && target_addr + 8 <= seg_end {
                let offset_in_seg = (target_addr - seg_start) as usize;

                if offset_in_seg + 8 <= segment.data.len() {
                    // Check if we're modifying executable segment (should not happen!)
                    if segment.flags.executable {
                        log::warn!("elf: relocation modifying EXECUTABLE segment at 0x{:x} (seg [0x{:x}, 0x{:x}))",
                            target_addr, seg_start, seg_end);
                        code_segment_modified += 1;
                    }

                    // For RELR (addend=0), read the current value at target as the addend
                    let addend = if reloc.addend == 0 {
                        read_u64(&segment.data, offset_in_seg) as i64
                    } else {
                        reloc.addend
                    };

                    let new_value = (load_bias as i64 + addend) as u64;

                    // Write the relocated value as little-endian u64
                    segment.data[offset_in_seg..offset_in_seg + 8]
                        .copy_from_slice(&new_value.to_le_bytes());
                    applied_count += 1;
                }
                break;
            }
        }
    }

    if code_segment_modified > 0 {
        log::error!(
            "elf: WARNING - {} relocations modified code segment!",
            code_segment_modified
        );
    }

    log::trace!(
        "elf: applied {} R_X86_64_RELATIVE relocations",
        applied_count
    );
}

fn normalize_alignment(value: u64) -> Result<u64, ElfError> {
    let align = if value == 0 { 1 } else { value };
    if !align.is_power_of_two() {
        Err(ElfError::BadSegmentAlignment)
    } else {
        Ok(align)
    }
}

fn is_user_canonical(addr: u64) -> bool {
    addr < USER_CANONICAL_LIMIT
}

fn segment_contains(segment: &ExecutableSegment, addr: u64) -> bool {
    let end = match segment.virtual_addr.checked_add(segment.mem_size as u64) {
        Some(limit) => limit,
        None => return false,
    };
    addr >= segment.virtual_addr && addr < end
}

fn read_u64(data: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
        data[offset + 4],
        data[offset + 5],
        data[offset + 6],
        data[offset + 7],
    ])
}

fn read_i64(data: &[u8], offset: usize) -> i64 {
    i64::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
        data[offset + 4],
        data[offset + 5],
        data[offset + 6],
        data[offset + 7],
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_u16(buf: &mut Vec<u8>, offset: usize, value: u16) {
        if buf.len() < offset + 2 {
            buf.resize(offset + 2, 0);
        }
        buf[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn write_u32(buf: &mut Vec<u8>, offset: usize, value: u32) {
        if buf.len() < offset + 4 {
            buf.resize(offset + 4, 0);
        }
        buf[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn write_u64(buf: &mut Vec<u8>, offset: usize, value: u64) {
        if buf.len() < offset + 8 {
            buf.resize(offset + 8, 0);
        }
        buf[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }

    fn base_elf(ph_count: usize) -> Vec<u8> {
        let mut elf = vec![0u8; ELF_HDR_LEN];
        elf[0..4].copy_from_slice(&ELF_MAGIC);
        elf[4] = ELF_CLASS_64;
        elf[5] = ELF_DATA_LITTLE;
        elf[EI_VERSION] = ELF_VERSION_CURRENT;
        elf[EI_OSABI] = ELF_OSABI_SYSV;
        write_u16(&mut elf, 16, ET_EXEC);
        write_u16(&mut elf, 18, EM_X86_64);
        write_u64(&mut elf, 24, 0x400000);
        let ph_offset = ELF_HDR_LEN as u64;
        write_u64(&mut elf, 32, ph_offset);
        write_u16(&mut elf, 54, PHDR_LEN as u16);
        write_u16(&mut elf, 56, ph_count as u16);
        let table_len = PHDR_LEN * ph_count;
        elf.resize(ELF_HDR_LEN + table_len, 0);
        elf
    }

    #[test]
    fn parse_records_interpreter_path() {
        let mut elf = base_elf(2);

        let load_ph = ELF_HDR_LEN;
        let interp_ph = load_ph + PHDR_LEN as usize;

        let load_data_offset = 0x1000;
        write_u32(&mut elf, load_ph, PT_LOAD);
        write_u32(&mut elf, load_ph + 4, (PF_R | PF_X) as u32);
        write_u64(&mut elf, load_ph + 8, load_data_offset as u64);
        write_u64(&mut elf, load_ph + 16, 0x400000);
        write_u64(&mut elf, load_ph + 32, 4);
        write_u64(&mut elf, load_ph + 40, 4);
        write_u64(&mut elf, load_ph + 48, 0x1000);

        let interp_path = b"/lib64/ld-linux-x86-64.so.2\0";
        let interp_data_offset = load_data_offset + 0x80;
        write_u32(&mut elf, interp_ph, PT_INTERP);
        write_u64(&mut elf, interp_ph + 8, interp_data_offset as u64);
        write_u64(&mut elf, interp_ph + 32, interp_path.len() as u64);
        write_u64(&mut elf, interp_ph + 40, interp_path.len() as u64);
        write_u64(&mut elf, interp_ph + 48, 1);

        elf.resize(interp_data_offset + interp_path.len(), 0);
        elf[load_data_offset..load_data_offset + 4].copy_from_slice(&[0u8; 4]);
        elf[interp_data_offset..interp_data_offset + interp_path.len()]
            .copy_from_slice(interp_path);

        let image = ExecutableImage::parse(&elf).expect("elf parses");
        assert_eq!(image.interpreter(), Some("/lib64/ld-linux-x86-64.so.2"));
        assert_eq!(image.stack_flags(), DEFAULT_STACK_FLAGS);
        assert!(image.tls_template().is_none());
    }

    #[test]
    fn parse_rejects_misaligned_segment() {
        let mut elf = base_elf(1);
        let load_ph = ELF_HDR_LEN;
        let data_offset = 0x1000;
        write_u32(&mut elf, load_ph, PT_LOAD);
        write_u32(&mut elf, load_ph + 4, (PF_R | PF_X) as u32);
        write_u64(&mut elf, load_ph + 8, data_offset as u64);
        write_u64(&mut elf, load_ph + 16, 0x400000);
        write_u64(&mut elf, load_ph + 32, 4);
        write_u64(&mut elf, load_ph + 40, 4);
        write_u64(&mut elf, load_ph + 48, 24); // not a power of two
        elf.resize(data_offset + 4, 0);
        let err = ExecutableImage::parse(&elf).expect_err("parse should fail");
        assert_eq!(err, ElfError::BadSegmentAlignment);
    }

    #[test]
    fn parse_extracts_gnu_stack_execute_flag() {
        let mut elf = base_elf(2);
        let load_ph = ELF_HDR_LEN;
        let stack_ph = load_ph + PHDR_LEN as usize;
        let data_offset = 0x1000;
        write_u32(&mut elf, load_ph, PT_LOAD);
        write_u32(&mut elf, load_ph + 4, (PF_R | PF_X) as u32);
        write_u64(&mut elf, load_ph + 8, data_offset as u64);
        write_u64(&mut elf, load_ph + 16, 0x400000);
        write_u64(&mut elf, load_ph + 32, 4);
        write_u64(&mut elf, load_ph + 40, 4);
        write_u64(&mut elf, load_ph + 48, 0x1000);
        elf.resize(data_offset + 4, 0);

        write_u32(&mut elf, stack_ph, PT_GNU_STACK);
        write_u32(&mut elf, stack_ph + 4, (PF_R | PF_W | PF_X) as u32);
        write_u64(&mut elf, stack_ph + 8, 0);
        write_u64(&mut elf, stack_ph + 16, 0);
        write_u64(&mut elf, stack_ph + 32, 0);
        write_u64(&mut elf, stack_ph + 40, 0);
        write_u64(&mut elf, stack_ph + 48, 16);

        let image = ExecutableImage::parse(&elf).expect("elf parses");
        let stack_flags = image.stack_flags();
        assert!(stack_flags.readable);
        assert!(stack_flags.writable);
        assert!(stack_flags.executable);
    }

    #[test]
    fn parse_tls_segment_records_template() {
        let mut elf = base_elf(2);
        let load_ph = ELF_HDR_LEN;
        let tls_ph = load_ph + PHDR_LEN as usize;
        let data_offset = 0x1000;
        write_u32(&mut elf, load_ph, PT_LOAD);
        write_u32(&mut elf, load_ph + 4, (PF_R | PF_X) as u32);
        write_u64(&mut elf, load_ph + 8, data_offset as u64);
        write_u64(&mut elf, load_ph + 16, 0x400000);
        write_u64(&mut elf, load_ph + 32, 4);
        write_u64(&mut elf, load_ph + 40, 4);
        write_u64(&mut elf, load_ph + 48, 0x1000);

        let tls_data_offset = data_offset + 0x20;
        elf.resize(tls_data_offset + 16, 0);

        write_u32(&mut elf, tls_ph, PT_TLS);
        write_u32(&mut elf, tls_ph + 4, (PF_R | PF_W) as u32);
        write_u64(&mut elf, tls_ph + 8, tls_data_offset as u64);
        write_u64(&mut elf, tls_ph + 16, 0x600000);
        write_u64(&mut elf, tls_ph + 32, 8);
        write_u64(&mut elf, tls_ph + 40, 16);
        write_u64(&mut elf, tls_ph + 48, 0x20);

        for i in 0..8 {
            elf[tls_data_offset + i] = i as u8;
        }

        let image = ExecutableImage::parse(&elf).expect("elf parses");
        let tls = image.tls_template().expect("tls template present");
        assert_eq!(tls.mem_size, 16);
        assert_eq!(tls.align, 0x20);
        assert_eq!(&tls.data[..8], &[0, 1, 2, 3, 4, 5, 6, 7]);
        assert!(tls.data[8..].iter().all(|&b| b == 0));
    }

    #[test]
    fn parse_requires_entry_within_segment() {
        let mut elf = base_elf(1);
        write_u64(&mut elf, 24, 0xDEAD_BEEF);

        let load_ph = ELF_HDR_LEN;
        let data_offset = 0x1000;
        write_u32(&mut elf, load_ph, PT_LOAD);
        write_u32(&mut elf, load_ph + 4, (PF_R | PF_X) as u32);
        write_u64(&mut elf, load_ph + 8, data_offset as u64);
        write_u64(&mut elf, load_ph + 16, 0x400000);
        write_u64(&mut elf, load_ph + 32, 4);
        write_u64(&mut elf, load_ph + 40, 4);
        write_u64(&mut elf, load_ph + 48, 0x1000);
        elf.resize(data_offset + 4, 0);

        let err = ExecutableImage::parse(&elf).expect_err("entry outside should fail");
        assert_eq!(err, ElfError::EntryNotLoadable);
    }
}
