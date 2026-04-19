//! BT-Coex init — ports Linux rtw89_btc_ntfy_init (coex.c:7746) +
//! rtw89_mac_coex_init (mac.c:6243) + __rtw8852bx_btc_init_cfg
//! (rtw8852b_common.c:1797).
//!
//! The chip has a shared WiFi+BT antenna on 8852BE (ant.num=2, type=
//! SHARED, bt_pos=BTG). Out of reset, the PTA (Packet Traffic Arbiter)
//! arbitrates the shared resources and — critically — has
//! B_AX_PTA_WL_TX_EN at default 0 so WiFi mgmt/data TX is silenced
//! until coex init explicitly enables it. That matches our post-v1.44
//! sniffer finding: 0 frames on-air from the NUC while IQK/DPK/TSSI
//! all run fine internally.
//!
//! Linux chain we replicate:
//!   rtw89_mac_coex_init(RTK mode, INNER direction):
//!     R_AX_GPIO_MUXCFG |= ENBT                  (enable BT GPIO)
//!     R_AX_BTC_FUNC_EN |= PTA_WL_TX_EN          ← THE BIT
//!     R_AX_BT_COEX_CFG_2 high-byte |= GNT_BT_POLARITY
//!     R_AX_CSR_MODE |= STATIS_BT_EN | WL_ACT_MSK
//!     R_AX_CSR_MODE+2 |= BT_CNT_RST>>16
//!     R_AX_TRXPTCL_RESP_0+3 &= ~RSP_CHK_BTCCA>>24
//!     R_AX_CCA_CFG_0: set BTCCA_EN, clr BTCCA_BRK_TXOP_EN
//!     LTE indirect: R_AX_LTE_SW_CFG_2 &= ~WL_RX_CTRL
//!     RTK mode setup on R_AX_GPIO_MUXCFG (BT_MODE_0_3), R_AX_TDMA_MODE
//!     (RTK_BT_ENABLE), R_AX_BT_COEX_CFG_5 (RPT sample rate)
//!     INNER direction: R_AX_GPIO_MUXCFG+1 set BIT1, clr BIT2
//!
//!   __rtw8852bx_btc_init_cfg (after mac_coex_init):
//!     set_wl_pri(TX_RESP, true)  → R_BTC_BT_COEX_MSK_TABLE |= BIT3
//!     set_wl_pri(BEACON, true)   → R_AX_WL_PRI_MSK |= BIT8
//!     write_rf(A+B, RR_WLSEL, 0) → RF GNT debug off
//!     set_trx_mask(SHARED, 4 × LUT writes per path)
//!     R_BTC_BREAK_TABLE = BTC_BREAK_PARAM (0xf0ffffff)
//!     R_AX_CSR_MODE |= BT_CNT_RST | STATIS_BT_EN

use crate::host;
use crate::phy::{rf_write_mask};

// ── PCIe MMIO register addresses (reg.h / mac.h, verified) ─────────
const R_AX_GPIO_MUXCFG:     u32 = 0x0040;
const R_AX_LTE_CTRL:        u32 = 0xDAF0;
const R_AX_LTE_WDATA:       u32 = 0xDAF4;
const R_AX_LTE_RDATA:       u32 = 0xDAF8;
const R_AX_LTE_SW_CFG_2:    u32 = 0x003C; // indirect via LTE_CTRL

const R_AX_CCA_CFG_0:       u32 = 0xC340;
const R_AX_TRXPTCL_RESP_0:  u32 = 0xCC04;

const R_AX_WL_PRI_MSK:      u32 = 0xDA10;
const R_AX_BTC_FUNC_EN:     u32 = 0xDA20;
const R_BTC_BREAK_TABLE:    u32 = 0xDA2C;
const R_BTC_BT_COEX_MSK_TABLE: u32 = 0xDA30;
const R_AX_BT_COEX_CFG_2:   u32 = 0xDA34;
const R_AX_CSR_MODE:        u32 = 0xDA40;
const R_AX_TDMA_MODE:       u32 = 0xDA4C;
const R_AX_BT_COEX_CFG_5:   u32 = 0xDA6C;

