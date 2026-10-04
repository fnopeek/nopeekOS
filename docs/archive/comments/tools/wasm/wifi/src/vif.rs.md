# `tools/wasm/wifi/src/vif.rs` @ 5e0102684

## L1-14 · `use crate::host;`

```
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
```

## L19-22 · `pub static mut STA_MAC: [u8; 6] = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55];`

```
/// Station MAC address. Initially a pseudo value; lib.rs overrides this
/// with the real chip MAC read from efuse as soon as efuse::read succeeds.
/// Probe Requests and the VIF addr_cam use whatever is stored here at the
/// point of the call.
```

## L29 · `const NET_TYPE_NO_LINK: u32    = 0;`

```
// ── rtw89 enum values (core.h) ────────────────────────────────────
```

## L35 · `const BSSID_MATCH_ALL: u32     = 0x3F;         // GENMASK(5,0)`

```
// GENMASK(5,0)
```

## L37 · `const ADDR_CAM_ENT_SIZE: u32  = 0x40;`

```
// CAM entry sizes (mac.h)
```

## L41 · `const R_AX_FILTER_MODEL_ADDR: u32  = 0x0C04;`

```
// MACID table base addresses (mac.h:309/315)
```

## L48 · `const R_AX_PORT_CFG_P0: u32        = 0xC400;`

```
// Port 0 register addresses (reg.h) — rtw89_port_base_ax (mac.c:4347)
```

## L65 · `const B_AX_BRK_SETUP: u32       = 1 << 16;`

```
// Port cfg bit masks (reg.h)
```

## L77 · `const BCN_INTERVAL: u32 = 100;`

```
// Defaults from mac.c:4435
```

## L85-87 · `pub fn init(mmio: i32, macid: u8) -> bool {`

```
// ═══════════════════════════════════════════════════════════════════
//  Main entry — full VIF init for MACID 0, port 0, band 0
// ═══════════════════════════════════════════════════════════════════
```

## L94-110 · `host::print("  VIF: 1. port_update(p0 NO_LINK)\n");`

```
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
```

## L123-125 · `wait_c2h(mmio, 100, "macid_pause");`

```
// Linux sends with rack=1, dack=0 — REC_ACK arrives asynchronously.
// Give the FW ~50 ms to process before the next H2C since the H2C
// queue depth is shallow; then drain anything that arrived.
```

## L144-145 · `host::print("[wifi] VIF full init complete\n");`

```
// Step 9 (h2c_default_dmac_tbl) — NULL for 8852B in rtw8852b.c:879,
// rtw89_chip_h2c_default_dmac_tbl is a no-op.
```

## L151-155 · `const VERBOSE: bool = false;`

```
/// Silence VIF-init wait lines in production builds. These H2Cs are
/// fire-and-forget per Linux (rack=1 only, dack=1 ack comes batched
/// behind the next scan H2C), so our simple HW_IDX-advance poll nearly
/// always logs "NO C2H after Xms" — misleading since the H2Cs did
/// succeed. Flip to `true` to re-enable the per-step timing.
```

## L159 · `const R_AX_RXQ_RXBD_IDX: u32 = 0x1080;`

```
// R_AX_RXQ_RXBD_IDX = 0x1080 — use the same address as mac.rs
```

## L182-184 · `fn port_update_p0_nolink(mmio: i32) {`

```
// ═══════════════════════════════════════════════════════════════════
//  port_update — STA on port 0 in NO_LINK state
// ═══════════════════════════════════════════════════════════════════
```

## L187-188 · `host::mmio_clr32(mmio, R_AX_PORT_CFG_P0, B_AX_TXBCN_RPT_EN | B_AX_RXBCN_RPT_EN);`

```
// rtw89_mac_port_cfg_func_sw (mac.c:4445):
//   early returns if PORT_FUNC_EN is not already set; on first init it isn't, skip.
```

## L190 · `host::mmio_clr32(mmio, R_AX_PORT_CFG_P0, B_AX_TXBCN_RPT_EN | B_AX_RXBCN_RPT_EN);`

