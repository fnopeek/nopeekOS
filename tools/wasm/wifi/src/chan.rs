//! set_channel(ch=1, 2.4GHz, 20MHz) — minimal 1:1 port of Linux
//! rtw8852b_set_channel for the default scan start channel.
//!
//! Linux entry: rtw8852b.c:580 rtw8852b_set_channel →
//!   __rtw8852bx_set_channel_mac  (rtw8852b_common.c:451)
//!   __rtw8852bx_set_channel_bb   (rtw8852b_common.c:1167)
//!   rtw8852b_set_channel_rf      (rtw8852b_rfk.c:4160 → ctrl_bw_ch)
//!
//! We hardcode ch=1, 20MHz, 2G — minimum needed so FW scan_offload has a
//! baseline channel to work from. Without it the RF is in post-table
//! default state (no frequency tuned) and the receiver is physically deaf.

use crate::host;

const CR: u32 = 0x10000; // PHY_CR_BASE for rtw89_phy_gen_ax

// RF path write via direct PHY access (8852b uses AX mechanism, not SWSI).
// base_addr[path] + (addr << 2) + CR_BASE.
// Linux phy.c:901 rtw89_phy_read_rf / phy.c:889 write_rf.
const RF_BASE_A: u32 = 0xE000;
const RF_BASE_B: u32 = 0xF000;
const RFREG_MASK: u32 = 0xF_FFFF;

fn rf_write(mmio: i32, path: u8, reg: u8, mask: u32, val: u32) {
    let base = if path == 0 { RF_BASE_A } else { RF_BASE_B };
    let addr = CR + base + ((reg as u32) << 2);
    let m = mask & RFREG_MASK;
    host::mmio_w32_mask(mmio, addr, m, val);
}

fn rf_read(mmio: i32, path: u8, reg: u8) -> u32 {
    let base = if path == 0 { RF_BASE_A } else { RF_BASE_B };
    let addr = CR + base + ((reg as u32) << 2);
    host::mmio_r32(mmio, addr) & RFREG_MASK
}

// ── RF register bits (reg.h around 8400) ────────────────────────
const RR_CFGCH: u8         = 0x18;
const RR_LCKST: u8         = 0xCF;
const RR_LUTWA: u8         = 0x33;
const RR_LUTWD0: u8        = 0x3F;
const RR_LUTWE2: u8        = 0xEE;
// bits for CFGCH
const RR_CFGCH_CH: u32     = 0xFF;        // [7:0]
const RR_CFGCH_BW: u32     = 0x3 << 10;   // [11:10]
const RR_CFGCH_BW2: u32    = 1 << 12;
const RR_CFGCH_BCN: u32    = 1 << 13;
const RR_CFGCH_TRX_AH: u32 = 1 << 14;
const RR_CFGCH_POW_LCK: u32 = 1 << 15;
const RR_CFGCH_BAND0: u32  = 0x3 << 8;
const RR_CFGCH_BAND1: u32  = 0x3 << 16;
const CFGCH_BW_20M: u32    = 3;
const RR_LCKST_BIN: u32    = 1 << 0;
const RR_LUTWA_M2: u32     = 0x1F;        // [4:0]
const RR_LUTWD0_LB: u32    = 0x3F;        // [5:0]
const RR_LUTWE2_RTXBW: u32 = 1 << 2;

// ── MAC registers (reg.h) ───────────────────────────────────────
const R_AX_WMAC_RFMOD: u32          = 0xCC1C;
const B_AX_WMAC_RFMOD_MASK: u32     = 0x3;
const R_AX_TX_SUB_CARRIER_VALUE: u32 = 0xC088;
const R_AX_TXRATE_CHK: u32          = 0xCA1C;
const B_AX_BAND_MODE: u32           = 1 << 0;
const B_AX_CHECK_CCK_EN: u32        = 1 << 1;
const B_AX_RTS_LIMIT_IN_OFDM6: u32  = 1 << 2;

