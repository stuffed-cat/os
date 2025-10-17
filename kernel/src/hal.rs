//! Hardware abstraction layer bridging architecture and services.

use x86_64::structures::paging::PageTableFlags;
use x86_64::VirtAddr;

use crate::{
    arch::x86_64::{interrupts::InterruptController, ArchBootstrap, Pic8259Controller},
    error::KernelError,
    memory::{BootFrameAllocator, FrameRange, MemoryManager},
};

/// Configuration input required to bootstrap the HAL.
pub struct HalConfig<'a> {
    /// Virtual address offset where physical memory is mapped.
    pub physical_memory_offset: u64,
    /// Physical memory ranges available for allocation.
    pub frame_ranges: &'a [FrameRange],
}

/// HAL facade for managing interrupts and paging.
pub struct Hal {
    interrupts: Pic8259Controller,
    memory: MemoryManager,
}

impl Hal {
    /// Performs early CPU initialization and constructs the HAL instance.
    pub unsafe fn bootstrap(config: HalConfig<'_>) -> Result<Self, KernelError> {
        ArchBootstrap::init_cpu_features()?;
        ArchBootstrap::validate_virtualization()?;

        let allocator = BootFrameAllocator::from_ranges(config.frame_ranges);
        let memory = MemoryManager::new(VirtAddr::new(config.physical_memory_offset), allocator);

        let controller = Pic8259Controller::new();
        controller.init();
        ArchBootstrap::init_interrupts()?;

        Ok(Self {
            interrupts: controller,
            memory,
        })
    }

    /// Enables interrupts globally.
    pub fn enable_interrupts(&self) {
        self.interrupts.enable();
    }

    /// Disables interrupts globally.
    pub fn disable_interrupts(&self) {
        self.interrupts.disable();
    }

    /// Returns a reference to the interrupt controller.
    pub fn interrupts(&self) -> &Pic8259Controller {
        &self.interrupts
    }

    /// Returns the memory manager.
    pub fn memory(&self) -> &MemoryManager {
        &self.memory
    }

    /// Convenience helper to map and initialize the kernel heap.
    pub fn map_heap(
        &self,
        start: VirtAddr,
        size: usize,
        flags: PageTableFlags,
    ) -> Result<(), KernelError> {
        self.memory
            .map_region(start, size, flags)
            .map_err(|_| KernelError::Memory("heap mapping failed"))
    }
}
