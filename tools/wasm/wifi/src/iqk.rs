//! IQK — IQ Imbalance Calibration — full 1:1 port of Linux rtw8852b_rfk.c.
//!
//! Linux entry: rtw8852b_rfk.c:3757 rtw8852b_iqk → _iqk → _doiqk → _iqk_by_path
//!
//! Coverage (1:1 with Linux, 2G + 5G branches both present):
//!   _iqk_init           (rfk.c:1538)
//!   _iqk_macbb_setting  (rfk.c:1514) — apply RTW8852B_SET_NONDBCC_PATH01
//!   _iqk_preset         (rfk.c:1493)
//!   _iqk_txclk_setting  (rfk.c:1303)
//!   _iqk_rxclk_setting  (rfk.c:977)
//!   _iqk_rxk_setting    (rfk.c:792)
//!   _iqk_txk_setting    (rfk.c:1273)
//!   _lok_res_table      (rfk.c:1127)
//!   _lok_finetune_check (rfk.c:1147)
//!   _iqk_lok            (rfk.c:1191)  — 3x retry via _iqk_by_path
//!   _txk_group_sel      (rfk.c:1016)
//!   _rxk_group_sel      (rfk.c:870)
//!   _iqk_nbtxk          (rfk.c:1079)
//!   _iqk_nbrxk          (rfk.c:928)
//!   _iqk_one_shot       (rfk.c:815)   — all ktypes
//!   _iqk_check_cal      (rfk.c:253)
//!   _iqk_restore        (rfk.c:1439)
//!   _iqk_afebb_restore  (rfk.c:1467) — apply RTW8852B_RESTORE_NONDBCC_PATH01
//!
//! Linux is_nbiqk defaults to false → we run _txk_group_sel + _rxk_group_sel
//! (wide-band, 4 group iterations each), matching Linux behaviour.

use crate::host;
use crate::fw;
use crate::phy::{rf_read, rf_write_mask};
use crate::iqk_tables::{
    RTW8852B_SET_NONDBCC_PATH01,
    RTW8852B_RESTORE_NONDBCC_PATH01,
};

const CR: u32 = 0x10000; // PHY_CR_BASE

// ── PHY register addresses (reg.h — verified 1:1) ─────────────────
const R_IQKINF: u32        = 0x9FE0;
const B_IQKINF_VER: u32    = 0xFF << 24;
const B_IQKINF_F_COR: u32  = 1 << 0;
const B_IQKINF_F_FIN: u32  = 1 << 1;
const B_IQKINF_F_TX: u32   = 1 << 2;
const B_IQKINF_F_RX: u32   = 1 << 3;

const R_IQKCH: u32         = 0x9FE4;
const B_IQKCH_BAND: u32    = 0xF << 0;
const B_IQKCH_BW: u32      = 0xF << 4;
const B_IQKCH_CH: u32      = 0xFF << 8;

const R_NCTL_CFG: u32      = 0x8000;
const R_NCTL_RPT: u32      = 0x8008;
const B_NCTL_RPT_FLG: u32  = 1 << 26;
const R_NCTL_N1: u32       = 0x8010; // was WRONG (0x8004) in previous version
const B_NCTL_N1_CIP: u32   = 0xFF << 0;

const R_IQK_DIF4: u32      = 0x802C;
const B_IQK_DIF4_TXT: u32  = 0xFFF << 0;
const B_IQK_DIF4_RXT: u32  = 0xFFF << 16;

const R_KIP_SYSCFG: u32    = 0x8088; // was WRONG (0x8240) in previous version
const R_COEF_SEL: u32      = 0x8104;       // + path<<8
const B_COEF_SEL_IQC: u32  = 1 << 0;

const R_CFIR_SYS: u32      = 0x8120;
const B_IQK_RES_K: u32     = 1 << 28;
const R_IQK_RES: u32       = 0x8124;       // + path<<8
const B_IQK_RES_TXCFIR: u32 = 0xF << 8;
const B_IQK_RES_RXCFIR: u32 = 0xF << 0;

const R_TXIQC: u32         = 0x8138;       // + path<<8 — was WRONG (0x81D8)
const R_RXIQC: u32         = 0x813C;       // + path<<8 — was WRONG (0x8220)

const R_CFIR_LUT: u32      = 0x8154;       // + path<<8
const B_CFIR_LUT_SEL: u32  = 1 << 8;
const B_CFIR_LUT_SET: u32  = 1 << 4;
const B_CFIR_LUT_G3: u32   = 1 << 3;       // was WRONG (1<<20)
const B_CFIR_LUT_G2: u32   = 1 << 2;
const B_CFIR_LUT_GP_V1: u32 = 0x7 << 0;    // 3-bit (RXK uses this)
const B_CFIR_LUT_GP: u32   = 0x3 << 0;     // 2-bit (TXK uses this)

const R_KIP_IQP: u32       = 0x81CC;       // + path<<8
const R_IQRSN: u32         = 0x8220;
const B_IQRSN_K1: u32      = 1 << 28;
const B_IQRSN_K2: u32      = 1 << 16;

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
const R_P0_RXCK: u32       = 0x12A0;
const B_P0_RXCK_ON: u32    = 1 << 19;
const B_P0_RXCK_VAL: u32   = 0x7 << 16;
const R_P1_RXCK: u32       = 0x32A0;
const B_P1_RXCK_ON: u32    = 1 << 19;
const B_P1_RXCK_VAL: u32   = 0x7 << 16;
const R_UPD_CLK_ADC: u32   = 0x0700;
const B_UPD_CLK_ADC_ON: u32  = 1 << 24;
const B_UPD_CLK_ADC_VAL: u32 = 0x3 << 25;

const R_RFK_ST: u32        = 0xBFF8;       // cal status poll