```
// rtw89_mac_port_cfg_tx_rpt(false) / rx_rpt(false) — clear both in PORT_CFG
```

## L193 · `host::mmio_w32_mask(mmio, R_AX_PORT_CFG_P0, B_AX_NET_TYPE_MASK, NET_TYPE_NO_LINK);`

```
// rtw89_mac_port_cfg_net_type — NET_TYPE = NO_LINK (0)
```

## L196 · `host::mmio_clr32(mmio, R_AX_PORT_CFG_P0, B_AX_TBTT_PROHIB_EN | B_AX_BRK_SETUP);`

```
// rtw89_mac_port_cfg_bcn_prct — NO_LINK → clear TBTT_PROHIB_EN | BRK_SETUP
```

## L199 · `host::mmio_clr32(mmio, R_AX_PORT_CFG_P0, B_AX_RX_BSSID_FIT_EN);`

```
// rtw89_mac_port_cfg_rx_sw — not INFRA/ADHOC → clear RX_BSSID_FIT_EN
```

## L202 · `host::mmio_clr32(mmio, R_AX_PORT_CFG_P0, B_AX_TSF_UDT_EN);`

```
// rtw89_mac_port_cfg_rx_sync_by_nettype — NO_LINK → clear TSF_UDT_EN
```

## L205 · `host::mmio_clr32(mmio, R_AX_PORT_CFG_P0, B_AX_BCNTX_EN);`

```
// rtw89_mac_port_cfg_tx_sw_by_nettype — not AP/ADHOC → clear BCNTX_EN
```

## L208-209 · `host::mmio_w32_mask(mmio, R_AX_BCN_SPACE_CFG_P0, 0xFFFF, BCN_INTERVAL);`

```
// rtw89_mac_port_cfg_bcn_intv — BCN_SPACE_MASK = BCN_INTERVAL=100
// bcn_space @ 0xC414, BCN_SPACE_MASK = GENMASK(15,0)
```

## L212-213 · `host::mmio_w32_mask(mmio, R_AX_P0MB_HGQ_WINDOW_CFG_0, 0xFF, 0);`

```
// rtw89_mac_port_cfg_hiq_win — NO_LINK → win = 0 (8-bit write)
// R_AX_P0MB_HGQ_WINDOW_CFG_0 = 0xC590. Low byte = win value.
```

## L216-217 · `host::mmio_set8(mmio, R_AX_MD_TSFT_STMP_CTL, 0x03);`

```
// rtw89_mac_port_cfg_hiq_dtim — set UPD_HGQMD|UPD_TIMIE in md_tsft, DTIM_NUM=0
// md_tsft @ 0xCA08 — 8-bit set of bits 0,1
```

## L219-220 · `host::mmio_w32_mask(mmio, 0xC424, 0xFF << 24, 0);`

```
// dtim_ctrl @ 0xC426 (16-bit reg), DTIM_NUM_MASK = GENMASK(15,8) → upper byte
// Access as 32-bit at 0xC424, DTIM is bits [31:24]
```

## L223-224 · `let drop = host::mmio_r32(mmio, R_AX_MBSSID_DROP_0);`

```
// rtw89_mac_port_cfg_hiq_drop — clear bit(port) in PORT_DROP_4_0_MASK (GENMASK(20,16))
//   and also bit 0 (for port 0)
```

## L229 · `host::mmio_w32_mask(mmio, R_AX_TBTT_PROHIB_P0, 0xFF, BCN_SETUP_DEF);`

```
// rtw89_mac_port_cfg_bcn_setup_time — TBTT_SETUP_MASK = GENMASK(7,0) = BCN_SETUP_DEF=2
```

## L232 · `host::mmio_w32_mask(mmio, R_AX_TBTT_PROHIB_P0, 0xFFF << 16, BCN_HOLD_DEF);`

```
// rtw89_mac_port_cfg_bcn_hold_time — TBTT_HOLD_MASK = GENMASK(27,16) = BCN_HOLD_DEF=200
```

## L235 · `host::mmio_w32_mask(mmio, R_AX_BCN_AREA_P0, 0xFFF << 16, 0);`

