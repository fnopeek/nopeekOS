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

use crate::host;
use crate::regs;
use crate::fw;
use crate::imr;

/// Global debug-verbosity flag. Flip to `true` for diagnostic runs —
/// every `dbg_checkpoint`, `[c2h LOG-FMT ...]`, scan rsn=1/2/4, and
/// post-scan listen dump will appear. `false` keeps the production log
/// focused on errors + progress milestones.
const VERBOSE: bool = false;

// ── Register addresses (mac.c / reg.h) ─────────────────────────────

// HFC
const R_AX_HCI_FC_CTRL: u32       = 0x8A00;
const R_AX_ACH0_PAGE_CTRL: u32    = 0x8A10;
const R_AX_PUB_PAGE_CTRL1: u32    = 0x8A90;
const R_AX_WP_PAGE_CTRL2: u32     = 0x8AA4;

// STA scheduler
const R_AX_SS_CTRL: u32           = 0x9E10;

// MPDU proc
const R_AX_ACTION_FWD0: u32       = 0x9C04;
const R_AX_TF_FWD: u32            = 0x9C0C;
const R_AX_CUT_AMSDU_CTRL: u32    = 0x9C10;

// Security engine
const R_AX_SEC_ENG_CTRL: u32      = 0x9D00;
const R_AX_SEC_MPDU_PROC: u32     = 0x9D04;

// CMAC scheduler
const R_AX_PREBKF_CFG_1: u32      = 0xC33C;
const R_AX_PREBKF_CFG_0: u32      = 0xC338;
const R_AX_CCA_CFG_0: u32         = 0xC340;
const R_AX_SCH_EXT_CTRL: u32      = 0xC3FC;

// Addr CAM
const R_AX_ADDR_CAM_CTRL: u32     = 0xCE34;

// RX filter
const R_AX_RX_FLTR_OPT: u32       = 0xCE20;
const R_AX_CTRL_FLTR: u32         = 0xCE24;
const R_AX_MGNT_FLTR: u32         = 0xCE28;
const R_AX_DATA_FLTR: u32         = 0xCE2C;
const R_AX_PLCP_HDR_FLTR: u32     = 0xCE04;

// CCA
const R_AX_CCA_CONTROL: u32       = 0xC390;

// NAV
const R_AX_WMAC_NAV_CTL: u32      = 0xCC80;

// Spatial reuse (byte offsets — handled via mmio_set8/clr8)
const R_AX_RX_SR_CTRL: u32        = 0xCE4A;

// TMAC
const R_AX_MAC_LOOPBACK: u32      = 0xCC20;
const R_AX_TCR0: u32              = 0xCA00;
const R_AX_TXD_FIFO_CTRL: u32     = 0xCA1C;

// TRXPTCL
const R_AX_TRXPTCL_RESP_0: u32    = 0xCC04;
const R_AX_RXTRIG_TEST_USER_2: u32 = 0xCCB0;

// RMAC
const R_AX_RESPBA_CAM_CTRL: u32   = 0xCE3C;
const R_AX_RCR: u32               = 0xCE00;

// PTCL
const R_AX_SIFS_SETTING: u32      = 0xC624;
const R_AX_PTCL_COMMON_SETTING_0: u32 = 0xC600;
const R_AX_AGG_LEN_VHT_0: u32     = 0xC618;

// CMAC com
const R_AX_TX_SUB_CARRIER_VALUE: u32 = 0xC088;
const R_AX_PTCL_RRSR1: u32        = 0xC090;

// LTR
const R_AX_LTR_CTRL_0: u32        = 0x8410;
const R_AX_LTR_CTRL_1: u32        = 0x8414;
const R_AX_LTR_IDLE_LATENCY: u32  = 0x8418;
const R_AX_LTR_ACTIVE_LATENCY: u32 = 0x841C;

// RXQ index register — now defined in regs.rs
use crate::regs::R_AX_RXQ_RXBD_IDX;

// ═══════════════════════════════════════════════════════════════════
//  Main entry
// ═══════════════════════════════════════════════════════════════════

