/* nopeekOS: AArch64 exception vectors (Arm ARM D1.10.2).
 *
 * Sixteen 128-byte entries: four sources (current EL with SP0, current
 * EL with SPx, lower EL AArch64, lower EL AArch32), each with four kinds
 * (sync, IRQ, FIQ, SError). With
 * VHE, VBAR_EL1 names VBAR_EL2 and the kernel runs at EL2.
 *
 * An IRQ taken at the current EL saves the interrupted context (general
 * registers, the full SIMD/FP file, ELR and SPSR), runs aarch64_irq and
 * returns to it. Every other entry reports its kind and the syndrome
 * registers through aarch64_exception, which never returns.
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
    .balign 0x80
    b       irq_entry
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

/* Frame: x0-x30 at 0, ELR and SPSR at 248, FPCR and FPSR at 264,
 * q0-q31 at 288; 800 bytes, a multiple of 16. */
irq_entry:
    sub     sp, sp, #800
    stp     x0, x1, [sp, #0]
    stp     x2, x3, [sp, #16]
    stp     x4, x5, [sp, #32]
    stp     x6, x7, [sp, #48]
    stp     x8, x9, [sp, #64]
    stp     x10, x11, [sp, #80]
    stp     x12, x13, [sp, #96]
    stp     x14, x15, [sp, #112]
    stp     x16, x17, [sp, #128]
    stp     x18, x19, [sp, #144]
    stp     x20, x21, [sp, #160]
    stp     x22, x23, [sp, #176]
    stp     x24, x25, [sp, #192]
    stp     x26, x27, [sp, #208]
    stp     x28, x29, [sp, #224]
    str     x30, [sp, #240]
    mrs     x0, ELR_EL1
    mrs     x1, SPSR_EL1
    stp     x0, x1, [sp, #248]
    mrs     x0, FPCR
    mrs     x1, FPSR
    stp     x0, x1, [sp, #264]
    add     x0, sp, #288
    stp     q0, q1, [x0, #0]
    stp     q2, q3, [x0, #32]
    stp     q4, q5, [x0, #64]
    stp     q6, q7, [x0, #96]
    stp     q8, q9, [x0, #128]
    stp     q10, q11, [x0, #160]
    stp     q12, q13, [x0, #192]
    stp     q14, q15, [x0, #224]
    stp     q16, q17, [x0, #256]
    stp     q18, q19, [x0, #288]
    stp     q20, q21, [x0, #320]
    stp     q22, q23, [x0, #352]
    stp     q24, q25, [x0, #384]
    stp     q26, q27, [x0, #416]
    stp     q28, q29, [x0, #448]
    stp     q30, q31, [x0, #480]

    mov     x0, sp
    bl      aarch64_irq

    add     x0, sp, #288
    ldp     q0, q1, [x0, #0]
    ldp     q2, q3, [x0, #32]
    ldp     q4, q5, [x0, #64]
    ldp     q6, q7, [x0, #96]
    ldp     q8, q9, [x0, #128]
    ldp     q10, q11, [x0, #160]
    ldp     q12, q13, [x0, #192]
    ldp     q14, q15, [x0, #224]
    ldp     q16, q17, [x0, #256]
    ldp     q18, q19, [x0, #288]
    ldp     q20, q21, [x0, #320]
    ldp     q22, q23, [x0, #352]
    ldp     q24, q25, [x0, #384]
    ldp     q26, q27, [x0, #416]
    ldp     q28, q29, [x0, #448]
    ldp     q30, q31, [x0, #480]
    ldp     x0, x1, [sp, #264]
    msr     FPCR, x0
    msr     FPSR, x1
    ldp     x0, x1, [sp, #248]
    msr     ELR_EL1, x0
    msr     SPSR_EL1, x1
    ldp     x2, x3, [sp, #16]
    ldp     x4, x5, [sp, #32]
    ldp     x6, x7, [sp, #48]
    ldp     x8, x9, [sp, #64]
    ldp     x10, x11, [sp, #80]
    ldp     x12, x13, [sp, #96]
    ldp     x14, x15, [sp, #112]
    ldp     x16, x17, [sp, #128]
    ldp     x18, x19, [sp, #144]
    ldp     x20, x21, [sp, #160]
    ldp     x22, x23, [sp, #176]
    ldp     x24, x25, [sp, #192]
    ldp     x26, x27, [sp, #208]
    ldp     x28, x29, [sp, #224]
    ldr     x30, [sp, #240]
    ldp     x0, x1, [sp, #0]
    add     sp, sp, #800
    eret
