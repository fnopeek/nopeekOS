//! SVM root-mode entry + VMRUN loops — 12.1.0b-svm through 12.1.1c-svm.
//!
//! Two consumer-facing entry points:
//!   - `enable_and_test()` — real-mode HLT/IOIO substrate test.
//!     Allocates a VMCB, host-save area, IOPM, MSRPM, NPT (identity)
//!     and a 5-byte stub `mov al,'O'; out 0x80,al; hlt`, enables
//!     EFER.SVME, VMRUNs, returns the resulting exit-code.
//!   - `run_linux()` — Linux Boot Protocol 32-bit entry. Allocates
//!     256 MB guest RAM, builds a non-identity NPT window, copies
//!     bzImage parts in via `microvm::linux::bzimage`, and dispatches
//!     a VMRUN/VMEXIT loop with handlers for HLT / IOIO / CPUID /
//!     MSR / INTR / SHUTDOWN / NPF.
//!
//! All allocations are *kept* (never freed) per call. EFER.SVME is
//! left set across calls (harmless — just enables SVM instructions).
//!
//! Reference: AMD64 APM Vol. 2 §15.4 (Enabling SVM), §15.5 (VMRUN
//! Instruction), §15.17 (Global Interrupt Flag), §15.10 (I/O
//! Intercepts), §15.11 (MSR Intercepts), §15.25 (Nested Paging).
//!
//! Compared to VMX 12.1.0b: SVM has no separate VMXON region — the
//! host-save area is conceptually similar, but selected by an MSR
//! rather than a region pointer. There's also no VMCLEAR/VMPTRLD
//! dance: VMRUN takes the VMCB physical address as an operand
//! (loaded into RAX), so multiple VMCBs can coexist trivially.

