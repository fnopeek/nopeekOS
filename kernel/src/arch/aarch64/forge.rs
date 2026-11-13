//! forge on AArch64: the module traps the processor raises and the trap a
//! host function raises.
//!
//! A data abort in compiled module code (the guard pages behind linear
//! memory) arrives as a synchronous exception at the current EL. The
//! vector checks the syndrome and whether ELR lies in forge's code range;
//! if so it points ELR at `forge_trap_oob` and returns into it. That
//! routine needs only the module's vmctx, which generated code keeps in
//! x28: it records the reason and unwinds to the entry, as the x86 #PF
//! stub does. x16/x17 are free in generated code at a fault (the run is
//! over), but not at a kernel fault, so the check keeps them in the EL0
//! thread registers, which the kernel does not use, until it knows.
//!
//! Division never faults on A64; generated code checks it explicitly.

use forge_core::vmctx;

/// ESR_ELx.EC of a data abort taken without a change in exception level.
const EC_DABT_CUR: u32 = 0x25;

core::arch::global_asm!(
    r#"
.pushsection .text, "ax"
.macro forge_check not_ours
    msr     TPIDR_EL0, x16
    msr     TPIDRRO_EL0, x17
    mrs     x16, ESR_EL1
    lsr     x16, x16, #26
    cmp     x16, #{ec}
    b.ne    1f
    mrs     x16, ELR_EL1
    ldr     x17, =({lo})
    cmp     x16, x17
    b.lo    1f
    adrp    x17, FORGE_NEXT_CODE
    ldr     x17, [x17, :lo12:FORGE_NEXT_CODE]
    cmp     x16, x17
    b.hs    1f
    adr     x16, forge_trap_oob
    msr     ELR_EL1, x16
    eret
1:  mrs     x16, TPIDR_EL0
    mrs     x17, TPIDRRO_EL0
    b       \not_ours
.endm

.section .text
.global forge_sync_sp0
forge_sync_sp0:
    forge_check sync_sp0_report
    .ltorg
.global forge_sync_spx
forge_sync_spx:
    forge_check sync_spx_report
    .ltorg

forge_trap_oob:
    mov     x16, #{oob}
    str     x16, [x28, #{tc}]
    ldr     x29, [x28, #{rbp}]
    ldr     x16, [x28, #{rsp}]
    mov     sp, x16
    ldr     x16, [x28, #{res}]
    br      x16

.global forge_host_trap
forge_host_trap:
    mov     x28, x0
    ldr     x26, [x0, #{fuel}]
    str     x1, [x0, #{tc}]
    ldr     x29, [x0, #{rbp}]
    ldr     x16, [x0, #{rsp}]
    mov     sp, x16
    ldr     x16, [x0, #{res}]
    br      x16
.popsection
"#,
    ec = const EC_DABT_CUR,
    lo = const crate::forge_rt::CODE_BASE,
    oob = const forge_core::trap::MEMORY_OUT_OF_BOUNDS,
    tc = const vmctx::TRAP_CODE,
    rbp = const vmctx::TRAP_RBP,
    rsp = const vmctx::TRAP_RSP,
    res = const vmctx::TRAP_RESUME,
    fuel = const vmctx::FUEL,
);

unsafe extern "C" {
    /// Leaves the running module with `code` as its trap reason; `vm` is its
    /// vmctx. Never returns.
    pub fn forge_host_trap(vm: *const u64, code: u64) -> !;
}

/// Make `len` bytes of freshly written code at `base` visible to
/// instruction fetch on every core (Arm ARM B2.4.4): clean the data cache
/// to the point of unification, invalidate the instruction cache, both
/// broadcast inner shareable.
pub fn sync_code(base: u64, len: usize) {
    let ctr: u64;
    // SAFETY: reads CTR_EL0, no other effect.
    unsafe { core::arch::asm!("mrs {}, CTR_EL0", out(reg) ctr, options(nomem, nostack, preserves_flags)) };
    let dline = 4u64 << ((ctr >> 16) & 0xf);
    let iline = 4u64 << (ctr & 0xf);
    let end = base + len as u64;
    let mut a = base & !(dline - 1);
    while a < end {
        // SAFETY: cleans a line of the mapped code, no memory change.
        unsafe { core::arch::asm!("dc cvau, {}", in(reg) a, options(nostack, preserves_flags)) };
        a += dline;
    }
    // SAFETY: barrier only.
    unsafe { core::arch::asm!("dsb ish", options(nostack, preserves_flags)) };
    let mut a = base & !(iline - 1);
    while a < end {
        // SAFETY: invalidates instruction cache lines of the code.
        unsafe { core::arch::asm!("ic ivau, {}", in(reg) a, options(nostack, preserves_flags)) };
        a += iline;
    }
    // SAFETY: barriers only.
    unsafe { core::arch::asm!("dsb ish", "isb", options(nostack, preserves_flags)) };
}

/// The stack pointer of the caller.
#[inline(always)]
pub fn stack_pointer() -> u64 {
    let sp: u64;
    // SAFETY: reads sp, nothing else.
    unsafe { core::arch::asm!("mov {}, sp", out(reg) sp, options(nomem, nostack, preserves_flags)) };
    sp
}
