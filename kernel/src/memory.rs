//! Virtual memory and capability aware allocator scaffolding.

use alloc::collections::BTreeMap;
use core::ops::Range;
use log::trace;
use spin::RwLock;

use crate::error::SubsystemError;

#[cfg(any(feature = "alloc", feature = "std"))]
use crate::user::{AddressSpace, MemoryFlags, SegmentMapping, Stack, StackImage};

#[cfg(any(feature = "alloc", feature = "std"))]
use alloc::vec::Vec;
#[cfg(any(feature = "alloc", feature = "std"))]
use arrayvec::ArrayVec;
#[cfg(any(feature = "alloc", feature = "std"))]
use core::ptr::{self, NonNull};
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

    fn start(&self) -> PhysAddr {
        self.start
    }

    fn end(&self) -> PhysAddr {
        self.end
    }

    #[allow(dead_code)]
    fn contains(&self, addr: PhysAddr) -> bool {
        addr >= self.start && addr < self.end
    }
}

#[cfg(any(feature = "alloc", feature = "std"))]
fn align_up(addr: u64, align: u64) -> u64 {
    (addr + align - 1) & !(align - 1)
}

const FRAME_RANGE_CAPACITY: usize = 256;

/// Boot-time frame allocator based on the list of available ranges.
#[cfg(any(feature = "alloc", feature = "std"))]
pub struct BootFrameAllocator {
    ranges: ArrayVec<FrameRange, FRAME_RANGE_CAPACITY>,
    current_range: usize,
    next_addr: u64,
}

#[cfg(any(feature = "alloc", feature = "std"))]
impl BootFrameAllocator {
    /// Builds a frame allocator from the provided frame ranges.
    pub fn from_frame_ranges(ranges: ArrayVec<FrameRange, FRAME_RANGE_CAPACITY>) -> Self {
        Self {
            ranges,
            current_range: 0,
            next_addr: 0,
        }
    }

    fn allocate(&mut self) -> Option<PhysFrame> {
        while self.current_range < self.ranges.len() {
            let range = self.ranges[self.current_range];
            if self.next_addr == 0 {
                self.next_addr = align_up(range.start().as_u64(), Size4KiB::SIZE);
            }

            if self.next_addr + Size4KiB::SIZE <= range.end().as_u64() {
                let frame = PhysFrame::containing_address(PhysAddr::new(self.next_addr));
                self.next_addr += Size4KiB::SIZE;
                return Some(frame);
            }

            self.current_range += 1;
            self.next_addr = 0;
        }
        None
    }
}

#[cfg(any(feature = "alloc", feature = "std"))]
unsafe impl FrameAllocator<Size4KiB> for BootFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
        self.allocate()
    }
}

#[cfg(any(feature = "alloc", feature = "std"))]
struct FramePool {
    base: BootFrameAllocator,
    recycled: Vec<PhysFrame>,
}

#[cfg(any(feature = "alloc", feature = "std"))]
impl FramePool {
    fn new(base: BootFrameAllocator) -> Self {
        Self {
            base,
            recycled: Vec::new(),
        }
    }

    fn allocate(&mut self) -> Option<PhysFrame> {
        if let Some(frame) = self.recycled.pop() {
            return Some(frame);
        }
        self.base.allocate()
    }

    fn recycle(&mut self, frame: PhysFrame) {
        self.recycled.push(frame);
    }

    fn recycle_many<I>(&mut self, frames: I)
    where
        I: IntoIterator<Item = PhysFrame>,
    {
        self.recycled.extend(frames);
    }
}

/// Page table entries reserved for kernel space (upper canonical half).
#[cfg(any(feature = "alloc", feature = "std"))]
#[cfg(any(feature = "alloc", feature = "std"))]
struct FrameAllocatorAdapter<'a> {
    manager: &'a MemoryManager,
    recorder: Option<NonNull<PageTableHandle>>,
}

#[cfg(any(feature = "alloc", feature = "std"))]
impl<'a> FrameAllocatorAdapter<'a> {
    fn new(manager: &'a MemoryManager) -> Self {
        Self {
            manager,
            recorder: None,
        }
    }

    fn with_recorder(manager: &'a MemoryManager, handle: NonNull<PageTableHandle>) -> Self {
        Self {
            manager,
            recorder: Some(handle),
        }
    }
}

#[cfg(any(feature = "alloc", feature = "std"))]
unsafe impl<'a> FrameAllocator<Size4KiB> for FrameAllocatorAdapter<'a> {
    fn allocate_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
        let frame = self.manager.allocate_zeroed_frame()?;
        if let Some(mut handle) = self.recorder {
            unsafe {
                handle.as_mut().record_page_table(frame);
            }
        }
        Some(frame)
    }
}

