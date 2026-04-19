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

// DELTA_SWINGIDX tables for 2G path A/B (rtw8852b_table.c 14637-14651).
// 30 entries each (DELTA_SWINGIDX_SIZE = 30). Used by set_tmeter_tbl to
// build the 64-byte thermal offset table when efuse thermal is valid.
const DELTA_SWINGIDX_SIZE: usize = 30;

const DELTA_2GA_N: [i8; DELTA_SWINGIDX_SIZE] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];
const DELTA_2GA_P: [i8; DELTA_SWINGIDX_SIZE] = [
    0, 1, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2, 2, 3, 3,
    3, 3, 3, 3, 3, 4, 4, 4, 4, 4, 4, 5, 5, 5,
];
const DELTA_2GB_N: [i8; DELTA_SWINGIDX_SIZE] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, -1, -1,
    -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -2, -2,
];
const DELTA_2GB_P: [i8; DELTA_SWINGIDX_SIZE] = [
    0, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2, 2, 3, 3, 3, 3,
    3, 4, 4, 4, 4, 4, 5, 5, 5, 5, 5, 5, 6, 6,
];

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

/// Build the 64-byte thermal offset table per Linux _tssi_set_tmeter_tbl:
///   thm_ofst[0..32]  = -thm_down[i], clamped to last entry
///   thm_ofst[32..64] =  thm_up[i],   in reverse from index 63 downwards
/// (Linux rfk.c:2853-2863.)
fn build_thm_ofst(up: &[i8; DELTA_SWINGIDX_SIZE], down: &[i8; DELTA_SWINGIDX_SIZE])
    -> [i8; 64]
{
    let mut t = [0i8; 64];
    let mut i = 0usize;
    for j in 0..32 {
        t[j] = if i < DELTA_SWINGIDX_SIZE {
            let v = -down[i]; i += 1; v
        } else {
            -down[DELTA_SWINGIDX_SIZE - 1]
        };
    }
    i = 1;
    for j in (32..64).rev() {
        t[j] = if i < DELTA_SWINGIDX_SIZE {
            let v = up[i]; i += 1; v
        } else {
            up[DELTA_SWINGIDX_SIZE - 1]
        };
    }
    t
}

/// Pack 4 s8 bytes into a little-endian u32 (Linux RTW8852B_TSSI_GET_VAL).
fn pack4(t: &[i8; 64], idx: usize) -> u32 {
    (t[idx] as u8 as u32)
        | ((t[idx + 1] as u8 as u32) << 8)
        | ((t[idx + 2] as u8 as u32) << 16)
        | ((t[idx + 3] as u8 as u32) << 24)
}

