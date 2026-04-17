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

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }

/// MMIO handle for BAR0 (set during init, used everywhere)
static mut MMIO: i32 = -1;

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    host::print("[wifi] RTL8852BE driver v0.89 — fix MPDU_MAX_LEN (was zero, rejected all frames)\n");

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

    // ── Phase 5: Full Linux set_channel + rfk_channel flow ────────
    // Linux rtw8852b_ops wraps this as:
    //   set_channel_help(ENTER) → set_channel → rfk_channel → set_channel_help(EXIT)
    // where rfk_channel = rx_dck + iqk + tssi + dpk. We skip TSSI/DPK
    // (TX-only) but rx_dck + iqk are essential for RX.
    chan::set_channel_help_enter(mmio);
    chan::set_channel_2g(mmio, 7);
    rfk::rx_dck(mmio);       // per-channel RX DC offset calibration
    iqk::run(mmio);          // full IQK with BB+RF backup/restore
    chan::set_channel_help_exit(mmio);
    host::print("[wifi] RFK per-channel flow complete (rx_dck + IQK)\n");

    // ── Phase 6: WiFi scan ─────────────────────────────────────────
    mac::scan(mmio);

    // ── Done — wait for exit ───────────────────────────────────────
    host::print("\n[wifi] Press 'q' to exit\n");
    loop {
        let key = host::input_wait(1000);
        if key == 0x71 { return; }
    }
}
