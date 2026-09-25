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

use super::{lapic, npt, rdmsr, vmcb, wrmsr};
use super::lapic::LocalApic;
use crate::microvm::devices::guest_mem::GuestMem;
use crate::microvm::linux::bzimage;
use crate::mm::memory;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Big-VM lock (guest SMP). Held by a vCPU only around its post-VMRUN
/// device/MMIO/IO handling — NEVER across `vmrun` or a fiber yield — so
/// the BSP and an AP vCPU serialize all access to the shared `VmShared`
/// (device model + guest memory) while their VMRUNs run truly in
/// parallel on separate host cores. Uncontended (and never even taken)
/// until an AP is admitted (`AP_ACTIVE`).
static VM_BIG_LOCK: spin::Mutex<()> = spin::Mutex::new(());

/// True once a second vCPU (AP) shares this VM. While false the BSP runs
/// exactly as the single-vCPU path did — `VM_BIG_LOCK` is not taken, so
/// the hot loop is byte-identical. Set by the orchestration layer when it
/// spawns the AP fiber, cleared after the last vCPU exits.
pub static AP_ACTIVE: AtomicBool = AtomicBool::new(false);

#[inline]
fn ap_active() -> bool {
    AP_ACTIVE.load(Ordering::Acquire)
}


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
    //    The VM_HSAVE_PA host-save area is programmed per-core lazily on
    //    the first VMRUN (`ensure_core_host_state` in `run_guest_once`).
    enable_efer_svme()?;

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

    // FPU areas for the in-asm save/restore (the stub doesn't touch
    // FPU, but run_guest_once unconditionally brackets vmrun).
    let mut hf = crate::microvm::cpu::FpuArea::boxed();
    let mut gf = crate::microvm::cpu::FpuArea::boxed();
    let outcome = run_guest_once(
        &mut regs, vmcb_ptr, vmcb_phys, &mut *hf, &mut *gf, crate::microvm::cpu::guest_cpuid::host_xcr0());

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

/// Max host cores we track per-core SVM state for (matches the
/// `[_; 256]` arrays in `smp::per_core`).
const MAX_CORES: usize = 256;

/// Per-core SVM host state, indexed by core id. Both are
/// per-PHYSICAL-CORE resources, so a single global frame is a
/// correctness bug the moment two cores run VMRUN concurrently
/// (guest SMP / multiple microvms):
///   * `HOST_SAVE_FRAMES` — the VM_HSAVE_PA target. VM_HSAVE_PA is a
///     per-core MSR; the CPU writes host state there on VMRUN and
///     reads it on VMEXIT. Two cores pointing at one frame clobber
///     each other.
///   * `HOST_EXTRA_FRAMES` — the `vmsave`/`vmload` frame for host
///     FS/GS/KernelGS/TR/LDTR/SYSCALL MSRs (`vmrun` preserves none —
///     APM Vol 2 §15.5.2). Concurrent vmsave/vmload against one frame
///     corrupts FS/GS → process-agnostic guest corruption (the
///     historical "A2 regression", now cross-core).
/// Each slot is lazily allocated the first time its core runs a VMRUN
/// (`ensure_core_host_state`), so any core — including future AP vCPU
/// fibers — is set up automatically without a separate init path.
static HOST_SAVE_FRAMES: [AtomicU64; MAX_CORES] =
    { const Z: AtomicU64 = AtomicU64::new(0); [Z; MAX_CORES] };
static HOST_EXTRA_FRAMES: [AtomicU64; MAX_CORES] =
    { const Z: AtomicU64 = AtomicU64::new(0); [Z; MAX_CORES] };