// ── PHY registers for BB channel config ─────────────────────────
const R_PATH0_BAND_SEL_V1: u32  = 0x4738;
const R_PATH1_BAND_SEL_V1: u32  = 0x4AA4;
const B_PATH_BAND_SEL_V1: u32   = 1 << 17;
const R_FC0_BW_V1: u32          = 0x49C0;
const B_FC0_BW_INV: u32         = 0x7F;         // [6:0]
const B_FC0_BW_SET: u32         = 0x3 << 30;    // [31:30]
const R_CHBW_MOD_V1: u32        = 0x49C4;
const B_CHBW_MOD_SBW: u32       = 0x3 << 12;
const B_CHBW_MOD_PRICH: u32     = 0xF << 8;
const R_P0_RFMODE_ORI_RX: u32   = 0x12AC;
const R_P1_RFMODE_ORI_RX: u32   = 0x32AC;
const B_RFMODE_ORI_RX_ALL: u32  = 0xFFF << 12;  // [23:12]
const R_TXFIR0: u32             = 0x2300;
const R_UPD_CLK_ADC: u32        = 0x0700;
const B_ENABLE_CCK: u32         = 1 << 5;
const R_RXCCA: u32              = 0x2344;
const B_RXCCA_DIS: u32          = 1 << 31;
const R_MAC_PIN_SEL: u32        = 0x0734;
const B_CH_IDX_SEG0: u32        = 0xFF << 16;
const R_RXSCOBC: u32            = 0x23B0;
const R_RXSCOCCK: u32           = 0x23B4;
const B_RXSCO_TH: u32           = 0x7_FFFF;   // [18:0]

// BW setting per-path ADC/WBADC (rtw8852b_bw_setting table):
// For 20MHz: adc_sel = 0 (mask 0x6000), wbadc_sel = 2 (mask 0x30)
const BW_ADC_SEL_A: u32   = 0xC0EC;
const BW_ADC_SEL_B: u32   = 0xC1EC;
const BW_WBADC_SEL_A: u32 = 0xC0E4;
const BW_WBADC_SEL_B: u32 = 0xC1E4;

/// SCO mapping table (rtw8852bx_sco_mapping, rtw8852b_common.c:519):
///   ch 1 → 109
///   ch 2..6 → 108
///   ch 7..10 → 107
///   ch 11..14 → 106
fn sco_mapping_2g(ch: u8) -> u32 {
    match ch {
        1 => 109,
        2..=6 => 108,
        7..=10 => 107,
        11..=14 => 106,
        _ => 0,
    }
}

/// SCO Barker thresholds (rtw8852bx_sco_barker_threshold, ch 1..14)
const SCO_BARKER: [u32; 14] = [
    0x1CFEA, 0x1D0E1, 0x1D1D7, 0x1D2CD, 0x1D3C3, 0x1D4B9, 0x1D5B0,
    0x1D6A6, 0x1D79C, 0x1D892, 0x1D988, 0x1DA7F, 0x1DB75, 0x1DDC4,
];

/// SCO CCK thresholds (rtw8852bx_sco_cck_threshold, ch 1..14)
const SCO_CCK: [u32; 14] = [
    0x27DE3, 0x27F35, 0x28088, 0x281DA, 0x2832D, 0x2847F, 0x285D2,
    0x28724, 0x28877, 0x289C9, 0x28B1C, 0x28C6E, 0x28DC1, 0x290ED,
];

// ═══════════════════════════════════════════════════════════════════
//  set_channel(channel=ch, 2.4GHz, 20MHz)  — ch ∈ 1..13
// ═══════════════════════════════════════════════════════════════════

/// Convenience: default scan baseline = channel 1.
pub fn set_channel_1_2g(mmio: i32) { set_channel_2g(mmio, 1); }