// Bit masks
const B_AX_ENBT:                 u32 = 1 << 5;
const B_AX_PTA_WL_TX_EN:         u32 = 1 << 1;
const B_AX_GNT_BT_POLARITY:      u32 = 1 << 8;
const B_AX_STATIS_BT_EN:         u32 = 1 << 2;
const B_AX_WL_ACT_MSK:           u32 = 1 << 3;
const B_AX_BT_CNT_RST:           u32 = 1 << 16;
const B_AX_RSP_CHK_BTCCA:        u32 = 1 << 25;
const B_AX_BTCCA_EN:             u32 = 1 << 5;
const B_AX_BTCCA_BRK_TXOP_EN:    u32 = 1 << 9;
const B_AX_WL_RX_CTRL:           u32 = 1 << 8;
const B_AX_BTMODE_MASK:          u32 = 0x3 << 6;
const B_AX_RTK_BT_ENABLE:        u32 = 1 << 0;
const B_AX_BT_RPT_SAMPLE_RATE_MASK: u32 = 0x3F;
const B_AX_PTA_WL_PRI_MASK_BCNQ: u32 = 1 << 8;
const B_BTC_PRI_MASK_TX_RESP_V1: u32 = 1 << 3;

// Constants
const MAC_AX_BT_MODE_0_3: u32 = 0;
const MAC_AX_RTK_RATE:    u32 = 5;
const BTC_BREAK_PARAM:    u32 = 0xF0FF_FFFF;

// RF
const RR_WLSEL:  u32 = 0x02;
const RR_LUTWE:  u32 = 0xEF;
const RR_LUTWA:  u32 = 0x33;
const RR_LUTWD0: u32 = 0x3F;
const RFREG_MASK: u32 = 0xF_FFFF;

// BT/WL coex groups
const BTC_BT_SS_GROUP: u32 = 0x0;
const BTC_BT_TX_GROUP: u32 = 0x2;

// ── LTE indirect access (mac.c:86/102) ─────────────────────────────
// Reads/writes to the 0x003C-ish LTE space go through the dedicated
// CTRL/WDATA/RDATA triplet.
fn lte_wait_ready(mmio: i32) -> bool {
    // Poll R_AX_LTE_CTRL+3 byte for BIT(5)
    for _ in 0..1000u32 {
        let v = host::mmio_r32(mmio, R_AX_LTE_CTRL) >> 24;
        if v & (1 << 5) != 0 { return true; }
        host::sleep_ms(1);
    }
    false
}

fn lte_write(mmio: i32, offset: u32, val: u32) {
    lte_wait_ready(mmio);
    host::mmio_w32(mmio, R_AX_LTE_WDATA, val);
    host::mmio_w32(mmio, R_AX_LTE_CTRL, 0xC00F_0000 | offset);
}

fn lte_read(mmio: i32, offset: u32) -> u32 {
    lte_wait_ready(mmio);
    host::mmio_w32(mmio, R_AX_LTE_CTRL, 0x800F_0000 | offset);
    host::mmio_r32(mmio, R_AX_LTE_RDATA)
}

// ── rtw89_mac_coex_init (mac.c:6243) ───────────────────────────────
fn mac_coex_init(mmio: i32) {
    // 1) GPIO_MUXCFG: set B_AX_ENBT
    host::mmio_set32(mmio, R_AX_GPIO_MUXCFG, B_AX_ENBT);

    // 2) BTC_FUNC_EN: set PTA_WL_TX_EN  ← THE CRITICAL BIT
    //    8852B is not 8851B or 8852BT, so this path applies.
    host::mmio_set32(mmio, R_AX_BTC_FUNC_EN, B_AX_PTA_WL_TX_EN);

    // 3) BT_COEX_CFG_2 high byte: set GNT_BT_POLARITY (bit 8 = bit 0 of byte+1)
    host::mmio_set32(mmio, R_AX_BT_COEX_CFG_2, B_AX_GNT_BT_POLARITY);

    // 4) CSR_MODE: set STATIS_BT_EN | WL_ACT_MSK
    host::mmio_set32(mmio, R_AX_CSR_MODE, B_AX_STATIS_BT_EN | B_AX_WL_ACT_MSK);
    // 4b) CSR_MODE+2: set BT_CNT_RST (bit 16)
    host::mmio_set32(mmio, R_AX_CSR_MODE, B_AX_BT_CNT_RST);

    // 5) TRXPTCL_RESP_0+3: clear RSP_CHK_BTCCA (bit 25)
    host::mmio_clr32(mmio, R_AX_TRXPTCL_RESP_0, B_AX_RSP_CHK_BTCCA);

    // 6) CCA_CFG_0: set BTCCA_EN, clear BTCCA_BRK_TXOP_EN
    host::mmio_set32(mmio, R_AX_CCA_CFG_0, B_AX_BTCCA_EN);
    host::mmio_clr32(mmio, R_AX_CCA_CFG_0, B_AX_BTCCA_BRK_TXOP_EN);

    // 7) LTE indirect: LTE_SW_CFG_2 &= WL_RX_CTRL (keep only that bit)
    let val32 = lte_read(mmio, R_AX_LTE_SW_CFG_2) & B_AX_WL_RX_CTRL;
    lte_write(mmio, R_AX_LTE_SW_CFG_2, val32);

    // 8) PTA mode = RTK:
    //    GPIO_MUXCFG: BTMODE_MASK = BT_MODE_0_3 (0)
    host::mmio_w32_mask(mmio, R_AX_GPIO_MUXCFG, B_AX_BTMODE_MASK,
                        MAC_AX_BT_MODE_0_3 << 6);
    //    TDMA_MODE: set RTK_BT_ENABLE
    host::mmio_set32(mmio, R_AX_TDMA_MODE, B_AX_RTK_BT_ENABLE);
    //    BT_COEX_CFG_5: sample-rate mask = RTK_RATE (5)
    host::mmio_w32_mask(mmio, R_AX_BT_COEX_CFG_5,
                        B_AX_BT_RPT_SAMPLE_RATE_MASK, MAC_AX_RTK_RATE);

    // 9) Direction = INNER: GPIO_MUXCFG+1 set BIT1, clear BIT2
    //    (equivalent: set bit 9, clear bit 10 of 32-bit reg)
    host::mmio_set32(mmio, R_AX_GPIO_MUXCFG, 1 << 9);
    host::mmio_clr32(mmio, R_AX_GPIO_MUXCFG, 1 << 10);
}

