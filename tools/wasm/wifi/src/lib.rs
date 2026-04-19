//! wifi — RTL8852BE WiFi 6 driver (WASM module)
//!
//! Phase 1: Chip probe — bind PCI device, map BAR0, read chip registers.
//! Uses the nopeekOS WASM Driver ABI (npk_pci_*, npk_mmio_*, npk_dma_*).

#![no_std]

mod host;
mod regs;
mod fw;
mod mac;
mod phy;
mod rfk;
mod rfk_tables;
mod vif;
mod chan;
mod iqk;
mod iqk_tables;
mod imr;
mod tx;
#[allow(dead_code)]
mod tssi_tables;
mod tssi;
mod dpk;
mod efuse;

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }

/// MMIO handle for BAR0 (set during init, used everywhere)
static mut MMIO: i32 = -1;

/// Parsed efuse data — filled after efuse::read in init, consumed by
/// TSSI, set_txpwr, and the station MAC for Probe Requests.
static mut EFUSE: efuse::EfuseData = efuse::EfuseData::empty();

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    host::print("[wifi] RTL8852BE driver v1.26.0 — IQK re-enabled\n");

    // ── Step 1: Bind PCI device ──────────────────────────────────
    let rc = host::pci_bind(regs::RTL8852B_VENDOR, regs::RTL8852B_DEVICE);
    if rc != 0 {
        host::print("[wifi] PCI bind failed (");
        match rc {
            -1 => host::print("not found"),
            -2 => host::print("denied"),
            _ => host::print("unknown error"),
        }
        host::print(")\n");
        return;
    }
    host::print("[wifi] PCI bind OK\n");

    // ── Step 2: Size BAR2 before enabling memory ─────────────────
    // Linux trusts the kernel (pci_resource_len). We don't have that luxury,
    // so size it ourselves via the 0xFFFFFFFF-write trick. Memory bit must
    // be off during sizing to avoid stray accesses with garbage BAR.
    let cmd = host::pci_read_config(0x04);
    host::pci_write_config(0x04, cmd & !0x02);
    let saved_bar2 = host::pci_read_config(0x18);
    host::pci_write_config(0x18, 0xFFFF_FFFF);
    let bar2_probe = host::pci_read_config(0x18);
    host::pci_write_config(0x18, saved_bar2);
    host::pci_write_config(0x04, cmd);
    let bar2_size = (!(bar2_probe & 0xFFFF_FFF0)).wrapping_add(1);
    let bar2_pages = (bar2_size / 4096) as u16;
    host::print("[wifi] BAR2 size: 0x"); host::print_hex32(bar2_size);
    host::print(" ("); fw::print_dec(bar2_pages as usize); host::print(" pages)\n");

    // ── Step 3: Enable bus master + memory space ──────────────────
    // NO FLR! FLR resets the Digital Die but NOT the Analog Die,
    // desynchronizing the XTAL SI interface between them.
    // Instead we use soft MAC reset (pwr_off → pwr_on) in fw::download().
    host::pci_enable_bus_master();

    // ── Step 4: Map BAR2 (MMIO registers) ─────────────────────────
    // RTL8852BE: BAR0=I/O, BAR2=MMIO (Linux rtw89: bar_id=2).
    // Map the actual BAR size — R_AX_INDIR_ACCESS_ENTRY at 0x40000 needs
    // at least 128 pages for dmac_tbl_init / cmac_tbl_init (mac.c:4291).
    let requested = if bar2_pages >= 128 { 128 } else { bar2_pages };
    let mmio = host::mmio_map_bar(2, requested);
    if mmio < 0 {
        host::print("[wifi] MMIO map BAR2 failed\n");
        host::print("[wifi] Press 'q' to exit\n");
        loop { if host::input_wait(1000) == 0x71 { return; } }
    }
    unsafe { MMIO = mmio; }
    host::print("[wifi] BAR2 mapped\n");
    host::print("\n");

    // ── Step 4: Chip probe — read key registers ──────────────────
    host::print("  RTL8852BE Register Dump\n");
    host::print("  ─────────────────────────────────\n");

    // PCI config space
    let subsys = host::pci_read_config(0x2C);
    host::log_reg("PCI Subsystem     ", subsys);

    let pci_cmd = host::pci_read_config(0x04);
    host::log_reg("PCI Command/Status", pci_cmd);

    // System registers
    let sys_iso = host::mmio_r32(mmio, regs::R_AX_SYS_ISO_CTRL);
    host::log_reg("SYS_ISO_CTRL      ", sys_iso);

    let sys_func = host::mmio_r32(mmio, regs::R_AX_SYS_FUNC_EN);
    host::log_reg("SYS_FUNC_EN       ", sys_func);

    let sys_pw = host::mmio_r32(mmio, regs::R_AX_SYS_PW_CTRL);
    host::log_reg("SYS_PW_CTRL       ", sys_pw);

    let sys_clk = host::mmio_r32(mmio, regs::R_AX_SYS_CLK_CTRL);
    host::log_reg("SYS_CLK_CTRL      ", sys_clk);

    let sys_cfg = host::mmio_r32(mmio, regs::R_AX_SYS_CFG1);
    host::log_reg("SYS_CFG1          ", sys_cfg);

    let platform = host::mmio_r32(mmio, regs::R_AX_PLATFORM_ENABLE);
    host::log_reg("PLATFORM_ENABLE   ", platform);

    // Firmware status
    let fw_ctrl = host::mmio_r32(mmio, regs::R_AX_WCPU_FW_CTRL);
    host::log_reg("WCPU_FW_CTRL      ", fw_ctrl);

    let boot_dbg = host::mmio_r32(mmio, regs::R_AX_BOOT_DBG);
    host::log_reg("BOOT_DBG          ", boot_dbg);

    let boot_reason = host::mmio_r32(mmio, regs::R_AX_BOOT_REASON);
    host::log_reg("BOOT_REASON       ", boot_reason);

    // HCI / DMA status
    let hci_func = host::mmio_r32(mmio, regs::R_AX_HCI_FUNC_EN);
    host::log_reg("HCI_FUNC_EN       ", hci_func);

    let pcie_init = host::mmio_r32(mmio, regs::R_AX_PCIE_INIT_CFG1);
    host::log_reg("PCIE_INIT_CFG1    ", pcie_init);

    let dmac_func = host::mmio_r32(mmio, regs::R_AX_DMAC_FUNC_EN);
    host::log_reg("DMAC_FUNC_EN      ", dmac_func);

    // Halt channels
    let halt_h2c = host::mmio_r32(mmio, regs::R_AX_HALT_H2C_CTRL);
    host::log_reg("HALT_H2C_CTRL     ", halt_h2c);

    let halt_c2h = host::mmio_r32(mmio, regs::R_AX_HALT_C2H_CTRL);
    host::log_reg("HALT_C2H_CTRL     ", halt_c2h);

    host::print("\n");

    // ── Interpret firmware status ────────────────────────────────
    // Real FW ready is in SYS_STATUS1 bit 0, NOT WCPU_FW_CTRL bit 0!
    let sys_status = host::mmio_r32(mmio, regs::R_AX_SYS_STATUS1);
    host::log_reg("SYS_STATUS1       ", sys_status);
    if sys_status & 1 != 0 {
        host::print("[wifi] Firmware: RUNNING (loaded by UEFI/previous boot)\n");
    } else {
        host::print("[wifi] Firmware: NOT RUNNING (needs download)\n");
    }
    if fw_ctrl & regs::FWDL_CHECKSUM_FAIL != 0 {
        host::print("[wifi] WARNING: Firmware checksum fail flag set\n");
    }

    // ── Phase 2: Firmware download ─────────────────────────────────
    host::print("[wifi] Starting firmware download...\n");
    if !fw::download(mmio) {
        host::print("[wifi] Firmware download FAILED\n");
        host::print("[wifi] Press 'q' to exit\n");
        loop {
            if host::input_wait(1000) == 0x71 { return; }
        }
    }

    // ── Phase 3: Post-FWDL status (brief) ─────────────────────────
    let fw_ctrl = host::mmio_r32(mmio, regs::R_AX_WCPU_FW_CTRL);
    host::print("[wifi] FW_CTRL=0x"); host::print_hex32(fw_ctrl);
    host::print(" — firmware running\n");

    // ── Phase 4: MAC init ──────────────────────────────────────────
    if !mac::init(mmio) {
        host::print("[wifi] MAC init FAILED — press 'q' to exit\n");
        loop { if host::input_wait(1000) == 0x71 { return; } }
    }

    // ── Gap 3.7.17: set_txpwr_ctrl — PA reference init.
    // Linux phy_dm_init calls chip->ops->set_txpwr_ctrl once to set the
    // OFDM/CCK power reference for both RF paths. Without this the
    // per-rate power table has no anchor and TX power is undefined.
    chan::apply_txpwr_ctrl(mmio);
    host::print("  TXPWR_CTRL: PA reference (ref=0, ofst=0) applied\n");

    // ── Efuse read — chip-specific calibration data.
    // Done after FW is up and MAC/PHY init is complete so SYS_ISO_CTRL
    // is in a known state. Data goes into EFUSE (static), consumed by
    // TSSI (thermal, tssi_cck/mcs), set_txpwr (gain offsets), and for
    // chip MAC address instead of our pseudo 00:11:22:33:44:55.
    let efuse_data = efuse::read(mmio);
    unsafe { EFUSE = efuse_data; }

    // Propagate efuse MAC into vif::STA_MAC so Probe Requests and the
    // VIF addr_cam use the real chip MAC (not the pseudo 00:11:22:...).
    // Only overwrite when autoload_valid — otherwise keep the pseudo.
    if efuse_data.autoload_valid && efuse_data.mac_addr != [0; 6]
        && efuse_data.mac_addr != [0xFF; 6] {
        unsafe { vif::STA_MAC = efuse_data.mac_addr; }
    }

    // ── Phase 4b: hci_start — 1:1 Linux rtw89_hci_start (core.c:5970).
    //   Unmask PCIe IRQs (HIMR0 + HIMR00 + HIMR10). On 8852BE this is
    //   the last step of rtw89_core_start and enables the RX-DMA event
    //   path. We poll instead of using IRQs, but the IMRs still gate
    //   DMA progress on some AX chips — without them HW_IDX may stay
    //   parked after the first C2H frame.
    mac::hci_start(mmio);

    // ── Phase 5: Baseline channel tune on ch 1 + RFK.
    //   set_channel + rx_dck + iqk is needed before the FW will accept
    //   SCANOFLD_START (Linux always programs a valid "baseline" chan
    //   before scan). We pick ch 1 because that's the canonical 2G
    //   scan entry point Linux uses in rtw89_hw_scan_prep.
    // DPK init (set_dpd_backoff) — runs once, not per-channel.
    // Linux rtw8852b_dpk_init is called from phy_dm_init after BB is up.
    dpk::init(mmio);

    let tx_en = chan::set_channel_help_enter(mmio);
    chan::set_channel_2g(mmio, 1);
    rfk::rx_dck(mmio);
    // IQK re-enabled in v1.26. It was disabled in v1.16 as a diagnostic
    // to rule out iqk_restore state corruption. Now with TSSI properly
    // set up (DE programmed), LOK fail stays (Linux also reports fail
    // on this chip) but TXK/RXK at least give baseline I/Q balance.
    iqk::run(mmio);
    // TSSI with real efuse thermal + set_efuse_to_de.
    let efuse_copy = unsafe { EFUSE };
    tssi::run(mmio, 0 /* BAND_2G */, 1, &efuse_copy);
    // DPK force-bypass: explicit disable instead of uninitialized DPK state.
    dpk::force_bypass(mmio);
    chan::set_channel_help_exit(mmio, tx_en);
    host::print("[wifi] RFK per-channel flow complete (rx_dck + IQK + TSSI + DPK-bypass)\n");

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
    vif::init(mmio, 0);

    // ── Phase 6: 3× FW scan_offload (2G ch 1..13) ─────────────────
    // A single scan pass gets ~100 ms per channel — often only 1-2
    // beacons per AP. Running three passes builds up a richer picture
    // (more beacon counts per BSSID, better chance to catch distant
    // APs whose beacons happen to land outside a single 100-ms window).
    // The BSS table accumulates across all passes since it's static.
    const SCAN_PASSES: u32 = 3;
    for pass in 1..=SCAN_PASSES {
        host::print("\n[wifi] Scan pass ");
        fw::print_dec(pass as usize);
        host::print("/");
        fw::print_dec(SCAN_PASSES as usize);
        host::print("\n");
        mac::scan(mmio);
    }
    mac::scan_summary();

    // ── Phase 7: TX smoke test — Probe Request on CH8 ───────────────
    // Target: IvyPie_New FritzBox on ch 7 (BSSID b4:fc:7d:56:a2:e8).
    // v1.10.0: diagnostic build — check whether RX survives the
    // scan→channel-switch transition before testing TX itself.
    // ── Phase 7: dual-channel TX diagnostic ─────────────────────────
    // Split the TX test into two isolated sub-tests so we can tell
    // "TX path is broken" apart from "channel switch is broken":
    //   7a: stay on ch 13 (where FW parks after last scan pass), TX
    //       a Probe Req with DS=13. If NETGEAR88's beacon count jumps
    //       above its natural ~100 ms rate → TX works on-air and
    //       NETGEAR88 answered.
    //   7b: SCANOFLD-stop to ch 7 + host-side set_channel(7), TX Probe
    //       with DS=7. If IvyPie_New's beacon count jumps → channel
    //       switch works too.
    // Ring allocation is done once and reused across both sub-tests.
    host::print("\n[wifi] Phase 7: dual-channel TX diagnostic\n");

    let mut ring = match tx::alloc() {
        Some(r) => r,
        None => {
            host::print("  TX: DMA alloc failed\n");
            loop { if host::input_wait(1000) == 0x71 { return; } }
        }
    };
    tx::init_ch8(mmio, &ring);
    host::print("  CH8 ring ready\n");

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

    // Final BSS table — delta tells the story
    host::print("\n[wifi] BSS table after Phase 7:\n");
    mac::scan_summary();

    // ── Done — wait for exit ───────────────────────────────────────
    host::print("\n[wifi] Press 'q' to exit\n");
    loop {
        let key = host::input_wait(1000);
        if key == 0x71 { return; }
    }
}
