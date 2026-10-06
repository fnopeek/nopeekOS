//! SVM (AMD-V) — AMD backend of the MicroVM substrate.
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

mod enable;
mod msr; // guest MSR policy: intercept-all + emulation (KVM model)
pub mod lapic; // per-vCPU local-APIC emulation
pub mod npt; // demand_fault_in / boot_window_bytes used by guest_mem
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
    summary();
}

/// One boot line; `report` has the details.
fn summary() {
    use crate::kprintln;
    match *PROBE.lock() {
        ProbeState::Available(c) => kprintln!("[svm] AMD-V available{}",
            if c.nested_paging { " (nested paging)" } else { "" }),
        ProbeState::Unavailable(reason) => kprintln!("[svm] AMD-V not available: {}", reason),
        ProbeState::NotProbed => {}
    }
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
            kprintln!("[svm]   avic            = {}", c.avic);
            kprintln!("[svm]   substrate-test  = run 'microvm test' to exercise");
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

/// Open a re-entrant VM context. Probe-gated like `run_linux`. The caller
/// drives `run_slice` + `close`.
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

/// Open an AP vCPU context (guest SMP) joining the VM the BSP published
/// (`VmContext::publish_for_aps`), entering real mode at `sipi_vector`. The
/// caller drives `run_slice` (not `close` — the BSP owns the shared state).
pub fn vm_open_ap(sipi_vector: u8, apic_id: u8) -> Result<VmContext, &'static str> {
    VmContext::open_ap(sipi_vector, apic_id)
}

pub use enable::clear_ap_shared;

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
#[allow(dead_code)] // used by enable.rs for VM_HSAVE_PA + EFER
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
pub(super) fn cpuid(leaf: u32, subleaf: u32) -> (u32, u32, u32, u32) {
    let r = core::arch::x86_64::__cpuid_count(leaf, subleaf);
    (r.eax, r.ebx, r.ecx, r.edx)
}
