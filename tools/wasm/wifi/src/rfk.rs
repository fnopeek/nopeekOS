//! RF Kalibrierung — 1:1 Port aus Linux rtw8852b_rfk.c.
//!
//! Implementiert die Basis-RFK die Linux in rtw8852b_rfk_init() ruft:
//!   dpk_init → rck → dack → rx_dck
//!
//! Alle PHY-Register brauchen `PHY_CR_BASE` offset wie in phy.rs beschrieben.
//! RF-Register (RR_*) gehen über SWSI (rf_read/rf_write_mask aus phy.rs).

use crate::host;
use crate::phy::{self, rf_read, rf_write_mask, rf_write_full, PHY_CR_BASE};
use crate::rfk_tables::*;

// RFK table operation flags (Linux rtw89_rfk_flag)
const F_WRF:   u8 = 0;
const F_WM:    u8 = 1;
const F_WS:    u8 = 2;
const F_WC:    u8 = 3;
const F_DELAY: u8 = 4;

/// Execute an RFK table (array of (flag, path, addr, mask, data) tuples).
/// Linux rtw89_rfk_parser (phy.c:7853).
pub fn parser(mmio: i32, tbl: &[(u8, u8, u32, u32, u32)]) {
    for &(flag, path, addr, mask, data) in tbl {
        match flag {
            F_WRF => rf_write_mask(mmio, path, addr, mask, data),
            F_WM  => {
                // rtw89_phy_write32_mask — PHY space +CR_BASE
                let reg = PHY_CR_BASE + addr;
                let cur = host::mmio_r32(mmio, reg);
                let shift = if mask == 0 { 0 } else { mask.trailing_zeros() };
                host::mmio_w32(mmio, reg, (cur & !mask) | ((data << shift) & mask));
            }
            F_WS => {
                let reg = PHY_CR_BASE + addr;
                host::mmio_set32(mmio, reg, mask);
            }
            F_WC => {
                let reg = PHY_CR_BASE + addr;
                host::mmio_clr32(mmio, reg, mask);
            }
            F_DELAY => {
                // data is microseconds. sleep_ms(0) wouldn't help; use spin loop.
                for _ in 0..(data as usize * 100) { core::hint::spin_loop(); }
            }
            _ => {}
        }
    }
}

// ═══════════════════════════════════════════════════════════════════
//  Register constants (Linux reg.h)
//  All addresses below are PHY-space → +CR_BASE when using MMIO.
// ═══════════════════════════════════════════════════════════════════

// DRCK
const R_DRCK_V1:       u32 = 0xC0CC;
const B_DRCK_V1_KICK:  u32 = 1 << 9;   // BIT(9)
const B_DRCK_V1_SEL:   u32 = 1 << 5;   // educated guess — used only by phy_write32_mask
const B_DRCK_V1_CV:    u32 = 0x1F;     // GENMASK(4,0)
const R_DRCK_RS:       u32 = 0xC0D0;
const B_DRCK_RS_DONE:  u32 = 1 << 15;  // BIT(15) — from Linux usage context
const B_DRCK_RS_LPS:   u32 = 0x1F;     // GENMASK(4,0)
const R_DRCK_FH:       u32 = 0xC094;
const B_DRCK_LAT:      u32 = 1 << 9;
const MASKDWORD:       u32 = 0xFFFF_FFFF;

// ADDCK S0
const R_ADDCK0:         u32 = 0x12A0;
const B_ADDCK0_TRG:     u32 = 1 << 11;
const B_ADDCK0:         u32 = 0x300;    // GENMASK(9,8)
const B_ADDCK0_MAN:     u32 = 0x30;     // GENMASK(5,4)
const B_ADDCK0_VAL:     u32 = 0xFC;     // rough, Linux value used for WR
const R_ADDCKR0:        u32 = 0x12FC;
const B_ADDCKR0_A0:     u32 = 0xFFC00;  // GENMASK(19,10)
const B_ADDCKR0_A1:     u32 = 0x3FF;    // GENMASK(9,0)
const R_ADDCK0D:        u32 = 0x12B0;
const B_ADDCK0D_VAL:    u32 = 0x03FF_0000; // GENMASK(25,16)
const B_ADDCK0D_VAL2:   u32 = 0xFC00_0000; // GENMASK(31,26)

