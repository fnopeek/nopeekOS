# `tools/wasm/wifi/src/lib.rs` @ 5e0102684

## L1-4 · `#![no_std]`

```
//! wifi — RTL8852BE WiFi 6 driver (WASM module)
//!
//! Phase 1: Chip probe — bind PCI device, map BAR0, read chip registers.
//! Uses the nopeekOS WASM Driver ABI (npk_pci_*, npk_mmio_*, npk_dma_*).
```

## L43 · `static mut MMIO: i32 = -1;`

```
/// MMIO handle for BAR0 (set during init, used everywhere)
```

## L46-47 · `static mut EFUSE: efuse::EfuseData = efuse::EfuseData::empty();`

```
/// Parsed efuse data — filled after efuse::read in init, consumed by
/// TSSI, set_txpwr, and the station MAC for Probe Requests.
```

## L54 · `let rc = host::pci_bind(regs::RTL8852B_VENDOR, regs::RTL8852B_DEVICE);`

```
// ── Step 1: Bind PCI device ──────────────────────────────────
```

## L68-71 · `let cmd = host::pci_read_config(0x04);`

```
// ── Step 2: Size BAR2 before enabling memory ─────────────────
// Linux trusts the kernel (pci_resource_len). We don't have that luxury,
// so size it ourselves via the 0xFFFFFFFF-write trick. Memory bit must
// be off during sizing to avoid stray accesses with garbage BAR.
```

## L84-87 · `host::pci_enable_bus_master();`

```
// ── Step 3: Enable bus master + memory space ──────────────────
// NO FLR! FLR resets the Digital Die but NOT the Analog Die,
// desynchronizing the XTAL SI interface between them.
// Instead we use soft MAC reset (pwr_off → pwr_on) in fw::download().
```

## L90-93 · `let requested = if bar2_pages >= 128 { 128 } else { bar2_pages };`

```
// ── Step 4: Map BAR2 (MMIO registers) ─────────────────────────
// RTL8852BE: BAR0=I/O, BAR2=MMIO (Linux rtw89: bar_id=2).
// Map the actual BAR size — R_AX_INDIR_ACCESS_ENTRY at 0x40000 needs
// at least 128 pages for dmac_tbl_init / cmac_tbl_init (mac.c:4291).
```

## L105 · `host::print("  RTL8852BE Register Dump\n");`

```
// ── Step 4: Chip probe — read key registers ──────────────────
```

## L109 · `let subsys = host::pci_read_config(0x2C);`

```
// PCI config space
```

## L116 · `let sys_iso = host::mmio_r32(mmio, regs::R_AX_SYS_ISO_CTRL);`

```
// System registers
```

## L135 · `let fw_ctrl = host::mmio_r32(mmio, regs::R_AX_WCPU_FW_CTRL);`

```
// Firmware status
```

## L145 · `let hci_func = host::mmio_r32(mmio, regs::R_AX_HCI_FUNC_EN);`

```
// HCI / DMA status
```

## L155 · `let halt_h2c = host::mmio_r32(mmio, regs::R_AX_HALT_H2C_CTRL);`

```
// Halt channels
```

## L164-165 · `let sys_status = host::mmio_r32(mmio, regs::R_AX_SYS_STATUS1);`

```
// ── Interpret firmware status ────────────────────────────────
// Real FW ready is in SYS_STATUS1 bit 0, NOT WCPU_FW_CTRL bit 0!
```

## L177 · `host::print("[wifi] Starting firmware download...\n");`

```
// ── Phase 2: Firmware download ─────────────────────────────────
```

## L187 · `let fw_ctrl = host::mmio_r32(mmio, regs::R_AX_WCPU_FW_CTRL);`

```
// ── Phase 3: Post-FWDL status (brief) ─────────────────────────
```

## L192 · `if !mac::init(mmio) {`

```
// ── Phase 4: MAC init ──────────────────────────────────────────
```

## L198-201 · `chan::apply_txpwr_ctrl(mmio);`

```
// ── Gap 3.7.17: set_txpwr_ctrl — PA reference init.
// Linux phy_dm_init calls chip->ops->set_txpwr_ctrl once to set the
// OFDM/CCK power reference for both RF paths. Without this the
// per-rate power table has no anchor and TX power is undefined.
```

## L205-209 · `let efuse_data = efuse::read(mmio);`

