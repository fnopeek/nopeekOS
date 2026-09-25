//! Guest MSR policy — the SVM mirror of KVM's `svm_recalc_msr_intercepts`,
//! `svm_get_msr`/`svm_set_msr` and `kvm_mtrr_*`.
//!
//! Every MSR is intercepted. Pass-through is limited to what the CPU switches
//! for us (VMLOAD/VMSAVE state), TSC_AUX (the host never reads it) and the
//! write-only barrier PRED_CMD. Everything else is emulated here, and nothing
//! the guest writes reaches a host MSR — with an all-zero MSRPM a guest could
//! rewrite the host's MTRRs, TSC, SYSCFG or microcode loader.

use super::vmcb;

/// Result of an emulated access: `Err(())` = inject #GP.
pub type MsrResult<T> = Result<T, ()>;

const MSR_TSC: u32 = 0x10;
const MSR_APIC_BASE: u32 = 0x1B;
const MSR_TSC_ADJUST: u32 = 0x3B;
const MSR_SPEC_CTRL: u32 = 0x48;
const MSR_PRED_CMD: u32 = 0x49;
const MSR_PATCH_LEVEL: u32 = 0x8B;
const MSR_MTRR_CAP: u32 = 0xFE;
const MSR_SYSENTER_CS: u32 = 0x174;
const MSR_SYSENTER_ESP: u32 = 0x175;
const MSR_SYSENTER_EIP: u32 = 0x176;
const MSR_MCG_CAP: u32 = 0x179;
const MSR_MCG_STATUS: u32 = 0x17A;
const MSR_MCG_CTL: u32 = 0x17B;
const MSR_MTRR_PHYS_FIRST: u32 = 0x200;
const MSR_MTRR_PHYS_LAST: u32 = 0x20F;
const MSR_PAT: u32 = 0x277;
const MSR_MTRR_DEF_TYPE: u32 = 0x2FF;
const MSR_MC_FIRST: u32 = 0x400;
const MSR_MC_LAST: u32 = 0x47F;
const MSR_XSS: u32 = 0xDA0;
const MSR_EFER: u32 = 0xC000_0080;
const MSR_STAR: u32 = 0xC000_0081;
const MSR_LSTAR: u32 = 0xC000_0082;
const MSR_CSTAR: u32 = 0xC000_0083;
const MSR_SFMASK: u32 = 0xC000_0084;
const MSR_FS_BASE: u32 = 0xC000_0100;
const MSR_GS_BASE: u32 = 0xC000_0101;
const MSR_KERNEL_GS_BASE: u32 = 0xC000_0102;
const MSR_TSC_AUX: u32 = 0xC000_0103;
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
/// HWCR.TscFreqSel: the TSC counts at P0 — true on every CPU with invariant TSC.
const HWCR_TSC_FREQ_SEL: u64 = 1 << 24;
const MSR_DE_CFG: u32 = 0xC001_1029;

/// Fixed-range MTRRs, in the order KVM's `fixed_msr_to_seg_unit` uses.
const MTRR_FIXED: [u32; 11] = [
    0x250, 0x258, 0x259, 0x268, 0x269, 0x26A, 0x26B, 0x26C, 0x26D, 0x26E, 0x26F,
];

const EFER_SCE: u64 = 1 << 0;
const EFER_LME: u64 = 1 << 8;
const EFER_LMA: u64 = 1 << 10;
const EFER_NXE: u64 = 1 << 11;
const EFER_SVME: u64 = 1 << 12;
const EFER_FFXSR: u64 = 1 << 14;
const EFER_TCE: u64 = 1 << 15;
const EFER_AUTOIBRS: u64 = 1 << 21;

/// MTRRcap: 8 variable ranges, fixed ranges, WC — KVM's `KVM_NR_VAR_MTRR | 0x500`.
const MTRR_CAP_VALUE: u64 = 0x508;
/// No firmware runs in the guest, so start where a BIOS leaves it: MTRRs on,
/// default type WB, no ranges. The guest's MTRRs never reach hardware — with
/// NPT the memory type comes from the host tables and the guest PAT.
const MTRR_DEF_TYPE_RESET: u64 = 0x806;

/// Per-vCPU emulated MSR state.
pub struct GuestMsrs {
    pub spec_ctrl: u64,
    hwcr: u64,
    mtrr_def_type: u64,
    mtrr_var: [u64; 16],
    mtrr_fixed: [u64; 11],
    mcg_status: u64,
    tsc_adjust: u64,
}