// ── RF register addresses (reg.h — u32 with high bank bits) ───────
const RR_MOD: u32          = 0x00;
const RR_MOD_IQK: u32      = 0xFFFF0;      // GENMASK(19,4)
const RR_MOD_MASK: u32     = 0xF0000;      // GENMASK(19,16)
const RR_MOD_RGM: u32      = 0x3FF0;       // GENMASK(13,4)
const RR_RSV1: u32         = 0x05;
const RR_RSV1_RST: u32     = 1 << 0;
const RR_LOKVB: u32        = 0x0A;
const RR_LOKVB_COI: u32    = 0x3F << 14;   // GENMASK(19,14)
const RR_LOKVB_COQ: u32    = 0x3F << 4;    // GENMASK(9,4)
const RR_TXIG: u32         = 0x11;
const RR_TXIG_GR0: u32     = 0x3;          // [1:0]
const RR_TXIG_GR1: u32     = 0x7 << 4;     // [6:4]
const RR_TXIG_TG: u32      = 0x1F << 12;   // [16:12]
const RR_CFGCH: u32        = 0x18;
const RR_RXKPLL: u32       = 0x1E;
const RR_RSV4: u32         = 0x1F;
const RR_RXK: u32          = 0x20;
const RR_RXK_SEL2G: u32    = 1 << 8;
const RR_RXK_SEL5G: u32    = 1 << 7;
const RR_LUTWA: u32        = 0x33;
const RR_LUTWA_M1: u32     = 0xFF;
const RR_LUTWD0: u32       = 0x3F;
const RR_TXG1: u32         = 0x51;
const RR_TXG1_ATT2: u32    = 1 << 19;
const RR_TXG1_ATT1: u32    = 1 << 11;
const RR_TXG2: u32         = 0x52;
const RR_TXG2_ATT0: u32    = 1 << 11;
const RR_TXGA: u32         = 0x55;
const RR_TXGA_LOK_EXT: u32 = 0x1F << 0;    // GENMASK(4,0)
const RR_TXMO: u32         = 0x58;
const RR_TXMO_COI: u32     = 0x1F << 15;   // GENMASK(19,15)
const RR_TXMO_COQ: u32     = 0x1F << 10;   // GENMASK(14,10)
const RR_BIASA: u32        = 0x60;
const RR_BIASA_A: u32      = 0x7 << 0;     // GENMASK(2,0)
const RR_TXVBUF: u32       = 0x7C;
const RR_TXVBUF_DACEN: u32 = 1 << 5;
const RR_RXBB: u32         = 0x83;
const RR_RXBB_C2G: u32     = 0x7F << 10;   // GENMASK(16,10)
const RR_RXBB_C1G: u32     = 0x3 << 8;     // GENMASK(9,8)
const RR_XGLNA2: u32       = 0x85;
const RR_XGLNA2_SW: u32    = 0x3 << 0;     // GENMASK(1,0)
const RR_RXA2: u32         = 0x8C;
const RR_RXA2_HATT: u32    = 0x7F;         // GENMASK(6,0)
const RR_RXA2_CC2: u32     = 0x3 << 7;     // GENMASK(8,7)
const RR_XALNA2: u32       = 0x90;
const RR_XALNA2_SW2: u32   = 0x3 << 8;     // GENMASK(9,8)
const RR_LUTWE: u32        = 0xEF;
const RR_LUTWE_LOK: u32    = 1 << 2;
const RR_BBDC: u32         = 0x10005;      // bit16 = ad_sel=1 (direct MMIO)
const RR_BBDC_SEL: u32     = 1 << 0;

const RFREG_MASK: u32      = 0xF_FFFF;

// ── IQK one-shot command IDs (Linux rtw8852b_iqk_type enum) ───────
const ID_FLOK_COARSE:  u8  = 0x1;
const ID_FLOK_FINE:    u8  = 0x2;
const ID_TXK:          u8  = 0x3;
const ID_RXK:          u8  = 0x5;
const ID_NBTXK:        u8  = 0x6;
const ID_NBRXK:        u8  = 0x7;
const ID_FLOK_VBUFFER: u8  = 0x8;

// ── Group constants (Linux rtw8852b_rfk.c:98..111) ────────────────
const RXK_GROUP_NR: usize = 4;
const G_IDXRXGAIN: [u32; RXK_GROUP_NR] = [0x212, 0x21C, 0x350, 0x360];
const G_IDXATTC2:  [u32; RXK_GROUP_NR] = [0x00, 0x00, 0x28, 0x5F];
const G_IDXATTC1:  [u32; RXK_GROUP_NR] = [0x3, 0x3, 0x2, 0x1];
const A_IDXRXGAIN: [u32; RXK_GROUP_NR] = [0x190, 0x198, 0x350, 0x352];
const A_IDXATTC2:  [u32; RXK_GROUP_NR] = [0x0F, 0x0F, 0x3F, 0x7F];
const A_IDXATTC1:  [u32; RXK_GROUP_NR] = [0x3, 0x1, 0x0, 0x0];
const G_POWER_RNG: [u32; RXK_GROUP_NR] = [0x0, 0x0, 0x0, 0x0];
const G_TRACK_RNG: [u32; RXK_GROUP_NR] = [0x4, 0x4, 0x6, 0x6];
const G_GAIN_BB:   [u32; RXK_GROUP_NR] = [0x08, 0x0E, 0x06, 0x0E];
const G_ITQT:      [u32; RXK_GROUP_NR] = [0x09, 0x12, 0x1B, 0x24];
const A_POWER_RNG: [u32; RXK_GROUP_NR] = [0x0, 0x0, 0x0, 0x0];
const A_TRACK_RNG: [u32; RXK_GROUP_NR] = [0x3, 0x3, 0x6, 0x6];
const A_GAIN_BB:   [u32; RXK_GROUP_NR] = [0x08, 0x0E, 0x06, 0x0E];
const A_ITQT:      [u32; RXK_GROUP_NR] = [0x12, 0x12, 0x12, 0x1B];

const IQK_VER: u32 = 0x2A;
const BAND_2G: u8 = 0;
const BAND_5G: u8 = 1;
const BW_20M: u8 = 0;

