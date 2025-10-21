use core::arch::global_asm;

use kernel::boot_info::{
    BootInfo, Framebuffer, FramebufferInfo, MemoryRegion, MemoryRegionKind, PixelFormat,
};
use kernel::arch::x86_64::serial;
use multiboot2::{
    BootInformation, BootInformationHeader, FramebufferTag, FramebufferType, MemoryAreaType,
};
use x86_64::instructions::hlt;

pub const PHYSICAL_MEMORY_OFFSET: u64 = 0xffff_8000_0000_0000;

static mut BOOT_INFO_STORAGE: BootInfo = BootInfo::new();

// Boot code is in boot.s

#[no_mangle]
pub extern "C" fn long_mode_start(magic: u64, info_ptr: u64) -> ! {
    const MULTIBOOT2_MAGIC: u32 = 0x36D7_6289;

    if magic as u32 != MULTIBOOT2_MAGIC {
        serial::write_str("boot: invalid multiboot magic\r\n");
        super::halt_loop()
    }

    let boot_information = unsafe {
        match BootInformation::load(info_ptr as *const BootInformationHeader) {
            Ok(info) => info,
            Err(_) => {
                serial::write_str("boot: failed to load multiboot info\r\n");
                loop {
                    hlt();
                }
            }
        }
    };

    let mut info = BootInfo::new();
    info.physical_memory_offset = Some(PHYSICAL_MEMORY_OFFSET);

    if let Some(memory_map_tag) = boot_information.memory_map_tag() {
        for area in memory_map_tag.memory_areas() {
            let kind = match MemoryAreaType::from(area.typ()) {
                MemoryAreaType::Available => MemoryRegionKind::Usable,
                MemoryAreaType::Reserved => MemoryRegionKind::Reserved,
                MemoryAreaType::AcpiAvailable => MemoryRegionKind::AcpiReclaimable,
                MemoryAreaType::ReservedHibernate => MemoryRegionKind::AcpiNvs,
                MemoryAreaType::Defective => MemoryRegionKind::BadMemory,
                MemoryAreaType::Custom(_) => MemoryRegionKind::Unknown,
            };
            info.memory_regions.push(MemoryRegion {
                start: area.start_address(),
                end: area.end_address(),
                kind,
            });
        }
    } else {
        serial::write_str("boot: missing memory map\r\n");
    }

    if let Some(module) = boot_information.module_tags().next() {
        let start = module.start_address() as u64;
        let end = module.end_address() as u64;
        info.ramdisk_addr = Some(PHYSICAL_MEMORY_OFFSET + start);
        info.ramdisk_len = end.saturating_sub(start);
    }

    if let Some(framebuffer_tag) = boot_information.framebuffer_tag() {
        match framebuffer_tag {
            Ok(tag) => {
                if let Some(framebuffer) = build_framebuffer(tag) {
                    info.framebuffer = Some(framebuffer);
                }
            }
            Err(_) => serial::write_str("boot: unsupported framebuffer format\r\n"),
        }
    }

    unsafe {
        BOOT_INFO_STORAGE = info;
        super::start(&mut BOOT_INFO_STORAGE, &crate::GLOBAL_ALLOCATOR)
    }
}

fn build_framebuffer(tag: &FramebufferTag) -> Option<Framebuffer> {
    let bytes_per_pixel = ((tag.bpp() + 7) / 8) as u8;
    if bytes_per_pixel == 0 {
        return None;
    }
    let stride_pixels = match bytes_per_pixel as usize {
        0 => return None,
        bpp => (tag.pitch() as usize) / bpp,
    };
    let buffer_len = (tag.pitch() as usize) * (tag.height() as usize);
    if buffer_len == 0 {
        return None;
    }

    let framebuffer_type = tag.buffer_type().ok()?;

    let pixel_format = match framebuffer_type {
        FramebufferType::RGB { red, blue, .. } => {
            let red_position = red.position;
            let blue_position = blue.position;
            if red_position == 16 && blue_position == 0 {
                PixelFormat::Rgb
            } else if red_position == 0 && blue_position == 16 {
                PixelFormat::Bgr
            } else {
                PixelFormat::Unknown
            }
        }
        FramebufferType::Indexed { .. } | FramebufferType::Text => PixelFormat::U8,
    };

    let info = FramebufferInfo::new(
        tag.width() as usize,
        tag.height() as usize,
        stride_pixels,
        bytes_per_pixel,
        pixel_format,
    );

    Some(Framebuffer::new(
        PHYSICAL_MEMORY_OFFSET + tag.address() as u64,
        buffer_len,
        info,
    ))
}

