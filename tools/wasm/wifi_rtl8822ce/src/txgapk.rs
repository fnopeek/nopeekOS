//! `rtw8822c_txgapk` — die Sendeverstaerkungs-Kalibrierung (Stufe 5d),
//! rtw8822c.c:1191-1823.
//!
//! Sie misst je Verstaerkungsstufe den Abstand zwischen dem, was die
//! Verstaerkungstabelle des RF verspricht, und dem, was herauskommt, und
//! schreibt die Tabelle danach korrigiert zurueck.
//!
//! **Zwei Ausstiege stehen ganz vorn**, und beide gehoeren zur Funktion:
//! ohne gelesene Verstaerkungstabelle (`read_txgain == 0`) gibt es nichts
//! zu korrigieren, und bei `power_track_type` 4..7 regelt der Chip seine
//! Sendeleistung ueber TSSI — dann waere die Korrektur doppelt.
#![allow(dead_code)]

use crate::host;
use crate::phy;
use crate::regs::*;

pub const RF_BAND_MAX_U: usize = RF_BAND_MAX as usize;
pub const RF_GAIN_NUM_U: usize = RF_GAIN_NUM as usize;
pub const RF_HW_OFFSET_NUM_U: usize = RF_HW_OFFSET_NUM as usize;
const PATHS: usize = 4; // RTW_RF_PATH_MAX

/// main.h:1663-1671 `struct rtw_gapk_info`. Der Tippfehler `fianl_offset`
/// steht in Linux so da und bleibt hier stehen — wer danach sucht, sucht
/// mit dem Namen, den die Quelle traegt.
pub struct GapkInfo {
    pub rf3f_bp: [[[u32; PATHS]; RF_GAIN_NUM_U]; RF_BAND_MAX_U],
    pub rf3f_fs: [[i8; RF_GAIN_NUM_U]; PATHS],
    pub txgapk_bp_done: bool,
    pub offset: [[i8; PATHS]; RF_GAIN_NUM_U],
    pub fianl_offset: [[i8; PATHS]; RF_GAIN_NUM_U],
    pub read_txgain: u8,
    pub channel: u8,
}

impl GapkInfo {
    pub const fn new() -> Self {
        GapkInfo {
            rf3f_bp: [[[0; PATHS]; RF_GAIN_NUM_U]; RF_BAND_MAX_U],
            rf3f_fs: [[0; RF_GAIN_NUM_U]; PATHS],
            txgapk_bp_done: false,
            offset: [[0; PATHS]; RF_GAIN_NUM_U],
            fianl_offset: [[0; PATHS]; RF_GAIN_NUM_U],
            read_txgain: 0,
            channel: 0,
        }
    }
}

impl Default for GapkInfo {
    fn default() -> Self {
        Self::new()
    }
}

#[inline]
fn fget(v: u32, mask: u32) -> u32 {
    (v & mask) >> mask.trailing_zeros()
}

/// rtw8822c.c:1191-1202 `rtw8822c_txgapk_backup_bb_reg`
fn backup_bb_reg(h: i32, reg: &[u32], backup: &mut [u32]) {
    for (i, &r) in reg.iter().enumerate() {
        backup[i] = host::r32(h, r);
    }
}

/// rtw8822c.c:1204-1215 `rtw8822c_txgapk_reload_bb_reg`
fn reload_bb_reg(h: i32, reg: &[u32], backup: &[u32]) {
    for (i, &r) in reg.iter().enumerate() {
        host::w32(h, r, backup[i]);
    }
}

/// rtw8822c.c:1217-1229 `check_rf_status`.
///
/// Wahr, wenn KEIN Pfad mehr im gefragten Zustand steht — der Name klingt
/// nach dem Gegenteil, und so steht er in Linux.
fn check_rf_status(h: i32, status: u32) -> bool {
    let a = phy::read_rf(h, phy::RF_PATH_A, RF_MODE_TRXAGC, BIT_RF_MODE);
    let b = phy::read_rf(h, phy::RF_PATH_B, RF_MODE_TRXAGC, BIT_RF_MODE);
    a != status && b != status
}

