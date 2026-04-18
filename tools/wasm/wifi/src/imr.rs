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

use crate::host;

// ── DMAC-seitige IMR-Register ────────────────────────────────────
const R_AX_HOST_DISPATCHER_ERR_IMR: u32   = 0x8850;
const R_AX_CPU_DISPATCHER_ERR_IMR:  u32   = 0x8854;
const R_AX_OTHER_DISPATCHER_ERR_IMR: u32  = 0x8858;
const R_AX_WDE_ERR_IMR:             u32   = 0x8C38;
const R_AX_PLE_ERR_IMR:             u32   = 0x9038;
const R_AX_WDRLS_ERR_IMR:           u32   = 0x9430;
const R_AX_BBRPT_COM_ERR_IMR_ISR:   u32   = 0x960C;
const R_AX_BBRPT_CHINFO_ERR_IMR_ISR: u32  = 0x962C;
const R_AX_BBRPT_DFS_ERR_IMR_ISR:   u32   = 0x963C;
const R_AX_LA_ERRFLAG:              u32   = 0x966C;
const R_AX_CPUIO_ERR_IMR:           u32   = 0x9840;
const R_AX_PKTIN_ERR_IMR:           u32   = 0x9A20;
const R_AX_MPDU_TX_ERR_IMR:         u32   = 0x9BF4;
const R_AX_MPDU_RX_ERR_IMR:         u32   = 0x9CF4;
const R_AX_SEC_DEBUG:               u32   = 0x9D1C;
const R_AX_STA_SCHEDULER_ERR_IMR:   u32   = 0x9EF0;
const R_AX_TXPKTCTL_ERR_IMR_ISR:    u32   = 0x9F1C;
const R_AX_TXPKTCTL_ERR_IMR_ISR_B1: u32   = 0x9F2C;

// ── CMAC-seitige IMR-Register (mac_idx=0 → reg + 0*0x2000) ────────
const R_AX_SCHEDULE_ERR_IMR:        u32   = 0xC3E8;
const R_AX_PTCL_IMR0:               u32   = 0xC6C0;
const R_AX_DLE_CTRL:                u32   = 0xC800;
const R_AX_TMAC_ERR_IMR_ISR:        u32   = 0xCCEC;
const R_AX_PHYINFO_ERR_IMR:         u32   = 0xCCFC;
const R_AX_RMAC_ERR_ISR:            u32   = 0xCEF4;

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

const WDRLS_IMR_SET:        u32 = 0x0000_3327;
const WDRLS_IMR_EN_CLR:     u32 = 0x0000_3337;
const SEC_DEBUG_IMR_ERROR:  u32 = 0x0000_0008;
const MPDU_TX_CLR:          u32 = 0x0000_003E;
const MPDU_RX_CLR:          u32 = 0x0000_000B;
const STA_SCH_IMR_CLR:      u32 = 0x0000_0007;
const STA_SCH_IMR_SET:      u32 = 0x0000_0007;
const TXPKTCTL_B0_CLR:      u32 = 0x0000_030F;
const TXPKTCTL_B0_SET:      u32 = 0x0000_0101;
const TXPKTCTL_B1_CLR:      u32 = 0x0000_030F;
const TXPKTCTL_B1_SET:      u32 = 0x0000_0303;
const WDE_IMR_CLR:          u32 = 0x070F_F0FF;
const WDE_IMR_SET:          u32 = 0x070F_F0FF;
const PLE_IMR_CLR:          u32 = 0x070F_F0FF;
const PLE_IMR_SET:          u32 = 0x070F_F0DF;
const PKTIN_IMR_SET:        u32 = 0x0000_0001;
const HOST_DISP_CLR:        u32 = 0xFF0F_FFFF;
const HOST_DISP_SET:        u32 = 0x0C00_0161;
const CPU_DISP_CLR:         u32 = 0xFF07_FFFF;
const CPU_DISP_SET:         u32 = 0x0400_0062;
const OTHER_DISP_CLR:       u32 = 0x3F03_1F1F;
const CPUIO_IMR_CLR:        u32 = 0x0000_1111;
const CPUIO_IMR_SET:        u32 = 0x0000_1111;
const BBRPT_COM_SET:        u32 = 0x0000_0001;  // NULL_PLPKTID
const BBRPT_CHINFO_CLR:     u32 = 0x0000_00FF;
const BBRPT_DFS_SET:        u32 = 0x0000_0001;
const LA_DATA_LOSS:         u32 = 0x0000_0001;

