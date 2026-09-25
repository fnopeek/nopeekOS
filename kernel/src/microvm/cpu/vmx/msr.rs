//! Guest MSR policy, VMX half — KVM's `vmx_possible_passthrough_msrs` and
//! `vmx_get_msr`/`vmx_set_msr`. Vendor-neutral MSRs live in `cpu::guest_msr`.
//!
//! Every MSR is intercepted except those the CPU switches through VMCS fields
//! (FS/GS base, SYSENTER, EFER), the SYSCALL set and TSC_AUX (the host kernel
//! uses neither SYSCALL/SWAPGS nor RDTSCP/RDPID, so the guest's values are the
//! only ones that matter on its core), and the write-only barriers. An all-zero
//! bitmap let the guest write IA32_S_CET (the host's IBT), the x2APIC ICR
//! (host IPIs) and PAT (host memory types).

use crate::microvm::cpu::guest_msr::{self as common, *};

pub use common::{GuestMsrs, spec_ctrl_enter, spec_ctrl_exit};

const MSR_TSC: u32 = 0x10;
const MSR_FEATURE_CONTROL: u32 = 0x3A;
const MSR_BIOS_SIGN_ID: u32 = 0x8B;
const MSR_PLATFORM_INFO: u32 = 0xCE;
const MSR_UMWAIT_CONTROL: u32 = 0xE1;
const MSR_ARCH_CAPABILITIES: u32 = 0x10A;
const MSR_FLUSH_CMD: u32 = 0x10B;
const MSR_TSX_CTRL: u32 = 0x122;
const MSR_MCU_OPT_CTRL: u32 = 0x123;
const MSR_MISC_ENABLE: u32 = 0x1A0;
const MSR_DEBUGCTL: u32 = 0x1D9;
const MSR_PERF_CAPABILITIES: u32 = 0x345;
const MSR_TSC_DEADLINE: u32 = 0x6E0;
/// CET: U_CET, S_CET, PL0-3_SSP, INT_SSP_TAB. S_CET carries the host's IBT.
const MSR_CET_FIRST: u32 = 0x6A0;
const MSR_CET_LAST: u32 = 0x6A8;
const MSR_X2APIC_FIRST: u32 = 0x800;
const MSR_X2APIC_LAST: u32 = 0x8FF;

/// Architectural PMU: PMCs, event selects, fixed counters, global control.
fn is_pmu(msr: u32) -> bool {
    matches!(msr, 0xC1..=0xC8 | 0x186..=0x18F | 0x309..=0x30B | 0x38D..=0x390 | 0x4C1..=0x4C8)
}

/// ARCH_CAPABILITIES bits describing the hardware that a guest may see
/// (KVM `KVM_SUPPORTED_ARCH_CAP` minus TSX_CTRL, whose MSR is not emulated).
const ARCH_CAP_MASK: u64 = (1 << 0) | (1 << 1) | (1 << 2) | (1 << 4) | (1 << 5) | (1 << 6)
    | (1 << 8) | (1 << 13) | (1 << 14) | (1 << 15) | (1 << 17) | (1 << 19) | (1 << 20)
    | (1 << 24) | (1 << 26) | (1 << 27) | (1 << 28);

fn cpuid(leaf: u32, sub: u32) -> (u32, u32, u32, u32) {
    let r = core::arch::x86_64::__cpuid_count(leaf, sub);
    (r.eax, r.ebx, r.ecx, r.edx)
}

fn l7_edx(bit: u32) -> bool { cpuid(7, 0).3 & (1 << bit) != 0 }

fn l7_2_edx(bit: u32) -> bool {
    cpuid(7, 0).0 >= 2 && cpuid(7, 2).3 & (1 << bit) != 0
}

/// SPEC_CTRL bits the host CPU implements (KVM `kvm_spec_ctrl_valid_bits`).
fn spec_ctrl_valid_bits() -> u64 {
    let mut bits = 0;
    if l7_edx(26) { bits |= 1 << 0; }                  // IBRS
    if l7_edx(27) { bits |= 1 << 1; }                  // STIBP
    if l7_edx(31) { bits |= 1 << 2; }                  // SSBD
    if l7_2_edx(0) { bits |= 1 << 7; }                 // PSFD
    if l7_2_edx(1) { bits |= (1 << 3) | (1 << 4); }    // IPRED_DIS_U/S
    if l7_2_edx(2) { bits |= (1 << 5) | (1 << 6); }    // RRSBA_DIS_U/S
    if l7_2_edx(4) { bits |= 1 << 10; }                // BHI_DIS_S
    bits
}