use super::{cpuid as host_cpuid, npt, rdmsr, vmcb, wrmsr};
use crate::microvm::linux::bzimage;
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

    // 5. IOPM bit for port 0x80 — substrate-test guest writes there.
    //    APM §15.10.1: bit `port % 8` of byte `port / 8`. Port 0x80
    //    → byte 16, bit 0 → IOPM[16] |= 0x01.
    // SAFETY: IOPM was just allocated + zeroed; exclusive ours.
    unsafe { (iopm_phys as *mut u8).add(0x10).write_volatile(0x01); }

    // 6. Guest stub page — 4 KB, write 32-bit prot-mode "OK" stub:
    //    B0 4F      mov al, 0x4F  ('O')
    //    E6 80      out 0x80, al
    //    F4         hlt
    //    Five bytes total. With IOIO_PROT + IOPM bit set, the OUT
    //    triggers VMEXIT_IOIO (0x7B). Without IOIO intercept (the
    //    earlier 12.1.0b path), it would fall through to HLT.
    let stub_phys = memory::allocate_frame()
        .ok_or("OOM allocating guest stub")?;
    // SAFETY: exclusive, identity-mapped.
    unsafe {
        core::ptr::write_bytes(stub_phys as *mut u8, 0, 4096);
        let p = stub_phys as *mut u8;
        p.add(0).write_volatile(0xB0); // mov al, imm8
        p.add(1).write_volatile(0x4F); // 'O'
        p.add(2).write_volatile(0xE6); // out imm8, al
        p.add(3).write_volatile(0x80); // port 0x80
        p.add(4).write_volatile(0xF4); // hlt
    }

    // 7. NPT — identity-map 256 MB of guest physical to host physical
    //    via 2 MB pages. NCR3 points at the PML4.
    let npt_root = npt::allocate_identity_npt()?;

    // 8. VMCB — 4 KB, allocated as Vmcb on the kernel heap. Since
    //    the kernel heap lies inside the identity-mapped region,
    //    the host-virtual address equals the host-physical address.
    let vmcb = alloc::boxed::Box::new(vmcb::Vmcb::zeroed());
    let vmcb_ptr = alloc::boxed::Box::leak(vmcb);
    let vmcb_phys = vmcb_ptr.phys_addr();

    setup_vmcb(vmcb_ptr, iopm_phys, msrpm_phys, stub_phys, npt_root);

    // 9. GuestRegs slot for the asm shim. The "OK" stub uses RAX
    //    only (CPU saves/loads via VMCB.SAVE.RAX); the other 14
    //    GPRs stay zero.
    let mut regs = vmcb::GuestRegs::default();

    // 10. CLGI + VMRUN + STGI. VMRUN takes the VMCB phys in RAX.
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
///   * 32-bit protected mode (CR0.PE=1, paging off — segmentation
///     only). CS.base = stub_phys, RIP = 0 → execution starts at
///     the 5-byte "OK" stub: mov al,0x4F; out 0x80,al; hlt.
///   * Intercept HLT + I/O (so VMEXIT fires on `out 0x80, al`).
///   * Intercept VMRUN (mandatory — guest can't run nested SVM
///     because we don't support nested-nested in 12.1).
///   * NPT on (12.1.1a-svm), guest paging off so guest physical =
///     guest linear and NPT translates straight through.
fn setup_vmcb(
    vmcb: &mut vmcb::Vmcb,
    iopm_phys: u64,
    msrpm_phys: u64,
    stub_phys: u64,
    npt_root: u64,
) {
    // ── Control area ──────────────────────────────────────────────
    vmcb.write_u32(
        vmcb::OFF_INTERCEPT_MISC1,
        vmcb::INTERCEPT_HLT | vmcb::INTERCEPT_IOIO_PROT,
    );
    vmcb.write_u32(vmcb::OFF_INTERCEPT_MISC2, vmcb::INTERCEPT_VMRUN);
    vmcb.write_u64(vmcb::OFF_IOPM_BASE_PA, iopm_phys);
    vmcb.write_u64(vmcb::OFF_MSRPM_BASE_PA, msrpm_phys);
    vmcb.write_u32(vmcb::OFF_ASID, 1);
    vmcb.write_u8(vmcb::OFF_TLB_CTL, 1); // flush this guest's TLB
    vmcb.write_u64(vmcb::OFF_NESTED_CTL, 1); // NP_ENABLE — NPT on
    vmcb.write_u64(vmcb::OFF_NCR3, npt_root);

    // ── State save area: 32-bit prot mode, CS at stub_phys ────────
    // CS.base = stub_phys; RIP = 0 → fetch starts at stub_phys + 0
    // = the `mov al, 0x4F` byte. CS limit=4 GB (G=1, limit=0xFFFFFFFF).
    vmcb.write_segment(
        vmcb::OFF_SAVE_CS,
        /* selector */ 0x08,
        /* attrib   */ vmcb::ATTR_CODE_PM32,
        /* limit    */ 0xFFFF_FFFF,
        /* base     */ stub_phys,
    );
    // SS/DS/ES: 32-bit data, base=0, limit=4 GB — flat data segments.
    for off in [
        vmcb::OFF_SAVE_SS,
        vmcb::OFF_SAVE_DS,
        vmcb::OFF_SAVE_ES,
    ] {
        vmcb.write_segment(off, 0x10, vmcb::ATTR_DATA_PM32, 0xFFFF_FFFF, 0);
    }

    // GDTR / IDTR: 32-bit prot mode normally consults GDTR for
    // segment descriptor reloads, but our segments don't reload —
    // we set everything in the VMCB once. Leave zero.
    vmcb.write_u32(vmcb::OFF_SAVE_GDTR + 4, 0);
    vmcb.write_u64(vmcb::OFF_SAVE_GDTR + 8, 0);
    vmcb.write_u32(vmcb::OFF_SAVE_IDTR + 4, 0);
    vmcb.write_u64(vmcb::OFF_SAVE_IDTR + 8, 0);

    // CR registers: 32-bit prot mode, paging off.
    vmcb.write_u64(vmcb::OFF_SAVE_CR0, 0x11); // PE=1, ET=1
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

// ── Linux launcher (12.1.1c-svm) ───────────────────────────────────

// AMD VMEXIT codes used by the Linux loop. APM Vol 2 Appendix C lists
// the full set; we match against the ones we expect Linux to trigger.
const EXIT_INTR: u64 = 0x060;
const EXIT_CPUID: u64 = 0x072;
const EXIT_HLT: u64 = 0x078;
const EXIT_IOIO: u64 = 0x07B;
const EXIT_MSR: u64 = 0x07C;
const EXIT_SHUTDOWN: u64 = 0x07F;
const EXIT_NPF: u64 = 0x400;
const EXIT_INVALID: u64 = 0xFFFF_FFFF_FFFF_FFFF;

/// Boot a Linux bzImage in our SVM substrate. Mirrors
/// `vmx::enable::run_linux` shape: alloc 256 MB guest RAM, build
/// non-identity NPT, copy bzImage via shared bzimage loader,
/// configure VMCB for 32-bit prot-mode entry at `code32_start` with
/// RSI = boot_params_phys, then run a serial-aware exit loop.
pub fn run_linux(
    bzimage_bytes: &[u8],
    cmdline: &[u8],
    initramfs: Option<&[u8]>,
    inject: &[u8],
) -> Result<vmcb::LaunchOutcome, &'static str> {
    enable_efer_svme()?;
    setup_host_save()?;

    let (host_base, npt_root) = alloc_guest_ram_and_npt()?;

    let load = bzimage::load_into_guest_ram(host_base, bzimage_bytes, cmdline, initramfs)?;

    // IOPM: 12 KB all-ones = trap every port. Linux touches dozens of
    // unique ports during boot (UART, PIC, PIT, RTC, …). Cheaper to
    // trap-all + handle the boring ones in the loop than to bitmap-
    // tune which ports matter.
    let iopm_phys = memory::allocate_contiguous(3)
        .ok_or("OOM allocating IOPM (12 KB)")?;
    // SAFETY: freshly allocated, identity-mapped, exclusive.
    unsafe { core::ptr::write_bytes(iopm_phys as *mut u8, 0xFF, 3 * 4096); }

    // MSRPM: 8 KB all-zero = pass-through every MSR. Architectural
    // CPU-state MSRs (EFER, FS_BASE, GS_BASE, KERNEL_GS_BASE, STAR/
    // LSTAR/CSTAR/SFMASK, …) are auto-saved/loaded by the CPU via
    // the VMCB.SAVE area on VMRUN/VMEXIT (APM §15.11.1) — pass-
    // through is the natural way to let Linux mutate them during
    // boot. Other MSR writes hit hardware; in our nested-SVM smoke
    // setup KVM virtualizes those, on real silicon we'll selectively
    // trap a few (PMU, perf-ctl, …) by setting MSRPM bits.
    let msrpm_phys = memory::allocate_contiguous(2)
        .ok_or("OOM allocating MSRPM (8 KB)")?;
    // SAFETY: as above.
    unsafe { core::ptr::write_bytes(msrpm_phys as *mut u8, 0, 2 * 4096); }

    let vmcb = alloc::boxed::Box::new(vmcb::Vmcb::zeroed());
    let vmcb_ptr = alloc::boxed::Box::leak(vmcb);
    let vmcb_phys = vmcb_ptr.phys_addr();

    setup_vmcb_linux(vmcb_ptr, iopm_phys, msrpm_phys, npt_root, load.entry_rip);

    // Initial GPRs: ESI = boot_params_phys per Linux 32-bit boot
    // protocol; the rest zero. (RAX/RSP go through VMCB SAVE area
    // and are already 0 from setup_vmcb_linux.)
    let mut regs = vmcb::GuestRegs::default();
    regs.rsi = load.boot_params_phys;

    // Memory fence — see lesson 2 in project_svm_bringup.md. The
    // VMCB writes above must be visible to the CPU's VMRUN
    // consistency-check path.
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);

    run_linux_loop(&mut regs, vmcb_ptr, vmcb_phys, host_base, inject)
}