// ── IQK state (per path) ──────────────────────────────────────────
// Mirrors the subset of rtw89_iqk_info that _iqk_by_path + sub-funcs read/write.
struct IqkState {
    band: [u8; 2],   // iqk_band[path]  (BAND_2G/5G)
    bw: [u8; 2],     // iqk_bw[path]    (BW_20M etc.)
    ch: [u8; 2],     // iqk_ch[path]
    lok_cor_fail: [bool; 2],
    lok_fin_fail: [bool; 2],
    tx_fail: [bool; 2],
    rx_fail: [bool; 2],
    nb_txcfir: [u32; 2],
    nb_rxcfir: [u32; 2],
    _is_wb_txiqk: [bool; 2],
    _is_wb_rxiqk: [bool; 2],
}

// ── BB/RF backup tables (Linux rtw8852b_backup_bb_regs / _rf_regs) ─
// IQK corrupts these; Linux _doiqk backs them up before and restores after.
// Skipping this step leaves TXPW_RSTB + R_RXCCA + per-path RF in IQK state
// after IQK finishes → RX pipe never recovers. rfk.c:113.
const BACKUP_BB_REGS: [u32; 3] = [0x2344, 0x5800, 0x7800];
// 11 RF registers backed up per path. 0x10005 has bit16 (ad_sel=1).
const BACKUP_RF_REGS: [u32; 11] = [
    0xDE, 0xDF, 0x8B, 0x90, 0x97, 0x85, 0x1E, 0x00, 0x02, 0x05, 0x10005,
];

// ═══════════════════════════════════════════════════════════════════
//  Helpers
// ═══════════════════════════════════════════════════════════════════

// Each iteration includes an MMIO read (~300-500ns on PCIe) plus spin_loops.
// Linux uses udelay(1) = real 1 microsecond. Our old 100×spin_loop was
// ~100-300ns — 5-10× too short. Bumped to 1000 spin_loops for ~1-3µs floor;
// combined with the MMIO read this stays >= 1µs per iteration.
fn udelay_1() { for _ in 0..1000 { core::hint::spin_loop(); } }
fn udelay_200() { for _ in 0..200000 { core::hint::spin_loop(); } }

fn backup_bb(mmio: i32, out: &mut [u32; 3]) {
    for (i, &addr) in BACKUP_BB_REGS.iter().enumerate() {
        out[i] = host::mmio_r32(mmio, CR + addr);
    }
}
fn restore_bb(mmio: i32, saved: &[u32; 3]) {
    for (i, &addr) in BACKUP_BB_REGS.iter().enumerate() {
        host::mmio_w32(mmio, CR + addr, saved[i]);
    }
}
fn backup_rf(mmio: i32, path: u8, out: &mut [u32; 11]) {
    for (i, &addr) in BACKUP_RF_REGS.iter().enumerate() {
        out[i] = rf_read(mmio, path, addr);
    }
}
fn restore_rf(mmio: i32, path: u8, saved: &[u32; 11]) {
    for (i, &addr) in BACKUP_RF_REGS.iter().enumerate() {
        rf_write_mask(mmio, path, addr, RFREG_MASK, saved[i]);
    }
}

/// _wait_rx_mode — poll RR_MOD.MOD_MASK until != 2 (TX). Linux rfk.c:1569.
/// Aborts IQK if the chip is stuck in TX mode at IQK entry.
pub fn wait_rx_mode_pub(mmio: i32) { wait_rx_mode(mmio); }

fn wait_rx_mode(mmio: i32) {
    for path in 0u8..2 {
        for _ in 0..5000u32 {
            let v = (rf_read(mmio, path, RR_MOD) & RR_MOD_MASK) >> RR_MOD_MASK.trailing_zeros();
            if v != 2 { break; }
            udelay_1();
        }
    }
}

fn pw(mmio: i32, addr: u32, val: u32) {
    host::mmio_w32(mmio, CR + addr, val);
}
fn pwm(mmio: i32, addr: u32, mask: u32, val: u32) {
    host::mmio_w32_mask(mmio, CR + addr, mask, val);
}
fn pr(mmio: i32, addr: u32) -> u32 {
    host::mmio_r32(mmio, CR + addr)
}
fn rw(mmio: i32, path: u8, addr: u32, mask: u32, val: u32) {
    rf_write_mask(mmio, path, addr, mask, val);
}
fn rr(mmio: i32, path: u8, addr: u32) -> u32 {
    rf_read(mmio, path, addr)
}

// ═══════════════════════════════════════════════════════════════════
//  _iqk_check_cal — Linux rfk.c:253
// ═══════════════════════════════════════════════════════════════════
fn iqk_check_cal(mmio: i32, path: u8) -> bool {
    let mut ok = false;
    // Linux: read_poll_timeout_atomic(1us, 8200us). Our udelay_1 may still
    // undershoot, so give the loop more iterations (20000) to guarantee
    // the calibration has at least ~20ms of wall clock to settle.
    for _ in 0..20000u32 {
        let v = pr(mmio, R_RFK_ST) & 0xFF;
        if v == 0x55 { ok = true; break; }
        udelay_1();
    }
    udelay_200();
    let fail = if ok {
        (pr(mmio, R_NCTL_RPT) & B_NCTL_RPT_FLG) != 0
    } else {
        true
    };
    pwm(mmio, R_NCTL_N1, 0xFF, 0);
    let _ = path;
    fail
}

