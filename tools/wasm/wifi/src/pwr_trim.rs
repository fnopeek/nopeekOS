//! rtw89_chip_power_trim — 1:1 port of rtw8852bx_thermal_trim
//! (rtw8852b_common.c:335) + rtw8852bx_pa_bias_trim (common.c:383).
//!
//! Called once from phy_dm_init right after set_txpwr_ctrl (phy.c:7706).
//! Applies per-chip PA-bias and thermal offsets loaded from efuse's
//! phycap region. Without these applied, PA bias stays at the HW
//! default which on some chips leaves the amplifier inactive, and
//! thermal correction is zero — neither is catastrophic on its own
//! but together they can keep TX physically silent.
//!
//! Both steps are gated by PG flags: if efuse byte is 0xFF, no PG,
//! skip (= NO-OP).

use crate::efuse::EfuseData;
use crate::phy::rf_write_mask;

// RF register addresses + masks (reg.h:8543+8574). Non-V1 variants
// apply for 8852B (V1 = 8852C).
const RR_TM2: u32 = 0x43;
const RR_TM2_OFF: u32 = 0x000F_0000; // GENMASK(19,16)

const RR_BIASA: u32 = 0x60;
const RR_BIASA_TXA: u32 = 0x000F_0000; // GENMASK(19,16)  — 5G
const RR_BIASA_TXG: u32 = 0x0000_F000; // GENMASK(15,12)  — 2G

/// rtw8852bx_thermal_trim (common.c:335).
/// For each RF path, if pg_thermal_trim is set, compute:
///   val = ((raw & 0x1) << 3) | ((raw & 0x1f) >> 1)
/// and write to RR_TM2[19:16].
fn thermal_trim(mmio: i32, e: &EfuseData) {
    if !e.pg_thermal_trim {
        crate::host::print("  PWR_TRIM: thermal no PG, skip\n");
        return;
    }
    for path in 0u8..2 {
        let raw = e.thermal_trim[path as usize];
        let val = (((raw as u32) & 0x1) << 3) | (((raw as u32) & 0x1F) >> 1);
        rf_write_mask(mmio, path, RR_TM2, RR_TM2_OFF, val);
    }
    crate::host::print("  PWR_TRIM: thermal applied (RR_TM2[19:16])\n");
}

/// rtw8852bx_pa_bias_trim (common.c:383).
/// For each RF path, if pg_pa_bias_trim is set:
///   pabias_2g = raw & 0xF
///   pabias_5g = (raw >> 4) & 0xF
/// write both into RR_BIASA (RR_BIASA_TXG = 2G, RR_BIASA_TXA = 5G).
fn pa_bias_trim(mmio: i32, e: &EfuseData) {
    if !e.pg_pa_bias_trim {
        crate::host::print("  PWR_TRIM: pa_bias no PG, skip\n");
        return;
    }
    for path in 0u8..2 {
        let raw = e.pa_bias_trim[path as usize] as u32;
        let pabias_2g = raw & 0xF;
        let pabias_5g = (raw >> 4) & 0xF;
        rf_write_mask(mmio, path, RR_BIASA, RR_BIASA_TXG, pabias_2g);
        rf_write_mask(mmio, path, RR_BIASA, RR_BIASA_TXA, pabias_5g);
    }
    crate::host::print("  PWR_TRIM: pa_bias applied (RR_BIASA.TXG/TXA)\n");
}

/// rtw89_chip_power_trim → __rtw8852bx_power_trim (common.c:445).
pub fn run(mmio: i32, e: &EfuseData) {
    thermal_trim(mmio, e);
    pa_bias_trim(mmio, e);
}
