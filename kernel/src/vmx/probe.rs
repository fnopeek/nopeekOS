//! VMX capability probe.
//!
//! Does NOT enable VMX, does NOT touch CR4. Pure read-side detection.
//! Bring-up (CR4.VMXE, IA32_FEATURE_CONTROL, VMXON) lives in 12.1.0b.
//!
//! Reference: Intel SDM Vol. 3C §23.6 (Discovering Support for VMX),
//! §A.1 (Basic VMX Information).

/// VMX capability snapshot returned by `probe()` when the CPU supports
/// virtualization. All fields come from architectural MSRs and are
/// stable across the boot.
#[derive(Debug, Clone, Copy)]
pub struct Capabilities {
    /// VMCS revision identifier (IA32_VMX_BASIC[30:0]). Must be the
    /// first dword of every VMXON region and every VMCS this CPU
    /// loads.
    pub revision_id: u32,
    /// Required size of VMXON / VMCS regions in bytes. Always ≤ 4096
    /// per SDM §A.1, but explicit because allocators must honour it.
    pub vmxon_region_size: u32,
    /// IA32_VMX_EPT_VPID_CAP MSR is readable (i.e. secondary
    /// processor-based controls expose EPT). Required for Phase 12.1.1+.
    pub ept_supported: bool,
    /// Unrestricted-guest secondary control available — lets the guest
    /// run in real mode without trampolining through paged 32-bit.
    /// Phase 12.1.1+ Linux boot benefits from this.
    pub unrestricted_guest: bool,
    /// VPID (tagged TLB) available. Reduces TLB flush cost on
    /// VM-entry/exit. Required for sane Phase 12.6 multi-VM density.
    pub vpid: bool,
}

/// Probe the running CPU for VMX support. Returns `None` if VMX is
/// either absent or fused off in firmware. Side-effect-free.
pub fn probe() -> Option<Capabilities> {
    if !cpuid_vmx_bit() {
        return None;
    }

    // IA32_FEATURE_CONTROL gates VMXON. Bit 2 (VMX outside SMX) must
    // be set AND bit 0 (lock) decides whether we can still toggle it.
    // We only *read* here — actual enable in 12.1.0b after this probe
    // returns Ok.
    let feat_ctrl = unsafe { rdmsr(IA32_FEATURE_CONTROL) };
    let locked = feat_ctrl & FEAT_CTRL_LOCK != 0;
    let vmx_outside_smx = feat_ctrl & FEAT_CTRL_VMX_OUTSIDE_SMX != 0;
    if locked && !vmx_outside_smx {
        // Firmware locked us out of VMX — N100 BIOS occasionally
        // ships this disabled. Surface it cleanly.
        return None;
    }

    let basic = unsafe { rdmsr(IA32_VMX_BASIC) };
    let revision_id = (basic & 0x7FFF_FFFF) as u32;
    let region_size = ((basic >> 32) & 0x1FFF) as u32;

    let (ept_supported, unrestricted_guest, vpid) = secondary_caps();

    Some(Capabilities {
        revision_id,
        vmxon_region_size: region_size,
        ept_supported,
        unrestricted_guest,
        vpid,
    })
}

// ── private helpers ────────────────────────────────────────────────

const IA32_FEATURE_CONTROL: u32 = 0x3A;
const IA32_VMX_BASIC: u32 = 0x480;
const IA32_VMX_PROCBASED_CTLS: u32 = 0x482;
const IA32_VMX_PROCBASED_CTLS2: u32 = 0x48B;

const FEAT_CTRL_LOCK: u64 = 1 << 0;
const FEAT_CTRL_VMX_OUTSIDE_SMX: u64 = 1 << 2;

/// CPUID.1:ECX[5] — VMX present.
fn cpuid_vmx_bit() -> bool {
    let ecx: u32;
    // SAFETY: CPUID is a non-privileged unprivileged-side-effect-free
    // instruction. `eax` is clobbered for the leaf 0x1 input, ebx/edx
    // are unused outputs we don't bind.
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 1u32 => _,
            out("ecx") ecx,
            out("edx") _,
            options(nostack, preserves_flags),
        );
    }
    ecx & (1 << 5) != 0
}

/// Read MSR — undefined-instruction if MSR is unsupported, but every
/// MSR we touch here is architectural since Nehalem. Caller must
/// guard with `cpuid_vmx_bit()` before reading any IA32_VMX_* MSR.
unsafe fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    // SAFETY: caller guarantees MSR is implemented on this CPU.
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

/// Decode the secondary-controls-allowed bitmap to surface the three
/// MicroVM-relevant feature flags. SDM §A.3.3 specifies the layout:
/// each capability MSR has its allowed-1 bits in the upper dword.
fn secondary_caps() -> (bool, bool, bool) {
    // Step 1: confirm secondary controls themselves are exposed.
    // IA32_VMX_PROCBASED_CTLS[63] = "activate secondary controls"
    // allowed-1 (bit 63 of the 64-bit MSR = bit 31 of the upper dword).
    let prim = unsafe { rdmsr(IA32_VMX_PROCBASED_CTLS) };
    let secondary_allowed = (prim >> 63) & 1 != 0;
    if !secondary_allowed {
        return (false, false, false);
    }

    // Step 2: read secondary capabilities. Allowed-1 bits in upper 32.
    let sec = unsafe { rdmsr(IA32_VMX_PROCBASED_CTLS2) };
    let allowed1 = (sec >> 32) as u32;

    let ept = allowed1 & (1 << 1) != 0;
    let unrestricted = allowed1 & (1 << 7) != 0;
    let vpid = allowed1 & (1 << 5) != 0;
    (ept, unrestricted, vpid)
}