/// Ensure THIS core has SVM enabled, its VM_HSAVE_PA programmed to a
/// private host-save frame, and a private host-extra-save frame
/// allocated; return the extra-save frame phys. Idempotent per core
/// (one atomic load + branch on the hot path). MUST run on the core
/// that will execute VMRUN — VM_HSAVE_PA is a per-core MSR. No
/// intra-core race (one core runs this serially); distinct cores
/// touch distinct slots.
fn ensure_core_host_state() -> u64 {
    let cid = crate::smp::per_core::current_core_id().min(MAX_CORES - 1);
    if HOST_SAVE_FRAMES[cid].load(Ordering::Acquire) == 0 {
        // EFER.SVME on this core (idempotent).
        let _ = enable_efer_svme();
        let p = memory::allocate_frame().expect("OOM allocating SVM host-save area");
        // SAFETY: freshly allocated, identity-mapped, exclusive.
        unsafe { core::ptr::write_bytes(p as *mut u8, 0, 4096); }
        // SAFETY: VM_HSAVE_PA is architectural + per-core; p is page-aligned
        // and identity-mapped. Programs THIS core's host-save area.
        unsafe { wrmsr(VM_HSAVE_PA, p); }
        HOST_SAVE_FRAMES[cid].store(p, Ordering::Release);
    }
    let extra = HOST_EXTRA_FRAMES[cid].load(Ordering::Acquire);
    if extra != 0 {
        return extra;
    }
    let p = memory::allocate_contiguous(1).expect("OOM host extra-save frame");
    // SAFETY: freshly allocated, identity-mapped, exclusive.
    unsafe { core::ptr::write_bytes(p as *mut u8, 0, 4096); }
    HOST_EXTRA_FRAMES[cid].store(p, Ordering::Release);
    p
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
    // First entry only: drop what a previous run left under ASID 1 on this
    // core. `run_slice` clears it after the first VMRUN (see TLB_CTL there).
    vmcb.write_u8(vmcb::OFF_TLB_CTL, vmcb::tlb_flush_guest());
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
    host_fpu: *mut crate::microvm::cpu::FpuArea,
    guest_fpu: *mut crate::microvm::cpu::FpuArea,
    guest_xcr0: u64,
) -> vmcb::LaunchOutcome {
    let regs_ptr: *mut vmcb::GuestRegs = regs;
    // [guest, host] XCR0, switched inside the asm next to the FPU swap
    // (KVM `kvm_load_guest_xsave_state`): the guest's XSETBV value must
    // never stay live on the host, and the host's must not leak in.
    let xcr: [u64; 2] = [guest_xcr0, crate::microvm::cpu::guest_cpuid::host_xcr0()];
    let xcr_ptr = xcr.as_ptr();
    // Per-core: VM_HSAVE_PA + host-extra-save frame for THIS core. Lazily
    // sets up any core that runs VMRUN (BSP today, AP vCPU fibers later).
    let host_extra = ensure_core_host_state();

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

            // Save struct ptr, vmcb_phys, host-extra-save phys below
            // the callee-saved (push order = struct, vmcb, host).
            "push rdi",                     // struct ptr
            "push rsi",                     // vmcb_phys
            "push rdx",                     // host_extra_save phys
            "push r8",                      // host_fpu  ptr
            "push r9",                      // guest_fpu ptr
            "push r10",                     // xcr pair ptr
            // Stack now: [rsp+0]=xcr [+8]=guest_fpu [+16]=host_fpu
            //   [+24]=host_extra [+32]=vmcb_phys [+40]=struct_ptr

            // ── FPU SAVE/RESTORE (KVM kvm_load_guest_fpu): vmrun
            // preserves no x87/SSE/AVX/AVX-512. Must be in-asm,
            // adjacent to vmrun — a +avx2-built kernel spills `ymm`
            // anywhere between a Rust helper and the asm, clobbering
            // the just-restored guest state. Mask = -1 (all
            // XCR0-enabled components; guest owns XCR0 via XSETBV).
            // Only GPR/vm* instructions sit between this xrstor and
            // vmrun, and between vmrun and the paired xsave below.
            "mov rcx, [rsp + 16]",          // host_fpu
            "mov eax, 0xffffffff",
            "mov edx, 0xffffffff",
            "xsave64 [rcx]",                // save host FPU (host XCR0)
            "mov r11, [rsp + 0]",           // xcr pair
            "mov rax, [r11]",               // guest XCR0
            "cmp rax, [r11 + 8]",
            "je 2f",
            "mov rdx, rax",
            "shr rdx, 32",
            "xor ecx, ecx",
            "xsetbv",                       // guest XCR0 on
            "2:",
            "mov rcx, [rsp + 8]",           // guest_fpu
            "mov eax, 0xffffffff",
            "mov edx, 0xffffffff",
            "xrstor64 [rcx]",               // restore guest FPU (guest XCR0)

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

            // ── HOST EXTRA-STATE SAVE + VMRUN + RESTORE ───────────
            // vmsave host FS/GS/KernelGS/TR/LDTR/SYSCALL MSRs (vmrun
            // does NOT preserve them); vmload them back on THIS core
            // right after #VMEXIT, before any GS-relative host access.
            "mov rax, [rsp + 24]",          // host_extra_save phys
            "vmsave rax",                   // save host FS/GS/TR/LDTR/MSRs
            "mov rax, [rsp + 32]",          // guest vmcb_phys
            "clgi",
            "vmload rax",                   // load guest FS/GS/KernelGS/STAR/LSTAR/SFMASK/SYSENTER
            "vmrun rax",
            "vmsave rax",                   // save guest's back into the guest VMCB
            "mov rax, [rsp + 24]",          // host_extra_save phys
            "vmload rax",                   // restore host FS/GS/... while GIF=0 (atomic vs IRQs)
            "stgi",                         // only NOW open the IRQ window — host state already correct
            // After VMEXIT: rsp restored by CPU, all GPRs hold guest
            // clobbers; host FS/GS restored by the vmload above.

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
            // Stack now: r15..rbx (14=112B), xcr[+112], guest_fpu[+120],
            //   host_fpu[+128], host_extra[+136], vmcb[+144],
            //   struct_ptr[+152], host_callee_saved(6).

            // ── FPU SAVE/RESTORE (paired exit half): no FP since
            // vmexit (only vm*/push above). Save guest FPU, restore
            // host's. Guest GPRs are on the stack now → rax/rcx/rdx
            // free to clobber.
            "mov rcx, [rsp + 120]",         // guest_fpu
            "mov eax, 0xffffffff",
            "mov edx, 0xffffffff",
            "xsave64 [rcx]",                // save guest FPU (guest XCR0)
            "mov r11, [rsp + 112]",         // xcr pair
            "mov rax, [r11 + 8]",           // host XCR0
            "cmp rax, [r11]",
            "je 3f",
            "mov rdx, rax",
            "shr rdx, 32",
            "xor ecx, ecx",
            "xsetbv",                       // host XCR0 back
            "3:",
            "mov rcx, [rsp + 128]",         // host_fpu
            "mov eax, 0xffffffff",
            "mov edx, 0xffffffff",
            "xrstor64 [rcx]",               // restore host FPU (host XCR0)

            // Recover struct ptr (rax free to clobber).
            "mov rax, [rsp + 152]",

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
            "add rsp, 48",                          // discard xcr,guest_fpu,host_fpu,host_extra,vmcb,struct

            // Restore host callee-saved.
            "pop r15",
            "pop r14",
            "pop r13",
            "pop r12",
            "pop rbx",
            "pop rbp",

            in("rdi") regs_ptr,
            in("rsi") vmcb_phys,
            in("rdx") host_extra,
            in("r8") host_fpu,
            in("r9") guest_fpu,
            in("r10") xcr_ptr,
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
const EXIT_NMI: u64 = 0x061;
const EXIT_VINTR: u64 = 0x064;
const EXIT_RDPMC: u64 = 0x06F;
const EXIT_INVD: u64 = 0x076;
const EXIT_INVLPGA: u64 = 0x07A;
const EXIT_VMRUN: u64 = 0x080;
const EXIT_VMMCALL: u64 = 0x081;
const EXIT_VMLOAD: u64 = 0x082;
const EXIT_VMSAVE: u64 = 0x083;
const EXIT_STGI: u64 = 0x084;
const EXIT_CLGI: u64 = 0x085;
const EXIT_SKINIT: u64 = 0x086;
const EXIT_WBINVD: u64 = 0x089;
const EXIT_MONITOR: u64 = 0x08A;
const EXIT_MWAIT: u64 = 0x08B;
const EXIT_MWAIT_COND: u64 = 0x08C;
const EXIT_XSETBV: u64 = 0x08D;
const EXIT_RDPRU: u64 = 0x08E;

const VEC_UD: u8 = 6;
const VEC_GP: u8 = 13;

/// Queue a hardware exception for the next VMRUN (EVENTINJ type 3). RIP is
/// NOT advanced: the faulting instruction is the one reported.
fn inject_exception(vmcb: &mut vmcb::Vmcb, vector: u8, error_code: Option<u32>) {
    let mut info = vector as u64 | (3u64 << 8) | (1u64 << 31);
    if let Some(e) = error_code {
        info |= (1u64 << 11) | ((e as u64) << 32);
    }
    vmcb.write_u64(vmcb::OFF_EVENT_INJ, info);
}

/// XSETBV rules (SDM/APM, KVM `__kvm_set_xcr`): x87 always on, AVX needs SSE,
/// nothing beyond what the host itself has enabled in XCR0.
fn xcr0_valid(v: u64) -> bool {
    let host = crate::microvm::cpu::guest_cpuid::host_xcr0();
    v & 1 != 0 && v & !host == 0 && (v & 0b100 == 0 || v & 0b010 != 0)
}
const EXIT_INVALID: u64 = 0xFFFF_FFFF_FFFF_FFFF;

// ── Re-entrant VM context (Phase 12.4 step 1c — SVM mirror of 1a) ──
//
// Same decomposition as vmx::enable: open() / run_slice(budget) /
// close() so the Core-0 event loop can interleave Shade rendering
// between bounded slices (docs/archive/PHASE12_DISPLAY_BRIDGE.md R1). SVM is
// simpler than VMX: no VMXON/VMCS bracket — EFER.SVME stays on for
// the life of the kernel (by design, see module header) and the VMCB
// is a leaked Box, so close() is a no-op. Step 1c is
// behaviour-preserving: run_linux calls run_slice(u32::MAX) once,
// identical to the old run_linux_loop.

/// Outcome of one bounded slice of guest execution (SVM).
pub enum SliceOutcome {
    /// Budget exhausted, guest still running — caller may re-enter
    /// immediately (busy guest).
    StillRunning,
    /// Guest is idle (halted / waiting on its timer) — caller should
    /// re-enter but may host-idle first so the dedicated core doesn't
    /// spin VMRUN while the guest has nothing to do.
    Idle,
    /// Guest exited (HLT / shutdown / NPF / idle / cap).
    Exited(vmcb::LaunchOutcome),
}

/// State shared by ALL vCPUs of one microvm: the single guest address
/// space (RAM + NPT), the device model, and the host-tick/display
/// bookkeeping. Today the one vCPU owns it directly inside `VmContext`;
/// Stage 0c moves it behind a big-VM lock + shared handle so AP vCPU
/// fibers attach to it (guest SMP). Splitting it out now (Stage 0b) is a
/// pure, behaviour-preserving decomposition.
/// Which target a nested page fault at `gpa` hits (`cores` NPF breakdown).
fn npf_kind(sh: &VmShared, gpa: u64) -> usize {
    use crate::microvm::cpu as c;
    if sh.pci.virtio_blk.bar0_in_range(gpa) { c::NPF_BLK }
    else if crate::microvm::devices::net_backend::bar0_in_range(gpa) { c::NPF_NET }
    else if crate::microvm::devices::gpu_backend::bar0_in_range(gpa) { c::NPF_GPU }
    else if sh.pci.virtio_input.bar0_in_range(gpa) { c::NPF_INPUT }
    else if sh.pci.virtio_9p.bar0_in_range(gpa) { c::NPF_P9 }
    else if sh.pci.virtio_blk_sqfs.bar0_in_range(gpa) { c::NPF_SQFS }
    else if sh.pci.virtio_snd.bar0_in_range(gpa) { c::NPF_SND }
    else if (lapic::LAPIC_BASE..lapic::LAPIC_BASE + lapic::LAPIC_SIZE).contains(&gpa) { c::NPF_LAPIC }
    else if (crate::microvm::devices::ioapic::IOAPIC_BASE
        ..crate::microvm::devices::ioapic::IOAPIC_BASE + crate::microvm::devices::ioapic::IOAPIC_SIZE)
        .contains(&gpa) { c::NPF_IOAPIC }
    else if gpa < sh.guest_mem.len() { c::NPF_RAM }
    else { c::NPF_OTHER }
}

pub struct VmShared {
    /// Shared handle to the active guest memory (owned by `guest_mem`'s
    /// `ACTIVE_GM`, freed at close). A reference — NOT the owned `GuestMem` —
    /// so the off-vCPU net backend can hold the same `&'static GuestMem` without
    /// aliasing this `&mut VmShared` (the borrow governs the pointer, not the
    /// `Sync` pointee).
    guest_mem: &'static GuestMem,
    /// NPT PML4 (= NCR3) phys — `close()` passes it to `npt::release`
    /// to free demand-faulted frames + demand PTs + NPT tables.
    npt_pml4: u64,
    /// Base of the **contiguous boot-window** allocation (pre-2 MB-
    /// align), + the IOPM/MSRPM frames — all freed in close(). The
    /// demand region is freed via `npt::release`.
    guest_raw_base: u64,
    iopm_phys: u64,
    msrpm_phys: u64,
    serial: SerialState,
    pci: crate::microvm::devices::PciBus,
    pic: crate::microvm::devices::pic8259::Pic8259,
    /// i8254 channel 0 — its output is IRQ0 on the PIC.
    pit: crate::microvm::devices::pit8253::Pit,
    /// `ticks()` of the last virtio-gpu display config-change IRQ — debounces
    /// the resize round-trip against a drag storm (UI timing, 250 ms).
    last_cfg_tick: u64,
    /// `ticks()` of the last NAT mapping reap.
    last_reap_tick: u64,
}




/// Bounded adaptive halt-polling (KVM `halt_poll_ns` model) — see the VMX twin.
/// Idle vCPU polls its window for a wake before parking; grows on a caught wake,
/// shrinks on expiry. Replaces the old global `recently_active` spin. µs units.
const HALT_POLL_MIN_US: u64 = 5;
const HALT_POLL_MAX_US: u64 = 200;

/// Per-vCPU state: its own VMCB + register file + FPU areas, plus the
/// per-vCPU exit-handling bookkeeping. Each vCPU fiber owns one; guest
/// SMP = N of these against one shared `VmShared`.
pub struct Vcpu {
    /// This vCPU's physical (x)APIC ID — BSP = 0, APs = 1.. It must be
    /// reported identically by CPUID (leaf 1 EBX[31:24], leaf 0xB/0x1F
    /// EDX), the emulated LAPIC ID register, the IA32_APICBASE BSP bit
    /// (set only when apic_id == 0), and the MP-table entry, or Linux's
    /// topology code rejects the CPU. We virtualize it instead of passing
    /// the host core's APIC ID through (which would differ per worker core).
    apic_id: u8,
    /// Owned (not leaked) so it's reclaimed on drop — relaunch fix.
    vmcb: alloc::boxed::Box<vmcb::Vmcb>,
    vmcb_phys: u64,
    regs: vmcb::GuestRegs,
    /// Host/guest FPU (XSAVE) save areas — `vmrun` preserves neither.
    /// See `cpu::FpuArea`. guest_fpu starts zeroed = FPU init state.
    host_fpu: alloc::boxed::Box<crate::microvm::cpu::FpuArea>,
    guest_fpu: alloc::boxed::Box<crate::microvm::cpu::FpuArea>,
    /// Event that was mid-delivery when the last #VMEXIT hit (raw
    /// VMCB.EXITINTINFO, identical encoding to EVENTINJ). Must be
    /// re-injected on the next VMRUN or the guest loses an interrupt
    /// mid-vectoring → corrupt guest state (AMD APM §15.20; KVM
    /// `svm_complete_interrupts`). 0 = nothing pending.
    reinject: u64,
    iter: u32,
    io_dropped: u32,
    /// Per-vCPU emulated local APIC (xAPIC MMIO @ 0xFEE00000). Inert
    /// while the guest boots `nolapic`; drives the timer + (later) IPIs
    /// once Linux enables it. See `svm::lapic`.
    lapic: LocalApic,
    /// Adaptive halt-poll window (µs), see `HALT_POLL_MIN_US`.
    halt_poll_us: u64,
    /// Consecutive HLT exits taken with guest RFLAGS.IF=0. The idle path
    /// is always `sti; hlt` (IF=1); a sustained `cli; hlt` loop is Linux's
    /// no-ACPI poweroff / panic path (we boot `acpi=off`, so there is no
    /// ACPI shutdown signal). Crossing the threshold = the guest powered
    /// off → exit the VM (tears the window down on browser close instead
    /// of leaving a black idle window). Reset on any IF=1 HLT.
    if_off_halts: u32,
    /// Emulated MSR state (MTRR, SPEC_CTRL, HWCR …) — see `svm::msr`.
    msrs: super::msr::GuestMsrs,
    /// Guest XCR0 (reset value 1 = x87). Swapped around VMRUN in
    /// `run_guest_once`; the host's XCR0 is never left in the guest's hands.
    xcr0: u64,
}

/// Handle to a microvm's `VmShared`. The BSP vCPU **owns** it, heap-boxed
/// so its address is stable while the BSP's `VmContext` moves on the fiber
/// stack; an AP vCPU (guest SMP, Stage 3b) holds `Borrowed` — a raw pointer
/// to the same box. `Deref`/`DerefMut` make every `self.shared.X` access
/// work unchanged for both. Concurrent access between vCPUs is serialized
/// by `VM_BIG_LOCK` (taken around post-VMRUN device handling), NOT by this
/// handle — it only resolves *which* `VmShared` a vCPU's exit-handler sees.
pub enum SharedRef {
    Owned(alloc::boxed::Box<VmShared>),
    /// AP vCPU: aliases the BSP's box. Valid for the VM's lifetime — the
    /// BSP frees the box only after the last vCPU has exited (last-one-out
    /// refcount), so the pointer never dangles while an AP runs.
    /// Constructed in Stage 3b-2 (AP spawn).
    Borrowed(*mut VmShared),
}

// SAFETY: SharedRef::Borrowed is a raw pointer; the type is only moved
// into AP fiber tasks, and all access to the pointee is serialized by
// VM_BIG_LOCK. The Send marker lets it cross into the spawned fiber.
unsafe impl Send for SharedRef {}

impl SharedRef {
    fn owned(s: VmShared) -> Self {
        SharedRef::Owned(alloc::boxed::Box::new(s))
    }
}

impl core::ops::Deref for SharedRef {
    type Target = VmShared;
    fn deref(&self) -> &VmShared {
        match self {
            SharedRef::Owned(b) => b,
            // SAFETY: see the type-level comment — valid for the VM lifetime.
            SharedRef::Borrowed(p) => unsafe { &**p },
        }
    }
}

impl core::ops::DerefMut for SharedRef {
    fn deref_mut(&mut self) -> &mut VmShared {
        match self {
            SharedRef::Owned(b) => b,
            // SAFETY: as above; access is VM_BIG_LOCK-serialized.
            SharedRef::Borrowed(p) => unsafe { &mut **p },
        }
    }
}

/// Persistent state of one Linux microvm across cooperative slices
/// (SVM backend). Core-agnostic (forward-compat contract #1). Composed
/// of `VmShared` (all-vCPU, behind `SharedRef`) + one `Vcpu`.
pub struct VmContext {
    shared: SharedRef,
    vcpu: Vcpu,
}

impl VmContext {
    /// Enable SVM, set up the host-save area, build NPT, place the
    /// guest image, configure the VMCB, pre-inject the UART RX FIFO.
    /// Mirrors the old `run_linux` setup. No teardown-on-error needed:
    /// SVM has no VMXOFF analogue; EFER.SVME staying on is intended.
    pub fn open(
        bzimage_bytes: &[u8],
        cmdline: &[u8],
        initramfs: Option<&[u8]>,
        inject: &[u8],
    ) -> Result<VmContext, &'static str> {
        use crate::kprintln;

        // EFER.SVME here; VM_HSAVE_PA + host-extra-save are programmed
        // per-core on the first VMRUN (`ensure_core_host_state`).
        enable_efer_svme()?;

        let guest_bytes = crate::microvm::cpu::choose_guest_ram_bytes();
        let (boot_base, npt_root, guest_raw_base) = alloc_guest_ram_and_npt(guest_bytes)?;
        let gm = GuestMem::new(
            boot_base,
            npt::boot_window_bytes(guest_bytes),
            guest_bytes,
            npt_root,
            crate::microvm::devices::guest_mem::SecondLevel::Npt,
        );
        let load = bzimage::load_into_guest_ram(&gm, bzimage_bytes, cmdline, initramfs)?;
        // Install as the active guest memory (out of VmShared); `gm` is now the
        // shared `&'static` handle, also reachable by the off-vCPU net backend.
        let gm = crate::microvm::devices::guest_mem::set_active(gm);

        // IOPM: 12 KB all-ones = trap every port. Linux touches dozens
        // of unique ports during boot (UART, PIC, PIT, RTC, …).
        // Cheaper to trap-all + handle the boring ones in the loop.
        let iopm_phys = memory::allocate_contiguous(3)
            .ok_or("OOM allocating IOPM (12 KB)")?;
        // SAFETY: freshly allocated, identity-mapped, exclusive.
        unsafe { core::ptr::write_bytes(iopm_phys as *mut u8, 0xFF, 3 * 4096); }

        // MSRPM: intercept everything except the VMLOAD/VMSAVE-switched set
        // (KVM `svm_recalc_msr_intercepts`). An all-zero map would hand the
        // guest the host's MTRRs, TSC, SYSCFG and microcode loader.
        let msrpm_phys = memory::allocate_contiguous(2)
            .ok_or("OOM allocating MSRPM (8 KB)")?;
        // SAFETY: freshly allocated, identity-mapped, exclusive 8 KB.
        unsafe { super::msr::init_msrpm(msrpm_phys); }

        let mut vmcb = alloc::boxed::Box::new(vmcb::Vmcb::zeroed());
        let vmcb_phys = vmcb.phys_addr();

        setup_vmcb_linux(&mut vmcb, iopm_phys, msrpm_phys, npt_root, load.entry_rip);

        // Initial GPRs: ESI = boot_params_phys per Linux 32-bit boot
        // protocol; the rest zero.
        let mut regs = vmcb::GuestRegs::default();
        regs.rsi = load.boot_params_phys;

        let mut serial = SerialState::new();
        if !inject.is_empty() {
            serial.inject(inject);
            kprintln!("[svm] pre-injected {} bytes into UART RX FIFO", inject.len());
        }

        // Memory fence — see lesson 2 in project_svm_bringup.md. The
        // VMCB writes above must be visible to the CPU's VMRUN
        // consistency-check path.
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);

        // Fresh IPI state for this VM (IPI_PENDING is a static now, not a
        // VmShared field, so it isn't auto-reset when a new VmShared is built).
        lapic::reset_posted();
        // virtio-net lives in net_backend (a static, out of VmShared) so the
        // off-vCPU backend can own it; re-arm it to power-on state per VM.
        crate::microvm::devices::net_backend::reset();
        crate::microvm::devices::gpu_backend::reset();

        Ok(VmContext {
            shared: SharedRef::owned(VmShared {
                guest_mem: gm,
                npt_pml4: npt_root,
                guest_raw_base,
                iopm_phys,
                msrpm_phys,
                serial,
                pci: crate::microvm::devices::PciBus::new(),
                pic: {
                    // The I/O APIC's ID follows the vCPUs' in the MP table.
                    let mut pic = crate::microvm::devices::pic8259::Pic8259::new();
                    pic.ioapic.set_id(crate::microvm::cpu::guest_vcpus());
                    pic
                },
                pit: crate::microvm::devices::pit8253::Pit::new(),
                last_cfg_tick: 0,
                last_reap_tick: 0,
            }),
            vcpu: Vcpu {
                apic_id: 0, // BSP
                vmcb,
                vmcb_phys,
                regs,
                host_fpu: crate::microvm::cpu::FpuArea::boxed(),
                guest_fpu: crate::microvm::cpu::FpuArea::boxed(),
                reinject: 0,
                iter: 0,
                io_dropped: 0,
                lapic: LocalApic::new(0),
                halt_poll_us: HALT_POLL_MIN_US,
                if_off_halts: 0,
                msrs: super::msr::GuestMsrs::new(),
                xcr0: 1,
            },
        })
    }

    /// Free the per-VM frame allocations so the VM can be relaunched
    /// in the same boot. The VMCB is an owned `Box` and is reclaimed
    /// when the VmContext drops (no explicit free here). EFER.SVME
    /// stays on for the life of the kernel by design (no analogue to
    /// VMXOFF).
    ///
    /// B3: `npt::release` now walks + frees the demand frames, demand
    /// PTs, and NPT tables (no longer leaked).
    pub fn close(&mut self) {
        // Stop the off-vCPU net backend FIRST: it holds &'static GuestMem via
        // guest_mem::active(); clear_active() below frees it, so the worker must
        // have stopped touching it. Bounded-waits for the fiber to exit;
        // idempotent (the teardown path calls it again).
        crate::microvm::devices::net_dataplane::stop_worker();
        // Persist the home image to npkFS BEFORE freeing. close() is
        // reached on EVERY teardown — crucially the Mod+Q window-close
        // path (VM_CLOSE_REQUESTED → break → close), where run_slice's
        // own loop-end save() never runs (it returned StillRunning).
        // Without this the browser profile is lost on every Mod+Q.
        self.shared.pci.virtio_blk.save();
        // Demand-faulted frames + demand PTs + NPT tables.
        npt::release(self.shared.npt_pml4, self.shared.guest_mem.len());
        // Contiguous boot window.
        memory::deallocate_contiguous(
            self.shared.guest_raw_base,
            npt::boot_frames_for(self.shared.guest_mem.len()),
        );
        memory::deallocate_contiguous(self.shared.iopm_phys, 3);
        memory::deallocate_contiguous(self.shared.msrpm_phys, 2);
        // Free the active GuestMem (held outside VmShared). Safe here: the page
        // tables are released above and all vCPUs + the net backend have stopped
        // by the time close() runs, so no handle is live.
        crate::microvm::devices::guest_mem::clear_active();
    }
}

