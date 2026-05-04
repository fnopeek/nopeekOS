//! SVM root-mode entry + minimal VMRUN — 12.1.0b-svm.
//!
//! One consumer-facing entry point at this milestone:
//!   - `enable_and_test()` — real-mode HLT-loop substrate test.
//!     Allocates a fresh VMCB, host-save area, IOPM, MSRPM, and a
//!     1-byte guest stub page (single `hlt`), enables EFER.SVME,
//!     VMRUNs, and returns the resulting exit-code.
//!
//! All allocations are *kept* (never freed) per call. EFER.SVME is
//! left set across calls (harmless — just enables SVM instructions).
//!
//! Reference: AMD64 APM Vol. 2 §15.4 (Enabling SVM), §15.5 (VMRUN
//! Instruction), §15.17 (Global Interrupt Flag).
//!
//! Compared to VMX 12.1.0b: SVM has no separate VMXON region — the
//! host-save area is conceptually similar, but selected by an MSR
//! rather than a region pointer. There's also no VMCLEAR/VMPTRLD
//! dance: VMRUN takes the VMCB physical address as an operand
//! (loaded into RAX), so multiple VMCBs can coexist trivially.

use super::{rdmsr, vmcb, wrmsr};
use crate::mm::memory;

// ── MSRs (APM Vol 2 §15.4) ─────────────────────────────────────────

/// Extended Feature Enable Register. Bit 12 = SVME (SVM enable).
/// Architectural MSR since K8.
const IA32_EFER: u32 = 0xC000_0080;
const EFER_SVME: u64 = 1 << 12;

/// VM_HSAVE_PA — physical address of the host-save area. The CPU
/// writes the host's state here on VMRUN and reads it back on
/// VMEXIT. Must be 4 KB aligned, page-sized region.
const VM_HSAVE_PA: u32 = 0xC001_0117;

// ── Public entry point ─────────────────────────────────────────────

/// Run a trivial real-mode HLT-loop guest in a fresh SVM VM.
/// Returns `Ok(LaunchOutcome { exit_reason: 0x78, .. })` (= HLT
/// intercept) on success.
///
/// Allocates everything fresh on each call; nothing is freed. This
/// matches `vmx::enable::enable_and_test`, and at the rate
/// substrate-test runs (a couple of times per session) the leak is
/// negligible.
pub fn enable_and_test() -> Result<vmcb::LaunchOutcome, &'static str> {
    // 1. EFER.SVME on. APM §15.4: "VMRUN faults with #UD if EFER.SVME=0".
    enable_efer_svme()?;

    // 2. Host-save area — one 4 KB frame, zeroed, written to VM_HSAVE_PA.
    setup_host_save()?;

    // 3. IOPM — 3 contiguous frames (12 KB), zeroed = no I/O traps.
    let iopm_phys = memory::allocate_contiguous(3)
        .ok_or("OOM allocating IOPM (12 KB)")?;
    // SAFETY: freshly allocated, identity-mapped, exclusive.
    unsafe { core::ptr::write_bytes(iopm_phys as *mut u8, 0, 3 * 4096); }

    // 4. MSRPM — 2 contiguous frames (8 KB), zeroed = no MSR traps.
    let msrpm_phys = memory::allocate_contiguous(2)
        .ok_or("OOM allocating MSRPM (8 KB)")?;
    // SAFETY: as above.
    unsafe { core::ptr::write_bytes(msrpm_phys as *mut u8, 0, 2 * 4096); }

    // 5. Guest stub page — 4 KB, write `hlt` (0xF4) at offset 0.
    let stub_phys = memory::allocate_frame()
        .ok_or("OOM allocating guest stub")?;
    // SAFETY: exclusive, identity-mapped.
    unsafe {
        core::ptr::write_bytes(stub_phys as *mut u8, 0, 4096);
        (stub_phys as *mut u8).write_volatile(0xF4); // hlt
    }

    // 6. VMCB — 4 KB, allocated as Vmcb on the kernel heap. Since
    //    the kernel heap lies inside the identity-mapped region,
    //    the host-virtual address equals the host-physical address.
    let vmcb = alloc::boxed::Box::new(vmcb::Vmcb::zeroed());
    let vmcb_ptr = alloc::boxed::Box::leak(vmcb);
    let vmcb_phys = vmcb_ptr.phys_addr();

    setup_vmcb(vmcb_ptr, iopm_phys, msrpm_phys, stub_phys);

    // 7. CLGI + VMRUN + STGI. VMRUN takes the VMCB phys in RAX.
    //    APM §15.17: VMRUN requires GIF=0 (else #UD). The CPU sets
    //    GIF=1 inside the guest, then clears it again on VMEXIT,
    //    so we explicitly STGI on return.
    let outcome = run_guest_once(vmcb_ptr, vmcb_phys);

    Ok(outcome)
}