```
// rtw89_mac_port_cfg_bcn_mask_area — BCN_MSK_AREA_MASK = GENMASK(27,16) = 0
```

## L238-239 · `host::mmio_w32_mask(mmio, R_AX_BCNERLYINT_CFG_P0, 0xFFF << 16, TBTT_ERLY_DEF);`

```
// rtw89_mac_port_cfg_tbtt_early — 16-bit at 0xC40E, TBTTERLY_MASK = GENMASK(11,0) = 5
// 32-bit access at 0xC40C (BCNERLY) has TBTTERLY in bits [27:16] (shifted by 16)
```

## L242-244 · `host::mmio_w32_mask(mmio, 0xC410, 0xFF << 24, TBTT_AGG_DEF);`

```
// rtw89_mac_port_cfg_tbtt_agg — 16-bit at 0xC412, TBTT_AGG_NUM_MASK = GENMASK(15,8) = 1
// 32-bit at 0xC410 would have it at [31:24]. Let me just use 0xC410 aligned.
// Actually TBTT_AGG @ 0xC412 is in upper 16 of 0xC410. Upper-byte of that 16-bit = bits [31:24]
```

## L247 · `host::mmio_w32_mask(mmio, R_AX_PTCL_BSS_COLOR_0, 0x3F, 0);`

```
// rtw89_mac_port_cfg_bss_color — port 0: BSS_COLOB_AX_PORT_0_MASK = GENMASK(5,0) = 0
```

## L250 · `host::mmio_clr32(mmio, R_AX_MBSSID_CTRL, 0x00FF_FFFE);`

```
// rtw89_mac_port_cfg_mbssid — NO_LINK + port 0: clear P0MB_ALL_MASK = GENMASK(23,1)
```

## L253 · `host::mmio_set32(mmio, R_AX_PORT_CFG_P0, B_AX_PORT_FUNC_EN);`

```
// rtw89_mac_port_cfg_func_en(true) — set PORT_FUNC_EN
```

## L256 · `host::sleep_ms(1);`

```
// rtw89_mac_port_tsf_resync_all — iterates all vifs; lone STA = no-op.
```

## L258 · `host::sleep_ms(1);`

```
// fsleep(BCN_ERLY_SET_DLY) = 20 μs; sleep_ms(1) is our minimum granularity.
```

## L261 · `host::mmio_w32_mask(mmio, R_AX_BCNERLYINT_CFG_P0, 0xFFF, BCN_ERLY_DEF);`

```
// rtw89_mac_port_cfg_bcn_early — BCNERLY_MASK = GENMASK(11,0) = 160
```

## L264 · `host::mmio_w32_mask(mmio, R_AX_BCN_PSR_RPT_P0, 0x7FF, 0);`

```
// rtw89_mac_port_cfg_bcn_psr_rpt — BCAID_P0_MASK = GENMASK(10,0), bssid_index=0
```

## L268-270 · `fn dmac_tbl_init(mmio: i32, macid: u8) {`

```
// ═══════════════════════════════════════════════════════════════════
//  dmac_tbl_init + cmac_tbl_init (for macid X)
// ═══════════════════════════════════════════════════════════════════
```

## L273 · `for i in 0..4u32 {`

```
// Linux mac.c:4291 — 4 iterations, writes 0 to each of 4 u32s.
```

## L282 · `let target = CMAC_TBL_BASE_ADDR + (macid as u32) * CCTL_INFO_SIZE;`

```
// Linux mac.c:4306 — sets target once, writes 8 u32 defaults.
```

## L295-297 · `fn h2c_macid_pause(mmio: i32, macid: u8, pause: bool) {`

```
// ═══════════════════════════════════════════════════════════════════
//  H2C: macid_pause — Linux fw.c:5088
// ═══════════════════════════════════════════════════════════════════
```

## L300 · `let mut payload = [0u8; 32];`

```
// struct rtw89_fw_macid_pause_grp: pause_grp[4] + mask_grp[4] = 32 bytes
```

## L305 · `let moff = 16 + grp * 4;`

```
// mask_grp[grp] @ offset 16 + grp*4
```

## L309 · `let poff = grp * 4;`

