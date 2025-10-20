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

// Relocation-safe Multiboot2 entry stub. Computes addresses relative to the 32-bit load base
// and reuses that base after entering long mode to avoid absolute relocations.
global_asm!(r#"
    .intel_syntax noprefix
    .extern long_mode_start

    .section .multiboot2_header,"a"
    .align 8
multiboot2_header:
    .long 0xE85250D6
    .long 0
    .long multiboot2_header_end - multiboot2_header
    .long -(0xE85250D6 + 0 + (multiboot2_header_end - multiboot2_header))
    .align 8
    .word 0
    .word 0
    .long 8
multiboot2_header_end:

    .section .text.entry,"ax"
    .globl _start
    .code32
_start:
    cli
    mov esi, eax
    mov ebp, ebx
    call 1f
1:
    pop ebx

    sub esp, 34
    mov edi, esp
    add edi, 10
    xor eax, eax
    mov dword ptr [edi], eax
    mov dword ptr [edi + 4], eax
    mov dword ptr [edi + 8], 0x0000FFFF
    mov dword ptr [edi + 12], 0x00AF9A00
    mov dword ptr [edi + 16], 0x0000FFFF
    mov dword ptr [edi + 20], 0x00AF9200

    mov word ptr [esp], 0x17
    mov eax, edi
    mov dword ptr [esp + 2], eax
    mov dword ptr [esp + 6], 0
    lgdt [esp]

    mov ax, 0x10
    mov ds, ax
    mov es, ax
    mov fs, ax
    mov gs, ax
    mov ss, ax

    mov eax, esp
    and eax, 0xFFFFF000
    sub eax, 0x2000
    mov esp, eax

    mov edi, esp
    mov ecx, 2048
    xor eax, eax
    rep stosd

    mov edi, esp
    mov edx, esp
    add edx, 0x1000

    mov eax, edx
    or eax, 0x3
    mov dword ptr [edi], eax
    mov dword ptr [edi + 4], 0
    mov dword ptr [edi + 2048], eax
    mov dword ptr [edi + 2052], 0

    mov eax, 0x00000083
    mov dword ptr [edx], eax
    mov dword ptr [edx + 4], 0

    mov eax, 0x40000000 | 0x83
    mov dword ptr [edx + 8], eax
    mov dword ptr [edx + 12], 0

    mov eax, 0x80000000 | 0x83
    mov dword ptr [edx + 16], eax
    mov dword ptr [edx + 20], 0

    mov eax, 0xC0000000 | 0x83
    mov dword ptr [edx + 24], eax
    mov dword ptr [edx + 28], 0

    mov eax, edi
    mov cr3, eax

    mov eax, cr4
    or eax, 0x20
    mov cr4, eax

    mov ecx, 0xC0000080
    rdmsr
    or eax, 0x00000100
    wrmsr

    mov eax, cr0
    or eax, 0x80000001
    mov cr0, eax

    push 0x08
    lea eax, [long_mode_entry]
    push eax
    retf

    .code64
long_mode_entry:
    mov ax, 0x10
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov fs, ax
    mov gs, ax

    lea rsp, [rip + boot_stack_top64]

    mov eax, esi
    mov rdi, rax
    mov eax, ebp
    mov rsi, rax

    and rsp, -16
    sub rsp, 8
    call long_mode_start

    hlt

    .section .bss.boot,"aw",@nobits
    .align 16
boot_stack:
    .zero 0x8000
boot_stack_top32:
boot_stack_top64:
"#);

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
