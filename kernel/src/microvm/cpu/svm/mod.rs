//! SVM (AMD-V) — Phase 12 MicroVM substrate, AMD backend.
//!
//! Status: 12.1.0a-svm — probe + report. Bring-up
//! (EFER.SVME, host-save, VMCB, VMRUN) lands in 12.1.0b-svm.
//!
//! The AMD equivalent of Intel VMX is documented in *AMD64
//! Architecture Programmer's Manual, Volume 2: System Programming*
//! Chapter 15 ("Secure Virtual Machine"). Mapping vs. VMX:
//!
//! | Concept              | Intel VMX        | AMD SVM             |
//! |----------------------|------------------|---------------------|
//! | Enable bit           | CR4.VMXE         | EFER.SVME           |
//! | Per-VM control struct| VMCS (4 KB, opaque, accessed via VMREAD/VMWRITE) | VMCB (4 KB, normal struct, MMIO-style) |
//! | Host save area       | Implicit (VMCS host-state region) | Host-save MSR (VM_HSAVE_PA) |
//! | Enter guest          | VMLAUNCH / VMRESUME | VMRUN |
//! | Exit reason          | 32-bit field, encoded in VMCS | VMCB.EXITCODE u64 |
//! | Nested paging        | EPT (4-level)    | NPT (4-level, same shape, different MSR) |
//! | I/O intercept        | I/O bitmap (2×4 KB) | IOPM (12 KB, ports 0..0xFFFF) |
//! | MSR intercept        | MSR bitmap (4 KB) | MSRPM (8 KB) |
//!
//! Phase 12.1 milestones (mirroring VMX bring-up):
//!   12.1.0a-svm  probe + report                          ← this
//!   12.1.0b-svm  EFER.SVME + host-save + trivial VMRUN
//!   12.1.0c/d-svm  VMCB save-area complete, host VMSAVE/VMLOAD
//!   12.1.1a-svm  NPT identity-map (256 MB)
//!   12.1.1b-svm  Real-mode unrestricted guest + I/O bitmap
//!   12.1.1c-svm  Linux bzImage 32-bit boot protocol entry
//!   12.1.1d-svm  Panic detection (shared SerialState scanner)
//!   12.1.3-svm   initramfs + Rust-PID-1 (init crate already exists)
//!   12.1.4-svm   inject_console echo round-trip

mod enable;
pub mod lapic; // per-vCPU local-APIC emulation (guest-SMP Stage 1)
pub mod npt; // demand_fault_in / boot_window_bytes used by guest_mem (B3)
mod probe;
mod vmcb;

pub use probe::Capabilities;
use probe::probe;
use spin::Mutex;

/// SVM availability as observed at boot. Set by `init()`, never
/// changes afterwards (capabilities are CPUID-fixed).
#[derive(Debug, Clone, Copy)]
pub enum ProbeState {
    NotProbed,
    Available(Capabilities),
    Unavailable(&'static str),
}

static PROBE: Mutex<ProbeState> = Mutex::new(ProbeState::NotProbed);

/// Boot-time probe — no MSR writes, no SVME. Stores the capability
/// snapshot for later `microvm`-intent invocations and prints a one-
/// shot status line.
pub fn init() {
    let state = match probe() {
        Some(c) => ProbeState::Available(c),
        None => ProbeState::Unavailable("AMD-V not supported or BIOS-locked"),
    };
    *PROBE.lock() = state;
    report();
}

pub fn report() {
    use crate::kprintln;
    match *PROBE.lock() {
        ProbeState::Available(c) => {
            kprintln!("[svm] AMD-V available");
            kprintln!("[svm]   revision        = {}", c.revision);
            kprintln!("[svm]   asid_count      = {}", c.asid_count);
            kprintln!("[svm]   nested_paging   = {}", c.nested_paging);
            kprintln!("[svm]   nrip_save       = {}", c.nrip_save);
            kprintln!("[svm]   decode_assists  = {}", c.decode_assists);
            kprintln!("[svm]   vmsave_vmload   = {}", c.vmsave_vmload);
            kprintln!("[svm]   avic            = {}  (nested-AVIC gate: Y ⇒ csd-fix buildable in QEMU)", c.avic);
            kprintln!("[svm]   substrate-test  = run 'microvm test' to exercise (12.1.0b)");
        }
        ProbeState::Unavailable(reason) => {
            kprintln!("[svm] AMD-V NOT available — MicroVM disabled");
            kprintln!("[svm]   {}", reason);
            kprintln!("[svm]   check BIOS: 'SVM Mode' / 'Virtualization' must be enabled");
        }
        ProbeState::NotProbed => {
            use crate::kprintln;
            kprintln!("[svm] probe not run yet");
        }
    }
}

pub fn run_substrate_test() -> Result<super::LaunchOutcome, &'static str> {
    match *PROBE.lock() {
        ProbeState::Available(_) => enable::enable_and_test(),
        ProbeState::Unavailable(reason) => Err(reason),
        ProbeState::NotProbed => Err("svm::init() not called yet"),
    }
}

