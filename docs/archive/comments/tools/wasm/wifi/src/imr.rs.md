# `tools/wasm/wifi/src/imr.rs` @ 5e0102684

## L1-17 · `use crate::host;`

```
//! IMR enable — Interrupt-Mask Register setup for DMAC + CMAC sub-blocks.
//!
//! 1:1 port of Linux `enable_imr_ax` (mac.c:3836) which dispatches to 11
//! per-block DMAC IMR enables and 6 per-block CMAC IMR enables. Each
//! enable clears a block-specific "not interesting" mask and sets a
//! block-specific "catch these errors" mask, using the chip-specific
//! values from `rtw89_imr_info rtw8852b_imr_info` (rtw8852b.c:157).
//!
//! Without the per-block IMRs, Linux observes that some error sources
//! never get propagated to the ISR path — which for 8852B appears to
//! include the CH12 H2C DONE/REC_ACK path. Our v1.0/v1.1 wedging after
//! the first VIF H2C was silent (no C2H of any kind for multiple
//! seconds) which is exactly that symptom.
//!
//! Register addresses + mask values inlined from Linux reg.h with line
//! refs. All values pre-computed from the combined `#define B_AX_*_IMR_*`
//! macros (each composed of 5-30 BIT() constituents).
```

## L21 · `const R_AX_HOST_DISPATCHER_ERR_IMR: u32   = 0x8850;`

```
// ── DMAC-seitige IMR-Register ────────────────────────────────────
```

## L41 · `const R_AX_SCHEDULE_ERR_IMR:        u32   = 0xC3E8;`

```
// ── CMAC-seitige IMR-Register (mac_idx=0 → reg + 0*0x2000) ────────
```

## L49-103 · `const WDRLS_IMR_SET:        u32 = 0x0000_3327;`

```
// ── Combined mask values (pre-computed from Linux reg.h combined
//    macro-definitions). All Linux-values verified via macro-expand
//    script (sum of constituent BIT() defines):
//
//    B_AX_WDRLS_IMR_SET       = 0x00003327
//    B_AX_WDRLS_IMR_EN_CLR    = 0x00003337
//    B_AX_IMR_ERROR           = 0x00000008     (BIT(3))
//    B_AX_STA_SCHEDULER_IMR_SET = 0x00000007
//    B_AX_WDE_IMR_SET         = 0x070FF0FF
//    B_AX_WDE_IMR_CLR         = 0x070FF0FF
//    B_AX_PLE_IMR_SET         = 0x070FF0DF
//    B_AX_PLE_IMR_CLR         = 0x070FF0FF
//    B_AX_HOST_DISP_IMR_SET   = 0x0C000161
//    B_AX_HOST_DISP_IMR_CLR   = 0xFF0FFFFF
//    B_AX_CPU_DISP_IMR_SET    = 0x04000062
//    B_AX_CPU_DISP_IMR_CLR    = 0xFF07FFFF
//    B_AX_OTHER_DISP_IMR_CLR  = 0x3F031F1F   (SET = 0 in 8852B imr_info)
//    B_AX_CPUIO_IMR_SET       = 0x00001111
//    B_AX_CPUIO_IMR_CLR       = 0x00001111
//    B_AX_BBRPT_CHINFO_IMR_CLR = 0x000000FF  (SET = 0 in 8852B imr_info)
//    B_AX_PTCL_IMR_SET        = 0x10800001
//    B_AX_PTCL_IMR_CLR_ALL    = 0xFFFFFFFF
//    B_AX_DLE_IMR_SET         = 0x0000C000
//    B_AX_DLE_IMR_CLR         = 0x0080C000
//    B_AX_RMAC_IMR_SET        = 0x000E4000
//    B_AX_RMAC_IMR_CLR        = 0x000FF000
//    B_AX_TMAC_IMR_SET        = 0x00000780
//    B_AX_TMAC_IMR_CLR        = 0x00000780
//    B_AX_TXPKTCTL_IMR_B0_SET = 0x00000101
//    B_AX_TXPKTCTL_IMR_B0_CLR = 0x0000030F
//    B_AX_TXPKTCTL_IMR_B1_SET = 0x00000303
//    B_AX_TXPKTCTL_IMR_B1_CLR = 0x0000030F
//
// Individual bits used in mpdu_trx / sta_sch / pktin / bbrpt / sched:
//    B_AX_TX_GET_ERRPKTID_INT_EN     = BIT(1) = 0x02
//    B_AX_TX_NXT_ERRPKTID_INT_EN     = BIT(2) = 0x04
//    B_AX_TX_MPDU_SIZE_ZERO_INT_EN   = BIT(3) = 0x08
//    B_AX_TX_OFFSET_ERR_INT_EN       = BIT(4) = 0x10
//    B_AX_TX_HDR3_SIZE_ERR_INT_EN    = BIT(5) = 0x20
//        → mpdu_tx_err combined CLR = 0x3E
//    B_AX_GETPKTID_ERR_INT_EN        = BIT(0) = 0x01
//    B_AX_MHDRLEN_ERR_INT_EN         = BIT(1) = 0x02
//    B_AX_RPT_ERR_INT_EN             = BIT(3) = 0x08
//        → mpdu_rx_err combined CLR = 0x0B
//    B_AX_SEARCH_HANG_TIMEOUT_INT_EN = BIT(0) = 0x01
//    B_AX_RPT_HANG_TIMEOUT_INT_EN    = BIT(1) = 0x02
//    B_AX_PLE_B_PKTID_ERR_INT_EN     = BIT(2) = 0x04
//        → sta_sch combined CLR     = 0x07
//    B_AX_PKTIN_GETPKTID_ERR_INT_EN  = BIT(0) = 0x01
//    B_AX_BBRPT_COM_NULL_PLPKTID_ERR_INT_EN = BIT(0) = 0x01
//    B_AX_BBRPT_DFS_TO_ERR_INT_EN    = BIT(0) = 0x01
//    B_AX_LA_IMR_DATA_LOSS_ERR       = BIT(0) = 0x01
//    B_AX_SORT_NON_IDLE_ERR_INT_EN   = BIT(1) = 0x02
//    B_AX_FSM_TIMEOUT_ERR_INT_EN     = BIT(0) = 0x01
//        → scheduler combined CLR   = 0x03
```

## L128 · `const BBRPT_COM_SET:        u32 = 0x0000_0001;  // NULL_PLPKTID`