/// Fill the 4 KB MSR bitmap: intercept everything, then open the pass-through set.
///
/// # Safety
/// `bitmap_phys` must be an exclusively owned, identity-mapped 4 KB frame.
pub unsafe fn init_msr_bitmap(bitmap_phys: u64) {
    // SAFETY: caller guarantees ownership of the 4 KB frame.
    unsafe { core::ptr::write_bytes(bitmap_phys as *mut u8, 0xFF, 4096); }
    for msr in [
        // Switched by the CPU through VMCS guest/host fields.
        MSR_FS_BASE, MSR_GS_BASE, MSR_SYSENTER_CS, MSR_SYSENTER_ESP, MSR_SYSENTER_EIP,
        MSR_EFER,
        // Not switched, and not used by the host (see module doc).
        MSR_KERNEL_GS_BASE, MSR_STAR, MSR_LSTAR, MSR_CSTAR, MSR_SFMASK, MSR_TSC_AUX,
    ] {
        // SAFETY: as above.
        unsafe { set_intercept(bitmap_phys, msr, false, false); }
    }
    // Write-only barriers; reads stay intercepted (#GP).
    if l7_edx(26) {
        // SAFETY: as above.
        unsafe { set_intercept(bitmap_phys, MSR_PRED_CMD, true, false); }
    }
    if l7_edx(28) {
        // SAFETY: as above.
        unsafe { set_intercept(bitmap_phys, MSR_FLUSH_CMD, true, false); }
    }
}

/// SDM §25.6.9: read-low @0x000, read-high @0x400, write-low @0x800,
/// write-high @0xC00; one bit per MSR.
unsafe fn set_intercept(bitmap_phys: u64, msr: u32, read: bool, write: bool) {
    let (rd, wr, idx) = match msr {
        0x0000_0000..=0x0000_1FFF => (0x000u64, 0x800u64, msr),
        0xC000_0000..=0xC000_1FFF => (0x400, 0xC00, msr - 0xC000_0000),
        _ => return, // outside the map: always intercepted
    };
    let byte = idx as u64 / 8;
    let bit = (idx % 8) as u8;
    // SAFETY: both bytes lie inside the caller-owned 4 KB bitmap.
    unsafe {
        let r = (bitmap_phys + rd + byte) as *mut u8;
        let w = (bitmap_phys + wr + byte) as *mut u8;
        *r = (*r & !(1 << bit)) | ((read as u8) << bit);
        *w = (*w & !(1 << bit)) | ((write as u8) << bit);
    }
}

/// Emulated RDMSR.
pub fn read(st: &GuestMsrs, apic_id: u8, lapic_on: bool, msr: u32) -> MsrResult<u64> {
    Ok(match msr {
        MSR_TSC => crate::interrupts::rdtsc(),
        MSR_PAT => super::vmcs::read_guest_pat().map_err(|_| ())?,
        // Locked, VMX off — the guest has no VMX (KVM without nested).
        MSR_FEATURE_CONTROL => 1,
        MSR_BIOS_SIGN_ID => host_rdmsr(MSR_BIOS_SIGN_ID),
        MSR_PLATFORM_INFO => 0,
        MSR_MISC_ENABLE => st.misc_enable,
        MSR_ARCH_CAPABILITIES => {
            if !l7_edx(29) { return Err(()); }
            host_rdmsr(MSR_ARCH_CAPABILITIES) & ARCH_CAP_MASK
        }
        MSR_TSX_CTRL | MSR_MCU_OPT_CTRL | MSR_DEBUGCTL | MSR_PERF_CAPABILITIES
        | MSR_UMWAIT_CONTROL | MSR_TSC_DEADLINE => 0,
        m if is_pmu(m) => 0,
        // CET and x2APIC are hidden in CPUID; real hardware faults here too.
        MSR_CET_FIRST..=MSR_CET_LAST | MSR_X2APIC_FIRST..=MSR_X2APIC_LAST => return Err(()),
        _ => return common::read(st, msr, apic_id, lapic_on, spec_ctrl_valid_bits()),
    })
}

/// Emulated WRMSR.
pub fn write(st: &mut GuestMsrs, msr: u32, val: u64) -> MsrResult<()> {
    match msr {
        MSR_TSC => {}
        MSR_PAT => {
            if !pat_valid(val) { return Err(()); }
            super::vmcs::write_guest_pat(val).map_err(|_| ())?;
        }
        MSR_FEATURE_CONTROL => return Err(()), // locked
        MSR_BIOS_SIGN_ID => {}
        MSR_MISC_ENABLE => st.misc_enable = val,
        MSR_ARCH_CAPABILITIES | MSR_PLATFORM_INFO => return Err(()),
        MSR_TSX_CTRL | MSR_MCU_OPT_CTRL | MSR_DEBUGCTL | MSR_PERF_CAPABILITIES
        | MSR_UMWAIT_CONTROL | MSR_TSC_DEADLINE => {}
        m if is_pmu(m) => {}
        MSR_CET_FIRST..=MSR_CET_LAST | MSR_X2APIC_FIRST..=MSR_X2APIC_LAST => return Err(()),
        _ => return common::write(st, msr, val, spec_ctrl_valid_bits()),
    }
    Ok(())
}