impl GuestMsrs {
    pub const fn new() -> Self {
        Self {
            spec_ctrl: 0,
            hwcr: HWCR_TSC_FREQ_SEL,
            mtrr_def_type: MTRR_DEF_TYPE_RESET,
            mtrr_var: [0; 16],
            mtrr_fixed: [0; 11],
            mcg_status: 0,
            tsc_adjust: 0,
        }
    }
}

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

fn host_rdmsr(msr: u32) -> u64 {
    let (lo, hi): (u32, u32);
    // SAFETY: only called for architectural MSRs present on every AMD CPU
    // that supports SVM (PATCH_LEVEL, DE_CFG).
    unsafe {
        core::arch::asm!("rdmsr", in("ecx") msr, out("eax") lo, out("edx") hi,
                         options(nomem, nostack, preserves_flags));
    }
    ((hi as u64) << 32) | lo as u64
}

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

fn valid_mem_type(t: u8) -> bool { matches!(t, 0 | 1 | 4 | 5 | 6) }

fn pat_valid(v: u64) -> bool {
    (0..8).all(|i| matches!((v >> (i * 8)) as u8, 0 | 1 | 4 | 5 | 6 | 7))
}

/// Emulated RDMSR. `apic_id` decides the APIC_BASE BSP bit.
pub fn read(st: &GuestMsrs, vmcb: &vmcb::Vmcb, apic_id: u8, msr: u32) -> MsrResult<u64> {
    Ok(match msr {
        MSR_TSC => {
            crate::interrupts::rdtsc().wrapping_add(vmcb.read_u64(vmcb::OFF_TSC_OFFSET))
        }
        MSR_APIC_BASE => {
            // BSP bit (8) only for the boot vCPU — Linux's topology code
            // otherwise mistakes the guest for a kdump kernel and caps it at 1 CPU.
            let mut v = super::lapic::APIC_BASE_MSR_VALUE;
            if apic_id != 0 { v &= !(1u64 << 8); }
            if !crate::microvm::cpu::GUEST_LAPIC { v &= !(1u64 << 11); }
            v
        }
        MSR_SPEC_CTRL => {
            if spec_ctrl_valid_bits() == 0 { return Err(()); }
            st.spec_ctrl
        }
        MSR_PRED_CMD => return Err(()),
        MSR_PATCH_LEVEL => host_rdmsr(MSR_PATCH_LEVEL),
        MSR_MTRR_CAP => MTRR_CAP_VALUE,
        MSR_MTRR_DEF_TYPE => st.mtrr_def_type,
        MSR_MTRR_PHYS_FIRST..=MSR_MTRR_PHYS_LAST => {
            st.mtrr_var[(msr - MSR_MTRR_PHYS_FIRST) as usize]
        }
        m if MTRR_FIXED.contains(&m) => {
            st.mtrr_fixed[MTRR_FIXED.iter().position(|&x| x == m).unwrap_or(0)]
        }
        MSR_PAT => vmcb.read_u64(vmcb::OFF_SAVE_G_PAT),
        // No machine-check banks (KVM with mcg_cap = 0).
        MSR_MCG_CAP => 0,
        MSR_MCG_STATUS => st.mcg_status,
        MSR_MCG_CTL => return Err(()),
        MSR_MC_FIRST..=MSR_MC_LAST => return Err(()),
        MSR_XSS => 0,
        // Guest view: SVME is ours, not the guest's (KVM `svm_set_efer`).
        MSR_EFER => vmcb.read_u64(vmcb::OFF_SAVE_EFER) & !EFER_SVME,
        MSR_HWCR => st.hwcr,
        MSR_TSC_ADJUST => st.tsc_adjust,
        // KVM answers these with 0: no SME/SEV, no NB config, no PMU (enable_pmu=0).
        MSR_SYSCFG | MSR_NB_CFG | MSR_CPUID_7_FEATURES | MSR_ZEN2_SPECTRAL_CHICKEN => 0,
        MSR_K7_PERF_FIRST..=MSR_K7_PERF_LAST | MSR_F15H_PERF_FIRST..=MSR_F15H_PERF_LAST => 0,
        // Feature MSR: only the LFENCE-serialising bit (KVM `kvm_get_feature_msr`).
        MSR_DE_CFG => host_rdmsr(MSR_DE_CFG) & (1 << 1),
        // Unknown: read as zero, like KVM with `ignore_msrs`. Nothing is
        // forwarded to the host.
        _ => return Ok(unknown(msr, None)),
    })
}

