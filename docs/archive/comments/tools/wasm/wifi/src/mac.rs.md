# `tools/wasm/wifi/src/mac.rs` @ 5e0102684

## L1-10 · `use crate::host;`

```
//! Post-FWDL MAC initialization + WiFi scan
//!
//! Sequence based on Linux rtw89 mac.c trx_init_ax:
//!   1. DLE re-init (SCC quotas)
//!   2. HFC init (all channels)
//!   3. DMAC sub-inits (sta_sch, mpdu_proc, sec_eng)
//!   4. CMAC init (12 sub-functions)
//!   5. PCIe post-init (LTR, enable DMA)
//!   6. RXQ setup for C2H
//!   7. H2C scan commands
```

## L17-20 · `const VERBOSE: bool = false;`

```
/// Global debug-verbosity flag. Flip to `true` for diagnostic runs —
/// every `dbg_checkpoint`, `[c2h LOG-FMT ...]`, scan rsn=1/2/4, and
/// post-scan listen dump will appear. `false` keeps the production log
/// focused on errors + progress milestones.
```

## L23 · `const R_AX_HCI_FC_CTRL: u32       = 0x8A00;`

```
// ── Register addresses (mac.c / reg.h) ─────────────────────────────
```

## L25 · `const R_AX_HCI_FC_CTRL: u32       = 0x8A00;`

```
// HFC
```

## L31 · `const R_AX_SS_CTRL: u32           = 0x9E10;`

```
// STA scheduler
```

## L34 · `const R_AX_ACTION_FWD0: u32       = 0x9C04;`

```
// MPDU proc
```

## L39 · `const R_AX_SEC_ENG_CTRL: u32      = 0x9D00;`

```
// Security engine
```

## L43 · `const R_AX_PREBKF_CFG_1: u32      = 0xC33C;`

```
// CMAC scheduler
```

## L49 · `const R_AX_ADDR_CAM_CTRL: u32     = 0xCE34;`

```
// Addr CAM
```

## L52 · `const R_AX_RX_FLTR_OPT: u32       = 0xCE20;`

```
// RX filter
```

## L59 · `const R_AX_CCA_CONTROL: u32       = 0xC390;`

```
// CCA
```

## L62 · `const R_AX_WMAC_NAV_CTL: u32      = 0xCC80;`

```
// NAV
```

## L65 · `const R_AX_RX_SR_CTRL: u32        = 0xCE4A;`

```
// Spatial reuse (byte offsets — handled via mmio_set8/clr8)
```

## L68 · `const R_AX_MAC_LOOPBACK: u32      = 0xCC20;`

```
// TMAC
```

## L73 · `const R_AX_TRXPTCL_RESP_0: u32    = 0xCC04;`

```
// TRXPTCL
```

## L77 · `const R_AX_RESPBA_CAM_CTRL: u32   = 0xCE3C;`

```
// RMAC
```

## L81 · `const R_AX_SIFS_SETTING: u32      = 0xC624;`

```
// PTCL
```

## L88 · `const R_AX_TX_SUB_CARRIER_VALUE: u32 = 0xC088;`

```
// CMAC com
```

## L92 · `const R_AX_LTR_CTRL_0: u32        = 0x8410;`

```
// LTR
```

## L98 · `use crate::regs::R_AX_RXQ_RXBD_IDX;`

```
// RXQ index register — now defined in regs.rs
```

## L101-103 · `pub fn init(mmio: i32) -> bool {`

```
// ═══════════════════════════════════════════════════════════════════
//  Main entry
// ═══════════════════════════════════════════════════════════════════
```

## L110-112 · `enable_bb_rf(mmio);`

```
// ── 0. Enable BB/RF — MUST come before MAC init! ──────────────
// Linux: rtw8852bx_mac_enable_bb_rf() — full 5-step sequence.
// Without this, the radio hardware is off and firmware can't scan.
```

## L117-124 · `sys_init_ax(mmio);`

```
// ── 0b. sys_init_ax — 1:1 Linux (mac.c:1696) re-assert after FWDL.
// Linux runs this in rtw89_mac_init AFTER partial_init (= FWDL).
// pwr_on_func already sets DMAC/CMAC func_en bits once, but FWDL
// can disturb them; sys_init_ax overwrites DMAC_FUNC_EN / CLK_EN
// with exact values (write32, not set32) and re-ORs CMAC. Without
// this the DMAC state after FWDL may contain extra bits our
// pwr_on OR'd in (DLE_WDE_EN, DLE_PLE_EN, BBRPT_EN, DMACREG_GCKEN)
// that Linux's canonical state does NOT set.
```

## L128 · `if !dle_init(mmio) { return false; }`

```
// ── 1. DLE re-init with SCC quotas ─────────────────────────────
```

## L131 · `hfc_init(mmio);`

```
// ── 2. HFC init (all channels) ─────────────────────────────────
```

## L134 · `sta_sch_init(mmio);`

```
// ── 3. DMAC sub-inits ──────────────────────────────────────────
```

## L140 · `cmac_init(mmio);`

```
// ── 4. CMAC init ───────────────────────────────────────────────
```

## L144-145 · `dbg_checkpoint(mmio, "after chip_func_en");`

```
// ── 4.5. chip_func_en_ax — moved into sys_init_ax (Phase 1.3).
// Kept the checkpoint comment for log continuity.
```

## L148-156 · `imr::enable_dmac(mmio);`

```
// ── 5. Enable IMRs — 1:1 Linux trx_init_ax (mac.c:3929):
//   a) 11 DMAC per-block IMR enables (imr::enable_dmac)
//   b) 6 CMAC per-block IMR enables (imr::enable_cmac)
//   c) err_imr_ctrl_ax(true) — master ERR_IMR unmask
// Previously we only wrote (c) with 0xFFFFFFFF (which happens to
// equal DMAC_ERR_IMR_EN / CMAC0_ERR_IMR_EN). The (a)+(b) block
// IMRs were missing entirely — strongest hypothesis for the H2C
// pipe wedge after VIF H2Cs. Linux considers (a)+(b)+(c) a single
// "enable interrupt sources" package and all three are required.
```

## L159 · `host::mmio_w32(mmio, 0x8520, 0xFFFFFFFF); // DMAC_ERR_IMR (EN = GENMASK(31,0))`

```
// DMAC_ERR_IMR (EN = GENMASK(31,0))
```

## L160 · `host::mmio_w32(mmio, 0xC160, 0xFFFFFFFF); // CMAC0_ERR_IMR (EN = GENMASK(31,0))`

```
// CMAC0_ERR_IMR (EN = GENMASK(31,0))
```

## L163-164 · `host::mmio_w32_mask(mmio, 0x9408, 0x3, 2);  // R_AX_WDRLS_CFG: MODE=POH`

```
// ── 6. Host report mode (set_host_rpr_ax) ─────────────────────
// Linux: mac.c set_host_rpr_ax — route TX release reports to RPQ.
```

## L165 · `host::mmio_w32_mask(mmio, 0x9408, 0x3, 2);  // R_AX_WDRLS_CFG: MODE=POH`

```
// R_AX_WDRLS_CFG: MODE=POH
```

## L166 · `host::mmio_set32(mmio, 0x9410, 0xFFFF_FFFF); // R_AX_RLSRPT0_CFG0: filter all`

```
// R_AX_RLSRPT0_CFG0: filter all
```

## L167 · `host::mmio_w32_mask(mmio, 0x9414, 0xFF, 30);       // AGGNUM=30`

```
// AGGNUM=30
```

## L168 · `host::mmio_w32_mask(mmio, 0x9414, 0xFF << 16, 255); // TO=255`

```
// TO=255
```

## L171-175 · `pcie_post_init(mmio);`

```
// ── 6.5. mac_post_init BEFORE phy::init — Linux order ─────────
// Linux runs mac_post_init_ax (LTR + enable all DMA + TX_ADDR_INFO +
// clear STOP_WPDMA|STOP_PCIEIO) at the END of mac_init, BEFORE
// core_start proceeds to reset_bb_rf + phy tables. We had this call
// AFTER phy::init, leaving PCIe in a stopped state during BB writes.
```

## L179-180 · `host::print("  PHY: reset_bb_rf (disable+enable)\n");`

```
// ── 6.6. reset_bb_rf (disable + enable) — MATCHES Linux core_start ───
// Linux calls rtw89_chip_reset_bb_rf between mac_init and phy_init_bb_reg.
```

## L185 · `crate::phy::init(mmio);`

```
// ── 7. PHY init — BB + RF + NCTL tables ───
```

## L189-199 · `let cr_base: u32 = 0x10000;`

```
// ── 7.2. phy_bb_reset + bb_reset_en(2G, true) — Linux phy.c:1313, rtw8852b.c:542/566
//
// After BB/RF/NCTL tables, Linux calls rtw89_phy_bb_reset which for
// 8852b toggles TXPW_RSTB MANON + TSSI_TRK and then runs bb_reset_all
// (toggle S0/S1_HW_SI_DIS + RSTB_ASYNC). Without this the BB DSP
// stays in an indeterminate post-table state.
//
// Even more critical: rtw8852b_bb_reset_en(2G, true) clears RXCCA_DIS
// and PD_HIT_DIS — enabling Packet Detection. Without PD the receiver
// literally does not see any frame arrive → our 0 beacons.
// All PHY space, so + PHY_CR_BASE (0x10000).
```

## L201 · `host::mmio_set32(mmio, cr_base + 0x58DC, 1 << 30);    // P0_TXPW_RSTB.MANON`

```
// rtw8852b_bb_reset — path 0 + path 1 TXPW manual on, TSSI trk en
```