/// Allocate 256 MB contiguous + slack for 2 MB alignment, install the
/// non-identity NPT window. Returns (host_base, NCR3).
fn alloc_guest_ram_and_npt() -> Result<(u64, u64), &'static str> {
    let raw_base = memory::allocate_contiguous(
        npt::GUEST_RAM_FRAMES + npt::GUEST_RAM_ALIGN_SLACK,
    )
    .ok_or("OOM allocating 256 MB guest RAM (+ slack)")?;
    let host_base = npt::round_up_to_2mb(raw_base);
    let npt_root = npt::allocate_window_npt(host_base)?;
    Ok((host_base, npt_root))
}

/// Configure a VMCB for Linux 32-bit boot protocol entry.
/// Differs from `setup_vmcb` (substrate test) in that:
///   * CS.base = 0 (Linux is at GPA `entry_rip`, not relative to CS).
///   * RIP = entry_rip (typically 0x100000 = code32_start).
///   * Wider intercept set: HLT + IOIO + MSR + CPUID + INTR + SHUTDOWN.
fn setup_vmcb_linux(
    vmcb: &mut vmcb::Vmcb,
    iopm_phys: u64,
    msrpm_phys: u64,
    npt_root: u64,
    entry_rip: u64,
) {
    // ── Control area ──────────────────────────────────────────────
    let misc1 = vmcb::INTERCEPT_INTR
        | vmcb::INTERCEPT_CPUID
        | vmcb::INTERCEPT_HLT
        | vmcb::INTERCEPT_IOIO_PROT
        | vmcb::INTERCEPT_MSR_PROT
        | vmcb::INTERCEPT_SHUTDOWN;
    vmcb.write_u32(vmcb::OFF_INTERCEPT_MISC1, misc1);
    vmcb.write_u32(vmcb::OFF_INTERCEPT_MISC2, vmcb::INTERCEPT_VMRUN);

    vmcb.write_u64(vmcb::OFF_IOPM_BASE_PA, iopm_phys);
    vmcb.write_u64(vmcb::OFF_MSRPM_BASE_PA, msrpm_phys);
    vmcb.write_u32(vmcb::OFF_ASID, 1);
    vmcb.write_u8(vmcb::OFF_TLB_CTL, 1); // flush this guest's TLB
    vmcb.write_u64(vmcb::OFF_NESTED_CTL, 1); // NP_ENABLE
    vmcb.write_u64(vmcb::OFF_NCR3, npt_root);

    // ── State save area: 32-bit prot mode flat segments ─────────────
    // CS.base = 0; RIP = entry_rip → fetch starts at GPA entry_rip,
    // which our NPT window maps to host_base + entry_rip = the
    // protected-mode kernel image copied by bzimage::load_into_guest_ram.
    vmcb.write_segment(
        vmcb::OFF_SAVE_CS,
        /* selector */ 0x08,
        /* attrib   */ vmcb::ATTR_CODE_PM32,
        /* limit    */ 0xFFFF_FFFF,
        /* base     */ 0,
    );
    for off in [vmcb::OFF_SAVE_SS, vmcb::OFF_SAVE_DS, vmcb::OFF_SAVE_ES] {
        vmcb.write_segment(off, 0x10, vmcb::ATTR_DATA_PM32, 0xFFFF_FFFF, 0);
    }
    // GDTR/IDTR limits — leave zero, Linux startup_32 reloads its own.
    vmcb.write_u32(vmcb::OFF_SAVE_GDTR + 4, 0);
    vmcb.write_u64(vmcb::OFF_SAVE_GDTR + 8, 0);
    vmcb.write_u32(vmcb::OFF_SAVE_IDTR + 4, 0);
    vmcb.write_u64(vmcb::OFF_SAVE_IDTR + 8, 0);

    // CR registers: 32-bit prot mode, paging off.
    vmcb.write_u64(vmcb::OFF_SAVE_CR0, 0x11); // PE=1, ET=1
    vmcb.write_u64(vmcb::OFF_SAVE_CR3, 0);
    vmcb.write_u64(vmcb::OFF_SAVE_CR4, 0);

    // EFER: APM §15.5.1 mandates SVME=1 in guest VMCB even when the
    // guest itself doesn't run SVM instructions.
    vmcb.write_u64(vmcb::OFF_SAVE_EFER, EFER_SVME);

    vmcb.write_u64(vmcb::OFF_SAVE_RFLAGS, 0x0000_0002); // bit 1 reserved-1
    vmcb.write_u64(vmcb::OFF_SAVE_RIP, entry_rip);
    vmcb.write_u64(vmcb::OFF_SAVE_RSP, 0);
    vmcb.write_u64(vmcb::OFF_SAVE_RAX, 0);
    vmcb.write_u8(vmcb::OFF_SAVE_CPL, 0);
    vmcb.write_u64(vmcb::OFF_SAVE_G_PAT, 0x0007_0406_0007_0406);
}