// ═══════════════════════════════════════════════════════════════════
//  _iqk_one_shot — Linux rfk.c:815, all ktypes
// ═══════════════════════════════════════════════════════════════════
fn iqk_one_shot(mmio: i32, state: &IqkState, path: u8, ktype: u8) -> bool {
    let iqk_cmd: u32;
    let bw = state.bw[path as usize] as u32;

    match ktype {
        ID_FLOK_COARSE => {
            pwm(mmio, R_P0_RFCTM, B_P0_RFCTM_EN, 1);
            iqk_cmd = 0x108 | (1 << (4 + path as u32));
        }
        ID_FLOK_FINE => {
            pwm(mmio, R_P0_RFCTM, B_P0_RFCTM_EN, 1);
            iqk_cmd = 0x208 | (1 << (4 + path as u32));
        }
        ID_FLOK_VBUFFER => {
            pwm(mmio, R_P0_RFCTM, B_P0_RFCTM_EN, 1);
            iqk_cmd = 0x308 | (1 << (4 + path as u32));
        }
        ID_TXK => {
            pwm(mmio, R_P0_RFCTM, B_P0_RFCTM_EN, 0);
            iqk_cmd = 0x008 | (1 << (path as u32 + 4)) | (((0x8 + bw) & 0xF) << 8);
        }
        ID_RXK => {
            pwm(mmio, R_P0_RFCTM, B_P0_RFCTM_EN, 1);
            iqk_cmd = 0x008 | (1 << (path as u32 + 4)) | (((0xB + bw) & 0xF) << 8);
        }
        ID_NBTXK => {
            pwm(mmio, R_P0_RFCTM, B_P0_RFCTM_EN, 0);
            pwm(mmio, R_IQK_DIF4, B_IQK_DIF4_TXT, 0x011);
            iqk_cmd = 0x408 | (1 << (4 + path as u32));
        }
        ID_NBRXK => {
            pwm(mmio, R_P0_RFCTM, B_P0_RFCTM_EN, 1);
            pwm(mmio, R_IQK_DIF4, B_IQK_DIF4_RXT, 0x011);
            iqk_cmd = 0x608 | (1 << (4 + path as u32));
        }
        _ => return false,
    }

    pw(mmio, R_NCTL_CFG, iqk_cmd + 1);
    udelay_1();
    let fail = iqk_check_cal(mmio, path);
    pwm(mmio, R_P0_RFCTM, B_P0_RFCTM_EN, 0);
    fail
}

// ═══════════════════════════════════════════════════════════════════
//  _iqk_txclk_setting — rfk.c:1303
// ═══════════════════════════════════════════════════════════════════
fn iqk_txclk_setting(mmio: i32, _path: u8) {
    pwm(mmio, R_P0_NRBW,     B_P0_NRBW_DBG,  1);
    pwm(mmio, R_P1_DBGMOD,   B_P1_DBGMOD_ON, 1);
    udelay_1();
    pwm(mmio, R_ANAPAR_PW15, B_ANAPAR_PW15,  0x1F);
    udelay_1();
    pwm(mmio, R_ANAPAR_PW15, B_ANAPAR_PW15,  0x13);
    pwm(mmio, R_ANAPAR,      B_ANAPAR_15,    0x0001);
    udelay_1();
    pwm(mmio, R_ANAPAR,      B_ANAPAR_15,    0x0041);
}

// ═══════════════════════════════════════════════════════════════════
//  _iqk_rxclk_setting — rfk.c:977 (20M/40M branch — BW_20M here)
// ═══════════════════════════════════════════════════════════════════
fn iqk_rxclk_setting(mmio: i32, _path: u8) {
    // BW != 80MHz branch
    pwm(mmio, R_P0_NRBW,     B_P0_NRBW_DBG,  1);
    pwm(mmio, R_P1_DBGMOD,   B_P1_DBGMOD_ON, 1);
    udelay_1();
    pwm(mmio, R_ANAPAR_PW15, B_ANAPAR_PW15,  0x0F);
    udelay_1();
    pwm(mmio, R_ANAPAR_PW15, B_ANAPAR_PW15,  0x03);
    pwm(mmio, R_ANAPAR,      B_ANAPAR_15,    0xA001);
    udelay_1();
    pwm(mmio, R_ANAPAR,      B_ANAPAR_15,    0xA041);
    pwm(mmio, R_P0_RXCK,     B_P0_RXCK_VAL,  0x1);
    pwm(mmio, R_P0_RXCK,     B_P0_RXCK_ON,   1);
    pwm(mmio, R_P1_RXCK,     B_P1_RXCK_VAL,  0x1);
    pwm(mmio, R_P1_RXCK,     B_P1_RXCK_ON,   1);
    pwm(mmio, R_UPD_CLK_ADC, B_UPD_CLK_ADC_ON,  1);
    pwm(mmio, R_UPD_CLK_ADC, B_UPD_CLK_ADC_VAL, 0);
}

// ═══════════════════════════════════════════════════════════════════
//  _iqk_rxk_setting — rfk.c:792
// ═══════════════════════════════════════════════════════════════════
fn iqk_rxk_setting(mmio: i32, state: &IqkState, path: u8) {
    match state.band[path as usize] {
        BAND_2G => {
            rw(mmio, path, RR_MOD, RR_MOD_MASK, 0xC);
            rw(mmio, path, RR_RXK, RR_RXK_SEL2G, 1);
            let tmp = rr(mmio, path, RR_CFGCH);
            rw(mmio, path, RR_RSV4, RFREG_MASK, tmp);
        }
        BAND_5G => {
            rw(mmio, path, RR_MOD, RR_MOD_MASK, 0xC);
            rw(mmio, path, RR_RXK, RR_RXK_SEL5G, 1);
            let tmp = rr(mmio, path, RR_CFGCH);
            rw(mmio, path, RR_RSV4, RFREG_MASK, tmp);
        }
        _ => {}
    }
}

// ═══════════════════════════════════════════════════════════════════
//  _iqk_txk_setting — rfk.c:1273
// ═══════════════════════════════════════════════════════════════════
fn iqk_txk_setting(mmio: i32, state: &IqkState, path: u8) {
    match state.band[path as usize] {
        BAND_2G => {
            rw(mmio, path, RR_XALNA2, RR_XALNA2_SW2,  0x00);
            rw(mmio, path, RR_TXG1,   RR_TXG1_ATT2,   0x0);
            rw(mmio, path, RR_TXG1,   RR_TXG1_ATT1,   0x0);
            rw(mmio, path, RR_TXG2,   RR_TXG2_ATT0,   0x1);
            rw(mmio, path, RR_TXGA,   RR_TXGA_LOK_EXT, 0x0);
            rw(mmio, path, RR_LUTWE,  RR_LUTWE_LOK,   0x1);
            rw(mmio, path, RR_LUTWA,  RR_LUTWA_M1,    0x00);
            rw(mmio, path, RR_MOD,    RR_MOD_IQK,     0x403E);
            udelay_1();
        }
        BAND_5G => {
            rw(mmio, path, RR_XGLNA2, RR_XGLNA2_SW,   0x00);
            rw(mmio, path, RR_BIASA,  RR_BIASA_A,     0x1);
            rw(mmio, path, RR_TXGA,   RR_TXGA_LOK_EXT, 0x0);
            rw(mmio, path, RR_LUTWE,  RR_LUTWE_LOK,   0x1);
            rw(mmio, path, RR_LUTWA,  RR_LUTWA_M1,    0x80);
            rw(mmio, path, RR_MOD,    RR_MOD_IQK,     0x403E);
            udelay_1();
        }
        _ => {}
    }
}