```
// ── Efuse read — chip-specific calibration data.
// Done after FW is up and MAC/PHY init is complete so SYS_ISO_CTRL
// is in a known state. Data goes into EFUSE (static), consumed by
// TSSI (thermal, tssi_cck/mcs), set_txpwr (gain offsets), and for
// chip MAC address instead of our pseudo 00:11:22:33:44:55.
```

## L213-215 · `if efuse_data.autoload_valid && efuse_data.mac_addr != [0; 6]`

```
// Propagate efuse MAC into vif::STA_MAC so Probe Requests and the
// VIF addr_cam use the real chip MAC (not the pseudo 00:11:22:...).
// Only overwrite when autoload_valid — otherwise keep the pseudo.
```

## L221-226 · `pwr_trim::run(mmio, &efuse_data);`

```
// ── Gap 3.7.18: power_trim — applies per-chip thermal + PA bias.
// In rtw89_phy_dm_init Linux calls set_txpwr_ctrl then power_trim
// then cfg_txrx_path. Without PA-bias trim, RR_BIASA.TXG/TXA stay
// at HW defaults — on this NUC efuse has thermal=0xf1 programmed,
// which makes it very likely pa_bias_trim is also programmed and
// the chip ships expecting those values to be applied before TX.
```

## L229-236 · `chan::bb_cfg_txrx_path(mmio);`

```
// ── bb_cfg_txrx_path — 1:1 __rtw8852bx_bb_cfg_txrx_path.
//   Linux calls this as the LAST step of rtw89_phy_dm_init. It sets
//   the TX-routing pattern in R_P0_RFMODE / R_P1_RFMODE bits[31:4]
//   to 0x1233312 — without this the chip doesn't know which RF path
//   to send TX through, so every TX silently dies at the BB→PA step.
//   That matches our v1.44 sniffer result (0 frames on-air).
//   Our earlier inline setup only wrote 0x333 to the wrong mask
//   ([23:12] instead of [11:0]) and never touched bits [31:4].
```

## L239-252 · `btc::init(mmio);`

```
// ── Phase 4a: BT-Coex init — CRITICAL for shared-antenna 8852BE.
//   Linux core.c:5961 calls rtw89_btc_ntfy_init(BTC_MODE_NORMAL)
//   right after phy_init_rf_reg. This runs rtw89_mac_coex_init +
//   __rtw8852bx_btc_init_cfg which sets:
//     R_AX_BTC_FUNC_EN.B_AX_PTA_WL_TX_EN = 1   ← THE bit that
//   lets the PTA (Packet Traffic Arbiter) route WiFi mgmt/data TX
//   to the shared WL+BT antenna. Out of reset it is 0, so TX dies
//   silently in the PTA before reaching the PA — which is exactly
//   what the v1.44 sniffer test showed (0 frames on-air despite
//   BUSY/IDX/TX_COUNTER all toggling).
//
//   Also programs WL priorities (TX_RESP+BEACON high-pri), RF GNT
//   debug off, SHARED-antenna TRX masks per path, PTA break table,
//   BT counter enable.
```

## L255-260 · `mac::hci_start(mmio);`

```
// ── Phase 4b: hci_start — 1:1 Linux rtw89_hci_start (core.c:5970).
//   Unmask PCIe IRQs (HIMR0 + HIMR00 + HIMR10). On 8852BE this is
//   the last step of rtw89_core_start and enables the RX-DMA event
//   path. We poll instead of using IRQs, but the IMRs still gate
//   DMA progress on some AX chips — without them HW_IDX may stay
//   parked after the first C2H frame.
```

## L263-269 · `dpk::init(mmio);`

```
// ── Phase 5: Baseline channel tune on ch 1 + RFK.
//   set_channel + rx_dck + iqk is needed before the FW will accept
//   SCANOFLD_START (Linux always programs a valid "baseline" chan
//   before scan). We pick ch 1 because that's the canonical 2G
//   scan entry point Linux uses in rtw89_hw_scan_prep.
// DPK init (set_dpd_backoff) — runs once, not per-channel.
// Linux rtw8852b_dpk_init is called from phy_dm_init after BB is up.
```

## L272-279 · `let tx_en = chan::set_channel_help_enter(mmio);`

