//! VIF (Virtual Interface) init — 1:1 port of Linux rtw89_mac_vif_init
//!
//! Linux chain (mac.c:4933):
//!   1. rtw89_mac_port_update           (mac.c:4992)
//!   2. rtw89_mac_dmac_tbl_init         (mac.c:4291)
//!   3. rtw89_mac_cmac_tbl_init         (mac.c:4306)
//!   4. rtw89_mac_set_macid_pause       (mac.c:4325) → fw.c:5088
//!   5. rtw89_fw_h2c_role_maintain      (fw.c:4857)
//!   6. rtw89_fw_h2c_join_info          (fw.c:4953)
//!   7. rtw89_cam_init                  (cam.c:741)       [software only]
//!   8. rtw89_fw_h2c_cam                (fw.c:2221)
//!   9. rtw89_chip_h2c_default_cmac_tbl (fw.c:3521)
//!
//! All tailored for: STATION, port 0, band 0, MACID 0, NOT CONNECTED (dis_conn=true).

use crate::host;
use crate::fw;

/// Station MAC address. Initially a pseudo value; lib.rs overrides this
/// with the real chip MAC read from efuse as soon as efuse::read succeeds.
/// Probe Requests and the VIF addr_cam use whatever is stored here at the
/// point of the call.
pub static mut STA_MAC: [u8; 6] = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55];

pub fn sta_mac() -> [u8; 6] {
    unsafe { STA_MAC }
}

// ── rtw89 enum values (core.h) ────────────────────────────────────
const NET_TYPE_NO_LINK: u32    = 0;
const NET_TYPE_INFRA: u32      = 2;
const WIFI_ROLE_STATION: u32   = 1;
const SELF_ROLE_CLIENT: u32    = 0;
const UPD_MODE_CREATE: u32     = 0;
const ADDR_CAM_SEC_NORMAL: u32 = 2;
const BSSID_MATCH_ALL: u32     = 0x3F;         // GENMASK(5,0)

// CAM entry sizes (mac.h)
const ADDR_CAM_ENT_SIZE: u32  = 0x40;
const BSSID_CAM_ENT_SIZE: u32 = 0x08;

// MACID table base addresses (mac.h:309/315)
const R_AX_FILTER_MODEL_ADDR: u32  = 0x0C04;
const R_AX_INDIR_ACCESS_ENTRY: u32 = 0x40000;
const DMAC_TBL_BASE_ADDR: u32 = 0x18800000;
const CMAC_TBL_BASE_ADDR: u32 = 0x18840000;
const CCTL_INFO_SIZE: u32 = 32;

// Port 0 register addresses (reg.h) — rtw89_port_base_ax (mac.c:4347)
const R_AX_PORT_CFG_P0: u32        = 0xC400;
const R_AX_TBTT_PROHIB_P0: u32     = 0xC404;
const R_AX_BCN_AREA_P0: u32        = 0xC408;
const R_AX_BCNERLYINT_CFG_P0: u32  = 0xC40C;
const R_AX_TBTTERLYINT_CFG_P0: u32 = 0xC40E;
const R_AX_TBTT_AGG_P0: u32        = 0xC412;
const R_AX_BCN_SPACE_CFG_P0: u32   = 0xC414;
const R_AX_DTIM_CTRL_P0: u32       = 0xC426;
const R_AX_BCN_CNT_TMR_P0: u32     = 0xC434;
const R_AX_MD_TSFT_STMP_CTL: u32   = 0xCA08;
const R_AX_PTCL_BSS_COLOR_0: u32   = 0xC6A0;
const R_AX_MBSSID_CTRL: u32        = 0xC568;
const R_AX_MBSSID_DROP_0: u32      = 0xC63C;
const R_AX_P0MB_HGQ_WINDOW_CFG_0: u32 = 0xC590;
const R_AX_BCN_PSR_RPT_P0: u32     = 0xCE84;

// Port cfg bit masks (reg.h)
const B_AX_BRK_SETUP: u32       = 1 << 16;
const B_AX_TBTT_PROHIB_EN: u32  = 1 << 13;
const B_AX_BCNTX_EN: u32        = 1 << 12;
const B_AX_NET_TYPE_MASK: u32   = 0x3 << 10;
const B_AX_TSFTR_RST: u32       = 1 << 5;
const B_AX_RX_BSSID_FIT_EN: u32 = 1 << 4;
const B_AX_TSF_UDT_EN: u32      = 1 << 3;
const B_AX_PORT_FUNC_EN: u32    = 1 << 2;
const B_AX_TXBCN_RPT_EN: u32    = 1 << 1;
const B_AX_RXBCN_RPT_EN: u32    = 1 << 0;

// Defaults from mac.c:4435
const BCN_INTERVAL: u32 = 100;
const BCN_ERLY_DEF: u32 = 160;
const BCN_SETUP_DEF: u32 = 2;
const BCN_HOLD_DEF: u32 = 200;
const TBTT_ERLY_DEF: u32 = 5;
const TBTT_AGG_DEF: u32 = 1;

