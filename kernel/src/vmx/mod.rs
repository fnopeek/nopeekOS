//! VMX (Intel VT-x) — Phase 12 MicroVM substrate.
//!
//! Layered as `kernel-side primitives only` per
//! `MICROKERNEL_REFACTOR.md` and `PHASE12_MICROVM.md`:
//! kernel owns VMX/VMCS/EPT/VT-d/VCPU-threads, WASM-Manager owns
//! lifecycle + bridges.
//!
//! As of v0.100.0 the boot path no longer enters VMX root mode —
//! `init()` only probes capabilities. The VMXON/VMCS/EPT/VMLAUNCH
//! pipeline is exercised on demand via the `microvm` shell-intent
//! (`run_substrate_test`), which matches the eventual per-app
//! lifecycle: `microvm <appname>` will spawn a per-app VM, not a
//! single boot-time VM.
//!
//! Phase 12.1 milestones:
//!   12.1.0a   probe + report                          ✓ v0.90.0
//!   12.1.0b   VMXON region + CR4.VMXE + round-trip    ✓ v0.91.0
//!   12.1.0c   VMCS region + VMCLEAR + VMPTRLD         ✓ v0.92.0
//!   12.1.0d-1 Host-state VMWRITE/VMREAD + trampoline  ✓ v0.93.0
//!   12.1.0d-2a TSS install (HOST_TR_SELECTOR ≠ 0)     ✓ v0.94.0
//!   12.1.0d-2b Guest-state + controls + VMLAUNCH      ✓ v0.95.0…0.96.0
//!   12.1.1a   EPT identity-map (1 GB)                 ✓ v0.97.0
//!   12.1.1b   Real-mode unrestricted guest + I/O exit ✓ v0.98.0
//!   12.1.1c-1 Non-identity 16 MB EPT window           ✓ v0.99.0…0.99.1
//!   12.1.1c-2 VMX bring-up off the boot path          ← this file
//!   12.1.1c-3 Alpine bzImage loader + microvm linux
//!   12.1.1d   Early-panic detection (I/O-bitmap)
//!   12.1.2    virtio-console backend
//!   12.1.3    initramfs + Rust-PID-1 + bash
//!   12.1.4    inject_console round-trip

pub mod bzimage;
mod enable;
mod ept;
mod probe;
mod vmcs;

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
    report();
}

/// Print VMX capability snapshot. Used by `init()` once at boot and
/// the `vmx` shell-intent on demand.
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

/// Run the real-mode I/O-loop substrate test on demand. Allocates a
/// fresh VMXON region, VMCS, EPT, 64 MB guest RAM, I/O bitmaps (all
/// leaked — not suitable for repeated invocation; persistent state
/// lands in 12.2). Returns the full VM-exit outcome (basic reason +
/// qualification + guest RAX), or an error string. Refuses if VT-x
/// wasn't available at probe time.
pub fn run_substrate_test() -> Result<vmcs::LaunchOutcome, &'static str> {
    match *PROBE.lock() {
        ProbeState::Available(_) => enable::enable_and_test(),
        ProbeState::Unavailable(reason) => Err(reason),
        ProbeState::NotProbed => Err("vmx::init() not called yet"),
    }
}

pub use vmcs::{decode_io_exit_qualification, LaunchOutcome};

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
