//! DPK — Digital Pre-Distortion (minimal bypass port).
//!
//! Full DPK is a ~500 LoC multi-ms calibration loop that measures PA
//! nonlinearity and writes per-rate correction coefficients. Linux has
//! an explicit "bypass" fallback path when:
//!   - external PA is present (fem.epa_*) — efuse-derived
//!   - or the cal would run on a band where it's known-broken
//!
//! We don't parse efuse yet, so we can't detect eFEM. For Phase 1 we
//! force bypass unconditionally: DPK correction is DISABLED, HW runs
//! with default (linear) output. Better than random coefficients from
//! an uninitialized DPK state.
//!
//! Linux entries:
//!   rtw8852b_dpk_init   → _set_dpd_backoff
//!   rtw8852b_dpk        → _dpk_bypass_check ? _dpk_force_bypass : _dpk_cal_select
//!
//! We port _set_dpd_backoff + _dpk_force_bypass only.

use crate::host;
use crate::phy::PHY_CR_BASE;

// Registers (reg.h)
const R_DPD_BF: u32     = 0x44A0;
const B_DPD_BF_OFDM: u32 = 0x0001_F000; // GENMASK(16,12)
const B_DPD_BF_SCA: u32 = 0x0000_007F;  // GENMASK(6,0)

const R_DPD_CH0A: u32   = 0x81BC;
const B_DPD_CFG: u32    = 0x007F_FFFF;  // GENMASK(22,0)
const MASKBYTE3: u32    = 0xFF00_0000;

const R_LDL_NORM: u32   = 0x80A0;
const B_LDL_NORM_OP: u32 = 0x0000_0003; // GENMASK(1,0)

fn pr(mmio: i32, addr: u32) -> u32 {
    host::mmio_r32(mmio, PHY_CR_BASE + addr)
}

fn pwm(mmio: i32, addr: u32, mask: u32, val: u32) {
    host::mmio_w32_mask(mmio, PHY_CR_BASE + addr, mask, val);
}

/// Port of _dpk_order_convert (rfk.c:1675).
/// Reads MDPD order setting, returns (3 >> order) mapped to 0..3.
fn order_convert(mmio: i32) -> u32 {
    let raw = pr(mmio, R_LDL_NORM);
    let order = raw & B_LDL_NORM_OP;
    3u32 >> order
}

/// Port of _set_dpd_backoff (rfk.c:2692), called from rtw8852b_dpk_init.
/// Reads OFDM and SCA backoff values; if their sum exceeds 44, moves
/// DPD backoff to BB and zeros the per-path CFG. Otherwise just tracks
/// the SW state (dpk_gs), which we skip — we have no SW state to track.
pub fn init(mmio: i32) {
    host::print("  DPK: init (set_dpd_backoff)\n");

    let bf = pr(mmio, R_DPD_BF);
    let ofdm_bkof = (bf & B_DPD_BF_OFDM) >> 12;
    let tx_scale = bf & B_DPD_BF_SCA;

    if ofdm_bkof + tx_scale >= 44 {
        // Move DPD backoff to BB: zero the per-path CFG.
        for path in 0u8..2 {
            let reg = R_DPD_CH0A + ((path as u32) << 8);
            pwm(mmio, reg, B_DPD_CFG, 0x007F_7F7F & B_DPD_CFG);
        }
        host::print("  DPK: backoff moved to BB (ofdm+sca >= 44)\n");
    }
}

/// Port of _dpk_onoff(path, off=true) (rfk.c:1688).
/// For kidx=0 (init state), writes MASKBYTE3 of R_DPD_CH0A + path*256
/// with `(order << 1) | 0` — DPK correction disabled.
fn onoff_path(mmio: i32, path: u8) {
    let order = order_convert(mmio);
    let kidx: u32 = 0;
    let reg = R_DPD_CH0A + ((path as u32) << 8) + (kidx << 2);
    // MASKBYTE3 shift = 24; value = (order << 1) | 0
    let val = (order << 1) & 0xFF;
    pwm(mmio, reg, MASKBYTE3, val);
}

/// Port of _dpk_force_bypass (rfk.c:2562) — disables DPK correction
/// on both RF paths. Replaces _dpk_cal_select for Phase 1.
pub fn force_bypass(mmio: i32) {
    host::print("  DPK: force bypass (both paths disabled)\n");
    for path in 0u8..2 {
        onoff_path(mmio, path);
    }
}