```
// pause_grp[grp] @ offset 0 + grp*4
```

## L313 · `fw::h2c_send(mmio, 1, 9, 0x8, true, false, &payload);`

```
// CAT=1, CLASS=9, FUNC=0x8, rack=1, dack=0
```

## L317-319 · `fn h2c_role_maintain(mmio: i32, macid: u8, port: u8, band: u8) {`

```
// ═══════════════════════════════════════════════════════════════════
//  H2C: role_maintain — Linux fw.c:4857
// ═══════════════════════════════════════════════════════════════════
```

## L322-330 · `let w0: u32 = (macid as u32) & 0xFF`

```
// struct rtw89_h2c_role_maintain: 1 × __le32 (4 bytes)
//   w0:
//     MACID    [7:0]
//     SELF_ROLE[9:8]
//     UPD_MODE [12:10]
//     WIFI_ROLE[16:13]
//     BAND     [18:17]
//     PORT     [21:19]
//     MACID_EXT[31:24]
```

## L338 · `fw::h2c_send(mmio, 1, 8, 0x4, false, true, &payload);`

```
// CAT=1, CLASS=8 (MEDIA_RPT), FUNC=0x4, rack=0, dack=1
```

## L342-344 · `fn h2c_join_info(mmio: i32, macid: u8, port: u8, band: u8) {`

```
// ═══════════════════════════════════════════════════════════════════
//  H2C: join_info — Linux fw.c:4953
// ═══════════════════════════════════════════════════════════════════
```

## L347-361 · `let w0: u32 = (macid as u32) & 0xFF`

```
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
```

## L363 · `| (1u32 << 8) // OP = dis_conn = true`

```
// OP = dis_conn = true
```

## L370 · `fw::h2c_send(mmio, 1, 8, 0x0, false, true, &payload);`

```
// CAT=1, CLASS=8 (MEDIA_RPT), FUNC=0x0, rack=0, dack=1
```

## L374-376 · `fn h2c_cam(mmio: i32, macid: u8, port: u8) {`

```
// ═══════════════════════════════════════════════════════════════════
//  H2C: addr_cam_upd — Linux fw.c:2221
// ═══════════════════════════════════════════════════════════════════
```

## L379 · `let mut buf = [0u8; 60];`

```
// struct rtw89_h2c_addr_cam_v0: 15 × __le32 = 60 bytes (AX)
```

## L382-388 · `let sma: [u8; 6] = sta_mac();`

```
// Addr CAM state (rtw89_cam_init_addr_cam defaults):
//   addr_cam_idx = 0, offset = 0, len = 0x40 (ADDR_CAM_ENT_SIZE for 8852B),
//   valid = 1, addr_mask = 0, mask_sel = NO_MSK (0),
//   sec_ent_mode = NORMAL (2), sec_cam_map = 0
// BSSID CAM state (rtw89_cam_init_bssid_cam):
//   bssid_cam_idx = 0, phy_idx = 0, len = 0x08, offset = 0,
//   valid = 1, bssid = 00:00:00:00:00:00
```

## L395-396 · `let w1: u32 = ADDR_CAM_ENT_SIZE << 16;`

```
// w0 — unused for init
// w1: IDX=0 [7:0], OFFSET=0 [15:8], LEN=0x40 [23:16]
```

## L400-403 · `let w2: u32 = 1u32`

```
// w2:
//   VALID[0]=1 | NET_TYPE[2:1]=0 | BCN_HIT_COND[4:3]=0 | HIT_RULE[6:5]=0
//   BB_SEL[7]=0 (phy_idx) | ADDR_MASK[13:8]=0 | MASK_SEL[15:14]=0
//   SMA_HASH[23:16] | TMA_HASH[31:24]
```

## L410-411 · `let w4: u32 = (sma[0] as u32)`

```
// w3: BSSID_CAM_IDX[5:0] = 0
// (zero, no write needed)
```

## L413 · `let w4: u32 = (sma[0] as u32)`

```
// w4: SMA[0..3]
```

## L420 · `let w5: u32 = (sma[4] as u32)`