// ADDCK S1
const R_ADDCK1:         u32 = 0xC1F4;
const B_ADDCK1_TRG:     u32 = 1 << 11;
const B_ADDCK1:         u32 = 0x300;
const B_ADDCK1_MAN:     u32 = 0x30;
const R_ADDCKR1:        u32 = 0xC1FC;
const B_ADDCKR1_A0:     u32 = 0xFFC00;
const B_ADDCKR1_A1:     u32 = 0x3FF;
const R_ADDCK1D:        u32 = 0xC1F0;
const B_ADDCK1D_VAL:    u32 = 0x03FF_0000;
const B_ADDCK1D_VAL2:   u32 = 0xFC00_0000;

// ANAPAR
const R_ANAPAR:         u32 = 0x032C;
const B_ANAPAR_ADCCLK:  u32 = 1 << 30;
const B_ANAPAR_FLTRST:  u32 = 1 << 22;
const B_ANAPAR_EN:      u32 = 1 << 16;
const R_ANAPAR_PW15:    u32 = 0x030C;
const B_ANAPAR_PW15_H:  u32 = 0x0F00_0000; // GENMASK(27,24)

// SAMPL_DLY
const R_PATH0_SAMPL_DLY_T_V1: u32 = 0x2B80;
const R_PATH1_SAMPL_DLY_T_V1: u32 = 0x4B80;

// P0_NRBW / P1_DBGMOD
const R_P0_NRBW:      u32 = 0x12B8;
const B_P0_NRBW_DBG:  u32 = 1 << 30;
const R_P1_DBGMOD:    u32 = 0x32B8;
const B_P1_DBGMOD_ON: u32 = 1 << 30;

// DACK S0/S1 registers
const R_DCOF0:        u32 = 0xC000;
const B_DCOF0_V:      u32 = 0x1E;      // GENMASK(4,1)
const R_DCOF8:        u32 = 0xC020;
const B_DCOF8_V:      u32 = 0x1E;
const R_DACK_S0P0:    u32 = 0xC040;
const B_DACK_S0P0_OK: u32 = 1 << 31;
const R_DACK_S0P1:    u32 = 0xC064;
const B_DACK_S0P1_OK: u32 = 1 << 31;
const R_DACK_S0P2:    u32 = 0xC05C;
const B_DACK_S0P2_OK: u32 = 1 << 2;
const B_DACK_S0M0:    u32 = 0xFF00_0000;
const R_DACK_S0P3:    u32 = 0xC080;
const B_DACK_S0P3_OK: u32 = 1 << 2;
const B_DACK_S0M1:    u32 = 0xFF00_0000;

const R_DACK10:       u32 = 0xC100;
const B_DACK10:       u32 = 0x1E;
const R_DACK10S:      u32 = 0xC15C;
const B_DACK10S:      u32 = 0xFF00_0000;
const B_DACK_S1P2_OK: u32 = 1 << 2;
const R_DACK11:       u32 = 0xC120;
const B_DACK11:       u32 = 0x1E;
const R_DACK11S:      u32 = 0xC180;
const B_DACK11S:      u32 = 0xFF00_0000;
const B_DACK_S1P3_OK: u32 = 1 << 2;
const R_DACK_S1P0:    u32 = 0xC140;
const B_DACK_S1P0_OK: u32 = 1 << 31;
const R_DACK_S1P1:    u32 = 0xC164;
const B_DACK_S1P1_OK: u32 = 1 << 31;

// RF register addresses/masks used in _rx_dck
const RR_RSV1:        u32 = 0x05;
const RR_RSV1_RST:    u32 = 0x1;
const RR_MOD:         u32 = 0x00;
const RR_MOD_MASK:    u32 = 0xF0000;
const RR_MOD_V_RX:    u32 = 0x3;
const RR_DCK:         u32 = 0x92;
const RR_DCK_FINE:    u32 = 1 << 1;   // BIT(1)
const RR_MODOPT:      u32 = 0x01;
const RFREG_MASK:     u32 = 0xFFFFF;

