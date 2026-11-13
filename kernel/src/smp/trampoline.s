/* nopeekOS SMP — AP Trampoline
 *
 * Copied to physical 0x8000 at runtime. APs begin here in 16-bit real mode
 * after INIT-SIPI-SIPI from BSP.
 *
 * Layout at BOOT (0x8000):
 *   0x000  Code: 16-bit → 32-bit → 64-bit
 *   0x0D0  GDT32 (null + code32 + data32)
 *   0x0E8  GDT32 pointer
 *   0x0F0  GDT64 pointer (filled by BSP)
 *   0x100  CR3 (u32, filled by BSP)
 *   0x108  Stack top (u64, filled by BSP)
 *   0x110  Rust entry (u64, filled by BSP)
 *   0x118  Core ID (u32, filled by BSP)
 *   0x11C  AP running flag (u32, set to 1 by AP)
 *   0x120  IDTR (10 bytes, filled by BSP)
 *
 * Code budget: 0x0D0 = 208 bytes (was 0x0C0 = 192). The +16-byte bump
 * landed in v0.85.5 to make room for the AVX bring-up sequence
 * (XSETBV with XCR0 = x87|SSE|AVX). All offsets shifted in lockstep
 * with kernel/src/smp/mod.rs OFF_* constants.
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
    lgdtl (BOOT + 0xE8)

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

    /* CR4: PAE | OSFXSR | OSXMMEXCPT | OSXSAVE
     * OSXSAVE (bit 18) is required for XSETBV / AVX state-save in
     * smp_lm64 below. */
    mov %cr4, %eax
    or $0x40620, %eax       /* PAE | OSFXSR | OSXMMEXCPT | OSXSAVE */
    mov %eax, %cr4

    /* Load CR3 from data area */
    mov (BOOT + 0x100), %eax
    mov %eax, %cr3

    /* Enable Long Mode (EFER.LME) + NX (EFER.NXE) */
    mov $0xC0000080, %ecx
    rdmsr
    or $0x900, %eax         /* bit 8 = LME, bit 11 = NXE */
    wrmsr

    /* CR0: clear EM, set MP|NE (FPU/SSE), then PG (paging). */
    mov %cr0, %eax
    and $~4, %eax           /* clear EM */
    or $0x80000022, %eax    /* PG | MP | NE */
    mov %eax, %cr0

    /* Load 64-bit GDT (written by BSP at BOOT+0xF0) */
    lgdt (BOOT + 0xF0)

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
    lidt 0x120(%rbx)

    /* Per-AP state */
    mov 0x108(%rbx), %rsp      /* stack top */

    /* Initialize FPU/SSE state (CR0/CR4 already set in 32-bit mode). */
    fninit
    pushq $0x1F80
    ldmxcsr (%rsp)
    add $8, %rsp

    /* XSETBV: enable x87|SSE|AVX in XCR0 (matches boot.s). */
    xor %ecx, %ecx
    xor %edx, %edx
    mov $7, %eax
    xsetbv

    mov 0x110(%rbx), %rax      /* Rust entry point */
    mov 0x118(%rbx), %edi      /* core ID (arg 1, System V ABI) */
    movl $1, 0x11C(%rbx)       /* signal BSP: AP is alive */

    /* Enter Rust via CALL (not JMP) so rsp ends up at the alignment
     * the System V ABI expects at function entry: rsp % 16 == 8
     * (i.e. an 8-byte return addr is pushed, the callee's first
     * push %rbp brings rsp back to 16-byte aligned).
     *
     * With SSE enabled the compiler may emit MOVAPS / MOVDQA on
     * stack-relative addresses; those #GP-fault if the frame is
     * misaligned. Before SSE this bug was latent.
     *
     * smp_ap_entry returns ! so the dead ret-addr never actually
     * returns. The hlt loop below catches it just in case. */
    call *%rax

    cli
1:  hlt
    jmp 1b

/* === GDT32: 3 entries (offset 0xD0) === */
.org smp_trampoline_start + 0xD0
    .quad 0                         /* Null */
    .quad 0x00CF9A000000FFFF        /* Code32: base=0, limit=4G, exec/read */
    .quad 0x00CF92000000FFFF        /* Data32: base=0, limit=4G, read/write */

/* GDT32 pointer (offset 0xE8) */
.org smp_trampoline_start + 0xE8
    .word 23                        /* limit: 3*8 - 1 */
    .long BOOT + 0xD0              /* base at runtime address */

/* GDT64 pointer (offset 0xF0, filled by BSP from SGDT) */
.org smp_trampoline_start + 0xF0
    .word 0
    .long 0
    .long 0                         /* 12 bytes reserved */

/* Data area (offset 0x100, filled by BSP) */
.org smp_trampoline_start + 0x100
    .long 0                         /* +0x100: CR3 */
    .long 0                         /* +0x104: reserved */
    .quad 0                         /* +0x108: stack top */
    .quad 0                         /* +0x110: entry point */
    .long 0                         /* +0x118: core ID */
    .long 0                         /* +0x11C: AP running */

/* IDTR (offset 0x120, filled by BSP from SIDT) */
.org smp_trampoline_start + 0x120
    .space 16, 0                    /* 10 bytes IDTR + 6 padding */

smp_trampoline_end:
