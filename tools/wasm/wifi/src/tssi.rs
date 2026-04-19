//! TSSI — Transmit Signal Strength Indicator calibration.
//!
//! 1:1 Port of Linux rtw8852b_rfk.c TSSI functions. Without TSSI the
//! PA runs open-loop and real output power is unknown — frames leave
//! the chip at undefined amplitude, APs see noise.
//!
//! Linux entry: rtw8852b_tssi(phy_idx, hwtx_en=true, chan).
//! We skip _tssi_alimentk (the TX-loop cal with sch_tx pause) for
//! Phase 1 — it requires hwtx_en and is a heavy multi-ms auto-cal.
//! Setup-only run lets the TSSI tracking hardware become live without
//! the per-level alignment sweep.
//!
//! Efuse dependencies: _tssi_set_tmeter_tbl reads per-path thermal from
//! efuse; with thermal=0xff (our default) it falls back to writing
//! zero offsets — HW uses the default delta table.
//! _tssi_set_efuse_to_de reads per-channel DE values from efuse;
//! without efuse we skip it (HW keeps defaults from the tables above).

use crate::host;
use crate::phy::{rf_write_mask, PHY_CR_BASE};
use crate::rfk;
use crate::tssi_tables::*;

const BAND_2G: u8 = 0;
const BAND_5G: u8 = 1;

// ── Register addresses (reg.h) ──────────────────────────────────

const RR_TXPOW: u32       = 0x7F;
const RR_TXPOW_TXG: u32   = 1 << 1;
const RR_TXPOW_TXA: u32   = 1 << 8;
const RR_TXGA_V1: u32     = 0x10055;
const RR_TXGA_V1_TRK_EN: u32 = 1 << 7;

// Path A PHY registers
const R_P0_TMETER: u32    = 0x5810;
const B_P0_TMETER: u32    = 0xFC00;      // GENMASK(15,10)
const B_P0_TMETER_DIS: u32 = 1 << 16;
const B_P0_TMETER_TRK: u32 = 1 << 24;
const R_P0_TSSIC: u32     = 0x5814;
const B_P0_TSSIC_BYPASS: u32 = 1 << 11;
const R_P0_TSSI_TRK: u32  = 0x5818;
const B_P0_TSSI_RFC: u32  = 0x18000000;  // GENMASK(28,27)
const B_P0_TSSI_OFT: u32  = 0xFF;         // GENMASK(7,0)
const B_P0_TSSI_OFT_EN: u32 = 1 << 28;
const R_P0_TSSI_AVG: u32  = 0x5820;
const B_P0_TSSI_EN: u32   = 1 << 31;
const R_P0_RFCTM: u32     = 0x5864;
const B_P0_RFCTM_VAL: u32 = 0x03F00000;   // GENMASK(25,20)
const R_P0_RFCTM_RDY: u32 = 1 << 26;
const R_P0_TSSI_MV_AVG: u32 = 0x58E4;
const B_P0_TSSI_MV_MIX: u32 = 0x000FF800; // GENMASK(19,11)
const B_P0_TSSI_MV_CLR: u32 = 1 << 14;
const R_P0_TSSI_BASE: u32 = 0x5C00;

// Path B PHY registers
const R_P1_TMETER: u32    = 0x7810;
const B_P1_TMETER: u32    = 0xFC00;
const B_P1_TMETER_DIS: u32 = 1 << 16;
const B_P1_TMETER_TRK: u32 = 1 << 24;
const R_P1_TSSIC: u32     = 0x7814;
const B_P1_TSSIC_BYPASS: u32 = 1 << 11;
const R_P1_TSSI_TRK: u32  = 0x7818;
const B_P1_TSSI_RFC: u32  = 0x18000000;
const B_P1_TSSI_OFT: u32  = 0xFF;
const B_P1_TSSI_OFT_EN: u32 = 1 << 28;
const R_P1_TSSI_AVG: u32  = 0x7820;
const B_P1_TSSI_EN: u32   = 1 << 31;
const R_P1_RFCTM: u32     = 0x7864;
const B_P1_RFCTM_VAL: u32 = 0x03F00000;
const R_P1_RFCTM_RDY: u32 = 1 << 26;
const R_P1_TSSI_MV_AVG: u32 = 0x78E4;
const B_P1_TSSI_MV_CLR: u32 = 1 << 14;
const B_P1_RFCTM_DEL: u32 = 0x000FF800;   // same layout as P0
const R_TSSI_THOF: u32    = 0x7C00;

// ── Helpers ─────────────────────────────────────────────────────

fn pwm(mmio: i32, addr: u32, mask: u32, val: u32) {
    let reg = PHY_CR_BASE + addr;
    host::mmio_w32_mask(mmio, reg, mask, val);
}

fn pw(mmio: i32, addr: u32, val: u32) {
    host::mmio_w32(mmio, PHY_CR_BASE + addr, val);
}

// ── TSSI sub-functions (rtw8852b_rfk.c 2719-3104) ───────────────

/// _tssi_rf_setting — rfk.c:2719. Enables TX PA chain for current band.
fn rf_setting(mmio: i32, path: u8, band: u8) {
    if band == BAND_2G {
        rf_write_mask(mmio, path, RR_TXPOW, RR_TXPOW_TXG, 0x1);
    } else {
        rf_write_mask(mmio, path, RR_TXPOW, RR_TXPOW_TXA, 0x1);
    }
}

/// _tssi_set_sys — rfk.c:2730. Applies sys-wide defs + per-path/band defs.
fn set_sys(mmio: i32, path: u8, band: u8) {
    rfk::parser(mmio, TSSI_SYS_DEFS);
    if path == 0 {
        if band == BAND_2G {
            rfk::parser(mmio, TSSI_SYS_A_DEFS_2G);
        } else {
            rfk::parser(mmio, TSSI_SYS_A_DEFS_5G);
        }
    } else {
        if band == BAND_2G {
            rfk::parser(mmio, TSSI_SYS_B_DEFS_2G);
        } else {
            rfk::parser(mmio, TSSI_SYS_B_DEFS_5G);
        }
    }
}

/// _tssi_ini_txpwr_ctrl_bb — rfk.c:2747. BB txpwr ctrl init per path.
fn ini_txpwr_ctrl_bb(mmio: i32, path: u8) {
    if path == 0 {
        rfk::parser(mmio, TSSI_INIT_TXPWR_DEFS_A);
    } else {
        rfk::parser(mmio, TSSI_INIT_TXPWR_DEFS_B);
    }
}

/// _tssi_ini_txpwr_ctrl_bb_he_tb — rfk.c:2756. HE-TB BB txpwr ctrl.
fn ini_txpwr_ctrl_bb_he_tb(mmio: i32, path: u8) {
    if path == 0 {
        rfk::parser(mmio, TSSI_INIT_TXPWR_HE_TB_DEFS_A);
    } else {
        rfk::parser(mmio, TSSI_INIT_TXPWR_HE_TB_DEFS_B);
    }
}

/// _tssi_set_dck — rfk.c:2765. DC-K per path.
fn set_dck(mmio: i32, path: u8) {
    if path == 0 {
        rfk::parser(mmio, TSSI_DCK_DEFS_A);
    } else {
        rfk::parser(mmio, TSSI_DCK_DEFS_B);
    }
}

/// _tssi_set_tmeter_tbl — rfk.c:2773. With thermal=0xff fallback:
/// writes 32 to R_Px_TMETER + R_Px_RFCTM.VAL, fills 64-byte offset
/// table at R_P0_TSSI_BASE / R_TSSI_THOF with zeroes. Efuse thermal
/// would give a per-chip delta table; without efuse the defaults are
/// sufficient for TSSI tracking to come up.
fn set_tmeter_tbl(mmio: i32, path: u8) {
    // thermal=0xff → fallback path in Linux
    if path == 0 {
        pwm(mmio, R_P0_TMETER, B_P0_TMETER_DIS, 0x0);
        pwm(mmio, R_P0_TMETER, B_P0_TMETER_TRK, 0x1);
        pwm(mmio, R_P0_TMETER, B_P0_TMETER, 32);
        pwm(mmio, R_P0_RFCTM, B_P0_RFCTM_VAL, 32);
        for off in (0..64u32).step_by(4) {
            pw(mmio, R_P0_TSSI_BASE + off, 0);
        }
        pwm(mmio, R_P0_RFCTM, R_P0_RFCTM_RDY, 0x1);
        pwm(mmio, R_P0_RFCTM, R_P0_RFCTM_RDY, 0x0);
    } else {
        pwm(mmio, R_P1_TMETER, B_P1_TMETER_DIS, 0x0);
        pwm(mmio, R_P1_TMETER, B_P1_TMETER_TRK, 0x1);
        pwm(mmio, R_P1_TMETER, B_P1_TMETER, 32);
        pwm(mmio, R_P1_RFCTM, B_P1_RFCTM_VAL, 32);
        for off in (0..64u32).step_by(4) {
            pw(mmio, R_TSSI_THOF + off, 0);
        }
        pwm(mmio, R_P1_RFCTM, R_P1_RFCTM_RDY, 0x1);
        pwm(mmio, R_P1_RFCTM, R_P1_RFCTM_RDY, 0x0);
    }
}

/// _tssi_set_dac_gain_tbl — rfk.c:2930.
fn set_dac_gain_tbl(mmio: i32, path: u8) {
    if path == 0 {
        rfk::parser(mmio, TSSI_DAC_GAIN_DEFS_A);
    } else {
        rfk::parser(mmio, TSSI_DAC_GAIN_DEFS_B);
    }
}

/// _tssi_slope_cal_org — rfk.c:2938.
fn slope_cal_org(mmio: i32, path: u8, band: u8) {
    if path == 0 {
        if band == BAND_2G { rfk::parser(mmio, TSSI_SLOPE_A_DEFS_2G); }
        else               { rfk::parser(mmio, TSSI_SLOPE_A_DEFS_5G); }
    } else {
        if band == BAND_2G { rfk::parser(mmio, TSSI_SLOPE_B_DEFS_2G); }
        else               { rfk::parser(mmio, TSSI_SLOPE_B_DEFS_5G); }
    }
}

/// _tssi_alignment_default — rfk.c:2953. For 2G ch 1-14 pick _2g_ tables.
/// `all=true` for full alignment apply (what rtw8852b_tssi calls).
fn alignment_default(mmio: i32, path: u8, band: u8, ch: u8) {
    // 2G ch 1..14
    if band == BAND_2G && ch >= 1 && ch <= 14 {
        if path == 0 { rfk::parser(mmio, TSSI_ALIGN_A_2G_ALL_DEFS); }
        else         { rfk::parser(mmio, TSSI_ALIGN_B_2G_ALL_DEFS); }
        return;
    }
    // 5G ch ranges
    if band == BAND_5G {
        if ch >= 36 && ch <= 64 {
            if path == 0 { rfk::parser(mmio, TSSI_ALIGN_A_5G1_ALL_DEFS); }
            else         { rfk::parser(mmio, TSSI_ALIGN_B_5G1_ALL_DEFS); }
        } else if ch >= 100 && ch <= 144 {
            if path == 0 { rfk::parser(mmio, TSSI_ALIGN_A_5G2_ALL_DEFS); }
            else         { rfk::parser(mmio, TSSI_ALIGN_B_5G2_ALL_DEFS); }
        } else if ch >= 149 && ch <= 177 {
            if path == 0 { rfk::parser(mmio, TSSI_ALIGN_A_5G3_ALL_DEFS); }
            else         { rfk::parser(mmio, TSSI_ALIGN_B_5G3_ALL_DEFS); }
        }
    }
}

/// _tssi_set_tssi_slope — rfk.c:3011.
fn set_tssi_slope(mmio: i32, path: u8) {
    if path == 0 {
        rfk::parser(mmio, TSSI_SLOPE_DEFS_A);
    } else {
        rfk::parser(mmio, TSSI_SLOPE_DEFS_B);
    }
}

/// _tssi_set_tssi_track — rfk.c:3019. Clear TSSIC_BYPASS so tracking
/// feedback loop runs.
fn set_tssi_track(mmio: i32, path: u8) {
    if path == 0 {
        pwm(mmio, R_P0_TSSIC, B_P0_TSSIC_BYPASS, 0x0);
    } else {
        pwm(mmio, R_P1_TSSIC, B_P1_TSSIC_BYPASS, 0x0);
    }
}

/// _tssi_set_txagc_offset_mv_avg — rfk.c:3028.
fn set_txagc_offset_mv_avg(mmio: i32, path: u8) {
    if path == 0 {
        pwm(mmio, R_P0_TSSI_MV_AVG, B_P0_TSSI_MV_MIX, 0x010);
    } else {
        pwm(mmio, R_P1_TSSI_MV_AVG, B_P1_RFCTM_DEL, 0x010);
    }
}

/// _tssi_enable — rfk.c:3041. Turns on TSSI hardware tracking per path.
/// This is the "TSSI mode ON" step.
fn enable(mmio: i32) {
    for path in 0u8..2 {
        set_tssi_track(mmio, path);
        set_txagc_offset_mv_avg(mmio, path);

        if path == 0 {
            pwm(mmio, R_P0_TSSI_MV_AVG, B_P0_TSSI_MV_CLR, 0x0);
            pwm(mmio, R_P0_TSSI_AVG,    B_P0_TSSI_EN,     0x0);
            pwm(mmio, R_P0_TSSI_AVG,    B_P0_TSSI_EN,     0x1);
            rf_write_mask(mmio, path, RR_TXGA_V1, RR_TXGA_V1_TRK_EN, 0x1);
            pwm(mmio, R_P0_TSSI_TRK,    B_P0_TSSI_RFC,    0x3);
            pwm(mmio, R_P0_TSSI_TRK,    B_P0_TSSI_OFT,    0xC0);
            pwm(mmio, R_P0_TSSI_TRK,    B_P0_TSSI_OFT_EN, 0x0);
            pwm(mmio, R_P0_TSSI_TRK,    B_P0_TSSI_OFT_EN, 0x1);
        } else {
            pwm(mmio, R_P1_TSSI_MV_AVG, B_P1_TSSI_MV_CLR, 0x0);
            pwm(mmio, R_P1_TSSI_AVG,    B_P1_TSSI_EN,     0x0);
            pwm(mmio, R_P1_TSSI_AVG,    B_P1_TSSI_EN,     0x1);
            rf_write_mask(mmio, path, RR_TXGA_V1, RR_TXGA_V1_TRK_EN, 0x1);
            pwm(mmio, R_P1_TSSI_TRK,    B_P1_TSSI_RFC,    0x3);
            pwm(mmio, R_P1_TSSI_TRK,    B_P1_TSSI_OFT,    0xC0);
            pwm(mmio, R_P1_TSSI_TRK,    B_P1_TSSI_OFT_EN, 0x0);
            pwm(mmio, R_P1_TSSI_TRK,    B_P1_TSSI_OFT_EN, 0x1);
        }
    }
}

/// _tssi_disable — rfk.c:3093. Called at TSSI entry to clear state.
fn disable(mmio: i32) {
    pwm(mmio, R_P0_TSSI_AVG,    B_P0_TSSI_EN,     0x0);
    pwm(mmio, R_P0_TSSI_TRK,    B_P0_TSSI_RFC,    0x1);
    pwm(mmio, R_P0_TSSI_MV_AVG, B_P0_TSSI_MV_CLR, 0x1);
    pwm(mmio, R_P1_TSSI_AVG,    B_P1_TSSI_EN,     0x0);
    pwm(mmio, R_P1_TSSI_TRK,    B_P1_TSSI_RFC,    0x1);
    pwm(mmio, R_P1_TSSI_MV_AVG, B_P1_TSSI_MV_CLR, 0x1);
}

// ═══════════════════════════════════════════════════════════════════
//  Public entry — rtw8852b_tssi (phase 1: setup only, no alimentk)
//
//  Channel parameters: band (2G=0, 5G=1), channel number.
//  hwtx_en is accepted for future alimentk integration; currently
//  ignored (we never run the TX alignment loop).
// ═══════════════════════════════════════════════════════════════════

pub fn run(mmio: i32, band: u8, ch: u8) {
    host::print("  TSSI: start (setup-only, no alimentk)\n");

    disable(mmio);

    for path in 0u8..2 {
        rf_setting(mmio, path, band);
        set_sys(mmio, path, band);
        ini_txpwr_ctrl_bb(mmio, path);
        ini_txpwr_ctrl_bb_he_tb(mmio, path);
        set_dck(mmio, path);
        set_tmeter_tbl(mmio, path);
        set_dac_gain_tbl(mmio, path);
        slope_cal_org(mmio, path, band);
        alignment_default(mmio, path, band, ch);
        set_tssi_slope(mmio, path);
        // _tssi_alimentk skipped (hwtx_en=false semantics)
    }

    enable(mmio);
    // _tssi_set_efuse_to_de skipped: needs efuse DE values

    host::print("  TSSI: enabled (both paths)\n");
}