// ═══════════════════════════════════════════════════════════════════
//  Main entry — full VIF init for MACID 0, port 0, band 0
// ═══════════════════════════════════════════════════════════════════

pub fn init(mmio: i32, macid: u8) -> bool {
    host::print("\n[wifi] Phase 5: VIF init (full Linux rtw89_mac_vif_init) macid=");
    fw::print_dec(macid as usize);
    host::print("\n");

    // Strict 1:1 port of Linux rtw89_mac_vif_init (mac.c:4942). The earlier
    // "minimal" two-H2C path was a v0.93 shortcut that left macid 0 with
    // empty DMAC/CMAC tables, so role_maintain / addr_cam had no state to
    // register against — the FW silently dropped both and wedged the H2C
    // pipe (v1.0.0 log showed both H2Cs timed out with NO C2H, and every
    // subsequent scan H2C also timed out).
    //
    // Linux order (all 8 steps for AX non-secure-boot; .h2c_default_dmac_tbl
    // is NULL for 8852B so step 9 drops out):
    //   1. mac_port_update         (MMIO, STA on port 0 NO_LINK)
    //   2. mac_dmac_tbl_init(macid)(MMIO INDIR_ACCESS, 4 × 0)
    //   3. mac_cmac_tbl_init(macid)(MMIO INDIR_ACCESS, 8 × defaults)
    //   4. set_macid_pause(false)  (H2C MAC_FW_OFLD, rack=1 dack=0)
    //   5. h2c_role_maintain       (H2C MEDIA_RPT, rack=0 dack=1)
    //   6. h2c_join_info(dis_conn) (H2C MEDIA_RPT, rack=0 dack=1)
    //   7. cam_init + h2c_cam      (SW + H2C ADDR_CAM_UPDATE, rack=0 dack=1)
    //   8. h2c_default_cmac_tbl    (H2C FR_EXCHG, rack=0 dack=1)

    host::print("  VIF: 1. port_update(p0 NO_LINK)\n");
    port_update_p0_nolink(mmio);

    host::print("  VIF: 2. dmac_tbl_init\n");
    dmac_tbl_init(mmio, macid);

    host::print("  VIF: 3. cmac_tbl_init\n");
    cmac_tbl_init(mmio, macid);

    host::print("  VIF: 4. macid_pause(unpause)\n");
    h2c_macid_pause(mmio, macid, false);
    // Linux sends with rack=1, dack=0 — REC_ACK arrives asynchronously.
    // Give the FW ~50 ms to process before the next H2C since the H2C
    // queue depth is shallow; then drain anything that arrived.
    wait_c2h(mmio, 100, "macid_pause");

    host::print("  VIF: 5. role_maintain(CREATE, STATION)\n");
    h2c_role_maintain(mmio, macid, 0, 0);
    wait_c2h(mmio, 500, "role_maintain");

    host::print("  VIF: 6. join_info(dis_conn, NO_LINK)\n");
    h2c_join_info(mmio, macid, 0, 0);
    wait_c2h(mmio, 500, "join_info");

    host::print("  VIF: 7. addr_cam_upd(CREATE)\n");
    h2c_cam(mmio, macid, 0);
    wait_c2h(mmio, 500, "addr_cam");

    host::print("  VIF: 8. default_cmac_tbl\n");
    h2c_default_cmac_tbl(mmio, macid);
    wait_c2h(mmio, 500, "default_cmac_tbl");

    // Step 9 (h2c_default_dmac_tbl) — NULL for 8852B in rtw8852b.c:879,
    // rtw89_chip_h2c_default_dmac_tbl is a no-op.

    host::print("[wifi] VIF full init complete\n");
    true
}

/// Silence VIF-init wait lines in production builds. These H2Cs are
/// fire-and-forget per Linux (rack=1 only, dack=1 ack comes batched
/// behind the next scan H2C), so our simple HW_IDX-advance poll nearly
/// always logs "NO C2H after Xms" — misleading since the H2Cs did
/// succeed. Flip to `true` to re-enable the per-step timing.
const VERBOSE: bool = false;

fn wait_c2h(mmio: i32, max_ms: u32, tag: &str) {
    // R_AX_RXQ_RXBD_IDX = 0x1080 — use the same address as mac.rs
    const R_AX_RXQ_RXBD_IDX: u32 = 0x1080;
    let idx0 = host::mmio_r32(mmio, R_AX_RXQ_RXBD_IDX);
    let hw0 = (idx0 >> 16) & 0xFFFF;
    for ms in 0..max_ms {
        let idx = host::mmio_r32(mmio, R_AX_RXQ_RXBD_IDX);
        let hw = (idx >> 16) & 0xFFFF;
        if hw != hw0 {
            if !VERBOSE { return; }
            host::print("    [diag "); host::print(tag);
            host::print("] C2H after "); fw::print_dec(ms as usize);
            host::print("ms (hw "); fw::print_dec(hw0 as usize);
            host::print("→"); fw::print_dec(hw as usize); host::print(")\n");
            return;
        }
        host::sleep_ms(1);
    }
    if !VERBOSE { return; }
    host::print("    [diag "); host::print(tag);
    host::print("] NO C2H after "); fw::print_dec(max_ms as usize);
    host::print("ms\n");
}