// ═══════════════════════════════════════════════════════════════════
//  _lok_res_table — rfk.c:1127
// ═══════════════════════════════════════════════════════════════════
fn lok_res_table(mmio: i32, state: &IqkState, path: u8, ibias: u8) {
    rw(mmio, path, RR_LUTWE,  RFREG_MASK, 0x2);
    if state.band[path as usize] == BAND_2G {
        rw(mmio, path, RR_LUTWA, RFREG_MASK, 0x0);
    } else {
        rw(mmio, path, RR_LUTWA, RFREG_MASK, 0x1);
    }
    rw(mmio, path, RR_LUTWD0, RFREG_MASK, ibias as u32);
    rw(mmio, path, RR_LUTWE,  RFREG_MASK, 0x0);
    rw(mmio, path, RR_TXVBUF, RR_TXVBUF_DACEN, 0x1);
}

// ═══════════════════════════════════════════════════════════════════
//  _lok_finetune_check — rfk.c:1147 (returns true = fail)
// ═══════════════════════════════════════════════════════════════════
fn lok_finetune_check(mmio: i32, path: u8) -> bool {
    let tmp = rr(mmio, path, RR_TXMO);
    let core_i = (tmp & RR_TXMO_COI) >> RR_TXMO_COI.trailing_zeros();
    let core_q = (tmp & RR_TXMO_COQ) >> RR_TXMO_COQ.trailing_zeros();
    let fail1 = core_i < 0x02 || core_i > 0x1D || core_q < 0x02 || core_q > 0x1D;

    let tmp = rr(mmio, path, RR_LOKVB);
    let vbuff_i = (tmp & RR_LOKVB_COI) >> RR_LOKVB_COI.trailing_zeros();
    let vbuff_q = (tmp & RR_LOKVB_COQ) >> RR_LOKVB_COQ.trailing_zeros();
    let fail2 = vbuff_i < 0x02 || vbuff_i > 0x3D || vbuff_q < 0x02 || vbuff_q > 0x3D;

    fail1 || fail2
}

// ═══════════════════════════════════════════════════════════════════
//  _iqk_lok — rfk.c:1191 (both 2G and 5G)
// ═══════════════════════════════════════════════════════════════════
fn iqk_lok(mmio: i32, state: &mut IqkState, path: u8) -> bool {
    pwm(mmio, R_IQK_DIF4, B_IQK_DIF4_TXT, 0x021);

    match state.band[path as usize] {
        BAND_2G => {
            rw(mmio, path, RR_TXIG, RR_TXIG_GR0, 0x0);
            rw(mmio, path, RR_TXIG, RR_TXIG_GR1, 0x6);
        }
        BAND_5G => {
            rw(mmio, path, RR_TXIG, RR_TXIG_GR0, 0x0);
            rw(mmio, path, RR_TXIG, RR_TXIG_GR1, 0x4);
        }
        _ => {}
    }
    rw(mmio, path, RR_TXIG, RR_TXIG_TG, 0x0);  // 2G and 5G both

    pw(mmio, R_KIP_IQP + ((path as u32) << 8), 0x9);
    let fail_cor = iqk_one_shot(mmio, state, path, ID_FLOK_COARSE);
    state.lok_cor_fail[path as usize] = fail_cor;

    rw(mmio, path, RR_TXIG, RR_TXIG_TG, 0x12); // both bands

    pw(mmio, R_KIP_IQP + ((path as u32) << 8), 0x24);
    iqk_one_shot(mmio, state, path, ID_FLOK_VBUFFER);

    rw(mmio, path, RR_TXIG, RR_TXIG_TG, 0x0);

    pw(mmio, R_KIP_IQP + ((path as u32) << 8), 0x9);
    pwm(mmio, R_IQK_DIF4, B_IQK_DIF4_TXT, 0x021);
    let fail_fin = iqk_one_shot(mmio, state, path, ID_FLOK_FINE);
    state.lok_fin_fail[path as usize] = fail_fin;

    rw(mmio, path, RR_TXIG, RR_TXIG_TG, 0x12);

    pw(mmio, R_KIP_IQP + ((path as u32) << 8), 0x24);
    iqk_one_shot(mmio, state, path, ID_FLOK_VBUFFER);

    lok_finetune_check(mmio, path)
}