## L202 · `host::mmio_set32(mmio, cr_base + 0x58DC, 1 << 30);    // P0_TXPW_RSTB.MANON`

```
// P0_TXPW_RSTB.MANON
```

## L203 · `host::mmio_set32(mmio, cr_base + 0x5818, 1 << 30);    // P0_TSSI_TRK.EN`

```
// P0_TSSI_TRK.EN
```

## L204 · `host::mmio_set32(mmio, cr_base + 0x78DC, 1 << 30);    // P1_TXPW_RSTB.MANON`

```
// P1_TXPW_RSTB.MANON
```

## L205 · `host::mmio_set32(mmio, cr_base + 0x7818, 1 << 30);    // P1_TSSI_TRK.EN`

```
// P1_TSSI_TRK.EN
```

## L206 · `host::mmio_w32_mask(mmio, cr_base + 0x1200, 0x7 << 28, 7);  // S0_HW_SI_DIS[30:28] = 7`

```
// bb_reset_all
```

## L207 · `host::mmio_w32_mask(mmio, cr_base + 0x1200, 0x7 << 28, 7);  // S0_HW_SI_DIS[30:28] = 7`

```
// S0_HW_SI_DIS[30:28] = 7
```

## L208 · `host::mmio_w32_mask(mmio, cr_base + 0x3200, 0x7 << 28, 7);  // S1_HW_SI_DIS[30:28] = 7`

```
// S1_HW_SI_DIS[30:28] = 7
```

## L210 · `host::mmio_set32(mmio, cr_base + 0x0704, 1 << 1);     // RSTB_ASYNC.ALL = 1`

```
// RSTB_ASYNC.ALL = 1
```

## L211 · `host::mmio_clr32(mmio, cr_base + 0x0704, 1 << 1);     // RSTB_ASYNC.ALL = 0`

```
// RSTB_ASYNC.ALL = 0
```

## L212 · `host::mmio_w32_mask(mmio, cr_base + 0x1200, 0x7 << 28, 0);  // S0_HW_SI_DIS[30:28] = 0`

```
// S0_HW_SI_DIS[30:28] = 0
```

## L213 · `host::mmio_w32_mask(mmio, cr_base + 0x3200, 0x7 << 28, 0);  // S1_HW_SI_DIS[30:28] = 0`

```
// S1_HW_SI_DIS[30:28] = 0
```

## L214 · `host::mmio_set32(mmio, cr_base + 0x0704, 1 << 1);     // RSTB_ASYNC.ALL = 1`

```
// RSTB_ASYNC.ALL = 1
```

## L215 · `host::mmio_clr32(mmio, cr_base + 0x58DC, 1 << 30);`

```
// Clear path 0/1 TXPW manual + TSSI trk
```

## L221 · `host::mmio_w32_mask(mmio, cr_base + 0x1200, 0x7 << 28, 0);  // S0_HW_SI_DIS clr`

```
// rtw8852b_bb_reset_en(RTW89_BAND_2G, phy_idx=0, en=true) — THIS enables PD/RXCCA
```

## L222 · `host::mmio_w32_mask(mmio, cr_base + 0x1200, 0x7 << 28, 0);  // S0_HW_SI_DIS clr`

```
// S0_HW_SI_DIS clr
```

## L223 · `host::mmio_w32_mask(mmio, cr_base + 0x3200, 0x7 << 28, 0);  // S1_HW_SI_DIS clr`

```
// S1_HW_SI_DIS clr
```

## L224 · `host::mmio_set32(mmio, cr_base + 0x0704, 1 << 1);           // RSTB_ASYNC.ALL = 1`

```
// RSTB_ASYNC.ALL = 1
```

## L225 · `host::mmio_clr32(mmio, cr_base + 0x2344, 1 << 31);          // RXCCA.DIS = 0 (2G: enable CCA)`

```
// RXCCA.DIS = 0 (2G: enable CCA)
```

## L226 · `host::mmio_clr32(mmio, cr_base + 0x0C3C, 1 << 9);           // PD_CTRL.PD_HIT_DIS = 0 (ENABLE packet detect)`

```
// PD_CTRL.PD_HIT_DIS = 0 (ENABLE packet detect)
```

## L229-238 · `host::mmio_clr32(mmio, cr_base + 0x58F0, 1 << 1);`

```
// ── 7.25. bb_sethw — Linux __rtw8852bx_bb_sethw (rtw8852b_common.c:1099)
//   Clear EN_SOUND_WO_NDP on both paths + zero MACID power limit table.
//   R_P0_EN_SOUND_WO_NDP + R_P1_EN_SOUND_WO_NDP cleared.
//   MACID pwr table @ R_AX_PWR_MACID_LMT_TABLE0..127 — 128 entries.
//   Addresses from reg.h grep:
//     R_P0_EN_SOUND_WO_NDP = 0x58F0 (B_P0_EN_SOUND_WO_NDP BIT(1))
//     R_P1_EN_SOUND_WO_NDP = 0x78F0
//     R_AX_PWR_MACID_LMT_TABLE0 = 0xD200, _127 = 0xD3FC (128×4 bytes)
//     (All PHY space for EN_SOUND; MACID table is MAC direct.)
//   Skip reads of RPL1 (gain calibration bases — used later for DIG).
```

## L242-244 · `host::mmio_w32(mmio, addr, 0);`

```
// Linux uses rtw89_mac_txpwr_write32(phy_idx=0, addr, 0) which goes
// through TXPWR indirect access. Direct MAC-space write should work
// at power-on since the table is in local MAC memory.
```

## L249-269 · `host::mmio_w32_mask(mmio, cr_base + 0x4860, 0x1F << 6, 0);   // PD_LOWER_BOUND=0`

```
// ── 7.26. phy_dig_init (8852b subset) — Linux phy.c:6838 __rtw89_phy_dig_init
//   For 8852B hal.support_igi=false (core.c:6294) so update_gain_para and
//   set_igi_cr are NO-OP. dig_para_reset + dig_update_para are software-
//   state only. The only MMIO bits are dig_dyn_pd_th(rssi=22, enable=false)
//   and sdagc_follow_pagc_config(false). These set PD thresholds to 0
//   (most sensitive) and disable pagcugc enables.
//
//   dig_regs for 8852b (rtw8852b.c:217):
//     seg0_pd_reg       = R_SEG0R_PD_V1         = 0x4860
//     pd_lower_bound    = [10:6]
//     pd_spatial_reuse  = bit 30
//     bmode_pd_reg      = R_BMODE_PDTH_EN_V1    = 0x4B74
//     bmode_cca_lim_en  = bit 30
//     bmode_lower_reg   = R_BMODE_PDTH_V1       = 0x4B64
//     bmode_lower_mask  = [31:24]
//     p0_p20_pagcugc_en = R_PATH0_P20_FOLLOW_BY_PAGCUGC_V2 = 0x46E8, bit 5
//     p0_s20_pagcugc_en = 0x46EC, bit 5
//     p1_p20_pagcugc_en = 0x47A8, bit 5
//     p1_s20_pagcugc_en = 0x47AC, bit 5
//   support_cckpd=true for 8852b (cv>CAV) → CCK PD writes also run.
//   With enable=false: everything goes to 0 (max sensitivity for scan).
```

## L270 · `host::mmio_w32_mask(mmio, cr_base + 0x4860, 0x1F << 6, 0);   // PD_LOWER_BOUND=0`

```
// PD_LOWER_BOUND=0
```

## L271 · `host::mmio_w32_mask(mmio, cr_base + 0x4860, 1 << 30, 0);     // spatial_reuse_en=0`

```
// spatial_reuse_en=0
```

## L272 · `host::mmio_w32_mask(mmio, cr_base + 0x4B74, 1 << 30, 0);     // bmode CCA limit en=0`

```
// bmode CCA limit en=0
```

## L273 · `host::mmio_w32_mask(mmio, cr_base + 0x4B64, 0xFFu32 << 24, 0); // bmode PD lower=0`

```
// bmode PD lower=0
```

## L274 · `host::mmio_w32_mask(mmio, cr_base + 0x46E8, 1 << 5, 0);      // p0 p20 pagcugc=0`

```
// p0 p20 pagcugc=0
```

## L275 · `host::mmio_w32_mask(mmio, cr_base + 0x46EC, 1 << 5, 0);      // p0 s20 pagcugc=0`

```
// p0 s20 pagcugc=0
```

## L276 · `host::mmio_w32_mask(mmio, cr_base + 0x47A8, 1 << 5, 0);      // p1 p20 pagcugc=0`

```
// p1 p20 pagcugc=0
```

## L277 · `host::mmio_w32_mask(mmio, cr_base + 0x47AC, 1 << 5, 0);      // p1 s20 pagcugc=0`

```
// p1 s20 pagcugc=0
```

## L280-287 · `host::mmio_w32_mask(mmio, cr_base + 0x0C00, 1 << 0, 1);`

```
// ── 7.28. env_monitor_init → ccx_top_setting_init — Linux phy.c:5799
//   Enables CCX measurement engine for channel quality tracking.
//   ccx_regs_ax (phy.c:8318 area): setting_addr = R_CCX = 0x0C00
//     en_mask             = B_CCX_EN_MSK             = BIT(0)
//     trig_opt_mask       = B_CCX_TRIG_OPT_MSK       = BIT(1)
//     measurement_trig    = B_MEASUREMENT_TRIG_MSK   = BIT(2)
//     edcca_opt_mask      = B_CCX_EDCCA_OPT_MSK      = GENMASK(6, 4)
//   RTW89_CCX_EDCCA_BW20_0 = 0
```

## L294-299 · `host::mmio_w32_mask(mmio, cr_base + 0x4494, 1 << 29, 1);`