/// rtw8822c.c:1232-1246 `rtw8822c_txgapk_tx_pause`
fn tx_pause(h: i32) -> bool {
    host::w8(h, REG_TXPAUSE, BIT_AC_QUEUE as u8);
    host::w32_mask(h, REG_TX_FIFO, BIT_STOP_TX, 0x2);

    // `read_poll_timeout_atomic(check_rf_status, status, status, 2, 5000, …)`
    let t0 = host::now_us();
    loop {
        if check_rf_status(h, 2) {
            return true;
        }
        if host::now_us() - t0 >= 5000 {
            host::print("[rtl8822ce] failed to pause TX\n");
            return false;
        }
        host::delay_us(2);
    }
}

/// rtw8822c.c:1248-1278 `rtw8822c_txgapk_bb_dpk`
fn bb_dpk(h: i32, path: usize) {
    host::w32_mask(h, REG_ENFN, BIT_IQK_DPK_EN, 0x1);
    host::w32_mask(h, REG_CH_DELAY_EXTR2, BIT_IQK_DPK_CLOCK_SRC, 0x1);
    host::w32_mask(h, REG_CH_DELAY_EXTR2, BIT_IQK_DPK_RESET_SRC, 0x1);
    host::w32_mask(h, REG_CH_DELAY_EXTR2, BIT_EN_IOQ_IQK_DPK, 0x1);
    host::w32_mask(h, REG_CH_DELAY_EXTR2, BIT_TST_IQK2SET_SRC, 0x0);
    host::w32_mask(h, REG_CCA_OFF, BIT_CCA_ON_BY_PW, 0x1ff);

    if path == phy::RF_PATH_A {
        host::w32_mask(h, REG_RFTXEN_GCK_A, BIT_RFTXEN_GCK_FORCE_ON, 0x1);
        host::w32_mask(h, REG_3WIRE, BIT_DIS_SHARERX_TXGAT, 0x1);
        host::w32_mask(h, REG_DIS_SHARE_RX_A, BIT_TX_SCALE_0DB, 0x1);
        host::w32_mask(h, REG_3WIRE, BIT_3WIRE_EN, 0x0);
    } else if path == phy::RF_PATH_B {
        host::w32_mask(h, REG_RFTXEN_GCK_B, BIT_RFTXEN_GCK_FORCE_ON, 0x1);
        host::w32_mask(h, REG_3WIRE2, BIT_DIS_SHARERX_TXGAT, 0x1);
        host::w32_mask(h, REG_DIS_SHARE_RX_B, BIT_TX_SCALE_0DB, 0x1);
        host::w32_mask(h, REG_3WIRE2, BIT_3WIRE_EN, 0x0);
    }
    host::w32_mask(h, REG_CCKSB, BIT_BBMODE, 0x2);
}

/// rtw8822c.c:1280-1314 `rtw8822c_txgapk_afe_dpk`.
///
/// Achtzehn Schreibzugriffe, und der erste Wert steht ZWEIMAL da. Das ist
/// kein Kopierfehler von mir — es steht in Linux so, und der letzte ebenso.
fn afe_dpk(h: i32, path: usize) {
    let reg = match path {
        p if p == phy::RF_PATH_A => REG_ANAPAR_A,
        p if p == phy::RF_PATH_B => REG_ANAPAR_B,
        _ => {
            host::print("[rtl8822ce] [TXGAPK] unknown path\n");
            return;
        }
    };
    host::w32_mask(h, REG_IQK_CTRL, MASKDWORD, MASKDWORD);
    for v in [
        0x700f0001u32, 0x700f0001, 0x701f0001, 0x702f0001, 0x703f0001,
        0x704f0001, 0x705f0001, 0x706f0001, 0x707f0001, 0x708f0001,
        0x709f0001, 0x70af0001, 0x70bf0001, 0x70cf0001, 0x70df0001,
        0x70ef0001, 0x70ff0001, 0x70ff0001,
    ] {
        host::w32_mask(h, reg, MASKDWORD, v);
    }
}