// ── Private plumbing ───────────────────────────────────────────────

/// Set EFER.SVME if not already set. Once on, stays on for the life
/// of this CPU — no harm in idempotent re-set.
fn enable_efer_svme() -> Result<(), &'static str> {
    // SAFETY: EFER is architectural since K8. SVME bit is the only
    // toggle; other bits (LME, LMA, NXE) we leave untouched.
    let efer = unsafe { rdmsr(IA32_EFER) };
    if efer & EFER_SVME == 0 {
        // SAFETY: setting SVME only enables SVM instructions; no
        // observable side-effect until the first VMRUN.
        unsafe { wrmsr(IA32_EFER, efer | EFER_SVME); }
    }
    Ok(())
}

/// Allocate the 4 KB host-save area, write its physical address to
/// VM_HSAVE_PA. Idempotent in the sense that calling twice leaks
/// the previous frame — fine for substrate tests.
fn setup_host_save() -> Result<(), &'static str> {
    let host_save_phys = memory::allocate_frame()
        .ok_or("OOM allocating SVM host-save area")?;
    // SAFETY: freshly allocated, identity-mapped, exclusive.
    unsafe { core::ptr::write_bytes(host_save_phys as *mut u8, 0, 4096); }
    // SAFETY: VM_HSAVE_PA is architectural; physical address is
    // page-aligned and within the host's identity-mapped region.
    unsafe { wrmsr(VM_HSAVE_PA, host_save_phys); }
    Ok(())
}