// ═══════════════════════════════════════════════════════════════════
//  port_update — STA on port 0 in NO_LINK state
// ═══════════════════════════════════════════════════════════════════

fn port_update_p0_nolink(mmio: i32) {
    // rtw89_mac_port_cfg_func_sw (mac.c:4445):
    //   early returns if PORT_FUNC_EN is not already set; on first init it isn't, skip.

    // rtw89_mac_port_cfg_tx_rpt(false) / rx_rpt(false) — clear both in PORT_CFG
    host::mmio_clr32(mmio, R_AX_PORT_CFG_P0, B_AX_TXBCN_RPT_EN | B_AX_RXBCN_RPT_EN);

    // rtw89_mac_port_cfg_net_type — NET_TYPE = NO_LINK (0)
    host::mmio_w32_mask(mmio, R_AX_PORT_CFG_P0, B_AX_NET_TYPE_MASK, NET_TYPE_NO_LINK);

    // rtw89_mac_port_cfg_bcn_prct — NO_LINK → clear TBTT_PROHIB_EN | BRK_SETUP
    host::mmio_clr32(mmio, R_AX_PORT_CFG_P0, B_AX_TBTT_PROHIB_EN | B_AX_BRK_SETUP);

    // rtw89_mac_port_cfg_rx_sw — not INFRA/ADHOC → clear RX_BSSID_FIT_EN
    host::mmio_clr32(mmio, R_AX_PORT_CFG_P0, B_AX_RX_BSSID_FIT_EN);

    // rtw89_mac_port_cfg_rx_sync_by_nettype — NO_LINK → clear TSF_UDT_EN
    host::mmio_clr32(mmio, R_AX_PORT_CFG_P0, B_AX_TSF_UDT_EN);

    // rtw89_mac_port_cfg_tx_sw_by_nettype — not AP/ADHOC → clear BCNTX_EN
    host::mmio_clr32(mmio, R_AX_PORT_CFG_P0, B_AX_BCNTX_EN);

    // rtw89_mac_port_cfg_bcn_intv — BCN_SPACE_MASK = BCN_INTERVAL=100
    // bcn_space @ 0xC414, BCN_SPACE_MASK = GENMASK(15,0)
    host::mmio_w32_mask(mmio, R_AX_BCN_SPACE_CFG_P0, 0xFFFF, BCN_INTERVAL);

    // rtw89_mac_port_cfg_hiq_win — NO_LINK → win = 0 (8-bit write)
    // R_AX_P0MB_HGQ_WINDOW_CFG_0 = 0xC590. Low byte = win value.
    host::mmio_w32_mask(mmio, R_AX_P0MB_HGQ_WINDOW_CFG_0, 0xFF, 0);

    // rtw89_mac_port_cfg_hiq_dtim — set UPD_HGQMD|UPD_TIMIE in md_tsft, DTIM_NUM=0
    // md_tsft @ 0xCA08 — 8-bit set of bits 0,1
    host::mmio_set8(mmio, R_AX_MD_TSFT_STMP_CTL, 0x03);
    // dtim_ctrl @ 0xC426 (16-bit reg), DTIM_NUM_MASK = GENMASK(15,8) → upper byte
    // Access as 32-bit at 0xC424, DTIM is bits [31:24]
    host::mmio_w32_mask(mmio, 0xC424, 0xFF << 24, 0);

    // rtw89_mac_port_cfg_hiq_drop — clear bit(port) in PORT_DROP_4_0_MASK (GENMASK(20,16))
    //   and also bit 0 (for port 0)
    let drop = host::mmio_r32(mmio, R_AX_MBSSID_DROP_0);
    let drop_new = drop & !((1u32 << 16) | 1u32);
    host::mmio_w32(mmio, R_AX_MBSSID_DROP_0, drop_new);

    // rtw89_mac_port_cfg_bcn_setup_time — TBTT_SETUP_MASK = GENMASK(7,0) = BCN_SETUP_DEF=2
    host::mmio_w32_mask(mmio, R_AX_TBTT_PROHIB_P0, 0xFF, BCN_SETUP_DEF);

    // rtw89_mac_port_cfg_bcn_hold_time — TBTT_HOLD_MASK = GENMASK(27,16) = BCN_HOLD_DEF=200
    host::mmio_w32_mask(mmio, R_AX_TBTT_PROHIB_P0, 0xFFF << 16, BCN_HOLD_DEF);

    // rtw89_mac_port_cfg_bcn_mask_area — BCN_MSK_AREA_MASK = GENMASK(27,16) = 0
    host::mmio_w32_mask(mmio, R_AX_BCN_AREA_P0, 0xFFF << 16, 0);

    // rtw89_mac_port_cfg_tbtt_early — 16-bit at 0xC40E, TBTTERLY_MASK = GENMASK(11,0) = 5
    // 32-bit access at 0xC40C (BCNERLY) has TBTTERLY in bits [27:16] (shifted by 16)
    host::mmio_w32_mask(mmio, R_AX_BCNERLYINT_CFG_P0, 0xFFF << 16, TBTT_ERLY_DEF);

    // rtw89_mac_port_cfg_tbtt_agg — 16-bit at 0xC412, TBTT_AGG_NUM_MASK = GENMASK(15,8) = 1
    // 32-bit at 0xC410 would have it at [31:24]. Let me just use 0xC410 aligned.
    // Actually TBTT_AGG @ 0xC412 is in upper 16 of 0xC410. Upper-byte of that 16-bit = bits [31:24]
    host::mmio_w32_mask(mmio, 0xC410, 0xFF << 24, TBTT_AGG_DEF);

    // rtw89_mac_port_cfg_bss_color — port 0: BSS_COLOB_AX_PORT_0_MASK = GENMASK(5,0) = 0
    host::mmio_w32_mask(mmio, R_AX_PTCL_BSS_COLOR_0, 0x3F, 0);

    // rtw89_mac_port_cfg_mbssid — NO_LINK + port 0: clear P0MB_ALL_MASK = GENMASK(23,1)
    host::mmio_clr32(mmio, R_AX_MBSSID_CTRL, 0x00FF_FFFE);

    // rtw89_mac_port_cfg_func_en(true) — set PORT_FUNC_EN
    host::mmio_set32(mmio, R_AX_PORT_CFG_P0, B_AX_PORT_FUNC_EN);

    // rtw89_mac_port_tsf_resync_all — iterates all vifs; lone STA = no-op.

    // fsleep(BCN_ERLY_SET_DLY) = 20 μs; sleep_ms(1) is our minimum granularity.
    host::sleep_ms(1);

    // rtw89_mac_port_cfg_bcn_early — BCNERLY_MASK = GENMASK(11,0) = 160
    host::mmio_w32_mask(mmio, R_AX_BCNERLYINT_CFG_P0, 0xFFF, BCN_ERLY_DEF);

    // rtw89_mac_port_cfg_bcn_psr_rpt — BCAID_P0_MASK = GENMASK(10,0), bssid_index=0
    host::mmio_w32_mask(mmio, R_AX_BCN_PSR_RPT_P0, 0x7FF, 0);
}

