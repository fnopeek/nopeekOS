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

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }

/// MMIO handle for BAR0 (set during init, used everywhere)
static mut MMIO: i32 = -1;

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    host::print("[wifi] RTL8852BE driver v1.10.0 — Phase 7 with RX diagnostic\n");

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
    chan::set_channel_help_enter(mmio);
    chan::set_channel_2g(mmio, 1);
    rfk::rx_dck(mmio);
    iqk::run(mmio);
    chan::set_channel_help_exit(mmio);
    host::print("[wifi] RFK per-channel flow complete (rx_dck + IQK)\n");

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
    host::print("\n[wifi] Phase 7: TX smoke test on ch 7 (IvyPie)\n");

    // Park on target AP's channel.
    chan::set_channel_help_enter(mmio);
    chan::set_channel_2g(mmio, 7);
    rfk::rx_dck(mmio);
    chan::set_channel_help_exit(mmio);
    host::print("  tuned to ch 7\n");

    // ── Dump RX-path state after the channel switch ─────────────────
    // Scan mode had set RX_FLTR = 0x03004438 (A_BC/A_A1_MATCH/BCN_CHK off,
    // MPDU_MAX_LEN preserved). Linux restores DEFAULT_AX_RX_FLTR
    // (0x030044BE) on scan_complete — we don't, so the filter stays in
    // scan mode. EDCCA was bumped to MAX(249) for scan, also not restored.
    // Print the raw state so we can see what HW is filtering on.
    let rx_fltr = host::mmio_r32(mmio, 0xCE20);
    let edcca   = host::mmio_r32(mmio, 0x0001_4884);
    let busy    = host::mmio_r32(mmio, regs::R_AX_PCIE_DMA_BUSY1);
    host::print("  RX_FLTR_OPT = 0x"); host::print_hex32(rx_fltr);
    host::print("  EDCCA_LVL = 0x"); host::print_hex32(edcca);
    host::print("\n  DMA_BUSY1 = 0x"); host::print_hex32(busy);
    host::print("\n");

    // ── Control dwell — 2 s listening on ch 7, WITHOUT sending ──────
    // If beacons arrive here, RX is healthy and the earlier 0-frame
    // result was just a too-short dwell window. If still 0, the scan→
    // post-scan transition has gated RX somehow (chanctx_pause, DIG
    // suspend, port_cfg_rx_sync — Linux does these, we don't).
    let f0 = mac::wifi_frames_seen();
    let b0 = mac::beacons_seen();
    host::print("  control dwell 2 s (no TX)...\n");
    mac::dwell(mmio, 2000);
    let f1 = mac::wifi_frames_seen();
    let b1 = mac::beacons_seen();
    host::print("  -> +"); fw::print_dec((f1 - f0) as usize);
    host::print(" frames, +"); fw::print_dec((b1 - b0) as usize);
    host::print(" beacons\n");

    // ── Allocate + init CH8 ring, send Probe Req ───────────────────
    let mut ring = match tx::alloc() {
        Some(r) => r,
        None => {
            host::print("  TX: DMA alloc failed\n");
            loop { if host::input_wait(1000) == 0x71 { return; } }
        }
    };
    tx::init_ch8(mmio, &ring);
    host::print("  CH8 ring: BD@0x");
    host::print_hex32((ring.bd_phys >> 32) as u32);
    host::print_hex32(ring.bd_phys as u32);
    host::print(" WD@0x");
    host::print_hex32((ring.wd_phys >> 32) as u32);
    host::print_hex32(ring.wd_phys as u32);
    host::print("\n");

    let idx_before = host::mmio_r32(mmio, regs::R_AX_CH8_TXBD_IDX);

    let mut frame = [0u8; 128];
    let len = tx::build_probe_req(&vif::STA_MAC, 7, &mut frame);
    host::print("  Probe Req (");
    fw::print_dec(len);
    host::print(" B) submitting...\n");

    if !tx::send_mgmt(mmio, &mut ring, &frame[..len]) {
        host::print("  TX submit FAILED\n");
    }

    // ── Post-TX dwell — 2 s ─────────────────────────────────────────
    let f2 = mac::wifi_frames_seen();
    let b2 = mac::beacons_seen();
    let mut saw_hw_move = false;
    for _ in 0..20u32 {
        mac::dwell(mmio, 100);
        let idx_now = host::mmio_r32(mmio, regs::R_AX_CH8_TXBD_IDX);
        if (idx_now >> 16) != (idx_before >> 16) { saw_hw_move = true; }
    }
    let f3 = mac::wifi_frames_seen();
    let b3 = mac::beacons_seen();
    let idx_after = host::mmio_r32(mmio, regs::R_AX_CH8_TXBD_IDX);

    host::print("  TXBD_IDX: 0x");  host::print_hex32(idx_before);
    host::print(" -> 0x"); host::print_hex32(idx_after);
    if saw_hw_move { host::print(" (hw consumed)\n"); }
    else           { host::print(" (stuck)\n"); }

    host::print("  post-TX RX: +");
    fw::print_dec((f3 - f2) as usize);
    host::print(" frames, +");
    fw::print_dec((b3 - b2) as usize);
    host::print(" beacons\n");

    // Show the BSS table again — if we hit anything new (especially
    // if IvyPie's count jumped), that's the TX proof we want.
    host::print("\n[wifi] BSS table after Phase 7:\n");
    mac::scan_summary();

    // ── Done — wait for exit ───────────────────────────────────────
    host::print("\n[wifi] Press 'q' to exit\n");
    loop {
        let key = host::input_wait(1000);
        if key == 0x71 { return; }
    }
}