pub fn init(mmio: i32) -> bool {
    host::print("\n[wifi] Phase 4: MAC init\n");

    dbg_checkpoint(mmio, "start");

    // ── 0. Enable BB/RF — MUST come before MAC init! ──────────────
    // Linux: rtw8852bx_mac_enable_bb_rf() — full 5-step sequence.
    // Without this, the radio hardware is off and firmware can't scan.
    enable_bb_rf(mmio);
    host::print("  BB/RF: enabled\n");
    dbg_checkpoint(mmio, "after BB/RF");

    // ── 0b. sys_init_ax — 1:1 Linux (mac.c:1696) re-assert after FWDL.
    // Linux runs this in rtw89_mac_init AFTER partial_init (= FWDL).
    // pwr_on_func already sets DMAC/CMAC func_en bits once, but FWDL
    // can disturb them; sys_init_ax overwrites DMAC_FUNC_EN / CLK_EN
    // with exact values (write32, not set32) and re-ORs CMAC. Without
    // this the DMAC state after FWDL may contain extra bits our
    // pwr_on OR'd in (DLE_WDE_EN, DLE_PLE_EN, BBRPT_EN, DMACREG_GCKEN)
    // that Linux's canonical state does NOT set.
    sys_init_ax(mmio);
    dbg_checkpoint(mmio, "after sys_init");

    // ── 1. DLE re-init with SCC quotas ─────────────────────────────
    if !dle_init(mmio) { return false; }

    // ── 2. HFC init (all channels) ─────────────────────────────────
    hfc_init(mmio);

    // ── 3. DMAC sub-inits ──────────────────────────────────────────
    sta_sch_init(mmio);
    mpdu_proc_init(mmio);
    sec_eng_init(mmio);
    dbg_checkpoint(mmio, "after DMAC");

    // ── 4. CMAC init ───────────────────────────────────────────────
    cmac_init(mmio);
    dbg_checkpoint(mmio, "after CMAC");

    // ── 4.5. chip_func_en_ax — moved into sys_init_ax (Phase 1.3).
    // Kept the checkpoint comment for log continuity.
    dbg_checkpoint(mmio, "after chip_func_en");

    // ── 5. Enable IMRs — 1:1 Linux trx_init_ax (mac.c:3929):
    //   a) 11 DMAC per-block IMR enables (imr::enable_dmac)
    //   b) 6 CMAC per-block IMR enables (imr::enable_cmac)
    //   c) err_imr_ctrl_ax(true) — master ERR_IMR unmask
    // Previously we only wrote (c) with 0xFFFFFFFF (which happens to
    // equal DMAC_ERR_IMR_EN / CMAC0_ERR_IMR_EN). The (a)+(b) block
    // IMRs were missing entirely — strongest hypothesis for the H2C
    // pipe wedge after VIF H2Cs. Linux considers (a)+(b)+(c) a single
    // "enable interrupt sources" package and all three are required.
    imr::enable_dmac(mmio);
    imr::enable_cmac(mmio, 0);
    host::mmio_w32(mmio, 0x8520, 0xFFFFFFFF); // DMAC_ERR_IMR (EN = GENMASK(31,0))
    host::mmio_w32(mmio, 0xC160, 0xFFFFFFFF); // CMAC0_ERR_IMR (EN = GENMASK(31,0))
    host::print("  IMR: per-block DMAC+CMAC enabled, master ERR_IMR set\n");

    // ── 6. Host report mode (set_host_rpr_ax) ─────────────────────
    // Linux: mac.c set_host_rpr_ax — route TX release reports to RPQ.
    host::mmio_w32_mask(mmio, 0x9408, 0x3, 2);  // R_AX_WDRLS_CFG: MODE=POH
    host::mmio_set32(mmio, 0x9410, 0xFFFF_FFFF); // R_AX_RLSRPT0_CFG0: filter all
    host::mmio_w32_mask(mmio, 0x9414, 0xFF, 30);       // AGGNUM=30
    host::mmio_w32_mask(mmio, 0x9414, 0xFF << 16, 255); // TO=255
    host::print("  RPR: POH mode\n");

    // ── 6.5. mac_post_init BEFORE phy::init — Linux order ─────────
    // Linux runs mac_post_init_ax (LTR + enable all DMA + TX_ADDR_INFO +
    // clear STOP_WPDMA|STOP_PCIEIO) at the END of mac_init, BEFORE
    // core_start proceeds to reset_bb_rf + phy tables. We had this call
    // AFTER phy::init, leaving PCIe in a stopped state during BB writes.
    pcie_post_init(mmio);
    dbg_checkpoint(mmio, "after mac_post_init");

    // ── 6.6. reset_bb_rf (disable + enable) — MATCHES Linux core_start ───
    // Linux calls rtw89_chip_reset_bb_rf between mac_init and phy_init_bb_reg.
    host::print("  PHY: reset_bb_rf (disable+enable)\n");
    reset_bb_rf(mmio);
    dbg_checkpoint(mmio, "after reset_bb_rf");

    // ── 7. PHY init — BB + RF + NCTL tables ───
    crate::phy::init(mmio);
    dbg_checkpoint(mmio, "after PHY");

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
    let cr_base: u32 = 0x10000;
    // rtw8852b_bb_reset — path 0 + path 1 TXPW manual on, TSSI trk en
    host::mmio_set32(mmio, cr_base + 0x58DC, 1 << 30);    // P0_TXPW_RSTB.MANON
    host::mmio_set32(mmio, cr_base + 0x5818, 1 << 30);    // P0_TSSI_TRK.EN
    host::mmio_set32(mmio, cr_base + 0x78DC, 1 << 30);    // P1_TXPW_RSTB.MANON
    host::mmio_set32(mmio, cr_base + 0x7818, 1 << 30);    // P1_TSSI_TRK.EN
    // bb_reset_all
    host::mmio_w32_mask(mmio, cr_base + 0x1200, 0x7 << 28, 7);  // S0_HW_SI_DIS[30:28] = 7
    host::mmio_w32_mask(mmio, cr_base + 0x3200, 0x7 << 28, 7);  // S1_HW_SI_DIS[30:28] = 7
    host::sleep_ms(1);
    host::mmio_set32(mmio, cr_base + 0x0704, 1 << 1);     // RSTB_ASYNC.ALL = 1
    host::mmio_clr32(mmio, cr_base + 0x0704, 1 << 1);     // RSTB_ASYNC.ALL = 0
    host::mmio_w32_mask(mmio, cr_base + 0x1200, 0x7 << 28, 0);  // S0_HW_SI_DIS[30:28] = 0
    host::mmio_w32_mask(mmio, cr_base + 0x3200, 0x7 << 28, 0);  // S1_HW_SI_DIS[30:28] = 0
    host::mmio_set32(mmio, cr_base + 0x0704, 1 << 1);     // RSTB_ASYNC.ALL = 1
    // Clear path 0/1 TXPW manual + TSSI trk
    host::mmio_clr32(mmio, cr_base + 0x58DC, 1 << 30);
    host::mmio_clr32(mmio, cr_base + 0x5818, 1 << 30);
    host::mmio_clr32(mmio, cr_base + 0x78DC, 1 << 30);
    host::mmio_clr32(mmio, cr_base + 0x7818, 1 << 30);

    // rtw8852b_bb_reset_en(RTW89_BAND_2G, phy_idx=0, en=true) — THIS enables PD/RXCCA
    host::mmio_w32_mask(mmio, cr_base + 0x1200, 0x7 << 28, 0);  // S0_HW_SI_DIS clr
    host::mmio_w32_mask(mmio, cr_base + 0x3200, 0x7 << 28, 0);  // S1_HW_SI_DIS clr
    host::mmio_set32(mmio, cr_base + 0x0704, 1 << 1);           // RSTB_ASYNC.ALL = 1
    host::mmio_clr32(mmio, cr_base + 0x2344, 1 << 31);          // RXCCA.DIS = 0 (2G: enable CCA)
    host::mmio_clr32(mmio, cr_base + 0x0C3C, 1 << 9);           // PD_CTRL.PD_HIT_DIS = 0 (ENABLE packet detect)
    host::print("  BB: reset + bb_reset_en(2G, true) → PD + CCA enabled\n");

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
    host::mmio_clr32(mmio, cr_base + 0x58F0, 1 << 1);
    host::mmio_clr32(mmio, cr_base + 0x78F0, 1 << 1);
    for addr in (0xD200u32..=0xD3FCu32).step_by(4) {
        // Linux uses rtw89_mac_txpwr_write32(phy_idx=0, addr, 0) which goes
        // through TXPWR indirect access. Direct MAC-space write should work
        // at power-on since the table is in local MAC memory.
        host::mmio_w32(mmio, addr, 0);
    }
    host::print("  BB: sethw (EN_SOUND clr + MACID pwr table 0)\n");

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
    host::mmio_w32_mask(mmio, cr_base + 0x4860, 0x1F << 6, 0);   // PD_LOWER_BOUND=0
    host::mmio_w32_mask(mmio, cr_base + 0x4860, 1 << 30, 0);     // spatial_reuse_en=0
    host::mmio_w32_mask(mmio, cr_base + 0x4B74, 1 << 30, 0);     // bmode CCA limit en=0
    host::mmio_w32_mask(mmio, cr_base + 0x4B64, 0xFFu32 << 24, 0); // bmode PD lower=0
    host::mmio_w32_mask(mmio, cr_base + 0x46E8, 1 << 5, 0);      // p0 p20 pagcugc=0
    host::mmio_w32_mask(mmio, cr_base + 0x46EC, 1 << 5, 0);      // p0 s20 pagcugc=0
    host::mmio_w32_mask(mmio, cr_base + 0x47A8, 1 << 5, 0);      // p1 p20 pagcugc=0
    host::mmio_w32_mask(mmio, cr_base + 0x47AC, 1 << 5, 0);      // p1 s20 pagcugc=0
    host::print("  PHY: dig_init (PD thresholds = 0)\n");

    // ── 7.28. env_monitor_init → ccx_top_setting_init — Linux phy.c:5799
    //   Enables CCX measurement engine for channel quality tracking.
    //   ccx_regs_ax (phy.c:8318 area): setting_addr = R_CCX = 0x0C00
    //     en_mask             = B_CCX_EN_MSK             = BIT(0)
    //     trig_opt_mask       = B_CCX_TRIG_OPT_MSK       = BIT(1)
    //     measurement_trig    = B_MEASUREMENT_TRIG_MSK   = BIT(2)
    //     edcca_opt_mask      = B_CCX_EDCCA_OPT_MSK      = GENMASK(6, 4)
    //   RTW89_CCX_EDCCA_BW20_0 = 0
    host::mmio_w32_mask(mmio, cr_base + 0x0C00, 1 << 0, 1);
    host::mmio_w32_mask(mmio, cr_base + 0x0C00, 1 << 1, 1);
    host::mmio_w32_mask(mmio, cr_base + 0x0C00, 1 << 2, 1);
    host::mmio_w32_mask(mmio, cr_base + 0x0C00, 0x7 << 4, 0);
    host::print("  PHY: ccx_top (CCX engine enabled)\n");

    // ── 7.29. cfo_init (subset) — Linux phy.c:4957
    //   Runs dcfo_comp_init. 8852b has cfo_hw_comp=true (rtw8852b.c:1032) so:
    //     PHY R_DCFO_OPT (0x4494), B_DCFO_OPT_EN (BIT(29)) = 1
    //     PHY R_DCFO_WEIGHT (0x4490), B_DCFO_WEIGHT_MSK (GENMASK(27,24)) = 8
    //     MAC R_AX_PWR_UL_CTRL2 (0xD248), B_AX_PWR_UL_CFO_MASK ([2:0]) = 6
    //   Skipping crystal_cap setting (needs efuse xtal_cap we don't parse).
    host::mmio_w32_mask(mmio, cr_base + 0x4494, 1 << 29, 1);
    host::mmio_w32_mask(mmio, cr_base + 0x4490, 0xF << 24, 8);
    host::mmio_w32_mask(mmio, 0xD248, 0x7, 6);
    host::print("  PHY: cfo_init (DCFO + hw comp)\n");

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
    host::mmio_set32(mmio, cr_base + 0x0738, (1 << 3) | (1 << 2));
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
    for i in 0u32..=15 {
        if i == 9 { continue; }
        let page = if i <= 8 { i } else { i - 1 };
        let addr = cr_base + 0x073C + (page << 2);
        let mut val = host::mmio_r32(mmio, addr);
        if i == 6 || i == 7 {
            val |= 1 << 13;
        } else if i == 10 {
            val |= (1 << 13) | (1 << 1);
        } else if i >= 11 {
            val &= !(0xFu32 << 4);  // clear GENMASK(7,4)
            if i == 11 {
                val |= 1 << 1;
            } else if i >= 13 {
                val |= 1 << 20;
            }
        }
        host::mmio_w32(mmio, addr, val);
    }
    host::print("  PHY: physts parsing init (15 pages configured)\n");

    // ── 7.3. cfg_txrx_path(RF_AB, 2G) — Linux rtw8852bx_bb_cfg_txrx_path
    //   (rtw8852b_common.c:1743) — enables RX antenna path. All writes are
    //   PHY-space, so each address gets + PHY_CR_BASE (0x10000).
    // Matches RF_AB (dual-path) + rx_nss=2 branch.
    const CR: u32 = 0x10000;
    //   R_CHBW_MOD_V1=0x49C4, B_ANT_RX_SEG0=GENMASK(3,0) → 3 (RF_AB)
    host::mmio_w32_mask(mmio, CR + 0x49C4, 0xF, 3);
    //   R_FC0_BW_V1=0x49C0, B_ANT_RX_1RCCA_SEG0=GENMASK(17,14) → 3
    host::mmio_w32_mask(mmio, CR + 0x49C0, 0xF << 14, 3);
    //   R_FC0_BW_V1=0x49C0, B_ANT_RX_1RCCA_SEG1=GENMASK(21,18) → 3
    host::mmio_w32_mask(mmio, CR + 0x49C0, 0xF << 18, 3);
    //   R_RXHT_MCS_LIMIT=0x0D18, B_RXHT_MCS_LIMIT=GENMASK(9,8) → 1 (2-stream)
    host::mmio_w32_mask(mmio, CR + 0x0D18, 0x3 << 8, 1);
    //   R_RXVHT_MCS_LIMIT=0x0D18, B_RXVHT_MCS_LIMIT=GENMASK(22,21) → 1
    host::mmio_w32_mask(mmio, CR + 0x0D18, 0x3 << 21, 1);
    //   R_RXHE=0x0D80, B_RXHE_USER_MAX=GENMASK(13,6) → 4
    host::mmio_w32_mask(mmio, CR + 0x0D80, 0xFF << 6, 4);
    //   R_RXHE, B_RXHE_MAX_NSS=GENMASK(16,14) → 1
    host::mmio_w32_mask(mmio, CR + 0x0D80, 0x7 << 14, 1);
    //   R_RXHE, B_RXHETB_MAX_NSS=GENMASK(25,23) → 1
    host::mmio_w32_mask(mmio, CR + 0x0D80, 0x7 << 23, 1);
    //   RFMODE — both P0 and P1 set same for RF_AB:
    //   R_P0_RFMODE=0x12AC, B_P0_RFMODE_ORI_TXRX_FTM_TX=GENMASK(31,4) → 0x1233312
    host::mmio_w32_mask(mmio, CR + 0x12AC, 0xFFFFFFF0, 0x1233312);
    //   R_P0_RFMODE_FTM_RX=0x12B0, B_P0_RFMODE_FTM_RX=GENMASK(11,0) → 0x333
    host::mmio_w32_mask(mmio, CR + 0x12B0, 0xFFF, 0x333);
    //   R_P1_RFMODE=0x32AC → 0x1233312
    host::mmio_w32_mask(mmio, CR + 0x32AC, 0xFFFFFFF0, 0x1233312);
    //   R_P1_RFMODE_FTM_RX=0x32B0 → 0x333
    host::mmio_w32_mask(mmio, CR + 0x32B0, 0xFFF, 0x333);
    //   TXPW reset toggle (P1 for rx_path != RF_A)
    //   R_P1_TXPW_RSTB=0x78DC, bit 30=MANON, bit 31=TSSI → 1 then 3
    host::mmio_w32_mask(mmio, CR + 0x78DC, (1<<30) | (1<<31), 1);
    host::mmio_w32_mask(mmio, CR + 0x78DC, (1<<30) | (1<<31), 3);
    //   R_MAC_SEL=0x09A4, B_MAC_SEL_MOD=GENMASK(4,2) → 0
    host::mmio_w32_mask(mmio, CR + 0x09A4, 0x7 << 2, 0);
    host::print("  TXRX path: RF_AB configured (2G)\n");

    // ── 7.5. cfg_ppdu_status(HOST) — Linux mac.c:6155 rtw89_mac_cfg_ppdu_status_ax
    // Enables PPDU status reports + routes them to HOST. Without this, RX
    // frames stay in FW-internal space and never reach the RXQ DMA ring.
    //   R_AX_PPDU_STAT = 0xCE40:
    //     bit 0 = B_AX_PPDU_STAT_RPT_EN
    //     bit 1 = B_AX_APP_MAC_INFO_RPT
    //     bit 3 = B_AX_APP_PLCP_HDR_RPT
    //     bit 5 = B_AX_PPDU_STAT_RPT_CRC32
    //   R_AX_HW_RPT_FWD = 0x9C18, mask [1:0] = RTW89_PRPT_DEST_HOST(1)
    host::mmio_w32(mmio, 0xCE40, (1 << 0) | (1 << 1) | (1 << 3) | (1 << 5));
    host::mmio_w32_mask(mmio, 0x9C18, 0x3, 1);
    host::print("  PPDU: status rpt → HOST\n");

    // ── 8. H2C set_ofld_cfg — Linux rtw89_fw_h2c_set_ofld_cfg (fw.c:5228)
    // Sent after mac_init + phy tables, tells FW the offload config.
    // Linux: rack=0, dack=1 (fw.c:5243) → FW MUST send DONE_ACK back.
    //   CAT=1 (MAC), CLASS=9 (MAC_FW_OFLD), FUNC=0x14 (OFLD_CFG)
    let ofld_cfg: [u8; 8] = [0x09, 0x00, 0x00, 0x00, 0x5E, 0x00, 0x00, 0x00];
    host::print("  H2C: set_ofld_cfg (dack=1)...\n");
    fw::h2c_send(mmio, 1, 9, 0x14, false, true, &ofld_cfg);
    diag_wait_c2h(mmio, 200, "ofld_cfg");

    // ── 9. H2C fw_log_cfg — Linux rtw89_fw_h2c_fw_log (fw.c:2787).
    // Activates FW trace log routed over C2H with LEVEL=LOUD on components
    // INIT/TASK/PS/ERROR/MLO/SCAN. Without this the FW is silent and our
    // C2H_LOG decoder (handle_c2h cls=0 fn=2) sees nothing — meaning any
    // init/scan/error condition on the FW side is invisible to us.
    // Linux calls this at the end of rtw89_core_start (core.c:5985).
    host::print("  H2C: fw_log_cfg (LEVEL=LOUD, PATH=C2H)...\n");
    fw::h2c_fw_log(mmio, true);
    diag_wait_c2h(mmio, 200, "fw_log");

    host::print("[wifi] MAC + PHY init complete\n");
    true
}

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