// ═══════════════════════════════════════════════════════════════════
//  dmac_tbl_init + cmac_tbl_init (for macid X)
// ═══════════════════════════════════════════════════════════════════

fn dmac_tbl_init(mmio: i32, macid: u8) {
    // Linux mac.c:4291 — 4 iterations, writes 0 to each of 4 u32s.
    for i in 0..4u32 {
        let target = DMAC_TBL_BASE_ADDR + ((macid as u32) << 4) + (i << 2);
        host::mmio_w32(mmio, R_AX_FILTER_MODEL_ADDR, target);
        host::mmio_w32(mmio, R_AX_INDIR_ACCESS_ENTRY, 0);
    }
}

fn cmac_tbl_init(mmio: i32, macid: u8) {
    // Linux mac.c:4306 — sets target once, writes 8 u32 defaults.
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

// ═══════════════════════════════════════════════════════════════════
//  H2C: macid_pause — Linux fw.c:5088
// ═══════════════════════════════════════════════════════════════════

fn h2c_macid_pause(mmio: i32, macid: u8, pause: bool) {
    // struct rtw89_fw_macid_pause_grp: pause_grp[4] + mask_grp[4] = 32 bytes
    let mut payload = [0u8; 32];
    let grp = (macid >> 5) as usize;
    let sh = (macid & 0x1F) as u32;
    let set_bit: u32 = 1u32 << sh;
    // mask_grp[grp] @ offset 16 + grp*4
    let moff = 16 + grp * 4;
    payload[moff..moff + 4].copy_from_slice(&set_bit.to_le_bytes());
    if pause {
        // pause_grp[grp] @ offset 0 + grp*4
        let poff = grp * 4;
        payload[poff..poff + 4].copy_from_slice(&set_bit.to_le_bytes());
    }
    // CAT=1, CLASS=9, FUNC=0x8, rack=1, dack=0
    fw::h2c_send(mmio, 1, 9, 0x8, true, false, &payload);
}

// ═══════════════════════════════════════════════════════════════════
//  H2C: role_maintain — Linux fw.c:4857
// ═══════════════════════════════════════════════════════════════════

fn h2c_role_maintain(mmio: i32, macid: u8, port: u8, band: u8) {
    // struct rtw89_h2c_role_maintain: 1 × __le32 (4 bytes)
    //   w0:
    //     MACID    [7:0]
    //     SELF_ROLE[9:8]
    //     UPD_MODE [12:10]
    //     WIFI_ROLE[16:13]
    //     BAND     [18:17]
    //     PORT     [21:19]
    //     MACID_EXT[31:24]
    let w0: u32 = (macid as u32) & 0xFF
        | ((SELF_ROLE_CLIENT & 0x3) << 8)
        | ((UPD_MODE_CREATE & 0x7) << 10)
        | ((WIFI_ROLE_STATION & 0xF) << 13)
        | (((band as u32) & 0x3) << 17)
        | (((port as u32) & 0x7) << 19);
    let payload: [u8; 4] = w0.to_le_bytes();
    // CAT=1, CLASS=8 (MEDIA_RPT), FUNC=0x4, rack=0, dack=1
    fw::h2c_send(mmio, 1, 8, 0x4, false, true, &payload);
}

// ═══════════════════════════════════════════════════════════════════
//  H2C: join_info — Linux fw.c:4953
// ═══════════════════════════════════════════════════════════════════

fn h2c_join_info(mmio: i32, macid: u8, port: u8, band: u8) {
    // AX version: 1 × __le32. dis_conn=true, net_type=NO_LINK.
    //   w0:
    //     MACID    [7:0]
    //     OP       [8]          — 1 = dis_conn
    //     BAND     [9]
    //     WMM      [11:10]
    //     TGR      [12]
    //     ISHESTA  [13]
    //     DLBW     [15:14]
    //     TF_MAC_PAD[17:16]
    //     DL_T_PE  [20:18]
    //     PORT_ID  [23:21]
    //     NET_TYPE [25:24]
    //     WIFI_ROLE[29:26]
    //     SELF_ROLE[31:30]
    let w0: u32 = (macid as u32) & 0xFF
        | (1u32 << 8) // OP = dis_conn = true
        | (((band as u32) & 0x1) << 9)
        | (((port as u32) & 0x7) << 21)
        | ((NET_TYPE_NO_LINK & 0x3) << 24)
        | ((WIFI_ROLE_STATION & 0xF) << 26)
        | ((SELF_ROLE_CLIENT & 0x3) << 30);
    let payload: [u8; 4] = w0.to_le_bytes();
    // CAT=1, CLASS=8 (MEDIA_RPT), FUNC=0x0, rack=0, dack=1
    fw::h2c_send(mmio, 1, 8, 0x0, false, true, &payload);
}

// ═══════════════════════════════════════════════════════════════════
//  H2C: addr_cam_upd — Linux fw.c:2221
// ═══════════════════════════════════════════════════════════════════

fn h2c_cam(mmio: i32, macid: u8, port: u8) {
    // struct rtw89_h2c_addr_cam_v0: 15 × __le32 = 60 bytes (AX)
    let mut buf = [0u8; 60];

    // Addr CAM state (rtw89_cam_init_addr_cam defaults):
    //   addr_cam_idx = 0, offset = 0, len = 0x40 (ADDR_CAM_ENT_SIZE for 8852B),
    //   valid = 1, addr_mask = 0, mask_sel = NO_MSK (0),
    //   sec_ent_mode = NORMAL (2), sec_cam_map = 0
    // BSSID CAM state (rtw89_cam_init_bssid_cam):
    //   bssid_cam_idx = 0, phy_idx = 0, len = 0x08, offset = 0,
    //   valid = 1, bssid = 00:00:00:00:00:00

    let sma: [u8; 6] = sta_mac();
    let tma: [u8; 6] = [0; 6];
    let sma_hash = sma.iter().fold(0u8, |a, b| a ^ b);
    let tma_hash = tma.iter().fold(0u8, |a, b| a ^ b);

    // w0 — unused for init
    // w1: IDX=0 [7:0], OFFSET=0 [15:8], LEN=0x40 [23:16]
    let w1: u32 = ADDR_CAM_ENT_SIZE << 16;
    buf[4..8].copy_from_slice(&w1.to_le_bytes());

    // w2:
    //   VALID[0]=1 | NET_TYPE[2:1]=0 | BCN_HIT_COND[4:3]=0 | HIT_RULE[6:5]=0
    //   BB_SEL[7]=0 (phy_idx) | ADDR_MASK[13:8]=0 | MASK_SEL[15:14]=0
    //   SMA_HASH[23:16] | TMA_HASH[31:24]
    let w2: u32 = 1u32
        | ((NET_TYPE_NO_LINK & 0x3) << 1)
        | ((sma_hash as u32) << 16)
        | ((tma_hash as u32) << 24);
    buf[8..12].copy_from_slice(&w2.to_le_bytes());

    // w3: BSSID_CAM_IDX[5:0] = 0
    // (zero, no write needed)

    // w4: SMA[0..3]
    let w4: u32 = (sma[0] as u32)
        | ((sma[1] as u32) << 8)
        | ((sma[2] as u32) << 16)
        | ((sma[3] as u32) << 24);
    buf[16..20].copy_from_slice(&w4.to_le_bytes());

    // w5: SMA[4..5] | TMA[0..1]
    let w5: u32 = (sma[4] as u32)
        | ((sma[5] as u32) << 8)
        | ((tma[0] as u32) << 16)
        | ((tma[1] as u32) << 24);
    buf[20..24].copy_from_slice(&w5.to_le_bytes());

    // w6: TMA[2..5] — zero, skip

    // w7 — unused

    // w8 (v0 layout):
    //   MACID[7:0] | PORT_INT[10:8] | TSF_SYNC[13:11]
    //   TF_TRS[14] | LSIG_TXOP[15] | TGT_IND[26:24] | FRM_TGT_IND[29:27]
    let w8: u32 = (macid as u32) & 0xFF
        | (((port as u32) & 0x7) << 8)
        | (((port as u32) & 0x7) << 11);
    buf[32..36].copy_from_slice(&w8.to_le_bytes());

    // w9:
    //   AID12[11:0]=0 (not associated)
    //   SEC_ENT_MODE[17:16] = ADDR_CAM_SEC_NORMAL (2)
    let w9: u32 = (ADDR_CAM_SEC_NORMAL & 0x3) << 16;
    buf[36..40].copy_from_slice(&w9.to_le_bytes());

    // w10..w11 — sec entries, all 0

    // w12: BSSID_IDX[7:0]=0 | BSSID_OFFSET[15:8]=0 | BSSID_LEN[23:16]=0x08
    let w12: u32 = BSSID_CAM_ENT_SIZE << 16;
    buf[48..52].copy_from_slice(&w12.to_le_bytes());

    // w13:
    //   BSSID_VALID[0]=1 | BB_SEL[1]=0 | BSSID_MASK[7:2]=0x3F
    //   BSS_COLOR[13:8]=0 | BSSID[0][23:16]=0 | BSSID[1][31:24]=0
    let w13: u32 = 1u32 | (BSSID_MATCH_ALL << 2);
    buf[52..56].copy_from_slice(&w13.to_le_bytes());

    // w14: BSSID[2..5] — zero, skip

    // CAT=1, CLASS=6 (ADDR_CAM_UPDATE), FUNC=0x0, rack=0, dack=1
    fw::h2c_send(mmio, 1, 6, 0x0, false, true, &buf);
}

// ═══════════════════════════════════════════════════════════════════
//  Switch MACID 0 from NO_LINK → INFRA and lock onto a target BSSID.
//
//  Linux flow (mac80211.c:756 bss_info_changed BSSID path):
//    1. Copy AP BSSID into rtwvif_link->bssid
//    2. rtw89_cam_bssid_changed  (software state, refreshes hash + idx)
//    3. rtw89_fw_h2c_cam(INFO_CHANGE) — re-send full addr_cam with new BSSID
//    4. rtw89_fw_h2c_join_info(dis_conn=false) — FW marks macid as connecting
//    5. port_cfg updates (NET_TYPE=INFRA, TBTT_PROHIB_EN, BSSID_FIT_EN, TSF_UDT_EN)
//
//  Call once before AUTH/ASSOC CH8 TX to the target AP.
// ═══════════════════════════════════════════════════════════════════

pub fn switch_to_infra(mmio: i32, macid: u8, bssid: [u8; 6]) {
    host::print("\n[wifi] VIF → INFRA, BSSID ");
    for i in 0..6 {
        print_hex2(bssid[i]);
        if i < 5 { host::print(":"); }
    }
    host::print("\n");

    // 1. port_cfg: NET_TYPE=INFRA (2), enable TBTT_PROHIB_EN + BRK_SETUP
    //    (bcn_prct for net_type != NO_LINK), BSSID_FIT_EN (rx_sw INFRA),
    //    TSF_UDT_EN (rx_sync_by_nettype INFRA). BCNTX_EN stays clear (not AP).
    host::mmio_w32_mask(mmio, R_AX_PORT_CFG_P0, B_AX_NET_TYPE_MASK, NET_TYPE_INFRA);
    host::mmio_set32(mmio, R_AX_PORT_CFG_P0,
        B_AX_TBTT_PROHIB_EN | B_AX_BRK_SETUP
        | B_AX_RX_BSSID_FIT_EN | B_AX_TSF_UDT_EN);
    host::print("  port_cfg: NET_TYPE=INFRA, BSSID_FIT+TSF_UDT set\n");

    // 2. addr_cam update with target BSSID (UPD_MODE = INFO_CHANGE=3)
    h2c_cam_infra(mmio, macid, 0, bssid);
    wait_c2h(mmio, 500, "cam_infra");
    host::print("  addr_cam: BSSID programmed, UPD_MODE=INFO_CHANGE\n");

    // 3. join_info: dis_conn=false, NET_TYPE=INFRA
    h2c_join_info_infra(mmio, macid, 0, 0);
    wait_c2h(mmio, 500, "join_info_infra");
    host::print("  join_info: dis_conn=0, NET_TYPE=INFRA\n");
}

fn print_hex2(b: u8) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let a = [HEX[(b >> 4) as usize], HEX[(b & 0xF) as usize]];
    host::print(core::str::from_utf8(&a).unwrap_or("??"));
}