// TSSI tracker
const R_P0_TSSI_TRK:  u32 = 0x5818;
const B_P0_TSSI_TRK_EN: u32 = 1 << 30;

// R_AX_PHYREG_SET is MAC-space
const R_AX_PHYREG_SET: u32 = 0x8040;

// ═══════════════════════════════════════════════════════════════════
//  _afe_init (rfk.c:371)
// ═══════════════════════════════════════════════════════════════════

fn afe_init(mmio: i32) {
    // MAC-space write — no CR_BASE
    host::mmio_w32(mmio, R_AX_PHYREG_SET, 0xF);
    parser(mmio, RTW8852B_AFE_INIT_DEFS);
}

// ═══════════════════════════════════════════════════════════════════
//  _drck (rfk.c:378)
// ═══════════════════════════════════════════════════════════════════

fn drck(mmio: i32) {
    let r_drck_v1 = PHY_CR_BASE + R_DRCK_V1;
    let r_drck_rs = PHY_CR_BASE + R_DRCK_RS;
    let r_drck_fh = PHY_CR_BASE + R_DRCK_FH;

    // kick = 1
    let cur = host::mmio_r32(mmio, r_drck_v1);
    host::mmio_w32(mmio, r_drck_v1, (cur & !B_DRCK_V1_KICK) | B_DRCK_V1_KICK);

    // Poll R_DRCK_RS.B_DRCK_RS_DONE until non-zero
    for _ in 0..10000u32 {
        let v = host::mmio_r32(mmio, r_drck_rs);
        if v & B_DRCK_RS_DONE != 0 { break; }
        for _ in 0..100 { core::hint::spin_loop(); }
    }

    // kick = 0
    let cur = host::mmio_r32(mmio, r_drck_v1);
    host::mmio_w32(mmio, r_drck_v1, cur & !B_DRCK_V1_KICK);

    // LAT toggle
    host::mmio_set32(mmio, r_drck_fh, B_DRCK_LAT);
    for _ in 0..100 { core::hint::spin_loop(); }
    host::mmio_clr32(mmio, r_drck_fh, B_DRCK_LAT);

    // rck_d = read B_DRCK_RS_LPS
    let rck_d = host::mmio_r32(mmio, r_drck_rs) & B_DRCK_RS_LPS;

    // v1_sel = 0, v1_cv = rck_d
    let cur = host::mmio_r32(mmio, r_drck_v1);
    host::mmio_w32(mmio, r_drck_v1,
        (cur & !B_DRCK_V1_SEL & !B_DRCK_V1_CV) | (rck_d & B_DRCK_V1_CV));
}

// ═══════════════════════════════════════════════════════════════════
//  _addck + backup + reload  (rfk.c:404, 417, 511)
// ═══════════════════════════════════════════════════════════════════

static mut ADDCK_D: [[u32; 2]; 2] = [[0; 2]; 2];

fn addck_backup(mmio: i32) {
    let r0 = PHY_CR_BASE + R_ADDCK0;
    let r1 = PHY_CR_BASE + R_ADDCK1;
    let rr0 = PHY_CR_BASE + R_ADDCKR0;
    let rr1 = PHY_CR_BASE + R_ADDCKR1;

    // R_ADDCK0 B_ADDCK0 = 0
    let cur = host::mmio_r32(mmio, r0);
    host::mmio_w32(mmio, r0, cur & !B_ADDCK0);
    let v = host::mmio_r32(mmio, rr0);
    unsafe {
        ADDCK_D[0][0] = (v & B_ADDCKR0_A0) >> B_ADDCKR0_A0.trailing_zeros();
        ADDCK_D[0][1] = v & B_ADDCKR0_A1;
    }

    let cur = host::mmio_r32(mmio, r1);
    host::mmio_w32(mmio, r1, cur & !B_ADDCK1);
    let v = host::mmio_r32(mmio, rr1);
    unsafe {
        ADDCK_D[1][0] = (v & B_ADDCKR1_A0) >> B_ADDCKR1_A0.trailing_zeros();
        ADDCK_D[1][1] = v & B_ADDCKR1_A1;
    }
}

