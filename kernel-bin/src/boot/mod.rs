#![cfg(feature = "boot")]

use arrayvec::ArrayVec;
use bootloader_api::info::{MemoryRegionKind, MemoryRegions};
use bootloader_api::BootInfo;
use kernel::{hal::HalConfig, FrameRange, Hal, KernelBuilder};
use linked_list_allocator::LockedHeap;
use x86_64::structures::paging::PageTableFlags;
use x86_64::VirtAddr;

const HEAP_START: u64 = 0x4444_0000_0000;
const HEAP_SIZE: usize = 4 * 1024 * 1024; // 4 MiB heap for early allocations

/// Boot-time coordinator converting bootloader metadata into kernel bootstrap state.
pub fn start(boot_info: &'static mut BootInfo, allocator: &'static LockedHeap) -> ! {
    let phys_offset = boot_info
        .physical_memory_offset
        .into_option()
        .expect("physical memory offset provided");

    let ranges = collect_frame_ranges(&boot_info.memory_regions);
    let hal_config = HalConfig {
        physical_memory_offset: phys_offset,
        frame_ranges: ranges.as_slice(),
    };

    let mut hal = unsafe { Hal::bootstrap(hal_config) }.expect("HAL bootstrap");
    hal.map_heap(
        VirtAddr::new(HEAP_START),
        HEAP_SIZE,
        PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::GLOBAL,
    )
    .expect("heap mapping");

    unsafe {
        allocator.lock().init(HEAP_START as *mut u8, HEAP_SIZE);
    }

    hal.enable_interrupts();

    let mut kernel = KernelBuilder::default().with_hal(hal).build();
    kernel.init().expect("kernel init");
    kernel.run().expect("kernel run");

    halt_loop();
}

fn collect_frame_ranges(regions: &MemoryRegions) -> ArrayVec<FrameRange, 256> {
    let mut ranges = ArrayVec::new();
    for region in regions.iter() {
        if region.kind == MemoryRegionKind::Usable {
            let start = region.start;
            let end = region.end;
            if ranges.try_push(FrameRange::new(start, end)).is_err() {
                break;
            }
        }
    }
    ranges
}

fn halt_loop() -> ! {
    loop {
        x86_64::instructions::hlt();
    }
}
