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

use super::{cpuid as host_cpuid, lapic, npt, rdmsr, vmcb, wrmsr};
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

/// True once the AP vCPU is well PAST its bring-up (its `iter` crossed
/// `AP_ESTABLISHED_ITERS`). The idle-yield-on-shadowed-HLT optimisation is
/// gated on this: applying it *during* AP bring-up changes the BSP's
/// idle/sleep timing and races the cpuhp handshake (the AP got stuck after
/// one CPUID exit — v0.193.4). Once the AP is established, both vCPUs may
/// park on an idle `sti; hlt`. Reset on teardown. Stays false on a UP guest
/// (no AP), which idles fine via the interruptible EXIT_INTR path anyway.
pub static AP_ESTABLISHED: AtomicBool = AtomicBool::new(false);

/// vCPU `iter` count after which the AP is considered past bring-up. The AP
/// does at most a few×10k exits during bring-up (~100 ms); idle-spinning it
/// reaches this in ~3 s — a large, safe margin past the handshake.
const AP_ESTABLISHED_ITERS: u32 = 200_000;

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
        &mut regs, vmcb_ptr, vmcb_phys, &mut *hf, &mut *gf);

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
    host_fpu: *mut crate::microvm::cpu::FpuArea,
    guest_fpu: *mut crate::microvm::cpu::FpuArea,
) -> vmcb::LaunchOutcome {
    let regs_ptr: *mut vmcb::GuestRegs = regs;
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
            // Stack now: [rsp+0]=guest_fpu [+8]=host_fpu
            //   [+16]=host_extra [+24]=vmcb_phys [+32]=struct_ptr

            // ── FPU SAVE/RESTORE (KVM kvm_load_guest_fpu): vmrun
            // preserves no x87/SSE/AVX/AVX-512. Must be in-asm,
            // adjacent to vmrun — a +avx2-built kernel spills `ymm`
            // anywhere between a Rust helper and the asm, clobbering
            // the just-restored guest state. Mask = -1 (all
            // XCR0-enabled components; guest owns XCR0 via XSETBV).
            // Only GPR/vm* instructions sit between this xrstor and
            // vmrun, and between vmrun and the paired xsave below.
            "mov rcx, [rsp + 8]",           // host_fpu
            "mov eax, 0xffffffff",
            "mov edx, 0xffffffff",
            "xsave64 [rcx]",                // save host FPU
            "mov rcx, [rsp + 0]",           // guest_fpu
            "mov eax, 0xffffffff",
            "mov edx, 0xffffffff",
            "xrstor64 [rcx]",               // restore guest FPU

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
            "mov rax, [rsp + 16]",          // host_extra_save phys
            "vmsave rax",                   // save host FS/GS/TR/LDTR/MSRs
            "mov rax, [rsp + 24]",          // guest vmcb_phys
            "clgi",
            "vmload rax",                   // load guest FS/GS/KernelGS/STAR/LSTAR/SFMASK/SYSENTER
            "vmrun rax",
            "vmsave rax",                   // save guest's back into the guest VMCB
            "mov rax, [rsp + 16]",          // host_extra_save phys
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
            // Stack now: r15..rbx (14=112B), guest_fpu[+112],
            //   host_fpu[+120], host_extra[+128], vmcb[+136],
            //   struct_ptr[+144], host_callee_saved(6).

            // ── FPU SAVE/RESTORE (paired exit half): no FP since
            // vmexit (only vm*/push above). Save guest FPU, restore
            // host's. Guest GPRs are on the stack now → rax/rcx/rdx
            // free to clobber.
            "mov rcx, [rsp + 112]",         // guest_fpu
            "mov eax, 0xffffffff",
            "mov edx, 0xffffffff",
            "xsave64 [rcx]",                // save guest FPU
            "mov rcx, [rsp + 120]",         // host_fpu
            "mov eax, 0xffffffff",
            "mov edx, 0xffffffff",
            "xrstor64 [rcx]",               // restore host FPU

            // Recover struct ptr (rax free to clobber).
            "mov rax, [rsp + 144]",

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
            "add rsp, 40",                          // discard guest_fpu,host_fpu,host_extra,vmcb,struct

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

// ── Re-entrant VM context (Phase 12.4 step 1c — SVM mirror of 1a) ──
//
// Same decomposition as vmx::enable: open() / run_slice(budget) /
// close() so the Core-0 event loop can interleave Shade rendering
// between bounded slices (PHASE12_DISPLAY_BRIDGE.md R1). SVM is
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
pub struct VmShared {
    guest_mem: GuestMem,
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
    /// Host tick at which we last injected guest timer IRQ0. The
    /// microvm provides no PIT/LAPIC timer event source, so without
    /// this the guest's nanosleep/timerfd/poll-timeout never wake
    /// (input read() wakes via virtio-input IRQ; time does not).
    /// One IRQ0 per host tick (≈100 Hz) drives jiffies + wakeups.
    last_timer_tick: u64,
    /// `ticks()` of the last virtio-gpu display config-change IRQ —
    /// rate-limits the resize round-trip against a drag storm. See
    /// the vmx mirror.
    last_cfg_tick: u64,
    /// Device IRQ lines (bit N = IRQ N) whose injection was deferred because
    /// the guest was non-interruptible (IF=0) when the device completed —
    /// the run loop injects them once the guest can take them. Avoids the
    /// SMP qspinlock deadlock from forcing an IRQ into a critical section.
    pending_irqs: u16,
    /// Is the 8259 PIT (i8253 ch0) currently programmed to generate the
    /// timer tick? True until Linux writes PIT mode-0 (port 0x43 ← 0x30,
    /// `clockevent_i8253_disable`) to shut it down after adopting the
    /// LAPIC timer. Gates IRQ0 injection so the PIT tick stops then and
    /// jiffies don't double-count against the LAPIC timer. Starts true so
    /// IRQ0 drives boot before Linux first programs the PIT.
    pit_enabled: bool,
    /// Pending inter-processor-interrupt vectors per target vCPU, indexed
    /// by apic_id: `ipi_pending[t]` is a 256-bit bitmap (bit V = vector V
    /// pending for vCPU t). A vCPU's ICR write (FIXED IPI) sets bits in the
    /// target's word; each vCPU drains its own word and injects the lowest
    /// pending vector via EVENTINJ when interruptible. This is the
    /// cross-vCPU IPI path that carries reschedule / call-function / TLB
    /// IPIs — and crucially the AP→BSP `complete()` wakeup without which the
    /// cpuhp bring-up `wait_for_completion` never returns. Guest SMP only.
    ipi_pending: [[u64; 4]; MAX_VCPUS],
}

/// Maximum vCPUs per guest (guest SMP). Sizes the per-vCPU IPI bitmaps;
/// `guest_vcpus()` (the count we actually enumerate) must be ≤ this. Matches the
/// VMX twin + `cpu::ORCH_MAX_VCPUS`/`MAX_VCPUS_CAP` so every apic_id has a slot.
pub const MAX_VCPUS: usize = 8;

impl VmShared {
    /// Mark interrupt `vector` pending for the vCPU with `apic_id` (an IPI
    /// target). No-op if the id is out of range.
    fn ipi_set(&mut self, apic_id: u8, vector: u8) {
        let t = apic_id as usize;
        if t < MAX_VCPUS {
            self.ipi_pending[t][(vector >> 6) as usize] |= 1u64 << (vector & 63);
        }
    }

    /// Take (and clear) the lowest pending IPI vector for `apic_id`, if any.
    fn ipi_take(&mut self, apic_id: u8) -> Option<u8> {
        let t = apic_id as usize;
        if t >= MAX_VCPUS {
            return None;
        }
        for (w, word) in self.ipi_pending[t].iter_mut().enumerate() {
            if *word != 0 {
                let bit = word.trailing_zeros();
                *word &= !(1u64 << bit);
                return Some((w as u32 * 64 + bit) as u8);
            }
        }
        None
    }
}

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
    msr_log_count: u32,
    consecutive_idle: u32,
    /// Per-vCPU emulated local APIC (xAPIC MMIO @ 0xFEE00000). Inert
    /// while the guest boots `nolapic`; drives the timer + (later) IPIs
    /// once Linux enables it. See `svm::lapic`.
    lapic: LocalApic,
    /// `ticks()` of the last injected LAPIC-timer (LVTT) interrupt — the
    /// per-vCPU analogue of `VmShared::last_timer_tick` (PIT IRQ0).
    last_lapic_tick: u64,
    /// Consecutive HLT exits taken with guest RFLAGS.IF=0. The idle path
    /// is always `sti; hlt` (IF=1); a sustained `cli; hlt` loop is Linux's
    /// no-ACPI poweroff / panic path (we boot `acpi=off`, so there is no
    /// ACPI shutdown signal). Crossing the threshold = the guest powered
    /// off → exit the VM (tears the window down on browser close instead
    /// of leaving a black idle window). Reset on any IF=1 HLT.
    if_off_halts: u32,
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

        // IOPM: 12 KB all-ones = trap every port. Linux touches dozens
        // of unique ports during boot (UART, PIC, PIT, RTC, …).
        // Cheaper to trap-all + handle the boring ones in the loop.
        let iopm_phys = memory::allocate_contiguous(3)
            .ok_or("OOM allocating IOPM (12 KB)")?;
        // SAFETY: freshly allocated, identity-mapped, exclusive.
        unsafe { core::ptr::write_bytes(iopm_phys as *mut u8, 0xFF, 3 * 4096); }

        // MSRPM: 8 KB all-zero = pass-through every MSR. Architectural
        // CPU-state MSRs are auto-saved/loaded by the CPU via the
        // VMCB.SAVE area on VMRUN/VMEXIT (APM §15.11.1).
        let msrpm_phys = memory::allocate_contiguous(2)
            .ok_or("OOM allocating MSRPM (8 KB)")?;
        // SAFETY: as above.
        unsafe { core::ptr::write_bytes(msrpm_phys as *mut u8, 0, 2 * 4096); }

        // Intercept IA32_APIC_BASE (0x1B) reads + writes so the EXIT_MSR
        // handler reports our emulated value — crucially with the BSP bit
        // (bit 8) set for the boot vCPU. Without this the all-zero MSRPM
        // passes the RDMSR straight to the host core, which is NOT the
        // host BSP (the guest runs on an arbitrary worker core) → its
        // APICBASE has the BSP bit clear. Linux's topology code then sees
        // an enumerated BSP whose APICBASE BSP bit is unset, mistakes the
        // guest for a kdump/crash kernel, and caps it at 1 CPU
        // (`topo_is_converted_bsp`, topology.c) — which silently defeats
        // guest SMP. MSRPM range 0 covers MSRs 0x0..0x1FFF at 2 bits each
        // (bit0 read / bit1 write); 0x1B → bit 54 → byte 6, bits 6|7.
        // Gated on GUEST_LAPIC: with `nolapic` we want host pass-through.
        if crate::microvm::cpu::GUEST_LAPIC {
            // SAFETY: msrpm_phys is a freshly allocated, identity-mapped
            // 8 KB region we own; byte 6 is well within it.
            unsafe { *((msrpm_phys + 6) as *mut u8) |= 0xC0; }
        }

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

        Ok(VmContext {
            shared: SharedRef::owned(VmShared {
                guest_mem: gm,
                npt_pml4: npt_root,
                guest_raw_base,
                iopm_phys,
                msrpm_phys,
                serial,
                pci: crate::microvm::devices::PciBus::new(),
                pic: crate::microvm::devices::pic8259::Pic8259::new(),
                last_timer_tick: 0,
                last_cfg_tick: 0,
                pending_irqs: 0,
                pit_enabled: true,
                ipi_pending: [[0; 4]; MAX_VCPUS],
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
                msr_log_count: 0,
                consecutive_idle: 0,
                lapic: LocalApic::new(0),
                last_lapic_tick: 0,
                if_off_halts: 0,
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
    }
}

/// Boot a Linux bzImage in our SVM substrate. Step 1c: open() then a
/// single unbounded run_slice — byte-for-byte the old run_linux_loop.
pub fn run_linux(
    bzimage_bytes: &[u8],
    cmdline: &[u8],
    initramfs: Option<&[u8]>,
    inject: &[u8],
) -> Result<vmcb::LaunchOutcome, &'static str> {
    let mut ctx = VmContext::open(bzimage_bytes, cmdline, initramfs, inject)?;
    let result = loop {
        match ctx.run_slice(u32::MAX) {
            Ok(SliceOutcome::Exited(o)) => break Ok(o),
            Ok(SliceOutcome::StillRunning) | Ok(SliceOutcome::Idle) => continue,
            Err(e) => break Err(e),
        }
    };
    ctx.close();
    result
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

/// Configure an AP vCPU's VMCB for a SIPI start (guest SMP, Stage 3b):
/// 16-bit real mode at CS = `(vector<<8):0000`, i.e. physical entry
/// `vector<<12`, which is where Linux's BSP copied the real-mode AP
/// trampoline. The trampoline takes the AP to protected→long mode and
/// into `start_secondary`. The control area mirrors `setup_vmcb_linux`
/// and **shares** the BSP's NCR3 / IOPM / MSRPM (one guest address space,
/// one device intercept policy). `TLB_CTL=1` (flush this guest's TLB on
/// every entry) means NPT mutations made by any vCPU are picked up by the
/// others on their next VMRUN — no separate cross-core TLB-shootdown IPI.
fn setup_vmcb_ap(
    vmcb: &mut vmcb::Vmcb,
    iopm_phys: u64,
    msrpm_phys: u64,
    npt_root: u64,
    sipi_vector: u8,
) {
    // ── Control area — identical to setup_vmcb_linux ────────────────
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
    vmcb.write_u8(vmcb::OFF_TLB_CTL, 1);
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
                msr_log_count: 0,
                consecutive_idle: 0,
                lapic: LocalApic::new(apic_id),
                last_lapic_tick: 0,
                if_off_halts: 0,
            },
        })
    }

    /// Raw pointer to this VM's shared state, for an AP vCPU to alias
    /// (`open_ap`). Only valid to call on the BSP's `Owned` context.
    pub fn shared_ptr(&mut self) -> *mut VmShared {
        &mut *self.shared as *mut VmShared
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
    const MSR_LOG_CAP: u32 = 32;

    // Idle detection — once init enters its pause(2)/wait-loop, the
    // only exits are external interrupts (= host timer). After
    // IDLE_THRESHOLD consecutive INTRs declare guest idle and bail.
    // Threshold: Linux's TSC-calibration path can spin for thousands
    // of host timer ticks before giving up + moving on, so we keep
    // this generous. Phase 12.1.4-svm replaces this with a real
    // cancel signal.
    // Match the VMX side (200). The original 5000 was a wall-of-spin
    // safety pad from the early AMD bring-up when TSC calibration was
    // still suspect; with `tsc_early_khz=2000000` short-circuiting
    // Linux's calib loop, 200 INTRs ≈ 2 s wait — short enough for
    // fast iterate-on-qemu test cycles.
    const IDLE_THRESHOLD: u32 = 200;
    // Yield Core 0 after this few consecutive idle INTRs — see the
    // vmx mirror. A parked guest must not freeze Core 0 for a whole
    // SLICE_BUDGET of timer-tick-rate VMRUNs.
    const IDLE_YIELD: u32 = 4;

    // MAX_ITERATIONS is a *lifetime* cap on cumulative VMRUNs (never
    // reset across slices) — a safety pad for short headless test
    // guests. A window-bound app VM (cage/Wayland, ~60 fps × many
    // exits/frame) blows past 100 000 in seconds; capping it kills a
    // perfectly-running compositor (observed: colored circles, then
    // teardown at the cap). So: no lifetime cap for a windowed VM —
    // it runs until the window closes or the guest really exits. The
    // per-slice `slice_n >= budget` bound below still keeps each call
    // cooperative. saturating_add so a long-lived windowed VM's iter
    // counter can't overflow once past the (now-irrelevant) cap.
    // Wall-clock slice cap — see the vmx mirror for the rationale.
    // A busy guest never reaches IDLE_YIELD, so the exit-count
    // budget alone lets one slice run tens of ms and starves Shade
    // (laggy UI, sluggish Mod+Q). Bound each slice to a few ms of
    // wall time; boot still hits the exit budget first.
    const SLICE_MS: u64 = 3;
    let slice_deadline = crate::interrupts::rdtsc()
        + (crate::interrupts::tsc_freq() / 1000) * SLICE_MS;

    while self.vcpu.iter < MAX_ITERATIONS || crate::microvm::vm_window() != 0 {
        if slice_n >= budget || crate::interrupts::rdtsc() >= slice_deadline {
            return Ok(SliceOutcome::StillRunning);
        }

        self.vcpu.iter = self.vcpu.iter.saturating_add(1);
        slice_n += 1;

        // Mark the AP "established" once it is well past bring-up — this
        // un-gates the idle-yield-on-shadowed-HLT for BOTH vCPUs (see
        // AP_ESTABLISHED). Only the AP sets it; the BSP reads it.
        if self.vcpu.apic_id != 0
            && self.vcpu.iter == AP_ESTABLISHED_ITERS
            && !AP_ESTABLISHED.load(Ordering::Relaxed)
        {
            AP_ESTABLISHED.store(true, Ordering::Release);
            kprintln!("[svm] AP established (past bring-up) — idle-park enabled");
        }

        // Re-inject an event that #VMEXITed mid-delivery on the
        // previous run (AMD APM §15.20; KVM svm_complete_interrupts).
        // Highest priority for the single EVENTINJ slot: a lost
        // in-flight interrupt corrupts the guest far worse than a
        // skipped fresh tick (which the next EXIT_INTR regenerates).
        if self.vcpu.reinject != 0 {
            self.vcpu.vmcb.write_u64(vmcb::OFF_EVENT_INJ, self.vcpu.reinject);
            self.vcpu.reinject = 0;
        }

        // Host↔guest FPU save/restore is embedded in run_guest_once's
        // asm, bracketing vmrun with zero compiler-emittable code (a
        // +avx2 kernel spills ymm between any Rust helper and the asm
        // → clobbers the restored guest FPU). Pass the areas in.
        let hf: *mut crate::microvm::cpu::FpuArea = &mut *self.vcpu.host_fpu;
        let gf: *mut crate::microvm::cpu::FpuArea = &mut *self.vcpu.guest_fpu;
        let outcome =
            run_guest_once(&mut self.vcpu.regs, &mut *self.vcpu.vmcb, self.vcpu.vmcb_phys, hf, gf);
        let exit = outcome.exit_reason;

        // Exit-reason histogram (diagnosis — `cores` shows the mix).
        crate::microvm::cpu::record_vm_exit(match exit {
            EXIT_INTR => crate::microvm::cpu::VMX_INTR,
            EXIT_HLT => crate::microvm::cpu::VMX_HLT,
            EXIT_NPF => crate::microvm::cpu::VMX_MMIO,
            EXIT_IOIO => crate::microvm::cpu::VMX_IO,
            EXIT_MSR => crate::microvm::cpu::VMX_MSR,
            EXIT_CPUID => crate::microvm::cpu::VMX_CPUID,
            _ => crate::microvm::cpu::VMX_OTHER,
        });

        // Consume the injection slot. Proven by the v0.172.41 probe:
        // under KVM-nested SVM the CPU does NOT clear EVENTINJ.V after
        // a successful injection (val=0x800000xx stayed valid before
        // the next VMRUN we never re-armed). Left set, the next VMRUN
        // re-injects the SAME IRQ → phantom duplicate interrupts →
        // cumulative guest corruption (rate ∝ VMRUN/s — fast on the
        // dedicated core, ~74 s cooperative). KVM clears
        // control.event_inj after every run for exactly this; do the
        // same. The reinject/EXIT_INTR/MMIO sites re-arm it for the
        // next VMRUN as needed.
        self.vcpu.vmcb.write_u64(vmcb::OFF_EVENT_INJ, 0);

        // Did an event abort mid-vectoring? EXITINTINFO has the same
        // encoding as EVENTINJ. Re-inject ONLY external interrupts
        // (type 0) / NMI (type 2): their source is transient, so a
        // lost one is gone forever (the udevd corruption v0.172.36
        // fixed). Exceptions (type 3, e.g. #PF — librewolf/cage spew
        // COW faults) and software ints (type 4) MUST NOT be
        // re-injected: the faulting instruction was not retired (we
        // don't advance RIP on #NPF), so the guest re-executes it and
        // the exception re-occurs naturally — re-injecting too =
        // double delivery → the recurring rt_sigprocmask `<ret>`
        // corruption. Mirrors KVM svm_complete_interrupts' type
        // discrimination.
        let eii = self.vcpu.vmcb.read_u64(vmcb::OFF_EXIT_INT_INFO);
        let eii_type = (eii >> 8) & 0x7;
        if eii & (1u64 << 31) != 0 && (eii_type == 0 || eii_type == 2) {
            self.vcpu.reinject = eii;
        }

        // EXIT_HLT counts as idle too (guest halted waiting for an IRQ),
        // so it shares the EXIT_INTR keepalive path below.
        if exit != EXIT_INTR && exit != EXIT_HLT { self.vcpu.consecutive_idle = 0; }

        // Take the big-VM lock around this exit's device/memory handling
        // when an AP shares the VM (guest SMP), so the two vCPUs serialize
        // access to `VmShared`. Both `_big` and `sh` are dropped at the end
        // of the loop body — `sh`'s `&mut VmShared` borrow ends before the
        // lock releases (reverse declaration order), so only the lock holder
        // ever has a live `&mut` to the shared state (sound aliasing across
        // the BSP's Owned box and the AP's Borrowed pointer to it). When no
        // AP is active the lock is not taken and this is byte-identical to
        // the single-vCPU path.
        let _big = if ap_active() { Some(VM_BIG_LOCK.lock()) } else { None };
        // Resolve the shared device/memory state once for this exit's
        // handling. Through `SharedRef::DerefMut` this is a single borrow
        // of `self.shared`, so the handlers below can still take disjoint
        // sub-field borrows (`&mut sh.serial` + `&mut sh.pci`) the way they
        // did when `shared` was a plain field; `self.vcpu` stays separately
        // borrowable (disjoint field of `self`).
        let sh = &mut *self.shared;

        match exit {
            EXIT_INTR | EXIT_HLT => {
                // Guest executed STI;HLT (idle, waiting for its next IRQ).
                // A headless test guest means "done" → exit. A window-bound
                // app idling is normal: fall through to the same inject-due-
                // IRQ + idle-accounting path as a host-timer EXIT_INTR so the
                // guest's timer/input/net wake it; never kill it on idle. The
                // injected IRQ (EVENT_INJ) is delivered at the HLT, so the
                // guest resumes past it — no manual RIP advance needed. The
                // dedicated core host-idles between ticks via SliceOutcome::Idle.
                if exit == EXIT_HLT && crate::microvm::vm_window() == 0 {
                    sh.serial.flush();
                    kprintln!("[svm] guest HLT after {} VM-exits — exiting", self.vcpu.iter);
                    last_outcome = Some(outcome);
                    break;
                }
                // A WINDOWED guest that HLTs with interrupts DISABLED
                // (RFLAGS.IF=0) is NOT idle — it has powered off / panicked
                // (Linux's no-ACPI poweroff + panic paths both end in a
                // `cli; hlt` loop; the idle path is always `sti; hlt` =
                // IF=1). We boot `acpi=off`, so there is no ACPI shutdown
                // signal — this IF check is it. Treat a sustained IF=0 HLT
                // as a clean exit so the tile tears down when the browser
                // closes (cage exits → PID-1 `halt -f` → cli;hlt loop)
                // instead of hanging as a black idle window until Mod+Q.
                if exit == EXIT_HLT {
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
                }
                // HLT is an intercepted instruction: advance RIP past it
                // (like KVM's skip_emulated_instruction) so that once an
                // injected IRQ's ISR returns, the guest continues into its
                // need_resched check and schedules — not back onto the HLT.
                if exit == EXIT_HLT {
                    advance_rip(&mut self.vcpu.vmcb);
                }
                // If the guest can't take an interrupt right now (IF=0 / in
                // an STI/MOV-SS shadow — i.e. mid `spin_lock_irqsave`), do
                // NOT inject anything: forcing an IRQ in via EVENTINJ here
                // deadlocks an SMP guest's qspinlock (the guest-SMP 9p-mount
                // livelock). Just re-enter so the guest finishes its critical
                // section; it's busy, not idle, so skip idle-accounting too.
                // Pending device IRQs + the timer are retried on a later exit
                // once IF=1 (≤1 ms via the worker/VM timer).
                // An idle `sti; hlt` runs the HLT inside the STI interrupt-
                // shadow, so `guest_interruptible` is false even though IF=1.
                // But the HLT CONSUMES the shadow and MUST be woken by a
                // pending interrupt — that is the whole point of `sti; hlt`.
                // So treat such an idle HLT as interruptible: fall through to
                // the injection path below (deliver the pending IPI/IRQ/timer
                // that wakes the guest — else the wakeup is LOST and the guest
                // stalls) and the idle-park accounting. Gated on
                // AP_ESTABLISHED so AP bring-up keeps the validated timing.
                let idle_hlt = exit == EXIT_HLT
                    && self.vcpu.vmcb.read_u64(vmcb::OFF_SAVE_RFLAGS) & (1 << 9) != 0;
                if !guest_interruptible(&self.vcpu.vmcb)
                    && !(idle_hlt && AP_ESTABLISHED.load(Ordering::Acquire))
                {
                    // A genuine IF=0 critical section (e.g. mid
                    // `spin_lock_irqsave`), or a non-HLT instruction in an
                    // STI/MOV-SS shadow: do NOT inject — forcing an IRQ in via
                    // EVENTINJ here deadlocks an SMP guest's qspinlock (the
                    // 9p-mount livelock). Re-enter so the guest finishes its
                    // critical section; busy, not idle, so skip idle-accounting.
                    last_outcome = Some(outcome);
                    continue;
                }
                // Cross-vCPU IPI (guest SMP): a reschedule / call-function /
                // TLB-flush / AP→BSP `complete()` wakeup another vCPU routed
                // to this one. Highest priority among injectables here — these
                // are LAPIC-delivered and carry the SMP bring-up handshake.
                // No-op on a UP guest (`ipi_pending` stays all-zero).
                if let Some(vec) = sh.ipi_take(self.vcpu.apic_id) {
                    let info: u64 = (vec as u64) | (1u64 << 31);
                    self.vcpu.vmcb.write_u64(vmcb::OFF_EVENT_INJ, info);
                    self.vcpu.consecutive_idle = 0;
                    continue;
                }
                // Device IRQs, the PIT tick, display-resize, NAT pump and
                // input are BSP-only (PIC mode delivers device lines to the
                // boot CPU); an AP vCPU only services its own LAPIC timer
                // (below) + cross-vCPU IPIs (above). Guest SMP, Stage 3b.
                let is_bsp = self.vcpu.apic_id == 0;
                // Deferred device IRQ (queued while the guest was IF=0) is now
                // deliverable — inject the lowest pending line.
                if is_bsp && sh.pending_irqs != 0 {
                    let line = sh.pending_irqs.trailing_zeros() as u8;
                    sh.pending_irqs &= !(1u16 << line);
                    let info: u64 = (sh.pic.vector_for_irq(line) as u64) | (1u64 << 31);
                    self.vcpu.vmcb.write_u64(vmcb::OFF_EVENT_INJ, info);
                    self.vcpu.consecutive_idle = 0;
                    continue;
                }
                // Guest timer tick. The microvm has no PIT/LAPIC
                // timer event source, so the only thing that ever
                // wakes a time-blocked task (nanosleep/timerfd/poll
                // timeout — the entire seatd/cage/wlroots/libwayland
                // event-loop machinery) is this injected IRQ0. Pace
                // to the host 100 Hz tick: one IRQ0 per host tick.
                // Gate on Linux having unmasked IRQ0 (8259 + PIT
                // handler installed) so the early guest doesn't take
                // a spurious vector. Claims the single EVENT_INJ slot
                // for this VMRUN → `continue`; EXIT_INTR fires again
                // immediately so net/input lose nothing.
                // D4 live-resize config-change MUST precede the
                // timer-tick block. An idle guest (LibreWolf on
                // about:blank) HLTs between ticks, so its only
                // EXIT_INTR exits are fresh 100 Hz host-timer ticks
                // (host USB is drained ON that timer, not as extra
                // IRQs) — the timer block then `continue`s on EVERY
                // exit and nothing past it ever runs. Rare + one-shot;
                // costs the timer at most one tick (Linux tolerates
                // the jitter) and is the only place an idle guest sees
                // it. See the vmx mirror.
                if is_bsp {
                    let wid = crate::microvm::vm_window();
                    let now_d4 = crate::interrupts::ticks();
                    // Phase 2 of any in-flight D4 cycle: fire reconnect
                    // once the 100 ms disconnect window expired.
                    if sh.pci.virtio_gpu.tick_d4(now_d4) {
                        let vector = sh.pic.vector_for_irq(9);
                        let info: u64 = (vector as u64) | (1u64 << 31);
                        self.vcpu.vmcb.write_u64(vmcb::OFF_EVENT_INJ, info);
                        self.vcpu.consecutive_idle = 0;
                        let _ = wid;
                        continue;
                    }
                    // Phase 1: a fresh dirty from Shade and no cycle
                    // in flight → kick off disconnect.
                    if wid != 0
                        && !sh.pci.virtio_gpu.d4_disconnecting()
                        && crate::shade::surface::display_dirty_peek(wid)
                    {
                        // R2 debounce — one cycle per ~250 ms; flag stays
                        // dirty during a drag so the final size lands.
                        if now_d4.wrapping_sub(sh.last_cfg_tick) >= 25 {
                            let _ = crate::shade::surface::take_display_dirty(wid);
                            sh.last_cfg_tick = now_d4;
                            sh.pci.virtio_gpu.signal_display_change(now_d4);
                            let vector = sh.pic.vector_for_irq(9);
                            let info: u64 = (vector as u64) | (1u64 << 31);
                            self.vcpu.vmcb.write_u64(vmcb::OFF_EVENT_INJ, info);
                            self.vcpu.consecutive_idle = 0;
                            continue;
                        }
                    }
                }
                // Bounded-skip timer floor — see the vmx mirror for
                // the full rationale. A strict priority always starves
                // the loser (timer-first → input dead when idle;
                // input/net-first → TIMER dead under page-load net →
                // RCU stall "timer wakeup didn't happen" → guest hang
                // → libwayland 4096 overflow → cage rc=139). The timer
                // is sacred but only needs ~100 Hz: if overdue by
                // ≥ TIMER_MAX_SKIP host ticks it FORCES the slot
                // (≤ ~30 ms jitter, far under any RCU threshold);
                // otherwise input > net, then the normal timer.
                const TIMER_MAX_SKIP: u64 = 3; // host ticks (~30 ms)
                let now = crate::interrupts::ticks();
                // Two ~100 Hz timer sources may be live, and BOTH must tick
                // during the LAPIC-timer verification window: apic.c:922
                // arms the LVTT periodic and checks that jiffies — driven by
                // the 8259 PIT — advance 1:1 with LVTT interrupts; if the PIT
                // froze, deltaj=0 → "APIC timer disabled due to verification
                // failure" → Linux drops the LAPIC timer (fatal for AP
                // bringup, which has no PIT IRQ0).
                //   * PIT (IRQ0): jiffies during boot + verification. Gated
                //     on `pit_enabled`, which goes false when Linux writes
                //     PIT mode-0 (port 0x43 ← 0x30, clockevent_i8253_disable)
                //     to shut the PIT down after adopting the LAPIC timer —
                //     so IRQ0 stops then and jiffies don't double-count.
                //   * LAPIC timer (LVTT vector): once Linux enables it.
                // One EVENTINJ slot per VMRUN → inject whichever is due; the
                // other lands next VMRUN (≥1 kHz). Both paced by the same
                // 100 Hz `ticks()` so their rates match (the 1:1 the
                // verification needs). Linux's wall-clock is TSC-based.
                let pit_live = is_bsp && sh.pic.irq_unmasked(0) && sh.pit_enabled;
                let lapic_vec = self.vcpu.lapic.timer_tick_vector();
                // Forced slot (anti-starvation under page-load net storm —
                // see vmx mirror): if either is overdue ≥ TIMER_MAX_SKIP host
                // ticks, force it (≤ ~30 ms jitter, far under any RCU stall).
                if pit_live && now.wrapping_sub(sh.last_timer_tick) >= TIMER_MAX_SKIP {
                    sh.last_timer_tick = now;
                    let info: u64 = (sh.pic.vector_for_irq(0) as u64) | (1u64 << 31);
                    self.vcpu.vmcb.write_u64(vmcb::OFF_EVENT_INJ, info);
                    self.vcpu.consecutive_idle = 0;
                    continue;
                }
                if let Some(vec) = lapic_vec {
                    if now.wrapping_sub(self.vcpu.last_lapic_tick) >= TIMER_MAX_SKIP {
                        self.vcpu.last_lapic_tick = now;
                        let info: u64 = (vec as u64) | (1u64 << 31);
                        self.vcpu.vmcb.write_u64(vmcb::OFF_EVENT_INJ, info);
                        self.vcpu.consecutive_idle = 0;
                        continue;
                    }
                }
                // Real net data this pass (drives idle accounting below).
                // BSP-only: the AP never pumps the NAT (stays false).
                let mut pumped = false;
                if is_bsp {
                    pumped = crate::microvm::devices::nat::pump(
                        &mut sh.pci.virtio_net, &sh.guest_mem);
                    if sh.pci.virtio_input.drain_injected(&sh.guest_mem) {
                        let vector = sh.pic.vector_for_irq(12);
                        let info: u64 = (vector as u64) | (1u64 << 31);
                        self.vcpu.vmcb.write_u64(vmcb::OFF_EVENT_INJ, info);
                        self.vcpu.consecutive_idle = 0;
                        continue;
                    }
                    if pumped {
                        let vector = sh.pic.vector_for_irq(10);
                        let info: u64 = (vector as u64) | (1u64 << 31);
                        self.vcpu.vmcb.write_u64(vmcb::OFF_EVENT_INJ, info);
                        self.vcpu.consecutive_idle = 0;
                        continue;
                    }
                }
                // Normal slot: PIT tick, then LAPIC-timer tick (one per
                // 100 Hz `ticks()` window; the other lands next VMRUN).
                if pit_live && now != sh.last_timer_tick {
                    sh.last_timer_tick = now;
                    let info: u64 = (sh.pic.vector_for_irq(0) as u64) | (1u64 << 31);
                    self.vcpu.vmcb.write_u64(vmcb::OFF_EVENT_INJ, info);
                    self.vcpu.consecutive_idle = 0;
                    continue;
                }
                if let Some(vec) = lapic_vec {
                    if now != self.vcpu.last_lapic_tick {
                        self.vcpu.last_lapic_tick = now;
                        let info: u64 = (vec as u64) | (1u64 << 31);
                        self.vcpu.vmcb.write_u64(vmcb::OFF_EVENT_INJ, info);
                        self.vcpu.consecutive_idle = 0;
                        continue;
                    }
                }
                // Real net data (`pumped`) means not idle. Merely having
                // open NAT sessions does NOT — a browser keeps connections
                // alive while sitting idle, and counting that as "busy"
                // pinned consecutive_idle at 0 so a windowed app never
                // yielded Idle → the dedicated core span at 100% (hlt=72k/s
                // observed). For a windowed VM, ignore open-but-quiet
                // sessions and let it idle (net is still pumped every
                // iteration / host wake-up, ≤1 ms latency). Headless test
                // guests keep the old behavior.
                if pumped
                    || (crate::microvm::devices::nat::active_session_count() > 0
                        && crate::microvm::vm_window() == 0)
                {
                    self.vcpu.consecutive_idle = 0;
                } else {
                    self.vcpu.consecutive_idle = self.vcpu.consecutive_idle.saturating_add(1);
                }
                // Idle-auto-exit only for an UNWINDOWED VM. A
                // window-bound VM is an app — idle is normal, never
                // kill it; it ends on real guest exit or window close
                // (VM_CLOSE_REQUESTED in vm_poll_slice). See the vmx
                // mirror for the full rationale.
                if self.vcpu.consecutive_idle >= IDLE_THRESHOLD
                    && crate::microvm::vm_window() == 0
                {
                    sh.serial.flush();
                    kprintln!(
                        "[svm] guest idle in userspace ({} consecutive INTRs after {} iters) — exiting cleanly",
                        self.vcpu.consecutive_idle, self.vcpu.iter,
                    );
                    last_outcome = Some(outcome);
                    break;
                }
                // Guest idle → yield, signalling Idle so the dedicated
                // core host-idles before re-entering (no VMRUN spin) and
                // Core 0 returns to the shell. Guest stays alive.
                // BUT do NOT deep-park while network data is in flight: the
                // ~2 ms park quantizes RX delivery, which kills a page-load's
                // many small request→response round-trips (latency, not
                // throughput — a single bulk stream is unaffected). Spin-pump
                // while recently active; park once the link is quiet (power).
                // Only the BSP services the network (device IRQs are BSP-only),
                // so ONLY the BSP stays spin-pumping while data is in flight —
                // the AP vCPUs have no network work and must park normally, or
                // every vCPU spins at 100% during a download (observed: all 5
                // cores pegged, useless burn).
                if self.vcpu.consecutive_idle >= IDLE_YIELD
                    && !(is_bsp && crate::microvm::devices::nat::recently_active(now))
                {
                    return Ok(SliceOutcome::Idle);
                }
                last_outcome = Some(outcome);
            }
            EXIT_CPUID => {
                let leaf = self.vcpu.vmcb.read_u64(vmcb::OFF_SAVE_RAX) as u32;
                let subleaf = self.vcpu.regs.rcx as u32;
                let (eax, mut ebx, mut ecx, mut edx);
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
                        // ECX bit 24: TSC-deadline timer. Hide it so Linux
                        // uses the classic APIC_TMICT/TMCCT periodic timer
                        // (which svm::lapic emulates) instead of the
                        // TSC-deadline MSR (0x6E0), which we don't emulate.
                        ecx &= !(1u32 << 24);
                        // EBX bits 31:24: initial (x)APIC ID. Pass-through
                        // would report the HOST core's ID (e.g. 2 on worker
                        // core 2) — mismatching our emulated LAPIC + MP-table
                        // (FW_BUG "APIC ID mismatch"). Report this vCPU's id.
                        ebx = (ebx & 0x00FF_FFFF) | ((self.vcpu.apic_id as u32) << 24);
                    }
                    // Leaf 0xB/0x1F (extended topology): EDX is the 32-bit
                    // x2APIC ID, which modern Linux uses as the CPU's APIC
                    // ID. Same reason as leaf 1 EBX — report this vCPU's id,
                    // not the host core's. EAX/EBX/ECX (level shifts/counts)
                    // pass through (host topology shape; the AP's distinct
                    // id places it on its own core — refined in Stage 3b).
                    if leaf == 0x0B || leaf == 0x1F {
                        edx = self.vcpu.apic_id as u32;
                    }
                    if leaf == 7 && subleaf == 0 {
                        // Hide CET — host has CR4.CET=1 for IBT but
                        // Linux's hand-asm stubs lack ENDBR64, so once
                        // CET is on in the guest, indirect calls #CP
                        // and BUG().
                        ecx &= !(1u32 << 7);   // CET_SS
                        edx &= !(1u32 << 20);  // CET_IBT
                        // Hide PKU — CPUID 0xD includes PKRU (+8 byte)
                        // in xsave state size; if Linux supports PK it
                        // expects to find it, mismatched calc → WARN
                        // + xsave-disable + fpstate_reset NULL-deref.
                        // Simpler to hide PK from the guest entirely.
                        ecx &= !(1u32 << 3);   // PKU
                        ecx &= !(1u32 << 4);   // OSPKE
                    }
                }
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
                handle_linux_io(&mut *self.vcpu.vmcb, &mut sh.serial, &mut sh.pci, &mut sh.pic, &mut self.vcpu.regs, port, dir_in, size, &mut self.vcpu.io_dropped, &mut sh.pit_enabled);
                advance_rip(&mut *self.vcpu.vmcb);
                last_outcome = Some(outcome);
            }
            EXIT_MSR => {
                // EXITINFO1 bit 0: 0=RDMSR, 1=WRMSR
                let is_write = outcome.exit_qualification & 1 != 0;
                let msr = self.vcpu.regs.rcx as u32;
                // IA32_APIC_BASE (0x1B): report the LAPIC enabled at its
                // architectural default base so Linux finds + uses it
                // (guest-SMP Stage 1) WITH the BSP bit set (Stage 2: this
                // is now intercepted via the MSRPM — see the 0x1B intercept
                // bit in the VM-open path — and the BSP bit is what keeps
                // Linux's topology code from capping the guest at 1 CPU).
                // Writes (enable / relocate) are accepted but we keep the
                // fixed base — the NPT trap page is wired at 0xFEE00000.
                // Stage 3 (APs): return BSP set only for the BSP vCPU
                // (apic_id 0); APs must read it clear.
                if msr == 0x1B {
                    // RDMSR: report enabled LAPIC at default base. WRMSR:
                    // accept silently (keep fixed base). Single trailing
                    // advance_rip below handles both.
                    if !is_write {
                        // BSP bit (8) only for the boot vCPU; APs read it
                        // clear or Linux's topology mistakes them for a 2nd
                        // BSP. `APIC_BASE_MSR_VALUE` has it set.
                        let mut v = lapic::APIC_BASE_MSR_VALUE;
                        if self.vcpu.apic_id != 0 {
                            v &= !(1u64 << 8);
                        }
                        self.vcpu.regs.rdx = v >> 32;
                        self.vcpu.vmcb.write_u64(vmcb::OFF_SAVE_RAX, v & 0xFFFF_FFFF);
                    }
                } else if is_write {
                    if self.vcpu.msr_log_count < MSR_LOG_CAP {
                        let val = (self.vcpu.regs.rdx << 32)
                            | (self.vcpu.vmcb.read_u64(vmcb::OFF_SAVE_RAX) & 0xFFFF_FFFF);
                        kprintln!("[svm] WRMSR {:#010x} = {:#018x} (absorbed)", msr, val);
                        self.vcpu.msr_log_count += 1;
                    }
                } else {
                    if !msr_is_known_noise(msr) && self.vcpu.msr_log_count < MSR_LOG_CAP {
                        kprintln!("[svm] RDMSR {:#010x} → 0 (unhandled)", msr);
                        self.vcpu.msr_log_count += 1;
                    }
                    self.vcpu.regs.rdx = 0;
                    self.vcpu.vmcb.write_u64(vmcb::OFF_SAVE_RAX, 0);
                }
                advance_rip(&mut *self.vcpu.vmcb);
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
                if sh.pci.virtio_blk.bar0_in_range(gpa) {
                    if handle_mmio_npf_blk(&mut *self.vcpu.vmcb, &mut self.vcpu.regs, &mut sh.pci.virtio_blk, &sh.pic, &mut sh.pending_irqs, gpa, &sh.guest_mem) {
                        last_outcome = Some(outcome);
                        continue;
                    }
                } else if sh.pci.virtio_net.bar0_in_range(gpa) {
                    if handle_mmio_npf_net(&mut *self.vcpu.vmcb, &mut self.vcpu.regs, &mut sh.pci.virtio_net, &sh.pic, &mut sh.pending_irqs, gpa, &sh.guest_mem) {
                        last_outcome = Some(outcome);
                        continue;
                    }
                } else if sh.pci.virtio_gpu.bar0_in_range(gpa) {
                    if handle_mmio_npf_gpu(&mut *self.vcpu.vmcb, &mut self.vcpu.regs, &mut sh.pci.virtio_gpu, &sh.pic, &mut sh.pending_irqs, gpa, &sh.guest_mem) {
                        last_outcome = Some(outcome);
                        continue;
                    }
                } else if sh.pci.virtio_input.bar0_in_range(gpa) {
                    if handle_mmio_npf_input(&mut *self.vcpu.vmcb, &mut self.vcpu.regs, &mut sh.pci.virtio_input, &sh.pic, &mut sh.pending_irqs, gpa, &sh.guest_mem) {
                        last_outcome = Some(outcome);
                        continue;
                    }
                } else if sh.pci.virtio_9p.bar0_in_range(gpa) {
                    if handle_mmio_npf_p9(&mut *self.vcpu.vmcb, &mut self.vcpu.regs, &mut sh.pci.virtio_9p, &sh.pic, &mut sh.pending_irqs, gpa, &sh.guest_mem) {
                        last_outcome = Some(outcome);
                        continue;
                    }
                } else if sh.pci.virtio_blk_sqfs.bar0_in_range(gpa) {
                    // Same handler — VirtioBlk carries its own IRQ line.
                    if handle_mmio_npf_blk(&mut *self.vcpu.vmcb, &mut self.vcpu.regs, &mut sh.pci.virtio_blk_sqfs, &sh.pic, &mut sh.pending_irqs, gpa, &sh.guest_mem) {
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
}

/// Advance guest RIP by the decoded instruction length. Used after
/// emulating an #NPF MMIO access (NRIP_SAVE is unreliable there).
fn advance_rip_by_length(vmcb: &mut vmcb::Vmcb, length: u8) {
    let rip = vmcb.read_u64(vmcb::OFF_SAVE_RIP);
    vmcb.write_u64(vmcb::OFF_SAVE_RIP, rip.wrapping_add(length as u64));
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
    pci: &mut crate::microvm::devices::PciBus,
    pic: &mut crate::microvm::devices::pic8259::Pic8259,
    _regs: &mut vmcb::GuestRegs,
    port: u16,
    dir_in: bool,
    size: u8,
    io_dropped: &mut u32,
    pit_enabled: &mut bool,
) {
    use crate::microvm::devices::{handle_pci_io, PCI_CONFIG_ADDR, PCI_CONFIG_DATA_END, PCI_CONFIG_DATA_START};
    use crate::microvm::devices::pic8259::{handle_pic_io, PIC_MASTER_CMD, PIC_MASTER_IMR, PIC_SLAVE_CMD, PIC_SLAVE_IMR};

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

    // 8259 PIC stub — see microvm::devices::pic8259.
    if matches!(port, PIC_MASTER_CMD | PIC_MASTER_IMR | PIC_SLAVE_CMD | PIC_SLAVE_IMR) {
        if let Some(v) = handle_pic_io(pic, port, dir_in, val_out as u8) {
            let new_rax = (rax & !mask) | (v & mask);
            vmcb.write_u64(vmcb::OFF_SAVE_RAX, new_rax);
        }
        return;
    }

    let in_value: Option<u64> = match (port, dir_in) {
        // i8253 PIT mode/command (0x43) write. Track whether channel 0 is
        // programmed to generate the timer tick: a real mode program
        // (access mode bits 5:4 != 0) on channel 0 (bits 7:6 == 0) sets
        // pit_enabled = (operating mode != 0). Linux shuts the PIT down with
        // mode 0 (0x30, clockevent_i8253_disable) after adopting the LAPIC
        // timer; periodic (0x34, mode 2) / oneshot (0x38, mode 4) keep it
        // live. Latch commands (bits 5:4 == 0) don't change the mode.
        (0x43, false) => {
            let v = val_out as u8;
            if (v >> 6) & 0x3 == 0 && (v >> 4) & 0x3 != 0 {
                *pit_enabled = ((v >> 1) & 0x7) != 0;
            }
            None
        }
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
/// and defer otherwise (see `deliver_irq` + the run-loop pending drain).
fn guest_interruptible(vmcb: &vmcb::Vmcb) -> bool {
    let rflags = vmcb.read_u64(vmcb::OFF_SAVE_RFLAGS);
    let int_state = vmcb.read_u64(vmcb::OFF_INT_STATE);
    (rflags & (1 << 9)) != 0 && (int_state & 1) == 0
}

/// Deliver an external IRQ `line` (vector `vector`) to the guest: inject now
/// via EVENTINJ if interruptible, else set its bit in `pending` so the run
/// loop injects it once the guest re-enables interrupts. See
/// `guest_interruptible` for why the gate matters on SMP.
fn deliver_irq(vmcb: &mut vmcb::Vmcb, pending: &mut u16, line: u8, vector: u8) {
    if guest_interruptible(vmcb) {
        vmcb.write_u64(vmcb::OFF_EVENT_INJ, (vector as u64) | (1u64 << 31));
    } else {
        *pending |= 1u16 << (line as u16 & 0xF);
    }
}

/// #NPF on the LAPIC MMIO page (guest-SMP Stage 1). Decode the faulting
/// MOV, service it against the per-vCPU `LocalApic`, advance RIP. xAPIC
/// registers are 32-bit; no device IRQ-kick (unlike the virtio handlers).
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
    let buf = match fetch_inst(rip, cr3, &sh.guest_mem) {
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
            route_ipi(sh, apic_id, &icr);
        }
    } else {
        let value = apic.read(off) as u64;
        write_guest_gpr(regs, vmcb, rax, dec.reg, dec.width, value);
    }
    advance_rip_by_length(vmcb, dec.length);
    true
}

/// Route a decoded ICR write (guest SMP). FIXED/LOWEST → mark the vector
/// pending on the target vCPU(s); STARTUP → ask the orchestration layer to
/// spawn the AP at the SIPI vector (once); INIT and other modes → no-op
/// (the AP starts from its SIPI; we don't model NMI/SMI cross-vCPU here).
fn route_ipi(sh: &mut VmShared, sender: u8, icr: &lapic::IcrWrite) {
    match icr.delivery_mode {
        lapic::ICR_DM_STARTUP => {
            crate::microvm::cpu::request_ap_spawn(icr.dest, icr.vector);
        }
        lapic::ICR_DM_FIXED | lapic::ICR_DM_LOWEST => {
            let n = crate::microvm::cpu::guest_vcpus();
            match icr.shorthand {
                0 => sh.ipi_set(icr.dest, icr.vector), // physical dest
                1 => sh.ipi_set(sender, icr.vector),   // self
                2 => {
                    for t in 0..n {
                        sh.ipi_set(t, icr.vector);
                    }
                } // all incl self
                _ => {
                    for t in 0..n {
                        if t != sender {
                            sh.ipi_set(t, icr.vector);
                        }
                    }
                } // all excl self
            }
        }
        _ => {}
    }
}

fn handle_mmio_npf_blk(
    vmcb: &mut vmcb::Vmcb,
    regs: &mut vmcb::GuestRegs,
    blk: &mut crate::microvm::devices::virtio_blk_pci::VirtioBlk,
    pic: &crate::microvm::devices::pic8259::Pic8259,
    pending: &mut u16,
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
            deliver_irq(vmcb, pending, blk.irq_line(), pic.vector_for_irq(blk.irq_line()));
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
    net: &mut crate::microvm::devices::virtio_net_pci::VirtioNet,
    pic: &crate::microvm::devices::pic8259::Pic8259,
    pending: &mut u16,
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

    let off = (gpa - net.bar0_base()) as u32;
    let rax = vmcb.read_u64(vmcb::OFF_SAVE_RAX);

    if dec.is_write {
        let value = read_guest_gpr(regs, rax, dec.reg) & width_mask(dec.width);
        net.mmio_write(off, dec.width, value);
    } else {
        let value = net.mmio_read(off, dec.width);
        write_guest_gpr(regs, vmcb, rax, dec.reg, dec.width, value);
    }

    if let Some(qidx) = net.take_pending_kick() {
        let advanced = net.service_queues(qidx, mem);
        if advanced {
            deliver_irq(vmcb, pending, 10, pic.vector_for_irq(10));
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
    pic: &crate::microvm::devices::pic8259::Pic8259,
    pending: &mut u16,
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
        let advanced = gpu.service_queues(qidx, mem);
        if advanced {
            // virtio-gpu IRQ line = 9.
            deliver_irq(vmcb, pending, 9, pic.vector_for_irq(9));
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
    pic: &crate::microvm::devices::pic8259::Pic8259,
    pending: &mut u16,
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
            deliver_irq(vmcb, pending, 12, pic.vector_for_irq(12));
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
    pic: &crate::microvm::devices::pic8259::Pic8259,
    pending: &mut u16,
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
            deliver_irq(vmcb, pending, p9.irq_line(), pic.vector_for_irq(p9.irq_line()));
        }
    }

    advance_rip_by_length(vmcb, dec.length);
    true
}
