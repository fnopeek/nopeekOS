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

    // ── 4.5. chip_func_en_ax (Linux mac.c:1675) — MISSING until now ───
    // For 8852B: write32_set(R_AX_SPS_DIG_ON_CTRL0, B_AX_OCP_L1_MASK).
    // This sets the Over-Current Protection L1 threshold bits [15:13].
    // Without this, the SPS digital regulator runs with default OCP,
    // and heavy BB register writes can trip the overcurrent protection,
    // which pulls PCIe power down → chip unreachable after BB table.
    host::mmio_set32(mmio, regs::R_AX_SPS_DIG_ON_CTRL0, 0x7 << 13); // B_AX_OCP_L1_MASK
    dbg_checkpoint(mmio, "after chip_func_en");

    // ── 5. Enable IMRs (simplified, matches Linux DMAC_ERR_IMR_EN=all) ──
    host::mmio_w32(mmio, 0x8520, 0xFFFFFFFF); // DMAC_ERR_IMR
    host::mmio_w32(mmio, 0xC160, 0xFFFFFFFF); // CMAC_ERR_IMR

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

    // ── 8. H2C set_ofld_cfg — Linux rtw89_fw_h2c_set_ofld_cfg (fw.c:5228)
    // Sent after mac_init + phy tables, tells FW the offload config.
    // Linux: rack=0, dack=1 (fw.c:5243) → FW MUST send DONE_ACK back.
    //   CAT=1 (MAC), CLASS=9 (MAC_FW_OFLD), FUNC=0x14 (OFLD_CFG)
    let ofld_cfg: [u8; 8] = [0x09, 0x00, 0x00, 0x00, 0x5E, 0x00, 0x00, 0x00];
    host::print("  H2C: set_ofld_cfg (dack=1)...\n");
    fw::h2c_send(mmio, 1, 9, 0x14, false, true, &ofld_cfg);
    diag_wait_c2h(mmio, 200, "ofld_cfg");

    // ── 9. MACID 0 tables — Linux rtw89_mac_vif_init (mac.c:4933) ─────
    // Before FW will treat MACID 0 as an active STA, the DMAC and CMAC
    // tables for MACID 0 must be initialized via indirect-access writes.
    // Without these, FW scan_offload accepts the command and ACKs it,
    // but no RX path is wired up to MACID 0 → no beacons delivered.
    host::print("  MAC: dmac_tbl_init(macid=0)\n");
    dmac_tbl_init(mmio, 0);
    host::print("  MAC: cmac_tbl_init(macid=0)\n");
    cmac_tbl_init(mmio, 0);

    // ── 10. macid_pause(0, false) — Linux mac.c:4325 / fw.c:5088 ─────
    // Tells FW "MACID 0 is active and not paused". Without this, FW may
    // drop any RX frames destined for MACID 0.
    //   CAT=1 (MAC), CLASS=9 (FW_OFLD), FUNC=0x8 (MACID_PAUSE), rack=1 dack=0
    //   Payload (32B): pause_grp[4]=0, mask_grp[4] = {BIT(0), 0, 0, 0}
    let mut pause_payload = [0u8; 32];
    pause_payload[16] = 0x01; // mask_grp[0] = BIT(0) → MACID 0
    host::print("  H2C: macid_pause(0, false)...\n");
    fw::h2c_send(mmio, 1, 9, 0x8, true, false, &pause_payload);
    diag_wait_c2h(mmio, 200, "macid_pause");

    host::print("[wifi] MAC + PHY init complete\n");
    true
}

// ═══════════════════════════════════════════════════════════════════
//  DMAC/CMAC table init — Linux mac.c:4291/4306
// ═══════════════════════════════════════════════════════════════════
//
// The DMAC and CMAC tables are per-MACID register banks accessed via
// two MAC registers:
//   R_AX_FILTER_MODEL_ADDR  = 0x0C04  — write target base addr here
//   R_AX_INDIR_ACCESS_ENTRY = 0x40000 — then write data through this window
//
// DMAC_TBL_BASE_ADDR = 0x18800000, CMAC_TBL_BASE_ADDR = 0x18840000
// Each MACID gets 16 bytes in DMAC (4 u32s) and 32 bytes in CMAC (8 u32s).

const R_AX_FILTER_MODEL_ADDR: u32  = 0x0C04;
const R_AX_INDIR_ACCESS_ENTRY: u32 = 0x40000;
const DMAC_TBL_BASE_ADDR: u32 = 0x18800000;
const CMAC_TBL_BASE_ADDR: u32 = 0x18840000;
const CCTL_INFO_SIZE: u32 = 32;

