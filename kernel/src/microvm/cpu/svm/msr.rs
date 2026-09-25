//! Guest MSR policy, SVM half — KVM's `svm_recalc_msr_intercepts` and
//! `svm_get_msr`/`svm_set_msr`. Vendor-neutral MSRs live in `cpu::guest_msr`.
//!
//! Every MSR is intercepted. Pass-through is limited to what the CPU switches
//! for us (VMLOAD/VMSAVE state), TSC_AUX (the host never reads it) and the
//! write-only barrier PRED_CMD. With an all-zero MSRPM a guest could rewrite
//! the host's MTRRs, TSC, SYSCFG or microcode loader.

use super::vmcb;
use crate::microvm::cpu::guest_msr::{self as common, *};

pub use common::{GuestMsrs, spec_ctrl_enter, spec_ctrl_exit};

const MSR_TSC: u32 = 0x10;
const MSR_PATCH_LEVEL: u32 = 0x8B;
const MSR_HWCR: u32 = 0xC001_0015;
const MSR_SYSCFG: u32 = 0xC001_0010;
const MSR_NB_CFG: u32 = 0xC001_001F;
const MSR_CPUID_7_FEATURES: u32 = 0xC001_1002;
/// Host-owned chicken bit (see `cpu_errata`); the guest's write stays here.
const MSR_ZEN2_SPECTRAL_CHICKEN: u32 = 0xC001_10E3;
/// Legacy K7 perf counters (EVNTSEL0-3, PERFCTR0-3) and the core extension.
const MSR_K7_PERF_FIRST: u32 = 0xC001_0000;
const MSR_K7_PERF_LAST: u32 = 0xC001_0007;
const MSR_F15H_PERF_FIRST: u32 = 0xC001_0200;
const MSR_F15H_PERF_LAST: u32 = 0xC001_020B;
const MSR_DE_CFG: u32 = 0xC001_1029;

const EFER_SCE: u64 = 1 << 0;
const EFER_LME: u64 = 1 << 8;
const EFER_LMA: u64 = 1 << 10;
const EFER_NXE: u64 = 1 << 11;
const EFER_SVME: u64 = 1 << 12;
const EFER_FFXSR: u64 = 1 << 14;
const EFER_TCE: u64 = 1 << 15;
const EFER_AUTOIBRS: u64 = 1 << 21;

/// Fill an 8 KB MSRPM: intercept everything, then open the pass-through set.
///
/// # Safety
/// `msrpm_phys` must be an exclusively owned, identity-mapped 8 KB region.
pub unsafe fn init_msrpm(msrpm_phys: u64) {
    // SAFETY: caller guarantees ownership of the 8 KB region.
    unsafe { core::ptr::write_bytes(msrpm_phys as *mut u8, 0xFF, 2 * 4096); }
    // Saved/loaded by VMSAVE/VMLOAD around every VMRUN (run_guest_once).
    for msr in [
        MSR_STAR, MSR_LSTAR, MSR_CSTAR, MSR_SFMASK,
        MSR_FS_BASE, MSR_GS_BASE, MSR_KERNEL_GS_BASE,
        MSR_SYSENTER_CS, MSR_SYSENTER_ESP, MSR_SYSENTER_EIP,
        // Only RDTSCP/RDPID read it; the host kernel uses neither.
        MSR_TSC_AUX,
    ] {
        // SAFETY: as above.
        unsafe { set_intercept(msrpm_phys, msr, false, false); }
    }
    if host_has_ibpb() {
        // SAFETY: as above. Write-only barrier; reads stay intercepted (#GP).
        unsafe { set_intercept(msrpm_phys, MSR_PRED_CMD, true, false); }
    }
}

/// APM Vol 2 §15.11: three 2 KB ranges, two bits per MSR (read, write).
unsafe fn set_intercept(msrpm_phys: u64, msr: u32, read: bool, write: bool) {
    let (base, idx) = match msr {
        0x0000_0000..=0x0000_1FFF => (0x000u64, msr),
        0xC000_0000..=0xC000_1FFF => (0x800, msr - 0xC000_0000),
        0xC001_0000..=0xC001_1FFF => (0x1000, msr - 0xC001_0000),
        _ => return, // outside the map: always intercepted
    };
    let bit = idx as u64 * 2;
    let byte = (msrpm_phys + base + bit / 8) as *mut u8;
    let shift = (bit % 8) as u8;
    // SAFETY: byte lies inside the caller-owned 8 KB MSRPM.
    unsafe {
        let mut v = *byte;
        v = (v & !(1 << shift)) | ((read as u8) << shift);
        v = (v & !(1 << (shift + 1))) | ((write as u8) << (shift + 1));
        *byte = v;
    }
}