```
// Linux __rtw89_set_channel (core.c:529):
//   set_channel_prepare (help_enter) → set_channel → set_txpwr →
//   set_channel_done (help_exit) → rfk_channel (rx_dck + iqk + tssi + dpk)
// Critical: rfk_channel runs OUTSIDE the set_channel_help bracket
// because set_channel_help turns OFF the ADC (B_ADC_FIFO_RST=0xF) and
// BB reset, which IQK needs enabled to measure TX-LO-leakage via the
// RX path. Running IQK with ADC off = all 4 cal stages fail (cor/fin/
// tx/rx all fail) — exactly our v1.x "LOK fail" pattern from day 1.
```

## L284 · `rfk::rx_dck(mmio);`

```
// RFK outside the help bracket — ADC and BB are ON here.
```

## L288 · `tssi::run(mmio, 0 /* BAND_2G */, 1, &efuse_copy);`

```
/* BAND_2G */
```

## L289 · `dpk::run(mmio, 0 /* band 2G */, 1, 0 /* bw 20M */);`

```
// Full DPK cal (Linux 1:1). Replaces force_bypass.
```

## L290 · `dpk::run(mmio, 0 /* band 2G */, 1, 0 /* bw 20M */);`

```
/* band 2G */
```

## L290 · `dpk::run(mmio, 0 /* band 2G */, 1, 0 /* bw 20M */);`

```
/* bw 20M */
```

## L293-305 · `vif::init(mmio, 0);`

```
// ── Phase 5b: VIF registration — re-enabled in v1.5.0.
//   v1.0/v1.1 wedged the CH12 H2C pipe because our mac::init was
//   missing the 17 per-block DMAC/CMAC IMR enables (Phase 1.1)
//   and the post-FWDL sys_init_ax re-assert (Phase 1.3). Without
//   the per-block IMRs some FW error paths never propagate back
//   through the H2C ACK channel — the FW hangs waiting for an
//   ACK that never comes, and subsequent H2Cs stack up silently.
//
//   Phase 1 closed those gaps in v1.3.0 and v1.4.0. This commit
//   tests the hypothesis: does the full 8-step rtw89_mac_vif_init
//   (port_update + dmac_tbl + cmac_tbl + macid_pause +
//   role_maintain + join_info + addr_cam + default_cmac_tbl)
//   now complete without wedging the pipe?
```

## L308-313 · `const SCAN_PASSES: u32 = 3;`

```
// ── Phase 6: 3× FW scan_offload (2G ch 1..13) ─────────────────
// A single scan pass gets ~100 ms per channel — often only 1-2
// beacons per AP. Running three passes builds up a richer picture
// (more beacon counts per BSSID, better chance to catch distant
// APs whose beacons happen to land outside a single 100-ms window).
// The BSS table accumulates across all passes since it's static.
```

## L325-335 · `host::print("\n[wifi] Phase 7: AUTH via CH8 → HomeAP_New\n");`

```
// ── Phase 7: AUTH via CH8 to HomeAP_New ────────────────────────
// v1.34: v1.33 proved the chip TX via FW-pool (5 APs incl. Probe
// Responses from QL-132 / TP-Link). Now test the real Linux
// direct-TX use-case: a unicast Open-System AUTH request to a
// known AP. This exercises the CH8 BD path with:
//   - INFRA net_type + TSF_UDT_EN + BSSID_FIT_EN (port_cfg)
//   - addr_cam with target BSSID filled + NET_TYPE=INFRA
//   - h2c_join_info dis_conn=false
//   - unicast frame: RTS_EN=1 per Linux !is_bmc rule
// If we see AUTH Response (seq=2, status=0) in RX, the CH8 TX
// path works for its intended Linux-supported use case.
```

## L338 · `const TARGET_BSSID: [u8; 6] = [0xb4, 0xfc, 0x7d, 0x56, 0xa2, 0xe8];`

```
// HomeAP_New FritzBox main on ch 7. BSSID from scan logs.
```

## L352-354 · `mac::scan_stop_to_channel(mmio, TARGET_CH);`

```
// Park FW on target channel, tune host PHY, switch VIF to INFRA.
// Follow Linux __rtw89_set_channel order exactly: help_enter →
// set_channel → set_txpwr → help_exit → RFK (outside bracket).
```

## L360 · `rfk::rx_dck(mmio);`

```
// RFK outside help bracket (ADC must be ON for IQK to measure)
```

## L365 · `dpk::run(mmio, 0 /* band 2G */, TARGET_CH, 0 /* bw 20M */);`

```
/* band 2G */
```

## L365 · `dpk::run(mmio, 0 /* band 2G */, TARGET_CH, 0 /* bw 20M */);`

