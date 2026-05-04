//! SVM (AMD-V) — Phase 12 MicroVM substrate, AMD backend.
//!
//! Status: SKELETON. Probes capabilities and reports them, but
//! `run_substrate_test` and `run_linux` return `Err` until the
//! real backend lands.
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
//! ## Implementation roadmap
//!
//! 1. Probe (CPUID 0x8000_0001 ECX bit 2 = SVM, plus 0x8000_000A
//!    leaves for SVM revision + features incl. NPT bit 0).
//! 2. Enable: set EFER.SVME, allocate host-save area, write its
//!    physical address to VM_HSAVE_PA MSR.
//! 3. VMCB allocation + state-load semantics; VMCB.CONTROL fields
//!    for intercepts, nested paging CR3 (NPT root), IOPM/MSRPM PA.
//! 4. VMRUN loop: save host GPRs, load guest GPRs, `vmrun rax`
//!    (where rax = VMCB phys), on exit save guest GPRs and dispatch
//!    on VMCB.EXITCODE.
//! 5. NPT page-table set-up — same 4-level format as EPT but different
//!    enable bit + CR3 plumbing.
//! 6. Linux loader integration — `microvm::linux::bzimage` is already
//!    platform-agnostic and writes into a host-mapped guest-RAM
//!    window; the SVM backend just needs to provide the host-base
//!    pointer + NPT mapping.
//!
//! All entry points here mirror the Intel `vmx::*` shape so the
//! dispatch in `microvm::cpu::mod.rs` is symmetric.

use crate::kprintln;

/// Boot-time entry. Probes for SVM, prints a one-shot status, but
/// does NOT enable SVM root mode — that happens lazily on the first
/// `microvm` shell-intent invocation, mirroring `vmx::init`.
pub fn init() {
    if probe_svm() {
        kprintln!("[svm] AMD-V detected — backend is a STUB, MicroVM disabled");
        kprintln!("[svm]   real implementation: AMD APM Vol 2 Ch. 15");
    } else {
        kprintln!("[svm] AMD-V not available — MicroVM disabled");
        kprintln!("[svm]   check BIOS: 'SVM Mode' / 'Virtualization' must be enabled");
    }
}

pub fn report() {
    if probe_svm() {
        kprintln!("[svm] AMD-V available (backend not yet implemented)");
    } else {
        kprintln!("[svm] AMD-V NOT available");
    }
}

pub fn run_substrate_test() -> Result<super::LaunchOutcome, &'static str> {
    Err("SVM substrate-test not yet implemented")
}

pub fn run_linux(
    _bzimage: &[u8],
    _cmdline: &[u8],
    _initramfs: Option<&[u8]>,
    _inject: &[u8],
) -> Result<super::LaunchOutcome, &'static str> {
    Err("SVM run_linux not yet implemented")
}

/// CPUID-based SVM detection. CPUID 0x8000_0001 ECX bit 2 = SVM
/// (AMD APM Vol 3, "CPUID Fn8000_0001_ECX"). Mirrors the Intel
/// `vmx::probe` style — pure CPUID, no MSR writes.
fn probe_svm() -> bool {
    let (_, _, ecx, _) = super::vmx::host_cpuid(0x8000_0001, 0);
    (ecx & (1 << 2)) != 0
}
