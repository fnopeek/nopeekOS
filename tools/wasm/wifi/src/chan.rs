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

// ═══════════════════════════════════════════════════════════════════
//  set_channel(channel=1, 2.4GHz, 20MHz)
// ═══════════════════════════════════════════════════════════════════

pub fn set_channel_1_2g(mmio: i32) {
    host::print("  CHAN: set_channel(ch=1, 2.4GHz, 20MHz)\n");

    // ── 1. set_channel_mac (__rtw8852bx_set_channel_mac) ─────────
    //   BW=20MHz → RFMOD mask clr (0 = 20MHz), TX_SUB_CARRIER = 0
    //   ch=1 is 2G (<=14) → TXRATE_CHK: set BAND_MODE, clear CCK_EN + RTS_LIMIT
    host::mmio_clr32(mmio, R_AX_WMAC_RFMOD, B_AX_WMAC_RFMOD_MASK);
    host::mmio_w32(mmio, R_AX_TX_SUB_CARRIER_VALUE, 0);
    host::mmio_set32(mmio, R_AX_TXRATE_CHK, B_AX_BAND_MODE);
    host::mmio_clr32(mmio, R_AX_TXRATE_CHK, B_AX_CHECK_CCK_EN | B_AX_RTS_LIMIT_IN_OFDM6);

    // ── 2. set_channel_bb ────────────────────────────────────────
    //   2a. SCO CCK thresholds for ch 1 (rtw8852bx_ctrl_sco_cck)
    host::mmio_w32_mask(mmio, CR + R_RXSCOBC, B_RXSCO_TH, 0x1CFEA);
    host::mmio_w32_mask(mmio, CR + R_RXSCOCCK, B_RXSCO_TH, 0x27DE3);

    //   2b. ctrl_ch: path A/B band_sel = 1 (2G), SCO comp = 109 (ch 1)
    host::mmio_w32_mask(mmio, CR + R_PATH0_BAND_SEL_V1, B_PATH_BAND_SEL_V1, 1);
    host::mmio_w32_mask(mmio, CR + R_PATH1_BAND_SEL_V1, B_PATH_BAND_SEL_V1, 1);
    host::mmio_w32_mask(mmio, CR + R_FC0_BW_V1, B_FC0_BW_INV, 109);

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

    //   2f. chan_idx encoding for 2G ch=1: BASE_IDX_2G(0)<<4 | ch(1) = 0x01
    host::mmio_w32_mask(mmio, CR + R_MAC_PIN_SEL, B_CH_IDX_SEG0, 0x01);

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
        v |= 1; // CH = 1 (2G)
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

    host::print("  CHAN: RF tuned to channel 1 (2.4GHz, 20MHz)\n");
}