/// rtw8822c.c:1316-1347 `rtw8822c_txgapk_afe_dpk_restore`
fn afe_dpk_restore(h: i32, path: usize) {
    let reg = match path {
        p if p == phy::RF_PATH_A => REG_ANAPAR_A,
        p if p == phy::RF_PATH_B => REG_ANAPAR_B,
        _ => {
            host::print("[rtl8822ce] [TXGAPK] unknown path\n");
            return;
        }
    };
    host::w32_mask(h, REG_IQK_CTRL, MASKDWORD, 0xffa1005e);
    for v in [
        0x700b8041u32, 0x70144041, 0x70244041, 0x70344041, 0x70444041,
        0x705b8041, 0x70644041, 0x707b8041, 0x708b8041, 0x709b8041,
        0x70ab8041, 0x70bb8041, 0x70cb8041, 0x70db8041, 0x70eb8041,
        0x70fb8041,
    ] {
        host::w32_mask(h, reg, MASKDWORD, v);
    }
}

/// rtw8822c.c:1349-1387 `rtw8822c_txgapk_bb_dpk_restore`
fn bb_dpk_restore(h: i32, path: usize) {
    phy::write_rf_reg_mix(h, path, RF_DEBUG, BIT_DE_TX_GAIN, 0x0);
    phy::write_rf_reg_mix(h, path, RF_DIS_BYPASS_TXBB, BIT_TIA_BYPASS, 0x0);
    phy::write_rf_reg_mix(h, path, RF_DIS_BYPASS_TXBB, BIT_TXBB, 0x0);

    host::w32_mask(h, REG_NCTL0, BIT_SEL_PATH, 0x0);
    host::w32_mask(h, REG_IQK_CTL1, BIT_TX_CFIR, 0x0);
    host::w32_mask(h, REG_SINGLE_TONE_SW, BIT_IRQ_TEST_MODE, 0x0);
    host::w32_mask(h, REG_R_CONFIG, MASKBYTE0, 0x00);
    host::w32_mask(h, REG_NCTL0, BIT_SEL_PATH, 0x1);
    host::w32_mask(h, REG_IQK_CTL1, BIT_TX_CFIR, 0x0);
    host::w32_mask(h, REG_SINGLE_TONE_SW, BIT_IRQ_TEST_MODE, 0x0);
    host::w32_mask(h, REG_R_CONFIG, MASKBYTE0, 0x00);
    host::w32_mask(h, REG_NCTL0, BIT_SEL_PATH, 0x0);
    host::w32_mask(h, REG_CCA_OFF, BIT_CCA_ON_BY_PW, 0x0);

    if path == phy::RF_PATH_A {
        host::w32_mask(h, REG_RFTXEN_GCK_A, BIT_RFTXEN_GCK_FORCE_ON, 0x0);
        host::w32_mask(h, REG_3WIRE, BIT_DIS_SHARERX_TXGAT, 0x0);
        host::w32_mask(h, REG_DIS_SHARE_RX_A, BIT_TX_SCALE_0DB, 0x0);
        host::w32_mask(h, REG_3WIRE, BIT_3WIRE_EN, 0x3);
    } else if path == phy::RF_PATH_B {
        host::w32_mask(h, REG_RFTXEN_GCK_B, BIT_RFTXEN_GCK_FORCE_ON, 0x0);
        host::w32_mask(h, REG_3WIRE2, BIT_DIS_SHARERX_TXGAT, 0x0);
        host::w32_mask(h, REG_DIS_SHARE_RX_B, BIT_TX_SCALE_0DB, 0x0);
        host::w32_mask(h, REG_3WIRE2, BIT_3WIRE_EN, 0x3);
    }

    host::w32_mask(h, REG_CCKSB, BIT_BBMODE, 0x0);
    host::w32_mask(h, REG_IQK_CTL1, BIT_CFIR_EN, 0x5);
}

/// rtw8822c.c:1389-1396 `_rtw8822c_txgapk_gain_valid`
fn gain_valid(gain: u32) -> bool {
    fget(gain, BIT_GAIN_TX_PAD_H) >= 0xc && fget(gain, BIT_GAIN_TX_PAD_L) >= 0xe
}