```
/* bw 20M */
```

## L369-373 · `let tx_counter_post_scan = host::mmio_r32(mmio, 0x0001_1A40) & 0xFFFF;`

```
// Diagnostic: sample TX_COUNTER after scan-finished. v1.33 proved
// the FW pool TX path works (5 APs incl. probe-responders). If
// TX_COUNTER increased across the 3 scan passes, our counter read
// is trustworthy and the "silent drop" diagnosis is real. If it
// stays 0 despite working FW TX, the counter isn't the right one.
```

## L379-382 · `vif::set_target_bssid(mmio, 0, TARGET_BSSID);`

```
// Just refresh addr_cam with target BSSID — NO NET_TYPE flip, no
// join_info. Linux stays NO_LINK until BSS_CHANGED_ASSOC fires
// after AUTH+ASSOC complete. Flipping to INFRA pre-AUTH makes FW
// see the macid as "already associated" and can gate pre-AUTH TX.
```

## L385 · `let ctn_txen = host::mmio_r32(mmio, 0xC348);`

```
// State dump BEFORE AUTH
```

## L394 · `let mut frame = [0u8; 64];`

```
// Build AUTH Open Request frame (30 bytes)
```

## L409 · `let mut busy_seen = false;`

```
// Poll CH8_BUSY 200 ms
```

## L431-435 · `let ptcl_info = host::mmio_r32(mmio, 0xC6F0);`

```
// Where did the frame die? Dump PTCL + WMAC TX debug registers.
// R_AX_PTCL_DBG_INFO = 0xC6F0 — PTCL arbiter state bits
// R_AX_PTCL_DBG      = 0xC6F4 — PTCL last-seen queue + result
// R_AX_WMAC_TX_CTRL_DEBUG = 0xCAE4 — WMAC TX scheduler
// R_AX_WMAC_TX_INFO0_DEBUG = 0xCAE8 — pkt info parsed by WMAC
```

## L446 · `mac::dwell(mmio, 2000);`

```
// Wait up to 2 s for AUTH Response or any RX from AP
```

## L455-457 · `let run_tx_test = |label: &str, ch: u8, ring: &mut tx::TxRing, mmio: i32| {`

```
/*  legacy probe-req diagnostic (kept for reference, unreachable)
    let _unused = f0;
    */
```

## L458-569 · `host::print("\n[wifi] BSS table after Phase 7:\n");`