/// Linux mac.c:4291 rtw89_mac_dmac_tbl_init — 4 u32 writes per MACID.
fn dmac_tbl_init(mmio: i32, macid: u8) {
    for i in 0..4u32 {
        let target = DMAC_TBL_BASE_ADDR + ((macid as u32) << 4) + (i << 2);
        host::mmio_w32(mmio, R_AX_FILTER_MODEL_ADDR, target);
        host::mmio_w32(mmio, R_AX_INDIR_ACCESS_ENTRY, 0);
    }
}

/// Linux mac.c:4306 rtw89_mac_cmac_tbl_init — 8 u32 writes per MACID.
/// Values from Linux are hardcoded defaults for an initial STA entry.
fn cmac_tbl_init(mmio: i32, macid: u8) {
    let target = CMAC_TBL_BASE_ADDR + (macid as u32) * CCTL_INFO_SIZE;
    host::mmio_w32(mmio, R_AX_FILTER_MODEL_ADDR, target);
    host::mmio_w32(mmio, R_AX_INDIR_ACCESS_ENTRY + 0,  0x4);
    host::mmio_w32(mmio, R_AX_INDIR_ACCESS_ENTRY + 4,  0x400A0004);
    host::mmio_w32(mmio, R_AX_INDIR_ACCESS_ENTRY + 8,  0);
    host::mmio_w32(mmio, R_AX_INDIR_ACCESS_ENTRY + 12, 0);
    host::mmio_w32(mmio, R_AX_INDIR_ACCESS_ENTRY + 16, 0);
    host::mmio_w32(mmio, R_AX_INDIR_ACCESS_ENTRY + 20, 0x0E43000B);
    host::mmio_w32(mmio, R_AX_INDIR_ACCESS_ENTRY + 24, 0);
    host::mmio_w32(mmio, R_AX_INDIR_ACCESS_ENTRY + 28, 0x000B8109);
}