```
// ── 7.29. cfo_init (subset) — Linux phy.c:4957
//   Runs dcfo_comp_init. 8852b has cfo_hw_comp=true (rtw8852b.c:1032) so:
//     PHY R_DCFO_OPT (0x4494), B_DCFO_OPT_EN (BIT(29)) = 1
//     PHY R_DCFO_WEIGHT (0x4490), B_DCFO_WEIGHT_MSK (GENMASK(27,24)) = 8
//     MAC R_AX_PWR_UL_CTRL2 (0xD248), B_AX_PWR_UL_CFO_MASK ([2:0]) = 6
//   Skipping crystal_cap setting (needs efuse xtal_cap we don't parse).
```

## L305-315 · `host::mmio_set32(mmio, cr_base + 0x0738, (1 << 3) | (1 << 2));`

```
// ── 7.27. physts_parsing_init — Linux phy.c:6683 for PHY_0
//   Configures how FW extracts PHY status info from received PPDUs.
//   Without this, RX frames reach the MAC but have invalid/missing
//   PHY status, and the scan-RX path drops them silently.
//
//   All writes PHY space (+CR_BASE).
//   setting_addr = R_PLCP_HISTOGRAM (0x0738)
//   dis_trigger_fail_mask = BIT(3), dis_trigger_brk_mask = BIT(2)
//   physt_bmp_start = 0x073C (page 0 base)
//
//   Step 1 — enable_fail_report(false) → SET both DIS bits
```

## L317-329 · `for i in 0u32..=15 {`

```
//   Step 2 — enable_hdr_2: skipped for RTW89_CHIP_AX.
//   Step 3 — loop bitmap pages 0..16 (skip RSVD_9 and EHT for AX).
//     i        ie_page (→ addr offset)   modify
//     0..5,8   page = i                   no change
//     6 (HE_MU), 7 (VHT_MU): val |= BIT(13)
//     9 (RSVD_9)                          SKIP
//     10 (TRIG_BASE_PPDU): page=9 → 0x0760, val |= BIT(13)|BIT(1)
//     11 (CCK_PKT):       page=10 → 0x0764, val &= ~(GENMASK(7,4)); val |= BIT(1)
//     12 (LEGACY_OFDM):   page=11 → 0x0768, val &= ~(GENMASK(7,4))
//     13 (HT_PKT):        page=12 → 0x076C, val &= ~(GENMASK(7,4)); val |= BIT(20)
//     14 (VHT_PKT):       page=13 → 0x0770, same as HT_PKT
//     15 (HE_PKT):        page=14 → 0x0774, same as HT_PKT
//     16 (EHT_PKT)                        SKIP for AX
```

## L340 · `val &= !(0xFu32 << 4);  // clear GENMASK(7,4)`

```
// clear GENMASK(7,4)
```

## L351-354 · `const CR: u32 = 0x10000;`

```
// ── 7.3. cfg_txrx_path(RF_AB, 2G) — Linux rtw8852bx_bb_cfg_txrx_path
//   (rtw8852b_common.c:1743) — enables RX antenna path. All writes are
//   PHY-space, so each address gets + PHY_CR_BASE (0x10000).
// Matches RF_AB (dual-path) + rx_nss=2 branch.
```

## L356 · `host::mmio_w32_mask(mmio, CR + 0x49C4, 0xF, 3);`

```
//   R_CHBW_MOD_V1=0x49C4, B_ANT_RX_SEG0=GENMASK(3,0) → 3 (RF_AB)
```

## L358 · `host::mmio_w32_mask(mmio, CR + 0x49C0, 0xF << 14, 3);`

```
//   R_FC0_BW_V1=0x49C0, B_ANT_RX_1RCCA_SEG0=GENMASK(17,14) → 3
```

## L360 · `host::mmio_w32_mask(mmio, CR + 0x49C0, 0xF << 18, 3);`

```
//   R_FC0_BW_V1=0x49C0, B_ANT_RX_1RCCA_SEG1=GENMASK(21,18) → 3
```

## L362 · `host::mmio_w32_mask(mmio, CR + 0x0D18, 0x3 << 8, 1);`

```
//   R_RXHT_MCS_LIMIT=0x0D18, B_RXHT_MCS_LIMIT=GENMASK(9,8) → 1 (2-stream)
```

## L364 · `host::mmio_w32_mask(mmio, CR + 0x0D18, 0x3 << 21, 1);`

```
//   R_RXVHT_MCS_LIMIT=0x0D18, B_RXVHT_MCS_LIMIT=GENMASK(22,21) → 1
```

## L366 · `host::mmio_w32_mask(mmio, CR + 0x0D80, 0xFF << 6, 4);`

```
//   R_RXHE=0x0D80, B_RXHE_USER_MAX=GENMASK(13,6) → 4
```

## L368 · `host::mmio_w32_mask(mmio, CR + 0x0D80, 0x7 << 14, 1);`

```
//   R_RXHE, B_RXHE_MAX_NSS=GENMASK(16,14) → 1
```

## L370 · `host::mmio_w32_mask(mmio, CR + 0x0D80, 0x7 << 23, 1);`

```
//   R_RXHE, B_RXHETB_MAX_NSS=GENMASK(25,23) → 1
```

## L372-373 · `host::mmio_w32_mask(mmio, CR + 0x12AC, 0xFFFFFFF0, 0x1233312);`

```
//   RFMODE — both P0 and P1 set same for RF_AB:
//   R_P0_RFMODE=0x12AC, B_P0_RFMODE_ORI_TXRX_FTM_TX=GENMASK(31,4) → 0x1233312
```

## L375 · `host::mmio_w32_mask(mmio, CR + 0x12B0, 0xFFF, 0x333);`

```
//   R_P0_RFMODE_FTM_RX=0x12B0, B_P0_RFMODE_FTM_RX=GENMASK(11,0) → 0x333
```

## L377 · `host::mmio_w32_mask(mmio, CR + 0x32AC, 0xFFFFFFF0, 0x1233312);`

```
//   R_P1_RFMODE=0x32AC → 0x1233312
```

## L379 · `host::mmio_w32_mask(mmio, CR + 0x32B0, 0xFFF, 0x333);`

```
//   R_P1_RFMODE_FTM_RX=0x32B0 → 0x333
```

## L381-382 · `host::mmio_w32_mask(mmio, CR + 0x78DC, (1<<30) | (1<<31), 1);`

```
//   TXPW reset toggle (P1 for rx_path != RF_A)
//   R_P1_TXPW_RSTB=0x78DC, bit 30=MANON, bit 31=TSSI → 1 then 3
```

## L385 · `host::mmio_w32_mask(mmio, CR + 0x09A4, 0x7 << 2, 0);`

```
//   R_MAC_SEL=0x09A4, B_MAC_SEL_MOD=GENMASK(4,2) → 0
```

## L389-397 · `host::mmio_w32(mmio, 0xCE40, (1 << 0) | (1 << 1) | (1 << 3) | (1 << 5));`

```
// ── 7.5. cfg_ppdu_status(HOST) — Linux mac.c:6155 rtw89_mac_cfg_ppdu_status_ax
// Enables PPDU status reports + routes them to HOST. Without this, RX
// frames stay in FW-internal space and never reach the RXQ DMA ring.
//   R_AX_PPDU_STAT = 0xCE40:
//     bit 0 = B_AX_PPDU_STAT_RPT_EN
//     bit 1 = B_AX_APP_MAC_INFO_RPT
//     bit 3 = B_AX_APP_PLCP_HDR_RPT
//     bit 5 = B_AX_PPDU_STAT_RPT_CRC32
//   R_AX_HW_RPT_FWD = 0x9C18, mask [1:0] = RTW89_PRPT_DEST_HOST(1)
```

## L402-405 · `let ofld_cfg: [u8; 8] = [0x09, 0x00, 0x00, 0x00, 0x5E, 0x00, 0x00, 0x00];`

```
// ── 8. H2C set_ofld_cfg — Linux rtw89_fw_h2c_set_ofld_cfg (fw.c:5228)
// Sent after mac_init + phy tables, tells FW the offload config.
// Linux: rack=0, dack=1 (fw.c:5243) → FW MUST send DONE_ACK back.
//   CAT=1 (MAC), CLASS=9 (MAC_FW_OFLD), FUNC=0x14 (OFLD_CFG)
```

## L411-416 · `host::print("  H2C: fw_log_cfg (LEVEL=LOUD, PATH=C2H)...\n");`

```
// ── 9. H2C fw_log_cfg — Linux rtw89_fw_h2c_fw_log (fw.c:2787).
// Activates FW trace log routed over C2H with LEVEL=LOUD on components
// INIT/TASK/PS/ERROR/MLO/SCAN. Without this the FW is silent and our
// C2H_LOG decoder (handle_c2h cls=0 fn=2) sees nothing — meaning any
// init/scan/error condition on the FW side is invisible to us.
// Linux calls this at the end of rtw89_core_start (core.c:5985).
```

## L425-439 · `pub fn hci_start(mmio: i32) {`

```
// ═══════════════════════════════════════════════════════════════════
//  hci_start — 1:1 Linux rtw89_hci_start → rtw89_pci_ops_start
//  (pci.c:1922). Called at the very end of rtw89_core_start
//  (core.c:5970). Arms the chip's IRQ mask registers so DMA-complete,
//  RX-descriptor-unavailable and HALT-C2H events can be delivered to
//  the host. Even though we poll instead of using IRQs, the unmask is
//  load-bearing: on some AX chips the RX DMA is gated on the IMR
//  being set, and on all of them the status bits only latch after
//  unmask — so without this the HW can appear "stuck" even though
//  the MAC is alive.
//
//  Linux build-time split: rtw89_pci_config_intr_mask picks the
//  non-recovery non-low-power path for 8852BE. We inline those values
//  directly since we never go into recovery/LPS.
// ═══════════════════════════════════════════════════════════════════
```