```
// w5: SMA[4..5] | TMA[0..1]
```

## L427 · `let w8: u32 = (macid as u32) & 0xFF`

```
// w6: TMA[2..5] — zero, skip
```

## L429 · `let w8: u32 = (macid as u32) & 0xFF`

```
// w7 — unused
```

## L431-433 · `let w8: u32 = (macid as u32) & 0xFF`

```
// w8 (v0 layout):
//   MACID[7:0] | PORT_INT[10:8] | TSF_SYNC[13:11]
//   TF_TRS[14] | LSIG_TXOP[15] | TGT_IND[26:24] | FRM_TGT_IND[29:27]
```

## L439-441 · `let w9: u32 = (ADDR_CAM_SEC_NORMAL & 0x3) << 16;`

```
// w9:
//   AID12[11:0]=0 (not associated)
//   SEC_ENT_MODE[17:16] = ADDR_CAM_SEC_NORMAL (2)
```

## L445 · `let w12: u32 = BSSID_CAM_ENT_SIZE << 16;`

```
// w10..w11 — sec entries, all 0
```

## L447 · `let w12: u32 = BSSID_CAM_ENT_SIZE << 16;`

```
// w12: BSSID_IDX[7:0]=0 | BSSID_OFFSET[15:8]=0 | BSSID_LEN[23:16]=0x08
```

## L451-453 · `let w13: u32 = 1u32 | (BSSID_MATCH_ALL << 2);`

```
// w13:
//   BSSID_VALID[0]=1 | BB_SEL[1]=0 | BSSID_MASK[7:2]=0x3F
//   BSS_COLOR[13:8]=0 | BSSID[0][23:16]=0 | BSSID[1][31:24]=0
```

## L457 · `fw::h2c_send(mmio, 1, 6, 0x0, false, true, &buf);`

```
// w14: BSSID[2..5] — zero, skip
```

## L459 · `fw::h2c_send(mmio, 1, 6, 0x0, false, true, &buf);`

```
// CAT=1, CLASS=6 (ADDR_CAM_UPDATE), FUNC=0x0, rack=0, dack=1
```

## L463-476 · `pub fn set_target_bssid(mmio: i32, macid: u8, bssid: [u8; 6]) {`

```
// ═══════════════════════════════════════════════════════════════════
//  Lock onto a target BSSID for AUTH/ASSOC frame exchange.
//
//  Linux only writes BSSID + re-sends addr_cam on BSS_CHANGED_BSSID
//  (mac80211.c:756). NET_TYPE stays NO_LINK until BSS_CHANGED_ASSOC fires
//  after association completes. Setting NET_TYPE=INFRA prematurely (as
//  an earlier version did) makes the FW consider the MACID "connected"
//  before AUTH even starts — some FW paths guard TX behind the
//  association state machine and silently drop pre-AUTH frames.
//
//  Here we only refresh the addr_cam entry with the target BSSID so that
//  an incoming AUTH Response (addr1 = our MAC, addr3 = target BSSID)
//  matches our BSSID CAM and gets delivered to host.
// ═══════════════════════════════════════════════════════════════════
```

## L498-503 · `let mut buf = [0u8; 60];`

```
// 60-byte v0 layout — Linux `rtw89_cam_fill_addr_cam_info` +
// `rtw89_cam_fill_bssid_cam_info`. For a STA about to AUTH but
// NOT yet associated, Linux keeps net_type = NO_LINK (see
// core.c:4992 rtw89_vif_type_mapping — only flips to INFRA when
// assoc=true). TMA becomes the target AP's BSSID so any response
// frame matches the CAM.
```

## L507 · `let tma: [u8; 6] = bssid;       // for STA: TMA == target AP BSSID`

```
// for STA: TMA == target AP BSSID
```

## L511 · `let w1: u32 = ADDR_CAM_ENT_SIZE << 16;`

```
// w1: IDX=0 | OFFSET=0 | LEN=0x40
```

## L515-516 · `let w2: u32 = 1u32`

```
// w2: VALID=1 | NET_TYPE=NO_LINK(0, stays NO_LINK until assoc)
//     | SMA_HASH | TMA_HASH
```