/// Diagnose: poll RXQ IDX for up to `max_ms` ms, report any HW_IDX advance.
/// Used to verify H2C → C2H pipe bidirectionality.
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
fn dbg_checkpoint(mmio: i32, tag: &str) {
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
    if cat == 1 && class == 8 && func == 3 {
        let w2 = host::dma_r32(dma, off + 8);
        let pri_ch = w2 & 0xFF;
        let rsn = (w2 >> 16) & 0xF;
        let status = (w2 >> 20) & 0xF;
        match rsn {
            3 => { // ENTER_CH
                host::print("  [scan] ch ");
                fw::print_dec(pri_ch as usize);
                host::print("\n");
            }
            5 => { // END_SCAN
                host::print("  [scan] complete (status=");
                fw::print_dec(status as usize);
                host::print(")\n");
            }
            _ => {
                host::print("  [c2h] scan rsn=");
                fw::print_dec(rsn as usize);
                host::print(" ch=");
                fw::print_dec(pri_ch as usize);
                host::print("\n");
            }
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

/// Handle a received WiFi frame — extract SSID from beacons/probe responses.
fn handle_wifi_frame(dma: i32, off: u32, len: u32) {
    unsafe { WIFI_FRAME_COUNT += 1; }

    // 802.11 header: FC(2) + Duration(2) + Addr1(6) + Addr2(6) + Addr3(6) + SeqCtrl(2) = 24 bytes
    if len < 24 + 12 { return; } // too short for beacon

    // Read frame control (first 2 bytes of 802.11 header)
    let fc_word = host::dma_r32(dma, off);
    let fc = fc_word & 0xFFFF;
    let frame_type = (fc >> 2) & 0x3;    // type field
    let frame_subtype = (fc >> 4) & 0xF;  // subtype field

    // Only process beacon (type=0/mgmt, subtype=8) or probe response (subtype=5)
    if frame_type != 0 { return; }
    if frame_subtype != 8 && frame_subtype != 5 { return; }
    unsafe { BEACON_COUNT += 1; }

    // Beacon/Probe fixed fields after 802.11 header:
    // Timestamp(8) + Interval(2) + Capability(2) = 12 bytes
    // Then Information Elements follow
    let ie_start = off + 24 + 12;
    let ie_end = off + len;
    if ie_start >= ie_end { return; }

    // Find SSID IE (tag=0)
    let mut pos = ie_start;
    while pos + 2 <= ie_end {
        let tag_len = host::dma_r32(dma, pos & !3); // aligned read
        let byte_off = (pos & 3) * 8;
        let tag = ((tag_len >> byte_off) & 0xFF) as u8;

        // Read length byte
        let len_pos = pos + 1;
        let tag_len2 = host::dma_r32(dma, len_pos & !3);
        let byte_off2 = (len_pos & 3) * 8;
        let ie_len = ((tag_len2 >> byte_off2) & 0xFF) as u32;

        if tag == 0 && ie_len > 0 && ie_len <= 32 {
            // SSID found! Read SSID bytes
            let mut ssid = [0u8; 33];
            let ssid_start = pos + 2;
            let mut buf = [0u8; 4];
            for i in 0..ie_len {
                let addr = ssid_start + i;
                let w = host::dma_r32(dma, addr & !3);
                let shift = (addr & 3) * 8;
                ssid[i as usize] = ((w >> shift) & 0xFF) as u8;
            }

            // Print SSID
            host::print("  ");
            let s = core::str::from_utf8(&ssid[..ie_len as usize]).unwrap_or("?");
            host::print(s);
            host::print("\n");
            return;
        }

        if tag == 0 && ie_len == 0 {
            return; // hidden SSID
        }

        pos += 2 + ie_len;
    }
}

// ═══════════════════════════════════════════════════════════════════
//  WiFi Scan
// ═══════════════════════════════════════════════════════════════════

/// Start a passive scan on 2.4GHz channels 1-13.
pub fn scan(mmio: i32) {
    host::print("\n[wifi] Phase 5: WiFi scan\n");

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
    let channels: [u8; 13] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13];
    for (i, &ch) in channels.iter().enumerate() {
        let off = 4 + i * 28;
        // w0: period[7:0]=0 | dwell[15:8]=50 | center_ch[23:16] | pri_ch[31:24]
        let w0: u32 = (50 << 8) | ((ch as u32) << 16) | ((ch as u32) << 24);
        // w1: bw=0(20MHz) | action=0 | ch_band=0(2.4GHz) | rest=0
        let w1: u32 = 0;
        // w2-w6: all zero (no probe request IDs for passive scan)
        buf[off..off + 4].copy_from_slice(&w0.to_le_bytes());
        buf[off + 4..off + 8].copy_from_slice(&w1.to_le_bytes());
        // w2-w6 already zero
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
    // w1: NOTIFY_END=1, SCAN_TYPE=1(passive), START_MODE=0(immediate)
    let w1: u32 = 1 | (1 << 3); // NOTIFY_END + SCAN_TYPE=passive
    // w2: NORM_PD=50 (50ms normal period)
    let w2: u32 = 50;
    scan_cmd[0..4].copy_from_slice(&w0.to_le_bytes());
    scan_cmd[4..8].copy_from_slice(&w1.to_le_bytes());
    scan_cmd[8..12].copy_from_slice(&w2.to_le_bytes());

    host::print("  Starting passive scan...\n");
    // Linux: rack=1, dack=1 (fw.c:6585)
    fw::h2c_send(mmio, 1, 9, 0x17, true, true, &scan_cmd);
    diag_wait_c2h(mmio, 200, "scanofld_start");

    // ── Poll for results (15 seconds) ────────────────────────────
    // Show initial RXQ state
    let idx0 = host::mmio_r32(mmio, R_AX_RXQ_RXBD_IDX);
    host::print("  RXQ IDX=0x"); host::print_hex32(idx0);
    host::print(" (host="); fw::print_dec((idx0 & 0xFFFF) as usize);
    host::print(" hw="); fw::print_dec(((idx0 >> 16) & 0xFFFF) as usize);
    host::print(")\n");
    host::print("  Listening (15s)...\n");
    let mut total_rx = 0u32;
    // 150 ticks × 100ms = 15 seconds max
    for tick in 0..150u32 {
        let n = rxq_poll(mmio);
        total_rx += n;

        // 100ms sleep (don't use input_wait — key routing may be broken)
        host::sleep_ms(100);

        // Progress every 5 seconds
        if tick > 0 && tick % 50 == 0 {
            host::print("  [");
            fw::print_dec((tick / 10) as usize);
            host::print("s] ");
            fw::print_dec(total_rx as usize);
            host::print(" pkts\n");
        }
    }

    // ── Diagnostik ─────────────────────────────────────────────────
    host::print("\n[wifi] Scan results:\n");
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

    // Dump first buffer: rxbd_info + RXD for debugging (data at offset 4096)
    let data_dma = unsafe { fw::RXQ_DMA };
    if data_dma >= 0 {
        host::print("  BUF[0]: ");
        for i in 0..6u32 {
            host::print_hex32(host::dma_r32(data_dma, 4096 + i * 4));
            host::print(" ");
        }
        host::print("\n");
    }
}