pub fn set_channel_2g(mmio: i32, ch: u8) {
    host::print("  CHAN: set_channel(ch=");
    crate::fw::print_dec(ch as usize);
    host::print(", 2.4GHz, 20MHz)\n");

    // ── 1. set_channel_mac (__rtw8852bx_set_channel_mac) ─────────
    //   BW=20MHz → RFMOD mask clr (0 = 20MHz), TX_SUB_CARRIER = 0
    //   ch=1 is 2G (<=14) → TXRATE_CHK: set BAND_MODE, clear CCK_EN + RTS_LIMIT
    host::mmio_clr32(mmio, R_AX_WMAC_RFMOD, B_AX_WMAC_RFMOD_MASK);
    host::mmio_w32(mmio, R_AX_TX_SUB_CARRIER_VALUE, 0);
    host::mmio_set32(mmio, R_AX_TXRATE_CHK, B_AX_BAND_MODE);
    host::mmio_clr32(mmio, R_AX_TXRATE_CHK, B_AX_CHECK_CCK_EN | B_AX_RTS_LIMIT_IN_OFDM6);

    let ch_idx = (ch as usize).saturating_sub(1).min(13);

    // ── 2. set_channel_bb ────────────────────────────────────────
    //   2a. SCO CCK thresholds per channel (rtw8852bx_ctrl_sco_cck)
    host::mmio_w32_mask(mmio, CR + R_RXSCOBC, B_RXSCO_TH, SCO_BARKER[ch_idx]);
    host::mmio_w32_mask(mmio, CR + R_RXSCOCCK, B_RXSCO_TH, SCO_CCK[ch_idx]);

    //   2b. ctrl_ch: path A/B band_sel = 1 (2G), SCO comp per channel
    host::mmio_w32_mask(mmio, CR + R_PATH0_BAND_SEL_V1, B_PATH_BAND_SEL_V1, 1);
    host::mmio_w32_mask(mmio, CR + R_PATH1_BAND_SEL_V1, B_PATH_BAND_SEL_V1, 1);
    host::mmio_w32_mask(mmio, CR + R_FC0_BW_V1, B_FC0_BW_INV, sco_mapping_2g(ch));

    //   2c. CCK TX FIR coefficients for ch != 14 (ch 1 case)
    //   From rtw8852b_common.c:788-795
    let cck_fir: [(u32, u32); 8] = [
        (R_TXFIR0,        0x3D23FF),
        (R_TXFIR0 + 0x04, 0x29B354),
        (R_TXFIR0 + 0x08, 0x0FC1C8),
        (R_TXFIR0 + 0x0C, 0xFDB053),
        (R_TXFIR0 + 0x10, 0xF86F9A),
        (R_TXFIR0 + 0x14, 0xFAEF92),
        (R_TXFIR0 + 0x18, 0xFE5FCC),
        (R_TXFIR0 + 0x1C, 0xFFDFF5),
    ];
    for (addr, val) in cck_fir.iter() {
        host::mmio_w32_mask(mmio, CR + addr, 0xFFFFFF, *val);
    }

    //   2c.5 set_gain_error(2G, path A+B) — 1:1 Linux rtw8852bx_ctrl_ch.
    //   Reads LNA/TIA gain values stored in phy::BB_GAIN during BB-gain
    //   table parse and writes them into the per-path LNA/TIA gain
    //   registers. Without this step the RX analog front-end has no gain
    //   → zero frames reach the MAC. set_gain_offset/set_rxsc_rpl_comp
    //   need efuse data (not parsed yet), so they stay out for now.
    crate::phy::apply_gain_error_2g(mmio, 0);
    crate::phy::apply_gain_error_2g(mmio, 1);

    //   2d. ctrl_bw (20MHz, pri_ch=0):
    //       FC0_BW_SET = 0, CHBW_MOD_SBW = 0, CHBW_MOD_PRICH = 0
    //       RFMODE_ORI_RX both paths = 0x333
    //       bw_setting(20MHz, A+B): adc_sel = 0, wbadc_sel = 2
    host::mmio_w32_mask(mmio, CR + R_FC0_BW_V1, B_FC0_BW_SET, 0);
    host::mmio_w32_mask(mmio, CR + R_CHBW_MOD_V1, B_CHBW_MOD_SBW, 0);
    host::mmio_w32_mask(mmio, CR + R_CHBW_MOD_V1, B_CHBW_MOD_PRICH, 0);
    host::mmio_w32_mask(mmio, CR + R_P0_RFMODE_ORI_RX, B_RFMODE_ORI_RX_ALL, 0x333);
    host::mmio_w32_mask(mmio, CR + R_P1_RFMODE_ORI_RX, B_RFMODE_ORI_RX_ALL, 0x333);
    // bw_setting per path (rtw8852b_bw_setting, 20MHz):
    host::mmio_w32_mask(mmio, CR + BW_ADC_SEL_A,   0x6000, 0);
    host::mmio_w32_mask(mmio, CR + BW_WBADC_SEL_A, 0x0030, 2);
    host::mmio_w32_mask(mmio, CR + BW_ADC_SEL_B,   0x6000, 0);
    host::mmio_w32_mask(mmio, CR + BW_WBADC_SEL_B, 0x0030, 2);

    //   2e. ctrl_cck_en(true): UPD_CLK_ADC.ENABLE_CCK = 1, RXCCA.DIS = 0
    host::mmio_set32(mmio, CR + R_UPD_CLK_ADC, B_ENABLE_CCK);
    host::mmio_clr32(mmio, CR + R_RXCCA, B_RXCCA_DIS);

    //   2f. chan_idx encoding for 2G: BASE_IDX_2G(0)<<4 | ch = ch for 2G
    host::mmio_w32_mask(mmio, CR + R_MAC_PIN_SEL, B_CH_IDX_SEG0, ch as u32);

    // ── 3. set_channel_rf (rtw8852b_ctrl_bw_ch) ──────────────────
    //   3a. _ctrl_ch: _ch_setting for path A/B with dav=true and false
    //       8852b maps RR_CFGCH and RR_CFGCH_V1 to same RF reg 0x18
    //       via direct addressing (addr &= 0xff). So both pairs update
    //       the same register — we only need one write per path.
    //       Set CH=1, clear BAND bits (2G), set BW2, clear POW_LCK/TRX_AH/BCN.
    for path in 0u8..2 {
        let mut v = rf_read(mmio, path, RR_CFGCH);
        v &= !(RR_CFGCH_BAND1 | RR_CFGCH_POW_LCK | RR_CFGCH_TRX_AH
             | RR_CFGCH_BCN | RR_CFGCH_BAND0 | RR_CFGCH_CH | RR_CFGCH_BW2);
        v |= ch as u32;  // CH = ch (2G, no BAND bits)
        v |= RR_CFGCH_BW2;
        rf_write(mmio, path, RR_CFGCH, RFREG_MASK, v);
        // Trigger LCK: toggle LCKST.BIN 0 → 1
        rf_write(mmio, path, RR_LCKST, RR_LCKST_BIN, 0);
        rf_write(mmio, path, RR_LCKST, RR_LCKST_BIN, 1);
    }

    //   3b. _ctrl_bw: set BW bits = 20M (3) in CFGCH
    for path in 0u8..2 {
        let mut v = rf_read(mmio, path, RR_CFGCH);
        v &= !RR_CFGCH_BW;
        v |= CFGCH_BW_20M << 10;
        v &= !(RR_CFGCH_POW_LCK | RR_CFGCH_TRX_AH | RR_CFGCH_BCN | RR_CFGCH_BW2);
        v |= RR_CFGCH_BW2;
        rf_write(mmio, path, RR_CFGCH, RFREG_MASK, v);
    }

    //   3c. _rxbb_bw(20MHz): 4 RF writes per path
    //       LUTWE2.RTXBW = 1, LUTWA.M2 = 0x12, LUTWD0.LB = 0x1B, LUTWE2.RTXBW = 0
    for path in 0u8..2 {
        rf_write(mmio, path, RR_LUTWE2, RR_LUTWE2_RTXBW, 1);
        rf_write(mmio, path, RR_LUTWA,  RR_LUTWA_M2,     0x12);
        rf_write(mmio, path, RR_LUTWD0, RR_LUTWD0_LB,    0x1B);
        rf_write(mmio, path, RR_LUTWE2, RR_LUTWE2_RTXBW, 0);
    }

    host::print("  CHAN: RF tuned to channel ");
    crate::fw::print_dec(ch as usize);
    host::print(" (2.4GHz, 20MHz)\n");
}

