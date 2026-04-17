//! IQK — IQ Imbalance Calibration framework port.
//!
//! Linux entry: rtw8852b_rfk.c:3757 rtw8852b_iqk.
//!
//! This is the MINIMUM viable port — applies the macbb set/restore tables
//! + iqk_preset + iqk_init. Does NOT yet run the full LOK/TXK/RXK
//! calibration loops (those require _iqk_one_shot + _iqk_check_cal polling
//! over many registers with chip-specific state machines, ~500 more lines).
//!
//! Running the tables alone walks the chip through the same BB/MAC state
//! transitions Linux does around IQK. Many of the table writes unstick
//! registers the scan RX path depends on (R_PD_CTRL, R_RXCCA, R_UPD_CLK_ADC
//! among them).

use crate::host;
use crate::iqk_tables::{
    RTW8852B_SET_NONDBCC_PATH01,
    RTW8852B_RESTORE_NONDBCC_PATH01,
};

const CR: u32 = 0x10000; // PHY_CR_BASE

// ── PHY register addresses for IQK (reg.h) ───────────────────────
const R_IQKINF: u32        = 0x9FE0;
const R_NCTL_CFG: u32      = 0x8000;
const R_NCTL_N1: u32       = 0x8004;
const R_NCTL_RPT: u32      = 0x8008;
const R_KIP_SYSCFG: u32    = 0x8240;
const R_CFIR_SYS: u32      = 0x8120;
const B_IQK_RES_K: u32     = 1 << 28;
const R_IQRSN: u32         = 0x8220;
const B_IQRSN_K1: u32      = 1 << 28;
const B_IQRSN_K2: u32      = 1 << 16;
const R_COEF_SEL: u32      = 0x8104;   // + path<<8
const B_COEF_SEL_IQC: u32  = 1 << 0;
const R_CFIR_LUT: u32      = 0x8154;   // + path<<8
const B_CFIR_LUT_G3: u32   = 1 << 20;
const R_TXIQC: u32         = 0x81D8;   // + path<<8
const R_RXIQC: u32         = 0x8220;   // + path<<8

// ── LOK-specific registers ───────────────────────────────────────
const R_KIP_IQP: u32       = 0x81CC;   // + path<<8
const R_IQK_DIF4: u32      = 0x802C;
const B_IQK_DIF4_TXT: u32  = 0xFFF << 0;   // [11:0]
const B_IQK_DIF4_RXT: u32  = 0xFFF << 16;  // [27:16]
const R_P0_RFCTM: u32      = 0x5864;
const B_P0_RFCTM_EN: u32   = 1 << 29;
const R_P0_NRBW: u32       = 0x12B8;
const B_P0_NRBW_DBG: u32   = 1 << 30;
const R_P1_DBGMOD: u32     = 0x32B8;
const B_P1_DBGMOD_ON: u32  = 1 << 30;
const R_ANAPAR_PW15: u32   = 0x030C;
const B_ANAPAR_PW15: u32   = 0xFF << 24;
const R_ANAPAR: u32        = 0x032C;
const B_ANAPAR_15: u32     = 0xFFFF << 16;
const B_NCTL_RPT_FLG: u32  = 1 << 26;

// RF RR_TXIG + fields
const RR_TXIG: u8          = 0x11;
const RR_TXIG_GR0: u32     = 0x3;          // [1:0]
const RR_TXIG_GR1: u32     = 0x7 << 4;     // [6:4]
const RR_TXIG_TG: u32      = 0x1F << 12;   // [16:12]

// _iqk_one_shot command IDs (implicit via iqk_cmd encoding)
// FLOK_COARSE  -> 0x108 | (1<<(4+path))
// FLOK_FINE    -> 0x208 | (1<<(4+path))
// FLOK_VBUFFER -> 0x308 | (1<<(4+path))
const IQK_CMD_FLOK_COARSE:  u32 = 0x108;
const IQK_CMD_FLOK_FINE:    u32 = 0x208;
const IQK_CMD_FLOK_VBUFFER: u32 = 0x308;

