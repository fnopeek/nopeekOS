//! Vendor-neutral half of guest MSR emulation — KVM `kvm_get_msr_common` /
//! `kvm_set_msr_common` and `kvm_mtrr_*`. `svm::msr` and `vmx::msr` handle
//! their vendor's MSRs and fall through to `read`/`write` here.
//!
//! Every MSR a guest reaches through here is intercepted. Nothing it writes
//! lands in a host MSR: unknown MSRs read 0 and swallow writes (KVM with
//! `ignore_msrs`), so the host keeps its MTRRs, TSC, CET and APIC state.

/// Result of an emulated access: `Err(())` = inject #GP.
pub type MsrResult<T> = Result<T, ()>;

pub const MSR_APIC_BASE: u32 = 0x1B;
pub const MSR_TSC_ADJUST: u32 = 0x3B;
pub const MSR_SPEC_CTRL: u32 = 0x48;
pub const MSR_PRED_CMD: u32 = 0x49;
pub const MSR_MTRR_CAP: u32 = 0xFE;
pub const MSR_SYSENTER_CS: u32 = 0x174;
pub const MSR_SYSENTER_ESP: u32 = 0x175;
pub const MSR_SYSENTER_EIP: u32 = 0x176;
const MSR_MCG_CAP: u32 = 0x179;
const MSR_MCG_STATUS: u32 = 0x17A;
const MSR_MCG_CTL: u32 = 0x17B;
const MSR_MTRR_PHYS_FIRST: u32 = 0x200;
const MSR_MTRR_PHYS_LAST: u32 = 0x20F;
pub const MSR_PAT: u32 = 0x277;
const MSR_MTRR_DEF_TYPE: u32 = 0x2FF;
const MSR_MC_FIRST: u32 = 0x400;
const MSR_MC_LAST: u32 = 0x47F;
const MSR_XSS: u32 = 0xDA0;
pub const MSR_EFER: u32 = 0xC000_0080;
pub const MSR_STAR: u32 = 0xC000_0081;
pub const MSR_LSTAR: u32 = 0xC000_0082;
pub const MSR_CSTAR: u32 = 0xC000_0083;
pub const MSR_SFMASK: u32 = 0xC000_0084;
pub const MSR_FS_BASE: u32 = 0xC000_0100;
pub const MSR_GS_BASE: u32 = 0xC000_0101;
pub const MSR_KERNEL_GS_BASE: u32 = 0xC000_0102;
pub const MSR_TSC_AUX: u32 = 0xC000_0103;

/// Fixed-range MTRRs, in the order KVM's `fixed_msr_to_seg_unit` uses.
const MTRR_FIXED: [u32; 11] = [
    0x250, 0x258, 0x259, 0x268, 0x269, 0x26A, 0x26B, 0x26C, 0x26D, 0x26E, 0x26F,
];
/// MTRRcap: 8 variable ranges, fixed ranges, WC — KVM's `KVM_NR_VAR_MTRR | 0x500`.
const MTRR_CAP_VALUE: u64 = 0x508;
/// No firmware runs in the guest, so start where a BIOS leaves it: MTRRs on,
/// default type WB, no ranges. The guest's MTRRs never reach hardware — with
/// NPT/EPT the memory type comes from the host tables and the guest PAT.
const MTRR_DEF_TYPE_RESET: u64 = 0x806;

/// HWCR.TscFreqSel (AMD): the TSC counts at P0 — true with invariant TSC.
pub const HWCR_TSC_FREQ_SEL: u64 = 1 << 24;
/// IA32_MISC_ENABLE (Intel): fast strings on, BTS/PEBS unavailable — KVM's
/// reset value plus the fast-string bit every BIOS sets (without it Linux
/// drops ERMS and REP_GOOD).
const MISC_ENABLE_RESET: u64 = (1 << 0) | (1 << 11) | (1 << 12);

/// Per-vCPU emulated MSR state (both vendors; unused fields stay at reset).
pub struct GuestMsrs {
    pub spec_ctrl: u64,
    pub hwcr: u64,
    pub misc_enable: u64,
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
            misc_enable: MISC_ENABLE_RESET,
            mtrr_def_type: MTRR_DEF_TYPE_RESET,
            mtrr_var: [0; 16],
            mtrr_fixed: [0; 11],
            mcg_status: 0,
            tsc_adjust: 0,
        }
    }
}

fn valid_mem_type(t: u8) -> bool { matches!(t, 0 | 1 | 4 | 5 | 6) }

pub fn pat_valid(v: u64) -> bool {
    (0..8).all(|i| matches!((v >> (i * 8)) as u8, 0 | 1 | 4 | 5 | 6 | 7))
}