pub fn hci_start(mmio: i32) {
    // halt_c2h_intrs (goes to R_AX_HIMR0 at 0x01A0)
    let halt_c2h_intrs: u32 = regs::B_AX_HALT_C2H_INT_EN;

    // intrs[0] → R_AX_PCIE_HIMR00 (0x10B0): the full default mask
    //   from rtw89_pci_config_intr_mask (non-recovery branch).
    let intrs0: u32 = regs::B_AX_TXDMA_STUCK_INT_EN
                   | regs::B_AX_RXDMA_INT_EN
                   | regs::B_AX_RXP1DMA_INT_EN
                   | regs::B_AX_RPQDMA_INT_EN
                   | regs::B_AX_RXDMA_STUCK_INT_EN
                   | regs::B_AX_RDU_INT_EN
                   | regs::B_AX_RPQBD_FULL_INT_EN
                   | regs::B_AX_HS0ISR_IND_INT_EN;

    // intrs[1] → R_AX_PCIE_HIMR10 (0x13B0)
    let intrs1: u32 = regs::B_AX_HC10ISR_IND_INT_EN;

    // Clear any pending ISR bits before unmasking (belt-and-braces —
    // Linux doesn't do this explicitly in ops_start, but IRQs that
    // latched before our IMR was known-good can confuse polling).
    let _ = host::mmio_r32(mmio, regs::R_AX_HISR0);
    let _ = host::mmio_r32(mmio, regs::R_AX_PCIE_HISR00);
    let _ = host::mmio_r32(mmio, regs::R_AX_PCIE_HISR10);

    // Unmask — 1:1 Linux rtw89_pci_enable_intr (pci.c:853):
    host::mmio_w32(mmio, regs::R_AX_HIMR0,       halt_c2h_intrs);
    host::mmio_w32(mmio, regs::R_AX_PCIE_HIMR00, intrs0);
    host::mmio_w32(mmio, regs::R_AX_PCIE_HIMR10, intrs1);

    host::print("  HCI: intr unmasked (HIMR0=0x");
    host::print_hex32(halt_c2h_intrs);
    host::print(" HIMR00=0x");
    host::print_hex32(intrs0);
    host::print(" HIMR10=0x");
    host::print_hex32(intrs1);
    host::print(")\n");
}


/// Diagnose: poll RXQ IDX for up to `max_ms` ms, report any HW_IDX advance.
/// Used to verify H2C → C2H pipe bidirectionality. The "NO C2H" branch
/// fires for fire-and-forget H2Cs (macid_pause, fw_log_cfg, some VIF
/// helpers whose DONE_ACK gets batched behind a later dack=1 H2C) — we
/// keep it silent in non-verbose builds to avoid misleading "H2C pipe
/// 1-way" spam that made earlier logs look like problems when the H2Cs
/// were in fact accepted fine.
fn diag_wait_c2h(mmio: i32, max_ms: u32, tag: &str) {
    let idx0 = host::mmio_r32(mmio, R_AX_RXQ_RXBD_IDX);
    let hw0 = (idx0 >> 16) & 0xFFFF;
    for ms in 0..max_ms {
        let idx = host::mmio_r32(mmio, R_AX_RXQ_RXBD_IDX);
        let hw = (idx >> 16) & 0xFFFF;
        if hw != hw0 {
            host::print("  [diag "); host::print(tag);
            host::print("] C2H arrived after ");
            fw::print_dec(ms as usize);
            host::print("ms (hw ");
            fw::print_dec(hw0 as usize);
            host::print("→");
            fw::print_dec(hw as usize);
            host::print(")\n");
            // Drain what we got
            rxq_poll(mmio);
            return;
        }
        host::sleep_ms(1);
    }
    if !VERBOSE { return; }
    host::print("  [diag "); host::print(tag);
    host::print("] NO C2H after ");
    fw::print_dec(max_ms as usize);
    host::print("ms (hw still ");
    fw::print_dec(hw0 as usize);
    host::print(") — H2C pipe 1-way\n");
}

/// Debug helper: dump a few registers to find where the 0x1000 range dies.
/// CFG1 (0x1000) vs HCI_OPT_CTRL (0x0074) vs SYS_CFG1 (0x00F0):
/// if CFG1=0xFFFFFFFF but the others are sane, only PCIe DMA block is gated.
/// Gated by `VERBOSE` — we leave it silent in production because once init
/// passes cleanly this dump is pure noise; keep the code for re-enabling.
fn dbg_checkpoint(mmio: i32, tag: &str) {
    if !VERBOSE { return; }
    let cfg1 = host::mmio_r32(mmio, regs::R_AX_PCIE_INIT_CFG1);
    let opt  = host::mmio_r32(mmio, 0x0074);
    let sys  = host::mmio_r32(mmio, regs::R_AX_SYS_CFG1);
    host::print("  [dbg "); host::print(tag);
    host::print("] CFG1=0x"); host::print_hex32(cfg1);
    host::print(" OPT=0x"); host::print_hex32(opt);
    host::print(" SYS=0x"); host::print_hex32(sys);
    host::print("\n");
}

// ═══════════════════════════════════════════════════════════════════
//  sys_init_ax — 1:1 Linux mac.c:1696. Runs AFTER FWDL inside
//  rtw89_mac_init. Re-asserts DMAC/CMAC func_en + clk_en + chip
//  OCP_L1. pwr_on_func did this before FWDL, but FWDL can disturb
//  bits and Linux re-canonicalises via write32 (DMAC) + set32 (CMAC).
// ═══════════════════════════════════════════════════════════════════

