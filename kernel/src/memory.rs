//! Virtual memory and capability aware allocator scaffolding.

use alloc::collections::BTreeMap;
use core::ops::Range;
use spin::RwLock;

use crate::error::SubsystemError;

#[cfg(any(feature = "alloc", feature = "std"))]
use alloc::vec::Vec;
#[cfg(any(feature = "alloc", feature = "std"))]
use spin::Mutex;
#[cfg(any(feature = "alloc", feature = "std"))]
use x86_64::registers::control::Cr3;
#[cfg(any(feature = "alloc", feature = "std"))]
use x86_64::structures::paging::{
    mapper::MapToError, FrameAllocator, Mapper, OffsetPageTable, Page, PageSize, PageTable,
    PageTableFlags, PhysFrame, Size4KiB,
};
#[cfg(any(feature = "alloc", feature = "std"))]
use x86_64::{PhysAddr, VirtAddr};

/// Physical frame identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Frame(u64);

/// Virtual memory region descriptor.
#[derive(Debug, Clone)]
pub struct VmRegion {
    /// Virtual address range.
    pub range: Range<u64>,
    /// Access permissions.
    pub flags: VmFlags,
}

bitflags::bitflags! {
    /// Access flags for virtual memory regions.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct VmFlags: u8 {
        /// Region is readable.
        const READ = 1 << 0;
        /// Region is writable.
        const WRITE = 1 << 1;
        /// Region is executable.
        const EXEC = 1 << 2;
        /// Region is accessible from userland.
        const USER = 1 << 3;
        /// Region is restricted to kernel mode.
        const KERNEL = 1 << 4;
    }
}

/// Capability token for memory objects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Capability(u64);

/// Virtual memory manager bridging microkernel isolation with monolithic performance.
pub struct VirtualMemoryManager {
    regions: RwLock<BTreeMap<u64, VmRegion>>,
    capabilities: RwLock<BTreeMap<Capability, VmRegion>>,
    next_cap: core::sync::atomic::AtomicU64,
}

impl Default for VirtualMemoryManager {
    fn default() -> Self {
        Self {
            regions: RwLock::new(BTreeMap::new()),
            capabilities: RwLock::new(BTreeMap::new()),
            next_cap: core::sync::atomic::AtomicU64::new(1),
        }
    }
}

impl VirtualMemoryManager {
    /// Creates a new VMM instance.
    pub fn new() -> Self {
        Self::default()
    }

    /// Maps a region with associated capability.
    pub fn map(&self, region: VmRegion) -> Result<Capability, SubsystemError> {
        let cap = Capability(
            self.next_cap
                .fetch_add(1, core::sync::atomic::Ordering::SeqCst),
        );
        self.regions
            .write()
            .insert(region.range.start, region.clone());
        self.capabilities.write().insert(cap, region);
        Ok(cap)
    }

    /// Looks up region metadata by address.
    pub fn region(&self, addr: u64) -> Option<VmRegion> {
        let guard = self.regions.read();
        guard.range(..=addr).next_back().and_then(|(_, region)| {
            if region.range.contains(&addr) {
                Some(region.clone())
            } else {
                None
            }
        })
    }

    /// Resolves capability to region metadata.
    pub fn lookup_capability(&self, cap: Capability) -> Option<VmRegion> {
        self.capabilities.read().get(&cap).cloned()
    }
}

/// Descriptor for ranges of physical memory frames.
#[cfg(any(feature = "alloc", feature = "std"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameRange {
    start: PhysAddr,
    end: PhysAddr,
}

#[cfg(any(feature = "alloc", feature = "std"))]
impl FrameRange {
    /// Creates a new range from raw physical addresses.
    pub fn new(start: u64, end: u64) -> Self {
        Self {
            start: PhysAddr::new(start),
            end: PhysAddr::new(end),
        }
    }

    #[allow(dead_code)]
    fn contains(&self, addr: PhysAddr) -> bool {
        addr >= self.start && addr < self.end
    }

    fn push_frames(&self, frames: &mut Vec<PhysFrame>) {
        let mut current = align_up(self.start.as_u64(), Size4KiB::SIZE);
        let end = self.end.as_u64();
        while current + Size4KiB::SIZE <= end {
            let frame = PhysFrame::containing_address(PhysAddr::new(current));
            frames.push(frame);
            current += Size4KiB::SIZE;
        }
    }
}