pub fn host_rdmsr(msr: u32) -> u64 {
    let (lo, hi): (u32, u32);
    // SAFETY: callers pass only MSRs architectural on the running vendor.
    unsafe {
        core::arch::asm!("rdmsr", in("ecx") msr, out("eax") lo, out("edx") hi,
                         options(nomem, nostack, preserves_flags));
    }
    ((hi as u64) << 32) | lo as u64
}

unsafe fn host_wrmsr(msr: u32, v: u64) {
    // SAFETY: caller guarantees msr/v are valid on this CPU.
    unsafe {
        core::arch::asm!("wrmsr", in("ecx") msr, in("eax") v as u32, in("edx") (v >> 32) as u32,
                         options(nomem, nostack, preserves_flags));
    }
}

/// Common RDMSR. `spec_valid` = the vendor's SPEC_CTRL bits (0 = no MSR).
pub fn read(st: &GuestMsrs, msr: u32, apic_id: u8, lapic_on: bool, spec_valid: u64) -> MsrResult<u64> {
    Ok(match msr {
        MSR_APIC_BASE => {
            // BSP bit (8) only for the boot vCPU — Linux's topology code
            // otherwise mistakes the guest for a kdump kernel and caps it at 1 CPU.
            let mut v = crate::microvm::cpu::svm::lapic::APIC_BASE_MSR_VALUE;
            if apic_id != 0 { v &= !(1u64 << 8); }
            if !lapic_on { v &= !(1u64 << 11); }
            v
        }
        MSR_TSC_ADJUST => st.tsc_adjust,
        MSR_SPEC_CTRL => {
            if spec_valid == 0 { return Err(()); }
            st.spec_ctrl
        }
        MSR_PRED_CMD => return Err(()), // write-only
        MSR_MTRR_CAP => MTRR_CAP_VALUE,
        MSR_MTRR_DEF_TYPE => st.mtrr_def_type,
        MSR_MTRR_PHYS_FIRST..=MSR_MTRR_PHYS_LAST => {
            st.mtrr_var[(msr - MSR_MTRR_PHYS_FIRST) as usize]
        }
        m if MTRR_FIXED.contains(&m) => {
            st.mtrr_fixed[MTRR_FIXED.iter().position(|&x| x == m).unwrap_or(0)]
        }
        // No machine-check banks (KVM with mcg_cap = 0).
        MSR_MCG_CAP => 0,
        MSR_MCG_STATUS => st.mcg_status,
        MSR_MCG_CTL => return Err(()),
        MSR_MC_FIRST..=MSR_MC_LAST => return Err(()),
        // No supervisor XSAVE states.
        MSR_XSS => 0,
        _ => unknown(msr, None),
    })
}

/// Common WRMSR.
pub fn write(st: &mut GuestMsrs, msr: u32, val: u64, spec_valid: u64) -> MsrResult<()> {
    match msr {
        MSR_APIC_BASE => {} // fixed base — the trap page sits at 0xFEE00000
        // Stored only; the guest's TSC offset stays ours.
        MSR_TSC_ADJUST => st.tsc_adjust = val,
        MSR_SPEC_CTRL => {
            if val & !spec_valid != 0 { return Err(()); }
            st.spec_ctrl = val;
        }
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
        MSR_MCG_STATUS => st.mcg_status = val,
        MSR_MCG_CAP | MSR_MCG_CTL => return Err(()),
        MSR_MC_FIRST..=MSR_MC_LAST => return Err(()),
        MSR_XSS => if val != 0 { return Err(()); },
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
            Some(v) => crate::kprintln!("[vm] WRMSR {:#010x} = {:#018x} (ignored)", msr, v),
            None => crate::kprintln!("[vm] RDMSR {:#010x} -> 0 (not emulated)", msr),
        }
    }
    0
}

/// Load the guest's SPEC_CTRL before VM entry; returns the host value to
/// restore. KVM without V_SPEC_CTRL does the same swap (`x86_spec_ctrl_set_guest`).
pub fn spec_ctrl_enter(guest: u64) -> Option<u64> {
    if guest == 0 { return None; }
    let host = host_rdmsr(MSR_SPEC_CTRL);
    if host == guest { return None; }
    // SAFETY: guest was validated against the host's SPEC_CTRL bits in `write`.
    unsafe { host_wrmsr(MSR_SPEC_CTRL, guest); }
    Some(host)
}

pub fn spec_ctrl_exit(host: Option<u64>) {
    if let Some(h) = host {
        // SAFETY: restoring the value read from this very MSR before entry.
        unsafe { host_wrmsr(MSR_SPEC_CTRL, h); }
    }
}