```
/*

    // helper closure-style: run one TX sub-test on the given channel.
    let run_tx_test = |label: &str, ch: u8, ring: &mut tx::TxRing, mmio: i32| {
        host::print("\n  ── Test "); host::print(label);
        host::print(": DS=ch "); fw::print_dec(ch as usize); host::print(" ──\n");

        // ── State dump BEFORE TX: are the TX gates open? ────────────
        // R_AX_CTN_TXEN (0xC348) bit 8 = MGQ — if 0, mgmt TX is paused
        // R_AX_RX_FLTR_OPT, EDCCA_LVL: leftover scan-mode state?
        let ctn_txen = host::mmio_r32(mmio, 0xC348);
        let rx_fltr  = host::mmio_r32(mmio, 0xCE20);
        let edcca    = host::mmio_r32(mmio, 0x0001_4884);
        host::print("    CTN_TXEN=0x"); host::print_hex32(ctn_txen);
        host::print(" (MGQ="); host::print(if ctn_txen & (1 << 8) != 0 { "on" } else { "OFF" });
        host::print(") RX_FLTR=0x"); host::print_hex32(rx_fltr);
        host::print(" EDCCA=0x"); host::print_hex32(edcca);
        host::print("\n");

        // Control dwell: listen 1 s without sending
        let f0 = mac::wifi_frames_seen();
        let b0 = mac::beacons_seen();
        mac::dwell(mmio, 1000);
        host::print("    pre-TX 1 s: +");
        fw::print_dec((mac::wifi_frames_seen() - f0) as usize);
        host::print(" frames, +");
        fw::print_dec((mac::beacons_seen()    - b0) as usize);
        host::print(" beacons\n");

        // Send Probe Req with matching DS IE
        let mut frame = [0u8; 128];
        let sma = vif::sta_mac();
        let len = tx::build_probe_req(&sma, ch, &mut frame);
        let idx_before = host::mmio_r32(mmio, regs::R_AX_CH8_TXBD_IDX);
        let f1 = mac::wifi_frames_seen();
        let b1 = mac::beacons_seen();

        // TX_COUNTER (0x1A40 in PHY space) counts HW-transmitted frames
        // at the PHY level. If our Probe Req actually reaches the PA,
        // this increments by >=1. If it stays flat, HW silently dropped
        // the frame somewhere between DMA and PA — no matter that
        // CH8_BUSY toggled.
        let tx_cnt_before = host::mmio_r32(mmio, 0x0001_1A40) & 0xFFFF;

        if !tx::send_mgmt(mmio, ring, &frame[..len]) {
            host::print("    TX submit FAILED\n");
            return;
        }

        // ── Poll CH8_BUSY for 200 ms right after TX submit ──────────
        // If the frame really hits the air, DMA_BUSY1.CH8_BUSY will
        // toggle to 1 for a brief moment. If it stays 0 the whole
        // time, the BD was dequeued but nothing went out.
        let mut busy_seen = false;
        for _ in 0..40u32 {
            let b = host::mmio_r32(mmio, regs::R_AX_PCIE_DMA_BUSY1);
            if b & regs::B_AX_CH8_BUSY != 0 { busy_seen = true; break; }
            host::sleep_ms(5);
        }
        host::print("    CH8_BUSY post-submit: ");
        host::print(if busy_seen { "toggled 1\n" } else { "stayed 0 (never active)\n" });

        // TX_COUNTER delta — did HW actually transmit?
        let tx_cnt_after = host::mmio_r32(mmio, 0x0001_1A40) & 0xFFFF;
        let tx_delta = tx_cnt_after.wrapping_sub(tx_cnt_before) & 0xFFFF;
        host::print("    TX_COUNTER: ");
        fw::print_dec(tx_cnt_before as usize);
        host::print(" -> ");
        fw::print_dec(tx_cnt_after as usize);
        host::print(" (delta=");
        fw::print_dec(tx_delta as usize);
        host::print(if tx_delta > 0 { ")  FRAME HIT THE AIR\n" } else { ")  NO TX (HW dropped silently)\n" });

        // Post-TX dwell 2 s
        mac::dwell(mmio, 2000);
        let idx_after = host::mmio_r32(mmio, regs::R_AX_CH8_TXBD_IDX);
        host::print("    TXBD_IDX: 0x"); host::print_hex32(idx_before);
        host::print(" -> 0x"); host::print_hex32(idx_after);
        if (idx_after >> 16) != (idx_before >> 16) {
            host::print(" (hw consumed)\n");
        } else {
            host::print(" (stuck)\n");
        }
        host::print("    post-TX 2 s: +");
        fw::print_dec((mac::wifi_frames_seen() - f1) as usize);
        host::print(" frames, +");
        fw::print_dec((mac::beacons_seen()    - b1) as usize);
        host::print(" beacons\n");
    };

    // ── Test A: ch 13 (where FW was last scanning) ──────────────────
    // Don't touch FW — assume it parked on ch 13. Host-side PHY retune
    // to 13 just in case. apply_default_txpwr fills R_AX_PWR_BY_RATE
    // with 20 dBm so the PA has a non-zero target to transmit at.
    let tx_en_a = chan::set_channel_help_enter(mmio);
    chan::set_channel_2g(mmio, 13);
    chan::apply_default_txpwr(mmio);
    rfk::rx_dck(mmio);
    chan::set_channel_help_exit(mmio, tx_en_a);
    host::print("  tuned to ch 13 (+ default txpwr 20 dBm)\n");
    run_tx_test("A (ch 13)", 13, &mut ring, mmio);

    // ── Test B: switch to ch 7 via SCANOFLD-stop + target ─────────
    mac::scan_stop_to_channel(mmio, 7);
    let tx_en_b = chan::set_channel_help_enter(mmio);
    chan::set_channel_2g(mmio, 7);
    chan::apply_default_txpwr(mmio);
    rfk::rx_dck(mmio);
    chan::set_channel_help_exit(mmio, tx_en_b);
    host::print("  tuned to ch 7 (+ default txpwr 20 dBm)\n");
    run_tx_test("B (ch 7)", 7, &mut ring, mmio);
    */
```

## L571 · `host::print("\n[wifi] BSS table after Phase 7:\n");`

```
// Final BSS table — delta tells the story
```

## L575 · `host::print("\n[wifi] Press 'q' to exit\n");`

```
// ── Done — wait for exit ───────────────────────────────────────
```

