//! wifi — RTL8852BE WiFi 6 driver (WASM module)
//!
//! Phase 1: Chip probe — bind PCI device, map BAR0, read chip registers.
//! Uses the nopeekOS WASM Driver ABI (npk_pci_*, npk_mmio_*, npk_dma_*).

#![no_std]

mod host;
mod regs;
mod fw;

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }

/// MMIO handle for BAR0 (set during init, used everywhere)
static mut MMIO: i32 = -1;

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    host::print("[wifi] RTL8852BE driver v0.55\n");

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

    // ── Step 2: Enable bus master + memory space ──────────────────
    // NO FLR! FLR resets the Digital Die but NOT the Analog Die,
    // desynchronizing the XTAL SI interface between them.
    // Instead we use soft MAC reset (pwr_off → pwr_on) in fw::download().
    host::pci_enable_bus_master();

    // ── Step 3: Map BAR2 (MMIO registers) ─────────────────────────
    // RTL8852BE: BAR0=I/O, BAR2=MMIO (Linux rtw89: bar_id=2).
    // Kernel auto-assigns address + configures bridge if BAR is empty.
    let mmio = host::mmio_map_bar(2, 16);
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

    // ── Phase 3: Post-FWDL status ──────────────────────────────────
    host::print("\n[wifi] Phase 3: Firmware status\n");
    host::print("  ─────────────────────────────────\n");

    let fw_ctrl = host::mmio_r32(mmio, regs::R_AX_WCPU_FW_CTRL);
    host::log_reg("WCPU_FW_CTRL      ", fw_ctrl);

    let sys_status = host::mmio_r32(mmio, regs::R_AX_SYS_STATUS1);
    host::log_reg("SYS_STATUS1       ", sys_status);
    if sys_status & 1 != 0 {
        host::print("  Firmware: RUNNING\n");
    }

    let boot_dbg = host::mmio_r32(mmio, regs::R_AX_BOOT_DBG);
    host::log_reg("BOOT_DBG          ", boot_dbg);

    // Read UDM registers — firmware may write version/status here
    let udm0 = host::mmio_r32(mmio, 0x01F0); // R_AX_UDM0
    let udm1 = host::mmio_r32(mmio, 0x01F4); // R_AX_UDM1
    let udm2 = host::mmio_r32(mmio, 0x01F8); // R_AX_UDM2
    let udm3 = host::mmio_r32(mmio, 0x01FC); // R_AX_UDM3
    host::log_reg("UDM0 (FW info)    ", udm0);
    host::log_reg("UDM1              ", udm1);
    host::log_reg("UDM2              ", udm2);
    host::log_reg("UDM3              ", udm3);

    // C2H registers — firmware may have sent us a message
    let halt_c2h = host::mmio_r32(mmio, regs::R_AX_HALT_C2H);
    let halt_c2h_ctrl = host::mmio_r32(mmio, regs::R_AX_HALT_C2H_CTRL);
    host::log_reg("HALT_C2H          ", halt_c2h);
    host::log_reg("HALT_C2H_CTRL     ", halt_c2h_ctrl);

    // MAC/DMAC status after FW boot (skip CMAC — might not be enabled yet)
    let hci_func = host::mmio_r32(mmio, regs::R_AX_HCI_FUNC_EN);
    let dmac_func = host::mmio_r32(mmio, regs::R_AX_DMAC_FUNC_EN);
    host::log_reg("HCI_FUNC_EN       ", hci_func);
    host::log_reg("DMAC_FUNC_EN      ", dmac_func);

    host::print("\n[wifi] Phase 3 complete — press 'q' to exit\n");

    // ── Wait for user input ──────────────────────────────────────
    loop {
        let key = host::input_wait(1000);
        if key == 0x71 { return; } // 'q'
    }
}
