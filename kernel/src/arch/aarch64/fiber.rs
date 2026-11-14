//! The fiber switch on AArch64: the callee-saved registers of AAPCS64
//! (x19-x28, x29, x30, d8-d15) and DAIF on the fiber's own stack, the stack
//! pointer in the context. DAIF travels with the context because the
//! interrupt mask is per-context, as RFLAGS.IF on x86.

// x0 = `from`, x1 = `to`. Frame of 176 bytes: x19..x30 at 0, d8..d15 at
// 96, DAIF at 160.
//   switch: save it below sp, sp into *from, sp from *to, restore, `ret`
//           to the restored x30.
//   trampoline: where a fresh fiber's first `ret` lands, with the entry
//           in x19 and its argument in x20.
core::arch::global_asm!(
    r#"
.pushsection .text, "ax"
.global fiber_context_switch
fiber_context_switch:
    sub     sp, sp, #176
    stp     x19, x20, [sp, #0]
    stp     x21, x22, [sp, #16]
    stp     x23, x24, [sp, #32]
    stp     x25, x26, [sp, #48]
    stp     x27, x28, [sp, #64]
    stp     x29, x30, [sp, #80]
    stp     d8, d9, [sp, #96]
    stp     d10, d11, [sp, #112]
    stp     d12, d13, [sp, #128]
    stp     d14, d15, [sp, #144]
    mrs     x9, DAIF
    str     x9, [sp, #160]
    mov     x9, sp
    str     x9, [x0]
    ldr     x9, [x1]
    mov     sp, x9
    ldp     x19, x20, [sp, #0]
    ldp     x21, x22, [sp, #16]
    ldp     x23, x24, [sp, #32]
    ldp     x25, x26, [sp, #48]
    ldp     x27, x28, [sp, #64]
    ldp     x29, x30, [sp, #80]
    ldp     d8, d9, [sp, #96]
    ldp     d10, d11, [sp, #112]
    ldp     d12, d13, [sp, #128]
    ldp     d14, d15, [sp, #144]
    ldr     x9, [sp, #160]
    add     sp, sp, #176
    msr     DAIF, x9
    ret

.global fiber_trampoline
fiber_trampoline:
    mov     x0, x20
    blr     x19
    bl      fiber_on_exit
1:  wfi
    b       1b
.popsection
"#
);

unsafe extern "C" {
    fn fiber_context_switch(from: *mut u64, to: *const u64);
    fn fiber_trampoline();
}

const FRAME: u64 = 176;
const DAIF_SLOT: u64 = 160;
/// DAIF with every exception masked.
const DAIF_MASKED: u64 = 0xf << 6;
/// PSTATE.I in DAIF.
const DAIF_I: u64 = 1 << 7;

/// Save the current stack pointer into `*from`, resume the one in `*to`.
///
/// # Safety
/// `*to` is a frame from `initial_frame` or a previous switch out, on a
/// stack that is still alive.
pub unsafe fn fiber_switch(from: *mut u64, to: *const u64) {
    // SAFETY: the caller's contract.
    unsafe { fiber_context_switch(from, to) }
}

/// Lay out a fresh fiber's first frame below `top` (16-aligned) so that
/// the first switch into it runs `entry(arg)`; returns its stack pointer.
///
/// # Safety
/// The 176 bytes below `top` belong to the new, unused stack.
pub unsafe fn initial_frame(top: u64, entry: extern "C" fn(u64), arg: u64) -> u64 {
    let sp0 = top - FRAME;
    // SAFETY: the caller's contract; the whole frame the switch reads.
    unsafe {
        core::ptr::write_bytes(sp0 as *mut u8, 0, FRAME as usize);
        let p = sp0 as *mut u64;
        *p.add(0) = entry as usize as u64; // x19 → blr target
        *p.add(1) = arg; // x20 → x0
        *p.add(11) = fiber_trampoline as *const () as u64; // x30 → ret
        *((sp0 + DAIF_SLOT) as *mut u64) = DAIF_MASKED;
    }
    sp0
}

/// A fiber that never ran starts as a task leaves `schedule_tail` in
/// Linux: in process context, interrupts enabled, the other DAIF bits as
/// the loop that first switches into it has them.
///
/// # Safety
/// `sp` is the stack pointer `initial_frame` returned, not yet switched to.
pub unsafe fn start_with_interrupts(sp: u64) {
    let daif: u64;
    // SAFETY: reading DAIF has no side effect; the slot is in the frame.
    unsafe {
        core::arch::asm!("mrs {}, DAIF", out(reg) daif, options(nomem, nostack, preserves_flags));
        *((sp + DAIF_SLOT) as *mut u64) = daif & !DAIF_I;
    }
}