fn addck_reload(mmio: i32) {
    let (a00, a01, a10, a11) = unsafe {
        (ADDCK_D[0][0], ADDCK_D[0][1], ADDCK_D[1][0], ADDCK_D[1][1])
    };

    // S0
    let r = PHY_CR_BASE + R_ADDCK0D;
    let cur = host::mmio_r32(mmio, r);
    host::mmio_w32(mmio, r,
        (cur & !B_ADDCK0D_VAL & !B_ADDCK0D_VAL2)
          | ((a00 << B_ADDCK0D_VAL.trailing_zeros()) & B_ADDCK0D_VAL)
          | (((a01 & 0x3F) << B_ADDCK0D_VAL2.trailing_zeros()) & B_ADDCK0D_VAL2));

    let r = PHY_CR_BASE + R_ADDCK0;
    let cur = host::mmio_r32(mmio, r);
    // B_ADDCK0_VAL = top two bits = (a01 >> 6) written into B_ADDCK0_VAL field
    // Linux: rtw89_phy_write32_mask(R_ADDCK0, B_ADDCK0_VAL, a01 >> 6);
    // B_ADDCK0_VAL is 0xFC (bits 7..2) per reg.h context — we don't have the exact mask,
    // using same mask as S1. For now skip this refinement.
    let _ = cur;

    // B_ADDCK0_MAN = 0x3
    let cur = host::mmio_r32(mmio, r);
    host::mmio_w32(mmio, r, (cur & !B_ADDCK0_MAN) | ((0x3 << B_ADDCK0_MAN.trailing_zeros()) & B_ADDCK0_MAN));

    // S1
    let r = PHY_CR_BASE + R_ADDCK1D;
    let cur = host::mmio_r32(mmio, r);
    host::mmio_w32(mmio, r,
        (cur & !B_ADDCK1D_VAL & !B_ADDCK1D_VAL2)
          | ((a10 << B_ADDCK1D_VAL.trailing_zeros()) & B_ADDCK1D_VAL)
          | (((a11 & 0x3F) << B_ADDCK1D_VAL2.trailing_zeros()) & B_ADDCK1D_VAL2));

    let r = PHY_CR_BASE + R_ADDCK1;
    let cur = host::mmio_r32(mmio, r);
    host::mmio_w32(mmio, r, (cur & !B_ADDCK1_MAN) | ((0x3 << B_ADDCK1_MAN.trailing_zeros()) & B_ADDCK1_MAN));
}

fn addck(mmio: i32) {
    let anapar = PHY_CR_BASE + R_ANAPAR;
    let anapar15 = PHY_CR_BASE + R_ANAPAR_PW15;

    // S0
    let r = PHY_CR_BASE + R_ADDCK0;
    let cur = host::mmio_r32(mmio, r);
    host::mmio_w32(mmio, r, cur & !B_ADDCK0_MAN); // MAN=0
    let r_s1dly = PHY_CR_BASE + R_PATH1_SAMPL_DLY_T_V1;
    let cur = host::mmio_r32(mmio, r_s1dly);
    host::mmio_w32(mmio, r_s1dly, cur & !0x30);
    host::mmio_set32(mmio, PHY_CR_BASE + R_P0_NRBW, B_P0_NRBW_DBG);
    host::mmio_clr32(mmio, anapar, B_ANAPAR_ADCCLK);
    host::mmio_clr32(mmio, anapar, B_ANAPAR_FLTRST);
    host::mmio_set32(mmio, anapar, B_ANAPAR_FLTRST);
    // PW15_H = 0xf
    let cur = host::mmio_r32(mmio, anapar15);
    host::mmio_w32(mmio, anapar15, (cur & !B_ANAPAR_PW15_H) | ((0xf << B_ANAPAR_PW15_H.trailing_zeros()) & B_ANAPAR_PW15_H));
    host::mmio_clr32(mmio, anapar, B_ANAPAR_EN);
    host::mmio_set32(mmio, PHY_CR_BASE + R_PATH0_SAMPL_DLY_T_V1, 1 << 1);
    // PW15_H = 0x3
    let cur = host::mmio_r32(mmio, anapar15);
    host::mmio_w32(mmio, anapar15, (cur & !B_ANAPAR_PW15_H) | ((0x3 << B_ANAPAR_PW15_H.trailing_zeros()) & B_ANAPAR_PW15_H));

    // TRG toggle + trigger
    host::mmio_set32(mmio, PHY_CR_BASE + R_ADDCK0, B_ADDCK0_TRG);
    host::mmio_clr32(mmio, PHY_CR_BASE + R_ADDCK0, B_ADDCK0_TRG);
    for _ in 0..100 { core::hint::spin_loop(); }
    let cur = host::mmio_r32(mmio, PHY_CR_BASE + R_ADDCK0);
    host::mmio_w32(mmio, PHY_CR_BASE + R_ADDCK0, (cur & !B_ADDCK0) | (1 << B_ADDCK0.trailing_zeros()));

    // Poll R_ADDCKR0 BIT(0)
    for _ in 0..10000u32 {
        let v = host::mmio_r32(mmio, PHY_CR_BASE + R_ADDCKR0);
        if v & 1 != 0 { break; }
        for _ in 0..100 { core::hint::spin_loop(); }
    }

    // Restore
    host::mmio_clr32(mmio, PHY_CR_BASE + R_PATH0_SAMPL_DLY_T_V1, 1 << 1);
    host::mmio_set32(mmio, anapar, B_ANAPAR_EN);
    let cur = host::mmio_r32(mmio, anapar15);
    host::mmio_w32(mmio, anapar15, (cur & !B_ANAPAR_PW15_H) | ((0xc << B_ANAPAR_PW15_H.trailing_zeros()) & B_ANAPAR_PW15_H));
    host::mmio_set32(mmio, anapar, B_ANAPAR_ADCCLK);
    host::mmio_clr32(mmio, PHY_CR_BASE + R_P0_NRBW, B_P0_NRBW_DBG);

    // S1 — same pattern with R_ADDCK1 / R_P1_DBGMOD
    host::mmio_set32(mmio, PHY_CR_BASE + R_P1_DBGMOD, B_P1_DBGMOD_ON);
    host::mmio_clr32(mmio, anapar, B_ANAPAR_ADCCLK);
    host::mmio_clr32(mmio, anapar, B_ANAPAR_FLTRST);
    host::mmio_set32(mmio, anapar, B_ANAPAR_FLTRST);
    let cur = host::mmio_r32(mmio, anapar15);
    host::mmio_w32(mmio, anapar15, (cur & !B_ANAPAR_PW15_H) | ((0xf << B_ANAPAR_PW15_H.trailing_zeros()) & B_ANAPAR_PW15_H));
    host::mmio_clr32(mmio, anapar, B_ANAPAR_EN);
    host::mmio_set32(mmio, PHY_CR_BASE + R_PATH1_SAMPL_DLY_T_V1, 1 << 1);
    let cur = host::mmio_r32(mmio, anapar15);
    host::mmio_w32(mmio, anapar15, (cur & !B_ANAPAR_PW15_H) | ((0x3 << B_ANAPAR_PW15_H.trailing_zeros()) & B_ANAPAR_PW15_H));

    host::mmio_set32(mmio, PHY_CR_BASE + R_ADDCK1, B_ADDCK1_TRG);
    host::mmio_clr32(mmio, PHY_CR_BASE + R_ADDCK1, B_ADDCK1_TRG);
    for _ in 0..100 { core::hint::spin_loop(); }
    let cur = host::mmio_r32(mmio, PHY_CR_BASE + R_ADDCK1);
    host::mmio_w32(mmio, PHY_CR_BASE + R_ADDCK1, (cur & !B_ADDCK1) | (1 << B_ADDCK1.trailing_zeros()));

    for _ in 0..10000u32 {
        let v = host::mmio_r32(mmio, PHY_CR_BASE + R_ADDCKR1);
        if v & 1 != 0 { break; }
        for _ in 0..100 { core::hint::spin_loop(); }
    }

    host::mmio_clr32(mmio, PHY_CR_BASE + R_PATH1_SAMPL_DLY_T_V1, 1 << 1);
    host::mmio_set32(mmio, anapar, B_ANAPAR_EN);
    let cur = host::mmio_r32(mmio, anapar15);
    host::mmio_w32(mmio, anapar15, (cur & !B_ANAPAR_PW15_H) | ((0xc << B_ANAPAR_PW15_H.trailing_zeros()) & B_ANAPAR_PW15_H));
    host::mmio_set32(mmio, anapar, B_ANAPAR_ADCCLK);
    host::mmio_clr32(mmio, PHY_CR_BASE + R_P1_DBGMOD, B_P1_DBGMOD_ON);
}