// ── RF register addresses + bits ─────────────────────────────────
const RR_LUTWE: u8         = 0xEF;
const RR_LUTWE_LOK: u32    = 1 << 2;
const RR_MOD: u8           = 0x00;
const RR_MOD_MASK: u32     = 0xF << 16;
const RR_RSV1: u8          = 0x05;
const RR_RSV1_RST: u32     = 1 << 1;
const RR_BBDC: u8          = 0xBD;
const RR_BBDC_SEL: u32     = 1 << 0;

const RF_BASE_A: u32 = 0xE000;
const RF_BASE_B: u32 = 0xF000;
const RFREG_MASK: u32 = 0xF_FFFF;

fn rf_write(mmio: i32, path: u8, reg: u8, mask: u32, val: u32) {
    let base = if path == 0 { RF_BASE_A } else { RF_BASE_B };
    let addr = CR + base + ((reg as u32) << 2);
    host::mmio_w32_mask(mmio, addr, mask & RFREG_MASK, val);
}

// ═══════════════════════════════════════════════════════════════════
//  _iqk_check_cal — poll NCTL1 status byte for 0x55 (cal done)
//  Linux rfk.c:253 — up to 8.2ms timeout, then reads NCTL_RPT.FLG.
// ═══════════════════════════════════════════════════════════════════
fn iqk_check_cal(mmio: i32, _path: u8) -> bool {
    // Poll 0xbff8 byte 0 for == 0x55 (PHY space, +CR_BASE)
    let mut ok = false;
    for _ in 0..8200u32 {
        let v = host::mmio_r32(mmio, CR + 0xBFF8) & 0xFF;
        if v == 0x55 { ok = true; break; }
    }
    // 200us settle
    host::sleep_ms(1);
    let mut fail = true;
    if ok {
        fail = (host::mmio_r32(mmio, CR + R_NCTL_RPT) & B_NCTL_RPT_FLG) != 0;
    }
    // Clear NCTL_N1 byte 0
    host::mmio_w32_mask(mmio, CR + R_NCTL_N1, 0xFF, 0);
    fail
}

// ═══════════════════════════════════════════════════════════════════
//  _iqk_one_shot — issue calibration command + wait
//  Linux rfk.c:815. We only handle FLOK_COARSE/FINE/VBUFFER here
//  (those are the LOK variants; TXK/RXK variants skipped).
// ═══════════════════════════════════════════════════════════════════
fn iqk_one_shot(mmio: i32, path: u8, base_cmd: u32) -> bool {
    // All FLOK variants set RFCTM.EN = 1 first
    host::mmio_w32_mask(mmio, CR + R_P0_RFCTM, B_P0_RFCTM_EN, 1);
    let iqk_cmd = base_cmd | (1 << (4 + path as u32));
    host::mmio_w32(mmio, CR + R_NCTL_CFG, iqk_cmd + 1);
    // udelay(1) — too small to approximate with sleep_ms
    let fail = iqk_check_cal(mmio, path);
    host::mmio_w32_mask(mmio, CR + R_P0_RFCTM, B_P0_RFCTM_EN, 0);
    fail
}

// ═══════════════════════════════════════════════════════════════════
//  _iqk_txclk_setting — Linux rfk.c:1303
// ═══════════════════════════════════════════════════════════════════
fn iqk_txclk_setting(mmio: i32, _path: u8) {
    host::mmio_w32_mask(mmio, CR + R_P0_NRBW, B_P0_NRBW_DBG, 1);
    host::mmio_w32_mask(mmio, CR + R_P1_DBGMOD, B_P1_DBGMOD_ON, 1);
    host::mmio_w32_mask(mmio, CR + R_ANAPAR_PW15, B_ANAPAR_PW15, 0x1F);
    host::mmio_w32_mask(mmio, CR + R_ANAPAR_PW15, B_ANAPAR_PW15, 0x13);
    host::mmio_w32_mask(mmio, CR + R_ANAPAR, B_ANAPAR_15, 0x0001);
    host::mmio_w32_mask(mmio, CR + R_ANAPAR, B_ANAPAR_15, 0x0041);
}