// ── rtw8852bx_set_trx_mask (rtw8852b_common.c:1789) ────────────────
// RF LUT write for per-coex-group TRX mask.
fn set_trx_mask(mmio: i32, path: u8, group: u32, val: u32) {
    rf_write_mask(mmio, path, RR_LUTWE,  RFREG_MASK, 0x20000);
    rf_write_mask(mmio, path, RR_LUTWA,  RFREG_MASK, group);
    rf_write_mask(mmio, path, RR_LUTWD0, RFREG_MASK, val);
    rf_write_mask(mmio, path, RR_LUTWE,  RFREG_MASK, 0x0);
}

// ── __rtw8852bx_btc_init_cfg (rtw8852b_common.c:1797) ──────────────
fn init_cfg_8852b(mmio: i32) {
    // mac_coex_init done by caller.

    // WL priorities: TX_RESP + BEACON high-priority vs BT
    host::mmio_set32(mmio, R_BTC_BT_COEX_MSK_TABLE, B_BTC_PRI_MASK_TX_RESP_V1);
    host::mmio_set32(mmio, R_AX_WL_PRI_MSK,         B_AX_PTA_WL_PRI_MASK_BCNQ);

    // RF GNT-debug OFF on both paths
    rf_write_mask(mmio, 0, RR_WLSEL, RFREG_MASK, 0x0);
    rf_write_mask(mmio, 1, RR_WLSEL, RFREG_MASK, 0x0);

    // SHARED-antenna group TRX masks — 8852B ant_type=SHARED always
    set_trx_mask(mmio, 0, BTC_BT_SS_GROUP, 0x5FF);
    set_trx_mask(mmio, 1, BTC_BT_SS_GROUP, 0x5FF);
    set_trx_mask(mmio, 0, BTC_BT_TX_GROUP, 0x5FF);
    set_trx_mask(mmio, 1, BTC_BT_TX_GROUP, 0x55F);

    // PTA break-table constant
    host::mmio_w32(mmio, R_BTC_BREAK_TABLE, BTC_BREAK_PARAM);

    // Redundant but mirrors Linux: CSR_MODE set both counters
    host::mmio_set32(mmio, R_AX_CSR_MODE, B_AX_BT_CNT_RST | B_AX_STATIS_BT_EN);
}

// ── Public entry — mirrors rtw89_btc_ntfy_init(BTC_MODE_NORMAL) ────
/// Run this after MAC/PHY/RF register init but before hci_start so
/// the PTA is ready to let WiFi TX through by the time the first
/// frame gets DMAed.
pub fn init(mmio: i32) {
    host::print("  BTC: mac_coex_init (PTA_WL_TX_EN)\n");
    mac_coex_init(mmio);
    host::print("  BTC: init_cfg_8852b (WL priorities + SHARED TRX masks)\n");
    init_cfg_8852b(mmio);
    host::print("  BTC: init done\n");
}