/// rtw8822c.c:1398-1450 `_rtw8822c_txgapk_write_gain_bb_table`
fn write_gain_bb_table_one(h: i32, g: &GapkInfo, band: usize, path: usize) {
    host::w32_mask(h, REG_NCTL0, BIT_SEL_PATH, path as u32);

    match band as u32 {
        RF_BAND_2G_OFDM => host::w32_mask(h, REG_TABLE_SEL, BIT_Q_GAIN_SEL, 0x0),
        RF_BAND_5G_L => host::w32_mask(h, REG_TABLE_SEL, BIT_Q_GAIN_SEL, 0x2),
        RF_BAND_5G_M => host::w32_mask(h, REG_TABLE_SEL, BIT_Q_GAIN_SEL, 0x3),
        RF_BAND_5G_H => host::w32_mask(h, REG_TABLE_SEL, BIT_Q_GAIN_SEL, 0x4),
        _ => {}
    }

    host::w32_mask(h, REG_TX_GAIN_SET, MASKBYTE0, 0x88);

    // `tmp_3f` wird NICHT je Durchlauf zurueckgesetzt: ist eine Stufe
    // gueltig, behaelt sie den Wert der letzten ungueltigen. So steht es da.
    let mut tmp_3f = 0u32;
    let mut check_txgain = false;
    for gain in 0..RF_GAIN_NUM_U {
        let v = g.rf3f_bp[band][gain][path];
        if gain_valid(v) {
            if !check_txgain {
                tmp_3f = v;
                check_txgain = true;
            }
        } else {
            tmp_3f = v;
        }

        host::w32_mask(h, REG_TABLE_SEL, BIT_Q_GAIN, tmp_3f);
        host::w32_mask(h, REG_TABLE_SEL, BIT_I_GAIN, gain as u32);
        host::w32_mask(h, REG_TABLE_SEL, BIT_GAIN_RST, 0x1);
        host::w32_mask(h, REG_TABLE_SEL, BIT_GAIN_RST, 0x0);
    }
}

/// rtw8822c.c:1452-1465 `rtw8822c_txgapk_write_gain_bb_table`
fn write_gain_bb_table(h: i32, g: &GapkInfo, rf_path_num: u8) {
    for band in 0..RF_BAND_MAX_U {
        for path in 0..rf_path_num as usize {
            write_gain_bb_table_one(h, g, band, path);
        }
    }
}

