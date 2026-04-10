/* nopeekOS SMP — AP Trampoline
 *
 * Copied to physical 0x8000 at runtime. APs begin here in 16-bit real mode
 * after INIT-SIPI-SIPI from BSP.
 *
 * Layout at BOOT (0x8000):
 *   0x000  Code: 16-bit → 32-bit → 64-bit
 *   0x0C0  GDT32 (null + code32 + data32)
 *   0x0D8  GDT32 pointer
 *   0x0E0  GDT64 pointer (filled by BSP)
 *   0x0F0  CR3 (u32, filled by BSP)
 *   0x0F8  Stack top (u64, filled by BSP)
 *   0x100  Rust entry (u64, filled by BSP)
 *   0x108  Core ID (u32, filled by BSP)
 *   0x10C  AP running flag (u32, set to 1 by AP)
 *   0x110  IDTR (10 bytes, filled by BSP)
 */

.set BOOT, 0x8000

.section .smp_trampoline, "a"

.global smp_trampoline_start
.global smp_trampoline_end

smp_trampoline_start:

/* === 16-bit Real Mode === */
.code16
    cli
    cld
    xor %ax, %ax
    mov %ax, %ds
    mov %ax, %es
    mov %ax, %ss

    /* Load 32-bit GDT */
    lgdtl (BOOT + 0xD8)

    /* Enable Protected Mode (CR0.PE) */
    mov %cr0, %eax
    or $1, %eax
    mov %eax, %cr0

    /* Far jump to 32-bit: manual encoding (0x66 prefix + EA + 32-bit offset + segment) */
    .byte 0x66, 0xEA
    .long BOOT + (smp_pm32 - smp_trampoline_start)
    .word 0x08

/* === 32-bit Protected Mode === */
.code32
smp_pm32:
    mov $0x10, %ax
    mov %ax, %ds
    mov %ax, %es
    mov %ax, %ss

    /* Enable PAE (CR4.5) */
    mov %cr4, %eax
    or $0x20, %eax
    mov %eax, %cr4

    /* Load CR3 from data area */
    mov (BOOT + 0xF0), %eax
    mov %eax, %cr3

    /* Enable Long Mode (EFER.LME) + NX (EFER.NXE) */
    mov $0xC0000080, %ecx
    rdmsr
    or $0x900, %eax         /* bit 8 = LME, bit 11 = NXE */
    wrmsr

    /* Enable Paging (CR0.PG) */
    mov %cr0, %eax
    or $0x80000000, %eax
    mov %eax, %cr0

    /* Load 64-bit GDT (written by BSP at BOOT+0xE0) */
    lgdt (BOOT + 0xE0)

    /* Far jump to 64-bit */
    .byte 0xEA
    .long BOOT + (smp_lm64 - smp_trampoline_start)
    .word 0x08

/* === 64-bit Long Mode === */
.code64
smp_lm64:
    mov $0x10, %ax
    mov %ax, %ds
    mov %ax, %es
    mov %ax, %ss
    xor %ax, %ax
    mov %ax, %fs
    mov %ax, %gs

    /* Base register for data area access */
    mov $BOOT, %rbx

    /* Load BSP's IDT */
    lidt 0x110(%rbx)

    /* Per-AP state */
    mov 0xF8(%rbx), %rsp       /* stack top */
    mov 0x100(%rbx), %rax      /* Rust entry point */
    mov 0x108(%rbx), %edi      /* core ID (arg 1, System V ABI) */
    movl $1, 0x10C(%rbx)       /* signal BSP: AP is alive */

    /* Enter Rust — never returns */
    jmp *%rax

    cli
1:  hlt
    jmp 1b

/* === GDT32: 3 entries (offset 0xC0) === */
.org smp_trampoline_start + 0xC0
    .quad 0                         /* Null */
    .quad 0x00CF9A000000FFFF        /* Code32: base=0, limit=4G, exec/read */
    .quad 0x00CF92000000FFFF        /* Data32: base=0, limit=4G, read/write */

/* GDT32 pointer (offset 0xD8) */
.org smp_trampoline_start + 0xD8
    .word 23                        /* limit: 3*8 - 1 */
    .long BOOT + 0xC0              /* base at runtime address */

/* GDT64 pointer (offset 0xE0, filled by BSP from SGDT) */
.org smp_trampoline_start + 0xE0
    .word 0
    .long 0
    .long 0                         /* 12 bytes reserved */

/* Data area (offset 0xF0, filled by BSP) */
.org smp_trampoline_start + 0xF0
    .long 0                         /* +0xF0: CR3 */
    .long 0                         /* +0xF4: reserved */
    .quad 0                         /* +0xF8: stack top */
    .quad 0                         /* +0x100: entry point */
    .long 0                         /* +0x108: core ID */
    .long 0                         /* +0x10C: AP running */

/* IDTR (offset 0x110, filled by BSP from SIDT) */
.org smp_trampoline_start + 0x110
    .space 16, 0                    /* 10 bytes IDTR + 6 padding */

smp_trampoline_end:
