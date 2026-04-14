//! RTL8852BE firmware download
//!
//! Sequence: disable CPU → enable FWDL → setup H2C ring (CH12) →
//! send firmware header → send firmware sections → verify ready.

use crate::host;
use crate::regs;

/// Embedded firmware blob (rtw8852b_fw-1.bin from linux-firmware)
static FW_DATA: &[u8] = include_bytes!("rtw8852b_fw.bin");

// ── TX Buffer Descriptor (BD) format ─────────────────────────────
// 8 bytes: length(u16) | option(u16) | dma_addr(u32)
const BD_SIZE: usize = 8;
const BD_OPT_LS: u16 = 1 << 14; // Last Segment

// Firmware download chunk size (4KB - 48 bytes header room)
const FW_CHUNK_SIZE: usize = 4000;

// CH12 ring: 16 buffer descriptors
const CH12_BD_COUNT: u16 = 16;

/// Run the full firmware download sequence.
/// Returns true if firmware is ready.
pub fn download(mmio: i32) -> bool {
    host::print("[wifi] Firmware embedded: ");
    print_dec(FW_DATA.len());
    host::print(" bytes\n");

    // Step 1: Full power cycle — UEFI left firmware running, need hard reset
    power_cycle(mmio);

    // Step 2: PCIe DMA pre-init (CRITICAL — FWDL path won't be ready without this)
    pcie_dma_pre_init(mmio);
    host::sleep_ms(10);

    // Step 3: Setup H2C ring (CH12) for DMA transfer
    let (ring_dma, data_dma) = match setup_ch12_ring(mmio) {
        Some(r) => r,
        None => {
            host::print("[wifi] CH12 ring setup failed\n");
            return false;
        }
    };

    // Step 4: Enable CPU in firmware download mode
    enable_cpu_fwdl(mmio);
    host::sleep_ms(10);

    // Step 5: Send firmware HEADER first via CH12 DMA
    // Key insight from rtw89: header is sent BEFORE checking FWDL_PATH_RDY.
    // The header download triggers the path to become ready.
    let hdr_len = fw_header_len();
    host::print("[wifi] Sending FW header (");
    print_dec(hdr_len);
    host::print(" bytes)...\n");
    send_fw_chunk(ring_dma, data_dma, mmio, 0, hdr_len);
    host::sleep_ms(20);

    // Step 6: NOW wait for FWDL path ready (triggered by header)
    if !wait_fwdl_path_ready(mmio) {
        host::print("[wifi] FWDL path not ready after header send\n");
        let ctrl = host::mmio_r32(mmio, regs::R_AX_WCPU_FW_CTRL);
        host::print("  WCPU_FW_CTRL=0x"); host::print_hex32(ctrl); host::print("\n");
        let init = host::mmio_r32(mmio, regs::R_AX_PCIE_INIT_CFG1);
        host::print("  PCIE_INIT=0x"); host::print_hex32(init); host::print("\n");
        let busy = host::mmio_r32(mmio, regs::R_AX_PCIE_DMA_BUSY1);
        host::print("  DMA_BUSY=0x"); host::print_hex32(busy); host::print("\n");
        return false;
    }

    // Step 7: Clear halt channels
    host::mmio_w32(mmio, regs::R_AX_HALT_H2C_CTRL, 0);
    host::mmio_w32(mmio, regs::R_AX_HALT_C2H_CTRL, 0);

    // Step 8: Send remaining firmware body in chunks
    send_firmware_body(mmio, ring_dma, data_dma, hdr_len);

    // Step 9: Wait for firmware ready
    if !wait_fw_ready(mmio) {
        let status = host::mmio_r32(mmio, regs::R_AX_WCPU_FW_CTRL);
        host::print("[wifi] FW ready TIMEOUT! WCPU_FW_CTRL=0x");
        host::print_hex32(status);
        let dbg = host::mmio_r32(mmio, regs::R_AX_BOOT_DBG);
        host::print(" BOOT_DBG=0x");
        host::print_hex32(dbg);
        host::print("\n");
        return false;
    }

    host::print("[wifi] Firmware loaded and running!\n");
    true
}