// CMAC
const SCHEDULER_IMR_CLR:    u32 = 0x0000_0003;  // SORT_NON_IDLE | FSM_TIMEOUT
const SCHEDULER_IMR_SET:    u32 = 0x0000_0001;  // FSM_TIMEOUT
const PTCL_IMR_CLR_ALL:     u32 = 0xFFFF_FFFF;
const PTCL_IMR_SET:         u32 = 0x1080_0001;
const DLE_IMR_CLR:          u32 = 0x0080_C000;
const DLE_IMR_SET:          u32 = 0x0000_C000;
const RMAC_IMR_CLR:         u32 = 0x000F_F000;
const RMAC_IMR_SET:         u32 = 0x000E_4000;
const TMAC_IMR_CLR:         u32 = 0x0000_0780;
const TMAC_IMR_SET:         u32 = 0x0000_0780;

/// Apply mac_idx offset. 8852B: mac_idx=0 → +0, mac_idx=1 → +0x2000.
/// We always use mac_idx=0 for now.
fn idx(reg: u32, mac_idx: u8) -> u32 {
    reg + (mac_idx as u32) * 0x2000
}

// ═══════════════════════════════════════════════════════════════════
//  DMAC side — 11 per-block IMR enables
//  Linux: enable_imr_ax(RTW89_MAC_0, RTW89_DMAC_SEL) (mac.c:3848)
// ═══════════════════════════════════════════════════════════════════

fn wdrls_imr_enable(mmio: i32) {
    // Linux rtw89_wdrls_imr_enable (mac.c:3639).
    host::mmio_clr32(mmio, R_AX_WDRLS_ERR_IMR, WDRLS_IMR_EN_CLR);
    host::mmio_set32(mmio, R_AX_WDRLS_ERR_IMR, WDRLS_IMR_SET);
}

fn wsec_imr_enable(mmio: i32) {
    // Linux rtw89_wsec_imr_enable (mac.c:3647). 8852B imr_info:
    //   wsec_imr_reg = R_AX_SEC_DEBUG, wsec_imr_set = B_AX_IMR_ERROR.
    host::mmio_set32(mmio, R_AX_SEC_DEBUG, SEC_DEBUG_IMR_ERROR);
}

fn mpdu_trx_imr_enable(mmio: i32) {
    // Linux rtw89_mpdu_trx_imr_enable (mac.c:3654). 8852B: SET = 0,
    // so we only clear the "not interesting" mask. 8852C-specific
    // ETH_TYPE / LLC / NW_TYPE / KSRCH clears are skipped.
    host::mmio_clr32(mmio, R_AX_MPDU_TX_ERR_IMR, MPDU_TX_CLR);
    host::mmio_clr32(mmio, R_AX_MPDU_RX_ERR_IMR, MPDU_RX_CLR);
}

fn sta_sch_imr_enable(mmio: i32) {
    // Linux rtw89_sta_sch_imr_enable (mac.c:3682).
    host::mmio_clr32(mmio, R_AX_STA_SCHEDULER_ERR_IMR, STA_SCH_IMR_CLR);
    host::mmio_set32(mmio, R_AX_STA_SCHEDULER_ERR_IMR, STA_SCH_IMR_SET);
}