/// B3: allocate only the **contiguous boot window** (256 MiB or the
/// whole guest if smaller); `[boot, guest_bytes)` is demand-paged
/// 4 KB. Returns `(boot_base, npt_root=ncr3=pml4, boot_raw_base)`.
fn alloc_guest_ram_and_npt(guest_bytes: u64) -> Result<(u64, u64, u64), &'static str> {
    let raw_base = memory::allocate_contiguous(npt::boot_frames_for(guest_bytes))
        .ok_or("OOM allocating guest boot window (+ slack)")?;
    let boot_base = npt::round_up_to_2mb(raw_base);
    let npt_root = npt::allocate_window_npt(boot_base, guest_bytes)?;
    Ok((boot_base, npt_root, raw_base))
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
    vmcb.write_u32(vmcb::OFF_INTERCEPT_MISC1, vmcb::LINUX_MISC1);
    vmcb.write_u32(vmcb::OFF_INTERCEPT_MISC2, vmcb::LINUX_MISC2);
    vmcb.write_u32(vmcb::OFF_INT_CTL, vmcb::V_INTR_MASKING);

    vmcb.write_u64(vmcb::OFF_IOPM_BASE_PA, iopm_phys);
    vmcb.write_u64(vmcb::OFF_MSRPM_BASE_PA, msrpm_phys);
    vmcb.write_u32(vmcb::OFF_ASID, 1);
    // First entry only: drop what a previous run left under ASID 1 on this
    // core. `run_slice` clears it after the first VMRUN (see TLB_CTL there).
    vmcb.write_u8(vmcb::OFF_TLB_CTL, vmcb::tlb_flush_guest());
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

/// Configure an AP vCPU's VMCB for a SIPI start (guest SMP, Stage 3b):
/// 16-bit real mode at CS = `(vector<<8):0000`, i.e. physical entry
/// `vector<<12`, which is where Linux's BSP copied the real-mode AP
/// trampoline. The trampoline takes the AP to protected→long mode and
/// into `start_secondary`. The control area mirrors `setup_vmcb_linux`
/// and **shares** the BSP's NCR3 / IOPM / MSRPM (one guest address space,
/// one device intercept policy). The NPT only gains entries while the
/// guest runs, so the vCPUs need no cross-core NPT shootdown.
fn setup_vmcb_ap(
    vmcb: &mut vmcb::Vmcb,
    iopm_phys: u64,
    msrpm_phys: u64,
    npt_root: u64,
    sipi_vector: u8,
) {
    // ── Control area — identical to setup_vmcb_linux ────────────────
    vmcb.write_u32(vmcb::OFF_INTERCEPT_MISC1, vmcb::LINUX_MISC1);
    vmcb.write_u32(vmcb::OFF_INTERCEPT_MISC2, vmcb::LINUX_MISC2);
    vmcb.write_u32(vmcb::OFF_INT_CTL, vmcb::V_INTR_MASKING);
    vmcb.write_u64(vmcb::OFF_IOPM_BASE_PA, iopm_phys);
    vmcb.write_u64(vmcb::OFF_MSRPM_BASE_PA, msrpm_phys);
    vmcb.write_u32(vmcb::OFF_ASID, 1);
    vmcb.write_u8(vmcb::OFF_TLB_CTL, vmcb::tlb_flush_guest());
    vmcb.write_u64(vmcb::OFF_NESTED_CTL, 1); // NP_ENABLE
    vmcb.write_u64(vmcb::OFF_NCR3, npt_root);

    // ── State save area: 16-bit real mode, SIPI entry ──────────────
    let cs_base = (sipi_vector as u64) << 12;
    vmcb.write_segment(
        vmcb::OFF_SAVE_CS,
        /* selector */ (sipi_vector as u16) << 8,
        /* attrib   */ vmcb::ATTR_CODE_RM,
        /* limit    */ 0xFFFF,
        /* base     */ cs_base,
    );
    for off in [
        vmcb::OFF_SAVE_SS,
        vmcb::OFF_SAVE_DS,
        vmcb::OFF_SAVE_ES,
        vmcb::OFF_SAVE_FS,
        vmcb::OFF_SAVE_GS,
    ] {
        vmcb.write_segment(off, 0, vmcb::ATTR_DATA_RM, 0xFFFF, 0);
    }
    // GDTR/IDTR limits — leave zero; the trampoline reloads its own.
    vmcb.write_u32(vmcb::OFF_SAVE_GDTR + 4, 0);
    vmcb.write_u64(vmcb::OFF_SAVE_GDTR + 8, 0);
    vmcb.write_u32(vmcb::OFF_SAVE_IDTR + 4, 0);
    vmcb.write_u64(vmcb::OFF_SAVE_IDTR + 8, 0);

    // CR registers: architectural INIT/reset state (real mode, paging off,
    // caches off — Linux's AP path re-enables caching via MTRR init).
    vmcb.write_u64(vmcb::OFF_SAVE_CR0, 0x6000_0010);
    vmcb.write_u64(vmcb::OFF_SAVE_CR3, 0);
    vmcb.write_u64(vmcb::OFF_SAVE_CR4, 0);

    // EFER: SVME mandatory (APM §15.5.1); no LME — the AP enters in real
    // mode and its own trampoline sets LME/LMA when it builds long mode.
    vmcb.write_u64(vmcb::OFF_SAVE_EFER, EFER_SVME);

    vmcb.write_u64(vmcb::OFF_SAVE_RFLAGS, 0x0000_0002);
    vmcb.write_u64(vmcb::OFF_SAVE_RIP, 0);
    vmcb.write_u64(vmcb::OFF_SAVE_RSP, 0);
    vmcb.write_u64(vmcb::OFF_SAVE_RAX, 0);
    vmcb.write_u8(vmcb::OFF_SAVE_CPL, 0);
    vmcb.write_u64(vmcb::OFF_SAVE_G_PAT, 0x0007_0406_0007_0406);
}

