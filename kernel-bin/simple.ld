ENTRY(_start)

OUTPUT_FORMAT("elf64-x86-64")
OUTPUT_ARCH("i386:x86-64")

SECTIONS {
    . = 0x100000;

    /* Multiboot2 header at 0x100000 */
    .multiboot2 : {
        KEEP(*(.multiboot2))
        . = ALIGN(16);
    }

    /* Boot code follows - named .text.boot to avoid collision with Rust .text */
    .text.boot : {
        *(.text.boot .text.boot.*)
    }

    /* Rust code goes after boot */
    .text : {
        *(.text .text.*)
        . = ALIGN(4K);
    }

    /* Read-only data */
    .rodata : {
        *(.rodata .rodata.*)
        . = ALIGN(4K);
    }

    /* Initialized data */
    .data : {
        *(.data .data.*)
        . = ALIGN(4K);
    }

    /* Uninitialized data */
    .bss : {
        *(COMMON)
        *(.bss .bss.*)
        . = ALIGN(4K);
    }

    /* Discard unnecessary sections */
    /DISCARD/ : {
        *(.note.GNU-stack)
        *(.gnu_debuglink)
        *(.gnu_debugaltlink)
        *(.eh_frame)
        *(.eh_frame_hdr)
        *(.comment)
        *(.debug_info)
        *(.debug_abbrev)
        *(.debug_aranges)
        *(.debug_line)
        *(.debug_str)
        *(.debug_loc)
    }
}
