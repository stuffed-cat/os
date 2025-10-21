ENTRY(_start)
OUTPUT_FORMAT("elf64-x86-64")

SECTIONS {
    . = 0x100000;
    
    .multiboot2 ALIGN(0x10) : {
        KEEP(*(.multiboot2))
    }
    
    .text.boot : {
        *(.text.boot .text.boot.*)
    }
    
    .text : {
        *(.text .text.*)
    }
    
    .rodata : {
        *(.rodata .rodata.*)
    }
    
    .data : {
        *(.data .data.*)
    }
    
    .bss : {
        *(COMMON)
        *(.bss .bss.*)
        . = ALIGN(8);
    }
    
    /DISCARD/ : {
        *(.debug_*)
        *(.comment)
        *(.eh_frame*)
        *(.gnu_debuglink)
        *(.gnu_debugaltlink)
        *(.note.GNU-stack)
        *(.note*)
        *(.got*)
        *(.symtab)
        *(.strtab)
        *(.shstrtab)
        *(.rela*)
        *(.rel*)
        *(.interp)
        *(.dynsym)
        *(.dynstr)
        *(.dynamic)
        *(.rel.dyn)
        *(.rel.plt)
    }
}