## L442 · `let halt_c2h_intrs: u32 = regs::B_AX_HALT_C2H_INT_EN;`

```
// halt_c2h_intrs (goes to R_AX_HIMR0 at 0x01A0)
```

## L445-446 · `let intrs0: u32 = regs::B_AX_TXDMA_STUCK_INT_EN`

```
// intrs[0] → R_AX_PCIE_HIMR00 (0x10B0): the full default mask
//   from rtw89_pci_config_intr_mask (non-recovery branch).
```

## L456 · `let intrs1: u32 = regs::B_AX_HC10ISR_IND_INT_EN;`

```
// intrs[1] → R_AX_PCIE_HIMR10 (0x13B0)
```

## L459-461 · `let _ = host::mmio_r32(mmio, regs::R_AX_HISR0);`

```
// Clear any pending ISR bits before unmasking (belt-and-braces —
// Linux doesn't do this explicitly in ops_start, but IRQs that
// latched before our IMR was known-good can confuse polling).
```

## L466 · `host::mmio_w32(mmio, regs::R_AX_HIMR0,       halt_c2h_intrs);`

```
// Unmask — 1:1 Linux rtw89_pci_enable_intr (pci.c:853):
```

## L481-487 · `fn diag_wait_c2h(mmio: i32, max_ms: u32, tag: &str) {`

```
/// Diagnose: poll RXQ IDX for up to `max_ms` ms, report any HW_IDX advance.
/// Used to verify H2C → C2H pipe bidirectionality. The "NO C2H" branch
/// fires for fire-and-forget H2Cs (macid_pause, fw_log_cfg, some VIF
/// helpers whose DONE_ACK gets batched behind a later dack=1 H2C) — we
/// keep it silent in non-verbose builds to avoid misleading "H2C pipe
/// 1-way" spam that made earlier logs look like problems when the H2Cs
/// were in fact accepted fine.
```

## L503 · `rxq_poll(mmio);`

```
// Drain what we got
```

## L518-522 · `fn dbg_checkpoint(mmio: i32, tag: &str) {`

```
/// Debug helper: dump a few registers to find where the 0x1000 range dies.
/// CFG1 (0x1000) vs HCI_OPT_CTRL (0x0074) vs SYS_CFG1 (0x00F0):
/// if CFG1=0xFFFFFFFF but the others are sane, only PCIe DMA block is gated.
/// Gated by `VERBOSE` — we leave it silent in production because once init
/// passes cleanly this dump is pure noise; keep the code for re-enabling.
```

## L535-540 · `fn sys_init_ax(mmio: i32) {`

```
// ═══════════════════════════════════════════════════════════════════
//  sys_init_ax — 1:1 Linux mac.c:1696. Runs AFTER FWDL inside
//  rtw89_mac_init. Re-asserts DMAC/CMAC func_en + clk_en + chip
//  OCP_L1. pwr_on_func did this before FWDL, but FWDL can disturb
//  bits and Linux re-canonicalises via write32 (DMAC) + set32 (CMAC).
// ═══════════════════════════════════════════════════════════════════
```

## L543-549 · `let dmac_func_en: u32 =`

```
// ── dmac_func_en_ax (mac.c:1651) — DIRECT write32, overwrites any
// extra bits from pwr_on. For non-8852C (= 8852B):
//   MAC_FUNC_EN | DMAC_FUNC_EN | MAC_SEC_EN | DISPATCHER_EN
// | DLE_CPUIO_EN | PKT_IN_EN | DMAC_TBL_EN | PKT_BUF_EN
// | STA_SCH_EN | TXPKT_CTRL_EN | WD_RLS_EN | MPDU_PROC_EN
// | DMAC_CRPRT
//   = 0xFB7D0000 (bits 30,29,28,27,25,24,22,21,20,19,18,16,31)
```

## L566-567 · `let dmac_clk_en: u32 =`

```
// dmac_clk_en: MAC_SEC | DISPATCHER | DLE_CPUIO | PKT_IN
//            | STA_SCH | TXPKT_CTRL | WD_RLS | BBRPT CLK
```

## L579-582 · `let cmac_ck_en: u32 =`

```
// ── cmac_func_en_ax(mac_idx=0, en=true) (mac.c:1605) — SET (OR)
//   ck_en:   CMAC | PHYINTF | CMAC_DMA | PTCLTOP | SCHEDULER | TMAC | RMAC
//   func_en: CMAC_EN | CMAC_TXEN | CMAC_RXEN | PHYINTF_EN | CMAC_DMA_EN
//          | PTCLTOP_EN | SCHEDULER_EN | TMAC_EN | RMAC_EN | CMAC_CRPRT
```

## L606-607 · `host::mmio_set32(mmio, regs::R_AX_SPS_DIG_ON_CTRL0, 0x7 << 13);`

```
// ── chip_func_en_ax (mac.c:1685) — 8852B: set OCP_L1_MASK.
// B_AX_OCP_L1_MASK = GENMASK(15,13) = 0xE000
```

## L613-615 · `fn enable_bb_rf(mmio: i32) {`

```
// ═══════════════════════════════════════════════════════════════════
//  BB/RF Enable — rtw8852bx_mac_enable_bb_rf()
// ═══════════════════════════════════════════════════════════════════
```

## L618-619 · `host::mmio_set8(mmio, regs::R_AX_SYS_FUNC_EN,`

```
// Linux __rtw8852bx_mac_enable_bb_rf, order matters.
// Step 1: Enable BB reset + global reset
```

## L623 · `host::mmio_w32_mask(mmio, regs::R_AX_SPS_DIG_ON_CTRL0,`

```
// Step 2: SPS digital supply voltage
```

## L627-629 · `host::mmio_clr32(mmio, regs::R_AX_WLRF_CTRL, regs::B_AX_AFC_AFEDIG);`

```
// Step 3: AFE toggle — Linux does SET-CLR-SET. We keep CLR-CLR-SET for now
// because empirically SET-CLR-SET kills BB writes. Re-evaluate once
// earlier init steps (disable_bb_rf, reset_bb_rf) are correct.
```

## L634 · `fw::write_xtal_si(mmio, regs::XTAL_SI_WL_RFC_S0, 0xC7, 0xFF);`

```
// Step 4: XTAL SI — enable RF switches S0 + S1 (write 0xC7 full mask)
```

## L638 · `host::mmio_set8(mmio, 0x8040, 0x01); // R_AX_PHYREG_SET = XYN_CYCLE`

```
// Step 5: PHY register access cycle time
```

## L639 · `host::mmio_set8(mmio, 0x8040, 0x01); // R_AX_PHYREG_SET = XYN_CYCLE`

```
// R_AX_PHYREG_SET = XYN_CYCLE
```

## L642-648 · `pub fn disable_bb_rf(mmio: i32) {`

```
/// 1:1 Linux __rtw8852bx_mac_disable_bb_rf (rtw8852b_common.c:2036).
/// Clears WLRF_CTRL.AFEDIG, clears BBRSTB+BB_GLB_RSTN, clears RFC S0/S1
/// enable bits via XTAL SI. Read-modify-write on XTAL SI needs a read,
/// which we don't have; Linux reads the current value and clears one bit.
/// We approximate by writing 0x00 with mask 0x01 (XTAL_SI_RF00S_EN/S1_EN
/// lives at bit 0) — only clears the target bit, other bits stay 0 after
/// pwr_on which is what Linux achieves via RMW in the common case.
```

## L650 · `host::mmio_clr32(mmio, regs::R_AX_WLRF_CTRL, regs::B_AX_AFC_AFEDIG);`

```
// Step 1: Clear AFE digital
```

## L653 · `host::mmio_clr8(mmio, regs::R_AX_SYS_FUNC_EN,`

```
// Step 2: Clear BB reset + global reset
```

## L657-660 · `fw::write_xtal_si(mmio, regs::XTAL_SI_WL_RFC_S0, 0x00, 0x01);`

```
// Step 3: Clear XTAL_SI_RF00S_EN on S0 (bit 0)
//         Clear XTAL_SI_RF10S_EN on S1 (bit 0)
// Linux does read-modify-write; we write value=0 with mask=0x01 so only
// bit 0 is affected.
```

## L665-667 · `pub fn reset_bb_rf(mmio: i32) {`

```
/// 1:1 Linux rtw89_chip_reset_bb_rf (mac.h:1321): disable then enable.
/// Linux calls this in core_start BETWEEN mac_init and phy_init_bb_reg,
/// so BB/RF is brought into a clean state before PHY tables are loaded.
```

## L673-675 · `fn dle_init(mmio: i32) -> bool {`

```
// ═══════════════════════════════════════════════════════════════════
//  DLE Init (SCC quotas — normal mode, not DLFW)
// ═══════════════════════════════════════════════════════════════════
```

## L678 · `host::mmio_clr32(mmio, regs::R_AX_DMAC_FUNC_EN,`

```
// Disable DLE
```

## L682 · `host::mmio_set32(mmio, regs::R_AX_DMAC_CLK_EN, (1 << 26) | (1 << 23));`

```
// Enable DLE clocks
```

## L685 · `let mut wde = host::mmio_r32(mmio, regs::R_AX_WDE_PKTBUF_CFG);`

```
// WDE: page_sel=0(64B), bound=0, free_pages=510
```

## L691 · `let mut ple = host::mmio_r32(mmio, regs::R_AX_PLE_PKTBUF_CFG);`