// ═══════════════════════════════════════════════════════════════════
//  set_channel_help — 1:1 port of rtw8852b_set_channel_help
//  Wraps set_channel with ENTER (quiesce) and EXIT (re-enable) blocks.
//  Linux rtw8852b.c:627.
// ═══════════════════════════════════════════════════════════════════

// Register addresses used by help_enter/exit + bb_reset_en + adc_en + tssi_cont_en.
const R_ADC_FIFO: u32       = 0x20FC;
const B_ADC_FIFO_RST: u32   = 0xFF << 24;
const R_PD_CTRL: u32        = 0x0C3C;
const B_PD_HIT_DIS: u32     = 1 << 9;
const R_S0_HW_SI_DIS: u32   = 0x1200;
const R_S1_HW_SI_DIS: u32   = 0x3200;
const B_HW_SI_DIS_TRIG: u32 = 0x7 << 28;
const R_RSTB_ASYNC: u32     = 0x0704;
const B_RSTB_ASYNC_ALL: u32 = 1 << 1;
const R_P0_TSSI_TRK: u32    = 0x5818;
const R_P0_TXPW_RSTB: u32   = 0x58DC;
const R_P1_TSSI_TRK: u32    = 0x7818;
const R_P1_TXPW_RSTB: u32   = 0x78DC;
const B_TSSI_TRK_EN: u32    = 1 << 30;
const B_TXPW_RSTB_MANON: u32 = 1 << 30;
const R_AX_PPDU_STAT: u32   = 0xCE40;
const B_AX_PPDU_STAT_RPT_EN: u32 = 1 << 0;
const B_AX_PPDU_STAT_RPT_CRC32: u32 = 1 << 5;
const B_AX_APP_PLCP_HDR_RPT: u32 = 1 << 3;
const B_AX_APP_MAC_INFO_RPT: u32 = 1 << 1;
const R_AX_HW_RPT_FWD: u32  = 0xBF0C;
const B_AX_FWD_PPDU_STAT_MASK: u32 = 0x3 << 24;
const RTW89_PRPT_DEST_HOST: u32 = 0;