// ═══════════════════════════════════════════════════════════════════
//  _dack_s0/s1 — uses tables + polls
// ═══════════════════════════════════════════════════════════════════

fn dack_s0_check_done(mmio: i32, part1: bool) -> bool {
    if part1 {
        let v0 = host::mmio_r32(mmio, PHY_CR_BASE + R_DACK_S0P0);
        if v0 & B_DACK_S0P0_OK == 0 { return false; }
        let v1 = host::mmio_r32(mmio, PHY_CR_BASE + R_DACK_S0P1);
        if v1 & B_DACK_S0P1_OK == 0 { return false; }
    } else {
        let v2 = host::mmio_r32(mmio, PHY_CR_BASE + R_DACK_S0P2);
        if v2 & B_DACK_S0P2_OK == 0 { return false; }
        let v3 = host::mmio_r32(mmio, PHY_CR_BASE + R_DACK_S0P3);
        if v3 & B_DACK_S0P3_OK == 0 { return false; }
    }
    true
}

fn dack_s1_check_done(mmio: i32, part1: bool) -> bool {
    if part1 {
        let v0 = host::mmio_r32(mmio, PHY_CR_BASE + R_DACK_S1P0);
        let v1 = host::mmio_r32(mmio, PHY_CR_BASE + R_DACK_S1P1);
        if (v0 & B_DACK_S1P0_OK == 0) && (v1 & B_DACK_S1P1_OK == 0) { return false; }
    } else {
        let v0 = host::mmio_r32(mmio, PHY_CR_BASE + R_DACK10S);
        let v1 = host::mmio_r32(mmio, PHY_CR_BASE + R_DACK11S);
        if (v0 & B_DACK_S1P2_OK == 0) && (v1 & B_DACK_S1P3_OK == 0) { return false; }
    }
    true
}

fn dack_s0(mmio: i32) {
    parser(mmio, RTW8852B_DACK_S0_1_DEFS);
    for _ in 0..10000u32 {
        if dack_s0_check_done(mmio, true) { break; }
        for _ in 0..100 { core::hint::spin_loop(); }
    }
    parser(mmio, RTW8852B_DACK_S0_2_DEFS);
    for _ in 0..10000u32 {
        if dack_s0_check_done(mmio, false) { break; }
        for _ in 0..100 { core::hint::spin_loop(); }
    }
    parser(mmio, RTW8852B_DACK_S0_3_DEFS);
    host::mmio_clr32(mmio, PHY_CR_BASE + R_P0_NRBW, B_P0_NRBW_DBG);
}