fn cpuid(leaf: u32) -> (u32, u32, u32, u32) { super::cpuid(leaf, 0) }

fn host_ext_8(bit: u32) -> bool {
    if cpuid(0x8000_0000).0 < 0x8000_0008 { return false; }
    cpuid(0x8000_0008).1 & (1 << bit) != 0
}

fn host_has_ibpb() -> bool { host_ext_8(12) }

/// SPEC_CTRL bits the host CPU implements (KVM `kvm_spec_ctrl_valid_bits`).
fn spec_ctrl_valid_bits() -> u64 {
    let mut bits = 0;
    if host_ext_8(14) { bits |= 1 << 0; } // IBRS
    if host_ext_8(15) { bits |= 1 << 1; } // STIBP
    if host_ext_8(24) { bits |= 1 << 2; } // SSBD
    if host_ext_8(28) { bits |= 1 << 7; } // PSFD
    bits
}

fn efer_allowed() -> u64 {
    let mut bits = EFER_SCE | EFER_LME | EFER_LMA | EFER_NXE;
    let e1 = cpuid(0x8000_0001).3;
    if e1 & (1 << 25) != 0 { bits |= EFER_FFXSR; }
    if cpuid(0x8000_0001).2 & (1 << 17) != 0 { bits |= EFER_TCE; }
    if cpuid(0x8000_0000).0 >= 0x8000_0021 && cpuid(0x8000_0021).0 & (1 << 8) != 0 {
        bits |= EFER_AUTOIBRS;
    }
    bits
}

/// Emulated RDMSR. `apic_id` decides the APIC_BASE BSP bit.
pub fn read(st: &GuestMsrs, vmcb: &vmcb::Vmcb, apic_id: u8, msr: u32) -> MsrResult<u64> {
    Ok(match msr {
        MSR_TSC => {
            crate::interrupts::rdtsc().wrapping_add(vmcb.read_u64(vmcb::OFF_TSC_OFFSET))
        }
        MSR_PATCH_LEVEL => host_rdmsr(MSR_PATCH_LEVEL),
        MSR_PAT => vmcb.read_u64(vmcb::OFF_SAVE_G_PAT),
        // Guest view: SVME is ours, not the guest's (KVM `svm_set_efer`).
        MSR_EFER => vmcb.read_u64(vmcb::OFF_SAVE_EFER) & !EFER_SVME,
        MSR_HWCR => st.hwcr,
        // KVM answers these with 0: no SME/SEV, no NB config, no PMU (enable_pmu=0).
        MSR_SYSCFG | MSR_NB_CFG | MSR_CPUID_7_FEATURES | MSR_ZEN2_SPECTRAL_CHICKEN => 0,
        MSR_K7_PERF_FIRST..=MSR_K7_PERF_LAST | MSR_F15H_PERF_FIRST..=MSR_F15H_PERF_LAST => 0,
        // Feature MSR: only the LFENCE-serialising bit (KVM `kvm_get_feature_msr`).
        MSR_DE_CFG => host_rdmsr(MSR_DE_CFG) & (1 << 1),
        _ => return common::read(
            st, msr, apic_id, crate::microvm::cpu::GUEST_LAPIC, spec_ctrl_valid_bits(),
        ),
    })
}

/// Emulated WRMSR.
pub fn write(st: &mut GuestMsrs, vmcb: &mut vmcb::Vmcb, msr: u32, val: u64) -> MsrResult<()> {
    match msr {
        // The guest does not own the TSC offset; Linux never writes it.
        MSR_TSC => {}
        MSR_PATCH_LEVEL => {}
        MSR_PAT => {
            if !pat_valid(val) { return Err(()); }
            vmcb.write_u64(vmcb::OFF_SAVE_G_PAT, val);
        }
        MSR_EFER => {
            if val & !efer_allowed() != 0 { return Err(()); }
            // LMA is the CPU's to set; SVME must stay on for VMRUN (APM §15.5.1).
            let cur = vmcb.read_u64(vmcb::OFF_SAVE_EFER);
            vmcb.write_u64(vmcb::OFF_SAVE_EFER, (val & !EFER_LMA) | (cur & EFER_LMA) | EFER_SVME);
        }
        MSR_HWCR => st.hwcr = val | HWCR_TSC_FREQ_SEL,
        MSR_SYSCFG | MSR_NB_CFG | MSR_CPUID_7_FEATURES | MSR_ZEN2_SPECTRAL_CHICKEN => {}
        MSR_K7_PERF_FIRST..=MSR_K7_PERF_LAST | MSR_F15H_PERF_FIRST..=MSR_F15H_PERF_LAST => {}
        MSR_DE_CFG => {}
        _ => return common::write(st, msr, val, spec_ctrl_valid_bits()),
    }
    Ok(())
}