fn h2c_join_info_infra(mmio: i32, macid: u8, port: u8, band: u8) {
    // AX join_info: 1 × __le32. dis_conn=false, net_type=INFRA.
    let w0: u32 = (macid as u32) & 0xFF
        | (0u32 << 8)                         // OP = dis_conn = false
        | (((band as u32) & 0x1) << 9)
        | (((port as u32) & 0x7) << 21)
        | ((NET_TYPE_INFRA & 0x3) << 24)
        | ((WIFI_ROLE_STATION & 0xF) << 26)
        | ((SELF_ROLE_CLIENT & 0x3) << 30);
    let payload: [u8; 4] = w0.to_le_bytes();
    fw::h2c_send(mmio, 1, 8, 0x0, false, true, &payload);
}

fn h2c_cam_infra(mmio: i32, macid: u8, port: u8, bssid: [u8; 6]) {
    // Same 60-byte v0 layout as the CREATE path, but with target BSSID
    // filled in, NET_TYPE=INFRA, TMA=BSSID, UPD_MODE=INFO_CHANGE.
    let mut buf = [0u8; 60];

    let sma: [u8; 6] = sta_mac();
    let tma: [u8; 6] = bssid;       // for STA: TMA == target AP BSSID
    let sma_hash = sma.iter().fold(0u8, |a, b| a ^ b);
    let tma_hash = tma.iter().fold(0u8, |a, b| a ^ b);

    // w1: IDX=0 | OFFSET=0 | LEN=0x40
    let w1: u32 = ADDR_CAM_ENT_SIZE << 16;
    buf[4..8].copy_from_slice(&w1.to_le_bytes());

    // w2: VALID=1 | NET_TYPE=INFRA(2) | SMA_HASH | TMA_HASH
    let w2: u32 = 1u32
        | ((NET_TYPE_INFRA & 0x3) << 1)
        | ((sma_hash as u32) << 16)
        | ((tma_hash as u32) << 24);
    buf[8..12].copy_from_slice(&w2.to_le_bytes());

    // w4: SMA[0..3]
    let w4: u32 = (sma[0] as u32)
        | ((sma[1] as u32) << 8)
        | ((sma[2] as u32) << 16)
        | ((sma[3] as u32) << 24);
    buf[16..20].copy_from_slice(&w4.to_le_bytes());

    // w5: SMA[4..5] | TMA[0..1]
    let w5: u32 = (sma[4] as u32)
        | ((sma[5] as u32) << 8)
        | ((tma[0] as u32) << 16)
        | ((tma[1] as u32) << 24);
    buf[20..24].copy_from_slice(&w5.to_le_bytes());

    // w6: TMA[2..5]
    let w6: u32 = (tma[2] as u32)
        | ((tma[3] as u32) << 8)
        | ((tma[4] as u32) << 16)
        | ((tma[5] as u32) << 24);
    buf[24..28].copy_from_slice(&w6.to_le_bytes());

    // w8: MACID | PORT_INT | TSF_SYNC (same as CREATE)
    let w8: u32 = (macid as u32) & 0xFF
        | (((port as u32) & 0x7) << 8)
        | (((port as u32) & 0x7) << 11);
    buf[32..36].copy_from_slice(&w8.to_le_bytes());

    // w9: AID12=0 | SEC_ENT_MODE=NORMAL(2). AID stays 0 until assoc.
    let w9: u32 = (ADDR_CAM_SEC_NORMAL & 0x3) << 16;
    buf[36..40].copy_from_slice(&w9.to_le_bytes());

    // w12: BSSID_IDX=0 | BSSID_OFFSET=0 | BSSID_LEN=0x08
    let w12: u32 = BSSID_CAM_ENT_SIZE << 16;
    buf[48..52].copy_from_slice(&w12.to_le_bytes());

    // w13: BSSID_VALID=1 | BSSID_MASK=0x3F | BSSID[0..1] @ [23:16], [31:24]
    let w13: u32 = 1u32
        | (BSSID_MATCH_ALL << 2)
        | ((bssid[0] as u32) << 16)
        | ((bssid[1] as u32) << 24);
    buf[52..56].copy_from_slice(&w13.to_le_bytes());

    // w14: BSSID[2..5]
    let w14: u32 = (bssid[2] as u32)
        | ((bssid[3] as u32) << 8)
        | ((bssid[4] as u32) << 16)
        | ((bssid[5] as u32) << 24);
    buf[56..60].copy_from_slice(&w14.to_le_bytes());

    // Buffer is exactly 60 bytes — the v0 layout used for AX chips. w15
    // (UPD_MODE) only exists on the extended struct sent for v1+ chips
    // (Linux fw.c:2279 skips w15 when chip_gen == AX). INFO_CHANGE vs
    // CREATE is a Linux-side bookkeeping flag, not a wire field on AX.

    // CAT=1, CLASS=6 (ADDR_CAM_UPDATE), FUNC=0x0, rack=0, dack=1
    fw::h2c_send(mmio, 1, 6, 0x0, false, true, &buf);
}