/// PCIe DMA pre-init: hard-stop ALL DMA, reset MAC, clear indices, enable CH12.
/// The UEFI leaves the chip in an active DMA state — we need an aggressive reset.
fn pcie_dma_pre_init(mmio: i32) {
    host::print("[wifi] PCIe DMA pre-init...\n");

    // 0. Show initial DMA state
    let busy0 = host::mmio_r32(mmio, regs::R_AX_PCIE_DMA_BUSY1);
    host::print("  DMA_BUSY before: 0x"); host::print_hex32(busy0); host::print("\n");

    // 1. Hard-stop ALL DMA: set every stop bit
    host::mmio_w32(mmio, regs::R_AX_PCIE_DMA_STOP1, 0xFFFFFFFF);
    host::sleep_ms(2);

    // 2. Disable HCI TX/RX
    let mut cfg = host::mmio_r32(mmio, regs::R_AX_PCIE_INIT_CFG1);
    cfg &= !(regs::B_AX_TXHCI_EN | regs::B_AX_RXHCI_EN);
    host::mmio_w32(mmio, regs::R_AX_PCIE_INIT_CFG1, cfg);
    host::sleep_ms(2);

    // 3. Wait for DMA idle (up to 500ms)
    let mut idle = false;
    for i in 0..50 {
        let busy = host::mmio_r32(mmio, regs::R_AX_PCIE_DMA_BUSY1);
        if busy == 0 { idle = true; break; }
        if i == 49 {
            host::print("  DMA_BUSY still: 0x"); host::print_hex32(busy);
            host::print(" (continuing anyway)\n");
        }
        host::sleep_ms(10);
    }
    if idle { host::print("  DMA idle\n"); }

    // 6. Clear ALL TX/RX ring indices
    host::mmio_w32(mmio, regs::R_AX_TXBD_RWPTR_CLR1, 0xFFFFFFFF);
    host::mmio_w32(mmio, regs::R_AX_RXBD_RWPTR_CLR, 0xFFFFFFFF);
    host::sleep_ms(1);

    // 7. Reset BDRAM — Realtek does NOT auto-clear this bit!
    cfg = host::mmio_r32(mmio, regs::R_AX_PCIE_INIT_CFG1);
    cfg |= regs::B_AX_RST_BDRAM;
    host::mmio_w32(mmio, regs::R_AX_PCIE_INIT_CFG1, cfg);
    host::sleep_ms(2);
    // Manually clear the reset bit
    cfg = host::mmio_r32(mmio, regs::R_AX_PCIE_INIT_CFG1);
    cfg &= !regs::B_AX_RST_BDRAM;
    host::mmio_w32(mmio, regs::R_AX_PCIE_INIT_CFG1, cfg);
    host::sleep_ms(2);

    // 8. Configure DMA stop: stop all channels EXCEPT CH12
    let stop = 0x0007FFFF & !regs::B_AX_STOP_CH12; // all channels stopped, CH12 open
    host::mmio_w32(mmio, regs::R_AX_PCIE_DMA_STOP1, stop);
    host::sleep_ms(1);

    // 9. Re-enable HCI TX/RX DMA
    cfg = host::mmio_r32(mmio, regs::R_AX_PCIE_INIT_CFG1);
    cfg |= regs::B_AX_TXHCI_EN | regs::B_AX_RXHCI_EN;
    host::mmio_w32(mmio, regs::R_AX_PCIE_INIT_CFG1, cfg);

    let busy1 = host::mmio_r32(mmio, regs::R_AX_PCIE_DMA_BUSY1);
    host::print("  DMA_BUSY after: 0x"); host::print_hex32(busy1); host::print("\n");
    host::print("[wifi] PCIe DMA pre-init done\n");
}