/// Initialize a VMCB for the substrate-test guest:
///   * Real-mode 16-bit, all segments based at the stub page,
///     CS:IP = 0:0 → executes the `hlt` byte at offset 0.
///   * Intercept HLT (so VMEXIT fires when the guest halts).
///   * Intercept VMRUN (mandatory — guest can't run nested SVM
///     because we don't support nested-nested in 12.1).
///   * NPT off, paging off, GDTR/IDTR null (real mode doesn't use
///     them).
fn setup_vmcb(
    vmcb: &mut vmcb::Vmcb,
    iopm_phys: u64,
    msrpm_phys: u64,
    stub_phys: u64,
) {
    // ── Control area ──────────────────────────────────────────────
    vmcb.write_u32(vmcb::OFF_INTERCEPT_MISC1, vmcb::INTERCEPT_HLT);
    vmcb.write_u32(vmcb::OFF_INTERCEPT_MISC2, vmcb::INTERCEPT_VMRUN);
    vmcb.write_u64(vmcb::OFF_IOPM_BASE_PA, iopm_phys);
    vmcb.write_u64(vmcb::OFF_MSRPM_BASE_PA, msrpm_phys);
    vmcb.write_u32(vmcb::OFF_ASID, 1);
    vmcb.write_u8(vmcb::OFF_TLB_CTL, 1); // flush this guest's TLB
    vmcb.write_u64(vmcb::OFF_NESTED_CTL, 0); // NPT off (12.1.1a-svm)
    vmcb.write_u64(vmcb::OFF_NCR3, 0);

    // ── State save area: real-mode, all segments at stub_phys ─────
    // CS.base = stub_phys; IP = 0 → fetch begins at stub_phys + 0,
    // which is our `hlt` instruction.
    vmcb.write_segment(
        vmcb::OFF_SAVE_CS,
        /* selector */ 0,
        /* attrib   */ vmcb::ATTR_CODE_RM,
        /* limit    */ 0xFFFF,
        /* base     */ stub_phys,
    );
    // SS/DS/ES/FS/GS: zero base, zero limit — guest doesn't touch them.
    for off in [
        vmcb::OFF_SAVE_SS,
        vmcb::OFF_SAVE_DS,
        vmcb::OFF_SAVE_ES,
    ] {
        vmcb.write_segment(off, 0, vmcb::ATTR_DATA_RM, 0xFFFF, 0);
    }

    // GDTR / IDTR: real mode doesn't consult them, but VMRUN's
    // consistency check still inspects limit/base. Leave zero.
    vmcb.write_u32(vmcb::OFF_SAVE_GDTR + 4, 0); // limit
    vmcb.write_u64(vmcb::OFF_SAVE_GDTR + 8, 0); // base
    vmcb.write_u32(vmcb::OFF_SAVE_IDTR + 4, 0);
    vmcb.write_u64(vmcb::OFF_SAVE_IDTR + 8, 0);

    // CR registers: real mode = paging off, protection off.
    vmcb.write_u64(vmcb::OFF_SAVE_CR0, 0x10); // ET=1 only
    vmcb.write_u64(vmcb::OFF_SAVE_CR3, 0);
    vmcb.write_u64(vmcb::OFF_SAVE_CR4, 0);

    // EFER: APM §15.5.1 requires guest EFER.SVME=1 in the VMCB,
    // even when the guest itself doesn't run any SVM instructions.
    // (Yes, the guest "owns" SVME from its CR-state perspective.)
    vmcb.write_u64(vmcb::OFF_SAVE_EFER, EFER_SVME);

    // RFLAGS: bit 1 must always be set (architecturally reserved).
    vmcb.write_u64(vmcb::OFF_SAVE_RFLAGS, 0x0000_0002);
    vmcb.write_u64(vmcb::OFF_SAVE_RIP, 0);
    vmcb.write_u64(vmcb::OFF_SAVE_RSP, 0);
    vmcb.write_u64(vmcb::OFF_SAVE_RAX, 0);
    vmcb.write_u8(vmcb::OFF_SAVE_CPL, 0);

    // G_PAT — guest Page Attribute Table. Standard reset value;
    // matters only when paging is on, but VMRUN consistency check
    // wants a sane value here.
    vmcb.write_u64(vmcb::OFF_SAVE_G_PAT, 0x0007_0406_0007_0406);
}

/// Execute one VMRUN against the supplied VMCB and return the
/// resulting exit-info. CLGI/STGI bracketing per APM §15.17.
///
/// For 12.1.0b-svm the guest is a 1-byte HLT stub — no GPR exchange
/// needed beyond what the CPU does automatically (RAX, RSP via
/// VMCB.SAVE). When 12.1.0c lands we'll add full GPR save/restore
/// here, mirroring `vmx::vmcs::run_guest_once`.
fn run_guest_once(
    vmcb: &mut vmcb::Vmcb,
    vmcb_phys: u64,
) -> vmcb::LaunchOutcome {
    // SAFETY: EFER.SVME is set (caller guarantee), VM_HSAVE_PA
    // points at a valid host-save frame, the VMCB has been
    // initialized to a state that passes APM §15.5.1 consistency
    // checks. CLGI/STGI bracket the call so no host interrupts
    // fire between save+VMRUN and post-VMEXIT state read.
    unsafe {
        core::arch::asm!(
            "clgi",
            "vmrun rax",
            "stgi",
            in("rax") vmcb_phys,
            // VMRUN clobbers nothing in the host (host state is
            // saved/restored via host-save area). But we mark all
            // GPRs as clobbered to be safe — the Rust ABI spills
            // them around the asm! block.
            clobber_abi("C"),
        );
    }

    let exit_code = vmcb.read_u64(vmcb::OFF_EXIT_CODE);
    let exit_info_1 = vmcb.read_u64(vmcb::OFF_EXIT_INFO_1);
    let guest_rax = vmcb.read_u64(vmcb::OFF_SAVE_RAX);

    vmcb::LaunchOutcome {
        exit_reason: exit_code,
        exit_qualification: exit_info_1,
        guest_rax,
    }
}