impl VmContext {
    /// Build an AP vCPU that **shares** an already-open BSP's `VmShared`
    /// (guest SMP, Stage 3b). `shared` aliases the BSP's heap-boxed
    /// `VmShared` (valid for the VM lifetime via the last-one-out
    /// refcount); `sipi_vector` is the SIPI start page; `apic_id` is the
    /// AP's id (1..). No guest RAM / NPT / device allocation here — those
    /// belong to the BSP and are shared. EFER.SVME is enabled on this
    /// (the AP's) physical core.
    pub fn open_ap(
        shared: *mut VmShared,
        sipi_vector: u8,
        apic_id: u8,
    ) -> Result<VmContext, &'static str> {
        enable_efer_svme()?;

        // Read the shared substrate fields (iopm/msrpm/NPT root — set once
        // at BSP open, never mutated). Take VM_BIG_LOCK: by now AP_ACTIVE is
        // set, so the BSP may be accessing `*shared` under the lock — holding
        // it here keeps the `&*shared` borrow from aliasing the BSP's `&mut`.
        let (iopm_phys, msrpm_phys, npt_root) = {
            let _big = VM_BIG_LOCK.lock();
            // SAFETY: `shared` is valid for the VM lifetime (see SharedRef),
            // and the lock excludes any concurrent `&mut` to `*shared`.
            let s = unsafe { &*shared };
            (s.iopm_phys, s.msrpm_phys, s.npt_pml4)
        };

        let mut vmcb = alloc::boxed::Box::new(vmcb::Vmcb::zeroed());
        let vmcb_phys = vmcb.phys_addr();
        setup_vmcb_ap(&mut vmcb, iopm_phys, msrpm_phys, npt_root, sipi_vector);

        let regs = vmcb::GuestRegs::default();
        core::sync::atomic::fence(Ordering::SeqCst);

        Ok(VmContext {
            shared: SharedRef::Borrowed(shared),
            vcpu: Vcpu {
                apic_id,
                vmcb,
                vmcb_phys,
                regs,
                host_fpu: crate::microvm::cpu::FpuArea::boxed(),
                guest_fpu: crate::microvm::cpu::FpuArea::boxed(),
                reinject: 0,
                iter: 0,
                io_dropped: 0,
                lapic: LocalApic::new(apic_id),
                halt_poll_us: HALT_POLL_MIN_US,
                if_off_halts: 0,
                msrs: super::msr::GuestMsrs::new(),
                xcr0: 1,
            },
        })
    }

    /// Raw pointer to this VM's shared state, for an AP vCPU to alias
    /// (`open_ap`). Only valid to call on the BSP's `Owned` context.
    pub fn shared_ptr(&mut self) -> *mut VmShared {
        &mut *self.shared as *mut VmShared
    }

    /// Host TSC of this vCPU's next timer event — the LAPIC timer, and on the
    /// BSP the PIT — the deadline a blocked vCPU parks until (KVM's hrtimer).
    pub fn next_timer_deadline_tsc(&self) -> Option<u64> {
        let lapic = self.vcpu.lapic.next_timer_deadline_tsc();
        let mut dev = None;
        if self.vcpu.apic_id == 0 {
            dev = self.shared.pit.next_deadline_tsc();
            if let Some(t) = crate::microvm::devices::gpu_backend::resume_tsc() {
                dev = Some(dev.map_or(t, |d| d.min(t)));
            }
            // A playing sound stream is serviced every millisecond.
            if self.shared.pci.virtio_snd.playing() {
                let t = crate::interrupts::rdtsc() + crate::interrupts::tsc_freq() / 1000;
                dev = Some(dev.map_or(t, |d| d.min(t)));
            }
        }
        match (lapic, dev) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// Feed every device line and the PIT into the PIC (BSP: in PIC mode the
    /// device lines are wired there). Lock held by the caller when APs run.
    fn collect_device_irqs(&mut self) {
        let sh = &mut *self.shared;
        if crate::microvm::devices::net_backend::take_irq() {
            sh.pic.pulse(10);
            crate::microvm::devices::nat::note_net_irq();
        }
        if crate::microvm::devices::gpu_backend::take_irq() { sh.pic.pulse(9); }
        // vblank: a paused controlq runs its next frame (virtio-gpu IRQ 9).
        if crate::microvm::devices::gpu_backend::take_resume(crate::interrupts::rdtsc())
            && crate::microvm::devices::gpu_backend::lock()
                .service_queues(0, sh.guest_mem)
        {
            sh.pic.pulse(9);
        }
        if sh.pci.virtio_snd.pump(sh.guest_mem) {
            let l = sh.pci.virtio_snd.irq_line();
            sh.pic.pulse(l);
        }
        if sh.pci.virtio_9p.drain_async_done(sh.guest_mem) {
            let l = sh.pci.virtio_9p.irq_line();
            sh.pic.pulse(l);
        }
        if sh.pci.virtio_input.drain_injected(sh.guest_mem) { sh.pic.pulse(12); }
        sh.pit.poll(&mut sh.pic);

        let now = crate::interrupts::ticks();
        // Live resize (D4): disconnect, then reconnect after 100 ms; a new
        // cycle at most every 250 ms while the window is dragged.
        if crate::microvm::devices::gpu_backend::d4_pending()
            && crate::microvm::devices::gpu_backend::lock().tick_d4(now)
        {
            sh.pic.pulse(9);
        } else {
            let wid = crate::microvm::vm_window();
            if wid != 0
                && !crate::microvm::devices::gpu_backend::d4_pending()
                && crate::shade::surface::display_dirty_peek(wid)
                && now.wrapping_sub(sh.last_cfg_tick) >= 25
            {
                let _ = crate::shade::surface::take_display_dirty(wid);
                sh.last_cfg_tick = now;
                crate::microvm::devices::gpu_backend::lock().signal_display_change(now);
                sh.pic.pulse(9);
            }
        }
        // NAT mapping reaper: an idle scan, once per 10 ms at most.
        if now != sh.last_reap_tick {
            sh.last_reap_tick = now;
            crate::microvm::devices::nat::housekeep();
        }
    }

    /// `kvm_cpu_has_interrupt`: the PIC's INTR if this vCPU takes it (LINT0 in
    /// ExtINT, or no LAPIC), else a LAPIC vector above PPR.
    fn pending_interrupt(&mut self) -> Option<bool> {
        let lapic_on = crate::microvm::cpu::GUEST_LAPIC;
        if self.vcpu.apic_id == 0
            && self.shared.pic.output()
            && (!lapic_on || self.vcpu.lapic.accept_pic_intr())
        {
            return Some(true);
        }
        if lapic_on && self.vcpu.lapic.has_interrupt().is_some() {
            return Some(false);
        }
        None
    }

    /// `inject_pending_event`, before every VMRUN: re-inject what was cut off
    /// mid-delivery, otherwise the highest-priority deliverable interrupt; if
    /// the guest cannot take it now (IF=0, interrupt shadow, or an exception
    /// already queued), open the interrupt window so it exits the moment it can.
    fn inject_pending_event(&mut self) {
        let is_bsp = self.vcpu.apic_id == 0;
        let _big = if is_bsp && ap_active() { Some(VM_BIG_LOCK.lock()) } else { None };
        if is_bsp { self.collect_device_irqs(); }

        if self.vcpu.reinject != 0 {
            self.vcpu.vmcb.write_u64(vmcb::OFF_EVENT_INJ, self.vcpu.reinject);
            self.vcpu.reinject = 0;
            if self.pending_interrupt().is_some() {
                enable_irq_window(&mut self.vcpu.vmcb);
            }
            return;
        }
        let Some(from_pic) = self.pending_interrupt() else {
            clear_irq_window(&mut self.vcpu.vmcb);
            return;
        };
        let queued = self.vcpu.vmcb.read_u64(vmcb::OFF_EVENT_INJ) & (1u64 << 31) != 0;
        if queued || !guest_interruptible(&self.vcpu.vmcb) {
            enable_irq_window(&mut self.vcpu.vmcb);
            return;
        }
        // `kvm_cpu_get_interrupt`: ExtINT first, then the LAPIC.
        let vector = if from_pic {
            self.shared.pic.read_irq()
        } else {
            match self.vcpu.lapic.has_interrupt() {
                Some(v) => { self.vcpu.lapic.ack_interrupt(v); v }
                None => { clear_irq_window(&mut self.vcpu.vmcb); return; }
            }
        };
        self.vcpu.vmcb.write_u64(vmcb::OFF_EVENT_INJ, vector as u64 | (1u64 << 31));
        clear_irq_window(&mut self.vcpu.vmcb);
    }

    /// Anything deliverable right now (takes the lock + collects on the BSP).
    fn event_pending(&mut self) -> bool {
        let is_bsp = self.vcpu.apic_id == 0;
        let _big = if is_bsp && ap_active() { Some(VM_BIG_LOCK.lock()) } else { None };
        if is_bsp { self.collect_device_irqs(); }
        self.vcpu.reinject != 0 || self.pending_interrupt().is_some()
    }

    /// `kvm_vcpu_halt` with adaptive halt-polling (`halt_poll_ns`): after a
    /// guest HLT, true if an event arrived within the poll window (resume at
    /// once), false to block. The spin watches only lock-free signals — this
    /// core's kick generation (every cross-core producer kicks), a posted IPI,
    /// the worker's RX IRQ and the timer deadlines; a hit is confirmed by the
    /// next entry's `inject_pending_event`.
    fn halt_poll(&mut self) -> bool {
        lapic::phase(self.vcpu.apic_id, lapic::PH_HALTPOLL);
        if self.event_pending() {
            return true;
        }
        let cid = crate::smp::per_core::current_core_id();
        let kick_gen = crate::smp::fiber::net_kick_gen(cid);
        let deadline = self.next_timer_deadline_tsc();
        let start = crate::interrupts::rdtsc();
        let budget = (crate::interrupts::tsc_freq() / 1_000_000) * self.vcpu.halt_poll_us;
        loop {
            let now = crate::interrupts::rdtsc();
            let hit = crate::smp::fiber::net_kick_gen(cid) != kick_gen
                || lapic::posted_pending(self.vcpu.apic_id)
                || (self.vcpu.apic_id == 0 && crate::microvm::devices::net_backend::irq_pending())
                || deadline.is_some_and(|d| now >= d);
            if hit {
                self.vcpu.halt_poll_us = (self.vcpu.halt_poll_us * 2).min(HALT_POLL_MAX_US);
                return true;
            }
            if now.wrapping_sub(start) >= budget {
                break;
            }
            core::hint::spin_loop();
        }
        self.vcpu.halt_poll_us = (self.vcpu.halt_poll_us / 2).max(HALT_POLL_MIN_US);
        // Last look after the window: a producer that fired while we stopped
        // polling must not wait for the park.
        self.event_pending()
    }
}

/// `svm_set_vintr` (`svm_enable_irq_window`): request a VINTR exit the moment
/// the guest can take an interrupt — V_IRQ at top priority, VINTR intercepted.
fn enable_irq_window(vmcb: &mut vmcb::Vmcb) {
    let ctl = vmcb.read_u32(vmcb::OFF_INT_CTL);
    vmcb.write_u32(vmcb::OFF_INT_CTL,
        (ctl & !(vmcb::V_IRQ | vmcb::V_INTR_PRIO_MASK | vmcb::V_IGN_TPR))
        | vmcb::V_IRQ | (0xF << vmcb::V_INTR_PRIO_SHIFT));
    let m = vmcb.read_u32(vmcb::OFF_INTERCEPT_MISC1);
    vmcb.write_u32(vmcb::OFF_INTERCEPT_MISC1, m | vmcb::INTERCEPT_VINTR);
}