fn txpktctl_imr_enable(mmio: i32) {
    // Linux rtw89_txpktctl_imr_enable (mac.c:3694). 8852B imr_info:
    //   b0_reg = R_AX_TXPKTCTL_ERR_IMR_ISR, b1_reg = ..._B1.
    host::mmio_clr32(mmio, R_AX_TXPKTCTL_ERR_IMR_ISR,    TXPKTCTL_B0_CLR);
    host::mmio_set32(mmio, R_AX_TXPKTCTL_ERR_IMR_ISR,    TXPKTCTL_B0_SET);
    host::mmio_clr32(mmio, R_AX_TXPKTCTL_ERR_IMR_ISR_B1, TXPKTCTL_B1_CLR);
    host::mmio_set32(mmio, R_AX_TXPKTCTL_ERR_IMR_ISR_B1, TXPKTCTL_B1_SET);
}

fn wde_imr_enable(mmio: i32) {
    // Linux rtw89_wde_imr_enable (mac.c:3708).
    host::mmio_clr32(mmio, R_AX_WDE_ERR_IMR, WDE_IMR_CLR);
    host::mmio_set32(mmio, R_AX_WDE_ERR_IMR, WDE_IMR_SET);
}

fn ple_imr_enable(mmio: i32) {
    // Linux rtw89_ple_imr_enable (mac.c:3716).
    host::mmio_clr32(mmio, R_AX_PLE_ERR_IMR, PLE_IMR_CLR);
    host::mmio_set32(mmio, R_AX_PLE_ERR_IMR, PLE_IMR_SET);
}

fn pktin_imr_enable(mmio: i32) {
    // Linux rtw89_pktin_imr_enable (mac.c:3724).
    host::mmio_set32(mmio, R_AX_PKTIN_ERR_IMR, PKTIN_IMR_SET);
}

fn dispatcher_imr_enable(mmio: i32) {
    // Linux rtw89_dispatcher_imr_enable (mac.c:3730). 8852B imr_info:
    //   other_disp_imr_set = 0 (only CLR).
    host::mmio_clr32(mmio, R_AX_HOST_DISPATCHER_ERR_IMR,  HOST_DISP_CLR);
    host::mmio_set32(mmio, R_AX_HOST_DISPATCHER_ERR_IMR,  HOST_DISP_SET);
    host::mmio_clr32(mmio, R_AX_CPU_DISPATCHER_ERR_IMR,   CPU_DISP_CLR);
    host::mmio_set32(mmio, R_AX_CPU_DISPATCHER_ERR_IMR,   CPU_DISP_SET);
    host::mmio_clr32(mmio, R_AX_OTHER_DISPATCHER_ERR_IMR, OTHER_DISP_CLR);
}

fn cpuio_imr_enable(mmio: i32) {
    // Linux rtw89_cpuio_imr_enable (mac.c:3748).
    host::mmio_clr32(mmio, R_AX_CPUIO_ERR_IMR, CPUIO_IMR_CLR);
    host::mmio_set32(mmio, R_AX_CPUIO_ERR_IMR, CPUIO_IMR_SET);
}

fn bbrpt_imr_enable(mmio: i32) {
    // Linux rtw89_bbrpt_imr_enable (mac.c:3754). 8852B imr_info:
    //   bbrpt_err_imr_set = 0 (only chinfo_clr is written).
    host::mmio_set32(mmio, R_AX_BBRPT_COM_ERR_IMR_ISR,    BBRPT_COM_SET);
    host::mmio_clr32(mmio, R_AX_BBRPT_CHINFO_ERR_IMR_ISR, BBRPT_CHINFO_CLR);
    // chinfo SET = 0, skip.
    host::mmio_set32(mmio, R_AX_BBRPT_DFS_ERR_IMR_ISR,    BBRPT_DFS_SET);
    host::mmio_set32(mmio, R_AX_LA_ERRFLAG,               LA_DATA_LOSS);
}

