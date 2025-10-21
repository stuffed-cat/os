.section .multiboot2, "aw"
.align 8

multiboot2_header_start:
    .long 0xE85250D6              # magic: 4 bytes
    .long 0                       # architecture: i386
    .long (multiboot2_header_end - multiboot2_header_start)  # header length: 4 bytes
    .long -(0xE85250D6 + 0 + (multiboot2_header_end - multiboot2_header_start))  # checksum

    # End tag
    .short 0                      # type: 2 bytes
    .short 0                      # flags: 2 bytes
    .long  8                      # size: 4 bytes

multiboot2_header_end:

.section .text.boot, "ax"
.code32
.globl _start
.extern long_mode_start

.equ PAGE_SIZE,           0x1000
.equ NUM_PD_TABLES,       4               # covers 4 GiB of physical memory
.equ ENTRY_PRESENT_RW,    0x3
.equ PAGE_HUGE,           0x80            # 2 MiB page flag in PD entry
.equ PML4_ENTRIES_DWORDS, 512 * 2         # 512 entries * 8 bytes / 4 bytes
.equ PDP_ENTRIES_DWORDS,  512 * 2
.equ PD_ENTRIES_DWORDS,   (NUM_PD_TABLES * 512 * 2)

_start:
    cli
    cld

    # Preserve Multiboot registers (EAX = magic, EBX = info pointer)
    mov %eax, multiboot_magic_low
    xor %edx, %edx
    mov %edx, multiboot_magic_high
    mov %ebx, multiboot_info_low
    mov %edx, multiboot_info_high

    # Initialize temporary stack
    mov $boot_stack_top, %esp
    mov %esp, %ebp

    call setup_page_tables

    # Load 64-bit GDT
    lgdt gdt64_descriptor

    # Enable PAE (required for long mode)
    mov %cr4, %eax
    or $(1 << 5), %eax            # CR4.PAE
    mov %eax, %cr4

    # Load PML4 physical address into CR3
    mov $pml4_table, %eax
    mov %eax, %cr3

    # Enable Long Mode via IA32_EFER MSR
    mov $0xC0000080, %ecx
    rdmsr
    or $(1 << 8), %eax            # EFER.LME
    wrmsr

    # Enable paging (and ensure protected mode stays enabled)
    mov %cr0, %eax
    or $(1 << 31), %eax           # CR0.PG
    or $1, %eax                   # CR0.PE
    mov %eax, %cr0

    # Far jump to 64-bit code segment
    ljmp $0x08, $long_mode_entry

setup_page_tables:
    push %ebp
    mov %esp, %ebp

    # Zero out page table structures
    xor %eax, %eax
    mov $PML4_ENTRIES_DWORDS, %ecx
    mov $pml4_table, %edi
    rep stosl

    mov $PDP_ENTRIES_DWORDS, %ecx
    mov $pdpt_table, %edi
    rep stosl

    mov $PD_ENTRIES_DWORDS, %ecx
    mov $pd_tables, %edi
    rep stosl

    # Populate PD entries with 2 MiB huge pages (identity mapping)
    mov $pd_tables, %edi
    xor %eax, %eax                # running physical base address
    mov $(NUM_PD_TABLES * 512), %ecx
fill_pd_loop:
    mov %eax, %edx
    or $(ENTRY_PRESENT_RW | PAGE_HUGE), %edx
    mov %edx, (%edi)
    mov $0, 4(%edi)
    add $0x200000, %eax
    add $8, %edi
    loop fill_pd_loop

    # Set up PDPT entries to reference PD tables
    mov $pdpt_table, %edi
    mov $pd_tables, %eax
    mov $NUM_PD_TABLES, %ecx
set_pdpt_loop:
    mov %eax, %edx
    or $ENTRY_PRESENT_RW, %edx
    mov %edx, (%edi)
    mov $0, 4(%edi)
    add $PAGE_SIZE, %eax
    add $8, %edi
    loop set_pdpt_loop

    # PML4[0] -> PDPT (identity mapping)
    mov $pdpt_table, %eax
    or $ENTRY_PRESENT_RW, %eax
    mov %eax, pml4_table
    mov $0, pml4_table + 4

    # PML4[256] -> PDPT (higher-half direct map at 0xffff_8000_0000_0000)
    mov $pdpt_table, %eax
    or $ENTRY_PRESENT_RW, %eax
    mov %eax, pml4_table + (256 * 8)
    mov $0, pml4_table + (256 * 8) + 4

    pop %ebp
    ret

.code64
long_mode_entry:
    # Set up segment registers
    mov $0x10, %ax
    mov %ax, %ds
    mov %ax, %es
    mov %ax, %ss
    mov %ax, %fs
    mov %ax, %gs

    # Set up 64-bit stack
    mov $boot_stack_top, %rsp
    mov %rsp, %rbp

    # Load saved Multiboot parameters
    mov multiboot_magic, %rax
    mov %rax, %rdi
    mov multiboot_info, %rax
    mov %rax, %rsi

    call long_mode_start

halt_loop:
    hlt
    jmp halt_loop

.section .bss.boot, "aw", @nobits
.align 16
boot_stack:
    .space 0x4000
boot_stack_top:

.align 16
multiboot_magic:
multiboot_magic_low:
    .long 0
multiboot_magic_high:
    .long 0

.align 16
multiboot_info:
multiboot_info_low:
    .long 0
multiboot_info_high:
    .long 0

.align 4096
pml4_table:
    .space 4096

.align 4096
pdpt_table:
    .space 4096

.align 4096
pd_tables:
    .space 4096 * NUM_PD_TABLES

.section .data.boot, "aw"
.align 8
gdt64:
    .quad 0x0000000000000000        # null descriptor
    .quad 0x00af9a000000ffff        # kernel code
    .quad 0x00af92000000ffff        # kernel data
gdt64_end:

gdt64_descriptor:
    .word gdt64_end - gdt64 - 1
    .long gdt64


