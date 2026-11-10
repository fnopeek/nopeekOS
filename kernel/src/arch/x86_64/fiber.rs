//! The fiber switch on x86_64: callee-saved registers (System V: rbx, rbp,
//! r12-r15) and RFLAGS on the fiber's own stack, the stack pointer in the
//! context. RFLAGS travels with the context because IF is per-context.

// AT&T syntax to match boot.s / trampoline.s. rdi = `from`, rsi = `to`.
//
//   switch: save callee-saved + RFLAGS + rsp into *from, load *to's rsp,
//           RFLAGS and regs, `ret` into to's resume point.
//   trampoline: where a fresh fiber's first `ret` lands. The initial frame
//           put the entry fn in r12 and its arg in r13 (just popped by
//           `switch`): move the arg into rdi and call the entry. If it
//           returns, fall into `fiber_on_exit`, which switches back to the
//           scheduler.
core::arch::global_asm!(
    r#"
.global fiber_context_switch
fiber_context_switch:
    pushq %rbp
    pushq %rbx
    pushq %r12
    pushq %r13
    pushq %r14
    pushq %r15
    pushfq
    movq %rsp, (%rdi)
    movq (%rsi), %rsp
    popfq
    popq %r15
    popq %r14
    popq %r13
    popq %r12
    popq %rbx
    popq %rbp
    ret

.global fiber_trampoline
fiber_trampoline:
    movq %r13, %rdi
    callq *%r12
    callq fiber_on_exit
.Lfiber_hang:
    hlt
    jmp .Lfiber_hang
"#,
    options(att_syntax)
);

unsafe extern "C" {
    fn fiber_context_switch(from: *mut u64, to: *const u64);
    fn fiber_trampoline();
}

/// RFLAGS bit 1 reads as one and must be written as one.
const RFLAGS_RESERVED: u64 = 1 << 1;

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
/// Eight words, as `switch` pops then `ret`s through:
/// rflags r15 r14 r13 r12 rbx rbp [return address].
///
/// # Safety
/// The 64 bytes below `top` belong to the new, unused stack.
pub unsafe fn initial_frame(top: u64, entry: extern "C" fn(u64), arg: u64) -> u64 {
    let sp0 = top - 64;
    // SAFETY: the caller's contract; exactly the words the switch reads.
    unsafe {
        let p = sp0 as *mut u64;
        *p.add(0) = RFLAGS_RESERVED; // IF=0 unless the scheduler says otherwise
        *p.add(1) = 0; // r15
        *p.add(2) = 0; // r14
        *p.add(3) = arg; // r13 → rdi in the trampoline
        *p.add(4) = entry as usize as u64; // r12 → call target
        *p.add(5) = 0; // rbx
        *p.add(6) = 0; // rbp
        *p.add(7) = fiber_trampoline as *const () as u64;
    }
    sp0
}

/// A fiber that never ran starts with the interrupt state of the loop
/// that first switches into it: write RFLAGS into its frame.
///
/// # Safety
/// `sp` is the stack pointer `initial_frame` returned, not yet switched to.
pub unsafe fn inherit_flags(sp: u64) {
    let flags: u64;
    // SAFETY: reading RFLAGS has no side effect; the slot is the frame's
    // lowest word.
    unsafe {
        core::arch::asm!("pushfq; pop {}", out(reg) flags, options(preserves_flags));
        *(sp as *mut u64) = flags;
    }
}