```
// PLE: page_sel=1(128B), bound=4, free_pages=496
```

## L697 · `host::mmio_w32(mmio, 0x8C40, (446 << 16) | 446); // HIF`

```
// WDE quotas (SCC): register format = min[11:0] | max[27:16]
```

## L698 · `host::mmio_w32(mmio, 0x8C40, (446 << 16) | 446); // HIF`

```
// HIF
```

## L699 · `host::mmio_w32(mmio, 0x8C44, (48 << 16) | 48);   // WCPU`

```
// WCPU
```

## L700 · `host::mmio_w32(mmio, 0x8C4C, 0);                   // PKT_IN`

```
// PKT_IN
```

## L701 · `host::mmio_w32(mmio, 0x8C50, (16 << 16) | 16);    // CPU_IO`

```
// CPU_IO
```

## L703 · `let ple_qt: [u32; 11] = [`

```
// PLE quotas (SCC)
```

## L714 · `host::mmio_set32(mmio, regs::R_AX_DMAC_FUNC_EN,`

```
// Enable DLE
```

## L718 · `for _ in 0..200 {`

```
// Poll WDE + PLE ready
```

## L731-733 · `fn hfc_init(mmio: i32) {`

```
// ═══════════════════════════════════════════════════════════════════
//  HFC Init (all channels)
// ═══════════════════════════════════════════════════════════════════
```

## L736 · `let mut fc = host::mmio_r32(mmio, R_AX_HCI_FC_CTRL);`

```
// Disable HFC before config
```

## L738 · `fc &= !((1 << 0) | (1 << 3)); // clear FC_EN + CH12_EN`

```
// clear FC_EN + CH12_EN
```

## L741-742 · `let ch_cfg: [(u16, u16); 13] = [`

```
// Per-channel config: R_AX_ACH0_PAGE_CTRL + ch*4
// Format: min[15:0] | max[31:16]  (grp bit at [30] but all grp_0)
```

## L744 · `(5, 341),   // ACH0`

```
// ACH0
```

## L745 · `(5, 341),   // ACH1`

```
// ACH1
```

## L746 · `(4, 342),   // ACH2`

```
// ACH2
```

## L747 · `(4, 342),   // ACH3`

```
// ACH3
```

## L748 · `(0, 0),     // ACH4`

```
// ACH4
```

## L749 · `(0, 0),     // ACH5`

```
// ACH5
```

## L750 · `(0, 0),     // ACH6`

```
// ACH6
```

## L751 · `(0, 0),     // ACH7`

```
// ACH7
```

## L752 · `(4, 342),   // B0MGQ (ch8)`

```
// B0MGQ (ch8)
```

## L753 · `(4, 342),   // B0HIQ (ch9)`

```
// B0HIQ (ch9)
```

## L754 · `(0, 0),     // B1MGQ (ch10)`

```
// B1MGQ (ch10)
```

## L755 · `(0, 0),     // B1HIQ (ch11)`

```
// B1HIQ (ch11)
```

## L756 · `(40, 0),    // FWCMDQ (ch12)`

```
// FWCMDQ (ch12)
```

## L763 · `host::mmio_w32(mmio, R_AX_PUB_PAGE_CTRL1, 446); // grp0[10:0]=446, grp1=0`

```
// Public buffer: grp0=446, grp1=0
```

## L764 · `host::mmio_w32(mmio, R_AX_PUB_PAGE_CTRL1, 446); // grp0[10:0]=446, grp1=0`

```
// grp0[10:0]=446, grp1=0
```

## L765 · `host::mmio_w32(mmio, R_AX_WP_PAGE_CTRL2, 0);    // wp_thrd=0`

```
// wp_thrd=0
```

## L767 · `fc = host::mmio_r32(mmio, R_AX_HCI_FC_CTRL);`

```
// Enable HFC + H2C
```

## L769 · `fc |= (1 << 0) | (1 << 3); // FC_EN (bit 0) + CH12_EN (bit 3)`

```
// FC_EN (bit 0) + CH12_EN (bit 3)
```

## L775-777 · `fn sta_sch_init(mmio: i32) {`

```
// ═══════════════════════════════════════════════════════════════════
//  DMAC sub-inits
// ═══════════════════════════════════════════════════════════════════
```

## L780 · `host::mmio_set32(mmio, R_AX_SS_CTRL, 1); // SS_EN`

```
// SS_EN
```

## L782 · `if host::mmio_r32(mmio, R_AX_SS_CTRL) & (1 << 31) != 0 { break; } // INIT_DONE`

```
// INIT_DONE
```

## L785 · `host::mmio_set32(mmio, R_AX_SS_CTRL, 1 << 29);  // WARM_INIT`

```
// WARM_INIT
```

## L786 · `host::mmio_clr32(mmio, R_AX_SS_CTRL, 1 << 28);  // clr NONEMPTY`

```
// clr NONEMPTY
```

## L797 · `val |= (1 << 0) | (1 << 1) | (1 << 2) | (1 << 4) | (1 << 5) | (1 << 6) | (1 << 7);`

```
// Set: CLK_EN_CGCMP, CLK_EN_WAPI, CLK_EN_WEP_TKIP, TX_ENC, RX_DEC, MC_DEC, BC_DEC
```

## L799 · `val &= !(1 << 8); // clear TX_PARTIAL_MODE (8852B)`

```
// clear TX_PARTIAL_MODE (8852B)
```

## L801 · `host::mmio_set32(mmio, R_AX_SEC_MPDU_PROC, 0x3); // APPEND_ICV | APPEND_MIC`

```
// APPEND_ICV | APPEND_MIC
```

## L804-806 · `fn cmac_init(mmio: i32) {`

```
// ═══════════════════════════════════════════════════════════════════
//  CMAC Init (12 sub-functions from cmac_init_ax)
// ═══════════════════════════════════════════════════════════════════
```

## L809 · `host::mmio_w32_mask(mmio, R_AX_PREBKF_CFG_1, 0x7F, 0x47);  // SIFS_MACTXEN`

```
// 1. Scheduler
```

## L810 · `host::mmio_w32_mask(mmio, R_AX_PREBKF_CFG_1, 0x7F, 0x47);  // SIFS_MACTXEN`

```
// SIFS_MACTXEN
```

## L811 · `host::mmio_set32(mmio, R_AX_SCH_EXT_CTRL, 1 << 1);          // RST_TSF_ADV (8852B)`

```
// RST_TSF_ADV (8852B)
```

## L812 · `host::mmio_clr32(mmio, R_AX_CCA_CFG_0, 1 << 5);             // clr BTCCA_EN`

```
// clr BTCCA_EN
```

## L813 · `host::mmio_w32_mask(mmio, R_AX_PREBKF_CFG_0, 0x1F, 0x18);   // PREBKF=24us`

```
// PREBKF=24us
```

## L815 · `let mut cam = host::mmio_r32(mmio, R_AX_ADDR_CAM_CTRL);`

```
// 2. Addr CAM
```

## L817 · `cam |= (1 << 0) | (1 << 1) | 0x7F; // EN + CLR + RANGE`

```
// EN + CLR + RANGE
```

## L824 · `host::mmio_w32(mmio, R_AX_MGNT_FLTR, 0x5555_5555);`

```
// 3. RX filter — accept all to host
```

## L828 · `host::mmio_w32(mmio, R_AX_PLCP_HDR_FLTR, 0x75); // CRC/SIG checks`

```
// CRC/SIG checks
```

## L830 · `let mut cca = host::mmio_r32(mmio, R_AX_CCA_CONTROL);`

```
// 4. CCA control
```

## L832 · `cca |= (1 << 0) | (1 << 1) | (1 << 2) | (1 << 3)    // TB checks`

```
// TB checks
```

## L833 · `| (1 << 8) | (1 << 9)                             // SIFS checks`

```
// SIFS checks
```

## L834 · `| (1 << 16) | (1 << 17) | (1 << 18) | (1 << 19)  // CTN checks`

```
// CTN checks
```

## L835 · `| (1 << 20) | (1 << 21) | (1 << 22) | (1 << 23); // CTN CCA`

```
// CTN CCA
```

## L838 · `let mut nav = host::mmio_r32(mmio, R_AX_WMAC_NAV_CTL);`

```
// 5. NAV
```

## L840 · `nav |= (1 << 16) | (1 << 17) | (1 << 26); // TF_UP_NAV + PLCP_UP_NAV + NAV_UPPER`

```
// TF_UP_NAV + PLCP_UP_NAV + NAV_UPPER
```

## L842 · `nav |= 0xC4 << 8; // NAV_UPPER = 25ms`

```
// NAV_UPPER = 25ms
```

## L845 · `host::mmio_clr8(mmio, R_AX_RX_SR_CTRL, 1);`

```
// 6. Spatial reuse — disable SR
```

## L848 · `host::mmio_clr32(mmio, R_AX_MAC_LOOPBACK, 1);           // disable loopback`

```
// 7. TMAC
```

## L849 · `host::mmio_clr32(mmio, R_AX_MAC_LOOPBACK, 1);           // disable loopback`

```
// disable loopback
```

## L850 · `host::mmio_w32_mask(mmio, R_AX_TCR0, 0x7F << 16, 6);    // UDF threshold`

```
// UDF threshold
```

## L851 · `host::mmio_w32_mask(mmio, R_AX_TXD_FIFO_CTRL, 0xF << 12, 7); // HIGH_MCS`

```
// HIGH_MCS
```

## L852 · `host::mmio_w32_mask(mmio, R_AX_TXD_FIFO_CTRL, 0xF << 8, 7);  // LOW_MCS`

```
// LOW_MCS
```

