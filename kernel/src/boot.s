/* nopeekOS – Boot Entry Point
 *
 * Multiboot2 startet in 32-bit Protected Mode.
 * Wir muessen manuell nach Long Mode (64-bit) wechseln:
 *   1. Page Tables aufsetzen (Identity Mapping)
 *   2. PAE + Long Mode aktivieren
 *   3. Paging einschalten
 *   4. GDT mit 64-bit Code Segment laden
 *   5. Far Jump nach 64-bit Code
 *   6. Rust kernel_main aufrufen
 *
 * Das ist das unvermeidbare Hardware-Ritual.
 * Alles danach ist nopeekOS.
 */

.section .multiboot2, "a"
.align 8

/* Multiboot2 Header */
multiboot2_header:
    .long 0xE85250D6
    .long 0
    .long multiboot2_header_end - multiboot2_header
    .long -(0xE85250D6 + 0 + (multiboot2_header_end - multiboot2_header))

    /* Framebuffer tag: request linear framebuffer from GRUB */
    .align 8
    .short 5        /* type = framebuffer */
    .short 0        /* flags = required */
    .long 20        /* size */
    .long 1920      /* preferred width */
    .long 1080      /* preferred height */
    .long 32        /* preferred bpp */

    /* End tag */
    .align 8
    .short 0
    .short 0
    .long 8
multiboot2_header_end:

/* ============================================================
 * 32-bit Entry Point
 * ============================================================ */
.section .text
.code32
.global _start

_start:
    /* Multiboot-Werte in callee-saved Registern sichern */
    mov %eax, %esi
    mov %ebx, %edi

    /* BSS nullen (BEVOR wir Werte in BSS schreiben!) */
    push %esi
    push %edi
    mov $__bss_start, %edi
    mov $__bss_end, %ecx
    sub %edi, %ecx
    shr $2, %ecx
    xor %eax, %eax
    rep stosl
    pop %edi
    pop %esi

    /* Jetzt Multiboot-Werte in BSS speichern */
    mov %esi, (multiboot_magic)
    mov %edi, (multiboot_info)
    mov $__stack_top, %esp

    call check_cpuid
    call check_long_mode
    call setup_page_tables
    call enable_paging

    lgdt (gdt64_pointer)
    ljmp $0x08, $long_mode_start

check_cpuid:
    pushfl
    pop %eax
    mov %eax, %ecx
    xor $0x200000, %eax
    push %eax
    popfl
    pushfl
    pop %eax
    push %ecx
    popfl
    cmp %ecx, %eax
    je .no_cpuid
    ret
.no_cpuid:
    hlt

check_long_mode:
    mov $0x80000000, %eax
    cpuid
    cmp $0x80000001, %eax
    jb .no_long_mode
    mov $0x80000001, %eax
    cpuid
    test $0x20000000, %edx
    jz .no_long_mode
    ret
.no_long_mode:
    hlt

setup_page_tables:
    /* PML4[0] -> PDPT */
    mov $pdpt, %eax
    or $0x03, %eax
    mov %eax, (pml4)

    /* PDPT: 64 x 1GB Huge Pages = 64GB Identity Map */
    /* Bit 0=Present, Bit 1=Writable, Bit 7=Huge (1GB page) */
    mov $pdpt, %edi
    movl $0x83, %eax        /* 0x83 = Present + Writable + Huge */
    mov $0, %edx            /* high 32 bits of address (starts at 0) */
    mov $0, %ecx
.fill_pdpt:
    mov %eax, (%edi)        /* low 32 bits */
    mov %edx, 4(%edi)       /* high 32 bits */
    add $0x40000000, %eax   /* +1GB */
    jnc .no_carry
    inc %edx                /* carry into high 32 bits for addresses > 4GB */
.no_carry:
    add $8, %edi
    inc %ecx
    cmp $64, %ecx           /* 64 entries = 64GB */
    jne .fill_pdpt
    ret

enable_paging:
    mov $pml4, %eax
    mov %eax, %cr3

    /* CR4: PAE | OSFXSR | OSXMMEXCPT (SSE/AES-NI bring-up) */
    mov %cr4, %eax
    or $0x620, %eax
    mov %eax, %cr4

    /* Long Mode (EFER.8) */
    mov $0xC0000080, %ecx
    rdmsr
    or $0x100, %eax
    wrmsr

    /* CR0: clear EM, set MP|NE (FPU/SSE), then PG (paging) */
    mov %cr0, %eax
    and $~4, %eax
    or $0x80000022, %eax
    mov %eax, %cr0
    ret

/* ============================================================
 * 64-bit Entry Point
 * ============================================================ */
.code64

long_mode_start:
    mov $0x10, %ax
    mov %ax, %ds
    mov %ax, %es
    mov %ax, %fs
    mov %ax, %gs
    mov %ax, %ss

    mov $__stack_top, %rsp

    /* Initialize FPU/SSE state (CR0/CR4 already set by enable_paging) */
    fninit
    pushq $0x1F80
    ldmxcsr (%rsp)
    add $8, %rsp

    /* Argumente fuer kernel_main(magic, info) */
    xor %rdi, %rdi
    mov (multiboot_magic), %edi
    xor %rsi, %rsi
    mov (multiboot_info), %esi

    call kernel_main

.halt64:
    cli
    hlt
    jmp .halt64

/* ============================================================
 * Read-Only Data
 * ============================================================ */
.section .rodata
.align 16

gdt64:
    .quad 0
gdt64_code:
    .quad 0x00AF9A000000FFFF
gdt64_data:
    .quad 0x00CF92000000FFFF
gdt64_end:

gdt64_pointer:
    .short gdt64_end - gdt64 - 1
    .long gdt64

/* ============================================================
 * BSS (zeroed)
 * ============================================================ */
.section .bss
.align 4096

pml4:   .space 4096
pdpt:   .space 4096

.align 4
multiboot_magic: .space 4
multiboot_info:  .space 4