/// `svm_clear_vintr`.
fn clear_irq_window(vmcb: &mut vmcb::Vmcb) {
    let ctl = vmcb.read_u32(vmcb::OFF_INT_CTL);
    if ctl & vmcb::V_IRQ != 0 {
        vmcb.write_u32(vmcb::OFF_INT_CTL, ctl & !(vmcb::V_IRQ | vmcb::V_INTR_PRIO_MASK));
    }
    let m = vmcb.read_u32(vmcb::OFF_INTERCEPT_MISC1);
    if m & vmcb::INTERCEPT_VINTR != 0 {
        vmcb.write_u32(vmcb::OFF_INTERCEPT_MISC1, m & !vmcb::INTERCEPT_VINTR);
    }
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
    /// Set on the first `reboot: System halted` / `Power down` line — the
    /// guest shut itself down (LibreWolf X → cage exit → PID-1 halt). Drives
    /// the auto-close-and-save so the user doesn't need a second Mod+Q.
    halt_observed: bool,
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
            halt_observed: false,
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
            self.scan_for_shutdown(n);
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
            self.scan_for_shutdown(n);
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

    /// Detect a clean guest self-shutdown (`reboot: System halted` from PID-1
    /// `halt`, or `Power down`). On the first hit, ask the run loop to take the
    /// same clean exit as a user Mod+Q (break → save → window close) instead of
    /// spinning forever on the guest's `cli;hlt`.
    fn scan_for_shutdown(&mut self, n: usize) {
        if self.halt_observed { return; }
        // Require the `reboot: ` prefix (kernel/reboot.c only). PID-1 mirrors
        // cage/moz app logs to /dev/kmsg → serial, so a bare "Power down"
        // substring could false-trigger; the prefix can't appear in app text.
        let line = &self.line[..n];
        let hit = line_contains(line, b"reboot: System halted")
            || line_contains(line, b"reboot: Power down")
            || line_contains(line, b"reboot: Restarting");
        if hit {
            self.halt_observed = true;
            crate::kprintln!("[microvm] guest shut down — closing window + saving profile");
            crate::microvm::cpu::note_guest_shutdown();
        }
    }
}

/// True if `hay` contains `needle` as a contiguous byte substring.
fn line_contains(hay: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || hay.len() < needle.len() { return false; }
    (0..=(hay.len() - needle.len())).any(|i| &hay[i..i + needle.len()] == needle)
}

/// Linux VMRUN/VMEXIT loop. Caps at MAX_ITERATIONS to bound the
/// shell's response time when the guest never makes progress.
impl VmContext {
    /// Run the guest for up to `budget` VMEXITs, or until it exits.
    /// `Ok(StillRunning)` = budget hit, re-enterable (step 1b);
    /// `Ok(Exited(o))` = guest left; `Err` = fault. Body is the old
    /// `run_linux_loop` verbatim, `self.`-scoped.
    pub fn run_slice(&mut self, budget: u32) -> Result<SliceOutcome, &'static str> {
    use crate::kprintln;

    const MAX_ITERATIONS: u32 = 100_000;

    let mut last_outcome: Option<vmcb::LaunchOutcome> = None;
    let mut slice_n: u32 = 0;

    // Wall-clock slice cap: return to the fiber scheduler every few ms so the
    // core's other fibers run; the exit budget bounds boot bursts.
    const SLICE_MS: u64 = 3;
    let slice_deadline = crate::interrupts::rdtsc()
        + (crate::interrupts::tsc_freq() / 1000) * SLICE_MS;

    // Publish this vCPU's host core so a sender can kick it.
    lapic::set_host_core(self.vcpu.apic_id, crate::smp::per_core::current_core_id());