/// Full MAC power-off + power-on cycle.
/// The UEFI leaves firmware running — a simple CPU disable is not enough.
/// Based on rtw8852b_pwr_off_func() + rtw8852b_pwr_on_func().
fn power_cycle(mmio: i32) {
    host::print("[wifi] Power cycling MAC...\n");

    // ── Phase 1: Power OFF ──────────────────────────────────────

    // Disable WCPU
    let mut val = host::mmio_r32(mmio, regs::R_AX_PLATFORM_ENABLE);
    val &= !regs::B_AX_WCPU_EN;
    host::mmio_w32(mmio, regs::R_AX_PLATFORM_ENABLE, val);

    // Disable AXI DMA
    val = host::mmio_r32(mmio, regs::R_AX_PLATFORM_ENABLE);
    val &= !regs::B_AX_AXIDMA_EN;
    host::mmio_w32(mmio, regs::R_AX_PLATFORM_ENABLE, val);

    // Clear FWDL enable
    val = host::mmio_r32(mmio, regs::R_AX_WCPU_FW_CTRL);
    val &= !regs::B_AX_WCPU_FWDL_EN;
    host::mmio_w32(mmio, regs::R_AX_WCPU_FW_CTRL, val);

    // Request MAC power OFF via SYS_PW_CTRL
    val = host::mmio_r32(mmio, regs::R_AX_SYS_PW_CTRL);
    val |= regs::B_AX_APFM_OFFMAC;
    host::mmio_w32(mmio, regs::R_AX_SYS_PW_CTRL, val);

    // Poll until OFFMAC clears (hardware auto-clears when done)
    for _ in 0..200 {
        let v = host::mmio_r32(mmio, regs::R_AX_SYS_PW_CTRL);
        if v & regs::B_AX_APFM_OFFMAC == 0 { break; }
        host::sleep_ms(1);
    }
    host::sleep_ms(10);
    host::print("  MAC powered off\n");

    // ── Phase 2: Power ON ───────────────────────────────────────

    // Clear suspend modes
    val = host::mmio_r32(mmio, regs::R_AX_SYS_PW_CTRL);
    val &= !(regs::B_AX_AFSM_WLSUS_EN | regs::B_AX_AFSM_PCIE_SUS_EN);
    host::mmio_w32(mmio, regs::R_AX_SYS_PW_CTRL, val);

    // Clear power-down and low-power states
    val = host::mmio_r32(mmio, regs::R_AX_SYS_PW_CTRL);
    val &= !(regs::B_AX_APDM_HPDN | regs::B_AX_APFM_SWLPS);
    host::mmio_w32(mmio, regs::R_AX_SYS_PW_CTRL, val);

    // Wait for system power ready
    for _ in 0..200 {
        let v = host::mmio_r32(mmio, regs::R_AX_SYS_PW_CTRL);
        if v & regs::B_AX_RDY_SYSPWR != 0 { break; }
        host::sleep_ms(1);
    }

    // Enable WLAN
    val = host::mmio_r32(mmio, regs::R_AX_SYS_PW_CTRL);
    val |= regs::B_AX_EN_WLON;
    host::mmio_w32(mmio, regs::R_AX_SYS_PW_CTRL, val);

    // Request MAC power ON
    val = host::mmio_r32(mmio, regs::R_AX_SYS_PW_CTRL);
    val |= regs::B_AX_APFN_ONMAC;
    host::mmio_w32(mmio, regs::R_AX_SYS_PW_CTRL, val);

    // Poll until ONMAC clears (hardware auto-clears when done)
    for _ in 0..200 {
        let v = host::mmio_r32(mmio, regs::R_AX_SYS_PW_CTRL);
        if v & regs::B_AX_APFN_ONMAC == 0 { break; }
        host::sleep_ms(1);
    }
    host::sleep_ms(10);

    // Verify SYS_STATUS1 — firmware should NOT be running now
    let status = host::mmio_r32(mmio, regs::R_AX_SYS_STATUS1);
    host::print("  SYS_STATUS1 after power cycle: 0x");
    host::print_hex32(status);
    host::print("\n");
    host::print("  MAC powered on\n");
}

/// Enable CPU in firmware download mode
/// Based on rtw89_mac_enable_cpu() + rtw89_mac_fwdl_enable()
fn enable_cpu_fwdl(mmio: i32) {
    host::print("[wifi] Enabling FWDL mode...\n");

    // 1. Clear halt channels
    host::mmio_w32(mmio, regs::R_AX_HALT_H2C_CTRL, 0);
    host::mmio_w32(mmio, regs::R_AX_HALT_C2H_CTRL, 0);

    // 2. Set boot reason to FWDL resume
    // R_AX_BOOT_REASON (0x01E6) is a BYTE register — must use aligned read-modify-write.
    // Aligned 32-bit register is at 0x01E4. BOOT_REASON is byte [2] = bits [23:16].
    let aligned_addr = regs::R_AX_BOOT_REASON & !0x3; // 0x01E4
    let byte_pos = (regs::R_AX_BOOT_REASON & 0x3) * 8; // bit 16
    let mut val = host::mmio_r32(mmio, aligned_addr);
    val &= !(0xFF << byte_pos); // clear byte
    val |= (regs::RTW89_FW_DLFW_RESUME & 0xFF) << byte_pos;
    host::mmio_w32(mmio, aligned_addr, val);

    // 3. Enable CPU clock
    val = host::mmio_r32(mmio, regs::R_AX_SYS_CLK_CTRL);
    val |= regs::B_AX_CPU_CLK_EN;
    host::mmio_w32(mmio, regs::R_AX_SYS_CLK_CTRL, val);

    // 4. Enable FWDL mode in FW_CTRL
    val = host::mmio_r32(mmio, regs::R_AX_WCPU_FW_CTRL);
    val |= regs::B_AX_WCPU_FWDL_EN;
    host::mmio_w32(mmio, regs::R_AX_WCPU_FW_CTRL, val);

    // 5. Enable AXI DMA
    val = host::mmio_r32(mmio, regs::R_AX_PLATFORM_ENABLE);
    val |= regs::B_AX_AXIDMA_EN;
    host::mmio_w32(mmio, regs::R_AX_PLATFORM_ENABLE, val);

    // 6. Enable WCPU
    val = host::mmio_r32(mmio, regs::R_AX_PLATFORM_ENABLE);
    val |= regs::B_AX_WCPU_EN;
    host::mmio_w32(mmio, regs::R_AX_PLATFORM_ENABLE, val);

    host::sleep_ms(50); // give boot ROM time to start

    // Debug: show state after enable
    let ctrl = host::mmio_r32(mmio, regs::R_AX_WCPU_FW_CTRL);
    host::print("  FW_CTRL after enable: 0x"); host::print_hex32(ctrl); host::print("\n");
}

