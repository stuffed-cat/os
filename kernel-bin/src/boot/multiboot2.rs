// Multiboot2启动头和入口点

use core::arch::asm;

// Multiboot2魔数
const MULTIBOOT2_MAGIC: u32 = 0xE85250D6;
const MULTIBOOT2_ARCHITECTURE_I386: u32 = 0;

#[repr(C, align(8))]
struct Multiboot2Header {
    magic: u32,
    architecture: u32,
    header_length: u32,
    checksum: u32,
    // 结束标签
    end_tag_type: u16,
    end_tag_flags: u16,
    end_tag_size: u32,
}

#[used]
#[link_section = ".multiboot2"]
static MULTIBOOT2_HEADER: Multiboot2Header = {
    let header_length = core::mem::size_of::<Multiboot2Header>() as u32;
    let checksum = 0u32
        .wrapping_sub(MULTIBOOT2_MAGIC)
        .wrapping_sub(MULTIBOOT2_ARCHITECTURE_I386)
        .wrapping_sub(header_length);
    
    Multiboot2Header {
        magic: MULTIBOOT2_MAGIC,
        architecture: MULTIBOOT2_ARCHITECTURE_I386,
        header_length,
        checksum,
        end_tag_type: 0,
        end_tag_flags: 0,
        end_tag_size: 8,
    }
};

#[repr(C)]
pub struct Multiboot2Info {
    pub total_size: u32,
    pub reserved: u32,
}

#[no_mangle]
#[link_section = ".boot"]
pub unsafe extern "C" fn _start() -> ! {
    // EAX包含魔数0x36d76289
    // EBX包含multiboot信息结构地址
    
    // 设置栈
    asm!(
        "mov esp, {stack_top}",
        "mov ebp, esp",
        stack_top = in(reg) STACK_TOP,
        options(nostack)
    );
    
    // 调用Rust入口
    asm!(
        "push ebx",  // multiboot info地址
        "push eax",  // 魔数
        "call {entry}",
        entry = sym multiboot_entry,
        options(noreturn)
    );
}

const STACK_SIZE: usize = 1024 * 1024; // 1MB栈
#[link_section = ".bss"]
static mut STACK: [u8; STACK_SIZE] = [0; STACK_SIZE];
const STACK_TOP: usize = unsafe { STACK.as_ptr().add(STACK_SIZE) as usize };

#[no_mangle]
extern "C" fn multiboot_entry(magic: u32, info_addr: u32) -> ! {
    // 验证魔数
    if magic != 0x36d76289 {
        loop {
            x86_64::instructions::hlt();
        }
    }
    
    // 转换为引用
    let _info = unsafe { &*(info_addr as *const Multiboot2Info) };
    
    // TODO: 解析multiboot信息并调用主内核入口
    crate::kernel_main();
}