pub use enable::{SliceOutcome, VmContext};

/// Open a re-entrant VM context (12.4 step 1b). Probe-gated like
/// `run_linux`. The caller drives `run_slice` + `close`.
pub fn vm_open(
    bzimage: &[u8],
    cmdline: &[u8],
    initramfs: Option<&[u8]>,
    inject: &[u8],
) -> Result<VmContext, &'static str> {
    match *PROBE.lock() {
        ProbeState::Available(_) => VmContext::open(bzimage, cmdline, initramfs, inject),
        ProbeState::Unavailable(reason) => Err(reason),
        ProbeState::NotProbed => Err("svm::init() not called yet"),
    }
}

/// Open an AP vCPU context (guest SMP, Stage 3b) sharing the BSP's
/// `VmShared` at `shared_ptr` (a `*mut VmShared` as a u64), entering real
/// mode at `sipi_vector`. The caller drives `run_slice` (NOT `close` — the
/// BSP owns the shared state).
pub fn vm_open_ap(
    shared_ptr: u64,
    sipi_vector: u8,
    apic_id: u8,
) -> Result<VmContext, &'static str> {
    VmContext::open_ap(shared_ptr as *mut enable::VmShared, sipi_vector, apic_id)
}

/// Set/clear the guest-SMP big-VM-lock engagement (see `enable::AP_ACTIVE`).
/// Clearing also resets `AP_ESTABLISHED` so the next VM's bring-up starts
/// with the idle-park optimisation gated off again.
pub fn set_ap_active(on: bool) {
    use core::sync::atomic::Ordering;
    enable::AP_ACTIVE.store(on, Ordering::Release);
    if !on {
        enable::AP_ESTABLISHED.store(false, Ordering::Release);
    }
}

// ── shared CPU primitives for SVM submodules ───────────────────────

/// Read MSR. Caller must guarantee the MSR exists on this CPU,
/// otherwise #GP. Mirrors `vmx::rdmsr` — both backends need the
/// same primitive but vendor isolation keeps each tree self-
/// contained.
pub(super) unsafe fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    // SAFETY: caller-guaranteed MSR validity.
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") msr,
            out("eax") lo,
            out("edx") hi,
            options(nostack, preserves_flags),
        );
    }
    ((hi as u64) << 32) | (lo as u64)
}

/// Write MSR. Same caveat as `rdmsr`. WRMSR can fail with #GP if
/// the value violates reserved bits — caller handles that case.
#[allow(dead_code)] // 12.1.0b will call this for VM_HSAVE_PA + EFER
pub(super) unsafe fn wrmsr(msr: u32, val: u64) {
    let lo = val as u32;
    let hi = (val >> 32) as u32;
    // SAFETY: caller-guaranteed MSR + value validity.
    unsafe {
        core::arch::asm!(
            "wrmsr",
            in("ecx") msr,
            in("eax") lo,
            in("edx") hi,
            options(nostack, preserves_flags),
        );
    }
}

/// CPUID with explicit subleaf. Returns (eax, ebx, ecx, edx).
/// Rust reserves rbx for LLVM internals so we save/restore it
/// manually. CPUID has no privileged side-effects.
pub(super) fn cpuid(leaf: u32, subleaf: u32) -> (u32, u32, u32, u32) {
    let eax: u32;
    let ebx: u32;
    let ecx: u32;
    let edx: u32;
    // SAFETY: CPUID is unprivileged.
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "mov {ebx_save:e}, ebx",
            "pop rbx",
            ebx_save = out(reg) ebx,
            inout("eax") leaf => eax,
            inout("ecx") subleaf => ecx,
            out("edx") edx,
            options(nostack, preserves_flags),
        );
    }
    (eax, ebx, ecx, edx)
}