/// rtw8822c.c:1467-1542 `rtw8822c_txgapk_read_offset`
fn read_offset(h: i32, g: &mut GapkInfo, path: usize) {
    const CFG1_1B00: [u32; 2] = [0x00000d18, 0x00000d2a];
    const CFG2_1B00: [u32; 2] = [0x00000d19, 0x00000d2b];
    const SET_PI: [u32; 2] = [REG_RSV_CTRL, REG_WLRF1];
    const PATH_SETTING: [u32; 2] = [REG_ORITXCODE, REG_ORITXCODE2];

    if path >= CFG1_1B00.len() {
        host::print("[rtl8822ce] [TXGAPK] wrong path\n");
        return;
    }
    let channel = g.channel;

    host::w32_mask(h, REG_ANTMAP0, BIT_ANT_PATH, path as u32 + 1);
    host::w32_mask(h, REG_TXLGMAP, MASKDWORD, 0xe4e40000);
    host::w32_mask(h, REG_TXANTSEG, BIT_ANTSEG, 0x3);
    host::w32_mask(h, PATH_SETTING[path], MASK20BITS, 0x33312);
    host::w32_mask(h, PATH_SETTING[path], BIT_PATH_EN, 0x1);
    host::w32_mask(h, SET_PI[path], BITS_RFC_DIRECT, 0x0);
    phy::write_rf_reg_mix(h, path, RF_LUTDBG, BIT_TXA_TANK, 0x1);
    phy::write_rf_reg_mix(h, path, RF_IDAC, BIT_TX_MODE, 0x820);
    host::w32_mask(h, REG_NCTL0, BIT_SEL_PATH, path as u32);
    host::w32_mask(h, REG_IQKSTAT, MASKBYTE0, 0x0);

    host::w32_mask(h, REG_TX_TONE_IDX, MASKBYTE0, 0x018);
    host::delay_us(1000);
    if (1..=14).contains(&channel) {
        host::w32_mask(h, REG_R_CONFIG, MASKBYTE0, BIT_2G_SWING);
    } else {
        host::w32_mask(h, REG_R_CONFIG, MASKBYTE0, BIT_5G_SWING);
    }
    host::delay_us(1000);

    host::w32_mask(h, REG_NCTL0, MASKDWORD, CFG1_1B00[path]);
    host::w32_mask(h, REG_NCTL0, MASKDWORD, CFG2_1B00[path]);

    // `read_poll_timeout(…, val == 0x55, 1000, 100000, …)` — Linux prueft
    // den Rueckgabewert NICHT; die Zeit ist der ganze Zweck.
    let t0 = host::now_us();
    while host::r32_mask(h, REG_RPT_CIP, BIT_RPT_CIP_STATUS) != 0x55 {
        if host::now_us() - t0 >= 100_000 {
            break;
        }
        host::delay_us(1000);
    }

    host::w32_mask(h, SET_PI[path], BITS_RFC_DIRECT, 0x2);
    host::w32_mask(h, REG_NCTL0, BIT_SEL_PATH, path as u32);
    host::w32_mask(h, REG_RXSRAM_CTL, BIT_RPT_EN, 0x1);
    host::w32_mask(h, REG_RXSRAM_CTL, BIT_RPT_SEL, 0x12);
    host::w32_mask(h, REG_TX_GAIN_SET, BIT_GAPK_RPT_IDX, 0x3);
    let val = host::r32(h, REG_STAT_RPT);

    g.offset[0][path] = fget(val, BIT_GAPK_RPT0) as i8;
    g.offset[1][path] = fget(val, BIT_GAPK_RPT1) as i8;
    g.offset[2][path] = fget(val, BIT_GAPK_RPT2) as i8;
    g.offset[3][path] = fget(val, BIT_GAPK_RPT3) as i8;
    g.offset[4][path] = fget(val, BIT_GAPK_RPT4) as i8;
    g.offset[5][path] = fget(val, BIT_GAPK_RPT5) as i8;
    g.offset[6][path] = fget(val, BIT_GAPK_RPT6) as i8;
    g.offset[7][path] = fget(val, BIT_GAPK_RPT7) as i8;

    host::w32_mask(h, REG_TX_GAIN_SET, BIT_GAPK_RPT_IDX, 0x4);
    let val = host::r32(h, REG_STAT_RPT);

    g.offset[8][path] = fget(val, BIT_GAPK_RPT0) as i8;
    g.offset[9][path] = fget(val, BIT_GAPK_RPT1) as i8;

    // Vorzeichen aus vier Bit: Bit 3 gesetzt heisst negativ.
    for i in 0..RF_HW_OFFSET_NUM_U {
        if g.offset[i][path] & (1 << 3) != 0 {
            g.offset[i][path] |= 0xf0u8 as i8;
        }
    }
}