## L854 · `let mut resp = host::mmio_r32(mmio, R_AX_TRXPTCL_RESP_0);`

```
// 8. TRXPTCL
```

## L856 · `resp &= !0xFF; resp |= 0x0A;     // SIFS_CCK = 10`

```
// SIFS_CCK = 10
```

## L857 · `resp &= !(0xFF << 8); resp |= 0x11 << 8;  // SIFS_OFDM = 17 (8852B)`

```
// SIFS_OFDM = 17 (8852B)
```

## L859 · `host::mmio_set32(mmio, R_AX_RXTRIG_TEST_USER_2, 1 << 20); // FCSCHK_EN`

```
// FCSCHK_EN
```

## L861 · `host::mmio_set32(mmio, R_AX_RESPBA_CAM_CTRL, 1 << 2);   // SSN_SEL`

```
// 9. RMAC
```

## L862 · `host::mmio_set32(mmio, R_AX_RESPBA_CAM_CTRL, 1 << 2);   // SSN_SEL`

```
// SSN_SEL
```

## L863 · `host::mmio_w32_mask(mmio, R_AX_RCR, 0xF, 1);             // CH_EN = 1`

```
// CH_EN = 1
```

## L865-869 · `host::mmio_w32_mask(mmio, R_AX_RX_FLTR_OPT, 0x3F << 16, 0x3F);`

```
//   9b. B_AX_RX_MPDU_MAX_LEN_MASK — bits [21:16] of R_AX_RX_FLTR_OPT.
//   Linux rmac_init_ax:2862 computes this from c0_rx_qta * ple_pg_size
//   / 512. A zero value makes the RMAC reject every incoming WiFi frame
//   as "too long" — this is why our scan saw only C2H messages (type 10)
//   and zero WiFi frames (type 0). Safe upper bound: 0x3F = 63 → 32 KB.
```

## L872 · `host::mmio_w32_mask(mmio, R_AX_PTCL_RRSR1, 0xF << 8, 3); // OFDM+CCK`

```
// 10. CMAC com
```

## L873 · `host::mmio_w32_mask(mmio, R_AX_PTCL_RRSR1, 0xF << 8, 3); // OFDM+CCK`

```
// OFDM+CCK
```

## L875-887 · `{`

```
// 11. PTCL — 1:1 Linux ptcl_init_ax (mac.c:2925), PCIe block + MAC_0.
//
// PCIe block (mac.c:2936-2949): the SIFS + FSM timeout writes are the
// load-bearing piece. Without R_AX_PTCL_FSM_MON.TX_ARB_TO_THR=0x3F
// (~2 ms), the arbiter default is 0 → PTCL aborts every TX attempt the
// instant it enters arbitration and the frame is silently dropped
// between DMA and PHY (CH8_BUSY toggles, TXBD_IDX advances, but
// TX_COUNTER stays at 0). This was the v1.30 diagnostic finding.
//
// R_AX_SIFS_SETTING (0xC624):
//   [31:24] HW_CTS2SELF_PKT_LEN_TH     = S_AX_CTS2S_TH_1K      = 4
//   [23:18] HW_CTS2SELF_PKT_LEN_TH_TWW = S_AX_CTS2S_TH_SEC_256B= 1
//   [16]    HW_CTS2SELF_EN             = 1
```

## L897-899 · `{`

```
// R_AX_PTCL_FSM_MON (0xC6E8):
//   [5:0] PTCL_TX_ARB_TO_THR = 0x3F (~2 ms)
//   [6]   PTCL_TX_ARB_TO_MODE = 0
```

## L907 · `host::mmio_set32(mmio, R_AX_PTCL_COMMON_SETTING_0, 0x3);  // TX_MODE_0/1`

```
// MAC_0 block (mac.c:2952-2960):
```

## L908 · `host::mmio_set32(mmio, R_AX_PTCL_COMMON_SETTING_0, 0x3);  // TX_MODE_0/1`

```
// TX_MODE_0/1
```

## L909 · `host::mmio_clr32(mmio, R_AX_PTCL_COMMON_SETTING_0, 0x1C); // clr TRIGGER_SS`

```
// clr TRIGGER_SS
```

## L910 · `host::mmio_w32_mask(mmio, R_AX_PTCLRPT_FULL_HDL, 0x3 << 4, 1 << 4);`

```
//   R_AX_PTCLRPT_FULL_HDL.SPE_RPT_PATH[5:4] = FWD_TO_WLCPU (1)
```

## L913 · `host::mmio_clr32(mmio, 0xC804, 0x3); // clear RX full modes`

```
// 12. CMAC DMA (8852B)
```

## L914 · `host::mmio_clr32(mmio, 0xC804, 0x3); // clear RX full modes`

```
// clear RX full modes
```

## L919-921 · `fn pcie_post_init(mmio: i32) {`

```
// ═══════════════════════════════════════════════════════════════════
//  PCIe post-init
// ═══════════════════════════════════════════════════════════════════
```

## L924-926 · `let mut ltr0 = host::mmio_r32(mmio, R_AX_LTR_CTRL_0);`

```
// Linux: rtw89_pci_ops_mac_post_init_ax (pci.c:3224) — LTR + addr-info
// format selector + DMA enable. Rings were set up in fw.rs pre_init
// and persist across FWDL.
```

## L928 · `let mut ltr0 = host::mmio_r32(mmio, R_AX_LTR_CTRL_0);`

```
// LTR setup
```

## L935-942 · `const R_AX_TX_ADDRESS_INFO_MODE_SETTING: u32 = 0x8810;`

```
// 8852B addr-info format = 8-byte (non-V1). Without these two writes
// the HW parses our 8-byte addr_info as something else (probably the
// 16-byte V1 layout), can't make sense of it, and silently drops
// every CH8 TX between DMA consumption and PHY transmit.
//   R_AX_TX_ADDRESS_INFO_MODE_SETTING = 0x8810
//     BIT(0) B_AX_HOST_ADDR_INFO_8B_SEL — set: 8-byte addr_info
//   R_AX_PKTIN_SETTING = 0x9A00
//     BIT(1) B_AX_WD_ADDR_INFO_LENGTH  — clear: 8-byte WD addr info
```

## L950-951 · `unsafe { RXQ_SW_IDX = 0; }`

```
// Ring addresses + wp were set in fw.rs pre_init and persist across FWDL.
// Linux mac_post_init_ax does NOT touch RXBD_IDX — don't fight the firmware.
```

## L954 · `host::mmio_clr32(mmio, regs::R_AX_PCIE_DMA_STOP1, 0x000F_FF00);`

```
// Enable ALL TX DMA channels (clear stop bits)
```

## L956 · `host::mmio_clr32(mmio, regs::R_AX_PCIE_DMA_STOP1, (1 << 19) | (1 << 20));`

```
// Clear WPDMA + PCIEIO stops
```

## L959 · `let desa = host::mmio_r32(mmio, regs::R_AX_RXQ_RXBD_DESA_L);`

```
// Verify RXQ state + sanity check PCIe range is accessible post-FWDL
```

## L972-974 · `const RXQ_BD_COUNT: u16 = 32;`

```
// ═══════════════════════════════════════════════════════════════════
//  RXQ ring setup for C2H messages
// ═══════════════════════════════════════════════════════════════════
```

## L978 · `static mut RXQ_SW_IDX: u16 = 0;`

```
/// RXQ state — DMA handle is in fw::RXQ_DMA (set during pre_init)
```

## L981 · `const AX_RXD_RPKT_LEN_MASK: u32      = 0x0000_3FFF; // [13:0]`

```
// AX RX descriptor dword0 fields (from Linux txrx.h)
```

## L982 · `const AX_RXD_RPKT_LEN_MASK: u32      = 0x0000_3FFF; // [13:0]`

```
// [13:0]
```

## L983 · `const AX_RXD_SHIFT_MASK: u32         = 0x0000_C000; // [15:14]`

```
// [15:14]
```

## L984 · `const AX_RXD_RPKT_TYPE_MASK: u32     = 0x0F00_0000; // [27:24]`

```
// [27:24]
```

## L985 · `const AX_RXD_DRV_INFO_SIZE_MASK: u32 = 0x7000_0000; // [30:28]`

```
// [30:28]
```

## L986 · `const AX_RXD_LONG_RXD: u32           = 0x8000_0000; // [31]`

```
// [31]
```

## L988 · `const RX_TYPE_WIFI: u32 = 0;`

```
// Packet types (from Linux core.h rtw89_core_rx_type)
```

## L993 · `static mut RX_BY_TYPE: [u32; 16] = [0; 16];`

```
/// Packet type counters for diagnostics
```

## L998-999 · `static mut SCAN_COMPLETE: bool = false;`

```
/// Set by handle_c2h when SCANOFLD_RSP arrives with rsn=5 (END_SCAN).
/// scan() polls until either this flips to `true` or a timeout expires.
```

## L1002-1005 · `const BSS_TABLE_MAX: usize = 32;`

```
// ── BSS discovery table ─────────────────────────────────────────
// Fills during scan from each beacon's BSSID (addr3) + SSID IE +
// DS Parameter Set IE (primary channel). Dedupe by BSSID so a
// nearby AP that emits 80 beacons in 30 s shows as one row.
```

## L1022-1023 · `fn dma_r8(dma: i32, addr: u32) -> u8 {`

```
/// Read one byte from DMA at unaligned address. Small wrapper so the
/// parser stays readable (DMA only exposes 32-bit word access).
```

## L1030-1032 · `fn bss_upsert(bssid: &[u8; 6], ssid: &[u8], ssid_len: u8, channel: u8) {`