/// Per-guest serial UART state across exits. Mirrors `vmx::enable
/// ::SerialState` but lives in the SVM tree to keep the two backends
/// independently evolvable.
struct SerialState {
    dlab: bool,
    line: [u8; 256],
    line_n: usize,
    panic_observed: bool,
    panic_msg: [u8; 192],
    panic_msg_n: usize,
    /// Phase 12.1.4-svm — RX FIFO. Pre-injected by the host before
    /// VMRUN; drained when the guest reads RBR (0x3F8 IN). LSR.DR
    /// (bit 0) on 0x3FD IN reflects `rx_pos < rx_n`.
    rx: [u8; 128],
    rx_pos: usize,
    rx_n: usize,
}

const PANIC_PREFIX: &[u8] = b"Kernel panic - not syncing: ";

impl SerialState {
    fn new() -> Self {
        Self {
            dlab: false,
            line: [0; 256],
            line_n: 0,
            panic_observed: false,
            panic_msg: [0; 192],
            panic_msg_n: 0,
            rx: [0; 128],
            rx_pos: 0,
            rx_n: 0,
        }
    }

    fn inject(&mut self, bytes: &[u8]) {
        let n = bytes.len().min(self.rx.len());
        self.rx[..n].copy_from_slice(&bytes[..n]);
        self.rx_pos = 0;
        self.rx_n = n;
    }

