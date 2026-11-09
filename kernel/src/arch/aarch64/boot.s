/* nopeekOS: AArch64 entry.
 *
 * The loader (Pi firmware, QEMU -kernel) enters at the first byte of the
 * image with the MMU off, x0 = physical address of the device tree, at
 * EL2 or EL1 (Linux Documentation/arch/arm64/booting.rst).
 *
 *   1. arm64 Image header, so the loader knows the size and placement.
 *   2. At EL2: stay there with VHE (HCR_EL2.E2H|TGE) when the CPU has it,
 *      so the EL1 register names reach the EL2 regime and the hypervisor
 *      keeps EL2; otherwise drop to EL1.
 *   3. FP/SIMD on, interrupts masked, BSS zeroed, our own stack.
 *   4. Apply R_AARCH64_RELATIVE relocations against the load address.
 *   5. early_mmu(dtb) maps memory and turns MMU and caches on.
 *   6. kernel_main(dtb), which never returns.
 */

.section .text.head, "ax"
.global _start

_start:
    /* Image header: code0/code1, text_offset, image_size, flags, res2-4,
     * magic "ARM\x64", res5. flags: little endian, 4K pages, placed
     * anywhere 2 MiB aligned. */
    b       primary_entry
    .long   0
    .quad   0
    .quad   __image_size
    .quad   0xA
    .quad   0
    .quad   0
    .quad   0
    .ascii  "ARM\x64"
    .long   0

primary_entry:
    mov     x19, x0

    mrs     x1, CurrentEL
    lsr     x1, x1, #2
    cmp     x1, #2
    b.ne    in_el1

    /* EL2. VHE present: ID_AA64MMFR1_EL1.VH (bits 11:8) != 0. */
    mrs     x2, ID_AA64MMFR1_EL1
    ubfx    x2, x2, #8, #4
    cbz     x2, drop_to_el1
    /* HCR_EL2: E2H (34), RW (31), TGE (27). */
    mov     x3, #(1 << 31) | (1 << 27)
    orr     x3, x3, #(1 << 34)
    msr     HCR_EL2, x3
    isb
    b       in_el1

drop_to_el1:
    mov     x3, #(1 << 31)
    msr     HCR_EL2, x3
    /* EL1 may read the physical counter and timer; no virtual offset. */
    mov     x3, #3
    msr     CNTHCTL_EL2, x3
    msr     CNTVOFF_EL2, xzr
    /* SCTLR_EL1 with only its RES1 bits: MMU and caches off. */
    movz    x3, #0x0800
    movk    x3, #0x30d0, lsl #16
    msr     SCTLR_EL1, x3
    /* EL1h, DAIF masked. */
    mov     x3, #0x3c5
    msr     SPSR_EL2, x3
    adr     x3, in_el1
    msr     ELR_EL2, x3
    eret

in_el1:
    msr     DAIFSet, #0xf
    /* CPACR_EL1.FPEN = 0b11: FP/SIMD without traps. */
    mov     x3, #(3 << 20)
    msr     CPACR_EL1, x3
    isb

    /* Zero BSS (the stack lies inside it). */
    adrp    x4, __bss_start
    add     x4, x4, :lo12:__bss_start
    adrp    x5, __bss_end
    add     x5, x5, :lo12:__bss_end
1:  cmp     x4, x5
    b.hs    2f
    stp     xzr, xzr, [x4], #16
    b       1b
2:
    adrp    x4, __stack_top
    add     x4, x4, :lo12:__stack_top
    mov     sp, x4

    /* Relocate: linked at 0, so the delta is the load address. Each
     * Elf64_Rela is [r_offset, r_info, r_addend]; for type
     * R_AARCH64_RELATIVE (1027): *(delta + r_offset) = delta + r_addend. */
    adrp    x6, __image_base
    add     x6, x6, :lo12:__image_base
    adrp    x7, __rela_start
    add     x7, x7, :lo12:__rela_start
    adrp    x8, __rela_end
    add     x8, x8, :lo12:__rela_end
3:  cmp     x7, x8
    b.hs    5f
    ldp     x9, x10, [x7], #16
    ldr     x11, [x7], #8
    cmp     w10, #1027
    b.ne    3b
    add     x11, x11, x6
    str     x11, [x9, x6]
    b       3b
5:
    mov     x0, x19
    bl      early_mmu
    mov     x0, x19
    bl      kernel_main
6:  wfe
    b       6b

/* A secondary core, started by PSCI CPU_ON with x0 = its logical index.
 * The same EL setup as the boot core, then the boot core's translation
 * regime from secondary_boot (written and cleaned to memory before
 * CPU_ON, read here with the MMU and caches off), its own stack, and
 * secondary_main(index). */
.section .text
.global secondary_entry
secondary_entry:
    mov     x19, x0
    mrs     x1, CurrentEL
    lsr     x1, x1, #2
    cmp     x1, #2
    b.ne    s_in_el1
    mrs     x2, ID_AA64MMFR1_EL1
    ubfx    x2, x2, #8, #4
    cbz     x2, s_drop_to_el1
    mov     x3, #(1 << 31) | (1 << 27)
    orr     x3, x3, #(1 << 34)
    msr     HCR_EL2, x3
    isb
    b       s_in_el1
s_drop_to_el1:
    mov     x3, #(1 << 31)
    msr     HCR_EL2, x3
    mov     x3, #3
    msr     CNTHCTL_EL2, x3
    msr     CNTVOFF_EL2, xzr
    movz    x3, #0x0800
    movk    x3, #0x30d0, lsl #16
    msr     SCTLR_EL1, x3
    mov     x3, #0x3c5
    msr     SPSR_EL2, x3
    adr     x3, s_in_el1
    msr     ELR_EL2, x3
    eret
s_in_el1:
    msr     DAIFSet, #0xf
    mov     x3, #(3 << 20)
    msr     CPACR_EL1, x3
    isb
    adrp    x4, secondary_boot
    add     x4, x4, :lo12:secondary_boot
    ldr     x5, [x4, #0]
    mov     sp, x5
    ldr     x5, [x4, #8]
    msr     MAIR_EL1, x5
    ldr     x5, [x4, #16]
    msr     TCR_EL1, x5
    ldr     x5, [x4, #24]
    msr     TTBR0_EL1, x5
    isb
    mrs     x1, CurrentEL
    lsr     x1, x1, #2
    cmp     x1, #2
    b.ne    1f
    tlbi    alle2
    b       2f
1:  tlbi    vmalle1
2:  dsb     sy
    isb
    ic      iallu
    dsb     sy
    isb
    ldr     x5, [x4, #32]
    msr     SCTLR_EL1, x5
    isb
    msr     TPIDR_EL1, x19
    mov     x0, x19
    bl      secondary_main
3:  wfe
    b       3b
