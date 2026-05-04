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

use super::{npt, rdmsr, vmcb, wrmsr};
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

    // 6. NPT — identity-map 64 GB of guest physical to host physical
    //    via 1 GB pages at the PDPT level. NCR3 points at the PML4.
    let npt_root = npt::allocate_identity_npt()?;

    // 7. VMCB — 4 KB, allocated as Vmcb on the kernel heap. Since
    //    the kernel heap lies inside the identity-mapped region,
    //    the host-virtual address equals the host-physical address.
    let vmcb = alloc::boxed::Box::new(vmcb::Vmcb::zeroed());
    let vmcb_ptr = alloc::boxed::Box::leak(vmcb);
    let vmcb_phys = vmcb_ptr.phys_addr();

    setup_vmcb(vmcb_ptr, iopm_phys, msrpm_phys, stub_phys, npt_root);

    // 8. GuestRegs slot for the asm shim. HLT stub doesn't touch
    //    any GPRs but the shim still spills/reloads through it.
    let mut regs = vmcb::GuestRegs::default();

    // 9. CLGI + VMRUN + STGI. VMRUN takes the VMCB phys in RAX.
    //    APM §15.17: VMRUN requires GIF=0 (else #UD). The CPU sets
    //    GIF=1 inside the guest, then clears it again on VMEXIT,
    //    so we explicitly STGI on return.
    // Memory fence: setup_vmcb wrote 200+ scattered bytes into VMCB;
    // ensure they're visible to the CPU's VMRUN consistency-check
    // path before we hand off. Empirically without this on KVM
    // nested SVM the VMCB read-side races into stale zeros.
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);

    let outcome = run_guest_once(&mut regs, vmcb_ptr, vmcb_phys);

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
    npt_root: u64,
) {
    // ── Control area ──────────────────────────────────────────────
    vmcb.write_u32(vmcb::OFF_INTERCEPT_MISC1, vmcb::INTERCEPT_HLT);
    vmcb.write_u32(vmcb::OFF_INTERCEPT_MISC2, vmcb::INTERCEPT_VMRUN);
    vmcb.write_u64(vmcb::OFF_IOPM_BASE_PA, iopm_phys);
    vmcb.write_u64(vmcb::OFF_MSRPM_BASE_PA, msrpm_phys);
    vmcb.write_u32(vmcb::OFF_ASID, 1);
    vmcb.write_u8(vmcb::OFF_TLB_CTL, 1); // flush this guest's TLB
    vmcb.write_u64(vmcb::OFF_NESTED_CTL, 1); // NP_ENABLE — NPT on
    vmcb.write_u64(vmcb::OFF_NCR3, npt_root);

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
/// Loads guest GPRs from `regs` before VMRUN, saves them back on
/// VMEXIT. RAX/RSP are auto-handled by the CPU via VMCB.SAVE.{RAX,
/// RSP} + the host-save area, so they're not in `regs`.
///
/// The struct pointer survives across VMRUN by being pushed onto
/// the stack: VMRUN restores host RSP from the host-save area on
/// VMEXIT, returning RSP to its post-push value. The pushed
/// struct pointer is then recovered after we spill all 14 guest
/// GPRs.
fn run_guest_once(
    regs: &mut vmcb::GuestRegs,
    vmcb: &mut vmcb::Vmcb,
    vmcb_phys: u64,
) -> vmcb::LaunchOutcome {
    let regs_ptr: *mut vmcb::GuestRegs = regs;

    // SAFETY: EFER.SVME is set (caller guarantee), VM_HSAVE_PA
    // points at a valid host-save frame, the VMCB has been
    // initialized to a state that passes APM §15.5.1 consistency
    // checks. CLGI/STGI bracket the call so no host interrupts
    // fire between save+VMRUN and post-VMEXIT state read.
    //
    // Register dance:
    //   in: rdi = struct ptr, rsi = vmcb_phys
    //   1. push rdi (struct ptr) — survives VMRUN via host-save RSP
    //   2. mov rax, rsi (vmcb_phys into VMRUN operand reg)
    //   3. load guest GPRs from struct, rdi LAST
    //   4. CLGI; vmrun rax; STGI
    //   5. spill all 14 guest GPRs to stack
    //   6. recover struct ptr from stack[14*8 = 112]
    //   7. pop guest GPRs (LIFO) into struct
    //   8. discard struct ptr
    unsafe {
        core::arch::asm!(
            // ── PROLOGUE: save host callee-saved (clobber_abi("C")
            //              expects them preserved across the asm).
            "push rbp",
            "push rbx",
            "push r12",
            "push r13",
            "push r14",
            "push r15",

            // Save struct ptr below callee-saved.
            "push rdi",
            "mov rax, rsi",                 // rax = vmcb_phys

            // ── ENTRY: load guest GPRs from struct ────────────────
            "mov rbx, [rdi +   0]",
            "mov rcx, [rdi +   8]",
            "mov rdx, [rdi +  16]",
            "mov rbp, [rdi +  40]",
            "mov r8,  [rdi +  48]",
            "mov r9,  [rdi +  56]",
            "mov r10, [rdi +  64]",
            "mov r11, [rdi +  72]",
            "mov r12, [rdi +  80]",
            "mov r13, [rdi +  88]",
            "mov r14, [rdi +  96]",
            "mov r15, [rdi + 104]",
            "mov rsi, [rdi +  24]",         // rsi (was vmcb_phys input)
            "mov rdi, [rdi +  32]",         // rdi LAST

            // ── VMRUN ─────────────────────────────────────────────
            "clgi",
            "vmrun rax",
            "stgi",
            // After VMEXIT: rsp restored by CPU, rax=vmcb_phys (host
            // value via host-save area), all other GPRs hold guest
            // clobbers.

            // ── EXIT: spill 14 guest GPRs to stack ────────────────
            "push rbx",
            "push rcx",
            "push rdx",
            "push rsi",
            "push rdi",
            "push rbp",
            "push r8",
            "push r9",
            "push r10",
            "push r11",
            "push r12",
            "push r13",
            "push r14",
            "push r15",
            // Stack now: r15, r14, ..., rbx, struct_ptr,
            //             host_callee_saved (6).
            // struct_ptr is at [rsp + 14*8 = 112].

            // Recover struct ptr (rax = vmcb_phys, free to clobber).
            "mov rax, [rsp + 112]",

            // Pop in reverse-push order, store at the right offset.
            "pop rcx", "mov [rax + 104], rcx",      // r15
            "pop rcx", "mov [rax +  96], rcx",      // r14
            "pop rcx", "mov [rax +  88], rcx",      // r13
            "pop rcx", "mov [rax +  80], rcx",      // r12
            "pop rcx", "mov [rax +  72], rcx",      // r11
            "pop rcx", "mov [rax +  64], rcx",      // r10
            "pop rcx", "mov [rax +  56], rcx",      // r9
            "pop rcx", "mov [rax +  48], rcx",      // r8
            "pop rcx", "mov [rax +  40], rcx",      // rbp
            "pop rcx", "mov [rax +  32], rcx",      // rdi (guest's)
            "pop rcx", "mov [rax +  24], rcx",      // rsi
            "pop rcx", "mov [rax +  16], rcx",      // rdx
            "pop rcx", "mov [rax +   8], rcx",      // rcx
            "pop rcx", "mov [rax +   0], rcx",      // rbx
            "add rsp, 8",                           // discard struct ptr

            // Restore host callee-saved.
            "pop r15",
            "pop r14",
            "pop r13",
            "pop r12",
            "pop rbx",
            "pop rbp",

            in("rdi") regs_ptr,
            in("rsi") vmcb_phys,
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