// ═══════════════════════════════════════════════════════════════════
//  H2C: default_cmac_tbl — Linux fw.c:3521
// ═══════════════════════════════════════════════════════════════════

fn h2c_default_cmac_tbl(mmio: i32, macid: u8) {
    // H2C_CMC_TBL_LEN = 68 bytes (17 × u32). Linux fw.c:3549.
    //
    // Layout is a value/mask pair split across the 68 bytes:
    //   dwords 0..7  = field values (per-field bit positions)
    //   dwords 8..15 = field masks at the SAME bit positions as 0..7
    //   dword 16     = extra / pad
    //
    // The FW ORs the masked value bits into the chip's CCTL entry, leaving
    // unmasked bits at their previous value. A mask of 0 = "ignore value".
    //
    // Linux default for 8852B (rf_path_num=2, net_type!=AP) sets:
    //   dword 0: MACID[6:0] | OP[7]=1
    //   dword 5: TXPWR_MODE[11:9]=0                    → value 0
    //            mask in dword 13 @ GENMASK(11,9)      = 0x0000_0E00
    //   dword 6: NTX_PATH_EN[19:16]=RF_AB(3)           = 0x0003_0000
    //            PATH_MAP_A[21:20]=0
    //            PATH_MAP_B[23:22]=1 (RF_AB → 1)       = 0x0040_0000
    //            PATH_MAP_C[25:24]=0 | PATH_MAP_D[27:26]=0
    //            ANTSEL_A/B/C/D[28..31]=0
    //            → dword 6 = 0x0043_0000
    //            mask in dword 14 = bits [31:16] set   = 0xFFFF_0000
    //   dword 1: MGQ_RPT_EN[21]=0 (tx_rpt_enabled=false)
    //            mask in dword 9 @ BIT(21)             = 0x0020_0000
    //   dword 7: DOPPLER_CTRL[19:18]=0 | TXPWR_TOLERENCE[27:24]=0
    //            mask in dword 15 @ GENMASK(19,18) | GENMASK(27,24)
    //                                                  = 0x0F0C_0000
    //
    // Without NTX_PATH_EN in the mask the FW leaves MACID 0 without a TX
    // RF path assigned, and HW silently drops every CH8 direct TX
    // attempt (TX_COUNTER stays 0 even though DMA consumes the BD) —
    // that was the v1.30..v1.34 diagnostic pattern.

    let mut buf = [0u8; 68];

    // dword 0: MACID[6:0] | OP[7]=1
    let dw0: u32 = (macid as u32 & 0x7F) | (1u32 << 7);
    buf[0..4].copy_from_slice(&dw0.to_le_bytes());

    // dword 5: TXPWR_MODE=0  (value stays 0 — mask below is what matters)
    // buf[20..24] already zero

    // dword 6: NTX_PATH_EN=RF_AB(3) | PATH_MAP_B=1  → 0x0043_0000
    let dw6: u32 = (3u32 << 16) | (1u32 << 22);
    buf[24..28].copy_from_slice(&dw6.to_le_bytes());

    // dword 9: MGQ_RPT_EN mask @ BIT(21)
    let dw9: u32 = 1u32 << 21;
    buf[36..40].copy_from_slice(&dw9.to_le_bytes());

    // dword 13: TXPWR_MODE mask @ GENMASK(11,9)
    let dw13: u32 = 0x7u32 << 9;
    buf[52..56].copy_from_slice(&dw13.to_le_bytes());

    // dword 14: NTX_PATH_EN | PATH_MAP_A..D | ANTSEL_A..D masks @ [31:16]
    let dw14: u32 = 0xFFFFu32 << 16;
    buf[56..60].copy_from_slice(&dw14.to_le_bytes());

    // dword 15: DOPPLER_CTRL mask [19:18] | TXPWR_TOLERENCE mask [27:24]
    let dw15: u32 = (0x3u32 << 18) | (0xFu32 << 24);
    buf[60..64].copy_from_slice(&dw15.to_le_bytes());

    // dword 7, 16 remain zero.

    // CAT=1, CLASS=5 (FR_EXCHG), FUNC=0x2 (CCTLINFO_UD for 8852b), rack=0, dack=1
    fw::h2c_send(mmio, 1, 5, 0x2, false, true, &buf);
}