/// rtw8822c.c:1544-1616 `rtw8822c_txgapk_calculate_offset`
fn calculate_offset(h: i32, g: &mut GapkInfo, path: usize) {
    const BB_REG: [u32; 5] = [REG_ANTMAP0, REG_TXLGMAP, REG_TXANTSEG,
                              REG_ORITXCODE, REG_ORITXCODE2];
    let channel = g.channel;
    let mut backup = [0u32; 5];
    backup_bb_reg(h, &BB_REG, &mut backup);

    if (1..=14).contains(&channel) {
        host::w32_mask(h, REG_SINGLE_TONE_SW, BIT_IRQ_TEST_MODE, 0x0);
        host::w32_mask(h, REG_NCTL0, BIT_SEL_PATH, path as u32);
        host::w32_mask(h, REG_R_CONFIG, BIT_IQ_SWITCH, 0x3f);
        host::w32_mask(h, REG_IQK_CTL1, BIT_TX_CFIR, 0x0);
        phy::write_rf_reg_mix(h, path, RF_DEBUG, BIT_DE_TX_GAIN, 0x1);
        phy::write_rf_reg_mix(h, path, RF_MODE_TRXAGC, phy::RFREG_MASK, 0x5000f);
        phy::write_rf_reg_mix(h, path, RF_TX_GAIN_OFFSET, BIT_RF_GAIN, 0x0);
        phy::write_rf_reg_mix(h, path, RF_RXG_GAIN, BIT_RXG_GAIN, 0x1);
        phy::write_rf_reg_mix(h, path, RF_MODE_TRXAGC, BIT_RXAGC, 0x0f);
        phy::write_rf_reg_mix(h, path, RF_DEBUG, BIT_DE_TRXBW, 0x1);
        phy::write_rf_reg_mix(h, path, RF_BW_TRXBB, BIT_BW_TXBB, 0x1);
        phy::write_rf_reg_mix(h, path, RF_BW_TRXBB, BIT_BW_RXBB, 0x0);
        phy::write_rf_reg_mix(h, path, RF_EXT_TIA_BW, BIT_PW_EXT_TIA, 0x1);

        host::w32_mask(h, REG_IQKSTAT, MASKBYTE0, 0x00);
        host::w32_mask(h, REG_TABLE_SEL, BIT_Q_GAIN_SEL, 0x0);

        read_offset(h, g, path);
    } else {
        host::w32_mask(h, REG_SINGLE_TONE_SW, BIT_IRQ_TEST_MODE, 0x0);
        host::w32_mask(h, REG_NCTL0, BIT_SEL_PATH, path as u32);
        host::w32_mask(h, REG_R_CONFIG, BIT_IQ_SWITCH, 0x3f);
        host::w32_mask(h, REG_IQK_CTL1, BIT_TX_CFIR, 0x0);
        phy::write_rf_reg_mix(h, path, RF_DEBUG, BIT_DE_TX_GAIN, 0x1);
        phy::write_rf_reg_mix(h, path, RF_MODE_TRXAGC, phy::RFREG_MASK, 0x50011);
        phy::write_rf_reg_mix(h, path, RF_TXA_LB_SW, BIT_TXA_LB_ATT, 0x3);
        phy::write_rf_reg_mix(h, path, RF_TXA_LB_SW, BIT_LB_ATT, 0x3);
        phy::write_rf_reg_mix(h, path, RF_TXA_LB_SW, BIT_LB_SW, 0x1);
        phy::write_rf_reg_mix(h, path, RF_RXA_MIX_GAIN, BIT_RXA_MIX_GAIN, 0x2);
        phy::write_rf_reg_mix(h, path, RF_MODE_TRXAGC, BIT_RXAGC, 0x12);
        phy::write_rf_reg_mix(h, path, RF_DEBUG, BIT_DE_TRXBW, 0x1);
        phy::write_rf_reg_mix(h, path, RF_BW_TRXBB, BIT_BW_RXBB, 0x0);
        phy::write_rf_reg_mix(h, path, RF_EXT_TIA_BW, BIT_PW_EXT_TIA, 0x1);
        phy::write_rf_reg_mix(h, path, RF_MODE_TRXAGC, BIT_RF_MODE, 0x5);

        host::w32_mask(h, REG_IQKSTAT, MASKBYTE0, 0x0);

        if (36..=64).contains(&channel) {
            host::w32_mask(h, REG_TABLE_SEL, BIT_Q_GAIN_SEL, 0x2);
        } else if (100..=144).contains(&channel) {
            host::w32_mask(h, REG_TABLE_SEL, BIT_Q_GAIN_SEL, 0x3);
        } else if (149..=177).contains(&channel) {
            host::w32_mask(h, REG_TABLE_SEL, BIT_Q_GAIN_SEL, 0x4);
        }

        read_offset(h, g, path);
    }
    reload_bb_reg(h, &BB_REG, &backup);
}

