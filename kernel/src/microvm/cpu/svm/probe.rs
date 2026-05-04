//! SVM capability probe.
//!
//! Does NOT enable SVM, does NOT touch EFER. Pure read-side detection.
//! Bring-up (EFER.SVME, host-save area, VM_HSAVE_PA MSR) lives in
//! 12.1.0b-svm (`enable.rs`).
//!
//! Reference: AMD APM Vol. 2, §15.4 (Enabling SVM) and §15.3
//! (CPUID Function 8000_000Ah).

/// SVM capability snapshot returned by `probe()` when AMD-V is
/// available. All fields come from CPUID + the VM_CR MSR and are
/// stable across the boot.
#[derive(Debug, Clone, Copy)]
pub struct Capabilities {
    /// SVM revision identifier (CPUID 8000_000A EAX[7:0]). Tracks
    /// the ISA generation; rev 1 = original Pacifica, rev 2+ adds
    /// Decode Assists, NP, NRIPS, etc.
    pub revision: u8,
    /// Number of address-space identifiers supported (CPUID
    /// 8000_000A EBX). VM_CB.guest_asid is masked to this count.
    /// Need ≥ 1 (we use ASID 1 for our single guest).
    pub asid_count: u32,
    /// Nested paging available (CPUID 8000_000A EDX[0]). AMD's
    /// equivalent of Intel EPT — required for our 256 MB guest-RAM
    /// window. nopeekOS bails if this is false; supported on every
    /// AMD CPU since Barcelona/K10 (2007).
    pub nested_paging: bool,
    /// Next-RIP save (CPUID 8000_000A EDX[3]). On VM-exit the CPU
    /// stores the address of the instruction *following* the
    /// faulting one in VMCB.NRIP, sparing us from manually decoding
    /// instruction lengths. nopeekOS uses this throughout the
    /// I/O-exit / CPUID-exit handlers.
    pub nrip_save: bool,
    /// VMSave/VMLoad virtualization (CPUID 8000_000A EDX[15]).
    /// Allows the guest to run VMSAVE/VMLOAD without a #VMEXIT —
    /// useful for nested guests but not strictly required.
    pub vmsave_vmload: bool,
    /// Decode Assists (CPUID 8000_000A EDX[7]). When set, the CPU
    /// populates VMCB exit-info fields with decoded operand info,
    /// avoiding instruction-stream re-fetch on string I/O exits.
    pub decode_assists: bool,
}

/// Probe the running CPU for SVM. Returns `None` if SVM is either
/// absent (non-AMD or pre-Pacifica), or fused off in firmware via
/// VM_CR.SVMDIS with VM_CR.SVMLOCK set. Side-effect-free.
pub fn probe() -> Option<Capabilities> {
    if !cpuid_svm_bit() {
        return None;
    }

    // VM_CR (0xC001_0114) — bit 4 SVMDIS, bit 3 SVMLOCK.
    // If SVMDIS=1 and the lock bit (CPUID 8000_000A EDX[2] SVM-Lock)
    // is set, BIOS sealed SVM off. Surface cleanly.
    let vm_cr = unsafe { super::rdmsr(VM_CR) };
    let svmdis = vm_cr & VM_CR_SVMDIS != 0;
    let (_, _, _, edx_a) = super::cpuid(0x8000_000A, 0);
    let svm_lock = edx_a & (1 << 2) != 0;
    if svmdis && svm_lock {
        return None;
    }

    let (eax_a, ebx_a, _, _) = super::cpuid(0x8000_000A, 0);
    let revision = (eax_a & 0xFF) as u8;
    let asid_count = ebx_a;

    Some(Capabilities {
        revision,
        asid_count,
        nested_paging: edx_a & (1 << 0) != 0,
        nrip_save: edx_a & (1 << 3) != 0,
        decode_assists: edx_a & (1 << 7) != 0,
        vmsave_vmload: edx_a & (1 << 15) != 0,
    })
}

// ── private constants ──────────────────────────────────────────────

const VM_CR: u32 = 0xC001_0114;
const VM_CR_SVMDIS: u64 = 1 << 4;

/// CPUID 8000_0001 ECX[2] — SVM present.
fn cpuid_svm_bit() -> bool {
    let (_, _, ecx, _) = super::cpuid(0x8000_0001, 0);
    ecx & (1 << 2) != 0
}