#[cfg(any(feature = "alloc", feature = "std"))]
fn align_up(addr: u64, align: u64) -> u64 {
    (addr + align - 1) & !(align - 1)
}

/// Boot-time frame allocator based on a pre-collected list of free frames.
#[cfg(any(feature = "alloc", feature = "std"))]
pub struct BootFrameAllocator {
    free: Vec<PhysFrame>,
    next: usize,
}

#[cfg(any(feature = "alloc", feature = "std"))]
impl BootFrameAllocator {
    /// Builds a frame allocator from the provided frame ranges.
    pub fn from_ranges(regions: &[FrameRange]) -> Self {
        let mut free = Vec::new();
        for region in regions {
            region.push_frames(&mut free);
        }
        free.sort_by_key(|frame| frame.start_address());
        free.dedup();
        Self { free, next: 0 }
    }

    fn allocate(&mut self) -> Option<PhysFrame> {
        if self.next >= self.free.len() {
            None
        } else {
            let frame = self.free[self.next];
            self.next += 1;
            Some(frame)
        }
    }
}

#[cfg(any(feature = "alloc", feature = "std"))]
unsafe impl FrameAllocator<Size4KiB> for BootFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
        self.allocate()
    }
}

/// Controller for paging using an offset page table and boot-time allocator.
#[cfg(any(feature = "alloc", feature = "std"))]
pub struct MemoryManager {
    mapper: Mutex<OffsetPageTable<'static>>,
    frames: Mutex<BootFrameAllocator>,
    phys_offset: VirtAddr,
}

#[cfg(any(feature = "alloc", feature = "std"))]
impl MemoryManager {
    /// Creates a new memory manager instance.
    pub unsafe fn new(phys_mem_offset: VirtAddr, allocator: BootFrameAllocator) -> Self {
        let mapper = init_offset_page_table(phys_mem_offset);
        Self {
            mapper: Mutex::new(mapper),
            frames: Mutex::new(allocator),
            phys_offset: phys_mem_offset,
        }
    }

    /// Maps a linear virtual memory region backed by fresh physical frames.
    pub fn map_region(
        &self,
        start: VirtAddr,
        size: usize,
        flags: PageTableFlags,
    ) -> Result<(), MapToError<Size4KiB>> {
        assert!(size > 0, "region size must be non-zero");
        let mut mapper = self.mapper.lock();
        let mut allocator = self.frames.lock();

        let start_page = Page::containing_address(start);
        let end_page = Page::containing_address(start + (size as u64 - 1));

        for page in Page::range_inclusive(start_page, end_page) {
            let frame = allocator
                .allocate_frame()
                .ok_or(MapToError::FrameAllocationFailed)?;
            unsafe {
                mapper.map_to(page, frame, flags, &mut *allocator)?.flush();
            }
        }
        Ok(())
    }

    /// Maps a single page to the given frame.
    pub fn map_to(
        &self,
        page: Page<Size4KiB>,
        frame: PhysFrame,
        flags: PageTableFlags,
    ) -> Result<(), MapToError<Size4KiB>> {
        let mut mapper = self.mapper.lock();
        let mut allocator = self.frames.lock();
        unsafe {
            mapper.map_to(page, frame, flags, &mut *allocator)?.flush();
        }
        Ok(())
    }

    /// Returns the physical memory offset used by the mapper.
    pub fn physical_memory_offset(&self) -> VirtAddr {
        self.phys_offset
    }
}

#[cfg(any(feature = "alloc", feature = "std"))]
unsafe fn init_offset_page_table(phys_mem_offset: VirtAddr) -> OffsetPageTable<'static> {
    let level_4_table = active_level_4_table(phys_mem_offset);
    OffsetPageTable::new(level_4_table, phys_mem_offset)
}

#[cfg(any(feature = "alloc", feature = "std"))]
unsafe fn active_level_4_table(phys_mem_offset: VirtAddr) -> &'static mut PageTable {
    let (frame, _) = Cr3::read();
    let phys = frame.start_address();
    let virt = phys_mem_offset + phys.as_u64();
    let page_table_ptr: *mut PageTable = virt.as_mut_ptr();
    &mut *page_table_ptr
}
