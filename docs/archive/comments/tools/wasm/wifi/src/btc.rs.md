# `tools/wasm/wifi/src/btc.rs` @ 5e0102684

## L1-33 · `use crate::host;`

```
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
```

## L38 · `const R_AX_GPIO_MUXCFG:     u32 = 0x0040;`

```
// ── PCIe MMIO register addresses (reg.h / mac.h, verified) ─────────
```

## L43 · `const R_AX_LTE_SW_CFG_2:    u32 = 0x003C; // indirect via LTE_CTRL`

```
// indirect via LTE_CTRL
```

## L57 · `const B_AX_ENBT:                 u32 = 1 << 5;`

```
// Bit masks
```

## L74 · `const MAC_AX_BT_MODE_0_3: u32 = 0;`

```
// Constants
```

## L79 · `const RR_WLSEL:  u32 = 0x02;`

```
// RF
```

## L86 · `const BTC_BT_SS_GROUP: u32 = 0x0;`

```
// BT/WL coex groups
```

## L90-92 · `fn lte_wait_ready(mmio: i32) -> bool {`

```
// ── LTE indirect access (mac.c:86/102) ─────────────────────────────
// Reads/writes to the 0x003C-ish LTE space go through the dedicated
// CTRL/WDATA/RDATA triplet.
```

## L94 · `for _ in 0..1000u32 {`

```
// Poll R_AX_LTE_CTRL+3 byte for BIT(5)
```

## L115 · `fn mac_coex_init(mmio: i32) {`

```
// ── rtw89_mac_coex_init (mac.c:6243) ───────────────────────────────
```

## L117 · `host::mmio_set32(mmio, R_AX_GPIO_MUXCFG, B_AX_ENBT);`

```
// 1) GPIO_MUXCFG: set B_AX_ENBT
```

## L120-121 · `host::mmio_set32(mmio, R_AX_BTC_FUNC_EN, B_AX_PTA_WL_TX_EN);`

```
// 2) BTC_FUNC_EN: set PTA_WL_TX_EN  ← THE CRITICAL BIT
//    8852B is not 8851B or 8852BT, so this path applies.
```

## L124 · `host::mmio_set32(mmio, R_AX_BT_COEX_CFG_2, B_AX_GNT_BT_POLARITY);`

```
// 3) BT_COEX_CFG_2 high byte: set GNT_BT_POLARITY (bit 8 = bit 0 of byte+1)
```

## L127 · `host::mmio_set32(mmio, R_AX_CSR_MODE, B_AX_STATIS_BT_EN | B_AX_WL_ACT_MSK);`

```
// 4) CSR_MODE: set STATIS_BT_EN | WL_ACT_MSK
```

## L129 · `host::mmio_set32(mmio, R_AX_CSR_MODE, B_AX_BT_CNT_RST);`

```
// 4b) CSR_MODE+2: set BT_CNT_RST (bit 16)
```

## L132 · `host::mmio_clr32(mmio, R_AX_TRXPTCL_RESP_0, B_AX_RSP_CHK_BTCCA);`

```
// 5) TRXPTCL_RESP_0+3: clear RSP_CHK_BTCCA (bit 25)
```

## L135 · `host::mmio_set32(mmio, R_AX_CCA_CFG_0, B_AX_BTCCA_EN);`

```
// 6) CCA_CFG_0: set BTCCA_EN, clear BTCCA_BRK_TXOP_EN
```

## L139 · `let val32 = lte_read(mmio, R_AX_LTE_SW_CFG_2) & B_AX_WL_RX_CTRL;`

```
// 7) LTE indirect: LTE_SW_CFG_2 &= WL_RX_CTRL (keep only that bit)
```

## L143-144 · `host::mmio_w32_mask(mmio, R_AX_GPIO_MUXCFG, B_AX_BTMODE_MASK,`

```
// 8) PTA mode = RTK:
//    GPIO_MUXCFG: BTMODE_MASK = BT_MODE_0_3 (0)
```

## L147 · `host::mmio_set32(mmio, R_AX_TDMA_MODE, B_AX_RTK_BT_ENABLE);`

```
//    TDMA_MODE: set RTK_BT_ENABLE
```

## L149 · `host::mmio_w32_mask(mmio, R_AX_BT_COEX_CFG_5,`

```
//    BT_COEX_CFG_5: sample-rate mask = RTK_RATE (5)
```

## L153-154 · `host::mmio_set32(mmio, R_AX_GPIO_MUXCFG, 1 << 9);`

```
// 9) Direction = INNER: GPIO_MUXCFG+1 set BIT1, clear BIT2
//    (equivalent: set bit 9, clear bit 10 of 32-bit reg)
```

## L159-160 · `fn set_trx_mask(mmio: i32, path: u8, group: u32, val: u32) {`

```
// ── rtw8852bx_set_trx_mask (rtw8852b_common.c:1789) ────────────────
// RF LUT write for per-coex-group TRX mask.
```

## L168 · `fn init_cfg_8852b(mmio: i32) {`

```
// ── __rtw8852bx_btc_init_cfg (rtw8852b_common.c:1797) ──────────────
```

## L170 · `host::mmio_set32(mmio, R_BTC_BT_COEX_MSK_TABLE, B_BTC_PRI_MASK_TX_RESP_V1);`

```
// mac_coex_init done by caller.
```

## L172 · `host::mmio_set32(mmio, R_BTC_BT_COEX_MSK_TABLE, B_BTC_PRI_MASK_TX_RESP_V1);`

```
// WL priorities: TX_RESP + BEACON high-priority vs BT
```

## L176 · `rf_write_mask(mmio, 0, RR_WLSEL, RFREG_MASK, 0x0);`

```
// RF GNT-debug OFF on both paths
```

## L180 · `set_trx_mask(mmio, 0, BTC_BT_SS_GROUP, 0x5FF);`

```
// SHARED-antenna group TRX masks — 8852B ant_type=SHARED always
```

## L186 · `host::mmio_w32(mmio, R_BTC_BREAK_TABLE, BTC_BREAK_PARAM);`

```
// PTA break-table constant
```

## L189 · `host::mmio_set32(mmio, R_AX_CSR_MODE, B_AX_BT_CNT_RST | B_AX_STATIS_BT_EN);`

```
// Redundant but mirrors Linux: CSR_MODE set both counters
```

## L193-196 · `pub fn init(mmio: i32) {`

```
// ── Public entry — mirrors rtw89_btc_ntfy_init(BTC_MODE_NORMAL) ────
/// Run this after MAC/PHY/RF register init but before hci_start so
/// the PTA is ready to let WiFi TX through by the time the first
/// frame gets DMAed.
```

## L198 · `let pre_btc_fn  = host::mmio_r32(mmio, R_AX_BTC_FUNC_EN);`

```
// Pre-state — baseline reads so we can see what changed.
```