// ═══════════════════════════════════════════════════════════════════
//  _iqk_lok — 2G branch only (Linux rfk.c:1191)
//  4 sub-calibrations: FLOK_COARSE, FLOK_VBUFFER, FLOK_FINE, FLOK_VBUFFER
// ═══════════════════════════════════════════════════════════════════
fn iqk_lok_2g(mmio: i32, path: u8) {
    host::mmio_w32_mask(mmio, CR + R_IQK_DIF4, B_IQK_DIF4_TXT, 0x021);
    // 2G RF TXIG init: GR0=0, GR1=6
    rf_write(mmio, path, RR_TXIG, RR_TXIG_GR0, 0x0);
    rf_write(mmio, path, RR_TXIG, RR_TXIG_GR1, 0x6);
    // TG = 0
    rf_write(mmio, path, RR_TXIG, RR_TXIG_TG, 0x0);
    // KIP_IQP = 0x9; one-shot FLOK_COARSE
    host::mmio_w32(mmio, CR + R_KIP_IQP + ((path as u32) << 8), 0x9);
    iqk_one_shot(mmio, path, IQK_CMD_FLOK_COARSE);
    // TG = 0x12; KIP_IQP = 0x24; one-shot FLOK_VBUFFER
    rf_write(mmio, path, RR_TXIG, RR_TXIG_TG, 0x12);
    host::mmio_w32(mmio, CR + R_KIP_IQP + ((path as u32) << 8), 0x24);
    iqk_one_shot(mmio, path, IQK_CMD_FLOK_VBUFFER);
    // TG = 0; KIP_IQP = 0x9; DIF4.TXT = 0x021; one-shot FLOK_FINE
    rf_write(mmio, path, RR_TXIG, RR_TXIG_TG, 0x0);
    host::mmio_w32(mmio, CR + R_KIP_IQP + ((path as u32) << 8), 0x9);
    host::mmio_w32_mask(mmio, CR + R_IQK_DIF4, B_IQK_DIF4_TXT, 0x021);
    iqk_one_shot(mmio, path, IQK_CMD_FLOK_FINE);
    // TG = 0x12; KIP_IQP = 0x24; one-shot FLOK_VBUFFER (final)
    rf_write(mmio, path, RR_TXIG, RR_TXIG_TG, 0x12);
    host::mmio_w32(mmio, CR + R_KIP_IQP + ((path as u32) << 8), 0x24);
    iqk_one_shot(mmio, path, IQK_CMD_FLOK_VBUFFER);
}

fn apply_reg3_table(mmio: i32, table: &[(u32, u32, u32)]) {
    for &(addr, mask, val) in table {
        // Linux rtw89_phy_write32_mask writes to PHY-space (+CR_BASE).
        host::mmio_w32_mask(mmio, CR + addr, mask, val);
    }
}

// ═══════════════════════════════════════════════════════════════════
//  Public entry — run the IQK framework for path A + B
// ═══════════════════════════════════════════════════════════════════