/// Handle owning the root and intermediate page table frames for a process.
#[cfg(any(feature = "alloc", feature = "std"))]
pub struct PageTableHandle {
    manager: NonNull<MemoryManager>,
    root: PhysFrame,
    tables: Vec<PhysFrame>,
    mappings: Vec<PhysFrame>,
}

#[cfg(any(feature = "alloc", feature = "std"))]
impl PageTableHandle {
    fn new(manager: &MemoryManager, root: PhysFrame) -> Self {
        Self {
            manager: NonNull::from(manager),
            root,
            tables: Vec::new(),
            mappings: Vec::new(),
        }
    }

    /// Returns the physical frame associated with the root PML4 table.
    pub fn root(&self) -> PhysFrame {
        self.root
    }

    /// Tracks an intermediate page table frame allocated while mapping regions.
    pub fn record_page_table(&mut self, frame: PhysFrame) {
        self.tables.push(frame);
    }

    /// Tracks a data frame mapped into the address space.
    pub fn record_mapping(&mut self, frame: PhysFrame) {
        self.mappings.push(frame);
    }
}

#[cfg(any(feature = "alloc", feature = "std"))]
impl Drop for PageTableHandle {
    fn drop(&mut self) {
        let manager = unsafe { self.manager.as_ref() };
        if !self.tables.is_empty() {
            let tables = core::mem::take(&mut self.tables);
            manager.recycle_frames(tables);
        }
        if !self.mappings.is_empty() {
            let mappings = core::mem::take(&mut self.mappings);
            manager.recycle_frames(mappings);
        }
        manager.recycle_frame(self.root);
    }
}

/// Controller for paging using an offset page table and boot-time allocator.
#[cfg(any(feature = "alloc", feature = "std"))]
pub struct MemoryManager {
    mapper: Mutex<OffsetPageTable<'static>>,
    frames: Mutex<FramePool>,
    phys_offset: VirtAddr,
    kernel_root: PhysFrame,
}

