//! Which speculation bugs this CPU has: Linux `cpu_set_bug_bits`
//! (arch/x86/kernel/cpu/common.c) and `tsa_init` (amd.c), with the vendor
//! tables generated from the same source (`cpu_vulns_table.rs`,
//! `tools/gen_cpu_vulns.py`). Only the bugs that bear on guest/host
//! isolation are kept; `spec_mitigations` picks what to do about them.

use crate::hw::msr;

#[path = "cpu_vulns_table.rs"]
mod table;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Vendor { Intel, Amd }

/// One row of `cpu_vuln_whitelist` / `cpu_vuln_blacklist`. `None` matches
/// any family or model; steppings from 0 up to `max_step`.
pub struct Row {
    pub vendor: Vendor,
    pub family: Option<u32>,
    pub model: Option<u32>,
    pub max_step: u32,
    pub bits: u32,
}

// Whitelist bits (common.c).
pub const NO_SPECULATION: u32 = 1 << 0;
pub const NO_MELTDOWN: u32 = 1 << 1;
pub const NO_SSB: u32 = 1 << 2;
pub const NO_L1TF: u32 = 1 << 3;
pub const NO_MDS: u32 = 1 << 4;
pub const MSBDS_ONLY: u32 = 1 << 5;
pub const NO_SWAPGS: u32 = 1 << 6;
pub const NO_ITLB_MULTIHIT: u32 = 1 << 7;
pub const NO_MMIO: u32 = 1 << 9;
pub const NO_EIBRS_PBRSB: u32 = 1 << 10;
pub const NO_BHI: u32 = 1 << 11;

// Blacklist bits (common.c).
pub const SRBDS: u32 = 1 << 0;
pub const MMIO: u32 = 1 << 1;
pub const MMIO_SBDS: u32 = 1 << 2;
pub const RETBLEED: u32 = 1 << 3;
pub const SMT_RSB: u32 = 1 << 4;
pub const SRSO: u32 = 1 << 5;
pub const GDS: u32 = 1 << 6;
pub const RFDS: u32 = 1 << 7;
pub const ITS: u32 = 1 << 8;
pub const ITS_NATIVE_ONLY: u32 = 1 << 9;
pub const TSA: u32 = 1 << 10;
pub const VMSCAPE: u32 = 1 << 11;

// IA32_ARCH_CAPABILITIES (msr-index.h).
const AC_RDCL_NO: u64 = 1 << 0;
const AC_IBRS_ALL: u64 = 1 << 1;
const AC_RSBA: u64 = 1 << 2;
pub const AC_SKIP_VMENTRY_L1DFLUSH: u64 = 1 << 3;
const AC_MDS_NO: u64 = 1 << 5;
const AC_TAA_NO: u64 = 1 << 8;
const AC_TSX_CTRL_MSR: u64 = 1 << 7;
const AC_SBDR_SSDP_NO: u64 = 1 << 13;
const AC_FBSDP_NO: u64 = 1 << 14;
const AC_PSDP_NO: u64 = 1 << 15;
const AC_PBRSB_NO: u64 = 1 << 24;
const AC_GDS_NO: u64 = 1 << 26;
const AC_RFDS_NO: u64 = 1 << 27;
pub const AC_RFDS_CLEAR: u64 = 1 << 28;
const AC_ITS_NO: u64 = 1 << 62;

const MSR_ARCH_CAPABILITIES: u32 = 0x10A;
/// IA32_BIOS_SIGN_ID on Intel, MSR_AMD64_PATCH_LEVEL on AMD.
const MSR_AMD64_PATCH_LEVEL: u32 = 0x8B;

/// The CPU's identity and the bugs Linux would set on it.
#[derive(Clone, Copy, Default)]
pub struct Bugs {
    pub intel: bool,
    pub amd: bool,
    pub family: u32,
    pub model: u32,
    pub stepping: u32,
    pub arch_cap: u64,
    pub hypervisor: bool,
    pub eibrs: bool,
    pub eibrs_pbrsb: bool,
    pub mds: bool,
    pub taa: bool,
    pub srbds: bool,
    pub mmio: bool,
    pub retbleed: bool,
    pub smt_rsb: bool,
    pub srso: bool,
    pub gds: bool,
    pub rfds: bool,
    pub bhi: bool,
    pub ibpb_no_ret: bool,
    pub its: bool,
    pub its_native_only: bool,
    pub tsa: bool,
    pub meltdown: bool,
    pub l1tf: bool,
    /// `X86_FEATURE_*` the mitigations need.
    pub md_clear: bool,
    pub flush_l1d: bool,
    pub verw_clear: bool,
    pub bhi_ctrl: bool,
    pub stibp: bool,
    pub stibp_always_on: bool,
    pub smt: bool,
    /// Microcode revision (`intel_get_microcode_revision`, AMD patch level).
    pub microcode: u32,
}

