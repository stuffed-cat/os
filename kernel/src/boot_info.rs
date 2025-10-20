use core::slice;

/// Maximum number of memory regions retained from the bootloader.
const MAX_MEMORY_REGIONS: usize = 256;

/// Description of the linear framebuffer exposed by the bootloader.
#[derive(Clone, Copy, Debug)]
pub struct FramebufferInfo {
    /// Framebuffer width in pixels.
    pub width: usize,
    /// Framebuffer height in pixels.
    pub height: usize,
    /// Number of pixels between consecutive rows.
    pub stride: usize,
    /// Bytes stored for each pixel.
    pub bytes_per_pixel: u8,
    /// Layout of the pixel data.
    pub pixel_format: PixelFormat,
}

impl FramebufferInfo {
    /// Creates a new framebuffer description.
    pub const fn new(
        width: usize,
        height: usize,
        stride: usize,
        bytes_per_pixel: u8,
        pixel_format: PixelFormat,
    ) -> Self {
        Self {
            width,
            height,
            stride,
            bytes_per_pixel,
            pixel_format,
        }
    }
}

/// Framebuffer wrapper storing metadata and backing memory address.
#[derive(Clone, Copy)]
pub struct Framebuffer {
    /// Physical address of the framebuffer buffer.
    pub buffer_addr: u64,
    /// Length of the framebuffer buffer in bytes.
    pub buffer_len: usize,
    /// Associated framebuffer metadata.
    pub info: FramebufferInfo,
}

impl Framebuffer {
    /// Constructs a framebuffer wrapper from raw components.
    pub const fn new(buffer_addr: u64, buffer_len: usize, info: FramebufferInfo) -> Self {
        Self {
            buffer_addr,
            buffer_len,
            info,
        }
    }

    /// Returns the framebuffer metadata.
    pub fn info(&self) -> FramebufferInfo {
        self.info
    }

    /// Returns a mutable slice to the framebuffer memory.
    ///
    /// # Safety
    ///
    /// Caller must ensure exclusive access.
    pub unsafe fn buffer_mut(&mut self) -> &mut [u8] {
        slice::from_raw_parts_mut(self.buffer_addr as *mut u8, self.buffer_len)
    }
}

/// Supported framebuffer pixel formats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelFormat {
    /// Red-Green-Blue byte order.
    Rgb,
    /// Blue-Green-Red byte order.
    Bgr,
    /// Single byte color index/greyscale.
    U8,
    /// Any other pixel layout.
    Unknown,
}

/// Memory region visibility reported by the bootloader.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoryRegionKind {
    /// Usable for general-purpose allocations.
    Usable,
    /// Reserved and must not be used.
    Reserved,
    /// ACPI reclaimable memory.
    AcpiReclaimable,
    /// ACPI non-volatile storage region.
    AcpiNvs,
    /// Known bad memory region.
    BadMemory,
    /// Any other region reported by the bootloader.
    Unknown,
}

impl MemoryRegionKind {
    /// Converts a raw Multiboot memory type into the strongly typed variant.
    pub const fn from_raw(raw: u32) -> Self {
        match raw {
            1 => MemoryRegionKind::Usable,
            2 => MemoryRegionKind::Reserved,
            3 => MemoryRegionKind::AcpiReclaimable,
            4 => MemoryRegionKind::AcpiNvs,
            5 => MemoryRegionKind::BadMemory,
            _ => MemoryRegionKind::Unknown,
        }
    }
}

/// Physical memory region descriptor returned by the bootloader.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryRegion {
    /// Starting physical address, inclusive.
    pub start: u64,
    /// Ending physical address, exclusive.
    pub end: u64,
    /// Classification of the memory range.
    pub kind: MemoryRegionKind,
}

impl MemoryRegion {
    /// Constructs an empty reserved memory region.
    pub const fn empty() -> Self {
        Self {
            start: 0,
            end: 0,
            kind: MemoryRegionKind::Reserved,
        }
    }
}

/// Collection of memory regions.
pub struct MemoryRegions {
    entries: [MemoryRegion; MAX_MEMORY_REGIONS],
    len: usize,
}

impl MemoryRegions {
    /// Creates an empty collection of memory regions.
    pub const fn new() -> Self {
        Self {
            entries: [MemoryRegion::empty(); MAX_MEMORY_REGIONS],
            len: 0,
        }
    }

    /// Attempts to append a memory region, discarding it when full.
    pub fn push(&mut self, region: MemoryRegion) {
        if self.len < MAX_MEMORY_REGIONS {
            self.entries[self.len] = region;
            self.len += 1;
        }
    }

    /// Returns an iterator over the stored memory regions.
    pub fn iter(&self) -> impl Iterator<Item = &MemoryRegion> {
        self.entries[..self.len].iter()
    }
}

/// Boot information exposed to the kernel entry point.
pub struct BootInfo {
    /// Offset between physical and virtual memory when identity mapping is not used.
    pub physical_memory_offset: Option<u64>,
    /// Discovered memory regions supplied by the bootloader.
    pub memory_regions: MemoryRegions,
    /// Optional framebuffer metadata and mapping.
    pub framebuffer: Option<Framebuffer>,
    /// Virtual address of the initial ramdisk.
    pub ramdisk_addr: Option<u64>,
    /// Length of the initial ramdisk in bytes.
    pub ramdisk_len: u64,
}

impl BootInfo {
    /// Returns an empty boot information structure.
    pub const fn new() -> Self {
        Self {
            physical_memory_offset: None,
            memory_regions: MemoryRegions::new(),
            framebuffer: None,
            ramdisk_addr: None,
            ramdisk_len: 0,
        }
    }
}