```
/// Insert or update a BSS entry. First call for a new BSSID allocates
/// a new slot (up to BSS_TABLE_MAX). Later calls just bump `count`
/// and refresh channel.
```

## L1039 · `if BSS_TABLE[i].ssid_len == 0 && ssid_len > 0 {`

```
// Refresh SSID if we saw a non-broadcast one later.
```

## L1058 · `}`

```
// If the table is full we just drop further BSSes silently.
```

## L1107 · `fn rxq_poll(mmio: i32) -> u32 {`

```
/// Poll RXQ for new entries. Max 8 packets per call to avoid CPU hogging.
```

## L1118 · `let buf_off = 4096 + (si as u32) * (4096u32);`

```
// Data buffers start at page 1 (offset 4096) in the unified DMA allocation
```

## L1121 · `let rxd_off = buf_off + 4;`

```
// Skip 4-byte rxbd_info (FS/LS/TAG) before the RX descriptor.
```

## L1124 · `let rxd0 = host::dma_r32(data_dma, rxd_off);`

```
// Read AX RX descriptor dword0
```

## L1132 · `let ti = (pkt_type & 0xF) as usize;`

```
// Track packet types
```

## L1136 · `let payload_off = buf_off + 4 + rxd_len + shift + drv_info;`

```
// Payload offset = rxbd_info(4) + RX descriptor + shift + driver info
```

## L1158 · `fn handle_c2h(dma: i32, off: u32) {`

```
/// Handle a C2H firmware message.
```

## L1168-1170 · `if cat == 1 && class == 1 && func == 9 {`

```
// Scan offload response: CAT=1, CLASS=8(OFLD), FUNC=3
// C2H classes (Linux mac.h:472): 0=INFO (REC_ACK, DONE_ACK, C2H_LOG),
//                                 1=OFLD (func 4=MACID_PAUSE_RSP, 9=SCANOFLD_RSP)
```

## L1177 · `3 => { // ENTER_CH — interesting: shows scan progress per channel`

```
// ENTER_CH — interesting: shows scan progress per channel
```

## L1182 · `5 => { // END_SCAN — crucial: shows scan completed with status`

```
// END_SCAN — crucial: shows scan completed with status
```

## L1189-1192 · `if VERBOSE {`

```
// rsn=1 (pre-enter), 2 (listen), 4 (leave) — verbose-only:
// they arrive 3× per channel × 13 channels = 39 lines of
// noise per scan pass. The ENTER (rsn=3) line already
// gives per-channel progress.
```

## L1203-1204 · `let w2 = host::dma_r32(dma, off + 8);`

```
// DONE_ACK — decode H2C identity + return code
// Linux fw.h:3820  W2_CAT[1:0]|W2_CLASS[7:2]|W2_FUNC[15:8]|W2_H2C_RETURN[23:16]|W2_SEQ[31:24]
```

## L1224-1232 · `if !VERBOSE { return; }`

```
// C2H_LOG — FW trace log. Linux: rtw89_fw_log_dump.
// Payload starts right after the 8-byte C2H hdr. Content is either
// struct rtw89_fw_c2h_log_fmt (binary, signature 0xA5A5) or raw ASCII.
// Without the runtime-loaded fmt table we cannot substitute %-args.
// Per scan cycle we receive ~50 LOG-FMTs with fmt_id=0x371..0x374
// that just trace internal state transitions and give no useful
// hint in production. Keep the full parser under VERBOSE and skip
// silently otherwise so the normal scan log stays focused on
// [scan] ch N / [scan] complete / [c2h] DONE_ACK.
```

## L1235 · `let total_len = _len as u32;            // includes 8-byte hdr`

```
// includes 8-byte hdr
```

## L1240-1242 · `let hdr1    = host::dma_r32(dma, payload_off + 4);`

```
// Linux struct rtw89_fw_c2h_log_fmt (fw.h:3845):
//   signature u16 | feature u8 | syntax u8 | fmt_id u32
//   | file_num u8 | line_num u16 | argc u8 | argv/raw[]
```

## L1266 · `host::print("  [c2h LOG] len=");`

```
// Plain ASCII log or missing signature — hex-dump up to 64 bytes.
```

## L1280 · `host::print("  [c2h] cat=");`

```
// Unknown / uninteresting C2H — only in verbose builds.
```

## L1291-1292 · `fn handle_wifi_frame(dma: i32, off: u32, len: u32) {`

```
/// Handle a received WiFi frame — extract BSSID + SSID + channel from
/// beacons / probe responses and update BSS_TABLE.
```

## L1296 · `if len < 10 { return; }`

```
// 802.11 header min (for any type): 24 bytes (mgmt/data) or less (ctrl).
```

## L1303-1304 · `if frame_type != 0 || (frame_subtype != 8 && frame_subtype != 5) {`

```
// Log non-beacon mgmt + any control frame — these carry the answers
// to AUTH/ASSOC TX. addr2 (SA) at offset 10..15 is who sent it.
```

## L1338-1339 · `if len < 24 + 12 { return; }`

```
// 802.11 header: FC(2) + Duration(2) + Addr1(6) + Addr2(6) + Addr3(6) + SeqCtrl(2)
// = 24 bytes. Then beacon fixed body: Timestamp(8) + Interval(2) + Capability(2).
```

## L1345 · `let mut bssid = [0u8; 6];`

```
// BSSID = addr3, offset 16..21 from 802.11 header start.
```

## L1351-1352 · `let ie_start = off + 24 + 12;`

```
// Walk IEs after the 12-byte fixed body. Collect SSID (tag=0) and
// DS Param Set (tag=3, 1 byte = primary channel).
```

## L1366 · `if pos + 2 + ie_len > ie_end { break; }`

```
// Sanity: bad length would run us off the buffer end.
```

## L1371 · `if ie_len > 0 && ie_len <= 32 {`

```
// SSID — may be hidden (len=0) or a valid 1..32 byte name.
```

## L1380 · `if ie_len == 1 {`

```
// DS Parameter Set — 1-byte primary channel.
```

## L1394-1396 · `pub fn listen_only(mmio: i32, seconds: u32) {`

```
// ═══════════════════════════════════════════════════════════════════
//  Listen-only Mode (v0.94 diagnostic)
// ═══════════════════════════════════════════════════════════════════
```

## L1398-1403 · `pub fn listen_only(mmio: i32, seconds: u32) {`

```
/// Passive listen on the currently-tuned channel, no FW scan_offload.
///
/// Purpose: isolate whether the RX pipe (RF → CMAC → RMAC → RXQ DMA) is
/// live at all. If we see type-0 (WiFi) frames here, the radio + MAC path
/// works and the scan_offload ret=4 problem is truly about FW state. If
/// we still see only type-10 (C2H), something upstream of RMAC is dead.
```

## L1407-1412 · `let cur = host::mmio_r32(mmio, 0xCE20);`

```
// Promiscuous RX filter — accept EVERYTHING:
//   clear A1_MATCH (don't require dest = our MAC)
//   clear BCN_CHK_EN (don't drop beacons from other BSSIDs)
//   clear A_BC (bit 2) — don't apply broadcast addr filter
//   keep MC, BC_CAM_MATCH, UC_CAM_MATCH, PWR_MGNT, FTM_REQ, UID_FILTER
//   preserve MPDU_MAX_LEN [21:16]
```

## L1415 · `let promisc: u32 = 0x03004438; // same as scan mode`

```
// same as scan mode
```

## L1419 · `const R_EDCCA_LVL: u32 = 0x1_4884;`

```
// EDCCA to MAX so CCA doesn't suppress weak beacons.
```

## L1436 · `let ticks = seconds * 10; // 100ms per tick`

```
// 100ms per tick
```

## L1470-1472 · `static mut SCAN_SETUP_DONE: bool = false;`

```
// ═══════════════════════════════════════════════════════════════════
//  WiFi Scan
// ═══════════════════════════════════════════════════════════════════
```

## L1474-1478 · `static mut SCAN_SETUP_DONE: bool = false;`

```
/// Start an **active** scan on 2.4GHz channels 1-13.
/// Called 3x from lib.rs to accumulate beacons across passes. The
/// RX_FLTR / EDCCA / BSS-table / probe-pool state is kept in statics
/// so only the first call prints the setup lines — subsequent calls
/// go straight to the channel list + scan_start + END_SCAN poll.
```

## L1481-1483 · `static mut PROBE_PKT_ID: u8 = 0xFF;`

```
/// Probe-Req pkt_id in the FW offload pool, set by first-pass scan().
/// 0xFF = unregistered. Once registered it stays in the pool across
/// scan passes so we only pay the H2C cost once.
```

## L1494-1507 · `let cur = host::mmio_r32(mmio, 0xCE20);`

```
// ── RX filter for scan — Linux fw.c:9103 rtw89_hw_scan_start
// DEFAULT_AX_RX_FLTR drops everything not matching A1 (our MAC), so
// beacons from other APs get filtered out before reaching RXQ.
//
//   DEFAULT = UID_FILTER(3<<24) | A_FTM_REQ | A_PWR_MGNT | A_BCN_CHK_EN
//           | A_BC_CAM_MATCH | A_UC_CAM_MATCH | A_MC | A_BC | A_A1_MATCH
//           = 0x030044BE
//   SCAN   = DEFAULT & ~(A_BCN_CHK_EN | A_BC | A_A1_MATCH)
//           = 0x03004438   ← let broadcasts + beacons through
//
// R_AX_RX_FLTR_OPT = 0xCE20 (reg.h:3312)
// Linux mac.c:2612 uses a PRESERVE-MASK = ~B_AX_RX_MPDU_MAX_LEN_MASK so
// that a scan-mode rx_fltr write cannot zero out MPDU_MAX_LEN. A raw
// w32 would drop MAX_LEN to 0 and RMAC would reject every beacon.
```