pub fn run(mmio: i32) {
    host::print("  IQK: start (tables-only minimum port)\n");

    // _iqk_init (rfk.c:1538) — clear R_IQKINF
    host::mmio_w32(mmio, CR + R_IQKINF, 0);

    // _iqk_macbb_setting for non-DBCC (rfk.c:1514+1529) — apply set table
    apply_reg3_table(mmio, RTW8852B_SET_NONDBCC_PATH01);
    host::print("  IQK: macbb setting applied (32 writes)\n");

    // _iqk_preset per path (rfk.c:1493) — 6 writes per path
    for path in 0u8..2 {
        let off = (path as u32) << 8;
        // COEF_SEL + CFIR_LUT G3 = table_idx (we use 0)
        host::mmio_w32_mask(mmio, CR + R_COEF_SEL + off, B_COEF_SEL_IQC, 0);
        host::mmio_w32_mask(mmio, CR + R_CFIR_LUT + off, B_CFIR_LUT_G3, 0);
        // RF: clear RSV1.RST, clear BBDC.SEL
        rf_write(mmio, path, RR_RSV1, RR_RSV1_RST, 0);
        rf_write(mmio, path, RR_BBDC, RR_BBDC_SEL, 0);
        // BB: NCTL_RPT = 0x80, KIP_SYSCFG = 0x81ff010a
        host::mmio_w32(mmio, CR + R_NCTL_RPT, 0x00000080);
        host::mmio_w32(mmio, CR + R_KIP_SYSCFG, 0x81FF010A);
    }

    // _iqk_by_path per path (rfk.c:1347) — LOK only (2G, 20MHz).
    // Skips TXK/RXK (TX-side calibrations, not needed for pure RX).
    for path in 0u8..2 {
        host::print("  IQK: LOK path ");
        crate::fw::print_dec(path as usize);
        host::print("...\n");
        iqk_txclk_setting(mmio, path);
        iqk_lok_2g(mmio, path);
    }
    host::print("  IQK: LOK complete\n");

    // _iqk_restore per path (rfk.c:1439) — teardown
    for path in 0u8..2 {
        let off = (path as u32) << 8;
        // Write zeros to TXIQC/RXIQC (we have no calibrated nb values yet)
        host::mmio_w32(mmio, CR + R_TXIQC + off, 0);
        host::mmio_w32(mmio, CR + R_RXIQC + off, 0);
        // NCTL_CFG = 0x0000_0e19 + (path << 4)
        host::mmio_w32(mmio, CR + R_NCTL_CFG, 0x00000E19 + ((path as u32) << 4));
        // NCTL_N1 CIP = 0x00
        host::mmio_w32_mask(mmio, CR + R_NCTL_N1, 0xFF, 0);
        // NCTL_RPT = 0
        host::mmio_w32(mmio, CR + R_NCTL_RPT, 0);
        // KIP_SYSCFG = 0x80000000
        host::mmio_w32(mmio, CR + R_KIP_SYSCFG, 0x80000000);
        // CFIR_SYS.IQK_RES_K = 0
        host::mmio_w32_mask(mmio, CR + R_CFIR_SYS, B_IQK_RES_K, 0);
        // IQRSN K1/K2 = 0
        host::mmio_w32_mask(mmio, CR + R_IQRSN, B_IQRSN_K1, 0);
        host::mmio_w32_mask(mmio, CR + R_IQRSN, B_IQRSN_K2, 0);
        // RF: LUTWE.LOK = 0 (twice like Linux), MOD.MASK = 0x3, RSV1.RST = 1, BBDC.SEL = 1
        rf_write(mmio, path, RR_LUTWE, RR_LUTWE_LOK, 0);
        rf_write(mmio, path, RR_LUTWE, RR_LUTWE_LOK, 0);
        rf_write(mmio, path, RR_MOD,   RR_MOD_MASK,  3);
        rf_write(mmio, path, RR_RSV1,  RR_RSV1_RST,  1);
        rf_write(mmio, path, RR_BBDC,  RR_BBDC_SEL,  1);
    }
    host::print("  IQK: restore per path (A+B)\n");

    // _iqk_afebb_restore for non-DBCC (rfk.c:1467+1484) — apply restore table
    apply_reg3_table(mmio, RTW8852B_RESTORE_NONDBCC_PATH01);
    host::print("  IQK: afebb restore applied (18 writes)\n");

    host::print("  IQK: done\n");
}
