//! Per-core CPU mitigations the host must set itself — Linux `init_amd_zen2`.
//!
//! A microvm guest's MSR writes stay in the guest, so the host sets these
//! itself on every core before any guest can run there.

use crate::hw::msr;

const MSR_DE_CFG: u32 = 0xC001_1029;
const DE_CFG_ZEN2_FP_BACKUP_FIX: u64 = 1 << 9;
const MSR_ZEN2_SPECTRAL_CHICKEN: u32 = 0xC001_10E3;
const SPECTRAL_CHICKEN_BIT: u64 = 1 << 1;
const MSR_PATCH_LEVEL: u32 = 0x8B;

fn cpuid(leaf: u32) -> (u32, u32, u32, u32) {
    let r = core::arch::x86_64::__cpuid_count(leaf, 0);
    (r.eax, r.ebx, r.ecx, r.edx)
}

/// The chicken-bit MSRs Linux `init_amd_zen2` sets.
#[derive(Clone, Copy)]
enum Chicken {
    DeCfg,
    Spectral,
}

/// A bare-metal AMD family 17h Zen2 core; only `apply` makes one, after the
/// check.
struct Zen2(());

impl Zen2 {
    fn patch_level(&self) -> u32 {
        // SAFETY: MSR_PATCH_LEVEL exists on every Zen2; reading it has no
        // side effect.
        unsafe { msr::read(MSR_PATCH_LEVEL) as u32 }
    }

    /// Read-modify-write one bit of a chicken MSR.
    fn set_bit(&self, c: Chicken, bit: u64, on: bool) {
        let m = match c {
            Chicken::DeCfg => MSR_DE_CFG,
            Chicken::Spectral => MSR_ZEN2_SPECTRAL_CHICKEN,
        };
        // SAFETY: both MSRs exist on every Zen2 (Linux `init_amd_zen2`), and
        // flipping the bits Linux flips only changes speculation behaviour.
        unsafe {
            let v = msr::read(m);
            msr::write(m, if on { v | bit } else { v & !bit });
        }
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
    let cpu = Zen2(());
    // Retbleed: suppress non-branch predictions.
    cpu.set_bit(Chicken::Spectral, SPECTRAL_CHICKEN_BIT, true);
    // Zenbleed: without fixed microcode the chicken bit is the mitigation —
    // otherwise vector registers leak across contexts, guest ↔ host included.
    let rev = cpu.patch_level();
    cpu.set_bit(Chicken::DeCfg, DE_CFG_ZEN2_FP_BACKUP_FIX, !zenbleed_fixed(model, rev));
}
