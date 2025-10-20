#![cfg(feature = "boot")]

use arrayvec::ArrayVec;
use bootloader_api::info::{MemoryRegionKind, MemoryRegions};
use bootloader_api::BootInfo;
use kernel::{
    arch::x86_64::{framebuffer, serial},
    fs,
    hal::HalConfig,
    memory::BootFrameAllocator,
    FrameRange, Hal, KernelBuilder,
};
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

    if let Some(framebuffer) = boot_info.framebuffer.as_mut() {
        framebuffer::init(framebuffer);
    }

    let ranges = collect_frame_ranges(&boot_info.memory_regions);
    let hal_config = HalConfig {
        physical_memory_offset: phys_offset,
    };

    let frame_allocator = BootFrameAllocator::from_frame_ranges(ranges);

    let hal = unsafe { Hal::bootstrap(hal_config, frame_allocator) }.expect("HAL bootstrap");
    hal.map_heap(
        VirtAddr::new(HEAP_START),
        HEAP_SIZE,
        PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::GLOBAL,
    )
    .expect("heap mapping");

    unsafe {
        allocator.lock().init(HEAP_START as *mut u8, HEAP_SIZE);
    }

    if let Some(addr) = boot_info.ramdisk_addr.as_ref().copied() {
        if boot_info.ramdisk_len > 0 {
            match fs::init_from_ramdisk(addr, boot_info.ramdisk_len) {
                Ok(_) => serial::write_str("kernel: filesystem initialized\r\n"),
                Err(_) => serial::write_str("kernel: failed to initialize filesystem\r\n"),
            }
        } else {
            serial::write_str("kernel: ramdisk length is zero\r\n");
        }
    } else {
        serial::write_str("kernel: no ramdisk provided\r\n");
    }

    // CRITICAL: Enable SSE before user mode!
    // Modern glibc and dynamic linker use SSE instructions (movq xmm, etc.)
    enable_sse();
    serial::write_str("kernel: SSE enabled\r\n");

    hal.enable_interrupts();
    serial::write_str("kernel: interrupts enabled\r\n");

    // Initialize IDT and load it before creating kernel
    kernel::arch::x86_64::idt::init();
    kernel::arch::x86_64::idt::load();
    serial::write_str("kernel: IDT initialized\r\n");

    let mut kernel = KernelBuilder::default()
        .with_hal(hal)
        .with_subsystem(kernel::shell::ShellSubsystem::new())
        .build();
    serial::write_str("kernel: builder constructed\r\n");
    kernel.init().expect("kernel init");
    serial::write_str("kernel: init complete\r\n");
    let _ = kernel.run();
    serial::write_str("kernel: run returned\r\n");

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

/// Enable SSE (Streaming SIMD Extensions) support.
/// Required for modern glibc which uses SSE instructions like movq %rsi,%xmm2
fn enable_sse() {
    use x86_64::registers::control::{Cr0, Cr0Flags, Cr4, Cr4Flags};

    unsafe {
        // Clear CR0.EM (bit 2) - Enable FPU emulation = disabled
        // Set CR0.MP (bit 1) - Monitor coprocessor = enabled
        let mut cr0 = Cr0::read();
        cr0.remove(Cr0Flags::EMULATE_COPROCESSOR);
        cr0.insert(Cr0Flags::MONITOR_COPROCESSOR);
        Cr0::write(cr0);

        // Set CR4.OSFXSR (bit 9) - Enable FXSAVE/FXRSTOR
        // Set CR4.OSXMMEXCPT (bit 10) - Enable SSE exceptions
        let mut cr4 = Cr4::read();
        cr4.insert(Cr4Flags::OSFXSR);
        cr4.insert(Cr4Flags::OSXMMEXCPT_ENABLE);
        Cr4::write(cr4);
    }
}

fn halt_loop() -> ! {
    loop {
        x86_64::instructions::hlt();
    }
}