// ═══════════════════════════════════════════════════════════════════
//  _txk_group_sel — rfk.c:1016 (wide-band: 4 groups)
// ═══════════════════════════════════════════════════════════════════
fn txk_group_sel(mmio: i32, state: &mut IqkState, path: u8) -> bool {
    let mut kfail = false;
    for gp in 0..RXK_GROUP_NR {
        match state.band[path as usize] {
            BAND_2G => {
                rw(mmio, path, RR_TXIG, RR_TXIG_GR0, G_POWER_RNG[gp]);
                rw(mmio, path, RR_TXIG, RR_TXIG_GR1, G_TRACK_RNG[gp]);
                rw(mmio, path, RR_TXIG, RR_TXIG_TG,  G_GAIN_BB[gp]);
                pw(mmio, R_KIP_IQP + ((path as u32) << 8), G_ITQT[gp]);
            }
            BAND_5G => {
                rw(mmio, path, RR_TXIG, RR_TXIG_GR0, A_POWER_RNG[gp]);
                rw(mmio, path, RR_TXIG, RR_TXIG_GR1, A_TRACK_RNG[gp]);
                rw(mmio, path, RR_TXIG, RR_TXIG_TG,  A_GAIN_BB[gp]);
                pw(mmio, R_KIP_IQP + ((path as u32) << 8), A_ITQT[gp]);
            }
            _ => {}
        }
        let off = (path as u32) << 8;
        pwm(mmio, R_CFIR_LUT + off, B_CFIR_LUT_SEL, 0x1);
        pwm(mmio, R_CFIR_LUT + off, B_CFIR_LUT_SET, 0x1);
        pwm(mmio, R_CFIR_LUT + off, B_CFIR_LUT_G2,  0x0);
        pwm(mmio, R_CFIR_LUT + off, B_CFIR_LUT_GP,  gp as u32);
        pwm(mmio, R_NCTL_N1,        B_NCTL_N1_CIP,  0x00);
        let fail = iqk_one_shot(mmio, state, path, ID_TXK);
        pwm(mmio, R_IQKINF, 1 << (8 + gp as u32 + (path as u32) * 4), fail as u32);
        kfail |= fail;
    }
    if kfail {
        state.nb_txcfir[path as usize] = 0x40000002;
        pwm(mmio, R_IQK_RES + ((path as u32) << 8), B_IQK_RES_TXCFIR, 0x0);
        state._is_wb_txiqk[path as usize] = false;
    } else {
        state.nb_txcfir[path as usize] = 0x40000000;
        pwm(mmio, R_IQK_RES + ((path as u32) << 8), B_IQK_RES_TXCFIR, 0x5);
        state._is_wb_txiqk[path as usize] = true;
    }
    kfail
}

// ═══════════════════════════════════════════════════════════════════
//  _iqk_nbtxk — rfk.c:1079 (narrow-band TX: single group)
// ═══════════════════════════════════════════════════════════════════
#[allow(dead_code)]
fn iqk_nbtxk(mmio: i32, state: &mut IqkState, path: u8) -> bool {
    let gp: usize = 0x2;
    match state.band[path as usize] {
        BAND_2G => {
            rw(mmio, path, RR_TXIG, RR_TXIG_GR0, G_POWER_RNG[gp]);
            rw(mmio, path, RR_TXIG, RR_TXIG_GR1, G_TRACK_RNG[gp]);
            rw(mmio, path, RR_TXIG, RR_TXIG_TG,  G_GAIN_BB[gp]);
            pw(mmio, R_KIP_IQP + ((path as u32) << 8), G_ITQT[gp]);
        }
        BAND_5G => {
            rw(mmio, path, RR_TXIG, RR_TXIG_GR0, A_POWER_RNG[gp]);
            rw(mmio, path, RR_TXIG, RR_TXIG_GR1, A_TRACK_RNG[gp]);
            rw(mmio, path, RR_TXIG, RR_TXIG_TG,  A_GAIN_BB[gp]);
            pw(mmio, R_KIP_IQP + ((path as u32) << 8), A_ITQT[gp]);
        }
        _ => {}
    }
    let off = (path as u32) << 8;
    pwm(mmio, R_CFIR_LUT + off, B_CFIR_LUT_SEL, 0x1);
    pwm(mmio, R_CFIR_LUT + off, B_CFIR_LUT_SET, 0x1);
    pwm(mmio, R_CFIR_LUT + off, B_CFIR_LUT_G2,  0x0);
    pwm(mmio, R_CFIR_LUT + off, B_CFIR_LUT_GP,  gp as u32);
    pwm(mmio, R_NCTL_N1,        B_NCTL_N1_CIP,  0x00);
    let kfail = iqk_one_shot(mmio, state, path, ID_NBTXK);
    if !kfail {
        state.nb_txcfir[path as usize] = pr(mmio, R_TXIQC + off) | 0x2;
    } else {
        state.nb_txcfir[path as usize] = 0x40000002;
    }
    kfail
}

// ═══════════════════════════════════════════════════════════════════
//  _rxk_group_sel — rfk.c:870 (wide-band: 4 groups)
// ═══════════════════════════════════════════════════════════════════
fn rxk_group_sel(mmio: i32, state: &mut IqkState, path: u8) -> bool {
    let mut kfail = false;
    for gp in 0..RXK_GROUP_NR {
        match state.band[path as usize] {
            BAND_2G => {
                rw(mmio, path, RR_MOD,  RR_MOD_RGM, G_IDXRXGAIN[gp]);
                rw(mmio, path, RR_RXBB, RR_RXBB_C2G, G_IDXATTC2[gp]);
                rw(mmio, path, RR_RXBB, RR_RXBB_C1G, G_IDXATTC1[gp]);
            }
            BAND_5G => {
                rw(mmio, path, RR_MOD,  RR_MOD_RGM, A_IDXRXGAIN[gp]);
                rw(mmio, path, RR_RXA2, RR_RXA2_HATT, A_IDXATTC2[gp]);
                rw(mmio, path, RR_RXA2, RR_RXA2_CC2,  A_IDXATTC1[gp]);
            }
            _ => {}
        }
        let off = (path as u32) << 8;
        pwm(mmio, R_CFIR_LUT + off, B_CFIR_LUT_SEL,   0x1);
        pwm(mmio, R_CFIR_LUT + off, B_CFIR_LUT_SET,   0x0);
        pwm(mmio, R_CFIR_LUT + off, B_CFIR_LUT_GP_V1, gp as u32);
        let fail = iqk_one_shot(mmio, state, path, ID_RXK);
        pwm(mmio, R_IQKINF, 1 << (16 + gp as u32 + (path as u32) * 4), fail as u32);
        kfail |= fail;
    }
    rw(mmio, path, RR_RXK, RR_RXK_SEL5G, 0x0);

    if kfail {
        state.nb_rxcfir[path as usize] = 0x40000002;
        pwm(mmio, R_IQK_RES + ((path as u32) << 8), B_IQK_RES_RXCFIR, 0x0);
        state._is_wb_rxiqk[path as usize] = false;
    } else {
        state.nb_rxcfir[path as usize] = 0x40000000;
        pwm(mmio, R_IQK_RES + ((path as u32) << 8), B_IQK_RES_RXCFIR, 0x5);
        state._is_wb_rxiqk[path as usize] = true;
    }
    kfail
}