    fn rx_has_data(&self) -> bool { self.rx_pos < self.rx_n }

    fn rx_take(&mut self) -> u8 {
        if self.rx_pos < self.rx_n {
            let b = self.rx[self.rx_pos];
            self.rx_pos += 1;
            b
        } else { 0 }
    }

    fn put_char(&mut self, byte: u8) {
        use crate::kprintln;
        if byte == b'\n' || self.line_n == self.line.len() {
            let n = self.line_n;
            self.scan_for_panic(n);
            let s = core::str::from_utf8(&self.line[..n]).unwrap_or("?");
            kprintln!("[guest] {}", s);
            self.line_n = 0;
            return;
        }
        if byte != b'\r' {
            self.line[self.line_n] = byte;
            self.line_n += 1;
        }
    }

    fn flush(&mut self) {
        use crate::kprintln;
        if self.line_n > 0 {
            let n = self.line_n;
            self.scan_for_panic(n);
            let s = core::str::from_utf8(&self.line[..n]).unwrap_or("?");
            kprintln!("[guest] {}", s);
            self.line_n = 0;
        }
    }

    fn scan_for_panic(&mut self, n: usize) {
        if self.panic_observed { return; }
        let line = &self.line[..n];
        let prefix = PANIC_PREFIX;
        if line.len() < prefix.len() { return; }
        for start in 0..=(line.len() - prefix.len()) {
            if &line[start..start + prefix.len()] == prefix {
                self.panic_observed = true;
                let body = &line[start + prefix.len()..];
                let copy_n = body.len().min(self.panic_msg.len());
                self.panic_msg[..copy_n].copy_from_slice(&body[..copy_n]);
                self.panic_msg_n = copy_n;
                return;
            }
        }
    }