/// 1:1 Linux enable_imr_ax(mac_idx=0, RTW89_DMAC_SEL) (mac.c:3848).
pub fn enable_dmac(mmio: i32) {
    wdrls_imr_enable(mmio);
    wsec_imr_enable(mmio);
    mpdu_trx_imr_enable(mmio);
    sta_sch_imr_enable(mmio);
    txpktctl_imr_enable(mmio);
    wde_imr_enable(mmio);
    ple_imr_enable(mmio);
    pktin_imr_enable(mmio);
    dispatcher_imr_enable(mmio);
    cpuio_imr_enable(mmio);
    bbrpt_imr_enable(mmio);
}

// ═══════════════════════════════════════════════════════════════════
//  CMAC side — 6 per-block IMR enables
//  Linux: enable_imr_ax(mac_idx, RTW89_CMAC_SEL) (mac.c:3860)
// ═══════════════════════════════════════════════════════════════════

fn scheduler_imr_enable(mmio: i32, mac_idx: u8) {
    // Linux rtw89_scheduler_imr_enable (mac.c:3769).
    let reg = idx(R_AX_SCHEDULE_ERR_IMR, mac_idx);
    host::mmio_clr32(mmio, reg, SCHEDULER_IMR_CLR);
    host::mmio_set32(mmio, reg, SCHEDULER_IMR_SET);
}

fn ptcl_imr_enable(mmio: i32, mac_idx: u8) {
    // Linux rtw89_ptcl_imr_enable (mac.c:3779).
    let reg = idx(R_AX_PTCL_IMR0, mac_idx);
    host::mmio_clr32(mmio, reg, PTCL_IMR_CLR_ALL);
    host::mmio_set32(mmio, reg, PTCL_IMR_SET);
}

fn cdma_imr_enable(mmio: i32, mac_idx: u8) {
    // Linux rtw89_cdma_imr_enable (mac.c:3789). 8852B imr_info:
    //   cdma_imr_0_reg = R_AX_DLE_CTRL. cdma_imr_1 block is 8852C-only.
    let reg = idx(R_AX_DLE_CTRL, mac_idx);
    host::mmio_clr32(mmio, reg, DLE_IMR_CLR);
    host::mmio_set32(mmio, reg, DLE_IMR_SET);
}

fn phy_intf_imr_enable(_mmio: i32, _mac_idx: u8) {
    // Linux rtw89_phy_intf_imr_enable (mac.c:3806). 8852B imr_info:
    //   phy_intf_imr_clr = 0, phy_intf_imr_set = 0 → effective NO-OP
    //   (both RMW would be no-op). Keep symmetric with Linux but skip.
}

fn rmac_imr_enable(mmio: i32, mac_idx: u8) {
    // Linux rtw89_rmac_imr_enable (mac.c:3816).
    let reg = idx(R_AX_RMAC_ERR_ISR, mac_idx);
    host::mmio_clr32(mmio, reg, RMAC_IMR_CLR);
    host::mmio_set32(mmio, reg, RMAC_IMR_SET);
}

fn tmac_imr_enable(mmio: i32, mac_idx: u8) {
    // Linux rtw89_tmac_imr_enable (mac.c:3826).
    let reg = idx(R_AX_TMAC_ERR_IMR_ISR, mac_idx);
    host::mmio_clr32(mmio, reg, TMAC_IMR_CLR);
    host::mmio_set32(mmio, reg, TMAC_IMR_SET);
}

/// 1:1 Linux enable_imr_ax(mac_idx, RTW89_CMAC_SEL) (mac.c:3860).
pub fn enable_cmac(mmio: i32, mac_idx: u8) {
    scheduler_imr_enable(mmio, mac_idx);
    ptcl_imr_enable(mmio, mac_idx);
    cdma_imr_enable(mmio, mac_idx);
    phy_intf_imr_enable(mmio, mac_idx);
    rmac_imr_enable(mmio, mac_idx);
    tmac_imr_enable(mmio, mac_idx);

    // Suppress unused-static warnings for the PHYINFO reg (kept as
    // doc reference; the 8852B imr_info makes phy_intf_imr a no-op).
    let _ = R_AX_PHYINFO_ERR_IMR;
}