fn sys_init_ax(mmio: i32) {
    // ── dmac_func_en_ax (mac.c:1651) — DIRECT write32, overwrites any
    // extra bits from pwr_on. For non-8852C (= 8852B):
    //   MAC_FUNC_EN | DMAC_FUNC_EN | MAC_SEC_EN | DISPATCHER_EN
    // | DLE_CPUIO_EN | PKT_IN_EN | DMAC_TBL_EN | PKT_BUF_EN
    // | STA_SCH_EN | TXPKT_CTRL_EN | WD_RLS_EN | MPDU_PROC_EN
    // | DMAC_CRPRT
    //   = 0xFB7D0000 (bits 30,29,28,27,25,24,22,21,20,19,18,16,31)
    let dmac_func_en: u32 =
          regs::B_AX_MAC_FUNC_EN
        | regs::B_AX_DMAC_FUNC_EN
        | regs::B_AX_MAC_SEC_EN
        | regs::B_AX_DISPATCHER_EN
        | regs::B_AX_DLE_CPUIO_EN
        | regs::B_AX_PKT_IN_EN
        | regs::B_AX_DMAC_TBL_EN
        | regs::B_AX_PKT_BUF_EN
        | regs::B_AX_STA_SCH_EN
        | regs::B_AX_TXPKT_CTRL_EN
        | regs::B_AX_WD_RLS_EN
        | regs::B_AX_MPDU_PROC_EN
        | regs::B_AX_DMAC_CRPRT;
    host::mmio_w32(mmio, regs::R_AX_DMAC_FUNC_EN, dmac_func_en);

    // dmac_clk_en: MAC_SEC | DISPATCHER | DLE_CPUIO | PKT_IN
    //            | STA_SCH | TXPKT_CTRL | WD_RLS | BBRPT CLK
    let dmac_clk_en: u32 =
          regs::B_AX_MAC_SEC_CLK_EN
        | regs::B_AX_DISPATCHER_CLK_EN
        | regs::B_AX_DLE_CPUIO_CLK_EN
        | regs::B_AX_PKT_IN_CLK_EN
        | regs::B_AX_STA_SCH_CLK_EN
        | regs::B_AX_TXPKT_CTRL_CLK_EN
        | regs::B_AX_WD_RLS_CLK_EN
        | regs::B_AX_BBRPT_CLK_EN;
    host::mmio_w32(mmio, regs::R_AX_DMAC_CLK_EN, dmac_clk_en);

    // ── cmac_func_en_ax(mac_idx=0, en=true) (mac.c:1605) — SET (OR)
    //   ck_en:   CMAC | PHYINTF | CMAC_DMA | PTCLTOP | SCHEDULER | TMAC | RMAC
    //   func_en: CMAC_EN | CMAC_TXEN | CMAC_RXEN | PHYINTF_EN | CMAC_DMA_EN
    //          | PTCLTOP_EN | SCHEDULER_EN | TMAC_EN | RMAC_EN | CMAC_CRPRT
    let cmac_ck_en: u32 =
          regs::B_AX_CMAC_CKEN
        | regs::B_AX_PHYINTF_CKEN
        | regs::B_AX_CMAC_DMA_CKEN
        | regs::B_AX_PTCLTOP_CKEN
        | regs::B_AX_SCHEDULER_CKEN
        | regs::B_AX_TMAC_CKEN
        | regs::B_AX_RMAC_CKEN;
    host::mmio_set32(mmio, regs::R_AX_CK_EN, cmac_ck_en);

    let cmac_func_en: u32 =
          regs::B_AX_CMAC_EN
        | regs::B_AX_CMAC_TXEN
        | regs::B_AX_CMAC_RXEN
        | regs::B_AX_PHYINTF_EN
        | regs::B_AX_CMAC_DMA_EN
        | regs::B_AX_PTCLTOP_EN
        | regs::B_AX_SCHEDULER_EN
        | regs::B_AX_TMAC_EN
        | regs::B_AX_RMAC_EN
        | regs::B_AX_CMAC_CRPRT;
    host::mmio_set32(mmio, regs::R_AX_CMAC_FUNC_EN, cmac_func_en);

    // ── chip_func_en_ax (mac.c:1685) — 8852B: set OCP_L1_MASK.
    // B_AX_OCP_L1_MASK = GENMASK(15,13) = 0xE000
    host::mmio_set32(mmio, regs::R_AX_SPS_DIG_ON_CTRL0, 0x7 << 13);

    host::print("  SYS: dmac/cmac func_en + clk_en re-asserted\n");
}

// ═══════════════════════════════════════════════════════════════════
//  BB/RF Enable — rtw8852bx_mac_enable_bb_rf()
// ═══════════════════════════════════════════════════════════════════

fn enable_bb_rf(mmio: i32) {
    // Linux __rtw8852bx_mac_enable_bb_rf, order matters.
    // Step 1: Enable BB reset + global reset
    host::mmio_set8(mmio, regs::R_AX_SYS_FUNC_EN,
        regs::B_AX_FEN_BBRSTB | regs::B_AX_FEN_BB_GLB_RSTN);

    // Step 2: SPS digital supply voltage
    host::mmio_w32_mask(mmio, regs::R_AX_SPS_DIG_ON_CTRL0,
        regs::B_AX_REG_ZCDC_H_MASK, 0x1);

    // Step 3: AFE toggle — Linux does SET-CLR-SET. We keep CLR-CLR-SET for now
    // because empirically SET-CLR-SET kills BB writes. Re-evaluate once
    // earlier init steps (disable_bb_rf, reset_bb_rf) are correct.
    host::mmio_clr32(mmio, regs::R_AX_WLRF_CTRL, regs::B_AX_AFC_AFEDIG);
    host::mmio_clr32(mmio, regs::R_AX_WLRF_CTRL, regs::B_AX_AFC_AFEDIG);
    host::mmio_set32(mmio, regs::R_AX_WLRF_CTRL, regs::B_AX_AFC_AFEDIG);

    // Step 4: XTAL SI — enable RF switches S0 + S1 (write 0xC7 full mask)
    fw::write_xtal_si(mmio, regs::XTAL_SI_WL_RFC_S0, 0xC7, 0xFF);
    fw::write_xtal_si(mmio, regs::XTAL_SI_WL_RFC_S1, 0xC7, 0xFF);

    // Step 5: PHY register access cycle time
    host::mmio_set8(mmio, 0x8040, 0x01); // R_AX_PHYREG_SET = XYN_CYCLE
}

/// 1:1 Linux __rtw8852bx_mac_disable_bb_rf (rtw8852b_common.c:2036).
/// Clears WLRF_CTRL.AFEDIG, clears BBRSTB+BB_GLB_RSTN, clears RFC S0/S1
/// enable bits via XTAL SI. Read-modify-write on XTAL SI needs a read,
/// which we don't have; Linux reads the current value and clears one bit.
/// We approximate by writing 0x00 with mask 0x01 (XTAL_SI_RF00S_EN/S1_EN
/// lives at bit 0) — only clears the target bit, other bits stay 0 after
/// pwr_on which is what Linux achieves via RMW in the common case.
pub fn disable_bb_rf(mmio: i32) {
    // Step 1: Clear AFE digital
    host::mmio_clr32(mmio, regs::R_AX_WLRF_CTRL, regs::B_AX_AFC_AFEDIG);

    // Step 2: Clear BB reset + global reset
    host::mmio_clr8(mmio, regs::R_AX_SYS_FUNC_EN,
        regs::B_AX_FEN_BBRSTB | regs::B_AX_FEN_BB_GLB_RSTN);

    // Step 3: Clear XTAL_SI_RF00S_EN on S0 (bit 0)
    //         Clear XTAL_SI_RF10S_EN on S1 (bit 0)
    // Linux does read-modify-write; we write value=0 with mask=0x01 so only
    // bit 0 is affected.
    fw::write_xtal_si(mmio, regs::XTAL_SI_WL_RFC_S0, 0x00, 0x01);
    fw::write_xtal_si(mmio, regs::XTAL_SI_WL_RFC_S1, 0x00, 0x01);
}

/// 1:1 Linux rtw89_chip_reset_bb_rf (mac.h:1321): disable then enable.
/// Linux calls this in core_start BETWEEN mac_init and phy_init_bb_reg,
/// so BB/RF is brought into a clean state before PHY tables are loaded.
pub fn reset_bb_rf(mmio: i32) {
    disable_bb_rf(mmio);
    enable_bb_rf(mmio);
}

// ═══════════════════════════════════════════════════════════════════
//  DLE Init (SCC quotas — normal mode, not DLFW)
// ═══════════════════════════════════════════════════════════════════

fn dle_init(mmio: i32) -> bool {
    // Disable DLE
    host::mmio_clr32(mmio, regs::R_AX_DMAC_FUNC_EN,
        regs::B_AX_DLE_WDE_EN | regs::B_AX_DLE_PLE_EN);

    // Enable DLE clocks
    host::mmio_set32(mmio, regs::R_AX_DMAC_CLK_EN, (1 << 26) | (1 << 23));

    // WDE: page_sel=0(64B), bound=0, free_pages=510
    let mut wde = host::mmio_r32(mmio, regs::R_AX_WDE_PKTBUF_CFG);
    wde &= !(0x3 | (0x3F << 8) | (0x1FFF << 16));
    wde |= 510 << 16;
    host::mmio_w32(mmio, regs::R_AX_WDE_PKTBUF_CFG, wde);

    // PLE: page_sel=1(128B), bound=4, free_pages=496
    let mut ple = host::mmio_r32(mmio, regs::R_AX_PLE_PKTBUF_CFG);
    ple &= !(0x3 | (0x3F << 8) | (0x1FFF << 16));
    ple |= 1 | (4 << 8) | (496 << 16);
    host::mmio_w32(mmio, regs::R_AX_PLE_PKTBUF_CFG, ple);

    // WDE quotas (SCC): register format = min[11:0] | max[27:16]
    host::mmio_w32(mmio, 0x8C40, (446 << 16) | 446); // HIF
    host::mmio_w32(mmio, 0x8C44, (48 << 16) | 48);   // WCPU
    host::mmio_w32(mmio, 0x8C4C, 0);                   // PKT_IN
    host::mmio_w32(mmio, 0x8C50, (16 << 16) | 16);    // CPU_IO

    // PLE quotas (SCC)
    let ple_qt: [u32; 11] = [
        (147 << 16) | 147, 0,              (16 << 16) | 16,
        (20 << 16) | 20,  (17 << 16) | 17, (13 << 16) | 13,
        (89 << 16) | 89,  0,               (32 << 16) | 32,
        (14 << 16) | 14,  (8 << 16) | 8,
    ];
    for i in 0..11u32 {
        host::mmio_w32(mmio, 0x9040 + i * 4, ple_qt[i as usize]);
    }

    // Enable DLE
    host::mmio_set32(mmio, regs::R_AX_DMAC_FUNC_EN,
        regs::B_AX_DLE_WDE_EN | regs::B_AX_DLE_PLE_EN);

    // Poll WDE + PLE ready
    for _ in 0..200 {
        if host::mmio_r32(mmio, 0x8D00) & 0x3 == 0x3 { break; }
        host::sleep_ms(1);
    }
    for _ in 0..200 {
        if host::mmio_r32(mmio, 0x9100) & 0x3 == 0x3 { break; }
        host::sleep_ms(1);
    }
    host::print("  DLE: SCC quotas OK\n");
    true
}

// ═══════════════════════════════════════════════════════════════════
//  HFC Init (all channels)
// ═══════════════════════════════════════════════════════════════════