## L1509 · `let cfg_mask: u32 = !(0x3F << 16); // preserve bits [21:16]`

```
// preserve bits [21:16]
```

## L1515-1524 · `const R_EDCCA_LVL: u32 = 0x1_4884; // 0x4884 + PHY_CR_BASE (0x10000)`

```
// ── config_edcca(scan=true) — Linux phy.c:8042 ────────────────────
// Saves current EDCCA levels + sets them to EDCCA_MAX (249) so that
// the CCA engine doesn't filter out real frames during scan. Without
// this the FW scans but RX is suppressed by noise floor.
// Registers (all PHY-space, +CR_BASE):
//   R_SEG0R_EDCCA_LVL_V1 = 0x4884
//   B_EDCCA_LVL_MSK0 = GENMASK(7,0)    (edcca_mask)
//   B_EDCCA_LVL_MSK1 = GENMASK(15,8)   (edcca_p_mask)
//   B_EDCCA_LVL_MSK3 = GENMASK(31,24)  (ppdu_mask)
// EDCCA_MAX = 249 (phy.h:130)
```

## L1525 · `const R_EDCCA_LVL: u32 = 0x1_4884; // 0x4884 + PHY_CR_BASE (0x10000)`

```
// 0x4884 + PHY_CR_BASE (0x10000)
```

## L1537-1543 · `if first_pass && unsafe { PROBE_PKT_ID == 0xFF } {`

```
// ── Register probe request in FW offload pool (first pass only) ──
// Linux fw.c:8291 ieee80211_probereq_get → rtw89_fw_h2c_add_pkt_offload.
// The FW stores the probe req body keyed by pkt_id. On each active
// scan channel the FW transmits the stored frame on air from its own
// TX path — bypassing our CH8 DMA entirely. This is the path Linux
// uses for broadcast probes during scan; CH8 is reserved for AUTH/
// ASSOC/Data after association.
```

## L1547-1548 · `let flen = crate::tx::build_probe_req(&sma, 0, &mut frame);`

```
// channel=0: FW fills DS Param per-channel automatically. The
// wildcard SSID (IE 0 len 0) asks every AP to respond.
```

## L1557-1560 · `const N_CH: u8 = 13;`

```
// ── Send channel list ──────────────────────────────────────────
// H2C: ADD_SCANOFLD_CH (CAT=1, CLASS=9, FUNC=0x16)
// Header: ch_num(u8), elem_size(u8=7), arg(u8=0), rsvd(u8=0)
// Then ch_num × 28 bytes per channel
```

## L1562 · `const ELEM_SIZE: u8 = 7; // 28 bytes / 4`

```
// 28 bytes / 4
```

## L1563 · `let hdr_len = 4 + (N_CH as usize) * 28; // 4-byte list header + channels`

```
// 4-byte list header + channels
```

## L1564 · `let mut buf = [0u8; 4 + 13 * 28]; // 368 bytes`

```
// 368 bytes
```

## L1567 · `let pkt_id = unsafe { PROBE_PKT_ID };`

```
// buf[2] = arg = 0, buf[3] = rsvd = 0
```

## L1569-1580 · `let pkt_id = unsafe { PROBE_PKT_ID };`

```
// 2.4GHz ACTIVE channels. Linux prep_chan_list_ax + add_chan_ax + the
// ACTIVE case in rtw89_hw_scan_add_chan_ax (fw.c:8430):
//   period        = RTW89_CHANNEL_TIME (45) for 2.4G non-P2P
//   dwell_time    = 0
//   bw            = RTW89_SCAN_WIDTH (0) = 20MHz
//   ch_band       = RTW89_BAND_2G (0)
//   notify_action = RTW89_SCANOFLD_DEBUG_MASK (0x1F) — all notifs
//   tx_pkt        = true                      — FW is allowed to TX
//   pause_data    = true (CHAN_ACTIVE path)   — data queues paused on-chan
//   num_pkt       = 1                         — one probe per channel
//   pkt_id[0]     = PROBE_PKT_ID              — FW-pool reference
//   probe_id      = RTW89_SCANOFLD_PKT_NONE (0xFF) — per-SSID probe slot unused
```

## L1586 · `let w0: u32 = 45u32`

```
// w0: period[7:0]=45 | dwell[15:8]=0 | center_ch[23:16] | pri_ch[31:24]
```

## L1590-1591 · `let w1: u32 = (0x1F << 3)`

```
// w1: bw[2:0]=0 | action[7:3]=0x1F | num_pkt[11:8] | tx[12]=1
//     | pause_data[13]=1 | ch_band[15:14]=0 | probe_id[23:16]=0xFF
```

## L1597 · `let w2: u32 = if num_pkt > 0 { pkt_id as u32 } else { 0 };`

```
// w2: pkt0[7:0] = probe pkt_id (0 when unregistered/inactive)
```

## L1602 · `}`

```
// w3..w6 stay zero (no further pkt slots used)
```

## L1610 · `fw::h2c_send(mmio, 1, 9, 0x16, true, true, &buf[..hdr_len]);`

```
// Linux: rack=1, dack=1 (fw.c:6393) — FW must send DONE_ACK + WAIT_COND_ADD_CH
```

## L1614-1616 · `let mut scan_cmd = [0u8; 28];`

```
// ── Start scan ─────────────────────────────────────────────────
// H2C: SCANOFLD (CAT=1, CLASS=9, FUNC=0x17)
// struct rtw89_h2c_scanofld: 7 dwords = 28 bytes
```

## L1618 · `let w0: u32 = 1 << 20; // OP = 1 (enable scan)`

```
// w0: MACID=0, NORM_CY=0, PORT_ID=0, BAND=0, OP=1(start)
```

## L1619 · `let w0: u32 = 1 << 20; // OP = 1 (enable scan)`

```
// OP = 1 (enable scan)
```

## L1620-1621 · `let w1: u32 = 1; // NOTIFY_END only`

```
// w1: NOTIFY_END=1, SCAN_TYPE=0 (RTW89_SCAN_ONCE), START_MODE=0(immediate)
//     SCAN_TYPE is option->repeat — 0=ONCE, 1=NORMAL (looping).
```

## L1622 · `let w1: u32 = 1; // NOTIFY_END only`

```
// NOTIFY_END only
```

## L1623 · `let w2: u32 = 0;`

```
// w2: NORM_PD=0, SLOW_PD=0 (Linux default — option = {0} unless explicitly set)
```

## L1632 · `fw::h2c_send(mmio, 1, 9, 0x17, true, true, &scan_cmd);`

```
// Linux: rack=1, dack=1 (fw.c:6585)
```

## L1636-1641 · `unsafe { SCAN_COMPLETE = false; }`

```
// ── Poll for END_SCAN ─────────────────────────────────────────
// FW sweeps all 13 channels at ~100ms each = ~1.3 s per pass; give
// it 8 s headroom before giving up. Each loop iteration drains up
// to 8 C2H frames via rxq_poll — beacons flow through
// handle_wifi_frame → bss_upsert, scan END_SCAN flips
// SCAN_COMPLETE in handle_c2h.
```

## L1648 · `for _ in 0..5u32 {`

```
// Drain any trailing beacons that came in while we were exiting.
```

## L1655-1659 · `pub fn scan_stop_to_channel(mmio: i32, ch: u8) {`

```
/// Send SCANOFLD H2C with OPERATION=0 (stop) and TARGET_CH_MODE=1, parking
/// the FW on `ch` (2.4 GHz, 20 MHz). Linux calls this inside
/// rtw89_hw_scan_complete → rtw89_fw_h2c_scan_offload so the FW hands
/// channel control back to the host on the VIF's channel. Without it
/// the FW stays parked on the last scan channel even after SCANOFLD_END.
```

## L1666 · `let w0: u32 = 0;`

```
// w0: MACID=0, OPERATION=0 (stop), TARGET_CH_BAND=0 (2G)
```

## L1668-1669 · `let w1: u32 = 1                       // NOTIFY_END`

```
// w1: NOTIFY_END=1 | TARGET_CH_MODE=1 | TARGET_CH_BW=0 (20MHz)
//     | TARGET_PRI_CH=ch | TARGET_CENTRAL_CH=ch (20MHz → same)
```

## L1670 · `let w1: u32 = 1                       // NOTIFY_END`

```
// NOTIFY_END
```

## L1671 · `| (1 << 1)                // TARGET_CH_MODE`

```
// TARGET_CH_MODE
```

## L1672 · `| ((ch as u32) << 8)      // TARGET_PRI_CH (bits 8..15)`

```
// TARGET_PRI_CH (bits 8..15)
```

## L1673 · `| ((ch as u32) << 16);    // TARGET_CENTRAL_CH (bits 16..23)`

```
// TARGET_CENTRAL_CH (bits 16..23)
```

## L1676 · `fw::h2c_send(mmio, 1, 9, 0x17, true, true, &cmd);`

```
// w2..w6 = 0 (default PDs, no TSF, no second MACID)
```

## L1680 · `for _ in 0..20u32 {`

```
// Drain C2H until DONE_ACK for this H2C, up to 200 ms.
```

## L1687-1689 · `pub fn dwell(mmio: i32, ms: u32) {`

```
/// Drain RX queue while sleeping for `ms` milliseconds. Used by callers
/// that need to keep beacon/C2H parsing alive during an otherwise-idle
/// wait (e.g., TX smoke test dwelling for a Probe Response).
```

## L1708-1709 · `pub fn scan_summary() {`

```
/// Print the aggregated scan results + BSS table.
/// Called by `lib.rs` after running one or more scan passes.
```

