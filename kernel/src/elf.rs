//! Minimal ELF64 loader for userland executables.
//!
//! The implementation purposely keeps the scope small: it validates 64-bit
//! little-endian ELF binaries for the x86-64 architecture and extracts the
//! loadable segments so that the process layer can build an address space.

use alloc::vec::Vec;
use core::fmt;

/// Constants defined by the ELF specification.
const ELF_MAGIC: [u8; 4] = [0x7F, b'E', b'L', b'F'];
const ELF_CLASS_64: u8 = 2;
const ELF_DATA_LITTLE: u8 = 1;
const ELF_HDR_LEN: usize = 64;
const PHDR_LEN: usize = 56;

const ET_EXEC: u16 = 2;
const ET_DYN: u16 = 3;
const EM_X86_64: u16 = 0x3E;

const PT_LOAD: u32 = 1;
const PF_X: u32 = 1;
const PF_W: u32 = 2;
const PF_R: u32 = 4;

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
    /// Program header table is out of bounds.
    BadProgramHeaderBounds,
    /// Program header entry has invalid bounds.
    BadSegmentBounds,
    /// No loadable segments were found.
    NoLoadSegments,
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
            BadProgramHeaderBounds => write!(f, "program header table out of bounds"),
            BadSegmentBounds => write!(f, "segment bounds invalid"),
            NoLoadSegments => write!(f, "ELF contains no loadable segments"),
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

        let e_type = read_u16(data, 16);
        if e_type != ET_EXEC && e_type != ET_DYN {
            return Err(ElfError::UnsupportedType);
        }

        let e_machine = read_u16(data, 18);
        if e_machine != EM_X86_64 {
            return Err(ElfError::UnsupportedArch);
        }

        let e_entry = read_u64(data, 24);
        let e_phoff = read_u64(data, 32) as usize;
        let e_phentsize = read_u16(data, 54) as usize;
        let e_phnum = read_u16(data, 56) as usize;

        if e_phentsize == 0 {
            return Err(ElfError::BadProgramHeaderBounds);
        }

        let header_table_len = e_phentsize.checked_mul(e_phnum).ok_or(ElfError::BadProgramHeaderBounds)?;
        if e_phoff.checked_add(header_table_len).map_or(true, |end| end > data.len()) {
            return Err(ElfError::BadProgramHeaderBounds);
        }

        let mut segments = Vec::new();
        for index in 0..e_phnum {
            let offset = e_phoff + index * e_phentsize;
            let header = &data[offset..offset + e_phentsize];
            let p_type = read_u32(header, 0);
            if p_type != PT_LOAD {
                continue;
            }

            let p_offset = read_u64(header, 8) as usize;
            let p_vaddr = read_u64(header, 16);
            let p_filesz = read_u64(header, 32) as usize;
            let p_memsz = read_u64(header, 40) as usize;
            let p_flags = read_u32(header, 4);

            if p_memsz < p_filesz {
                return Err(ElfError::BadSegmentBounds);
            }

            if p_offset.checked_add(p_filesz).map_or(true, |end| end > data.len()) {
                return Err(ElfError::BadSegmentBounds);
            }

            let mut segment_data = Vec::with_capacity(p_memsz);
            segment_data.extend_from_slice(&data[p_offset..p_offset + p_filesz]);
            if p_memsz > p_filesz {
                segment_data.resize(p_memsz, 0);
            }

            let flags = SegmentFlags {
                readable: (p_flags & PF_R) != 0,
                writable: (p_flags & PF_W) != 0,
                executable: (p_flags & PF_X) != 0,
            };

            segments.push(ExecutableSegment {
                virtual_addr: p_vaddr,
                data: segment_data,
                flags,
            });
        }

        if segments.is_empty() {
            return Err(ElfError::NoLoadSegments);
        }

        Ok(Self { entry_point: e_entry, segments })
    }

    /// Builds an executable image from already prepared segments.
    pub fn from_parts(entry_point: u64, segments: Vec<ExecutableSegment>) -> Result<Self, ElfError> {
        if segments.is_empty() {
            return Err(ElfError::NoLoadSegments);
        }
        Ok(Self { entry_point, segments })
    }

    /// Entry point address specified by the ELF header.
    pub fn entry_point(&self) -> u64 {
        self.entry_point
    }

    /// Loadable segments extracted from the ELF binary.
    pub fn segments(&self) -> &[ExecutableSegment] {
        &self.segments
    }
}

/// A single loadable segment from the executable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutableSegment {
    /// Virtual address where the segment should be mapped.
    pub virtual_addr: u64,
    /// Data payload for the segment (already zero-extended to memsz).
    pub data: Vec<u8>,
    /// Access permissions extracted from the program header.
    pub flags: SegmentFlags,
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