fn hfc_init(mmio: i32) {
    // Disable HFC before config
    let mut fc = host::mmio_r32(mmio, R_AX_HCI_FC_CTRL);
    fc &= !((1 << 0) | (1 << 3)); // clear FC_EN + CH12_EN
    host::mmio_w32(mmio, R_AX_HCI_FC_CTRL, fc);

    // Per-channel config: R_AX_ACH0_PAGE_CTRL + ch*4
    // Format: min[15:0] | max[31:16]  (grp bit at [30] but all grp_0)
    let ch_cfg: [(u16, u16); 13] = [
        (5, 341),   // ACH0
        (5, 341),   // ACH1
        (4, 342),   // ACH2
        (4, 342),   // ACH3
        (0, 0),     // ACH4
        (0, 0),     // ACH5
        (0, 0),     // ACH6
        (0, 0),     // ACH7
        (4, 342),   // B0MGQ (ch8)
        (4, 342),   // B0HIQ (ch9)
        (0, 0),     // B1MGQ (ch10)
        (0, 0),     // B1HIQ (ch11)
        (40, 0),    // FWCMDQ (ch12)
    ];
    for (i, &(min, max)) in ch_cfg.iter().enumerate() {
        let val = (min as u32) | ((max as u32) << 16);
        host::mmio_w32(mmio, R_AX_ACH0_PAGE_CTRL + (i as u32) * 4, val);
    }

    // Public buffer: grp0=446, grp1=0
    host::mmio_w32(mmio, R_AX_PUB_PAGE_CTRL1, 446); // grp0[10:0]=446, grp1=0
    host::mmio_w32(mmio, R_AX_WP_PAGE_CTRL2, 0);    // wp_thrd=0

    // Enable HFC + H2C
    fc = host::mmio_r32(mmio, R_AX_HCI_FC_CTRL);
    fc |= (1 << 0) | (1 << 3); // FC_EN (bit 0) + CH12_EN (bit 3)
    host::mmio_w32(mmio, R_AX_HCI_FC_CTRL, fc);

    host::print("  HFC: OK\n");
}

// ═══════════════════════════════════════════════════════════════════
//  DMAC sub-inits
// ═══════════════════════════════════════════════════════════════════

fn sta_sch_init(mmio: i32) {
    host::mmio_set32(mmio, R_AX_SS_CTRL, 1); // SS_EN
    for _ in 0..200 {
        if host::mmio_r32(mmio, R_AX_SS_CTRL) & (1 << 31) != 0 { break; } // INIT_DONE
        host::sleep_ms(1);
    }
    host::mmio_set32(mmio, R_AX_SS_CTRL, 1 << 29);  // WARM_INIT
    host::mmio_clr32(mmio, R_AX_SS_CTRL, 1 << 28);  // clr NONEMPTY
}

fn mpdu_proc_init(mmio: i32) {
    host::mmio_w32(mmio, R_AX_ACTION_FWD0, 0x02A9_5A95);
    host::mmio_w32(mmio, R_AX_TF_FWD, 0x0000_AA55);
    host::mmio_w32(mmio, R_AX_CUT_AMSDU_CTRL, 0x010E_05F0);
}

fn sec_eng_init(mmio: i32) {
    let mut val = host::mmio_r32(mmio, R_AX_SEC_ENG_CTRL);
    // Set: CLK_EN_CGCMP, CLK_EN_WAPI, CLK_EN_WEP_TKIP, TX_ENC, RX_DEC, MC_DEC, BC_DEC
    val |= (1 << 0) | (1 << 1) | (1 << 2) | (1 << 4) | (1 << 5) | (1 << 6) | (1 << 7);
    val &= !(1 << 8); // clear TX_PARTIAL_MODE (8852B)
    host::mmio_w32(mmio, R_AX_SEC_ENG_CTRL, val);
    host::mmio_set32(mmio, R_AX_SEC_MPDU_PROC, 0x3); // APPEND_ICV | APPEND_MIC
}

// ═══════════════════════════════════════════════════════════════════
//  CMAC Init (12 sub-functions from cmac_init_ax)
// ═══════════════════════════════════════════════════════════════════

fn cmac_init(mmio: i32) {
    // 1. Scheduler
    host::mmio_w32_mask(mmio, R_AX_PREBKF_CFG_1, 0x7F, 0x47);  // SIFS_MACTXEN
    host::mmio_set32(mmio, R_AX_SCH_EXT_CTRL, 1 << 1);          // RST_TSF_ADV (8852B)
    host::mmio_clr32(mmio, R_AX_CCA_CFG_0, 1 << 5);             // clr BTCCA_EN
    host::mmio_w32_mask(mmio, R_AX_PREBKF_CFG_0, 0x1F, 0x18);   // PREBKF=24us

    // 2. Addr CAM
    let mut cam = host::mmio_r32(mmio, R_AX_ADDR_CAM_CTRL);
    cam |= (1 << 0) | (1 << 1) | 0x7F; // EN + CLR + RANGE
    host::mmio_w32(mmio, R_AX_ADDR_CAM_CTRL, cam);
    for _ in 0..200 {
        if host::mmio_r32(mmio, R_AX_ADDR_CAM_CTRL) & (1 << 1) == 0 { break; }
        host::sleep_ms(1);
    }

    // 3. RX filter — accept all to host
    host::mmio_w32(mmio, R_AX_MGNT_FLTR, 0x5555_5555);
    host::mmio_w32(mmio, R_AX_CTRL_FLTR, 0x5555_5555);
    host::mmio_w32(mmio, R_AX_DATA_FLTR, 0x5555_5555);
    host::mmio_w32(mmio, R_AX_PLCP_HDR_FLTR, 0x75); // CRC/SIG checks

    // 4. CCA control
    let mut cca = host::mmio_r32(mmio, R_AX_CCA_CONTROL);
    cca |= (1 << 0) | (1 << 1) | (1 << 2) | (1 << 3)    // TB checks
         | (1 << 8) | (1 << 9)                             // SIFS checks
         | (1 << 16) | (1 << 17) | (1 << 18) | (1 << 19)  // CTN checks
         | (1 << 20) | (1 << 21) | (1 << 22) | (1 << 23); // CTN CCA
    host::mmio_w32(mmio, R_AX_CCA_CONTROL, cca);

    // 5. NAV
    let mut nav = host::mmio_r32(mmio, R_AX_WMAC_NAV_CTL);
    nav |= (1 << 16) | (1 << 17) | (1 << 26); // TF_UP_NAV + PLCP_UP_NAV + NAV_UPPER
    nav &= !(0xFF << 8);
    nav |= 0xC4 << 8; // NAV_UPPER = 25ms
    host::mmio_w32(mmio, R_AX_WMAC_NAV_CTL, nav);

    // 6. Spatial reuse — disable SR
    host::mmio_clr8(mmio, R_AX_RX_SR_CTRL, 1);

    // 7. TMAC
    host::mmio_clr32(mmio, R_AX_MAC_LOOPBACK, 1);           // disable loopback
    host::mmio_w32_mask(mmio, R_AX_TCR0, 0x7F << 16, 6);    // UDF threshold
    host::mmio_w32_mask(mmio, R_AX_TXD_FIFO_CTRL, 0xF << 12, 7); // HIGH_MCS
    host::mmio_w32_mask(mmio, R_AX_TXD_FIFO_CTRL, 0xF << 8, 7);  // LOW_MCS

    // 8. TRXPTCL
    let mut resp = host::mmio_r32(mmio, R_AX_TRXPTCL_RESP_0);
    resp &= !0xFF; resp |= 0x0A;     // SIFS_CCK = 10
    resp &= !(0xFF << 8); resp |= 0x11 << 8;  // SIFS_OFDM = 17 (8852B)
    host::mmio_w32(mmio, R_AX_TRXPTCL_RESP_0, resp);
    host::mmio_set32(mmio, R_AX_RXTRIG_TEST_USER_2, 1 << 20); // FCSCHK_EN

    // 9. RMAC
    host::mmio_set32(mmio, R_AX_RESPBA_CAM_CTRL, 1 << 2);   // SSN_SEL
    host::mmio_w32_mask(mmio, R_AX_RCR, 0xF, 1);             // CH_EN = 1

    //   9b. B_AX_RX_MPDU_MAX_LEN_MASK — bits [21:16] of R_AX_RX_FLTR_OPT.
    //   Linux rmac_init_ax:2862 computes this from c0_rx_qta * ple_pg_size
    //   / 512. A zero value makes the RMAC reject every incoming WiFi frame
    //   as "too long" — this is why our scan saw only C2H messages (type 10)
    //   and zero WiFi frames (type 0). Safe upper bound: 0x3F = 63 → 32 KB.
    host::mmio_w32_mask(mmio, R_AX_RX_FLTR_OPT, 0x3F << 16, 0x3F);

    // 10. CMAC com
    host::mmio_w32_mask(mmio, R_AX_PTCL_RRSR1, 0xF << 8, 3); // OFDM+CCK

    // 11. PTCL
    host::mmio_set32(mmio, R_AX_PTCL_COMMON_SETTING_0, 0x3);  // TX_MODE_0/1
    host::mmio_clr32(mmio, R_AX_PTCL_COMMON_SETTING_0, 0x1C); // clr TRIGGER_SS

    // 12. CMAC DMA (8852B)
    host::mmio_clr32(mmio, 0xC804, 0x3); // clear RX full modes

    host::print("  CMAC: OK\n");
}

// ═══════════════════════════════════════════════════════════════════
//  PCIe post-init
// ═══════════════════════════════════════════════════════════════════

