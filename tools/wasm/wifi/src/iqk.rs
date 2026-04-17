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
const R_IQKINF: u32        = 0x8190;
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