// ═══════════════════════════════════════════════════════════════════
//  _iqk_nbrxk — rfk.c:928 (narrow-band RX: single group 0x3)
// ═══════════════════════════════════════════════════════════════════
#[allow(dead_code)]
fn iqk_nbrxk(mmio: i32, state: &mut IqkState, path: u8) -> bool {
    let gp: usize = 0x3;
    match state.band[path as usize] {
        BAND_2G => {
            rw(mmio, path, RR_MOD,  RR_MOD_RGM, G_IDXRXGAIN[gp]);
            rw(mmio, path, RR_RXBB, RR_RXBB_C2G, G_IDXATTC2[gp]);
            rw(mmio, path, RR_RXBB, RR_RXBB_C1G, G_IDXATTC1[gp]);
        }
        BAND_5G => {
            rw(mmio, path, RR_MOD,  RR_MOD_RGM, A_IDXRXGAIN[gp]);
            rw(mmio, path, RR_RXA2, RR_RXA2_HATT, A_IDXATTC2[gp]);
            rw(mmio, path, RR_RXA2, RR_RXA2_CC2,  A_IDXATTC1[gp]);
        }
        _ => {}
    }
    let off = (path as u32) << 8;
    pwm(mmio, R_CFIR_LUT + off, B_CFIR_LUT_SEL,   0x1);
    pwm(mmio, R_CFIR_LUT + off, B_CFIR_LUT_SET,   0x0);
    pwm(mmio, R_CFIR_LUT + off, B_CFIR_LUT_GP_V1, gp as u32);
    rw(mmio, path, RR_RXKPLL, RFREG_MASK, 0x80013);
    udelay_1();

    let kfail = iqk_one_shot(mmio, state, path, ID_NBRXK);
    pwm(mmio, R_IQKINF, 1 << (16 + gp as u32 + (path as u32) * 4), kfail as u32);
    rw(mmio, path, RR_RXK, RR_RXK_SEL5G, 0x0);

    if !kfail {
        state.nb_rxcfir[path as usize] = pr(mmio, R_RXIQC + off) | 0x2;
    } else {
        state.nb_rxcfir[path as usize] = 0x40000002;
    }
    kfail
}

// ═══════════════════════════════════════════════════════════════════
//  _iqk_preset — rfk.c:1493
// ═══════════════════════════════════════════════════════════════════
fn iqk_preset(mmio: i32, path: u8) {
    let idx: u32 = 0; // rfk_mcc.table_idx — always 0 for us
    let off = (path as u32) << 8;
    pwm(mmio, R_COEF_SEL + off, B_COEF_SEL_IQC, idx);
    pwm(mmio, R_CFIR_LUT + off, B_CFIR_LUT_G3,  idx);
    rw(mmio, path, RR_RSV1, RR_RSV1_RST, 0x0);
    rw(mmio, path, RR_BBDC, RR_BBDC_SEL, 0x0);
    pw(mmio, R_NCTL_RPT,   0x00000080);
    pw(mmio, R_KIP_SYSCFG, 0x81FF010A);
}

// ═══════════════════════════════════════════════════════════════════
//  _iqk_macbb_setting + _iqk_afebb_restore — rfk.c:1514/1467
// ═══════════════════════════════════════════════════════════════════
fn apply_reg3(mmio: i32, table: &[(u32, u32, u32)]) {
    for &(addr, mask, val) in table {
        host::mmio_w32_mask(mmio, CR + addr, mask, val);
    }
}

// ═══════════════════════════════════════════════════════════════════
//  _iqk_restore — rfk.c:1439
// ═══════════════════════════════════════════════════════════════════
fn iqk_restore(mmio: i32, state: &IqkState, path: u8) {
    let off = (path as u32) << 8;
    pw(mmio, R_TXIQC + off, state.nb_txcfir[path as usize]);
    pw(mmio, R_RXIQC + off, state.nb_rxcfir[path as usize]);
    pw(mmio, R_NCTL_CFG, 0x00000E19 + ((path as u32) << 4));
    let _fail = iqk_check_cal(mmio, path);

    pwm(mmio, R_NCTL_N1,    B_NCTL_N1_CIP, 0x00);
    pw(mmio, R_NCTL_RPT,    0x00000000);
    pw(mmio, R_KIP_SYSCFG,  0x80000000);
    pwm(mmio, R_CFIR_SYS,   B_IQK_RES_K,   0x0);
    pwm(mmio, R_IQRSN,      B_IQRSN_K1,    0x0);
    pwm(mmio, R_IQRSN,      B_IQRSN_K2,    0x0);
    rw(mmio, path, RR_LUTWE, RR_LUTWE_LOK, 0x0);
    rw(mmio, path, RR_LUTWE, RR_LUTWE_LOK, 0x0);
    rw(mmio, path, RR_MOD,   RR_MOD_MASK,  0x3);
    rw(mmio, path, RR_RSV1,  RR_RSV1_RST,  0x1);
    rw(mmio, path, RR_BBDC,  RR_BBDC_SEL,  0x1);
}

