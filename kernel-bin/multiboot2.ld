ENTRY(_start)

SECTIONS
{
    . = 1M;

    .multiboot2 ALIGN(8) : {
        KEEP(*(.multiboot2))
    }

    .boot ALIGN(4K) : {
        KEEP(*(.boot))
    }

    .text ALIGN(4K) : {
        *(.text .text.*)
    }

    .rodata ALIGN(4K) : {
        *(.rodata .rodata.*)
    }

    .data ALIGN(4K) : {
        *(.data .data.*)
    }

    .bss ALIGN(4K) : {
        *(COMMON)
        *(.bss .bss.*)
    }

    /DISCARD/ : {
        *(.comment)
        *(.eh_frame)
    }
}