```
// NULL_PLPKTID
```

## L133 · `const SCHEDULER_IMR_CLR:    u32 = 0x0000_0003;  // SORT_NON_IDLE | FSM_TIMEOUT`

```
// CMAC
```

## L134 · `const SCHEDULER_IMR_CLR:    u32 = 0x0000_0003;  // SORT_NON_IDLE | FSM_TIMEOUT`

```
// SORT_NON_IDLE | FSM_TIMEOUT
```

## L135 · `const SCHEDULER_IMR_SET:    u32 = 0x0000_0001;  // FSM_TIMEOUT`

```
// FSM_TIMEOUT
```

## L145-146 · `fn idx(reg: u32, mac_idx: u8) -> u32 {`

```
/// Apply mac_idx offset. 8852B: mac_idx=0 → +0, mac_idx=1 → +0x2000.
/// We always use mac_idx=0 for now.
```

## L151-154 · `fn wdrls_imr_enable(mmio: i32) {`

```
// ═══════════════════════════════════════════════════════════════════
//  DMAC side — 11 per-block IMR enables
//  Linux: enable_imr_ax(RTW89_MAC_0, RTW89_DMAC_SEL) (mac.c:3848)
// ═══════════════════════════════════════════════════════════════════
```

## L157 · `host::mmio_clr32(mmio, R_AX_WDRLS_ERR_IMR, WDRLS_IMR_EN_CLR);`

```
// Linux rtw89_wdrls_imr_enable (mac.c:3639).
```

## L163-164 · `host::mmio_set32(mmio, R_AX_SEC_DEBUG, SEC_DEBUG_IMR_ERROR);`

```
// Linux rtw89_wsec_imr_enable (mac.c:3647). 8852B imr_info:
//   wsec_imr_reg = R_AX_SEC_DEBUG, wsec_imr_set = B_AX_IMR_ERROR.
```

## L169-171 · `host::mmio_clr32(mmio, R_AX_MPDU_TX_ERR_IMR, MPDU_TX_CLR);`

```
// Linux rtw89_mpdu_trx_imr_enable (mac.c:3654). 8852B: SET = 0,
// so we only clear the "not interesting" mask. 8852C-specific
// ETH_TYPE / LLC / NW_TYPE / KSRCH clears are skipped.
```

## L177 · `host::mmio_clr32(mmio, R_AX_STA_SCHEDULER_ERR_IMR, STA_SCH_IMR_CLR);`

```
// Linux rtw89_sta_sch_imr_enable (mac.c:3682).
```