fn pcie_post_init(mmio: i32) {
    // Linux: rtw89_pci_ops_mac_post_init_ax — simple DMA enable.
    // NO BDRAM reset, NO ring reconfiguration.
    // Rings were set up in fw.rs pre_init and persist across FWDL.

    // LTR setup
    let mut ltr0 = host::mmio_r32(mmio, R_AX_LTR_CTRL_0);
    ltr0 |= (1 << 0) | (1 << 1) | (1 << 2);
    host::mmio_w32(mmio, R_AX_LTR_CTRL_0, ltr0);
    host::mmio_w32(mmio, R_AX_LTR_IDLE_LATENCY, 0x9003_9003);
    host::mmio_w32(mmio, R_AX_LTR_ACTIVE_LATENCY, 0x880B_880B);

    // Ring addresses + wp were set in fw.rs pre_init and persist across FWDL.
    // Linux mac_post_init_ax does NOT touch RXBD_IDX — don't fight the firmware.
    unsafe { RXQ_SW_IDX = 0; }

    // Enable ALL TX DMA channels (clear stop bits)
    host::mmio_clr32(mmio, regs::R_AX_PCIE_DMA_STOP1, 0x000F_FF00);
    // Clear WPDMA + PCIEIO stops
    host::mmio_clr32(mmio, regs::R_AX_PCIE_DMA_STOP1, (1 << 19) | (1 << 20));

    // Verify RXQ state + sanity check PCIe range is accessible post-FWDL
    let desa = host::mmio_r32(mmio, regs::R_AX_RXQ_RXBD_DESA_L);
    let idx = host::mmio_r32(mmio, R_AX_RXQ_RXBD_IDX);
    let cfg1 = host::mmio_r32(mmio, regs::R_AX_PCIE_INIT_CFG1);
    let ch12d = host::mmio_r32(mmio, regs::R_AX_CH12_TXBD_DESA_L);
    host::print("  RXQ: DESA=0x"); host::print_hex32(desa);
    host::print(" IDX=0x"); host::print_hex32(idx);
    host::print(" CFG1=0x"); host::print_hex32(cfg1);
    host::print(" CH12D=0x"); host::print_hex32(ch12d);
    host::print("\n");
    host::print("  PCIe: DMA enabled\n");
}

// ═══════════════════════════════════════════════════════════════════
//  RXQ ring setup for C2H messages
// ═══════════════════════════════════════════════════════════════════

const RXQ_BD_COUNT: u16 = 32;

/// RXQ state — DMA handle is in fw::RXQ_DMA (set during pre_init)
static mut RXQ_SW_IDX: u16 = 0;

// AX RX descriptor dword0 fields (from Linux txrx.h)
const AX_RXD_RPKT_LEN_MASK: u32      = 0x0000_3FFF; // [13:0]
const AX_RXD_SHIFT_MASK: u32         = 0x0000_C000; // [15:14]
const AX_RXD_RPKT_TYPE_MASK: u32     = 0x0F00_0000; // [27:24]
const AX_RXD_DRV_INFO_SIZE_MASK: u32 = 0x7000_0000; // [30:28]
const AX_RXD_LONG_RXD: u32           = 0x8000_0000; // [31]

// Packet types (from Linux core.h rtw89_core_rx_type)
const RX_TYPE_WIFI: u32 = 0;
const RX_TYPE_PPDU: u32 = 1;
const RX_TYPE_C2H: u32  = 10;

/// Packet type counters for diagnostics
static mut RX_BY_TYPE: [u32; 16] = [0; 16];
static mut WIFI_FRAME_COUNT: u32 = 0;
static mut BEACON_COUNT: u32 = 0;

/// Set by handle_c2h when SCANOFLD_RSP arrives with rsn=5 (END_SCAN).
/// scan() polls until either this flips to `true` or a timeout expires.
static mut SCAN_COMPLETE: bool = false;

// ── BSS discovery table ─────────────────────────────────────────
// Fills during scan from each beacon's BSSID (addr3) + SSID IE +
// DS Parameter Set IE (primary channel). Dedupe by BSSID so a
// nearby AP that emits 80 beacons in 30 s shows as one row.
const BSS_TABLE_MAX: usize = 32;

#[derive(Copy, Clone)]
struct BssEntry {
    bssid: [u8; 6],
    ssid: [u8; 32],
    ssid_len: u8,
    channel: u8,
    count: u16,
}

static mut BSS_TABLE: [BssEntry; BSS_TABLE_MAX] = [BssEntry {
    bssid: [0; 6], ssid: [0; 32], ssid_len: 0, channel: 0, count: 0,
}; BSS_TABLE_MAX];
static mut BSS_COUNT: usize = 0;

/// Read one byte from DMA at unaligned address. Small wrapper so the
/// parser stays readable (DMA only exposes 32-bit word access).
fn dma_r8(dma: i32, addr: u32) -> u8 {
    let word = host::dma_r32(dma, addr & !3);
    let shift = (addr & 3) * 8;
    ((word >> shift) & 0xFF) as u8
}

/// Insert or update a BSS entry. First call for a new BSSID allocates
/// a new slot (up to BSS_TABLE_MAX). Later calls just bump `count`
/// and refresh channel.
fn bss_upsert(bssid: &[u8; 6], ssid: &[u8], ssid_len: u8, channel: u8) {
    unsafe {
        for i in 0..BSS_COUNT {
            if BSS_TABLE[i].bssid == *bssid {
                BSS_TABLE[i].count = BSS_TABLE[i].count.saturating_add(1);
                if channel != 0 { BSS_TABLE[i].channel = channel; }
                // Refresh SSID if we saw a non-broadcast one later.
                if BSS_TABLE[i].ssid_len == 0 && ssid_len > 0 {
                    let n = ssid_len.min(32) as usize;
                    BSS_TABLE[i].ssid[..n].copy_from_slice(&ssid[..n]);
                    BSS_TABLE[i].ssid_len = ssid_len.min(32);
                }
                return;
            }
        }
        if BSS_COUNT < BSS_TABLE_MAX {
            let slot = BSS_COUNT;
            BSS_TABLE[slot].bssid = *bssid;
            let n = ssid_len.min(32) as usize;
            BSS_TABLE[slot].ssid[..n].copy_from_slice(&ssid[..n]);
            BSS_TABLE[slot].ssid_len = ssid_len.min(32);
            BSS_TABLE[slot].channel = channel;
            BSS_TABLE[slot].count = 1;
            BSS_COUNT += 1;
        }
        // If the table is full we just drop further BSSes silently.
    }
}

fn print_hex_byte(b: u8) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let s = [HEX[(b >> 4) as usize], HEX[(b & 0xF) as usize]];
    host::print(unsafe { core::str::from_utf8_unchecked(&s) });
}

fn print_bss_table() {
    let n = unsafe { BSS_COUNT };
    host::print("\n[wifi] Discovered BSS: ");
    fw::print_dec(n);
    host::print(" unique AP(s)\n");
    if n == 0 { return; }
    host::print("  ch  bssid              beacons  ssid\n");
    host::print("  ──  ─────────────────  ───────  ────\n");
    unsafe {
        for i in 0..n {
            let e = &BSS_TABLE[i];
            host::print("  ");
            if e.channel < 10 { host::print(" "); }
            fw::print_dec(e.channel as usize);
            host::print("  ");
            for j in 0..6 {
                print_hex_byte(e.bssid[j]);
                if j < 5 { host::print(":"); }
            }
            host::print("  ");
            let c = e.count as usize;
            if c < 10 { host::print("    "); }
            else if c < 100 { host::print("   "); }
            else if c < 1000 { host::print("  "); }
            else { host::print(" "); }
            fw::print_dec(c);
            host::print("  ");
            if e.ssid_len == 0 {
                host::print("<hidden>");
            } else {
                let s = core::str::from_utf8(&e.ssid[..e.ssid_len as usize])
                    .unwrap_or("<non-utf8>");
                host::print(s);
            }
            host::print("\n");
        }
    }
}

/// Poll RXQ for new entries. Max 8 packets per call to avoid CPU hogging.
fn rxq_poll(mmio: i32) -> u32 {
    let (bd_dma, data_dma, sw_idx) = unsafe { (fw::RXQ_DMA, fw::RXQ_DMA, RXQ_SW_IDX) };
    if bd_dma < 0 { return 0; }

    let idx_reg = host::mmio_r32(mmio, R_AX_RXQ_RXBD_IDX);
    let hw_idx = ((idx_reg >> 16) & 0xFFFF) as u16;
    let mut count = 0u32;
    let mut si = sw_idx;

    while si != hw_idx && count < 8 {
        // Data buffers start at page 1 (offset 4096) in the unified DMA allocation
        let buf_off = 4096 + (si as u32) * (4096u32);

        // Skip 4-byte rxbd_info (FS/LS/TAG) before the RX descriptor.
        let rxd_off = buf_off + 4;

        // Read AX RX descriptor dword0
        let rxd0 = host::dma_r32(data_dma, rxd_off);
        let pkt_type = (rxd0 & AX_RXD_RPKT_TYPE_MASK) >> 24;
        let pkt_len = rxd0 & AX_RXD_RPKT_LEN_MASK;
        let shift = ((rxd0 & AX_RXD_SHIFT_MASK) >> 14) * 2;
        let drv_info = ((rxd0 & AX_RXD_DRV_INFO_SIZE_MASK) >> 28) * 8;
        let rxd_len = if rxd0 & AX_RXD_LONG_RXD != 0 { 32u32 } else { 16u32 };

        // Track packet types
        let ti = (pkt_type & 0xF) as usize;
        unsafe { RX_BY_TYPE[ti] += 1; }

        // Payload offset = rxbd_info(4) + RX descriptor + shift + driver info
        let payload_off = buf_off + 4 + rxd_len + shift + drv_info;

        if pkt_type == RX_TYPE_C2H {
            handle_c2h(data_dma, payload_off);
        } else if pkt_type == RX_TYPE_WIFI && pkt_len > 36 {
            handle_wifi_frame(data_dma, payload_off, pkt_len);
        }

        si = (si + 1) % RXQ_BD_COUNT;
        count += 1;
    }

    if count > 0 {
        unsafe { RXQ_SW_IDX = si; }
        let wp = if si == 0 { RXQ_BD_COUNT - 1 } else { si - 1 };
        host::mmio_w16(mmio, R_AX_RXQ_RXBD_IDX, wp);
    }

    count
}