// ═══════════════════════════════════════════════════════════════════
//  _iqk_by_path — rfk.c:1347 (main dispatcher: LOK + TXK + RXK)
// ═══════════════════════════════════════════════════════════════════
fn iqk_by_path(mmio: i32, state: &mut IqkState, path: u8) {
    iqk_txclk_setting(mmio, path);

    // LOK with 3x retry (ibias starts at 1, increments each try)
    let mut ibias: u8 = 0x1;
    let mut lok_is_fail = false;
    for _try in 0..3 {
        lok_res_table(mmio, state, path, ibias);
        ibias = ibias.wrapping_add(1);
        iqk_txk_setting(mmio, state, path);
        lok_is_fail = iqk_lok(mmio, state, path);
        if !lok_is_fail { break; }
    }
    if lok_is_fail {
        host::print("    IQK: LOK fail path ");
        fw::print_dec(path as usize);
        host::print("\n");
    }

    // TXK — wide-band (is_nbiqk=false default)
    state.tx_fail[path as usize] = txk_group_sel(mmio, state, path);

    // RX
    iqk_rxclk_setting(mmio, path);
    iqk_rxk_setting(mmio, state, path);
    state.rx_fail[path as usize] = rxk_group_sel(mmio, state, path);

    // _iqk_info_iqk (debug only — write fail flags into R_IQKINF)
    let off4 = (path as u32) * 4;
    pwm(mmio, R_IQKINF, B_IQKINF_F_COR << off4, state.lok_cor_fail[path as usize] as u32);
    pwm(mmio, R_IQKINF, B_IQKINF_F_FIN << off4, state.lok_fin_fail[path as usize] as u32);
    pwm(mmio, R_IQKINF, B_IQKINF_F_TX  << off4, state.tx_fail[path as usize] as u32);
    pwm(mmio, R_IQKINF, B_IQKINF_F_RX  << off4, state.rx_fail[path as usize] as u32);
}

// ═══════════════════════════════════════════════════════════════════
//  Public entry — runs the complete IQK flow for path A+B
//  Mirrors Linux _doiqk (rfk.c:1596) for the RF_AB kpath.
// ═══════════════════════════════════════════════════════════════════
pub fn run(mmio: i32) {
    host::print("  IQK: start (full 1:1 Linux, backup/restore BB+RF)\n");

    // ── TX scheduler pause — 1:1 Linux rtw8852b_iqk (rtw8852b_rfk.c:3764).
    // IQK measures TX LO leakage / I-Q mismatch; if the CMAC scheduler is
    // still firing TX slots (even NOP) the cal engine sees stale TX energy
    // and LOK produces values outside [0x02..0x1D] → cor/fin fail.
    // FW is running, so Linux routes this through H2CREG SCH_TX_EN.
    let tx_en_saved = fw::stop_sch_tx(mmio, 0);
    host::print("    [iqk] sch_tx stopped (saved tx_en=0x");
    host::print_hex16(tx_en_saved);
    host::print(")\n");

    // State — defaults match rtw89_iqk_info after _iqk_init.
    // Channel is 2G ch 7 @ 20 MHz (set by chan::set_channel_2g(mmio, 7)).
    let mut state = IqkState {
        band: [BAND_2G, BAND_2G],
        bw:   [BW_20M,  BW_20M],
        ch:   [7, 7],
        lok_cor_fail: [false; 2],
        lok_fin_fail: [false; 2],
        tx_fail: [false; 2],
        rx_fail: [false; 2],
        nb_txcfir: [0; 2],
        nb_rxcfir: [0; 2],
        _is_wb_txiqk: [false; 2],
        _is_wb_rxiqk: [false; 2],
    };

    // _wait_rx_mode — make sure the radio is in RX (not TX) before IQK.
    // Linux rtw8852b_iqk calls this under chip_stop_sch_tx.
    wait_rx_mode(mmio);

    // _iqk_init — R_IQKINF = 0
    pw(mmio, R_IQKINF, 0);

    // _iqk_get_ch_info — write version/band/bw/ch into R_IQKINF/R_IQKCH
    pwm(mmio, R_IQKINF, B_IQKINF_VER, IQK_VER);
    for p in 0..2u8 {
        let shift = (p as u32) * 16;
        pwm(mmio, R_IQKCH, B_IQKCH_BAND << shift, state.band[p as usize] as u32);
        pwm(mmio, R_IQKCH, B_IQKCH_BW   << shift, state.bw[p as usize] as u32);
        pwm(mmio, R_IQKCH, B_IQKCH_CH   << shift, state.ch[p as usize] as u32);
    }

    // Per-path: backup BB+RF, run full IQK path, restore BB+RF.
    // Linux _doiqk pattern — without backup/restore, IQK permanently
    // corrupts 3 BB regs (0x2344, 0x5800, 0x7800) and 11 RF regs per
    // path, leaving the RX pipe in IQK state after IQK finishes.
    for path in 0u8..2 {
        let mut bb_save = [0u32; 3];
        let mut rf_save = [0u32; 11];
        backup_bb(mmio, &mut bb_save);
        backup_rf(mmio, path, &mut rf_save);

        // _iqk_macbb_setting — apply SET_NONDBCC table per path
        apply_reg3(mmio, RTW8852B_SET_NONDBCC_PATH01);

        host::print("  IQK: path ");
        fw::print_dec(path as usize);
        host::print(" preset + LOK+TXK+RXK...\n");
        iqk_preset(mmio, path);
        iqk_by_path(mmio, &mut state, path);
        iqk_restore(mmio, &state, path);

        // _iqk_afebb_restore — apply RESTORE_NONDBCC
        apply_reg3(mmio, RTW8852B_RESTORE_NONDBCC_PATH01);

        restore_bb(mmio, &bb_save);
        restore_rf(mmio, path, &rf_save);
    }
    host::print("  IQK: BB+RF restored\n");

    // Resume TX scheduler — Linux rtw8852b_iqk line 3770.
    fw::resume_sch_tx(mmio, 0, tx_en_saved);
    host::print("    [iqk] sch_tx resumed\n");

    // Final status report
    host::print("  IQK: done | A: cor=");
    fw::print_dec(state.lok_cor_fail[0] as usize);
    host::print(" fin=");  fw::print_dec(state.lok_fin_fail[0] as usize);
    host::print(" tx=");   fw::print_dec(state.tx_fail[0] as usize);
    host::print(" rx=");   fw::print_dec(state.rx_fail[0] as usize);
    host::print(" | B: cor=");
    fw::print_dec(state.lok_cor_fail[1] as usize);
    host::print(" fin=");  fw::print_dec(state.lok_fin_fail[1] as usize);
    host::print(" tx=");   fw::print_dec(state.tx_fail[1] as usize);
    host::print(" rx=");   fw::print_dec(state.rx_fail[1] as usize);
    host::print("\n");
}
