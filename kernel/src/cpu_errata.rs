//! Per-core CPU mitigations the host must set itself — Linux `init_amd_zen2`.
//!
//! A microvm guest used to set these on the host core through pass-through
//! MSRs. Now that its MSR writes stay in the guest, the host does it, on
//! every core, before any guest can run there.

const MSR_DE_CFG: u32 = 0xC001_1029;
const DE_CFG_ZEN2_FP_BACKUP_FIX: u64 = 1 << 9;
const MSR_ZEN2_SPECTRAL_CHICKEN: u32 = 0xC001_10E3;
const SPECTRAL_CHICKEN_BIT: u64 = 1 << 1;
const MSR_PATCH_LEVEL: u32 = 0x8B;

fn cpuid(leaf: u32) -> (u32, u32, u32, u32) {
    let r = core::arch::x86_64::__cpuid_count(leaf, 0);
    (r.eax, r.ebx, r.ecx, r.edx)
}

fn rdmsr(msr: u32) -> u64 {
    let (lo, hi): (u32, u32);
    // SAFETY: only called for MSRs Linux documents on Zen2 (see `apply`).
    unsafe {
        core::arch::asm!("rdmsr", in("ecx") msr, out("eax") lo, out("edx") hi,
                         options(nomem, nostack, preserves_flags));
    }
    ((hi as u64) << 32) | lo as u64
}

fn wrmsr(msr: u32, v: u64) {
    // SAFETY: read-modify-write of a Zen2 chicken bit Linux sets the same way.
    unsafe {
        core::arch::asm!("wrmsr", in("ecx") msr, in("eax") v as u32, in("edx") (v >> 32) as u32,
                         options(nomem, nostack, preserves_flags));
    }
}

/// (family, model) per CPUID leaf 1, AMD encoding.
fn amd_family_model() -> Option<(u32, u32)> {
    let (_, b, c, d) = cpuid(0);
    // "AuthenticAMD"
    if (b, d, c) != (0x6874_7541, 0x6974_6E65, 0x444D_4163) { return None; }
    let (a, _, ecx1, _) = cpuid(1);
    // Under a hypervisor these MSRs are its business (Linux skips too).
    if ecx1 & (1 << 31) != 0 { return None; }
    let base_fam = (a >> 8) & 0xF;
    let fam = if base_fam == 0xF { base_fam + ((a >> 20) & 0xFF) } else { base_fam };
    let model = ((a >> 4) & 0xF) | (((a >> 16) & 0xF) << 4);
    Some((fam, model))
}

fn is_zen2(model: u32) -> bool {
    matches!(model, 0x30..=0x4F | 0x60..=0x7F | 0x90..=0x91 | 0xA0..=0xAF)
}

/// First microcode revision with the Zenbleed fix (`cpu_has_zenbleed_microcode`).
fn zenbleed_fixed(model: u32, rev: u32) -> bool {
    let good = match model {
        0x30..=0x3F => 0x0830_107B,
        0x60..=0x67 => 0x0860_010C,
        0x68..=0x6F => 0x0860_8107,
        0x70..=0x7F => 0x0870_1033,
        0xA0..=0xAF => 0x08A0_0009,
        _ => return false,
    };
    rev >= good
}

/// Run on every core (BSP and each AP) during bring-up.
pub fn apply() {
    let Some((fam, model)) = amd_family_model() else { return };
    if fam != 0x17 || !is_zen2(model) { return; }
    // Retbleed: suppress non-branch predictions.
    wrmsr(MSR_ZEN2_SPECTRAL_CHICKEN, rdmsr(MSR_ZEN2_SPECTRAL_CHICKEN) | SPECTRAL_CHICKEN_BIT);
    // Zenbleed: without fixed microcode the chicken bit is the mitigation —
    // otherwise vector registers leak across contexts, guest ↔ host included.
    let rev = rdmsr(MSR_PATCH_LEVEL) as u32;
    let de = rdmsr(MSR_DE_CFG);
    if zenbleed_fixed(model, rev) {
        wrmsr(MSR_DE_CFG, de & !DE_CFG_ZEN2_FP_BACKUP_FIX);
    } else {
        wrmsr(MSR_DE_CFG, de | DE_CFG_ZEN2_FP_BACKUP_FIX);
    }
}