#[cfg(any(feature = "alloc", feature = "std"))]
impl MemoryManager {
    /// Creates a new memory manager instance.
    pub unsafe fn new(phys_mem_offset: VirtAddr, allocator: BootFrameAllocator) -> Self {
        let mapper = init_offset_page_table(phys_mem_offset);
        let (kernel_root, _) = Cr3::read();
        Self {
            mapper: Mutex::new(mapper),
            frames: Mutex::new(FramePool::new(allocator)),
            phys_offset: phys_mem_offset,
            kernel_root,
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
        let mut table_allocator = FrameAllocatorAdapter::new(self);

        let start_page = Page::containing_address(start);
        let end_page = Page::containing_address(start + (size as u64 - 1));

        for page in Page::range_inclusive(start_page, end_page) {
            let frame = self
                .allocate_zeroed_frame()
                .ok_or(MapToError::FrameAllocationFailed)?;
            unsafe {
                mapper
                    .map_to(page, frame, flags, &mut table_allocator)?
                    .flush();
            }
        }
        Ok(())
    }

    /// Builds a user address space by cloning kernel mappings and inserting user segments.
    pub fn map_address_space(
        &self,
        layout: &AddressSpace,
    ) -> Result<PageTableHandle, SubsystemError> {
        let mut handle = self.clone_kernel_page_table()?;
        let handle_ptr = NonNull::from(&mut handle);
        let root_virt = self.phys_offset + handle.root().start_address().as_u64();
        let root_table: &mut PageTable = unsafe { &mut *(root_virt.as_mut_ptr::<PageTable>()) };
        let mut mapper = unsafe { OffsetPageTable::new(root_table, self.phys_offset) };
        let mut allocator = FrameAllocatorAdapter::with_recorder(self, handle_ptr);
        for segment in layout.segments() {
            self.map_segment_into(&mut mapper, &mut allocator, handle_ptr, segment)?;
        }
        self.map_stack_into(&mut mapper, &mut allocator, handle_ptr, layout.stack())?;
        Ok(handle)
    }

    fn map_segment_into(
        &self,
        mapper: &mut OffsetPageTable<'static>,
        allocator: &mut FrameAllocatorAdapter<'_>,
        mut handle: NonNull<PageTableHandle>,
        segment: &SegmentMapping,
    ) -> Result<(), SubsystemError> {
        if segment.length() == 0 {
            return Ok(());
        }

        let start = VirtAddr::new(segment.base());
        let end_addr = segment.base() + segment.length() as u64 - 1;
        let end = VirtAddr::new(end_addr);
        let start_page = Page::containing_address(start);
        let end_page = Page::containing_address(end);
        let mut flags = Self::flags_from_memory(segment.permissions());

        trace!(
            "memory: map segment [{:#x}, {:#x}) perms={:?} -> computed_flags={:?}",
            segment.base(),
            segment.base() + segment.length() as u64,
            segment.permissions(),
            flags
        );
        
        // x86_64 crate requires WRITABLE for initial page table setup
        // We'll use set_flags to fix permissions after mapping
        let needs_write_for_setup = !flags.contains(PageTableFlags::WRITABLE);
        if needs_write_for_setup {
            flags |= PageTableFlags::WRITABLE;
            trace!("memory: temporarily adding WRITABLE for setup");
        }

        for page in Page::range_inclusive(start_page, end_page) {
            let frame = self
                .allocate_zeroed_frame()
                .ok_or(SubsystemError::Resource("out of physical frames"))?;
            unsafe {
                mapper
                    .map_to(page, frame, flags, allocator)
                    .map_err(|_| SubsystemError::Runtime("page mapping failed"))?
                    .flush();
                handle.as_mut().record_mapping(frame);
            }
            self.copy_segment_into_frame(frame, segment, page);
        }
        
        // Fix permissions for read-only segments
        if needs_write_for_setup {
            let correct_flags = Self::flags_from_memory(segment.permissions());
            trace!("memory: fixing permissions to {:?}", correct_flags);
            for page in Page::range_inclusive(start_page, end_page) {
                unsafe {
                    if mapper.translate_page(page).is_ok() {
                        mapper
                            .update_flags(page, correct_flags)
                            .map_err(|_| SubsystemError::Runtime("failed to update page flags"))?
                            .flush();
                    }
                }
            }
        }
        
        Ok(())
    }

    fn map_stack_into(
        &self,
        mapper: &mut OffsetPageTable<'static>,
        allocator: &mut FrameAllocatorAdapter<'_>,
        mut handle: NonNull<PageTableHandle>,
        stack: &Stack,
    ) -> Result<(), SubsystemError> {
        let start = VirtAddr::new(stack.base());
        let end_addr = stack.top().saturating_sub(1);
        if end_addr < stack.base() {
            return Ok(());
        }
        let end = VirtAddr::new(end_addr);
        let start_page = Page::containing_address(start);
        let end_page = Page::containing_address(end);
        let flags = Self::stack_flags();

        for page in Page::range_inclusive(start_page, end_page) {
            let frame = self
                .allocate_zeroed_frame()
                .ok_or(SubsystemError::Resource("out of physical frames"))?;
            unsafe {
                mapper
                    .map_to(page, frame, flags, allocator)
                    .map_err(|_| SubsystemError::Runtime("stack mapping failed"))?
                    .flush();
                handle.as_mut().record_mapping(frame);
            }
            if let Some(image) = stack.image() {
                self.copy_stack_image_into_frame(frame, stack, image, page);
            }
        }
        Ok(())
    }

    fn copy_segment_into_frame(
        &self,
        frame: PhysFrame,
        segment: &SegmentMapping,
        page: Page<Size4KiB>,
    ) {
        let page_start = page.start_address().as_u64();
        let page_end = page_start + Size4KiB::SIZE; // exclusive upper bound
        let seg_start = segment.base();
        let seg_end = segment.base() + segment.length() as u64;

        let copy_start = core::cmp::max(seg_start, page_start);
        let copy_end = core::cmp::min(seg_end, page_end);
        if copy_end <= copy_start {
            return;
        }

        let payload_offset = (copy_start - seg_start) as usize;
        let copy_len = (copy_end - copy_start) as usize;
        let dest_offset = (copy_start - page_start) as usize;

        // Only copy what's actually in the payload (rest is already zeroed)
        let available_payload = segment.payload().len().saturating_sub(payload_offset);
        let actual_copy_len = core::cmp::min(copy_len, available_payload);

        if actual_copy_len > 0 {
            let phys = frame.start_address().as_u64();
            let virt = self.phys_offset + phys;
            unsafe {
                let dest = virt.as_mut_ptr::<u8>().add(dest_offset);
                let src = segment.payload()[payload_offset..payload_offset + actual_copy_len].as_ptr();
                ptr::copy_nonoverlapping(src, dest, actual_copy_len);
            }
        }
    }

    fn flags_from_memory(flags: MemoryFlags) -> PageTableFlags {
        let mut result = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
        
        // Set WRITABLE for segments that need write permission
        if flags.contains(MemoryFlags::WRITE) {
            result |= PageTableFlags::WRITABLE;
        }
        
        // Set NO_EXECUTE for non-executable segments (data, rodata, etc.)
        // Only code segments should be executable
        if !flags.contains(MemoryFlags::EXEC) {
            result |= PageTableFlags::NO_EXECUTE;
        }
        
        result
    }

    fn stack_flags() -> PageTableFlags {
        PageTableFlags::PRESENT
            | PageTableFlags::USER_ACCESSIBLE
            | PageTableFlags::WRITABLE
            | PageTableFlags::NO_EXECUTE
    }
    /// Maps a single page to the given frame.
    pub fn map_to(
        &self,
        page: Page<Size4KiB>,
        frame: PhysFrame,
        flags: PageTableFlags,
    ) -> Result<(), MapToError<Size4KiB>> {
        let mut mapper = self.mapper.lock();
        let mut table_allocator = FrameAllocatorAdapter::new(self);
        unsafe {
            mapper
                .map_to(page, frame, flags, &mut table_allocator)?
                .flush();
        }
        Ok(())
    }

    fn copy_stack_image_into_frame(
        &self,
        frame: PhysFrame,
        stack: &Stack,
        image: &StackImage,
        page: Page<Size4KiB>,
    ) {
        let page_start = page.start_address().as_u64();
        let page_end = page_start + Size4KiB::SIZE;
        let image_start = stack.base() + image.offset() as u64;
        let image_end = image_start + image.data().len() as u64;

        let copy_start = core::cmp::max(image_start, page_start);
        let copy_end = core::cmp::min(image_end, page_end);
        if copy_end <= copy_start {
            return;
        }

        let payload_offset = (copy_start - image_start) as usize;
        let copy_len = (copy_end - copy_start) as usize;
        let dest_offset = (copy_start - page_start) as usize;

        let phys = frame.start_address().as_u64();
        let virt = self.phys_offset + phys;
        unsafe {
            let dest = virt.as_mut_ptr::<u8>().add(dest_offset);
            let src = &image.data()[payload_offset..payload_offset + copy_len];
            ptr::copy_nonoverlapping(src.as_ptr(), dest, copy_len);
        }
    }

    /// Returns the physical memory offset used by the mapper.
    pub fn physical_memory_offset(&self) -> VirtAddr {
        self.phys_offset
    }

    /// Returns the physical frame for the kernel's active PML4 table.
    pub fn kernel_root_frame(&self) -> PhysFrame {
        self.kernel_root
    }

    /// Creates a new page table hierarchy seeded with kernel-space mappings.
    pub fn clone_kernel_page_table(&self) -> Result<PageTableHandle, SubsystemError> {
        let root = self
            .allocate_zeroed_frame()
            .ok_or(SubsystemError::Resource("out of physical frames"))?;

        self.with_page_table(root, |new_root| {
            self.with_page_table_read(self.kernel_root, |kernel_root| {
                for index in 0..512 {
                    let entry = &kernel_root[index];
                    let flags = entry.flags();
                    let is_kernel_mapping = flags.contains(PageTableFlags::PRESENT)
                        && !flags.contains(PageTableFlags::USER_ACCESSIBLE);
                    if is_kernel_mapping {
                        // Clone the entry as-is for kernel mappings
                        // NX bit handling is done at the leaf PTE level for user pages
                        log::trace!(
                            "memory: cloning kernel pml4 entry {} flags={:?}",
                            index,
                            flags
                        );
                        new_root[index].set_addr(entry.addr(), flags);
                    }
                }
            });
        });

        Ok(PageTableHandle::new(self, root))
    }

    fn allocate_zeroed_frame(&self) -> Option<PhysFrame> {
        let frame = {
            let mut pool = self.frames.lock();
            pool.allocate()
        }?;
        self.zero_frame(frame);
        Some(frame)
    }

    fn recycle_frame(&self, frame: PhysFrame) {
        let mut pool = self.frames.lock();
        pool.recycle(frame);
    }

    fn recycle_frames<I>(&self, frames: I)
    where
        I: IntoIterator<Item = PhysFrame>,
    {
        let mut pool = self.frames.lock();
        pool.recycle_many(frames);
    }

    fn zero_frame(&self, frame: PhysFrame) {
        let virt = self.phys_offset + frame.start_address().as_u64();
        unsafe {
            let ptr = virt.as_u64() as *mut u8;
            ptr::write_bytes(ptr, 0, Size4KiB::SIZE as usize);
        }
    }

    fn with_page_table<F, T>(&self, frame: PhysFrame, f: F) -> T
    where
        F: FnOnce(&mut PageTable) -> T,
    {
        let virt = self.phys_offset + frame.start_address().as_u64();
        unsafe {
            let table: &mut PageTable = &mut *(virt.as_u64() as *mut PageTable);
            f(table)
        }
    }

    fn with_page_table_read<F, T>(&self, frame: PhysFrame, f: F) -> T
    where
        F: FnOnce(&PageTable) -> T,
    {
        let virt = self.phys_offset + frame.start_address().as_u64();
        unsafe {
            let table: &PageTable = &*(virt.as_u64() as *const PageTable);
            f(table)
        }
    }
}

#[cfg(any(feature = "alloc", feature = "std"))]
unsafe impl Send for MemoryManager {}

#[cfg(any(feature = "alloc", feature = "std"))]
unsafe impl Sync for MemoryManager {}

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