/// Emulated WRMSR.
pub fn write(st: &mut GuestMsrs, vmcb: &mut vmcb::Vmcb, msr: u32, val: u64) -> MsrResult<()> {
    match msr {
        // The guest does not own the TSC offset; Linux never writes it.
        MSR_TSC => {}
        MSR_APIC_BASE => {} // fixed base — the NPT trap page sits at 0xFEE00000
        MSR_SPEC_CTRL => {
            if val & !spec_ctrl_valid_bits() != 0 { return Err(()); }
            st.spec_ctrl = val;
        }
        MSR_PATCH_LEVEL => {}
        MSR_MTRR_CAP => return Err(()),
        MSR_MTRR_DEF_TYPE => {
            if val & !0xCFF != 0 || !valid_mem_type(val as u8) { return Err(()); }
            st.mtrr_def_type = val;
        }
        MSR_MTRR_PHYS_FIRST..=MSR_MTRR_PHYS_LAST => {
            let i = (msr - MSR_MTRR_PHYS_FIRST) as usize;
            if i % 2 == 0 && !valid_mem_type(val as u8) { return Err(()); }
            st.mtrr_var[i] = val;
        }
        m if MTRR_FIXED.contains(&m) => {
            if (0..8).any(|b| !valid_mem_type((val >> (b * 8)) as u8)) { return Err(()); }
            st.mtrr_fixed[MTRR_FIXED.iter().position(|&x| x == m).unwrap_or(0)] = val;
        }
        MSR_PAT => {
            if !pat_valid(val) { return Err(()); }
            vmcb.write_u64(vmcb::OFF_SAVE_G_PAT, val);
        }
        MSR_MCG_STATUS => st.mcg_status = val,
        MSR_MCG_CAP | MSR_MCG_CTL => return Err(()),
        MSR_MC_FIRST..=MSR_MC_LAST => return Err(()),
        MSR_XSS => if val != 0 { return Err(()); },
        MSR_EFER => {
            if val & !efer_allowed() != 0 { return Err(()); }
            // LMA is the CPU's to set; SVME must stay on for VMRUN (APM §15.5.1).
            let cur = vmcb.read_u64(vmcb::OFF_SAVE_EFER);
            vmcb.write_u64(vmcb::OFF_SAVE_EFER, (val & !EFER_LMA) | (cur & EFER_LMA) | EFER_SVME);
        }
        MSR_HWCR => st.hwcr = val | HWCR_TSC_FREQ_SEL,
        // Stored only; the guest's TSC offset stays ours.
        MSR_TSC_ADJUST => st.tsc_adjust = val,
        MSR_SYSCFG | MSR_NB_CFG | MSR_CPUID_7_FEATURES | MSR_ZEN2_SPECTRAL_CHICKEN => {}
        MSR_K7_PERF_FIRST..=MSR_K7_PERF_LAST | MSR_F15H_PERF_FIRST..=MSR_F15H_PERF_LAST => {}
        MSR_DE_CFG => {}
        _ => { unknown(msr, Some(val)); }
    }
    Ok(())
}

/// Capped log of MSRs nobody emulates yet — the list to work through.
fn unknown(msr: u32, write: Option<u64>) -> u64 {
    use core::sync::atomic::{AtomicU32, Ordering};
    static LOGGED: AtomicU32 = AtomicU32::new(0);
    if LOGGED.fetch_add(1, Ordering::Relaxed) < 32 {
        match write {
            Some(v) => crate::kprintln!("[svm] WRMSR {:#010x} = {:#018x} (ignored)", msr, v),
            None => crate::kprintln!("[svm] RDMSR {:#010x} -> 0 (not emulated)", msr),
        }
    }
    0
}

/// Load the guest's SPEC_CTRL before VMRUN; returns the host value to restore.
/// KVM without V_SPEC_CTRL does the same swap (`x86_spec_ctrl_set_guest`).
pub fn spec_ctrl_enter(guest: u64) -> Option<u64> {
    if guest == 0 { return None; }
    let host = host_rdmsr(MSR_SPEC_CTRL);
    if host == guest { return None; }
    // SAFETY: guest was validated against the host's SPEC_CTRL bits in `write`.
    unsafe { wrmsr(MSR_SPEC_CTRL, guest); }
    Some(host)
}

pub fn spec_ctrl_exit(host: Option<u64>) {
    if let Some(h) = host {
        // SAFETY: restoring the value read from this very MSR before VMRUN.
        unsafe { wrmsr(MSR_SPEC_CTRL, h); }
    }
}

unsafe fn wrmsr(msr: u32, v: u64) {
    // SAFETY: caller guarantees msr/v are valid on this CPU.
    unsafe {
        core::arch::asm!("wrmsr", in("ecx") msr, in("eax") v as u32, in("edx") (v >> 32) as u32,
                         options(nomem, nostack, preserves_flags));
    }
}