    while self.vcpu.iter < MAX_ITERATIONS || crate::microvm::vm_window() != 0 {
        if slice_n >= budget || crate::interrupts::rdtsc() >= slice_deadline {
            return Ok(SliceOutcome::StillRunning);
        }
        self.vcpu.iter = self.vcpu.iter.saturating_add(1);
        slice_n += 1;

        lapic::phase(self.vcpu.apic_id, lapic::PH_INJECT);
        self.inject_pending_event();
        self.vcpu.lapic.pv_eoi_sync_to(self.shared.guest_mem);

        // The guest's next timer, or the slice end, as a host one-shot on this
        // core: its fire is the exit that delivers the tick on time.
        crate::interrupts::arm_vcpu_timer(
            self.next_timer_deadline_tsc().map_or(slice_deadline, |d| d.min(slice_deadline)));

        // Host↔guest FPU save/restore is embedded in run_guest_once's asm.
        let hf: *mut crate::microvm::cpu::FpuArea = &mut *self.vcpu.host_fpu;
        let gf: *mut crate::microvm::cpu::FpuArea = &mut *self.vcpu.guest_fpu;
        let host_spec = super::msr::spec_ctrl_enter(self.vcpu.msrs.spec_ctrl);
        lapic::phase(self.vcpu.apic_id, lapic::PH_GUEST);
        let outcome = run_guest_once(
            &mut self.vcpu.regs, &mut *self.vcpu.vmcb, self.vcpu.vmcb_phys, hf, gf, self.vcpu.xcr0,
        );
        super::msr::spec_ctrl_exit(host_spec);
        let exit = outcome.exit_reason;
        lapic::phase(self.vcpu.apic_id, lapic::PH_EXIT);
        self.vcpu.lapic.pv_eoi_sync_from(self.shared.guest_mem);

        // Guest-RIP profiler: EXIT_INTR samples a running guest.
        if exit == EXIT_INTR {
            let rip = self.vcpu.vmcb.read_u64(vmcb::OFF_SAVE_RIP);
            crate::microvm::cpu::rip_sample::record(rip, self.vcpu.apic_id);
        }
        crate::microvm::cpu::rip_sample::maybe_dump();
        lapic::note_exit(self.vcpu.apic_id);
        crate::microvm::cpu::record_vm_exit(match exit {
            EXIT_INTR => crate::microvm::cpu::VMX_INTR,
            EXIT_HLT => crate::microvm::cpu::VMX_HLT,
            EXIT_NPF => crate::microvm::cpu::VMX_MMIO,
            EXIT_IOIO => crate::microvm::cpu::VMX_IO,
            EXIT_MSR => crate::microvm::cpu::VMX_MSR,
            EXIT_CPUID => crate::microvm::cpu::VMX_CPUID,
            _ => crate::microvm::cpu::VMX_OTHER,
        });

        // KVM clears control.event_inj after every run: under KVM-nested SVM
        // the CPU does not clear EVENTINJ.V, and a stale valid bit re-injects.
        self.vcpu.vmcb.write_u64(vmcb::OFF_EVENT_INJ, 0);
        // TLB_CONTROL back to DO_NOTHING (KVM `svm_flush_tlb_*` sets it only on
        // demand). The NPT only gains entries while the guest runs — not-present
        // to present needs no flush — so the one flush at the first entry is
        // all. It was 1 (flush ALL ASIDs, the host's too) on every VMRUN.
        self.vcpu.vmcb.write_u8(vmcb::OFF_TLB_CTL, vmcb::TLB_DO_NOTHING);

        // `svm_complete_interrupts`: an external interrupt / NMI aborted
        // mid-vectoring is re-injected; exceptions and software interrupts
        // re-occur on their own (the instruction was not retired).
        let eii = self.vcpu.vmcb.read_u64(vmcb::OFF_EXIT_INT_INFO);
        let eii_type = (eii >> 8) & 0x7;
        if eii & (1u64 << 31) != 0 && (eii_type == 0 || eii_type == 2) {
            self.vcpu.reinject = eii;
        }

        // Exits that touch only this vCPU take no VM_BIG_LOCK: a host
        // interrupt, the interrupt window, an MSR (x2APIC, PV-EOI) and a
        // hypercall (PV IPI). Behind the lock, one vCPU's EOI or IPI waited
        // for the other's device work — a framebuffer copy, a 9p write.
        match exit {
            // A host interrupt (timer, device, kick IPI) or NMI pre-empted the
            // guest; the host took it at STGI. Pending guest events are
            // injected at the next entry.
            EXIT_INTR | EXIT_NMI => {
                last_outcome = Some(outcome);
                continue;
            }
            // The interrupt window opened (`svm_enable_irq_window`): the guest
            // can take the event now — the next entry injects it.
            EXIT_VINTR => {
                clear_irq_window(&mut self.vcpu.vmcb);
                last_outcome = Some(outcome);
                continue;
            }
            EXIT_MSR => {
                // EXITINFO1 bit 0: 0=RDMSR, 1=WRMSR. Every MSR is intercepted
                // and emulated in `svm::msr`; none reaches the host.
                let is_write = outcome.exit_qualification & 1 != 0;
                let msr = self.vcpu.regs.rcx as u32;
                let wval = (self.vcpu.regs.rdx << 32)
                    | (self.vcpu.vmcb.read_u64(vmcb::OFF_SAVE_RAX) & 0xFFFF_FFFF);
                let lapic_r = if crate::microvm::cpu::GUEST_LAPIC {
                    self.vcpu.lapic.msr(msr, is_write.then_some(wval))
                } else {
                    None
                };
                let ok = if let Some(r) = lapic_r {
                    // IA32_APIC_BASE, x2APIC registers, PV-EOI enable.
                    match r {
                        Ok((v, ipi)) => {
                            if let Some(icr) = ipi { lapic::deliver_ipi(self.vcpu.apic_id, &icr); }
                            if !is_write {
                                self.vcpu.regs.rdx = v >> 32;
                                self.vcpu.vmcb.write_u64(vmcb::OFF_SAVE_RAX, v & 0xFFFF_FFFF);
                            }
                            true
                        }
                        Err(()) => false,
                    }
                } else if is_write {
                    let val = (self.vcpu.regs.rdx << 32)
                        | (self.vcpu.vmcb.read_u64(vmcb::OFF_SAVE_RAX) & 0xFFFF_FFFF);
                    super::msr::write(&mut self.vcpu.msrs, &mut self.vcpu.vmcb, msr, val).is_ok()
                } else {
                    match super::msr::read(&self.vcpu.msrs, &self.vcpu.vmcb, self.vcpu.apic_id, msr) {
                        Ok(v) => {
                            self.vcpu.regs.rdx = v >> 32;
                            self.vcpu.vmcb.write_u64(vmcb::OFF_SAVE_RAX, v & 0xFFFF_FFFF);
                            true
                        }
                        Err(()) => false,
                    }
                };
                if ok {
                    advance_rip(&mut *self.vcpu.vmcb);
                } else {
                    inject_exception(&mut self.vcpu.vmcb, VEC_GP, Some(0));
                }
                last_outcome = Some(outcome);
                continue;
            }
            EXIT_VMMCALL => {
                let nr = self.vcpu.vmcb.read_u64(vmcb::OFF_SAVE_RAX);
                let cpl = self.vcpu.vmcb.read_u8(vmcb::OFF_SAVE_CPL);
                let r = &self.vcpu.regs;
                let ret = crate::microvm::cpu::kvm_hypercall(
                    self.vcpu.apic_id, cpl, nr, r.rbx, r.rcx, r.rdx, r.rsi);
                self.vcpu.vmcb.write_u64(vmcb::OFF_SAVE_RAX, ret as u64);
                advance_rip(&mut *self.vcpu.vmcb);
                last_outcome = Some(outcome);
                continue;
            }
            _ => {}
        }

        // Serialize VmShared between vCPUs (guest SMP).
        lapic::phase(self.vcpu.apic_id, lapic::PH_BIGLOCK);
        let _big = if ap_active() { Some(VM_BIG_LOCK.lock()) } else { None };
        lapic::phase(self.vcpu.apic_id, lapic::PH_EXIT);
        let sh = &mut *self.shared;

        match exit {
            EXIT_HLT => {
                // A headless test guest halting is done.
                if crate::microvm::vm_window() == 0 {
                    sh.serial.flush();
                    kprintln!("[svm] guest HLT after {} VM-exits — exiting", self.vcpu.iter);
                    last_outcome = Some(outcome);
                    break;
                }
                // `cli; hlt` is Linux's no-ACPI poweroff / panic end state.
                if self.vcpu.vmcb.read_u64(vmcb::OFF_SAVE_RFLAGS) & (1 << 9) == 0 {
                    self.vcpu.if_off_halts = self.vcpu.if_off_halts.saturating_add(1);
                    if self.vcpu.if_off_halts >= 64 {
                        sh.serial.flush();
                        kprintln!(
                            "[svm] guest powered off (HLT with IF=0) after {} iters — exiting",
                            self.vcpu.iter
                        );
                        last_outcome = Some(outcome);
                        break;
                    }
                } else {
                    self.vcpu.if_off_halts = 0;
                }
                advance_rip(&mut self.vcpu.vmcb);
                last_outcome = Some(outcome);
                drop(_big);
                // `kvm_vcpu_halt`: resume at once if something is deliverable,
                // else halt-poll, then block (the fiber parks until the next
                // timer deadline or a kick).
                if !self.halt_poll() {
                    return Ok(SliceOutcome::Idle);
                }
            }
            EXIT_CPUID => {
                let leaf = self.vcpu.vmcb.read_u64(vmcb::OFF_SAVE_RAX) as u32;
                let subleaf = self.vcpu.regs.rcx as u32;
                let (eax, ebx, ecx, edx) = crate::microvm::cpu::guest_cpuid::guest_cpuid(
                    crate::microvm::cpu::guest_cpuid::Vendor::Amd, leaf, subleaf, self.vcpu.apic_id,
                    self.vcpu.vmcb.read_u64(vmcb::OFF_SAVE_CR4), self.vcpu.xcr0,
                );
                self.vcpu.vmcb.write_u64(vmcb::OFF_SAVE_RAX, eax as u64);
                self.vcpu.regs.rbx = ebx as u64;
                self.vcpu.regs.rcx = ecx as u64;
                self.vcpu.regs.rdx = edx as u64;
                advance_rip(&mut *self.vcpu.vmcb);
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
                crate::microvm::cpu::record_io_port(port);
                handle_linux_io(&mut *self.vcpu.vmcb, &mut sh.serial, &mut sh.pci, &mut sh.pic, &mut self.vcpu.regs, port, dir_in, size, &mut self.vcpu.io_dropped, &mut sh.pit);
                advance_rip(&mut *self.vcpu.vmcb);
                last_outcome = Some(outcome);
            }
            // SVM instructions: the guest has no SVM (CPUID hides it, EFER.SVME
            // writes #GP), so they #UD — KVM `nested_svm_check_permissions`.
            // Left unintercepted they would run natively: the VMCB carries
            // EFER.SVME=1, and VMLOAD/VMSAVE then take a HOST physical address.
            // KVM hypercall (`kvm_emulate_hypercall`): nr in RAX, args in
            // RBX/RCX/RDX/RSI, result in RAX. Only from CPL 0.
            EXIT_VMRUN | EXIT_VMLOAD | EXIT_VMSAVE | EXIT_STGI
            | EXIT_CLGI | EXIT_SKINIT | EXIT_INVLPGA | EXIT_RDPRU
            | EXIT_MONITOR | EXIT_MWAIT | EXIT_MWAIT_COND => {
                inject_exception(&mut self.vcpu.vmcb, VEC_UD, None);
                last_outcome = Some(outcome);
            }
            // No vPMU (KVM with enable_pmu=0): RDPMC faults.
            EXIT_RDPMC => {
                inject_exception(&mut self.vcpu.vmcb, VEC_GP, Some(0));
                last_outcome = Some(outcome);
            }
            // No non-coherent DMA into the guest → both are no-ops
            // (KVM `kvm_emulate_wbinvd`, INVD treated as WBINVD).
            EXIT_WBINVD | EXIT_INVD => {
                advance_rip(&mut *self.vcpu.vmcb);
                last_outcome = Some(outcome);
            }
            EXIT_XSETBV => {
                let idx = self.vcpu.regs.rcx as u32;
                let val = (self.vcpu.regs.rdx << 32)
                    | (self.vcpu.vmcb.read_u64(vmcb::OFF_SAVE_RAX) & 0xFFFF_FFFF);
                let cpl = self.vcpu.vmcb.read_u8(vmcb::OFF_SAVE_CPL);
                if cpl == 0 && idx == 0 && xcr0_valid(val) {
                    self.vcpu.xcr0 = val;
                    advance_rip(&mut *self.vcpu.vmcb);
                } else {
                    inject_exception(&mut self.vcpu.vmcb, VEC_GP, Some(0));
                }
                last_outcome = Some(outcome);
            }
            EXIT_SHUTDOWN => {
                sh.serial.flush();
                if sh.serial.panic_observed {
                    kprintln!(
                        "[svm] linux kernel panicked (after {} iters): {}",
                        self.vcpu.iter, sh.serial.panic_msg_str(),
                    );
                    kprintln!("[svm] guest then triple-faulted via emergency_restart (= expected reboot path)");
                } else {
                    kprintln!(
                        "[svm] guest triple-faulted/shutdown after {} iters (no kernel-panic on console)",
                        self.vcpu.iter,
                    );
                }
                last_outcome = Some(outcome);
                break;
            }
            EXIT_NPF => {
                let gpa = self.vcpu.vmcb.read_u64(vmcb::OFF_EXIT_INFO_2);
                crate::microvm::cpu::record_npf(npf_kind(sh, gpa));
                if sh.pci.virtio_blk.bar0_in_range(gpa) {
                    if handle_mmio_npf_blk(&mut *self.vcpu.vmcb, &mut self.vcpu.regs, &mut sh.pci.virtio_blk, &mut sh.pic, gpa, sh.guest_mem) {
                        last_outcome = Some(outcome);
                        continue;
                    }
                } else if crate::microvm::devices::net_backend::bar0_in_range(gpa) {
                    if handle_mmio_npf_net(&mut *self.vcpu.vmcb, &mut self.vcpu.regs, &mut sh.pic, gpa, sh.guest_mem) {
                        // Deliver a deferred device IRQ (esp. an async 9p
                        // write-completion) NOW rather than at the next
                        // EXIT_INTR/EXIT_HLT ~10 ms out — the download rxlat fix.
                        last_outcome = Some(outcome);
                        continue;
                    }
                } else if crate::microvm::devices::gpu_backend::bar0_in_range(gpa) {
                    let mut gpu = crate::microvm::devices::gpu_backend::lock();
                    if handle_mmio_npf_gpu(&mut *self.vcpu.vmcb, &mut self.vcpu.regs, &mut *gpu, &mut sh.pic, gpa, sh.guest_mem) {
                        last_outcome = Some(outcome);
                        continue;
                    }
                } else if sh.pci.virtio_input.bar0_in_range(gpa) {
                    if handle_mmio_npf_input(&mut *self.vcpu.vmcb, &mut self.vcpu.regs, &mut sh.pci.virtio_input, &mut sh.pic, gpa, sh.guest_mem) {
                        last_outcome = Some(outcome);
                        continue;
                    }
                } else if sh.pci.virtio_9p.bar0_in_range(gpa) {
                    if handle_mmio_npf_p9(&mut *self.vcpu.vmcb, &mut self.vcpu.regs, &mut sh.pci.virtio_9p, &mut sh.pic, gpa, sh.guest_mem) {
                        // Deliver the freshly-completed 9p write-reply IRQ NOW
                        // (latched by drain_async_done at the loop top) instead
                        // of waiting for the next EXIT_INTR/EXIT_HLT.
                        last_outcome = Some(outcome);
                        continue;
                    }
                } else if sh.pci.virtio_blk_sqfs.bar0_in_range(gpa) {
                    // Same handler — VirtioBlk carries its own IRQ line.
                    if handle_mmio_npf_blk(&mut *self.vcpu.vmcb, &mut self.vcpu.regs, &mut sh.pci.virtio_blk_sqfs, &mut sh.pic, gpa, sh.guest_mem) {
                        last_outcome = Some(outcome);
                        continue;
                    }
                } else if sh.pci.virtio_snd.bar0_in_range(gpa) {
                    if handle_mmio_npf_snd(&mut *self.vcpu.vmcb, &mut self.vcpu.regs, &mut sh.pci.virtio_snd, &mut sh.pic, gpa, sh.guest_mem) {
                        last_outcome = Some(outcome);
                        continue;
                    }
                }
                // I/O APIC page (0xFEC00000), NPT-not-present like the LAPIC's.
                {
                    use crate::microvm::devices::ioapic::{IOAPIC_BASE, IOAPIC_SIZE};
                    if (IOAPIC_BASE..IOAPIC_BASE + IOAPIC_SIZE).contains(&gpa)
                        && handle_mmio_npf_ioapic(&mut self.vcpu.vmcb, &mut self.vcpu.regs, sh, gpa)
                    {
                        last_outcome = Some(outcome);
                        continue;
                    }
                }
                // Local APIC MMIO (guest-SMP Stage 1). The LAPIC page
                // (0xFEE00000) is left NPT-not-present (npt.rs) so the
                // guest's xAPIC accesses #NPF here → trap-and-emulate.
                // Inert while booting `nolapic`.
                if (lapic::LAPIC_BASE..lapic::LAPIC_BASE + lapic::LAPIC_SIZE).contains(&gpa) {
                    if handle_mmio_npf_lapic(
                        &mut self.vcpu.vmcb, &mut self.vcpu.regs,
                        &mut self.vcpu.lapic, self.vcpu.apic_id, sh, gpa,
                    ) {
                        last_outcome = Some(outcome);
                        continue;
                    }
                }
                // B3: demand-paged guest RAM. #NPF on a gpa inside the
                // advertised window but above the contiguous boot
                // block = first touch of a 4-KB demand page → fault it
                // in + re-enter. Order: MMIO BAR ranges first (above),
                // RAM-demand here, fatal dump last.
                if sh.guest_mem.ensure(gpa) {
                    last_outcome = Some(outcome);
                    continue;
                }
                sh.serial.flush();
                kprintln!(
                    "[svm] NPF: gpa={:#018x} info1={:#x} after {} iters",
                    gpa, outcome.exit_qualification, self.vcpu.iter,
                );
                last_outcome = Some(outcome);
                break;
            }
            EXIT_INVALID => {
                sh.serial.flush();
                kprintln!(
                    "[svm] VMEXIT_INVALID — VMCB consistency check failed (info1={:#x})",
                    outcome.exit_qualification,
                );
                last_outcome = Some(outcome);
                break;
            }
            _ => {
                sh.serial.flush();
                kprintln!(
                    "[svm] unhandled exit {:#x} info1={:#x} after {} iters",
                    exit, outcome.exit_qualification, self.vcpu.iter,
                );
                last_outcome = Some(outcome);
                break;
            }
        }
    }

    if self.vcpu.iter >= MAX_ITERATIONS && crate::microvm::vm_window() == 0 {
        self.shared.serial.flush();
        kprintln!(
            "[svm] iteration cap ({}) reached — guest still running ({} I/O drops)",
            MAX_ITERATIONS, self.vcpu.io_dropped,
        );
    }

    // Persist the virtio-blk profile-image to npkFS (encrypted at
    // rest). Reached only when the loop ended (guest exit / cap), not
    // on a StillRunning yield — identical to the old run_linux_loop.
    self.shared.pci.virtio_blk.save();

    match last_outcome {
        Some(o) => Ok(SliceOutcome::Exited(o)),
        None => Err("SVM Linux guest exceeded max iterations without first VMEXIT"),
    }
    }
}

/// Advance guest RIP across an *intercept* exit (CPUID / IOIO / MSR /
/// HLT etc.) via NRIP_SAVE. Requires CPUID 8000_000A EDX[3].
///
/// NRIP_SAVE is **undefined for #NPF and other hardware exceptions**
/// (APM Vol 2 §15.7.1) — KVM nested SVM zeroes it, which would land
/// the next VMRUN at RIP=0 and panic the guest. MMIO emulation must
/// use `advance_rip_by_length` instead.
fn advance_rip(vmcb: &mut vmcb::Vmcb) {
    let nrip = vmcb.read_u64(vmcb::OFF_NRIP);
    vmcb.write_u64(vmcb::OFF_SAVE_RIP, nrip);
    clear_interrupt_shadow(vmcb);
}

/// KVM `__svm_skip_emulated_instruction` → `svm_set_interrupt_shadow(0)`: the
/// skipped instruction was the one an STI / MOV SS shadow covered.
fn clear_interrupt_shadow(vmcb: &mut vmcb::Vmcb) {
    let st = vmcb.read_u64(vmcb::OFF_INT_STATE);
    if st & 1 != 0 { vmcb.write_u64(vmcb::OFF_INT_STATE, st & !1); }
}

/// Advance guest RIP by the decoded instruction length. Used after
/// emulating an #NPF MMIO access (NRIP_SAVE is unreliable there).
fn advance_rip_by_length(vmcb: &mut vmcb::Vmcb, length: u8) {
    let rip = vmcb.read_u64(vmcb::OFF_SAVE_RIP);
    vmcb.write_u64(vmcb::OFF_SAVE_RIP, rip.wrapping_add(length as u64));
    clear_interrupt_shadow(vmcb);
}