/// _tssi_set_tmeter_tbl — rfk.c:2773. Efuse thermal drives the delta
/// tables. thermal=0xff → fallback (zero offsets, TMETER=32). With a
/// real thermal value, writes the thermal into TMETER + RFCTM_VAL and
/// fills the 64-byte offset table from the DELTA_SWINGIDX_2G{A,B}_{N,P}
/// constants above.
fn set_tmeter_tbl(mmio: i32, path: u8, thermal: u8) {
    let (r_tmeter, b_tmeter_dis, b_tmeter_trk, b_tmeter,
         r_rfctm, b_rfctm_val, r_rfctm_rdy, r_base)
    = if path == 0 {
        (R_P0_TMETER, B_P0_TMETER_DIS, B_P0_TMETER_TRK, B_P0_TMETER,
         R_P0_RFCTM, B_P0_RFCTM_VAL, R_P0_RFCTM_RDY, R_P0_TSSI_BASE)
    } else {
        (R_P1_TMETER, B_P1_TMETER_DIS, B_P1_TMETER_TRK, B_P1_TMETER,
         R_P1_RFCTM, B_P1_RFCTM_VAL, R_P1_RFCTM_RDY, R_TSSI_THOF)
    };

    pwm(mmio, r_tmeter, b_tmeter_dis, 0x0);
    pwm(mmio, r_tmeter, b_tmeter_trk, 0x1);

    if thermal == 0xFF {
        // Fallback: TMETER = 32, all offsets = 0.
        pwm(mmio, r_tmeter, b_tmeter,    32);
        pwm(mmio, r_rfctm,  b_rfctm_val, 32);
        for off in (0..64u32).step_by(4) {
            pw(mmio, r_base + off, 0);
        }
    } else {
        // Real thermal: program it + build delta table.
        pwm(mmio, r_tmeter, b_tmeter,    thermal as u32);
        pwm(mmio, r_rfctm,  b_rfctm_val, thermal as u32);

        let (up, down) = if path == 0 {
            (&DELTA_2GA_P, &DELTA_2GA_N)
        } else {
            (&DELTA_2GB_P, &DELTA_2GB_N)
        };
        let t = build_thm_ofst(up, down);
        for off in 0..16u32 {
            pw(mmio, r_base + off * 4, pack4(&t, (off * 4) as usize));
        }
    }

    pwm(mmio, r_rfctm, r_rfctm_rdy, 0x1);
    pwm(mmio, r_rfctm, r_rfctm_rdy, 0x0);
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

// ── _tssi_set_efuse_to_de (rfk.c:3296) ─────────────────────────
//
// Writes per-channel CCK and MCS DE values into the BB from the
// efuse-derived tssi_cck / tssi_mcs / tssi_trim arrays.
//
// The BB target registers are per-path + per-bandwidth:
//   CCK long:   path A 0x5858 / path B 0x7858
//   CCK short:  path A 0x5860 / path B 0x7860
//   MCS  5m:    path A 0x5828 / path B 0x7828
//   MCS 10m:    path A 0x5830 / path B 0x7830
//   MCS 20m:    path A 0x5838 / path B 0x7838
//   MCS 40m:    path A 0x5840 / path B 0x7840
//   MCS 80m:    path A 0x5848 / path B 0x7848
//   MCS 80_80m: path A 0x5850 / path B 0x7850
// Each register bit field: _TSSI_DE_MASK = GENMASK(21,12).

const TSSI_DE_MASK: u32 = 0x003F_F000; // GENMASK(21,12)

const DE_CCK_LONG:   [u32; 2] = [0x5858, 0x7858];
const DE_CCK_SHORT:  [u32; 2] = [0x5860, 0x7860];
const DE_MCS_5M:     [u32; 2] = [0x5828, 0x7828];
const DE_MCS_10M:    [u32; 2] = [0x5830, 0x7830];
const DE_MCS_20M:    [u32; 2] = [0x5838, 0x7838];
const DE_MCS_40M:    [u32; 2] = [0x5840, 0x7840];
const DE_MCS_80M:    [u32; 2] = [0x5848, 0x7848];
const DE_MCS_80M80M: [u32; 2] = [0x5850, 0x7850];

/// _tssi_get_cck_group — rfk.c:3106. 2G ch 1..14 → group 0..5.
fn cck_group(ch: u8) -> usize {
    match ch {
        1..=2   => 0,
        3..=5   => 1,
        6..=8   => 2,
        9..=11  => 3,
        12..=13 => 4,
        14      => 5,
        _       => 0,
    }
}

/// _tssi_get_ofdm_group — rfk.c:3132. Full table ported. High bit 31
/// marks an "extra" group where the DE is averaged between two
/// adjacent groups. Returns packed u32 exactly as Linux does.
const EXTRA: u32 = 1 << 31;
fn ofdm_group(ch: u8) -> u32 {
    match ch {
        1..=2     => 0,
        3..=5     => 1,
        6..=8     => 2,
        9..=11    => 3,
        12..=14   => 4,
        36..=40   => 5,
        41..=43   => EXTRA | 5,
        44..=48   => 6,
        49..=51   => EXTRA | 6,
        52..=56   => 7,
        57..=59   => EXTRA | 7,
        60..=64   => 8,
        100..=104 => 9,
        105..=107 => EXTRA | 9,
        108..=112 => 10,
        113..=115 => EXTRA | 10,
        116..=120 => 11,
        121..=123 => EXTRA | 11,
        124..=128 => 12,
        129..=131 => EXTRA | 12,
        132..=136 => 13,
        137..=139 => EXTRA | 13,
        140..=144 => 14,
        149..=153 => 15,
        154..=156 => EXTRA | 15,
        157..=161 => 16,
        162..=164 => EXTRA | 16,
        165..=169 => 17,
        170..=172 => EXTRA | 17,
        173..=177 => 18,
        _         => 0,
    }
}

/// _tssi_get_trim_group — rfk.c:3200. 2G ch 1..14 → 0..1, 5G → 2..7.
fn trim_group(ch: u8) -> usize {
    match ch {
        1..=8     => 0,
        9..=14    => 1,
        36..=48   => 2,
        52..=64   => 3,
        100..=112 => 4,
        116..=128 => 5,
        132..=144 => 6,
        149..=177 => 7,
        _         => 0,
    }
}

/// Look up MCS DE from efuse tssi_mcs arrays (2G or 5G depending on
/// group idx). Handles "EXTRA" averaged groups like Linux.
fn get_mcs_de(e: &crate::efuse::EfuseData, path: usize, ch: u8) -> i8 {
    let g = ofdm_group(ch);
    let lookup = |idx: u32| -> i8 {
        let i = idx as usize;
        if i < 5 {
            e.tssi_mcs_2g[path][i] as i8
        } else {
            let k = i - 5;
            if k < 14 { e.tssi_mcs_5g[path][k] as i8 } else { 0 }
        }
    };
    if g & EXTRA != 0 {
        let a = lookup(g & !EXTRA);
        let b = lookup((g & !EXTRA) + 1);
        (((a as i16) + (b as i16)) / 2) as i8
    } else {
        lookup(g)
    }
}

fn get_trim_de(e: &crate::efuse::EfuseData, path: usize, ch: u8) -> i8 {
    let t = trim_group(ch);
    e.tssi_trim[path][t]
}

/// _tssi_set_efuse_to_de — rfk.c:3296. Writes CCK + MCS DE values for
/// the current channel into 8 BB registers per path (16 total).
pub fn set_efuse_to_de(mmio: i32, e: &crate::efuse::EfuseData, ch: u8) {
    host::print("  TSSI: set_efuse_to_de ch=");
    crate::fw::print_dec(ch as usize);
    host::print("\n");

    for path in 0..2usize {
        let trim = get_trim_de(e, path, ch);
        let cck_base = e.tssi_cck[path][cck_group(ch)] as i8;
        // Linux: val = (s32)cck_base + trim_de (_TSSI_DE_MASK keeps it in 10 bits).
        let cck_val = ((cck_base as i32) + (trim as i32)) as u32 & 0x3FF;
        pwm(mmio, DE_CCK_LONG[path],  TSSI_DE_MASK, cck_val);
        pwm(mmio, DE_CCK_SHORT[path], TSSI_DE_MASK, cck_val);

        let mcs_base = get_mcs_de(e, path, ch);
        let mcs_val = ((mcs_base as i32) + (trim as i32)) as u32 & 0x3FF;
        pwm(mmio, DE_MCS_5M[path],     TSSI_DE_MASK, mcs_val);
        pwm(mmio, DE_MCS_10M[path],    TSSI_DE_MASK, mcs_val);
        pwm(mmio, DE_MCS_20M[path],    TSSI_DE_MASK, mcs_val);
        pwm(mmio, DE_MCS_40M[path],    TSSI_DE_MASK, mcs_val);
        pwm(mmio, DE_MCS_80M[path],    TSSI_DE_MASK, mcs_val);
        pwm(mmio, DE_MCS_80M80M[path], TSSI_DE_MASK, mcs_val);
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

// ── alimentk (rfk.c:3559): the actual TX-loop auto-calibration ───
//
// Sends 4 test-TX bursts at decreasing power levels, reads the TSSI
// feedback ADC CW value for each, and computes per-path alignment
// offsets that get written into the TSSI_ALIM1/2/3/4 BB registers.
// These offsets are what normal (non-PMAC) TX uses to know how to
// drive the PA for a target output power.
//
// Without alimentk the TSSI loop tracks against defaults, so actual
// output power for a given target dBm is unknown. Linux always runs
// this per channel; it's why every AP replies to their Probe Reqs.

const ALIM_REG_P0: [u32; 4] = [0x5630, 0x5634, 0x563C, 0x5640]; // ALIM1/3/2/4 (Linux order)
const ALIM_REG_P1: [u32; 4] = [0x7630, 0x7634, 0x763C, 0x7640];

// _tssi_cw_default_addr per path × 4 (rfk.c:85).
const CW_DEFAULT_ADDR: [[u32; 4]; 2] = [
    [0x5634, 0x5630, 0x5630, 0x5630],
    [0x7634, 0x7630, 0x7630, 0x7630],
];
const CW_DEFAULT_MASK: [u32; 4] = [0x000003FF, 0x3FF00000, 0x000FFC00, 0x000003FF];

// ALIM write fields (reg.h).
const B_P0_TSSI_ALIM11: u32 = 0x3FF0_0000; // GENMASK(29,20)
const B_P0_TSSI_ALIM12: u32 = 0x000F_FC00; // GENMASK(19,10)
const B_P0_TSSI_ALIM13: u32 = 0x0000_03FF; // GENMASK(9,0)
const B_P0_TSSI_ALIM1:  u32 = 0x3FFF_FFFF; // GENMASK(29,0)

// TSSI trigger / CW report.
const B_TSSI_CWRPT:     u32 = 0x0000_01FF;
const B_TSSI_CWRPT_RDY: u32 = 1 << 16;
const TSSI_TRIGGER: [u32; 2] = [0x5820, 0x7820];
const TSSI_CW_RPT:  [u32; 2] = [0x1C18, 0x3C18];
const B_P0_TSSI_AVG_F: u32 = 0x0000_F000; // GENMASK(15,12)
const B_P0_TSSI_MV_AVG_F: u32 = 0x0000_3800; // GENMASK(13,11)

const R_TX_COUNTER:  u32 = 0x1A40;
const MASK_LWORD: u32 = 0x0000_FFFF;

fn pr(mmio: i32, addr: u32) -> u32 {
    host::mmio_r32(mmio, crate::phy::PHY_CR_BASE + addr)
}

/// Sign-extend a `bits`-bit value to i32.
fn sext(v: u32, bits: u32) -> i32 {
    let shift = 32 - bits;
    ((v << shift) as i32) >> shift
}

/// _tssi_hw_tx wrapper — enable or disable PMAC test-TX for one path.
fn hw_tx(mmio: i32, path: u8, pwr_dbm: i16, enable: bool) {
    if enable {
        crate::bb::set_plcp_tx(mmio);
        crate::bb::cfg_tx_path(mmio, path);
        // Simplified: always route RX to path AB during alimentk.
        crate::bb::ctrl_rx_path(mmio, crate::bb::RF_PATH_AB);
        crate::bb::set_power(mmio, pwr_dbm);
    }
    crate::bb::set_pmac_pkt_tx(mmio, enable, 100, 5000);
}

/// _tssi_get_cw_report — sweeps 2 power levels, waits for CW_RDY,
/// records the CW report per slot. Returns Some([cw0, cw1]) or None
/// on timeout.
fn get_cw_report(mmio: i32, path: u8, power: &[i16]) -> Option<[u32; 2]> {
    let mut rpt = [0u32; 2];
    for j in 0..2 {
        // Re-arm TSSI_EN
        let (trig_reg, en_mask) = if path == 0 {
            (TSSI_TRIGGER[0], B_P0_TSSI_EN)
        } else {
            (TSSI_TRIGGER[1], B_P1_TSSI_EN)
        };
        host::mmio_w32_mask(mmio, crate::phy::PHY_CR_BASE + trig_reg, en_mask, 0);
        host::mmio_w32_mask(mmio, crate::phy::PHY_CR_BASE + trig_reg, en_mask, 1);

        // Start test-TX at this power
        hw_tx(mmio, path, power[j], true);

        // Poll CW_RPT_RDY — Linux: 100 × 30µs = 3ms max
        let rpt_addr = TSSI_CW_RPT[path as usize];
        let mut ready = false;
        for _ in 0..100u32 {
            let v = pr(mmio, rpt_addr);
            if v & B_TSSI_CWRPT_RDY != 0 {
                rpt[j] = v & B_TSSI_CWRPT;
                ready = true;
                break;
            }
            // ~30 µs
            for _ in 0..3000u32 { core::hint::spin_loop(); }
        }

        // Stop test-TX
        hw_tx(mmio, path, power[j], false);

        if !ready { return None; }
    }
    Some(rpt)
}

/// alimentk — per-path cal run. Writes alignment offsets into
/// R_P0_TSSI_ALIM1..4 (or P1_*) so regular TX uses calibrated power.
pub fn alimentk(mmio: i32, path: u8, ch: u8) {
    host::print("  TSSI alimentk path=");
    crate::fw::print_dec(path as usize);
    host::print(" ch=");
    crate::fw::print_dec(ch as usize);
    host::print("\n");

    let _ = ch; // channel is already tuned

    // 4 test power levels for 2G (Linux rfk.c:3564).
    let power: [i16; 4] = [48, 20, 4, 4];

    // Save BB state so we can restore cleanly.
    let bak = crate::bb::backup_tssi(mmio);
    let regs: [u32; 8] = [0x5820, 0x7820, 0x4978, 0x58E4, 0x78E4, 0x49C0, 0x0D18, 0x0D80];
    let mut reg_bak = [0u32; 8];
    for i in 0..8 { reg_bak[i] = pr(mmio, regs[i]); }

    // Configure TSSI averaging for the sweep.
    pwm(mmio, R_P0_TSSI_AVG,    B_P0_TSSI_AVG_F,    0x8);
    pwm(mmio, R_P1_TSSI_AVG,    B_P0_TSSI_AVG_F,    0x8);
    pwm(mmio, R_P0_TSSI_MV_AVG, B_P0_TSSI_MV_AVG_F, 0x2);
    pwm(mmio, R_P1_TSSI_MV_AVG, B_P0_TSSI_MV_AVG_F, 0x2);

    let cw = match get_cw_report(mmio, path, &power) {
        Some(v) => v,
        None => {
            host::print("    alimentk: CW report timeout — skip\n");
            // Restore
            for i in 0..8 {
                host::mmio_w32(mmio, crate::phy::PHY_CR_BASE + regs[i], reg_bak[i]);
            }
            crate::bb::restore_tssi(mmio, &bak);
            return;
        }
    };

    // Compute offsets (rfk.c:3641).
    let p = path as usize;
    let raw1 = (pr(mmio, CW_DEFAULT_ADDR[p][1]) & CW_DEFAULT_MASK[1]) >> CW_DEFAULT_MASK[1].trailing_zeros();
    let cw_def1 = sext(raw1, 8);
    let offset_1 = (cw[0] as i32) - ((power[0] - power[1]) as i32) * 2 - (cw[1] as i32) + cw_def1;
    let aliment_diff = offset_1 - cw_def1;

    let raw2 = (pr(mmio, CW_DEFAULT_ADDR[p][2]) & CW_DEFAULT_MASK[2]) >> CW_DEFAULT_MASK[2].trailing_zeros();
    let cw_def2 = sext(raw2, 8);
    let offset_2 = cw_def2 + aliment_diff;

    let raw3 = (pr(mmio, CW_DEFAULT_ADDR[p][3]) & CW_DEFAULT_MASK[3]) >> CW_DEFAULT_MASK[3].trailing_zeros();
    let cw_def3 = sext(raw3, 8);
    let offset_3 = cw_def3 + aliment_diff;

    host::print("    cw=[");
    crate::fw::print_dec(cw[0] as usize); host::print(",");
    crate::fw::print_dec(cw[1] as usize);
    host::print("] offsets=[");
    crate::fw::print_dec((offset_1 & 0x3FF) as usize); host::print(",");
    crate::fw::print_dec((offset_2 & 0x3FF) as usize); host::print(",");
    crate::fw::print_dec((offset_3 & 0x3FF) as usize);
    host::print("]\n");

    // Pack into 30-bit ALIM value: offset_1 in bits 29..20, offset_2 in
    // 19..10, offset_3 in 9..0 (all 10-bit signed).
    let packed = (((offset_1 as u32) & 0x3FF) << 20)
               | (((offset_2 as u32) & 0x3FF) << 10)
               | ((offset_3 as u32) & 0x3FF);

    // Write ALIM1 + ALIM2 per path.
    let alim1 = if path == 0 { ALIM_REG_P0[0] } else { ALIM_REG_P1[0] };
    let alim2 = if path == 0 { ALIM_REG_P0[2] } else { ALIM_REG_P1[2] };
    pwm(mmio, alim1, B_P0_TSSI_ALIM1, packed);
    pwm(mmio, alim2, B_P0_TSSI_ALIM1, packed);

    // Restore BB state.
    for i in 0..8 {
        host::mmio_w32(mmio, crate::phy::PHY_CR_BASE + regs[i], reg_bak[i]);
    }
    crate::bb::restore_tssi(mmio, &bak);
}

pub fn run(mmio: i32, band: u8, ch: u8, e: &crate::efuse::EfuseData) {
    host::print("  TSSI: start (setup-only, thermal=[0x");
    for i in 0..2 {
        let v = e.thermal[i];
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let buf = [HEX[((v >> 4) & 0xF) as usize], HEX[(v & 0xF) as usize]];
        host::print(unsafe { core::str::from_utf8_unchecked(&buf) });
        if i == 0 { host::print(",0x"); }
    }
    host::print("])\n");

    disable(mmio);

    for path in 0u8..2 {
        rf_setting(mmio, path, band);
        set_sys(mmio, path, band);
        ini_txpwr_ctrl_bb(mmio, path);
        ini_txpwr_ctrl_bb_he_tb(mmio, path);
        set_dck(mmio, path);
        set_tmeter_tbl(mmio, path, e.thermal[path as usize]);
        set_dac_gain_tbl(mmio, path);
        slope_cal_org(mmio, path, band);
        alignment_default(mmio, path, band, ch);
        set_tssi_slope(mmio, path);
        // alimentk re-enabled in v1.42 — v1.28/v1.29 timed out on CW_RPT
        // because IQK was running with ADC disabled (v1.38 fixed that),
        // so PMAC test-TX had no feedback path. With IQK now clean
        // (cor=0 fin=0 tx=0 rx=0) the TSSI alignment loop should see
        // real CW reports and calibrate PA output per channel.
        let tx_en = crate::fw::stop_sch_tx(mmio, 0);
        crate::iqk::wait_rx_mode_pub(mmio);
        alimentk(mmio, path, ch);
        crate::fw::resume_sch_tx(mmio, 0, tx_en);
    }

    enable(mmio);

    // Efuse→DE: per-channel CCK + MCS power correction. The reason TSSI
    // runs at all: without this the thermal loop has nothing to correct
    // *against*. 16 BB register writes.
    set_efuse_to_de(mmio, e, ch);

    host::print("  TSSI: enabled (both paths) + DE programmed\n");
}