## L183-184 · `host::mmio_clr32(mmio, R_AX_TXPKTCTL_ERR_IMR_ISR,    TXPKTCTL_B0_CLR);`

```
// Linux rtw89_txpktctl_imr_enable (mac.c:3694). 8852B imr_info:
//   b0_reg = R_AX_TXPKTCTL_ERR_IMR_ISR, b1_reg = ..._B1.
```

## L192 · `host::mmio_clr32(mmio, R_AX_WDE_ERR_IMR, WDE_IMR_CLR);`

```
// Linux rtw89_wde_imr_enable (mac.c:3708).
```

## L198 · `host::mmio_clr32(mmio, R_AX_PLE_ERR_IMR, PLE_IMR_CLR);`

```
// Linux rtw89_ple_imr_enable (mac.c:3716).
```

## L204 · `host::mmio_set32(mmio, R_AX_PKTIN_ERR_IMR, PKTIN_IMR_SET);`

```
// Linux rtw89_pktin_imr_enable (mac.c:3724).
```

## L209-210 · `host::mmio_clr32(mmio, R_AX_HOST_DISPATCHER_ERR_IMR,  HOST_DISP_CLR);`

```
// Linux rtw89_dispatcher_imr_enable (mac.c:3730). 8852B imr_info:
//   other_disp_imr_set = 0 (only CLR).
```

## L219 · `host::mmio_clr32(mmio, R_AX_CPUIO_ERR_IMR, CPUIO_IMR_CLR);`

```
// Linux rtw89_cpuio_imr_enable (mac.c:3748).
```

## L225-226 · `host::mmio_set32(mmio, R_AX_BBRPT_COM_ERR_IMR_ISR,    BBRPT_COM_SET);`

```
// Linux rtw89_bbrpt_imr_enable (mac.c:3754). 8852B imr_info:
//   bbrpt_err_imr_set = 0 (only chinfo_clr is written).
```

## L229 · `host::mmio_set32(mmio, R_AX_BBRPT_DFS_ERR_IMR_ISR,    BBRPT_DFS_SET);`

```
// chinfo SET = 0, skip.
```

## L234 · `pub fn enable_dmac(mmio: i32) {`

```
/// 1:1 Linux enable_imr_ax(mac_idx=0, RTW89_DMAC_SEL) (mac.c:3848).
```

## L249-252 · `fn scheduler_imr_enable(mmio: i32, mac_idx: u8) {`

```
// ═══════════════════════════════════════════════════════════════════
//  CMAC side — 6 per-block IMR enables
//  Linux: enable_imr_ax(mac_idx, RTW89_CMAC_SEL) (mac.c:3860)
// ═══════════════════════════════════════════════════════════════════
```

## L255 · `let reg = idx(R_AX_SCHEDULE_ERR_IMR, mac_idx);`

```
// Linux rtw89_scheduler_imr_enable (mac.c:3769).
```

## L262 · `let reg = idx(R_AX_PTCL_IMR0, mac_idx);`

```
// Linux rtw89_ptcl_imr_enable (mac.c:3779).
```

## L269-270 · `let reg = idx(R_AX_DLE_CTRL, mac_idx);`

```
// Linux rtw89_cdma_imr_enable (mac.c:3789). 8852B imr_info:
//   cdma_imr_0_reg = R_AX_DLE_CTRL. cdma_imr_1 block is 8852C-only.
```

## L277-279 · `}`

```
// Linux rtw89_phy_intf_imr_enable (mac.c:3806). 8852B imr_info:
//   phy_intf_imr_clr = 0, phy_intf_imr_set = 0 → effective NO-OP
//   (both RMW would be no-op). Keep symmetric with Linux but skip.
```

## L283 · `let reg = idx(R_AX_RMAC_ERR_ISR, mac_idx);`

```
// Linux rtw89_rmac_imr_enable (mac.c:3816).
```

## L290 · `let reg = idx(R_AX_TMAC_ERR_IMR_ISR, mac_idx);`

```
// Linux rtw89_tmac_imr_enable (mac.c:3826).
```

## L296 · `pub fn enable_cmac(mmio: i32, mac_idx: u8) {`

```
/// 1:1 Linux enable_imr_ax(mac_idx, RTW89_CMAC_SEL) (mac.c:3860).
```

## L305-306 · `let _ = R_AX_PHYINFO_ERR_IMR;`

```
// Suppress unused-static warnings for the PHYINFO reg (kept as
// doc reference; the 8852B imr_info makes phy_intf_imr a no-op).
```

