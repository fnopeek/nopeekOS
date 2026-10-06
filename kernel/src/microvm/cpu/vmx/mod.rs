//! VMX (Intel VT-x) backend of the MicroVM substrate.
//!
//! Kernel-side primitives only: the kernel owns VMX/VMCS/EPT/vCPU
//! threads, the WASM manager owns lifecycle and bridges.
//!
//! The boot path does not enter VMX root mode; `init()` only probes
//! capabilities. VMXON/VMCS/EPT/VMLAUNCH run on demand via the `microvm`
//! shell intent.

mod enable;
pub mod ept; // demand_fault_in / boot_window_bytes used by guest_mem
mod probe;
mod vmcs;
mod msr; // guest MSR policy: intercept-all + emulation (KVM model)

pub use probe::Capabilities;
use probe::probe;
use spin::Mutex;

/// VMX availability as observed at boot. Set by `init()`, never
/// changes afterwards (capabilities are CPUID-fixed).
#[derive(Debug, Clone, Copy)]
pub enum ProbeState {
    NotProbed,
    Available(Capabilities),
    Unavailable(&'static str),
}

static PROBE: Mutex<ProbeState> = Mutex::new(ProbeState::NotProbed);

/// Boot-time probe — no MSR writes, no VMXON. Stores the capability
/// snapshot for later `microvm`-intent invocations and prints a one-
/// shot status line.
pub fn init() {
    let state = match probe() {
        Some(c) => ProbeState::Available(c),
        None => ProbeState::Unavailable("VT-x not supported or BIOS-locked"),
    };
    *PROBE.lock() = state;
    summary();
}

/// One boot line; `report` has the details.
fn summary() {
    use crate::kprintln;
    match *PROBE.lock() {
        ProbeState::Available(c) => kprintln!("[vmx] VT-x available{}",
            if c.ept_supported { " (EPT)" } else { "" }),
        ProbeState::Unavailable(reason) => kprintln!("[vmx] VT-x not available: {}", reason),
        ProbeState::NotProbed => {}
    }
}

/// Print the VMX capability snapshot (`vmx` intent).
pub fn report() {
    use crate::kprintln;
    match *PROBE.lock() {
        ProbeState::Available(c) => {
            kprintln!("[vmx] VT-x available");
            kprintln!("[vmx]   revision_id     = {:#010x}", c.revision_id);
            kprintln!("[vmx]   vmxon_region_sz = {} bytes", c.vmxon_region_size);
            kprintln!("[vmx]   ept_supported   = {}", c.ept_supported);
            kprintln!("[vmx]   unrestricted    = {}", c.unrestricted_guest);
            kprintln!("[vmx]   vpid            = {}", c.vpid);
            kprintln!("[vmx]   substrate-test  = run 'microvm test' to exercise");
        }
        ProbeState::Unavailable(reason) => {
            kprintln!("[vmx] VT-x NOT available — MicroVM disabled");
            kprintln!("[vmx]   {}", reason);
            kprintln!("[vmx]   check BIOS: 'Intel Virtualization Technology' must be enabled");
        }
        ProbeState::NotProbed => {
            kprintln!("[vmx] probe not run yet");
        }
    }
}

/// Run the substrate test (32-bit prot mode HLT-loop OK-stub).
/// Allocates a fresh VMXON region, VMCS, EPT, 64 MB guest RAM,
/// I/O bitmaps (all leaked). Returns the final VM-exit outcome.
pub fn run_substrate_test() -> Result<vmcs::LaunchOutcome, &'static str> {
    match *PROBE.lock() {
        ProbeState::Available(_) => enable::enable_and_test(),
        ProbeState::Unavailable(reason) => Err(reason),
        ProbeState::NotProbed => Err("vmx::init() not called yet"),
    }
}

pub use enable::{SliceOutcome, VmContext};

/// Open a re-entrant VM context. Probe-gated like `run_linux`. The
/// caller drives `run_slice` + `close`.
pub fn vm_open(
    bzimage: &[u8],
    cmdline: &[u8],
    initramfs: Option<&[u8]>,
    inject: &[u8],
) -> Result<VmContext, &'static str> {
    match *PROBE.lock() {
        ProbeState::Available(_) => VmContext::open(bzimage, cmdline, initramfs, inject),
        ProbeState::Unavailable(reason) => Err(reason),
        ProbeState::NotProbed => Err("vmx::init() not called yet"),
    }
}

/// Open an AP vCPU context (guest SMP) joining the VM the BSP published
/// (`VmContext::publish_for_aps`), entering real mode at `sipi_vector`.
/// Enters VMX root on this (the AP's) worker core. The caller drives
/// `run_slice` then `close_ap` (not `close`; the BSP owns the shared state).
pub fn vm_open_ap(sipi_vector: u8, apic_id: u8) -> Result<VmContext, &'static str> {
    VmContext::open_ap(sipi_vector, apic_id)
}

pub use enable::clear_ap_shared;

pub use vmcs::{decode_io_exit_qualification, host_cpuid, LaunchOutcome};

// ── shared CPU primitives for submodules ───────────────────────────

/// Read MSR. Caller must guarantee the MSR exists on this CPU,
/// otherwise #GP. All MSRs we touch are architectural since Nehalem
/// or VMX-gated by `probe()`.
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

/// Write MSR. Same caveat as `rdmsr`. WRMSR can also fail with #GP if
/// the value violates reserved bits — caller handles that case.
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