fn cpuid(leaf: u32, sub: u32) -> (u32, u32, u32, u32) {
    let r = core::arch::x86_64::__cpuid_count(leaf, sub);
    (r.eax, r.ebx, r.ecx, r.edx)
}

fn bit(v: u32, b: u32) -> bool { v & (1 << b) != 0 }

/// `x86_match_cpu`: the first row that names this CPU.
fn find(rows: &[Row], b: &Bugs) -> u32 {
    let vendor = if b.intel { Vendor::Intel } else if b.amd { Vendor::Amd } else { return 0 };
    rows.iter()
        .find(|r| r.vendor == vendor
            && r.family.is_none_or(|f| f == b.family)
            && r.model.is_none_or(|m| m == b.model)
            && b.stepping <= r.max_step)
        .map_or(0, |r| r.bits)
}

pub fn detect() -> Bugs {
    let (_, vb, vc, vd) = cpuid(0, 0);
    let max = cpuid(0, 0).0;
    let max_ext = cpuid(0x8000_0000, 0).0;
    let mut b = Bugs {
        intel: (vb, vd, vc) == (0x756e_6547, 0x4965_6e69, 0x6c65_746e),
        amd: (vb, vd, vc) == (0x6874_7541, 0x6974_6e65, 0x444d_4163),
        ..Bugs::default()
    };
    let (l1a, l1b, l1c, l1d) = cpuid(1, 0);
    let base_family = (l1a >> 8) & 0xF;
    b.family = base_family + if base_family == 0xF { (l1a >> 20) & 0xFF } else { 0 };
    b.model = ((l1a >> 4) & 0xF) | if base_family >= 6 { ((l1a >> 16) & 0xF) << 4 } else { 0 };
    b.stepping = l1a & 0xF;
    b.hypervisor = bit(l1c, 31);
    let (_, l7b, _, l7d) = if max >= 7 { cpuid(7, 0) } else { (0, 0, 0, 0) };
    let l72d = if max >= 7 && cpuid(7, 0).0 >= 2 { cpuid(7, 2).3 } else { 0 };
    let (_, e8b, _, _) = if max_ext >= 0x8000_0008 { cpuid(0x8000_0008, 0) } else { (0, 0, 0, 0) };
    let (e21a, _, e21c, _) = if max_ext >= 0x8000_0021 { cpuid(0x8000_0021, 0) } else { (0, 0, 0, 0) };

    if bit(l7d, 29) {
        // SAFETY: CPUID 7.0 EDX[29] says IA32_ARCH_CAPABILITIES exists.
        b.arch_cap = unsafe { msr::read(MSR_ARCH_CAPABILITIES) };
    }
    let ac = b.arch_cap;
    let rtm = bit(l7b, 11);
    let rdrand = bit(l1c, 30);
    let rdseed = bit(l7b, 18);
    let avx = bit(l1c, 28);
    let autoibrs = b.amd && bit(e21a, 8);
    b.md_clear = bit(l7d, 10);
    b.flush_l1d = bit(l7d, 28);
    b.bhi_ctrl = bit(l72d, 4);
    b.stibp = if b.amd { bit(e8b, 15) } else { bit(l7d, 27) };
    b.stibp_always_on = b.amd && bit(e8b, 17);
    // HTT, and more than one thread per core: leaf 0xB level 0 on Intel,
    // 0x8000_001E EBX[15:8] on AMD.
    b.smt = bit(l1d, 28) && if b.amd {
        max_ext >= 0x8000_001E && (cpuid(0x8000_001E, 0).1 >> 8) & 0xFF > 0
    } else {
        max >= 0xB && cpuid(0xB, 0).1 & 0xFFFF > 1
    };
    let _ = l1b;
    if b.intel && !b.hypervisor {
        // SAFETY: IA32_BIOS_SIGN_ID is architectural on Intel; writing 0 and
        // a CPUID make the CPU store the revision in its upper half.
        unsafe { msr::write(MSR_AMD64_PATCH_LEVEL, 0) };
        let _ = cpuid(1, 0);
        // SAFETY: as above.
        b.microcode = (unsafe { msr::read(MSR_AMD64_PATCH_LEVEL) } >> 32) as u32;
    } else if b.amd && !b.hypervisor {
        // SAFETY: MSR_AMD64_PATCH_LEVEL exists on every AMD family 0x10+.
        b.microcode = unsafe { msr::read(MSR_AMD64_PATCH_LEVEL) } as u32;
    }

    // `tsa_init`: Zen 3/4 get VERW_CLEAR from a microcode revision, every
    // other AMD part is TSA-immune.
    let mut verw_clear = bit(e21a, 5);
    let mut tsa_sq_no = bit(e21c, 1);
    let mut tsa_l1_no = bit(e21c, 2);
    if b.amd && !b.hypervisor {
        let zen34 = b.family == 0x19 && matches!(b.model, 0x00..=0x0f | 0x20..=0x5f | 0x10..=0x1f | 0x60..=0xaf);
        if zen34 {
            let rev = b.microcode;
            if table::AMD_TSA_MICROCODE.iter().any(|&(f, m, s, min)|
                f == b.family && m == b.model && s == b.stepping && rev >= min)
            {
                verw_clear = true;
            }
        } else {
            tsa_sq_no = true;
            tsa_l1_no = true;
        }
    }
    b.verw_clear = verw_clear;

    let wl = find(table::WHITELIST, &b);
    let bl = find(table::BLACKLIST, &b);
    if wl & NO_SPECULATION != 0 {
        return b;
    }
    if ac & AC_IBRS_ALL != 0 || autoibrs {
        b.eibrs = true;
        b.eibrs_pbrsb = wl & NO_EIBRS_PBRSB == 0 && ac & AC_PBRSB_NO == 0;
    }
    b.mds = wl & NO_MDS == 0 && ac & AC_MDS_NO == 0;
    b.taa = ac & AC_TAA_NO == 0 && (rtm || ac & AC_TSX_CTRL_MSR != 0);
    b.srbds = (rdrand || rdseed) && bl & (SRBDS | MMIO_SBDS) != 0;
    let mmio_immune = ac & AC_FBSDP_NO != 0 && ac & AC_PSDP_NO != 0 && ac & AC_SBDR_SSDP_NO != 0;
    b.mmio = !mmio_immune && bl & MMIO != 0;
    b.retbleed = !bit(e8b, 29) && (bl & RETBLEED != 0 || ac & AC_RSBA != 0);
    b.smt_rsb = bl & SMT_RSB != 0;
    b.srso = !bit(e21a, 29) && bl & SRSO != 0;
    b.gds = bl & GDS != 0 && ac & AC_GDS_NO == 0 && avx;
    // `vulnerable_to_rfds`: ADL/RPL without Atom cores are not affected;
    // hybrid is CPUID 7.0 EDX[15].
    b.rfds = if ac & AC_RFDS_NO != 0 {
        false
    } else if ac & AC_RFDS_CLEAR != 0 {
        true
    } else {
        bl & RFDS != 0
            && !(b.intel && matches!(b.model, 0x97 | 0xB7) && !bit(l7d, 15)
                && !(max >= 0x1A && cpuid(0x1A, 0).0 >> 24 == 0x20))
    };
    b.bhi = wl & NO_BHI == 0 && (b.eibrs || b.hypervisor);
    b.ibpb_no_ret = b.amd && bit(e8b, 12) && !bit(e8b, 30);
    // `vulnerable_to_its`.
    b.its = ac & AC_ITS_NO == 0 && b.intel && !b.bhi_ctrl
        && (b.hypervisor || bl & ITS != 0);
    b.its_native_only = b.its && bl & ITS_NATIVE_ONLY != 0;
    b.tsa = b.amd && (!tsa_sq_no || !tsa_l1_no) && bl & TSA != 0;
    if wl & NO_MELTDOWN != 0 || ac & AC_RDCL_NO != 0 {
        return b;
    }
    b.meltdown = true;
    b.l1tf = wl & NO_L1TF == 0;
    b
}
