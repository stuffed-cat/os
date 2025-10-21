.section .multiboot2, "aw"
.align 8

multiboot2_header_start:
    .long 0xE85250D6              # magic: 4 bytes
    .long 0                       # architecture: 4 bytes
    .long (multiboot2_header_end - multiboot2_header_start)  # header length: 4 bytes
    .long -(0xE85250D6 + 0 + (multiboot2_header_end - multiboot2_header_start))  # checksum: 4 bytes
    
    # End tag
    .short 0                      # type: 2 bytes
    .short 0                      # flags: 2 bytes
    .long 8                       # size: 4 bytes

multiboot2_header_end:

.section .text.boot, "ax"
.code32
.globl _start

_start:
    cli
    
    # Write "START" directly to memory at 0xb8000 (VGA text mode buffer)
    mov $0xb8000, %eax
    mov $'S', %bl
    mov $0x0f, %bh              # white on black
    mov %bx, (%eax)
    
    add $2, %eax
    mov $'T', %bl
    mov %bx, (%eax)
    
    add $2, %eax
    mov $'A', %bl
    mov %bx, (%eax)
    
    add $2, %eax
    mov $'R', %bl
    mov %bx, (%eax)
    
    add $2, %eax
    mov $'T', %bl
    mov %bx, (%eax)

halt_loop:
    hlt
    jmp halt_loop

