//! VMX (Intel VT-x) bring-up — Phase 12 MicroVM substrate.
//!
//! Layered as `kernel-side primitives only` per
//! `MICROKERNEL_REFACTOR.md` and `PHASE12_MICROVM.md`:
//! kernel owns VMX/VMCS/EPT/VT-d/VCPU-threads, WASM-Manager owns
//! lifecycle + bridges.
//!
//! Phase 12.1 milestones:
//!   12.1.0a   probe + report                          ✓ v0.90.0
//!   12.1.0b   VMXON region + CR4.VMXE + round-trip    ✓ v0.91.0
//!   12.1.0c   VMCS region + VMCLEAR + VMPTRLD         ✓ v0.92.0
//!   12.1.0d-1 Host-state VMWRITE/VMREAD + trampoline  ✓ v0.93.0
//!   12.1.0d-2a TSS install (HOST_TR_SELECTOR ≠ 0)     ✓ v0.94.0
//!   12.1.0d-2b Guest-state + controls + VMLAUNCH      ✓ v0.95.0…0.96.0
//!   12.1.1a   EPT identity-map (1 GB)                 ✓ v0.97.0
//!   12.1.1b   Real-mode unrestricted guest + I/O exit ← this file
//!   12.1.1c-d Linux 6.18 LTS bzImage to early-panic
//!   12.1.2    virtio-console backend
//!   12.1.3    initramfs + Rust-PID-1 + bash
//!   12.1.4    inject_console round-trip

mod enable;
mod ept;
mod probe;
mod vmcs;

use probe::probe;
use spin::Mutex;

/// Outcome of the boot-time VMX bring-up. Read-only after init().
#[derive(Debug, Clone, Copy)]
pub enum BringupState {
    NotRun,
    Skipped(&'static str),
    /// VMLAUNCH succeeded; payload is the basic VM-exit reason
    /// (expected: 12 = HLT-exit) per SDM Appendix C.
    Launched(u16),
    Failed(&'static str),
}

static BRINGUP: Mutex<BringupState> = Mutex::new(BringupState::NotRun);

/// One-shot bring-up: probe, enter+exit VMX root mode once to validate
/// the full path (CR4.VMXE + IA32_FEATURE_CONTROL + VMXON/VMXOFF),
/// log result. Subsequent `vmx` shell-intents read the cached state.
pub fn init() {
    let state = match probe() {
        Some(_caps) => match enable::enable_and_test() {
            Ok(reason) => BringupState::Launched(reason),
            Err(reason) => BringupState::Failed(reason),
        },
        None => BringupState::Skipped("VT-x not supported or BIOS-locked"),
    };
    *BRINGUP.lock() = state;
    report();
}

/// Print VMX capability snapshot + last bring-up result. Used by
/// `init()` once at boot and the `vmx` shell-intent on demand.
pub fn report() {
    use crate::kprintln;
    let state = *BRINGUP.lock();
    match probe() {
        Some(c) => {
            kprintln!("[vmx] VT-x supported");
            kprintln!("[vmx]   revision_id     = {:#010x}", c.revision_id);
            kprintln!("[vmx]   vmxon_region_sz = {} bytes", c.vmxon_region_size);
            kprintln!("[vmx]   ept_supported   = {}", c.ept_supported);
            kprintln!("[vmx]   unrestricted    = {}", c.unrestricted_guest);
            kprintln!("[vmx]   vpid            = {}", c.vpid);
            match state {
                BringupState::NotRun => {
                    kprintln!("[vmx]   bring-up        = not run");
                }
                BringupState::Skipped(r) => {
                    kprintln!("[vmx]   bring-up        = skipped ({})", r);
                }
                BringupState::Launched(reason) => {
                    let label = match reason {
                        12 => " (HLT)",
                         30 => " (I/O instruction)",
                        _ => "",
                    };
                    kprintln!(
                        "[vmx]   bring-up        = OK (EPT + real-mode + VMLAUNCH → exit_reason={}{}, 12.1.1b)",
                        reason, label,
                    );
                }
                BringupState::Failed(r) => {
                    kprintln!("[vmx]   bring-up        = FAILED ({})", r);
                }
            }
        }
        None => {
            kprintln!("[vmx] VT-x NOT available — MicroVM disabled this boot");
            kprintln!("[vmx]   check BIOS: 'Intel Virtualization Technology' must be enabled");
        }
    }
}

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