/// Handle a C2H firmware message.
fn handle_c2h(dma: i32, off: u32) {
    let w0 = host::dma_r32(dma, off);
    let w1 = host::dma_r32(dma, off + 4);

    let cat = w0 & 0x3;
    let class = (w0 >> 2) & 0x3F;
    let func = (w0 >> 8) & 0xFF;
    let _len = w1 & 0x3FFF;

    // Scan offload response: CAT=1, CLASS=8(OFLD), FUNC=3
    // C2H classes (Linux mac.h:472): 0=INFO (REC_ACK, DONE_ACK, C2H_LOG),
    //                                 1=OFLD (func 4=MACID_PAUSE_RSP, 9=SCANOFLD_RSP)
    if cat == 1 && class == 1 && func == 9 {
        let w2 = host::dma_r32(dma, off + 8);
        let pri_ch = w2 & 0xFF;
        let rsn = (w2 >> 16) & 0xF;
        let status = (w2 >> 20) & 0xF;
        match rsn {
            3 => { // ENTER_CH — interesting: shows scan progress per channel
                host::print("  [scan] ch ");
                fw::print_dec(pri_ch as usize);
                host::print("\n");
            }
            5 => { // END_SCAN — crucial: shows scan completed with status
                host::print("  [scan] complete (status=");
                fw::print_dec(status as usize);
                host::print(")\n");
                unsafe { SCAN_COMPLETE = true; }
            }
            _ => {
                // rsn=1 (pre-enter), 2 (listen), 4 (leave) — verbose-only:
                // they arrive 3× per channel × 13 channels = 39 lines of
                // noise per scan pass. The ENTER (rsn=3) line already
                // gives per-channel progress.
                if VERBOSE {
                    host::print("  [c2h] scan rsn=");
                    fw::print_dec(rsn as usize);
                    host::print(" ch=");
                    fw::print_dec(pri_ch as usize);
                    host::print("\n");
                }
            }
        }
    } else if cat == 1 && class == 0 && func == 1 {
        // DONE_ACK — decode H2C identity + return code
        // Linux fw.h:3820  W2_CAT[1:0]|W2_CLASS[7:2]|W2_FUNC[15:8]|W2_H2C_RETURN[23:16]|W2_SEQ[31:24]
        let w2 = host::dma_r32(dma, off + 8);
        let h2c_cat    =  w2        & 0x3;
        let h2c_class  = (w2 >> 2)  & 0x3F;
        let h2c_func   = (w2 >> 8)  & 0xFF;
        let h2c_return = (w2 >> 16) & 0xFF;
        let h2c_seq    = (w2 >> 24) & 0xFF;
        host::print("  [c2h] DONE_ACK h2c(cat=");
        fw::print_dec(h2c_cat as usize);
        host::print(" cls=");  fw::print_dec(h2c_class as usize);
        host::print(" fn=0x"); host::print_hex32(h2c_func);
        host::print(" seq=");  fw::print_dec(h2c_seq as usize);
        host::print(" ret=");  fw::print_dec(h2c_return as usize);
        if h2c_return != 0 { host::print(" !!FAIL!!"); }
        host::print(")\n");
    } else if cat == 1 && class == 0 && func == 0 {
        host::print("  [c2h] REC_ACK\n");
    } else if cat == 1 && class == 0 && func == 2 {
        // C2H_LOG — FW trace log. Linux: rtw89_fw_log_dump.
        // Payload starts right after the 8-byte C2H hdr. Content is either
        // struct rtw89_fw_c2h_log_fmt (binary, signature 0xA5A5) or raw ASCII.
        // Without the runtime-loaded fmt table we cannot substitute %-args.
        // Per scan cycle we receive ~50 LOG-FMTs with fmt_id=0x371..0x374
        // that just trace internal state transitions and give no useful
        // hint in production. Keep the full parser under VERBOSE and skip
        // silently otherwise so the normal scan log stays focused on
        // [scan] ch N / [scan] complete / [c2h] DONE_ACK.
        if !VERBOSE { return; }
        let payload_off = off + 8;
        let total_len = _len as u32;            // includes 8-byte hdr
        let body_len = total_len.saturating_sub(8);
        let hdr0 = host::dma_r32(dma, payload_off);
        let sig = (hdr0 & 0xFFFF) as u16;
        if sig == 0xA5A5 && body_len >= 11 {
            // Linux struct rtw89_fw_c2h_log_fmt (fw.h:3845):
            //   signature u16 | feature u8 | syntax u8 | fmt_id u32
            //   | file_num u8 | line_num u16 | argc u8 | argv/raw[]
            let hdr1    = host::dma_r32(dma, payload_off + 4);
            let hdr2    = host::dma_r32(dma, payload_off + 8);
            let feature = ((hdr0 >> 16) & 0xFF) as u8;
            let syntax  = ((hdr0 >> 24) & 0xFF) as u8;
            let fmt_id  = hdr1;
            let file_nr = ( hdr2        & 0xFF) as u8;
            let line_nr = ((hdr2 >>  8) & 0xFFFF) as u16;
            let argc    = ((hdr2 >> 24) & 0xFF) as u8;
            host::print("  [c2h LOG-FMT] feat=0x");
            host::print_hex32(feature as u32);
            host::print(" syn="); fw::print_dec(syntax as usize);
            host::print(" fmt="); host::print_hex32(fmt_id);
            host::print(" file="); fw::print_dec(file_nr as usize);
            host::print(" line="); fw::print_dec(line_nr as usize);
            host::print(" argc="); fw::print_dec(argc as usize);
            host::print(" args[");
            let max_args = core::cmp::min(argc as u32, 8);
            for i in 0..max_args {
                if i > 0 { host::print(" "); }
                host::print_hex32(host::dma_r32(dma, payload_off + 12 + i * 4));
            }
            host::print("]\n");
        } else {
            // Plain ASCII log or missing signature — hex-dump up to 64 bytes.
            host::print("  [c2h LOG] len=");
            fw::print_dec(body_len as usize);
            host::print(" hex=");
            let max = core::cmp::min(body_len, 64);
            let mut i = 0u32;
            while i < max {
                host::print_hex32(host::dma_r32(dma, payload_off + i));
                host::print(" ");
                i += 4;
            }
            host::print("\n");
        }
    } else {
        host::print("  [c2h] cat=");
        fw::print_dec(cat as usize);
        host::print(" cls=");
        fw::print_dec(class as usize);
        host::print(" fn=");
        fw::print_dec(func as usize);
        host::print("\n");
    }
}

/// Handle a received WiFi frame — extract BSSID + SSID + channel from
/// beacons / probe responses and update BSS_TABLE.
fn handle_wifi_frame(dma: i32, off: u32, len: u32) {
    unsafe { WIFI_FRAME_COUNT += 1; }

    // 802.11 header: FC(2) + Duration(2) + Addr1(6) + Addr2(6) + Addr3(6) + SeqCtrl(2)
    // = 24 bytes. Then beacon fixed body: Timestamp(8) + Interval(2) + Capability(2).
    if len < 24 + 12 { return; }

    let fc = host::dma_r32(dma, off) & 0xFFFF;
    let frame_type    = (fc >> 2) & 0x3;
    let frame_subtype = (fc >> 4) & 0xF;
    if frame_type != 0 { return; }
    if frame_subtype != 8 && frame_subtype != 5 { return; }
    unsafe { BEACON_COUNT += 1; }

    // BSSID = addr3, offset 16..21 from 802.11 header start.
    let mut bssid = [0u8; 6];
    for i in 0..6u32 {
        bssid[i as usize] = dma_r8(dma, off + 16 + i);
    }

    // Walk IEs after the 12-byte fixed body. Collect SSID (tag=0) and
    // DS Param Set (tag=3, 1 byte = primary channel).
    let ie_start = off + 24 + 12;
    let ie_end = off + len;
    if ie_start >= ie_end { return; }

    let mut ssid = [0u8; 32];
    let mut ssid_len: u8 = 0;
    let mut channel: u8 = 0;

    let mut pos = ie_start;
    while pos + 2 <= ie_end {
        let tag = dma_r8(dma, pos);
        let ie_len = dma_r8(dma, pos + 1) as u32;

        // Sanity: bad length would run us off the buffer end.
        if pos + 2 + ie_len > ie_end { break; }

        match tag {
            0 => {
                // SSID — may be hidden (len=0) or a valid 1..32 byte name.
                if ie_len > 0 && ie_len <= 32 {
                    for i in 0..ie_len {
                        ssid[i as usize] = dma_r8(dma, pos + 2 + i);
                    }
                    ssid_len = ie_len as u8;
                }
            }
            3 => {
                // DS Parameter Set — 1-byte primary channel.
                if ie_len == 1 {
                    channel = dma_r8(dma, pos + 2);
                }
            }
            _ => {}
        }

        pos += 2 + ie_len;
    }

    bss_upsert(&bssid, &ssid, ssid_len, channel);
}

// ═══════════════════════════════════════════════════════════════════
//  Listen-only Mode (v0.94 diagnostic)
// ═══════════════════════════════════════════════════════════════════