## L523 · `let w4: u32 = (sma[0] as u32)`

```
// w4: SMA[0..3]
```

## L530 · `let w5: u32 = (sma[4] as u32)`

```
// w5: SMA[4..5] | TMA[0..1]
```

## L537 · `let w6: u32 = (tma[2] as u32)`

```
// w6: TMA[2..5]
```

## L544 · `let w8: u32 = (macid as u32) & 0xFF`

```
// w8: MACID | PORT_INT | TSF_SYNC (same as CREATE)
```

## L550 · `let w9: u32 = (ADDR_CAM_SEC_NORMAL & 0x3) << 16;`

```
// w9: AID12=0 | SEC_ENT_MODE=NORMAL(2). AID stays 0 until assoc.
```

## L554 · `let w12: u32 = BSSID_CAM_ENT_SIZE << 16;`

```
// w12: BSSID_IDX=0 | BSSID_OFFSET=0 | BSSID_LEN=0x08
```

## L558 · `let w13: u32 = 1u32`

```
// w13: BSSID_VALID=1 | BSSID_MASK=0x3F | BSSID[0..1] @ [23:16], [31:24]
```

## L565 · `let w14: u32 = (bssid[2] as u32)`

```
// w14: BSSID[2..5]
```

## L572-575 · `fw::h2c_send(mmio, 1, 6, 0x0, false, true, &buf);`

```
// Buffer is exactly 60 bytes — the v0 layout used for AX chips. w15
// (UPD_MODE) only exists on the extended struct sent for v1+ chips
// (Linux fw.c:2279 skips w15 when chip_gen == AX). INFO_CHANGE vs
// CREATE is a Linux-side bookkeeping flag, not a wire field on AX.
```

## L577 · `fw::h2c_send(mmio, 1, 6, 0x0, false, true, &buf);`

```
// CAT=1, CLASS=6 (ADDR_CAM_UPDATE), FUNC=0x0, rack=0, dack=1
```

## L581-583 · `fn h2c_default_cmac_tbl(mmio: i32, macid: u8) {`

```
// ═══════════════════════════════════════════════════════════════════
//  H2C: default_cmac_tbl — Linux fw.c:3521
// ═══════════════════════════════════════════════════════════════════
```

## L586-616 · `let mut buf = [0u8; 68];`

```
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
```

## L620 · `let dw0: u32 = (macid as u32 & 0x7F) | (1u32 << 7);`

```
// dword 0: MACID[6:0] | OP[7]=1
```

## L624-625 · `let dw6: u32 = (3u32 << 16) | (1u32 << 22);`

```
// dword 5: TXPWR_MODE=0  (value stays 0 — mask below is what matters)
// buf[20..24] already zero
```

## L627 · `let dw6: u32 = (3u32 << 16) | (1u32 << 22);`

```
// dword 6: NTX_PATH_EN=RF_AB(3) | PATH_MAP_B=1  → 0x0043_0000
```

## L631 · `let dw9: u32 = 1u32 << 21;`

```
// dword 9: MGQ_RPT_EN mask @ BIT(21)
```

## L635 · `let dw13: u32 = 0x7u32 << 9;`

```
// dword 13: TXPWR_MODE mask @ GENMASK(11,9)
```

## L639 · `let dw14: u32 = 0xFFFFu32 << 16;`

```
// dword 14: NTX_PATH_EN | PATH_MAP_A..D | ANTSEL_A..D masks @ [31:16]
```

## L643 · `let dw15: u32 = (0x3u32 << 18) | (0xFu32 << 24);`

```
// dword 15: DOPPLER_CTRL mask [19:18] | TXPWR_TOLERENCE mask [27:24]
```

## L647 · `fw::h2c_send(mmio, 1, 5, 0x2, false, true, &buf);`

```
// dword 7, 16 remain zero.
```

## L649 · `fw::h2c_send(mmio, 1, 5, 0x2, false, true, &buf);`

```
// CAT=1, CLASS=5 (FR_EXCHG), FUNC=0x2 (CCTLINFO_UD for 8852b), rack=0, dack=1
```