/// rtw8822c.c:1618-1628 `rtw8822c_txgapk_rf_restore`
fn rf_restore(h: i32, path: usize, rf_path_num: u8) {
    if path >= rf_path_num as usize {
        return;
    }
    phy::write_rf_reg_mix(h, path, RF_MODE_TRXAGC, BIT_RF_MODE, 0x3);
    phy::write_rf_reg_mix(h, path, RF_DEBUG, BIT_DE_TRXBW, 0x0);
    phy::write_rf_reg_mix(h, path, RF_EXT_TIA_BW, BIT_PW_EXT_TIA, 0x0);
}

/// rtw8822c.c:1630-1652 `rtw8822c_txgapk_cal_gain`
fn cal_gain(gain: u32, offset: i8) -> u32 {
    if gain_valid(gain) {
        return gain;
    }
    // `(gain << 1) + offset` in u32-Arithmetik, wie in C: ein negatives
    // `offset` wird zur grossen Zahl und zieht beim Addieren ab.
    let gain_x2 = (gain << 1).wrapping_add(offset as i32 as u32);
    (gain_x2 >> 1) | if gain_x2 & (1 << 0) != 0 { BIT_GAIN_EXT } else { 0 }
}

/// rtw8822c.c:1654-1722 `rtw8822c_txgapk_write_tx_gain`
fn write_tx_gain(h: i32, g: &mut GapkInfo, rf_path_num: u8) {
    let channel = g.channel;
    let (tmp, band) = if (1..=14).contains(&channel) {
        (0x20u32, RF_BAND_2G_OFDM as usize)
    } else if (36..=64).contains(&channel) {
        (0x200, RF_BAND_5G_L as usize)
    } else if (100..=144).contains(&channel) {
        (0x280, RF_BAND_5G_M as usize)
    } else if (149..=177).contains(&channel) {
        (0x300, RF_BAND_5G_H as usize)
    } else {
        host::print("[rtl8822ce] [TXGAPK] unknown channel\n");
        return;
    };

    for path in 0..rf_path_num as usize {
        let mut offset_tmp = [0i8; RF_GAIN_NUM_U];
        for i in 0..RF_GAIN_NUM_U {
            offset_tmp[i] = 0;
            for j in i..RF_GAIN_NUM_U {
                if gain_valid(g.rf3f_bp[band][j][path]) {
                    continue;
                }
                offset_tmp[i] = offset_tmp[i].wrapping_add(g.offset[j][path]);
                g.fianl_offset[i][path] = offset_tmp[i];
            }
            if !gain_valid(g.rf3f_bp[band][i][path]) {
                g.rf3f_fs[path][i] = offset_tmp[i];
            }
        }

        phy::write_rf_reg_mix(h, path, RF_LUTWE2, phy::RFREG_MASK, 0x10000);
        for i in 0..RF_GAIN_NUM_U {
            phy::write_rf_reg_mix(h, path, RF_LUTWA, phy::RFREG_MASK,
                                  tmp + i as u32);
            let tmp_3f = cal_gain(g.rf3f_bp[band][i][path], offset_tmp[i]);
            phy::write_rf_reg_mix(h, path, RF_LUTWD0,
                                  BIT_GAIN_EXT | BIT_DATA_L, tmp_3f);
        }
        phy::write_rf_reg_mix(h, path, RF_LUTWE2, phy::RFREG_MASK, 0x0);
    }
}