/// Setup CH12 (FW command) DMA ring.
/// Returns (ring_dma_handle, data_dma_handle) or None on failure.
fn setup_ch12_ring(mmio: i32) -> Option<(i32, i32)> {
    // Allocate 1 page for ring descriptors (16 BDs × 8 bytes = 128 bytes)
    let ring_dma = host::dma_alloc(1);
    if ring_dma < 0 { return None; }
    let ring_phys = host::dma_phys(ring_dma);

    // Allocate 2 pages for data buffer (8KB, enough for fw chunks)
    let data_dma = host::dma_alloc(2);
    if data_dma < 0 { return None; }

    // Program CH12 ring base address (32-bit writes)
    host::mmio_w32(mmio, regs::R_AX_CH12_TXBD_DESA_L, ring_phys as u32);
    host::mmio_w32(mmio, regs::R_AX_CH12_TXBD_DESA_H, (ring_phys >> 32) as u32);

    // Set ring size — MUST be 16-bit write! (rtw89 uses rtw89_write16)
    host::mmio_w16(mmio, regs::R_AX_CH12_TXBD_NUM, CH12_BD_COUNT);

    // Reset ring index — also 16-bit write for host_idx
    host::mmio_w16(mmio, regs::R_AX_CH12_TXBD_IDX, 0);

    host::print("[wifi] CH12 ring: phys=0x");
    host::print_hex32((ring_phys >> 32) as u32);
    host::print_hex32(ring_phys as u32);
    host::print(" (");
    print_dec(CH12_BD_COUNT as usize);
    host::print(" BDs)\n");

    Some((ring_dma, data_dma))
}

/// Wait for FWDL path to be ready (poll R_AX_WCPU_FW_CTRL bit 2)
fn wait_fwdl_path_ready(mmio: i32) -> bool {
    host::print("[wifi] Waiting for FWDL path ready...\n");
    for _ in 0..1000 {
        let val = host::mmio_r32(mmio, regs::R_AX_WCPU_FW_CTRL);
        if val & regs::B_AX_FWDL_PATH_RDY != 0 {
            host::print("[wifi] FWDL path ready\n");
            return true;
        }
        host::sleep_ms(1);
    }
    false
}

/// Firmware header length — the rtw89 header starts at offset 0x60.
/// The first 0x60 bytes are the section table, then the actual header follows.
/// For rtw8852b_fw-1.bin, the header is typically 0x60 bytes (96 bytes).
fn fw_header_len() -> usize {
    // Read header length from firmware: offset 0x04 bits [15:0] in section table
    // For safety, use the section table size (first 0x60 bytes)
    if FW_DATA.len() < 0x60 { return FW_DATA.len(); }
    0x60
}

/// BD ring state — tracks the current write index across multiple sends
static mut BD_IDX: u16 = 0;

// TX descriptor size for AX generation (RTL8852B): 8 bytes
// Prepended before firmware data in the DMA buffer.
const TXDESC_SIZE: usize = 8;