/// _tssi_cont_en / rtw8852b_tssi_cont_en_phyidx with en=false/true.
fn tssi_cont_en(mmio: i32, en: bool) {
    let v = if en { 0 } else { 1 };
    host::mmio_w32_mask(mmio, CR + R_P0_TXPW_RSTB, B_TXPW_RSTB_MANON, v);
    host::mmio_w32_mask(mmio, CR + R_P0_TSSI_TRK,  B_TSSI_TRK_EN,     v);
    host::mmio_w32_mask(mmio, CR + R_P1_TXPW_RSTB, B_TXPW_RSTB_MANON, v);
    host::mmio_w32_mask(mmio, CR + R_P1_TSSI_TRK,  B_TSSI_TRK_EN,     v);
}

/// rtw8852b_adc_en — en=false: ADC_FIFO_RST=0xf (reset), en=true: =0 (run).
fn adc_en(mmio: i32, en: bool) {
    let v = if en { 0 } else { 0xF };
    host::mmio_w32_mask(mmio, CR + R_ADC_FIFO, B_ADC_FIFO_RST, v);
}

/// rtw8852b_bb_reset_en (Linux rtw8852b.c:542, 2G branch only).
fn bb_reset_en(mmio: i32, en: bool) {
    if en {
        host::mmio_w32_mask(mmio, CR + R_S0_HW_SI_DIS, B_HW_SI_DIS_TRIG, 0x0);
        host::mmio_w32_mask(mmio, CR + R_S1_HW_SI_DIS, B_HW_SI_DIS_TRIG, 0x0);
        host::mmio_w32_mask(mmio, CR + R_RSTB_ASYNC,   B_RSTB_ASYNC_ALL, 1);
        // 2G: clear RXCCA_DIS
        host::mmio_w32_mask(mmio, CR + R_RXCCA,   B_RXCCA_DIS,   0x0);
        host::mmio_w32_mask(mmio, CR + R_PD_CTRL, B_PD_HIT_DIS,  0x0);
    } else {
        host::mmio_w32_mask(mmio, CR + R_RXCCA,   B_RXCCA_DIS,   0x1);
        host::mmio_w32_mask(mmio, CR + R_PD_CTRL, B_PD_HIT_DIS,  0x1);
        host::mmio_w32_mask(mmio, CR + R_S0_HW_SI_DIS, B_HW_SI_DIS_TRIG, 0x7);
        host::mmio_w32_mask(mmio, CR + R_S1_HW_SI_DIS, B_HW_SI_DIS_TRIG, 0x7);
        host::sleep_ms(1); // fsleep(1) in Linux
        host::mmio_w32_mask(mmio, CR + R_RSTB_ASYNC,   B_RSTB_ASYNC_ALL, 0);
    }
}