    fn panic_msg_str(&self) -> &str {
        core::str::from_utf8(&self.panic_msg[..self.panic_msg_n]).unwrap_or("?")
    }
}

/// Linux VMRUN/VMEXIT loop. Caps at MAX_ITERATIONS to bound the
/// shell's response time when the guest never makes progress.
fn run_linux_loop(
    regs: &mut vmcb::GuestRegs,
    vmcb: &mut vmcb::Vmcb,
    vmcb_phys: u64,
    _host_base: u64,
    inject: &[u8],
) -> Result<vmcb::LaunchOutcome, &'static str> {
    use crate::kprintln;

    const MAX_ITERATIONS: u32 = 100_000;

    let mut serial = SerialState::new();
    if !inject.is_empty() {
        serial.inject(inject);
        kprintln!("[svm] pre-injected {} bytes into UART RX FIFO", inject.len());
    }

    let mut iter: u32 = 0;
    let mut last_outcome: Option<vmcb::LaunchOutcome> = None;
    let mut io_dropped: u32 = 0;
    let mut msr_log_count: u32 = 0;
    const MSR_LOG_CAP: u32 = 32;

    // Idle detection — once init enters its pause(2)/wait-loop, the
    // only exits are external interrupts (= host timer). After
    // IDLE_THRESHOLD consecutive INTRs declare guest idle and bail.
    // Threshold: Linux's TSC-calibration path can spin for thousands
    // of host timer ticks before giving up + moving on, so we keep
    // this generous. Phase 12.1.4-svm replaces this with a real
    // cancel signal.
    let mut consecutive_idle: u32 = 0;
    const IDLE_THRESHOLD: u32 = 5_000;

    while iter < MAX_ITERATIONS {
        iter += 1;
        let outcome = run_guest_once(regs, vmcb, vmcb_phys);
        let exit = outcome.exit_reason;

        if exit != EXIT_INTR { consecutive_idle = 0; }

        match exit {
            EXIT_INTR => {
                consecutive_idle = consecutive_idle.saturating_add(1);
                if consecutive_idle >= IDLE_THRESHOLD {
                    serial.flush();
                    kprintln!(
                        "[svm] guest idle in userspace ({} consecutive INTRs after {} iters) — exiting cleanly",
                        consecutive_idle, iter,
                    );
                    last_outcome = Some(outcome);
                    break;
                }
                last_outcome = Some(outcome);
            }
            EXIT_HLT => {
                serial.flush();
                kprintln!("[svm] guest HLT after {} VM-exits", iter);
                last_outcome = Some(outcome);
                break;
            }
            EXIT_CPUID => {
                let leaf = vmcb.read_u64(vmcb::OFF_SAVE_RAX) as u32;
                let subleaf = regs.rcx as u32;
                let (eax, ebx, mut ecx, mut edx);
                // Hide hypervisor presence + KVM paravirt leafs entirely.
                // Without this, Linux sees the L1 KVM signature through
                // pass-through CPUID, enables kvm-clock, then divides
                // by zero in pvclock_tsc_khz because the WRMSR to
                // KVM_SYSTEM_TIME_NEW is absorbed (never reaches L1 KVM)
                // so the pvclock_vcpu_time_info struct stays zeroed.
                if (0x4000_0000..=0x4000_FFFF).contains(&leaf) {
                    eax = 0; ebx = 0; ecx = 0; edx = 0;
                } else {
                    let (a, b, c, d) = host_cpuid(leaf, subleaf);
                    eax = a; ebx = b; ecx = c; edx = d;
                    if leaf == 1 {
                        // ECX bit 31: hypervisor present. Clearing it
                        // tells Linux we're "bare metal" — no probe of
                        // 0x40000000+ leafs, no kvm-clock activation.
                        ecx &= !(1u32 << 31);
                    }
                    if leaf == 7 && subleaf == 0 {
                        // Hide CET — host has CR4.CET=1 for IBT but
                        // Linux's hand-asm stubs lack ENDBR64, so once
                        // CET is on in the guest, indirect calls #CP
                        // and BUG().
                        ecx &= !(1u32 << 7);   // CET_SS
                        edx &= !(1u32 << 20);  // CET_IBT
                    }
                }
                vmcb.write_u64(vmcb::OFF_SAVE_RAX, eax as u64);
                regs.rbx = ebx as u64;
                regs.rcx = ecx as u64;
                regs.rdx = edx as u64;
                advance_rip(vmcb);
                last_outcome = Some(outcome);
            }
            EXIT_IOIO => {
                let info = outcome.exit_qualification;
                let port = ((info >> 16) & 0xFFFF) as u16;
                let dir_in = info & 1 != 0;
                let size: u8 =
                    if info & 0x10 != 0 { 1 }
                    else if info & 0x20 != 0 { 2 }
                    else if info & 0x40 != 0 { 4 }
                    else { 1 };
                handle_linux_io(vmcb, &mut serial, regs, port, dir_in, size, &mut io_dropped);
                advance_rip(vmcb);
                last_outcome = Some(outcome);
            }
            EXIT_MSR => {
                // EXITINFO1 bit 0: 0=RDMSR, 1=WRMSR
                let is_write = outcome.exit_qualification & 1 != 0;
                let msr = regs.rcx as u32;
                if is_write {
                    if msr_log_count < MSR_LOG_CAP {
                        let val = (regs.rdx << 32)
                            | (vmcb.read_u64(vmcb::OFF_SAVE_RAX) & 0xFFFF_FFFF);
                        kprintln!("[svm] WRMSR {:#010x} = {:#018x} (absorbed)", msr, val);
                        msr_log_count += 1;
                    }
                } else {
                    if !msr_is_known_noise(msr) && msr_log_count < MSR_LOG_CAP {
                        kprintln!("[svm] RDMSR {:#010x} → 0 (unhandled)", msr);
                        msr_log_count += 1;
                    }
                    regs.rdx = 0;
                    vmcb.write_u64(vmcb::OFF_SAVE_RAX, 0);
                }
                advance_rip(vmcb);
                last_outcome = Some(outcome);
            }
            EXIT_SHUTDOWN => {
                serial.flush();
                if serial.panic_observed {
                    kprintln!(
                        "[svm] linux kernel panicked (after {} iters): {}",
                        iter, serial.panic_msg_str(),
                    );
                    kprintln!("[svm] guest then triple-faulted via emergency_restart (= expected reboot path)");
                } else {
                    kprintln!(
                        "[svm] guest triple-faulted/shutdown after {} iters (no kernel-panic on console)",
                        iter,
                    );
                }
                last_outcome = Some(outcome);
                break;
            }
            EXIT_NPF => {
                serial.flush();
                let gpa = vmcb.read_u64(vmcb::OFF_EXIT_INFO_2);
                kprintln!(
                    "[svm] NPF: gpa={:#018x} info1={:#x} after {} iters",
                    gpa, outcome.exit_qualification, iter,
                );
                last_outcome = Some(outcome);
                break;
            }
            EXIT_INVALID => {
                serial.flush();
                kprintln!(
                    "[svm] VMEXIT_INVALID — VMCB consistency check failed (info1={:#x})",
                    outcome.exit_qualification,
                );
                last_outcome = Some(outcome);
                break;
            }
            _ => {
                serial.flush();
                kprintln!(
                    "[svm] unhandled exit {:#x} info1={:#x} after {} iters",
                    exit, outcome.exit_qualification, iter,
                );
                last_outcome = Some(outcome);
                break;
            }
        }
    }

    if iter >= MAX_ITERATIONS {
        serial.flush();
        kprintln!(
            "[svm] iteration cap ({}) reached — guest still running ({} I/O drops)",
            MAX_ITERATIONS, io_dropped,
        );
    }

    last_outcome.ok_or("SVM Linux guest exceeded max iterations without first VMEXIT")
}