/// rtw8822c.c:1724-1781 `rtw8822c_txgapk_save_all_tx_gain_table`
fn save_all_tx_gain_table(h: i32, g: &mut GapkInfo, rf_path_num: u8,
                          dm_flags: u32) {
    const THREE_WIRE: [u32; 2] = [REG_3WIRE, REG_3WIRE2];
    const CH_NUM: [u32; RF_BAND_MAX_U] = [1, 1, 36, 100, 149];
    const BAND_NUM: [u32; RF_BAND_MAX_U] = [0x0, 0x0, 0x1, 0x3, 0x5];
    const CCK: [u32; RF_BAND_MAX_U] = [0x1, 0x0, 0x0, 0x0, 0x0];

    // `BIT(RTW_DM_CAP_TXGAPK)` GESETZT heisst ABGESCHALTET — die Pruefung
    // ist umgekehrt, als der Name vermuten laesst.
    if dm_flags & (1 << RTW_DM_CAP_TXGAPK) != 0 {
        return;
    }

    if g.read_txgain == 1 {
        write_gain_bb_table(h, g, rf_path_num);
        return;
    }

    for band in 0..RF_BAND_MAX_U {
        for path in 0..rf_path_num as usize {
            let rf18 = phy::read_rf(h, path, RF_CFGCH, phy::RFREG_MASK);

            host::w32_mask(h, THREE_WIRE[path], BIT_3WIRE_EN, 0x0);
            phy::write_rf_reg_mix(h, path, RF_CFGCH, MASKBYTE0, CH_NUM[band]);
            phy::write_rf_reg_mix(h, path, RF_CFGCH, BIT_BAND, BAND_NUM[band]);
            phy::write_rf_reg_mix(h, path, RF_BW_TRXBB, BIT_DBG_CCK_CCA,
                                  CCK[band]);
            phy::write_rf_reg_mix(h, path, RF_BW_TRXBB, BIT_TX_CCK_IND,
                                  CCK[band]);
            let mut gain = 0usize;
            let mut rf0_idx = 1u32;
            while rf0_idx < 32 {
                phy::write_rf_reg_mix(h, path, RF_MODE_TRXAGC, MASKBYTE0,
                                      rf0_idx);
                let v = phy::read_rf(h, path, RF_TX_RESULT, phy::RFREG_MASK);
                g.rf3f_bp[band][gain][path] = v & BIT_DATA_L;
                gain += 1;
                rf0_idx += 3;
            }
            phy::write_rf_reg_mix(h, path, RF_CFGCH, phy::RFREG_MASK, rf18);
            host::w32_mask(h, THREE_WIRE[path], BIT_3WIRE_EN, 0x3);
        }
    }
    write_gain_bb_table(h, g, rf_path_num);
    g.read_txgain = 1;
}

/// Warum TXGAPK nicht gelaufen ist — oder dass es lief.
#[derive(Clone, Copy, PartialEq)]
pub enum TxgapkRpt {
    Ran,
    NoTxGain,
    TssiMode(u8),
    Disabled,
}

/// rtw8822c.c:1783-1823 `rtw8822c_txgapk`
pub fn txgapk(h: i32, g: &mut GapkInfo, rf_path_num: u8, dm_flags: u32,
              power_track_type: u8) -> TxgapkRpt {
    const BB_REG: [u32; 2] = [REG_TX_PTCL_CTRL, REG_TX_FIFO];
    let mut backup = [0u32; 2];

    save_all_tx_gain_table(h, g, rf_path_num, dm_flags);

    if g.read_txgain == 0 {
        return if dm_flags & (1 << RTW_DM_CAP_TXGAPK) != 0 {
            TxgapkRpt::Disabled
        } else {
            TxgapkRpt::NoTxGain
        };
    }

    // „Normal Mode in TSSI mode. return!!!" — der Chip regelt seine
    // Sendeleistung dann selbst, eine zweite Korrektur waere doppelt.
    if (4..=7).contains(&power_track_type) {
        return TxgapkRpt::TssiMode(power_track_type);
    }

    backup_bb_reg(h, &BB_REG, &mut backup);
    tx_pause(h);
    for path in 0..rf_path_num as usize {
        g.channel = (phy::read_rf(h, path, RF_CFGCH, phy::RFREG_MASK)
                     & MASKBYTE0) as u8;
        bb_dpk(h, path);
        afe_dpk(h, path);
        calculate_offset(h, g, path);
        rf_restore(h, path, rf_path_num);
        afe_dpk_restore(h, path);
        bb_dpk_restore(h, path);
    }
    write_tx_gain(h, g, rf_path_num);
    reload_bb_reg(h, &BB_REG, &backup);
    TxgapkRpt::Ran
}