/// rtw89_mac_cfg_ppdu_status — Linux mac.c:6155.
fn mac_cfg_ppdu_status(mmio: i32, enable: bool) {
    if !enable {
        host::mmio_clr32(mmio, R_AX_PPDU_STAT, B_AX_PPDU_STAT_RPT_EN);
        return;
    }
    host::mmio_w32(mmio, R_AX_PPDU_STAT,
        B_AX_PPDU_STAT_RPT_EN | B_AX_APP_MAC_INFO_RPT
        | B_AX_APP_PLCP_HDR_RPT | B_AX_PPDU_STAT_RPT_CRC32);
    host::mmio_w32_mask(mmio, R_AX_HW_RPT_FWD,
        B_AX_FWD_PPDU_STAT_MASK, RTW89_PRPT_DEST_HOST);
}

/// 1:1 port of rtw8852b_set_channel_help(enter=true).
/// Quiesces TX, TSSI, ADC, PPDU reporting and disables BB reset before
/// set_channel runs. Matches Linux rtw8852b.c:633.
pub fn set_channel_help_enter(mmio: i32) {
    mac_cfg_ppdu_status(mmio, false);
    tssi_cont_en(mmio, false);
    adc_en(mmio, false);
    host::sleep_ms(1); // fsleep(40)
    bb_reset_en(mmio, false);
}

/// 1:1 port of rtw8852b_set_channel_help(enter=false) — the EXIT path.
/// Re-enables ADC, TSSI, PPDU reporting and kicks BB reset. Without
/// this the chip stays in the quiesced state and RX is dead.
pub fn set_channel_help_exit(mmio: i32) {
    mac_cfg_ppdu_status(mmio, true);
    adc_en(mmio, true);
    tssi_cont_en(mmio, true);
    bb_reset_en(mmio, true);
}

// ═══════════════════════════════════════════════════════════════════
//  apply_default_txpwr — smoke-test stand-in for rtw8852bx_set_txpwr
//
//  Linux's set_txpwr pipeline reads per-rate dBm values from efuse
//  (set_txpwr_byrate + offset + limit + limit_ru + diff). We don't
//  parse efuse, so for the Phase 7 smoke test we uniformly fill the
//  per-rate table with a safe 20 dBm (= 0x50 in 0.25-dBm units) so
//  HW has *something* to transmit at. Without this the table reads
//  0 and the PA stays at minimum output — AP never sees our frame.
//
//  R_AX_PWR_BY_RATE_TABLE0..10 = 0xD2C0..0xD2E8, 11 dwords.
//  Each byte = one rate's power setting in 0.25-dBm units.
//  0x50 = 80 → 20 dBm = 100 mW (2.4G legal maximum most regions).
// ═══════════════════════════════════════════════════════════════════

const R_AX_PWR_BY_RATE_TABLE0: u32 = 0xD2C0;
const R_AX_PWR_BY_RATE_TABLE10: u32 = 0xD2E8;

// R_AX_PWR_RATE_CTRL (0xD200):
//   bits 27..10 = B_AX_PWR_REF (signed s18, 0.25 dBm units)
//   bit  9      = B_AX_FORCE_PWR_BY_RATE_EN
//   bits 8..0   = B_AX_FORCE_PWR_BY_RATE_VALUE_MASK
// When FORCE_PWR_BY_RATE_EN=1, HW ignores the per-rate table and
// transmits every frame at VALUE. Useful smoke-test override until
// we port the full rtw8852bx_set_txpwr_ref/offset/limit pipeline.
const R_AX_PWR_RATE_CTRL: u32 = 0xD200;

pub fn apply_default_txpwr(mmio: i32) {
    // Fill per-rate table uniformly with 0x50 = 20 dBm (0.25-dBm units).
    let v: u32 = 0x50505050;
    let mut off = R_AX_PWR_BY_RATE_TABLE0;
    while off <= R_AX_PWR_BY_RATE_TABLE10 {
        host::mmio_w32(mmio, off, v);
        off += 4;
    }

    // Force all rates to 20 dBm regardless of what the per-rate table
    // says, and set PWR_REF to 0 (neutral — no offset). This gives HW
    // a clear target and stops the PA from idling at minimum output.
    // bit 9 = FORCE_EN, bits 0..8 = FORCE_VALUE (0x50 = 20 dBm).
    let ctrl: u32 = (1 << 9) | 0x50;
    host::mmio_w32(mmio, R_AX_PWR_RATE_CTRL, ctrl);
}