/// Advance guest RIP across a non-fault exit. Requires NRIP_SAVE
/// (probed via CPUID 8000_000A EDX[3]) — every modern AMD CPU since
/// K10 has it. APM §15.7.1.
fn advance_rip(vmcb: &mut vmcb::Vmcb) {
    let nrip = vmcb.read_u64(vmcb::OFF_NRIP);
    vmcb.write_u64(vmcb::OFF_SAVE_RIP, nrip);
}

/// MSRs Linux probes via `safe_rdmsr` (catches #GP) — known noise on
/// nopeekOS, suppress the per-exit log line.
fn msr_is_known_noise(msr: u32) -> bool {
    matches!(msr,
        0xC001_1029 | 0xC001_0015 | 0xC001_001F | // AMD LS_CFG/HWCR/NB_CFG
        0x0000_001B | // IA32_APIC_BASE — Linux probes early
        0x0000_003A   // IA32_FEAT_CTL — VMX-only, absent on AMD
    )
}

/// Dispatch one I/O VMEXIT. UART COM1 (0x3F8-0x3FF) gets proper
/// synthetic responses; everything else absorbed (return 0 for IN,
/// no-op for OUT). Mirror of `vmx::handle_linux_io` shape, but
/// reads/writes RAX through VMCB instead of the GPR struct (SVM
/// auto-saves RAX in VMCB.SAVE.RAX on every VMEXIT).
fn handle_linux_io(
    vmcb: &mut vmcb::Vmcb,
    serial: &mut SerialState,
    _regs: &mut vmcb::GuestRegs,
    port: u16,
    dir_in: bool,
    size: u8,
    io_dropped: &mut u32,
) {
    let mask: u64 = match size { 1 => 0xFF, 2 => 0xFFFF, 4 => 0xFFFF_FFFF, _ => 0xFF };
    let rax = vmcb.read_u64(vmcb::OFF_SAVE_RAX);
    let val_out = (rax & mask) as u32;

    let in_value: Option<u64> = match (port, dir_in) {
        (0x3F8, false) => {
            if !serial.dlab { serial.put_char(val_out as u8); }
            None
        }
        (0x3F9, false) => None, // IER / DLM — ignore
        (0x3FB, false) => {
            serial.dlab = (val_out & 0x80) != 0;
            None
        }
        (0x3F8, true) => {
            // RBR (DLAB=0): pop one byte from injected RX FIFO.
            // DLL (DLAB=1): unmodelled, return 0.
            Some(if !serial.dlab { serial.rx_take() as u64 } else { 0 })
        }
        (0x3FA, true) => Some(0x01), // IIR: bit 0 = no IRQ pending
        (0x3FD, true) => {
            // LSR: THR-empty + TSR-empty always set, DR reflects FIFO.
            let dr = if serial.rx_has_data() { 0x01u64 } else { 0 };
            Some(0x60 | dr)
        }
        (0x3FE, true) => Some(0xB0), // MSR: CTS+DSR+DCD
        (0x3FA..=0x3FF, true) => Some(0),
        (_, true) => { *io_dropped += 1; Some(0) }
        (_, false) => { *io_dropped += 1; None }
    };

    if let Some(v) = in_value {
        let new_rax = (rax & !mask) | (v & mask);
        vmcb.write_u64(vmcb::OFF_SAVE_RAX, new_rax);
    }
}