/// Send a single firmware chunk via CH12 DMA ring.
/// Prepends an 8-byte TX descriptor (zeroed for fwcmd) before the data.
fn send_fw_chunk(ring_dma: i32, data_dma: i32, mmio: i32, offset: usize, len: usize) {
    let data_phys = host::dma_phys(data_dma);
    let total_len = TXDESC_SIZE + len;

    // Write 8-byte TX descriptor (zeroed for fwcmd — rtw89 uses memset(0) + fill)
    host::dma_w32(data_dma, 0, 0);
    host::dma_w32(data_dma, 4, 0);

    // Copy firmware data after the descriptor
    host::dma_write_buf(data_dma, TXDESC_SIZE as u32, &FW_DATA[offset..offset + len]);
    host::fence();

    // Write TX BD: length(u16) | option(u16) | dma_addr(u32)
    let bd_idx = unsafe { BD_IDX };
    let bd_offset = (bd_idx as u32) * (BD_SIZE as u32);
    let word0 = (total_len as u32) | ((BD_OPT_LS as u32) << 16);
    host::dma_w32(ring_dma, bd_offset, word0);
    host::dma_w32(ring_dma, bd_offset + 4, data_phys as u32);
    host::fence();

    // Read index register BEFORE update
    let idx_before = host::mmio_r32(mmio, regs::R_AX_CH12_TXBD_IDX);

    // Advance ring write pointer — MUST be 16-bit write!
    let new_idx = (bd_idx + 1) % CH12_BD_COUNT;
    host::mmio_w16(mmio, regs::R_AX_CH12_TXBD_IDX, new_idx);

    // Wait for hardware to consume (HW_IDX should advance in upper 16 bits)
    let mut consumed = false;
    for i in 0..500 {
        let reg = host::mmio_r32(mmio, regs::R_AX_CH12_TXBD_IDX);
        let hw_idx = (reg >> 16) & 0xFFF;
        if hw_idx == new_idx as u32 { consumed = true; break; }
        if i == 499 {
            host::print("  BD TIMEOUT! before=0x");
            host::print_hex32(idx_before);
            host::print(" now=0x");
            host::print_hex32(reg);
            host::print("\n  BD=0x");
            host::print_hex32(host::dma_r32(ring_dma, bd_offset));
            host::print(":0x");
            host::print_hex32(host::dma_r32(ring_dma, bd_offset + 4));
            host::print(" phys=0x");
            host::print_hex32(data_phys as u32);
            host::print("\n");
        }
        host::sleep_ms(1);
    }
    if consumed {
        host::print("  BD consumed OK\n");
    }

    unsafe { BD_IDX = new_idx; }
}

/// Send firmware body (everything after header) in chunks via CH12
fn send_firmware_body(mmio: i32, ring_dma: i32, data_dma: i32, start_offset: usize) {
    let total = FW_DATA.len();
    let mut offset = start_offset;
    let mut chunk_num = 0usize;

    host::print("[wifi] Downloading firmware body: ");

    while offset < total {
        let remaining = total - offset;
        let chunk_len = if remaining > FW_CHUNK_SIZE { FW_CHUNK_SIZE } else { remaining };

        send_fw_chunk(ring_dma, data_dma, mmio, offset, chunk_len);

        offset += chunk_len;
        chunk_num += 1;
        if chunk_num % 10 == 0 { host::print("."); }
    }

    host::print(" done (");
    print_dec(chunk_num);
    host::print(" chunks)\n");
}

/// Poll for firmware ready — real status is in R_AX_SYS_STATUS1 bit 0,
/// NOT in WCPU_FW_CTRL (where bit 0 is FWDL_EN that we set ourselves!)
fn wait_fw_ready(mmio: i32) -> bool {
    host::print("[wifi] Waiting for firmware ready...\n");
    for i in 0..500 {
        // Real FW ready: SYS_STATUS1 bit 0
        let status = host::mmio_r32(mmio, regs::R_AX_SYS_STATUS1);
        let rdy = status & 1 != 0;

        // Checksum fail: still in WCPU_FW_CTRL
        let fw_ctrl = host::mmio_r32(mmio, regs::R_AX_WCPU_FW_CTRL);
        let chk_fail = fw_ctrl & regs::FWDL_CHECKSUM_FAIL != 0;

        if rdy && !chk_fail {
            host::print("[wifi] FW ready after ");
            print_dec(i * 10);
            host::print("ms\n");
            return true;
        }
        if rdy && chk_fail && i > 100 {
            host::print("[wifi] FW ready but checksum fail persists\n");
            return false;
        }
        host::sleep_ms(10);
    }
    false
}

fn print_dec(n: usize) {
    if n >= 10 { print_dec(n / 10); }
    let d = (n % 10) as u8 + b'0';
    let s = [d];
    host::print(unsafe { core::str::from_utf8_unchecked(&s) });
}
