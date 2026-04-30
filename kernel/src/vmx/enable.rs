//! VMX root-mode entry/exit — 12.1.0b round-trip validation.
//!
//! This module does the full bring-up dance:
//!   1. Allocate a 4-KB VMXON region, write the VMCS revision-id.
//!   2. Unlock IA32_FEATURE_CONTROL if firmware left it open.
//!   3. Apply IA32_VMX_CR0/CR4_FIXED0/1 constraints to CR0/CR4 and
//!      set CR4.VMXE.
//!   4. Execute VMXON; check VMfailInvalid via RFLAGS.CF.
//!   5. Execute VMXOFF.
//!
//! The VMXON region is allocated and *kept* (never freed) so 12.1.0c
//! can re-enter VMX root mode against the same region without
//! re-allocating. CR4.VMXE is left set after the round-trip — harmless
//! and saves a write next time.
//!
//! Reference: Intel SDM Vol. 3C §23.7 (Enabling and Entering VMX
//! Operation), §A.7-A.8 (VMX-Fixed Bits in CR0/CR4).

use super::{rdmsr, wrmsr};
use crate::mm::memory;

const IA32_FEATURE_CONTROL: u32 = 0x3A;
const IA32_VMX_BASIC: u32 = 0x480;
const IA32_VMX_CR0_FIXED0: u32 = 0x486;
const IA32_VMX_CR0_FIXED1: u32 = 0x487;
const IA32_VMX_CR4_FIXED0: u32 = 0x488;
const IA32_VMX_CR4_FIXED1: u32 = 0x489;

const FEAT_CTRL_LOCK: u64 = 1 << 0;
const FEAT_CTRL_VMX_OUTSIDE_SMX: u64 = 1 << 2;

const CR4_VMXE: u64 = 1 << 13;

const RFLAGS_CF: u64 = 1 << 0;
const RFLAGS_ZF: u64 = 1 << 6;

/// Round-trip the full VMX bring-up: enter VMX root mode, exit it.
/// On success, `Ok(())`. On failure, a static string naming the step
/// that failed.
pub fn enable_and_test() -> Result<(), &'static str> {
    // 1. Allocate a 4-KB frame for the VMXON region. Kernel memory
    //    is identity-mapped (virt == phys), so we can cast and write
    //    directly. The frame is leaked deliberately: 12.1.0c re-uses
    //    it.
    let region_phys = memory::allocate_frame().ok_or("OOM allocating VMXON region")?;

    // 2. Zero the page and write the VMCS revision-id at offset 0.
    //    SDM §A.1: bit 31 of the revision dword must be clear; the
    //    upper 1 bit signals "shadow VMCS" which only applies to
    //    VMCS pages, not VMXON. We're using the same constant for
    //    both per Intel guidance.
    let basic = unsafe { rdmsr(IA32_VMX_BASIC) };
    let revision_id = (basic & 0x7FFF_FFFF) as u32;

    // SAFETY: identity-mapped, freshly-allocated, exclusive ownership
    // — nothing else has a pointer to this frame.
    unsafe {
        let region = region_phys as *mut u32;
        core::ptr::write_bytes(region as *mut u8, 0, 4096);
        region.write_volatile(revision_id);
    }

    // 3. Unlock IA32_FEATURE_CONTROL if firmware left it unlocked,
    //    or verify the firmware-locked state is compatible. The
    //    probe() pre-check already rejected the locked-off case;
    //    this step covers the unlocked + locked-on cases.
    let feat = unsafe { rdmsr(IA32_FEATURE_CONTROL) };
    if feat & FEAT_CTRL_LOCK == 0 {
        // Unlocked — we set lock-bit + VMX-outside-SMX in one write.
        // Once the lock-bit is set, this MSR is read-only until next
        // boot (RESET clears it).
        let new = feat | FEAT_CTRL_LOCK | FEAT_CTRL_VMX_OUTSIDE_SMX;
        // SAFETY: writing lock + outside-SMX bits to architectural
        // MSR; value cannot fault (no reserved bits set).
        unsafe { wrmsr(IA32_FEATURE_CONTROL, new); }
    } else if feat & FEAT_CTRL_VMX_OUTSIDE_SMX == 0 {
        return Err("IA32_FEATURE_CONTROL locked with VMX disabled (BIOS lock)");
    }

    // 4. Apply VMX-fixed bits to CR0 and CR4, set CR4.VMXE.
    //    SDM §A.7: CR0_FIXED0 = "must be 1", CR0_FIXED1 = "may be 1"
    //    (so & FIXED1 clears must-be-0 bits). Same for CR4.
    let cr0_f0 = unsafe { rdmsr(IA32_VMX_CR0_FIXED0) };
    let cr0_f1 = unsafe { rdmsr(IA32_VMX_CR0_FIXED1) };
    let cr4_f0 = unsafe { rdmsr(IA32_VMX_CR4_FIXED0) };
    let cr4_f1 = unsafe { rdmsr(IA32_VMX_CR4_FIXED1) };

    let mut cr0: u64;
    let mut cr4: u64;
    // SAFETY: CR0/CR4 reads cannot fault.
    unsafe {
        core::arch::asm!("mov {}, cr0", out(reg) cr0, options(nostack, preserves_flags));
        core::arch::asm!("mov {}, cr4", out(reg) cr4, options(nostack, preserves_flags));
    }
    cr0 = (cr0 | cr0_f0) & cr0_f1;
    cr4 = ((cr4 | cr4_f0) & cr4_f1) | CR4_VMXE;
    // SAFETY: values satisfy the architectural fixed-bit constraints
    // by construction; VMXE is always allowed when VMX is supported.
    unsafe {
        core::arch::asm!("mov cr0, {}", in(reg) cr0, options(nostack, preserves_flags));
        core::arch::asm!("mov cr4, {}", in(reg) cr4, options(nostack, preserves_flags));
    }

    // 5. VMXON. Operand is a memory location holding the phys-addr
    //    of the VMXON region. We stash region_phys in a stack slot
    //    and pass its address.
    //    On success: RFLAGS.CF = 0 and RFLAGS.ZF = 0.
    //    VMfailInvalid (no current-VMCS): CF = 1.
    //    VMfailValid (current-VMCS error): ZF = 1; not applicable
    //    on first VMXON because no VMCS is loaded.
    let region_addr_slot: u64 = region_phys;
    let rflags: u64;
    // SAFETY: VMXON requires CR4.VMXE=1 (set above) and a valid
    // 4-KB-aligned region with revision-id (set above). pushfq/pop
    // touches the stack, hence no `nostack` option.
    unsafe {
        core::arch::asm!(
            "vmxon [{addr}]",
            "pushfq",
            "pop {flags}",
            addr = in(reg) &region_addr_slot,
            flags = lateout(reg) rflags,
        );
    }
    if rflags & RFLAGS_CF != 0 {
        return Err("VMXON returned VMfailInvalid (CF=1)");
    }
    if rflags & RFLAGS_ZF != 0 {
        return Err("VMXON returned VMfailValid (ZF=1) — unexpected on first call");
    }

    // 6. VMXOFF. Cleanly leave VMX root mode. Only legal after a
    //    successful VMXON, which we just verified.
    // SAFETY: in VMX root mode (verified above).
    unsafe {
        core::arch::asm!("vmxoff", options(nostack, preserves_flags));
    }

    Ok(())
}