/// Dispatch one I/O VMEXIT. UART COM1 (0x3F8-0x3FF) gets proper
/// synthetic responses; everything else absorbed (return 0 for IN,
/// no-op for OUT). Mirror of `vmx::handle_linux_io` shape, but
/// reads/writes RAX through VMCB instead of the GPR struct (SVM
/// auto-saves RAX in VMCB.SAVE.RAX on every VMEXIT).
fn handle_linux_io(
    vmcb: &mut vmcb::Vmcb,
    serial: &mut SerialState,
    pci: &mut crate::microvm::devices::PciBus,
    pic: &mut crate::microvm::devices::pic8259::Pic8259,
    _regs: &mut vmcb::GuestRegs,
    port: u16,
    dir_in: bool,
    size: u8,
    io_dropped: &mut u32,
    pit: &mut crate::microvm::devices::pit8253::Pit,
) {
    use crate::microvm::devices::{handle_pci_io, PCI_CONFIG_ADDR, PCI_CONFIG_DATA_END, PCI_CONFIG_DATA_START};
    use crate::microvm::devices::pic8259::{PIC_ELCR_MASTER, PIC_ELCR_SLAVE, PIC_MASTER_CMD, PIC_MASTER_IMR, PIC_SLAVE_CMD, PIC_SLAVE_IMR};

    let mask: u64 = match size { 1 => 0xFF, 2 => 0xFFFF, 4 => 0xFFFF_FFFF, _ => 0xFF };
    let rax = vmcb.read_u64(vmcb::OFF_SAVE_RAX);
    let val_out = (rax & mask) as u32;

    // PCI config-space ports — dispatch to the bus emulator.
    if port == PCI_CONFIG_ADDR
        || (PCI_CONFIG_DATA_START..=PCI_CONFIG_DATA_END).contains(&port)
    {
        if let Some(v) = handle_pci_io(pci, port, dir_in, size, val_out) {
            let new_rax = (rax & !mask) | (v & mask);
            vmcb.write_u64(vmcb::OFF_SAVE_RAX, new_rax);
        }
        return;
    }

    // 8259 PIC + ELCR.
    if matches!(port, PIC_MASTER_CMD | PIC_MASTER_IMR | PIC_SLAVE_CMD | PIC_SLAVE_IMR
                    | PIC_ELCR_MASTER | PIC_ELCR_SLAVE) {
        if let Some(v) = pic.ioport(port, dir_in, val_out as u8) {
            let new_rax = (rax & !mask) | (v & mask);
            vmcb.write_u64(vmcb::OFF_SAVE_RAX, new_rax);
        }
        return;
    }

    let in_value: Option<u64> = match (port, dir_in) {
        // i8254 channel 0 (the other channels are not a tick source).
        (0x43, false) => { pit.command(val_out as u8); None }
        (0x40, false) => { pit.write_counter(val_out as u8); None }
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

/// Handle a #NPF on virtio-blk's BAR0 MMIO range. Uses SVM
/// decode-assists (CPUID 8000_000A EDX[7], probed at init) to read the
/// faulting instruction bytes from the VMCB, decodes the MOV form,
/// emulates the access against the device's MMIO model and advances
/// RIP via NRIP_SAVE.
///
/// Returns `true` if the fault was handled. `false` falls through to
/// the generic NPF dump path (decode failure, unsupported opcode).
/// Can the guest take a maskable external interrupt right now? RFLAGS.IF=1
/// AND no interrupt shadow (the 1-instruction block after STI / MOV SS).
/// Mirrors KVM `svm_interrupt_allowed` / the VMX `guest_interruptible`.
///
/// EVENTINJ is a FORCED injection — AMD delivers it regardless of guest IF
/// (APM Vol 2 §15.20). Harmless on a UP guest (spinlocks compile to no-ops),
/// but on an SMP guest, firing a device IRQ into a `spin_lock_irqsave`
/// critical section makes the handler spin on the held qspinlock → deadlock
/// (the guest-SMP 9p-mount livelock). So we only inject when interruptible,
/// and open the interrupt window otherwise (`inject_pending_event`).
fn guest_interruptible(vmcb: &vmcb::Vmcb) -> bool {
    let rflags = vmcb.read_u64(vmcb::OFF_SAVE_RFLAGS);
    let int_state = vmcb.read_u64(vmcb::OFF_INT_STATE);
    (rflags & (1 << 9)) != 0 && (int_state & 1) == 0
}

/// #NPF on the LAPIC MMIO page (guest-SMP Stage 1). Decode the faulting
/// MOV, service it against the per-vCPU `LocalApic`, advance RIP. xAPIC
/// registers are 32-bit; no device IRQ-kick (unlike the virtio handlers).
/// #NPF on the I/O APIC page → `devices::ioapic` (register window).
fn handle_mmio_npf_ioapic(
    vmcb: &mut vmcb::Vmcb,
    regs: &mut vmcb::GuestRegs,
    sh: &mut VmShared,
    gpa: u64,
) -> bool {
    use crate::kprintln;
    use crate::microvm::devices::guest_fetch::fetch_inst;
    use crate::microvm::devices::insn_decoder::{decode_mov, width_mask};

    let rip = vmcb.read_u64(vmcb::OFF_SAVE_RIP);
    let cr3 = vmcb.read_u64(vmcb::OFF_SAVE_CR3);
    let Some(buf) = fetch_inst(rip, cr3, sh.guest_mem) else {
        kprintln!("[svm] ioapic mmio: insn fetch failed (rip={:#x} gpa={:#x})", rip, gpa);
        return false;
    };
    let Some(dec) = decode_mov(&buf) else {
        kprintln!("[svm] ioapic mmio: unsupported insn @ gpa={:#x} bytes={:02x?}", gpa, &buf[..8]);
        return false;
    };
    let off = (gpa - crate::microvm::devices::ioapic::IOAPIC_BASE) as u32;
    let rax = vmcb.read_u64(vmcb::OFF_SAVE_RAX);
    if dec.is_write {
        let value = read_guest_gpr(regs, rax, dec.reg) & width_mask(dec.width);
        sh.pic.ioapic.mmio(off, Some(value as u32));
    } else {
        let value = sh.pic.ioapic.mmio(off, None).unwrap_or(0) as u64;
        write_guest_gpr(regs, vmcb, rax, dec.reg, dec.width, value & width_mask(dec.width));
    }
    advance_rip_by_length(vmcb, dec.length);
    true
}

fn handle_mmio_npf_lapic(
    vmcb: &mut vmcb::Vmcb,
    regs: &mut vmcb::GuestRegs,
    apic: &mut LocalApic,
    apic_id: u8,
    sh: &mut VmShared,
    gpa: u64,
) -> bool {
    use crate::kprintln;
    use crate::microvm::devices::guest_fetch::fetch_inst;
    use crate::microvm::devices::insn_decoder::{decode_mov, width_mask};

    let rip = vmcb.read_u64(vmcb::OFF_SAVE_RIP);
    let cr3 = vmcb.read_u64(vmcb::OFF_SAVE_CR3);
    let buf = match fetch_inst(rip, cr3, sh.guest_mem) {
        Some(b) => b,
        None => {
            kprintln!("[svm] lapic mmio: insn fetch failed (rip={:#x} gpa={:#x})", rip, gpa);
            return false;
        }
    };
    let dec = match decode_mov(&buf) {
        Some(d) => d,
        None => {
            kprintln!("[svm] lapic mmio: unsupported insn @ gpa={:#x} bytes={:02x?}", gpa, &buf[..8]);
            return false;
        }
    };

    let off = (gpa - lapic::LAPIC_BASE) as u32;
    let rax = vmcb.read_u64(vmcb::OFF_SAVE_RAX);
    if dec.is_write {
        let value = read_guest_gpr(regs, rax, dec.reg) & width_mask(dec.width);
        if let Some(icr) = apic.write(off, value as u32) {
            lapic::deliver_ipi(apic_id, &icr);
        }
    } else {
        let value = apic.read(off) as u64;
        write_guest_gpr(regs, vmcb, rax, dec.reg, dec.width, value);
    }
    advance_rip_by_length(vmcb, dec.length);
    true
}

fn handle_mmio_npf_blk(
    vmcb: &mut vmcb::Vmcb,
    regs: &mut vmcb::GuestRegs,
    blk: &mut crate::microvm::devices::virtio_blk_pci::VirtioBlk,
    pic: &mut crate::microvm::devices::pic8259::Pic8259,
    gpa: u64,
    mem: &GuestMem,
) -> bool {
    use crate::kprintln;
    use crate::microvm::devices::guest_fetch::fetch_inst;
    use crate::microvm::devices::insn_decoder::{decode_mov, width_mask};

    // Walk the guest's page tables to fetch the faulting instruction.
    // KVM nested SVM doesn't populate decode-assists for #NPF, so we
    // can't rely on VMCB.GUEST_INST_BYTES.
    let rip = vmcb.read_u64(vmcb::OFF_SAVE_RIP);
    let cr3 = vmcb.read_u64(vmcb::OFF_SAVE_CR3);
    let buf = match fetch_inst(rip, cr3, mem) {
        Some(b) => b,
        None => {
            kprintln!(
                "[svm] mmio: insn fetch failed (rip={:#x} cr3={:#x} gpa={:#x})",
                rip, cr3, gpa,
            );
            return false;
        }
    };

    let dec = match decode_mov(&buf) {
        Some(d) => d,
        None => {
            kprintln!(
                "[svm] mmio: unsupported insn @ gpa={:#x}, bytes={:02x?}",
                gpa, &buf[..8],
            );
            return false;
        }
    };

    let off = (gpa - blk.bar0_base()) as u32;
    let rax = vmcb.read_u64(vmcb::OFF_SAVE_RAX);

    if dec.is_write {
        let value = read_guest_gpr(regs, rax, dec.reg) & width_mask(dec.width);
        blk.mmio_write(off, dec.width, value);
    } else {
        let value = blk.mmio_read(off, dec.width);
        write_guest_gpr(regs, vmcb, rax, dec.reg, dec.width, value);
    }

    if let Some(qidx) = blk.take_pending_kick() {
        let advanced = blk.service_queues(qidx, mem);
        if advanced {
            // Inject if interruptible, else defer (SMP qspinlock safety).
            pic.pulse(blk.irq_line());
        }
    }

    advance_rip_by_length(vmcb, dec.length);
    true
}

/// Read a guest GPR by ModR/M index 0..15. RAX comes from VMCB.SAVE.RAX
/// (SVM auto-saves it); the rest from the GuestRegs struct we maintain.
fn read_guest_gpr(regs: &vmcb::GuestRegs, rax: u64, idx: u8) -> u64 {
    match idx {
        0  => rax,
        1  => regs.rcx,
        2  => regs.rdx,
        3  => regs.rbx,
        4  => 0, // RSP — never an MMIO source on Linux's path
        5  => regs.rbp,
        6  => regs.rsi,
        7  => regs.rdi,
        8  => regs.r8,
        9  => regs.r9,
        10 => regs.r10,
        11 => regs.r11,
        12 => regs.r12,
        13 => regs.r13,
        14 => regs.r14,
        15 => regs.r15,
        _  => 0,
    }
}

/// Write a value into a guest GPR by ModR/M index, honouring x86 width
/// rules. RAX writes go to VMCB.SAVE.RAX directly.
fn write_guest_gpr(
    regs: &mut vmcb::GuestRegs,
    vmcb: &mut vmcb::Vmcb,
    rax: u64,
    idx: u8,
    width: u8,
    value: u64,
) {
    use crate::microvm::devices::insn_decoder::merge_reg;
    match idx {
        0  => vmcb.write_u64(vmcb::OFF_SAVE_RAX, merge_reg(rax, value, width)),
        1  => regs.rcx = merge_reg(regs.rcx, value, width),
        2  => regs.rdx = merge_reg(regs.rdx, value, width),
        3  => regs.rbx = merge_reg(regs.rbx, value, width),
        4  => {} // RSP — silently drop
        5  => regs.rbp = merge_reg(regs.rbp, value, width),
        6  => regs.rsi = merge_reg(regs.rsi, value, width),
        7  => regs.rdi = merge_reg(regs.rdi, value, width),
        8  => regs.r8  = merge_reg(regs.r8,  value, width),
        9  => regs.r9  = merge_reg(regs.r9,  value, width),
        10 => regs.r10 = merge_reg(regs.r10, value, width),
        11 => regs.r11 = merge_reg(regs.r11, value, width),
        12 => regs.r12 = merge_reg(regs.r12, value, width),
        13 => regs.r13 = merge_reg(regs.r13, value, width),
        14 => regs.r14 = merge_reg(regs.r14, value, width),
        15 => regs.r15 = merge_reg(regs.r15, value, width),
        _  => {}
    }
}

/// Handle a #NPF on virtio-net's BAR0. Mirror of `handle_mmio_npf_blk`
/// for the second device. To be de-duplicated alongside the VMX twin
/// when virtio-gpu lands.
fn handle_mmio_npf_net(
    vmcb: &mut vmcb::Vmcb,
    regs: &mut vmcb::GuestRegs,
    pic: &mut crate::microvm::devices::pic8259::Pic8259,
    gpa: u64,
    mem: &GuestMem,
) -> bool {
    use crate::kprintln;
    use crate::microvm::devices::guest_fetch::fetch_inst;
    use crate::microvm::devices::insn_decoder::{decode_mov, width_mask};

    let rip = vmcb.read_u64(vmcb::OFF_SAVE_RIP);
    let cr3 = vmcb.read_u64(vmcb::OFF_SAVE_CR3);
    let buf = match fetch_inst(rip, cr3, mem) {
        Some(b) => b,
        None => {
            kprintln!("[svm] mmio-net: insn fetch failed (rip={:#x} gpa={:#x})", rip, gpa);
            return false;
        }
    };
    let dec = match decode_mov(&buf) {
        Some(d) => d,
        None => {
            kprintln!("[svm] mmio-net: unsupported insn @ gpa={:#x}, bytes={:02x?}", gpa, &buf[..8]);
            return false;
        }
    };

    let off = (gpa - crate::microvm::devices::virtio_net_dev::BAR0_BASE) as u32;
    let rax = vmcb.read_u64(vmcb::OFF_SAVE_RAX);

    // ISR read and TX doorbell: lock-free (the worker may hold the device).
    if let Some(v) = crate::microvm::devices::net_backend::mmio_fast(off, dec.is_write) {
        if !dec.is_write {
            write_guest_gpr(regs, vmcb, rax, dec.reg, dec.width, v & width_mask(dec.width));
        }
        advance_rip_by_length(vmcb, dec.length);
        return true;
    }

    let mut net = crate::microvm::devices::net_backend::lock();
    if dec.is_write {
        let value = read_guest_gpr(regs, rax, dec.reg) & width_mask(dec.width);
        net.mmio_write(off, dec.width, value);
    } else {
        let value = net.mmio_read(off, dec.width);
        write_guest_gpr(regs, vmcb, rax, dec.reg, dec.width, value);
    }

    if let Some(qidx) = net.take_pending_kick() {
        if crate::microvm::devices::net_backend::full_active() && qidx == 1 {
            // TX off-vCPU (Stage 2c): hand the TX kick to the worker, which owns
            // service_tx + tx_flush on its core — RX and TX then share one core /
            // one NET path (no cross-core NET-lock fight delaying ACK egress).
            crate::microvm::devices::net_backend::note_tx_kick();
        } else {
            let advanced = net.service_queues(qidx, mem);
            if advanced {
                pic.pulse(10);
            }
        }
    }

    advance_rip_by_length(vmcb, dec.length);
    true
}

/// Handle a #NPF on virtio-gpu BAR0. Mirror of `handle_mmio_npf_net`.
fn handle_mmio_npf_gpu(
    vmcb: &mut vmcb::Vmcb,
    regs: &mut vmcb::GuestRegs,
    gpu: &mut crate::microvm::devices::virtio_gpu_pci::VirtioGpu,
    pic: &mut crate::microvm::devices::pic8259::Pic8259,
    gpa: u64,
    mem: &GuestMem,
) -> bool {
    use crate::kprintln;
    use crate::microvm::devices::guest_fetch::fetch_inst;
    use crate::microvm::devices::insn_decoder::{decode_mov, width_mask};

    let rip = vmcb.read_u64(vmcb::OFF_SAVE_RIP);
    let cr3 = vmcb.read_u64(vmcb::OFF_SAVE_CR3);
    let buf = match fetch_inst(rip, cr3, mem) {
        Some(b) => b,
        None => {
            kprintln!("[svm] mmio-gpu: insn fetch failed (rip={:#x} gpa={:#x})", rip, gpa);
            return false;
        }
    };
    let dec = match decode_mov(&buf) {
        Some(d) => d,
        None => {
            kprintln!("[svm] mmio-gpu: unsupported insn @ gpa={:#x}, bytes={:02x?}", gpa, &buf[..8]);
            return false;
        }
    };

    let off = (gpa - gpu.bar0_base()) as u32;
    let rax = vmcb.read_u64(vmcb::OFF_SAVE_RAX);

    if dec.is_write {
        let value = read_guest_gpr(regs, rax, dec.reg) & width_mask(dec.width);
        gpu.mmio_write(off, dec.width, value);
    } else {
        let value = gpu.mmio_read(off, dec.width);
        write_guest_gpr(regs, vmcb, rax, dec.reg, dec.width, value);
    }

    if let Some(qidx) = gpu.take_pending_kick() {
        if crate::microvm::devices::gpu_backend::full_active() {
            // Off-vCPU: defer the heavy ~8 MB copy + write_frame to the GPU worker
            // on its own core (it raises IRQ9 + kicks the BSP). The vCPU exit stays
            // cheap → no framebuffer copy stealing net cycles. note_gpu_kick is
            // lock-free (atomics); the worker briefly waits for this exit to drop
            // the gpu_backend lock on return — no cycle (it never takes VM_BIG_LOCK).
            crate::microvm::devices::gpu_backend::note_gpu_kick(qidx);
        } else {
            let advanced = gpu.service_queues(qidx, mem);
            if advanced {
                // virtio-gpu IRQ line = 9.
                pic.pulse(9);
            }
        }
    }

    advance_rip_by_length(vmcb, dec.length);
    true
}

/// Handle a #NPF on virtio-input BAR0. Mirror of `handle_mmio_npf_gpu`
/// — only the device and IRQ line differ.
fn handle_mmio_npf_input(
    vmcb: &mut vmcb::Vmcb,
    regs: &mut vmcb::GuestRegs,
    input: &mut crate::microvm::devices::virtio_input_pci::VirtioInput,
    pic: &mut crate::microvm::devices::pic8259::Pic8259,
    gpa: u64,
    mem: &GuestMem,
) -> bool {
    use crate::kprintln;
    use crate::microvm::devices::guest_fetch::fetch_inst;
    use crate::microvm::devices::insn_decoder::{decode_mov, width_mask};

    let rip = vmcb.read_u64(vmcb::OFF_SAVE_RIP);
    let cr3 = vmcb.read_u64(vmcb::OFF_SAVE_CR3);
    let buf = match fetch_inst(rip, cr3, mem) {
        Some(b) => b,
        None => {
            kprintln!("[svm] mmio-input: insn fetch failed (rip={:#x} gpa={:#x})", rip, gpa);
            return false;
        }
    };
    let dec = match decode_mov(&buf) {
        Some(d) => d,
        None => {
            kprintln!("[svm] mmio-input: unsupported insn @ gpa={:#x}, bytes={:02x?}", gpa, &buf[..8]);
            return false;
        }
    };

    let off = (gpa - input.bar0_base()) as u32;
    let rax = vmcb.read_u64(vmcb::OFF_SAVE_RAX);

    if dec.is_write {
        let value = read_guest_gpr(regs, rax, dec.reg) & width_mask(dec.width);
        input.mmio_write(off, dec.width, value);
    } else {
        let value = input.mmio_read(off, dec.width);
        write_guest_gpr(regs, vmcb, rax, dec.reg, dec.width, value);
    }

    if let Some(qidx) = input.take_pending_kick() {
        let advanced = input.service_queues(qidx, mem);
        if advanced {
            // virtio-input IRQ line = 12.
            pic.pulse(12);
        }
    }

    advance_rip_by_length(vmcb, dec.length);
    true
}

/// Handle a #NPF on virtio-9p BAR0. Mirror of `handle_mmio_npf_input`
/// — only the device and IRQ line differ (9p = line 6).
fn handle_mmio_npf_p9(
    vmcb: &mut vmcb::Vmcb,
    regs: &mut vmcb::GuestRegs,
    p9: &mut crate::microvm::devices::virtio_9p_pci::Virtio9p,
    pic: &mut crate::microvm::devices::pic8259::Pic8259,
    gpa: u64,
    mem: &GuestMem,
) -> bool {
    use crate::kprintln;
    use crate::microvm::devices::guest_fetch::fetch_inst;
    use crate::microvm::devices::insn_decoder::{decode_mov, width_mask};

    let rip = vmcb.read_u64(vmcb::OFF_SAVE_RIP);
    let cr3 = vmcb.read_u64(vmcb::OFF_SAVE_CR3);
    let buf = match fetch_inst(rip, cr3, mem) {
        Some(b) => b,
        None => {
            kprintln!("[svm] mmio-9p: insn fetch failed (rip={:#x} gpa={:#x})", rip, gpa);
            return false;
        }
    };
    let dec = match decode_mov(&buf) {
        Some(d) => d,
        None => {
            kprintln!("[svm] mmio-9p: unsupported insn @ gpa={:#x}, bytes={:02x?}", gpa, &buf[..8]);
            return false;
        }
    };

    let off = (gpa - p9.bar0_base()) as u32;
    let rax = vmcb.read_u64(vmcb::OFF_SAVE_RAX);

    if dec.is_write {
        let value = read_guest_gpr(regs, rax, dec.reg) & width_mask(dec.width);
        p9.mmio_write(off, dec.width, value);
    } else {
        let value = p9.mmio_read(off, dec.width);
        write_guest_gpr(regs, vmcb, rax, dec.reg, dec.width, value);
    }

    if let Some(qidx) = p9.take_pending_kick() {
        let advanced = p9.service_queues(qidx, mem);
        if advanced {
            pic.pulse(p9.irq_line());
        }
    }

    advance_rip_by_length(vmcb, dec.length);
    true
}

/// Handle a #NPF on virtio-snd BAR0. Mirror of `handle_mmio_npf_p9`.
fn handle_mmio_npf_snd(
    vmcb: &mut vmcb::Vmcb,
    regs: &mut vmcb::GuestRegs,
    snd: &mut crate::microvm::devices::virtio_snd_pci::VirtioSnd,
    pic: &mut crate::microvm::devices::pic8259::Pic8259,
    gpa: u64,
    mem: &GuestMem,
) -> bool {
    use crate::microvm::devices::guest_fetch::fetch_inst;
    use crate::microvm::devices::insn_decoder::{decode_mov, width_mask};

    let rip = vmcb.read_u64(vmcb::OFF_SAVE_RIP);
    let cr3 = vmcb.read_u64(vmcb::OFF_SAVE_CR3);
    let buf = match fetch_inst(rip, cr3, mem) { Some(b) => b, None => return false };
    let dec = match decode_mov(&buf) { Some(d) => d, None => return false };

    let off = (gpa - snd.bar0_base()) as u32;
    let rax = vmcb.read_u64(vmcb::OFF_SAVE_RAX);
    if dec.is_write {
        let value = read_guest_gpr(regs, rax, dec.reg) & width_mask(dec.width);
        snd.mmio_write(off, dec.width, value);
    } else {
        let value = snd.mmio_read(off, dec.width);
        write_guest_gpr(regs, vmcb, rax, dec.reg, dec.width, value);
    }

    if let Some(qidx) = snd.take_pending_kick() {
        let advanced = snd.service_queues(qidx, mem);
        if advanced {
            pic.pulse(snd.irq_line());
        }
    }

    advance_rip_by_length(vmcb, dec.length);
    true
}