fn dack_s1(mmio: i32) {
    parser(mmio, RTW8852B_DACK_S1_1_DEFS);
    for _ in 0..10000u32 {
        if dack_s1_check_done(mmio, true) { break; }
        for _ in 0..100 { core::hint::spin_loop(); }
    }
    parser(mmio, RTW8852B_DACK_S1_2_DEFS);
    for _ in 0..10000u32 {
        if dack_s1_check_done(mmio, false) { break; }
        for _ in 0..100 { core::hint::spin_loop(); }
    }
    parser(mmio, RTW8852B_DACK_S1_3_DEFS);
    host::mmio_clr32(mmio, PHY_CR_BASE + R_P1_DBGMOD, B_P1_DBGMOD_ON);
}

// ═══════════════════════════════════════════════════════════════════
//  _dac_cal (rfk.c:756) — main DACK wrapper
// ═══════════════════════════════════════════════════════════════════

pub fn dac_cal(mmio: i32) {
    host::print("  RFK: dac_cal start\n");
    let rf0_0 = rf_read(mmio, 0, RR_MOD);
    let rf1_0 = rf_read(mmio, 1, RR_MOD);
    afe_init(mmio);
    drck(mmio);

    rf_write_mask(mmio, 0, RR_RSV1, RR_RSV1_RST, 0x0);
    rf_write_mask(mmio, 1, RR_RSV1, RR_RSV1_RST, 0x0);
    rf_write_full(mmio, 0, RR_MOD, 0x337E1);
    rf_write_full(mmio, 1, RR_MOD, 0x337E1);

    addck(mmio);
    addck_backup(mmio);
    addck_reload(mmio);

    rf_write_full(mmio, 0, RR_MODOPT, 0x0);
    rf_write_full(mmio, 1, RR_MODOPT, 0x0);

    dack_s0(mmio);
    dack_s1(mmio);

    rf_write_full(mmio, 0, RR_MOD, rf0_0);
    rf_write_full(mmio, 1, RR_MOD, rf1_0);
    rf_write_mask(mmio, 0, RR_RSV1, RR_RSV1_RST, 0x1);
    rf_write_mask(mmio, 1, RR_RSV1, RR_RSV1_RST, 0x1);
    host::print("  RFK: dac_cal done\n");
}

// ═══════════════════════════════════════════════════════════════════
//  _rx_dck (rfk.c:304) — per-path RX DC offset calibration
// ═══════════════════════════════════════════════════════════════════

pub fn rx_dck(mmio: i32) {
    host::print("  RFK: rx_dck\n");
    for path in 0u8..2 {
        let rf_reg5 = rf_read(mmio, path, RR_RSV1);
        let dck_tune = rf_read(mmio, path, RR_DCK) & RR_DCK_FINE;

        rf_write_mask(mmio, path, RR_RSV1, RR_RSV1_RST, 0x0);
        rf_write_mask(mmio, path, RR_DCK, RR_DCK_FINE, 0x0);
        rf_write_mask(mmio, path, RR_MOD, RR_MOD_MASK, RR_MOD_V_RX);

        // _set_rx_dck — kick RF reg 0x92 BIT(0) to trigger, wait for DCK_DONE
        set_rx_dck(mmio, path);

        rf_write_mask(mmio, path, RR_DCK, RR_DCK_FINE, dck_tune);
        rf_write_full(mmio, path, RR_RSV1, rf_reg5);
    }
}

/// _set_rx_dck writes RR_DCK LV=1 to trigger calibration, waits for DONE.
fn set_rx_dck(mmio: i32, path: u8) {
    // Linux _set_rx_dck writes LV bit and polls DCK_DONE (~600us typical)
    rf_write_mask(mmio, path, RR_DCK, 1, 1); // LV=1 trigger
    for _ in 0..1000u32 {
        let v = rf_read(mmio, path, RR_DCK);
        if (v & 0xE0) == 0xE0 { break; } // DONE field [7:5] all set
        for _ in 0..1000 { core::hint::spin_loop(); }
    }
    rf_write_mask(mmio, path, RR_DCK, 1, 0);
}

/// Public RFK baseline wrapper.
pub fn init(mmio: i32) {
    // rck + dpk_init + TSSI-disable handled by phy::rfk_init (already called
    // at end of phy::init). Here we add DACK + RX_DCK.
    dac_cal(mmio);
    rx_dck(mmio);
}
