/* nopeekOS: AArch64 exception vectors (Arm ARM D1.10.2).
 *
 * Sixteen 128-byte entries: four sources (current EL with SP0, current
 * EL with SPx, lower EL AArch64, lower EL AArch32), each with four kinds
 * (sync, IRQ, FIQ, SError). With
 * VHE, VBAR_EL1 names VBAR_EL2 and the kernel runs at EL2.
 *
 * No exception is handled yet: every entry reports its kind and the
 * syndrome registers through aarch64_exception, which never returns.
 */

.macro entry kind
    .balign 0x80
    mov     x0, #\kind
    b       report
.endm

.section .text
.balign 0x800
.global exception_vectors
exception_vectors:
    entry 0
    entry 1
    entry 2
    entry 3
    entry 4
    entry 5
    entry 6
    entry 7
    entry 8
    entry 9
    entry 10
    entry 11
    entry 12
    entry 13
    entry 14
    entry 15

report:
    mrs     x1, ESR_EL1
    mrs     x2, ELR_EL1
    mrs     x3, FAR_EL1
    mrs     x4, SPSR_EL1
    mov     x5, sp
    mov     x6, x30
    bl      aarch64_exception
1:  wfe
    b       1b