/// Passive listen on the currently-tuned channel, no FW scan_offload.
///
/// Purpose: isolate whether the RX pipe (RF → CMAC → RMAC → RXQ DMA) is
/// live at all. If we see type-0 (WiFi) frames here, the radio + MAC path
/// works and the scan_offload ret=4 problem is truly about FW state. If
/// we still see only type-10 (C2H), something upstream of RMAC is dead.
pub fn listen_only(mmio: i32, seconds: u32) {
    host::print("\n[wifi] Phase 5: passive listen on current channel\n");

    // Promiscuous RX filter — accept EVERYTHING:
    //   clear A1_MATCH (don't require dest = our MAC)
    //   clear BCN_CHK_EN (don't drop beacons from other BSSIDs)
    //   clear A_BC (bit 2) — don't apply broadcast addr filter
    //   keep MC, BC_CAM_MATCH, UC_CAM_MATCH, PWR_MGNT, FTM_REQ, UID_FILTER
    //   preserve MPDU_MAX_LEN [21:16]
    let cur = host::mmio_r32(mmio, 0xCE20);
    let cfg_mask: u32 = !(0x3F << 16);
    let promisc: u32 = 0x03004438; // same as scan mode
    host::mmio_w32(mmio, 0xCE20, (cur & !cfg_mask) | (promisc & cfg_mask));
    host::print("  RX_FLTR: promiscuous (A1/BCN_CHK/A_BC off)\n");

    // EDCCA to MAX so CCA doesn't suppress weak beacons.
    const R_EDCCA_LVL: u32 = 0x1_4884;
    const EDCCA_MAX: u32 = 249;
    let cur = host::mmio_r32(mmio, R_EDCCA_LVL);
    let new = (cur & !0xFF_00_FF_FF)
            | EDCCA_MAX | (EDCCA_MAX << 8) | (EDCCA_MAX << 24);
    host::mmio_w32(mmio, R_EDCCA_LVL, new);
    host::print("  EDCCA: MAX\n");

    let idx0 = host::mmio_r32(mmio, R_AX_RXQ_RXBD_IDX);
    host::print("  RXQ start: host:"); fw::print_dec((idx0 & 0xFFFF) as usize);
    host::print(" hw:"); fw::print_dec(((idx0 >> 16) & 0xFFFF) as usize);
    host::print("\n");
    host::print("  Listening ("); fw::print_dec(seconds as usize);
    host::print("s) on ch 7...\n");

    let mut total_rx = 0u32;
    let ticks = seconds * 10; // 100ms per tick
    for tick in 0..ticks {
        let n = rxq_poll(mmio);
        total_rx += n;
        host::sleep_ms(100);
        if tick > 0 && tick % 50 == 0 {
            let idx = host::mmio_r32(mmio, R_AX_RXQ_RXBD_IDX);
            host::print("  [");
            fw::print_dec((tick / 10) as usize);
            host::print("s] pkts="); fw::print_dec(total_rx as usize);
            host::print(" RXQ=host:"); fw::print_dec((idx & 0xFFFF) as usize);
            host::print(" hw:"); fw::print_dec(((idx >> 16) & 0xFFFF) as usize);
            host::print("\n");
        }
    }

    host::print("\n[wifi] Listen results:\n");
    let types = unsafe { RX_BY_TYPE };
    let wifi_frames = unsafe { WIFI_FRAME_COUNT };
    let beacons = unsafe { BEACON_COUNT };
    host::print("  RX total: "); fw::print_dec(total_rx as usize); host::print("\n");
    host::print("  By type: ");
    for i in 0..16u32 {
        if types[i as usize] > 0 {
            host::print("t"); fw::print_dec(i as usize);
            host::print("="); fw::print_dec(types[i as usize] as usize);
            host::print(" ");
        }
    }
    host::print("\n");
    host::print("  WiFi frames: "); fw::print_dec(wifi_frames as usize); host::print("\n");
    host::print("  Beacons: "); fw::print_dec(beacons as usize); host::print("\n");
}

// ═══════════════════════════════════════════════════════════════════
//  WiFi Scan
// ═══════════════════════════════════════════════════════════════════

/// Start a passive scan on 2.4GHz channels 1-13.
#[allow(dead_code)]
pub fn scan(mmio: i32) {
    host::print("\n[wifi] Phase 5: WiFi scan\n");

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
    let cur = host::mmio_r32(mmio, 0xCE20);
    let cfg_mask: u32 = !(0x3F << 16); // preserve bits [21:16]
    host::mmio_w32(mmio, 0xCE20, (cur & !cfg_mask) | (0x03004438 & cfg_mask));
    host::print("  RX_FLTR: scan mode (BCN_CHK/BC/A1 off, MPDU_MAX_LEN preserved)\n");

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
    const R_EDCCA_LVL: u32 = 0x1_4884; // 0x4884 + PHY_CR_BASE (0x10000)
    const EDCCA_MAX: u32 = 249;
    let cur = host::mmio_r32(mmio, R_EDCCA_LVL);
    let new = (cur & !0xFF_00_FF_FF)
            | EDCCA_MAX
            | (EDCCA_MAX << 8)
            | (EDCCA_MAX << 24);
    host::mmio_w32(mmio, R_EDCCA_LVL, new);
    host::print("  EDCCA: set to MAX for scan\n");

    // ── Send channel list ──────────────────────────────────────────
    // H2C: ADD_SCANOFLD_CH (CAT=1, CLASS=9, FUNC=0x16)
    // Header: ch_num(u8), elem_size(u8=7), arg(u8=0), rsvd(u8=0)
    // Then ch_num × 28 bytes per channel
    const N_CH: u8 = 13;
    const ELEM_SIZE: u8 = 7; // 28 bytes / 4
    let hdr_len = 4 + (N_CH as usize) * 28; // 4-byte list header + channels
    let mut buf = [0u8; 4 + 13 * 28]; // 368 bytes
    buf[0] = N_CH;
    buf[1] = ELEM_SIZE;
    // buf[2] = arg = 0, buf[3] = rsvd = 0

    // 2.4GHz channels: center = primary = channel number
    // Linux prep_chan_list_ax (fw.c:8596) + add_chan_ax (fw.c:8314) values:
    //   period      = RTW89_CHANNEL_TIME (45) for 2.4G non-P2P
    //   dwell_time  = 0 (not set for non-DFS)
    //   bw          = RTW89_SCAN_WIDTH (0) = 20MHz
    //   ch_band     = RTW89_BAND_2G (0)
    //   notify_action = RTW89_SCANOFLD_DEBUG_MASK (0x1F)  ← enables all notifs
    //   tx_pkt      = true (bit 12)  — FW may not TX since num_pkt=0
    //   probe_id    = RTW89_SCANOFLD_PKT_NONE (0xFF)  ← no probe request
    //   pause_data  = true (ACTIVE chan_type in Linux 2G path)
    //   num_pkt     = 0 (passive)
    let channels: [u8; 13] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13];
    for (i, &ch) in channels.iter().enumerate() {
        let off = 4 + i * 28;
        // w0: period[7:0]=45 | dwell[15:8]=0 | center_ch[23:16] | pri_ch[31:24]
        let w0: u32 = 45u32
            | ((ch as u32) << 16)
            | ((ch as u32) << 24);
        // w1: bw[2:0]=0 | action[7:3]=0x1F | num_pkt[11:8]=0 | tx[12]=1
        //     | pause_data[13]=1 | ch_band[15:14]=0 | probe_id[23:16]=0xFF
        //     | dfs[24]=0 | tx_null[25]=0 | random[26]=0
        let w1: u32 = (0x1F << 3)
            | (1 << 12)       // tx_pkt
            | (1 << 13)       // pause_data
            | (0xFF << 16);   // probe_id = PKT_NONE
        buf[off..off + 4].copy_from_slice(&w0.to_le_bytes());
        buf[off + 4..off + 8].copy_from_slice(&w1.to_le_bytes());
        // w2..w6 already zero (no pkt_ids for passive scan with num_pkt=0)
    }

    host::print("  Sending channel list (");
    fw::print_dec(N_CH as usize);
    host::print(" channels)...\n");
    // Linux: rack=1, dack=1 (fw.c:6393) — FW must send DONE_ACK + WAIT_COND_ADD_CH
    fw::h2c_send(mmio, 1, 9, 0x16, true, true, &buf[..hdr_len]);
    diag_wait_c2h(mmio, 200, "add_scanofld_ch");

    // ── Start scan ─────────────────────────────────────────────────
    // H2C: SCANOFLD (CAT=1, CLASS=9, FUNC=0x17)
    // struct rtw89_h2c_scanofld: 7 dwords = 28 bytes
    let mut scan_cmd = [0u8; 28];
    // w0: MACID=0, NORM_CY=0, PORT_ID=0, BAND=0, OP=1(start)
    let w0: u32 = 1 << 20; // OP = 1 (enable scan)
    // w1: NOTIFY_END=1, SCAN_TYPE=0 (RTW89_SCAN_ONCE), START_MODE=0(immediate)
    //     SCAN_TYPE is option->repeat — 0=ONCE, 1=NORMAL (looping).
    let w1: u32 = 1; // NOTIFY_END only
    // w2: NORM_PD=0, SLOW_PD=0 (Linux default — option = {0} unless explicitly set)
    let w2: u32 = 0;
    scan_cmd[0..4].copy_from_slice(&w0.to_le_bytes());
    scan_cmd[4..8].copy_from_slice(&w1.to_le_bytes());
    scan_cmd[8..12].copy_from_slice(&w2.to_le_bytes());

    host::print("  Starting passive scan...\n");
    // Linux: rack=1, dack=1 (fw.c:6585)
    fw::h2c_send(mmio, 1, 9, 0x17, true, true, &scan_cmd);
    diag_wait_c2h(mmio, 200, "scanofld_start");

    // ── Poll for END_SCAN ─────────────────────────────────────────
    // FW sweeps all 13 channels at ~100ms each = ~1.3 s per pass; give
    // it 8 s headroom before giving up. Each loop iteration drains up
    // to 8 C2H frames via rxq_poll — beacons flow through
    // handle_wifi_frame → bss_upsert, scan END_SCAN flips
    // SCAN_COMPLETE in handle_c2h.
    unsafe { SCAN_COMPLETE = false; }
    for _ in 0..80u32 {
        rxq_poll(mmio);
        if unsafe { SCAN_COMPLETE } { break; }
        host::sleep_ms(100);
    }
    // Drain any trailing beacons that came in while we were exiting.
    for _ in 0..5u32 {
        rxq_poll(mmio);
        host::sleep_ms(20);
    }
}

/// Print the aggregated scan results + BSS table.
/// Called by `lib.rs` after running one or more scan passes.
pub fn scan_summary() {
    host::print("\n[wifi] Scan results:\n");
    let types = unsafe { RX_BY_TYPE };
    let wifi_frames = unsafe { WIFI_FRAME_COUNT };
    let beacons = unsafe { BEACON_COUNT };
    host::print("  By type: ");
    for i in 0..16u32 {
        if types[i as usize] > 0 {
            host::print("t"); fw::print_dec(i as usize);
            host::print("="); fw::print_dec(types[i as usize] as usize);
            host::print(" ");
        }
    }
    host::print("\n");
    host::print("  WiFi frames: "); fw::print_dec(wifi_frames as usize); host::print("\n");
    host::print("  Beacons: "); fw::print_dec(beacons as usize); host::print("\n");

    print_bss_table();
}
