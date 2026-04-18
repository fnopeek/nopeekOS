//! RTL8852BE firmware download
//!
//! Full sequence based on Linux rtw89 driver (rtw8852b.c + mac.c):
//!   1. pwr_off  — clean UEFI state (disable CPU, XTAL SI restore, OFFMAC)
//!   2. pwr_on   — full power-on (SPS, XTAL SI, ISO, LDO, DMAC/CMAC func_en)
//!   3. PCIe DMA pre-init (stop DMA, reset BDRAM, enable HCI)
//!   4. disable_cpu — clean CPU state
//!   5. enable_cpu  — FWDL mode (WCPU_EN + FWDL_EN)
//!   6. H2C_PATH_RDY → CH12 ring → header → FWDL_PATH_RDY → body → FW ready

use crate::host;
use crate::regs;

/// Embedded firmware blob (rtw8852b_fw-1.bin from linux-firmware)
/// This is an MFW container (sig=0xFF). The actual FW is extracted at runtime.
static MFW_DATA: &[u8] = include_bytes!("rtw8852b_fw.bin");

/// Actual firmware slice (set by mfw_find_fw)
static mut FW_OFFSET: usize = 0;
static mut FW_SIZE: usize = 0;

fn fw_data() -> &'static [u8] {
    unsafe { &MFW_DATA[FW_OFFSET..FW_OFFSET + FW_SIZE] }
}

/// Parse MFW container and find the NORMAL firmware (type=5) matching chip cv.
/// Sets FW_OFFSET and FW_SIZE for the rest of the download.
/// Linux: rtw89_mfw_recognize — picks highest cv <= chip_cv with mp=0.
fn mfw_find_fw(mmio: i32) -> bool {
    if MFW_DATA.len() < 16 || MFW_DATA[0] != 0xFF {
        // Not MFW, use entire blob as firmware
        unsafe { FW_OFFSET = 0; FW_SIZE = MFW_DATA.len(); }
        return true;
    }

    let fw_nr = MFW_DATA[1] as usize;
    host::print("  MFW container: ");
    print_dec(fw_nr);
    host::print(" entries\n");

    // Read chip version from SYS_CFG1[15:12] — determines which firmware to use
    let sys_cfg1 = host::mmio_r32(mmio, regs::R_AX_SYS_CFG1);
    let chip_cv = ((sys_cfg1 >> 12) & 0xF) as u8;
    host::print("  Chip CV = ");
    print_dec(chip_cv as usize);
    host::print("\n");

    // Find best matching firmware: highest cv <= chip_cv, type=5 (NORMAL), mp=0
    let mut best_idx: Option<usize> = None;
    let mut best_cv: u8 = 0;

    for i in 0..fw_nr {
        let off = 16 + i * 16;
        if off + 16 > MFW_DATA.len() { break; }
        let cv = MFW_DATA[off];
        let typ = MFW_DATA[off + 1];
        let mp = MFW_DATA[off + 2];
        let shift = u32::from_le_bytes([
            MFW_DATA[off + 4], MFW_DATA[off + 5],
            MFW_DATA[off + 6], MFW_DATA[off + 7],
        ]) as usize;
        let size = u32::from_le_bytes([
            MFW_DATA[off + 8], MFW_DATA[off + 9],
            MFW_DATA[off + 10], MFW_DATA[off + 11],
        ]) as usize;

        if typ == 5 && mp == 0 && cv <= chip_cv && shift + size <= MFW_DATA.len() {
            if best_idx.is_none() || cv > best_cv {
                best_idx = Some(i);
                best_cv = cv;
            }
        }
    }

    match best_idx {
        Some(i) => {
            let off = 16 + i * 16;
            let shift = u32::from_le_bytes([
                MFW_DATA[off + 4], MFW_DATA[off + 5],
                MFW_DATA[off + 6], MFW_DATA[off + 7],
            ]) as usize;
            let size = u32::from_le_bytes([
                MFW_DATA[off + 8], MFW_DATA[off + 9],
                MFW_DATA[off + 10], MFW_DATA[off + 11],
            ]) as usize;
            host::print("  Using fw[");
            print_dec(i);
            host::print("]: cv=");
            print_dec(best_cv as usize);
            host::print(" offset=0x");
            host::print_hex32(shift as u32);
            host::print(" size=");
            print_dec(size);
            host::print("\n");
            unsafe { FW_OFFSET = shift; FW_SIZE = size; }
            true
        }
        None => {
            host::print("  No matching NORMAL firmware found for chip CV ");
            print_dec(chip_cv as usize);
            host::print("!\n");
            false
        }
    }
}

// ── TX Buffer Descriptor (BD) format ─────────────────────────────
// 8 bytes: length(u16) | option(u16) | dma_addr(u32)
const BD_SIZE: usize = 8;
const BD_OPT_LS: u16 = 1 << 14; // Last Segment

// ── WiFi Descriptor (WD) body ────────────────────────────────────
// 24 bytes (6 dwords), prepended to ALL CH12 DMA transfers.
// The PCIe DMA engine reads the WD to know how to process the packet.
// Linux: struct rtw89_txwd_body, pushed by rtw89_pci_fwcmd_submit.
const WD_BODY_SIZE: usize = 24;

// WD dword0: CHANNEL_DMA = 12 (CH12 = H2C/FWCMD), FW_DL = 0
// Used for FW HEADER download (H2C descriptor follows WD).
const WD_DWORD0_FWCMD_HDR: u32 = 12 << 16;

// WD dword0: CHANNEL_DMA = 12, FW_DL = 1
// Used for FW SECTION download (raw data follows WD, no H2C descriptor).
const WD_DWORD0_FWCMD_BODY: u32 = (1 << 20) | (12 << 16);

// WD dword2: PKT_SIZE in bits [13:0] — data length after WD (set per-packet)

// Firmware download chunk size — must match Linux FWDL_SECTION_PER_PKT_LEN
const FW_CHUNK_SIZE: usize = 2020;

// CH12 ring: 16 buffer descriptors
const CH12_BD_COUNT: u16 = 16;

/// BD ring state — tracks the current write index across multiple sends
pub static mut BD_IDX: u16 = 0;

/// DMA handles (set during FWDL pre-init, reused after init)
pub static mut RING_DMA: i32 = -1;
pub static mut DATA_DMA: i32 = -1;
pub static mut RXQ_DMA: i32 = -1; // RXQ: page 0=BD ring, pages 1-32=data

// ═══════════════════════════════════════════════════════════════════
//  Main entry point
// ═══════════════════════════════════════════════════════════════════

/// Run the full firmware download sequence.
pub fn download(mmio: i32) -> bool {
    // Parse MFW container to find actual firmware matching chip version
    if !mfw_find_fw(mmio) {
        host::print("[wifi] MFW parse failed\n");
        return false;
    }
    host::print("[wifi] Firmware: ");
    print_dec(fw_data().len());
    host::print(" bytes\n");

    dump_state(mmio, "initial");

    // ── Step 1: Power OFF (soft MAC reset, no FLR!) ─────────────
    // FLR is forbidden: it desynchronizes DDIE↔ADIE (kills XTAL SI).
    // Soft pwr_off properly shuts down the MAC while keeping analog die intact.
    host::print("[wifi] Power off...\n");
    if !pwr_off(mmio) {
        host::print("[wifi] WARNING: pwr_off incomplete\n");
    }
    dump_state(mmio, "pwr-off");

    // ── Step 2: Power ON (full sequence with XTAL SI) ───────────
    host::print("[wifi] Power on...\n");
    if !pwr_on(mmio) {
        host::print("[wifi] WARNING: pwr_on incomplete\n");
    }
    dump_state(mmio, "pwr-on");

    // ── Step 3: Enable HCI DMA (rtw89_mac_ctrl_hci_dma_trx) ─────
    // MUST come before dmac_pre_init — enables HCI TX/RX DMA engines.
    // Without this, H2C path can never become ready.
    host::mmio_set32(mmio, regs::R_AX_HCI_FUNC_EN, 0x03); // TXDMA_EN | RXDMA_EN

    // ── Step 4: DMAC/DLE/HFC pre-init for FWDL ─────────────────
    if !dmac_pre_init_dlfw(mmio) {
        host::print("[wifi] WARNING: DLE init incomplete\n");
    }

    // ── Step 5: PCIe DMA pre-init (includes ALL ring setup) ─────
    let (ring_dma, data_dma) = match pcie_dma_pre_init(mmio) {
        Some(r) => r,
        None => {
            host::print("[wifi] PCIe DMA init failed\n");
            return false;
        }
    };
    // Save DMA handles for post-FWDL H2C commands
    unsafe { RING_DMA = ring_dma; DATA_DMA = data_dma; }

    // ── Step 6: Disable + Enable CPU in FWDL mode ───────────────
    disable_cpu(mmio);
    enable_cpu_fwdl(mmio);

    // Note: rtw89_fwdl_secure_idmem_share_mode_ax is a NO-OP without secure boot.
    // SEC_CTRL[17:16] stays at 2 from enable_cpu (correct for 8852B).
    host::sleep_ms(5);

    // ── Summary ─────────────────────────────────────────────────
    let (si_ok, si_fail) = unsafe { (XTAL_SI_OK, XTAL_SI_FAIL) };
    host::print("[wifi] XTAL_SI: ");
    print_dec(si_ok as usize); host::print("/");
    print_dec((si_ok + si_fail) as usize); host::print(" OK\n");

    // ── Step 8: Wait for H2C_PATH_RDY ───────────────────────────
    host::print("[wifi] Waiting H2C path ready...\n");
    let mut h2c_ready = false;
    for i in 0..2000u32 {
        let val = host::mmio_r32(mmio, regs::R_AX_WCPU_FW_CTRL);
        if val & regs::B_AX_H2C_PATH_RDY != 0 {
            host::print("[wifi] H2C ready (");
            print_dec(i as usize);
            host::print("ms)\n");
            h2c_ready = true;
            break;
        }
        if i < 5 || i % 500 == 0 {
            host::print("  ["); print_dec(i as usize);
            host::print("] FW=0x"); host::print_hex32(val);
            host::print("\n");
        }
        host::sleep_ms(1);
    }
    if !h2c_ready {
        host::print("[wifi] H2C path NOT READY\n");
        return false;
    }

    // ── Step 9: Send FW header ──────────────────────────────────
    let (hdr_send_len, body_offset) = fw_header_info();
    host::print("[wifi] Sending header (");
    print_dec(hdr_send_len);
    host::print(" bytes)...\n");
    send_fw_header(ring_dma, data_dma, mmio, hdr_send_len);
    host::sleep_ms(20);

    // ── DMA diagnostic after header send ─────────────────────────
    {
        let stop = host::mmio_r32(mmio, regs::R_AX_PCIE_DMA_STOP1);
        let busy = host::mmio_r32(mmio, regs::R_AX_PCIE_DMA_BUSY1);
        let cfg1 = host::mmio_r32(mmio, regs::R_AX_PCIE_INIT_CFG1);
        let idx  = host::mmio_r32(mmio, regs::R_AX_CH12_TXBD_IDX);
        let dbg  = host::mmio_r32(mmio, regs::R_AX_BOOT_DBG);
        host::print("  DMA_STOP=0x"); host::print_hex32(stop);
        host::print(" BUSY=0x"); host::print_hex32(busy);
        host::print("\n  CFG1=0x"); host::print_hex32(cfg1);
        host::print(" CH12_IDX=0x"); host::print_hex32(idx);
        host::print(" BOOT_DBG=0x"); host::print_hex32(dbg);
        host::print("\n");
    }

    // ── Step 10: Wait FWDL_PATH_RDY ─────────────────────────────
    if !wait_fwdl_path_ready(mmio) {
        host::print("[wifi] FWDL path not ready after header\n");
        return false;
    }

    // ── Step 10: Clear halt channels ────────────────────────────
    host::mmio_w32(mmio, regs::R_AX_HALT_H2C_CTRL, 0);
    host::mmio_w32(mmio, regs::R_AX_HALT_C2H_CTRL, 0);

    // ── Step 11: Send firmware body (section by section, skip BB) ─
    send_firmware_sections(mmio, ring_dma, data_dma, body_offset);

    // ── Step 12: Wait FW ready ──────────────────────────────────
    if !wait_fw_ready(mmio) {
        let status = host::mmio_r32(mmio, regs::R_AX_WCPU_FW_CTRL);
        host::print("[wifi] FW TIMEOUT! FW_CTRL=0x");
        host::print_hex32(status);
        let dbg = host::mmio_r32(mmio, regs::R_AX_BOOT_DBG);
        host::print(" BOOT_DBG=0x"); host::print_hex32(dbg);
        host::print("\n");
        return false;
    }

    host::print("[wifi] Firmware loaded!\n");
    true
}

// ═══════════════════════════════════════════════════════════════════
//  XTAL SI indirect register access
// ═══════════════════════════════════════════════════════════════════

/// PCIe Function Level Reset — hard-resets the device via config space.
/// BARs are cleared but kernel auto-assigns them in mmio_map_bar.
pub fn pcie_flr() {
    // Walk PCIe capability list to find Express Capability (ID=0x10)
    let mut cap_ptr = (host::pci_read_config(0x34) & 0xFF) as u8;
    while cap_ptr != 0 {
        let cap_id = (host::pci_read_config(cap_ptr) & 0xFF) as u8;
        if cap_id == 0x10 {
            // Check FLR support (DevCap bit 28)
            let dev_cap = host::pci_read_config(cap_ptr + 4);
            if dev_cap & (1 << 28) == 0 {
                host::print("[wifi] FLR not supported\n");
                return;
            }
            // Trigger FLR (DevCtl bit 15)
            let mut dev_ctrl = host::pci_read_config(cap_ptr + 8);
            dev_ctrl |= 0x8000;
            host::pci_write_config(cap_ptr + 8, dev_ctrl);
            host::print("[wifi] FLR triggered\n");
            return;
        }
        cap_ptr = ((host::pci_read_config(cap_ptr) >> 8) & 0xFF) as u8;
    }
}

/// XTAL SI success counter
static mut XTAL_SI_OK: u32 = 0;
static mut XTAL_SI_FAIL: u32 = 0;

/// Write to an XTAL SI register via the indirect interface at 0x0270.
pub fn write_xtal_si(mmio: i32, offset: u8, val: u8, mask: u8) -> bool {
    let cmd: u32 = (1u32 << 31)
        | ((mask as u32) << 16)
        | ((val as u32) << 8)
        | (offset as u32);
    host::mmio_w32(mmio, regs::R_AX_WLAN_XTAL_SI_CTRL, cmd);

    for _ in 0..50 {
        let v = host::mmio_r32(mmio, regs::R_AX_WLAN_XTAL_SI_CTRL);
        if v & (1u32 << 31) == 0 {
            unsafe { XTAL_SI_OK += 1; }
            return true;
        }
        host::sleep_ms(1);
    }
    unsafe { XTAL_SI_FAIL += 1; }
    false
}

// ═══════════════════════════════════════════════════════════════════
//  Power OFF — rtw8852b_pwr_off_func()
// ═══════════════════════════════════════════════════════════════════

/// Full power-off sequence (based on Linux rtw8852b_pwr_off_func).
/// Cleans UEFI state so we can re-initialize from scratch.
fn pwr_off(mmio: i32) -> bool {
    // ── Disable CPU first ───────────────────────────────────────
    host::mmio_clr32(mmio, regs::R_AX_PLATFORM_ENABLE, regs::B_AX_WCPU_EN);
    host::mmio_clr32(mmio, regs::R_AX_PLATFORM_ENABLE, regs::B_AX_AXIDMA_EN);
    host::mmio_clr32(mmio, regs::R_AX_WCPU_FW_CTRL, regs::B_AX_WCPU_FWDL_EN);

    // ── XTAL SI: restore power-off defaults ─────────────────────
    write_xtal_si(mmio, regs::XTAL_SI_ANAPAR_WL, regs::XTAL_SI_RFC2RF, regs::XTAL_SI_RFC2RF);
    write_xtal_si(mmio, regs::XTAL_SI_ANAPAR_WL, 0, regs::XTAL_SI_OFF_EI);
    write_xtal_si(mmio, regs::XTAL_SI_ANAPAR_WL, 0, regs::XTAL_SI_OFF_WEI);
    write_xtal_si(mmio, regs::XTAL_SI_WL_RFC_S0, 0, regs::XTAL_SI_RF00);
    write_xtal_si(mmio, regs::XTAL_SI_WL_RFC_S1, 0, regs::XTAL_SI_RF10);
    write_xtal_si(mmio, regs::XTAL_SI_ANAPAR_WL, regs::XTAL_SI_SRAM2RFC, regs::XTAL_SI_SRAM2RFC);
    write_xtal_si(mmio, regs::XTAL_SI_ANAPAR_WL, 0, regs::XTAL_SI_PON_EI);
    write_xtal_si(mmio, regs::XTAL_SI_ANAPAR_WL, 0, regs::XTAL_SI_PON_WEI);

    // ── System power down ───────────────────────────────────────
    host::mmio_set32(mmio, regs::R_AX_SYS_PW_CTRL, regs::B_AX_EN_WLON);
    host::mmio_clr32(mmio, regs::R_AX_WLRF_CTRL, regs::B_AX_AFC_AFEDIG);
    host::mmio_clr8(mmio, regs::R_AX_SYS_FUNC_EN,
        regs::B_AX_FEN_BB_GLB_RSTN | regs::B_AX_FEN_BBRSTB);
    host::mmio_clr32(mmio, regs::R_AX_SYS_ADIE_PAD_PWR_CTRL,
        regs::B_AX_SYM_PADPDN_WL_RFC_1P3);

    write_xtal_si(mmio, regs::XTAL_SI_ANAPAR_WL, 0, regs::XTAL_SI_SHDN_WL);

    host::mmio_clr32(mmio, regs::R_AX_SYS_ADIE_PAD_PWR_CTRL,
        regs::B_AX_SYM_PADPDN_WL_PTA_1P3);

    write_xtal_si(mmio, regs::XTAL_SI_ANAPAR_WL, 0, regs::XTAL_SI_GND_SHDN_WL);

    // ── Request MAC power off ───────────────────────────────────
    host::mmio_set32(mmio, regs::R_AX_SYS_PW_CTRL, regs::B_AX_APFM_OFFMAC);

    // Poll OFFMAC cleared (auto-clears when done)
    let mut ok = false;
    for _ in 0..20 {
        let v = host::mmio_r32(mmio, regs::R_AX_SYS_PW_CTRL);
        if v & regs::B_AX_APFM_OFFMAC == 0 {
            ok = true;
            break;
        }
        host::sleep_ms(1);
    }
    if !ok {
        host::print("  OFFMAC stuck\n");
    }

    // ── PCIe post-off config ────────────────────────────────────
    host::mmio_w32(mmio, regs::R_AX_WLLPS_CTRL, regs::SW_LPS_OPTION);
    host::mmio_set32(mmio, regs::R_AX_SYS_SWR_CTRL1, regs::B_AX_SYM_CTRL_SPS_PWMFREQ);
    host::mmio_w32_mask(mmio, regs::R_AX_SPS_DIG_ON_CTRL0,
        regs::B_AX_REG_ZCDC_H_MASK, 0x3);
    host::mmio_set32(mmio, regs::R_AX_SYS_PW_CTRL, regs::B_AX_APFM_SWLPS);

    ok
}

// ═══════════════════════════════════════════════════════════════════
//  Power ON — rtw8852b_pwr_on_func()
// ═══════════════════════════════════════════════════════════════════

/// Full power-on sequence (based on Linux rtw8852b_pwr_on_func).
/// Initializes XTAL, LDO, ISO, SPS, then enables DMAC + CMAC.
fn pwr_on(mmio: i32) -> bool {
    // ── Wake from low-power ─────────────────────────────────────
    host::mmio_clr32(mmio, regs::R_AX_SYS_PW_CTRL,
        regs::B_AX_AFSM_WLSUS_EN | regs::B_AX_AFSM_PCIE_SUS_EN);
    host::mmio_set32(mmio, regs::R_AX_SYS_PW_CTRL,
        regs::B_AX_DIS_WLBT_PDNSUSEN_SOPC);
    host::mmio_set32(mmio, regs::R_AX_WLLPS_CTRL,
        regs::B_AX_DIS_WLBT_LPSEN_LOPC);
    host::mmio_clr32(mmio, regs::R_AX_SYS_PW_CTRL, regs::B_AX_APDM_HPDN);
    host::mmio_clr32(mmio, regs::R_AX_SYS_PW_CTRL, regs::B_AX_APFM_SWLPS);

    // Poll RDY_SYSPWR (up to 100ms after FLR)
    let mut ok = false;
    for _ in 0..100 {
        if host::mmio_r32(mmio, regs::R_AX_SYS_PW_CTRL) & regs::B_AX_RDY_SYSPWR != 0 {
            ok = true;
            break;
        }
        host::sleep_ms(1);
    }
    if !ok { host::print("  RDY_SYSPWR timeout\n"); return false; }

    // ── AFE LDO ─────────────────────────────────────────────────
    host::mmio_set32(mmio, regs::R_AX_SYS_AFE_LDO_CTRL, regs::B_AX_AON_OFF_PC_EN);
    ok = false;
    for _ in 0..100 {
        if host::mmio_r32(mmio, regs::R_AX_SYS_AFE_LDO_CTRL) & regs::B_AX_AON_OFF_PC_EN != 0 {
            ok = true;
            break;
        }
        host::sleep_ms(1);
    }
    if !ok { host::print("  AON_OFF_PC timeout\n"); return false; }

    // ── SPS dig off config (default non-RFE5 path) ──────────────
    host::mmio_w32_mask(mmio, regs::R_AX_SPS_DIG_OFF_CTRL0,
        regs::B_AX_C1_L1_MASK, 0x1);
    host::mmio_w32_mask(mmio, regs::R_AX_SPS_DIG_OFF_CTRL0,
        regs::B_AX_C3_L1_MASK, 0x3);

    // ── Enable WLAN + request MAC power on ──────────────────────
    host::mmio_set32(mmio, regs::R_AX_SYS_PW_CTRL, regs::B_AX_EN_WLON);
    host::mmio_set32(mmio, regs::R_AX_SYS_PW_CTRL, regs::B_AX_APFN_ONMAC);

    // Poll ONMAC cleared (auto-clears when done, up to 100ms)
    ok = false;
    for _ in 0..100 {
        if host::mmio_r32(mmio, regs::R_AX_SYS_PW_CTRL) & regs::B_AX_APFN_ONMAC == 0 {
            ok = true;
            break;
        }
        host::sleep_ms(1);
    }
    if !ok { host::print("  ONMAC timeout\n"); return false; }

    // ── Platform enable reset dance ─────────────────────────────
    // rtw89 toggles PLATFORM_EN via write8 five times (set-clr-set-clr-set)
    host::mmio_set8(mmio, regs::R_AX_PLATFORM_ENABLE, regs::B_AX_PLATFORM_EN as u8);
    host::mmio_clr8(mmio, regs::R_AX_PLATFORM_ENABLE, regs::B_AX_PLATFORM_EN as u8);
    host::mmio_set8(mmio, regs::R_AX_PLATFORM_ENABLE, regs::B_AX_PLATFORM_EN as u8);
    host::mmio_clr8(mmio, regs::R_AX_PLATFORM_ENABLE, regs::B_AX_PLATFORM_EN as u8);
    host::mmio_set8(mmio, regs::R_AX_PLATFORM_ENABLE, regs::B_AX_PLATFORM_EN as u8);

    // ── PCIe: disable calibration ───────────────────────────────
    host::mmio_clr32(mmio, regs::R_AX_SYS_SDIO_CTRL, regs::B_AX_PCIE_CALIB_EN_V1);

    // ── ADIE PAD power + XTAL SI crystal init ───────────────────
    // Without FLR, DDIE↔ADIE sync is intact → XTAL SI should work.
    host::mmio_set32(mmio, regs::R_AX_SYS_ADIE_PAD_PWR_CTRL,
        regs::B_AX_SYM_PADPDN_WL_PTA_1P3);

    if !write_xtal_si(mmio, regs::XTAL_SI_ANAPAR_WL,
        regs::XTAL_SI_GND_SHDN_WL, regs::XTAL_SI_GND_SHDN_WL) {
        host::print("  XTAL SI failed\n");
    }

    host::mmio_set32(mmio, regs::R_AX_SYS_ADIE_PAD_PWR_CTRL,
        regs::B_AX_SYM_PADPDN_WL_RFC_1P3);

    write_xtal_si(mmio, regs::XTAL_SI_ANAPAR_WL,
        regs::XTAL_SI_SHDN_WL, regs::XTAL_SI_SHDN_WL);
    write_xtal_si(mmio, regs::XTAL_SI_ANAPAR_WL,
        regs::XTAL_SI_OFF_WEI, regs::XTAL_SI_OFF_WEI);
    write_xtal_si(mmio, regs::XTAL_SI_ANAPAR_WL,
        regs::XTAL_SI_OFF_EI, regs::XTAL_SI_OFF_EI);
    write_xtal_si(mmio, regs::XTAL_SI_ANAPAR_WL,
        0, regs::XTAL_SI_RFC2RF);
    write_xtal_si(mmio, regs::XTAL_SI_ANAPAR_WL,
        regs::XTAL_SI_PON_WEI, regs::XTAL_SI_PON_WEI);
    write_xtal_si(mmio, regs::XTAL_SI_ANAPAR_WL,
        regs::XTAL_SI_PON_EI, regs::XTAL_SI_PON_EI);
    write_xtal_si(mmio, regs::XTAL_SI_ANAPAR_WL,
        0, regs::XTAL_SI_SRAM2RFC);
    write_xtal_si(mmio, regs::XTAL_SI_SRAM_CTRL,
        0, regs::XTAL_SI_SRAM_DIS);
    write_xtal_si(mmio, regs::XTAL_SI_XTAL_XMD_2,
        0, regs::XTAL_SI_LDO_LPS);
    write_xtal_si(mmio, regs::XTAL_SI_XTAL_XMD_4,
        0, regs::XTAL_SI_LPS_CAP);

    // ── ISO control ─────────────────────────────────────────────
    host::mmio_set32(mmio, regs::R_AX_PMC_DBG_CTRL2,
        regs::B_AX_SYSON_DIS_PMCR_AX_WRMSK);
    host::mmio_set32(mmio, regs::R_AX_SYS_ISO_CTRL, regs::B_AX_ISO_EB2CORE);
    host::mmio_clr32(mmio, regs::R_AX_SYS_ISO_CTRL, regs::B_AX_PWC_EV2EF_B15);

    host::sleep_ms(1);

    host::mmio_clr32(mmio, regs::R_AX_SYS_ISO_CTRL, regs::B_AX_PWC_EV2EF_B14);
    host::mmio_clr32(mmio, regs::R_AX_PMC_DBG_CTRL2,
        regs::B_AX_SYSON_DIS_PMCR_AX_WRMSK);

    // ── DMAC Function Enable (full set from rtw8852b_pwr_on_func +
    //    sys_init_ax dmac_func_en_ax, including B_AX_DMAC_CRPRT) ──
    host::mmio_set32(mmio, regs::R_AX_DMAC_FUNC_EN,
        regs::B_AX_MAC_FUNC_EN | regs::B_AX_DMAC_FUNC_EN
        | regs::B_AX_MPDU_PROC_EN | regs::B_AX_WD_RLS_EN
        | regs::B_AX_DLE_WDE_EN | regs::B_AX_TXPKT_CTRL_EN
        | regs::B_AX_STA_SCH_EN | regs::B_AX_DLE_PLE_EN
        | regs::B_AX_PKT_BUF_EN | regs::B_AX_DMAC_TBL_EN
        | regs::B_AX_PKT_IN_EN | regs::B_AX_DLE_CPUIO_EN
        | regs::B_AX_DISPATCHER_EN | regs::B_AX_BBRPT_EN
        | regs::B_AX_MAC_SEC_EN | regs::B_AX_DMACREG_GCKEN
        | regs::B_AX_DMAC_CRPRT);

    // ── DMAC Clock Enable — Linux sys_init_ax:1664 ────────────────
    // Enables clocks for all DMAC sub-blocks. Without these DMA/packet-
    // in/dispatcher/wd_rls do not tick → RX DMA never completes.
    host::mmio_set32(mmio, regs::R_AX_DMAC_CLK_EN,
        regs::B_AX_MAC_SEC_CLK_EN | regs::B_AX_DISPATCHER_CLK_EN
        | regs::B_AX_DLE_CPUIO_CLK_EN | regs::B_AX_PKT_IN_CLK_EN
        | regs::B_AX_STA_SCH_CLK_EN | regs::B_AX_TXPKT_CTRL_CLK_EN
        | regs::B_AX_WD_RLS_CLK_EN | regs::B_AX_BBRPT_CLK_EN);

    // ── CMAC Clock Enable — Linux cmac_func_en_ax:1624 ────────────
    // Must come BEFORE CMAC_FUNC_EN. Enables clocks for RMAC, TMAC,
    // PHYINTF, CMAC_DMA, Scheduler, PTCLTOP, CMAC. Without RMAC_CKEN
    // and CMAC_DMA_CKEN the RX path from PHY → MAC → DMA is dead:
    // frames may enter the radio but never reach the host ring.
    host::mmio_set32(mmio, regs::R_AX_CK_EN,
        regs::B_AX_CMAC_CKEN | regs::B_AX_PHYINTF_CKEN
        | regs::B_AX_CMAC_DMA_CKEN | regs::B_AX_PTCLTOP_CKEN
        | regs::B_AX_SCHEDULER_CKEN | regs::B_AX_TMAC_CKEN
        | regs::B_AX_RMAC_CKEN);

    // ── CMAC Function Enable (with B_AX_CMAC_CRPRT) ───────────────
    host::mmio_set32(mmio, regs::R_AX_CMAC_FUNC_EN,
        regs::B_AX_CMAC_EN | regs::B_AX_CMAC_TXEN | regs::B_AX_CMAC_RXEN
        | regs::B_AX_FORCE_CMACREG_GCKEN | regs::B_AX_PHYINTF_EN
        | regs::B_AX_CMAC_DMA_EN | regs::B_AX_PTCLTOP_EN
        | regs::B_AX_SCHEDULER_EN | regs::B_AX_TMAC_EN | regs::B_AX_RMAC_EN
        | regs::B_AX_CMAC_CRPRT);

    // ── Pinmux: EESK func = BT_LOG ─────────────────────────────
    host::mmio_w32_mask(mmio, regs::R_AX_EECS_EESK_FUNC_SEL,
        regs::B_AX_PINMUX_EESK_FUNC_SEL_MASK, 0x1);

    true
}

// ═══════════════════════════════════════════════════════════════════
//  CPU control
// ═══════════════════════════════════════════════════════════════════

/// Disable CPU — clean state before FWDL.
/// Based on rtw89_mac_disable_cpu / disable_cpu_ax.
fn disable_cpu(mmio: i32) {
    // Stop WCPU
    host::mmio_clr32(mmio, regs::R_AX_PLATFORM_ENABLE, regs::B_AX_WCPU_EN);

    // Clear FW control state
    host::mmio_clr32(mmio, regs::R_AX_WCPU_FW_CTRL,
        regs::B_AX_WCPU_FWDL_EN | regs::B_AX_H2C_PATH_RDY | regs::B_AX_FWDL_PATH_RDY);

    // Stop CPU clock
    host::mmio_clr32(mmio, regs::R_AX_SYS_CLK_CTRL, regs::B_AX_CPU_CLK_EN);

    // Toggle APB_WRAP (watchdog reset)
    host::mmio_clr32(mmio, regs::R_AX_PLATFORM_ENABLE, regs::B_AX_APB_WRAP_EN);
    host::mmio_set32(mmio, regs::R_AX_PLATFORM_ENABLE, regs::B_AX_APB_WRAP_EN);

    // Toggle PLATFORM_EN
    host::mmio_clr32(mmio, regs::R_AX_PLATFORM_ENABLE, regs::B_AX_PLATFORM_EN);
    host::mmio_set32(mmio, regs::R_AX_PLATFORM_ENABLE, regs::B_AX_PLATFORM_EN);
}

/// Enable CPU in FWDL mode — rtw89_mac_enable_cpu_ax(fwdl=true).
fn enable_cpu_fwdl(mmio: i32) {
    // Clear UDM registers
    host::mmio_w32(mmio, regs::R_AX_UDM1, 0);
    host::mmio_w32(mmio, regs::R_AX_UDM2, 0);

    // Clear halt channels
    host::mmio_w32(mmio, regs::R_AX_HALT_H2C_CTRL, 0);
    host::mmio_w32(mmio, regs::R_AX_HALT_C2H_CTRL, 0);
    host::mmio_w32(mmio, regs::R_AX_HALT_H2C, 0);
    host::mmio_w32(mmio, regs::R_AX_HALT_C2H, 0);

    // Enable CPU clock
    host::mmio_set32(mmio, regs::R_AX_SYS_CLK_CTRL, regs::B_AX_CPU_CLK_EN);

    // Set FW_CTRL: clear state, set FWDL_EN
    let mut val = host::mmio_r32(mmio, regs::R_AX_WCPU_FW_CTRL);
    val &= !(regs::B_AX_WCPU_FWDL_EN | regs::B_AX_H2C_PATH_RDY | regs::B_AX_FWDL_PATH_RDY);
    val &= !((0x7u32) << 5); // clear FWDL_STS field
    val |= regs::B_AX_WCPU_FWDL_EN;
    host::mmio_w32(mmio, regs::R_AX_WCPU_FW_CTRL, val);

    // Set SEC_IDMEM_SIZE_CONFIG = 2
    val = host::mmio_r32(mmio, regs::R_AX_SEC_CTRL);
    val &= !regs::B_AX_SEC_IDMEM_MASK;
    val |= 0x2 << 16;
    host::mmio_w32(mmio, regs::R_AX_SEC_CTRL, val);

    // Write boot reason = 0 (initial FW download, NOT 3=DLFW_RESUME)
    // Linux: mac->fwdl_enable_wcpu(rtwdev, 0, true, false)
    let aligned = regs::R_AX_BOOT_REASON & !0x3; // 0x01E4
    let shift = (regs::R_AX_BOOT_REASON & 0x2) * 8; // 16
    let mut br = host::mmio_r32(mmio, aligned);
    br &= !((0x7u32) << shift); // clear bits [2:0] → boot_reason = 0
    host::mmio_w32(mmio, aligned, br);

    // Enable WCPU — boot ROM starts in FWDL mode
    host::mmio_set32(mmio, regs::R_AX_PLATFORM_ENABLE, regs::B_AX_WCPU_EN);
}

// ═══════════════════════════════════════════════════════════════════
//  PCIe DMA pre-init
// ═══════════════════════════════════════════════════════════════════

/// DLE + HFC initialization for firmware download mode.
/// Based on rtw89_mac_dmac_pre_init → hci_func_en + dmac_func_pre_en + dle_init(DLFW) + hfc_init.
fn dmac_pre_init_dlfw(mmio: i32) -> bool {
    host::print("[wifi] DMAC/DLE/HFC init (DLFW)...\n");

    // ── hci_func_en_ax: write (NOT set) basic DMAC enables ──────
    let val = regs::B_AX_MAC_FUNC_EN | regs::B_AX_DMAC_FUNC_EN
            | regs::B_AX_DISPATCHER_EN | regs::B_AX_PKT_BUF_EN;
    host::mmio_w32(mmio, regs::R_AX_DMAC_FUNC_EN, val);

    // ── dmac_func_pre_en_ax: enable dispatcher clock ────────────
    const B_AX_DISPATCHER_CLK_EN: u32 = 1 << 18;
    host::mmio_w32(mmio, regs::R_AX_DMAC_CLK_EN, B_AX_DISPATCHER_CLK_EN);

    // ── dle_init(RTW89_QTA_DLFW) ────────────────────────────────

    // 1. Disable DLE
    host::mmio_clr32(mmio, regs::R_AX_DMAC_FUNC_EN,
        regs::B_AX_DLE_WDE_EN | regs::B_AX_DLE_PLE_EN);

    // 2. Enable DLE clocks
    const B_AX_DLE_WDE_CLK_EN: u32 = 1 << 26;
    const B_AX_DLE_PLE_CLK_EN: u32 = 1 << 23;
    host::mmio_set32(mmio, regs::R_AX_DMAC_CLK_EN,
        B_AX_DLE_WDE_CLK_EN | B_AX_DLE_PLE_CLK_EN);

    // 3. Configure WDE buffer: wde_size9 = {PG_64, lnk=0, unlnk=1024}
    //    page_sel=0(64B), bound=0, free_page_num=0
    let mut wde = host::mmio_r32(mmio, regs::R_AX_WDE_PKTBUF_CFG);
    wde &= !(0x3 | (0x3F << 8) | (0x1FFF << 16));
    // page_sel=0, bound=0, free_pages=0
    host::mmio_w32(mmio, regs::R_AX_WDE_PKTBUF_CFG, wde);

    // 4. Configure PLE buffer: ple_size8 = {PG_128, lnk=64, unlnk=960}
    //    WDE total = (0+1024)*64 = 65536, bound = 65536/8192 = 8
    //    page_sel=1(128B), bound=8, free_page_num=64
    let mut ple = host::mmio_r32(mmio, regs::R_AX_PLE_PKTBUF_CFG);
    ple &= !(0x3 | (0x3F << 8) | (0x1FFF << 16));
    ple |= 1;           // page_sel = 1 (128-byte pages)
    ple |= 8 << 8;      // start_bound = 8
    ple |= 64 << 16;    // free_page_num = 64
    host::mmio_w32(mmio, regs::R_AX_PLE_PKTBUF_CFG, ple);

    // 5. Set quotas (DLFW mode: wde_qt4 all zeros, ple_qt13 minimal)
    //    WDE: all 4 channels = {min=0, max=0}
    for i in 0u32..4 {
        host::mmio_w32(mmio, 0x8C40 + i * 4, 0); // R_AX_WDE_QTAn_CFG
    }
    //    PLE: qt13 = {0,0,16,48, 0,0,0,0, 0,0,0}
    //    Format: min[11:0] | max[27:16]
    let ple_qt: [u32; 11] = [
        0,                          // QTA0: mpdu  {min=0, max=0}
        0,                          // QTA1: qtv   {min=0, max=0}
        (16 << 16) | 16,           // QTA2: cpuio {min=16, max=16}
        (48 << 16) | 48,           // QTA3: wcpu  {min=48, max=48}
        0, 0, 0, 0, 0, 0, 0,      // QTA4-QTA10: all zero
    ];
    for i in 0u32..11 {
        host::mmio_w32(mmio, 0x9040 + i * 4, ple_qt[i as usize]);
    }

    // 6. Enable DLE
    host::mmio_set32(mmio, regs::R_AX_DMAC_FUNC_EN,
        regs::B_AX_DLE_WDE_EN | regs::B_AX_DLE_PLE_EN);

    // 7. Poll WDE ready (R_AX_WDE_INI_STATUS bits [1:0] = 0x3)
    let mut wde_ok = false;
    for _ in 0..200 {
        if host::mmio_r32(mmio, 0x8D00) & 0x3 == 0x3 {
            wde_ok = true;
            break;
        }
        host::sleep_ms(1);
    }
    if !wde_ok {
        host::print("  WDE not ready: 0x");
        host::print_hex32(host::mmio_r32(mmio, 0x8D00));
        host::print("\n");
    }

    // 8. Poll PLE ready (R_AX_PLE_INI_STATUS bits [1:0] = 0x3)
    let mut ple_ok = false;
    for _ in 0..200 {
        if host::mmio_r32(mmio, 0x9100) & 0x3 == 0x3 {
            ple_ok = true;
            break;
        }
        host::sleep_ms(1);
    }
    if !ple_ok {
        host::print("  PLE not ready: 0x");
        host::print_hex32(host::mmio_r32(mmio, 0x9100));
        host::print("\n");
    }

    // ── hfc_init(reset=true, en=false, h2c_en=true) ────────────
    // For DLFW: only enable CH12 (H2C) flow control, not full HFC
    let mut hfc = host::mmio_r32(mmio, 0x8A00); // R_AX_HCI_FC_CTRL
    hfc &= !(1u32); // clear HCI_FC_EN
    hfc |= 1 << 3;  // set HCI_FC_CH12_EN
    host::mmio_w32(mmio, 0x8A00, hfc);

    host::print("  DLE: WDE=");
    host::print(if wde_ok { "OK" } else { "FAIL" });
    host::print(" PLE=");
    host::print(if ple_ok { "OK" } else { "FAIL" });
    host::print("\n");

    wde_ok && ple_ok
}

/// Complete PCIe DMA pre-init matching Linux rtw89_pci_ops_mac_pre_init_ax.
/// Includes: PCIe helpers, DMA stop, mode_op, ALL ring setup + BDRAM, DMA enable.
/// Returns (ch12_ring_dma, ch12_data_dma) handles for firmware transfer.
fn pcie_dma_pre_init(mmio: i32) -> Option<(i32, i32)> {
    host::print("[wifi] PCIe DMA init...\n");

    // ── PCIe pre-init helpers (Linux: called before DMA stop) ────
    host::mmio_clr32(mmio, 0x1008, 1 << 5);           // l1off_pwroff
    host::mmio_clr32(mmio, regs::R_AX_SYS_PW_CTRL, 1 << 14); // aphy_pwrcut
    host::mmio_set32(mmio, regs::R_AX_SYS_SDIO_CTRL, 1 << 15); // hci_ldo set
    host::mmio_clr32(mmio, regs::R_AX_SYS_SDIO_CTRL, 1 << 14); // hci_ldo clr
    host::mmio_set32(mmio, 0x0074, 1 << 5);           // power_wake_ax
    host::mmio_clr32(mmio, regs::R_AX_PCIE_EXP_CTRL, 1 << 4); // set_sic
    let mut lbc = host::mmio_r32(mmio, 0x11D8);       // set_lbc
    lbc &= !(0xF << 4); lbc |= 8 << 4; lbc |= 0x3;
    host::mmio_w32(mmio, 0x11D8, lbc);
    host::mmio_set32(mmio, 0x11C0, 0x3);               // set_dbg
    host::mmio_w32_mask(mmio, regs::R_AX_PCIE_EXP_CTRL, 0x3, 1);
    host::mmio_set32(mmio, regs::R_AX_PCIE_INIT_CFG1, (1 << 23) | (1 << 22)); // set_keep_reg

    // ── 5b. Stop WPDMA ──────────────────────────────────────────
    host::mmio_set32(mmio, regs::R_AX_PCIE_DMA_STOP1, 1 << 19);

    // ── 5c. ctrl_dma_all(false): stop PCIEIO + disable TXHCI/RXHCI
    host::mmio_set32(mmio, regs::R_AX_PCIE_DMA_STOP1, 1 << 20);
    host::mmio_clr32(mmio, regs::R_AX_PCIE_INIT_CFG1,
        regs::B_AX_TXHCI_EN | regs::B_AX_RXHCI_EN);

    // ── 5d. Poll DMA idle ───────────────────────────────────────
    for i in 0..100 {
        let busy = host::mmio_r32(mmio, regs::R_AX_PCIE_DMA_BUSY1);
        if busy == 0 { break; }
        if i == 99 { host::print("  DMA busy!\n"); }
        host::sleep_ms(5);
    }

    // ── 5e. Clear all ring indices ──────────────────────────────
    host::mmio_set32(mmio, regs::R_AX_TXBD_RWPTR_CLR1, 0x070F); // ACH0-3,CH8,CH9,CH12
    host::mmio_set32(mmio, regs::R_AX_RXBD_RWPTR_CLR, 0x03);    // RXQ + RPQ

    // ── 5f. mode_op ─────────────────────────────────────────────
    {
        let mut cfg1 = host::mmio_r32(mmio, regs::R_AX_PCIE_INIT_CFG1);
        cfg1 &= !(1 << 18);     // clear RXBD_MODE
        cfg1 &= !(0x7 << 8);  cfg1 |= 7 << 8;   // TX burst = 2048B
        cfg1 &= !(0x7 << 14); cfg1 |= 3 << 14;  // RX burst = 128B
        cfg1 |= 1 << 12;      // LATENCY_CONTROL
        host::mmio_w32(mmio, regs::R_AX_PCIE_INIT_CFG1, cfg1);

        host::mmio_w32_mask(mmio, regs::R_AX_PCIE_EXP_CTRL,
            regs::B_AX_MAX_TAG_NUM_MASK, 7);

        let mut cfg2 = host::mmio_r32(mmio, regs::R_AX_PCIE_INIT_CFG2);
        cfg2 &= !(0xF << 24); cfg2 |= 1 << 24;  // WD idle = 256ns
        cfg2 &= !(0xF << 16); cfg2 |= 1 << 16;  // WD active = 256ns
        host::mmio_w32(mmio, regs::R_AX_PCIE_INIT_CFG2, cfg2);

        host::mmio_set32(mmio, regs::R_AX_TX_ADDR_INFO_MODE, 1);
        host::mmio_clr32(mmio, regs::R_AX_PKTIN_SETTING, 1 << 1);
    }

    // ── 5g. ops_reset: program ALL rings + BDRAM ────────────────
    // Allocate DMA buffers: 1 shared page for dummy rings, 1 for CH12 ring, 2 for data
    let dummy_dma = host::dma_alloc(1);
    if dummy_dma < 0 { return None; }
    let dummy_phys = host::dma_phys(dummy_dma) as u32;

    let ring_dma = host::dma_alloc(1);
    if ring_dma < 0 { return None; }
    let ring_phys = host::dma_phys(ring_dma);

    let data_dma = host::dma_alloc(2);
    if data_dma < 0 { return None; }
    let data_phys = host::dma_phys(data_dma);

    host::print("  CH12 ring=0x"); host::print_hex32(ring_phys as u32);
    host::print(" data=0x"); host::print_hex32(data_phys as u32);
    host::print("\n");

    // TX rings: ACH0-ACH3, CH8, CH9 (dummy), CH12 (real)
    // BDRAM table (rtw89_bd_ram_table_single):
    //   ACH0: start=0, max=5, min=2  → 0x00020500
    //   ACH1: start=5, max=5, min=2  → 0x00020505
    //   ACH2: start=10, max=5, min=2 → 0x0002050A
    //   ACH3: start=15, max=5, min=2 → 0x0002050F
    //   CH8:  start=20, max=4, min=1 → 0x00010414
    //   CH9:  start=24, max=4, min=1 → 0x00010418
    //   CH12: start=28, max=4, min=1 → 0x0001041C

    // ACH0
    host::mmio_w32(mmio, regs::R_AX_ACH0_BDRAM_CTRL, 0x00020500);
    host::mmio_w32(mmio, regs::R_AX_ACH0_TXBD_DESA_L, dummy_phys);
    host::mmio_w32(mmio, regs::R_AX_ACH0_TXBD_DESA_H, 0);
    // ACH1
    host::mmio_w32(mmio, regs::R_AX_ACH1_BDRAM_CTRL, 0x00020505);
    host::mmio_w32(mmio, regs::R_AX_ACH1_TXBD_DESA_L, dummy_phys);
    host::mmio_w32(mmio, regs::R_AX_ACH1_TXBD_DESA_L + 4, 0);
    // ACH2
    host::mmio_w32(mmio, regs::R_AX_ACH2_BDRAM_CTRL, 0x0002050A);
    host::mmio_w32(mmio, regs::R_AX_ACH2_TXBD_DESA_L, dummy_phys);
    host::mmio_w32(mmio, regs::R_AX_ACH2_TXBD_DESA_L + 4, 0);
    // ACH3
    host::mmio_w32(mmio, regs::R_AX_ACH3_BDRAM_CTRL, 0x0002050F);
    host::mmio_w32(mmio, regs::R_AX_ACH3_TXBD_DESA_L, dummy_phys);
    host::mmio_w32(mmio, regs::R_AX_ACH3_TXBD_DESA_L + 4, 0);
    // CH8
    host::mmio_w32(mmio, regs::R_AX_CH8_BDRAM_CTRL, 0x00010414);
    host::mmio_w32(mmio, regs::R_AX_CH8_TXBD_DESA_L, dummy_phys);
    host::mmio_w32(mmio, regs::R_AX_CH8_TXBD_DESA_L + 4, 0);
    // CH9
    host::mmio_w32(mmio, regs::R_AX_CH9_BDRAM_CTRL, 0x00010418);
    host::mmio_w32(mmio, regs::R_AX_CH9_TXBD_DESA_L, dummy_phys);
    host::mmio_w32(mmio, regs::R_AX_CH9_TXBD_DESA_L + 4, 0);

    // CH12 — FWCMD queue (the one we actually use)
    host::mmio_w32(mmio, regs::R_AX_CH12_BDRAM_CTRL, 0x0001041C);
    host::mmio_w32(mmio, regs::R_AX_CH12_TXBD_DESA_L, ring_phys as u32);
    host::mmio_w32(mmio, regs::R_AX_CH12_TXBD_DESA_H, (ring_phys >> 32) as u32);
    host::mmio_w32(mmio, regs::R_AX_CH12_TXBD_NUM, CH12_BD_COUNT as u32);

    // ── RXQ: allocate REAL ring (not dummy!) ───────────────────────
    // Linux allocates rings during probe, BEFORE BDRAM reset.
    // 33 pages: page 0 = BD ring, pages 1-32 = data buffers
    let rxq_dma = host::dma_alloc(33);
    if rxq_dma < 0 { return None; }
    let rxq_phys = host::dma_phys(rxq_dma);
    let rxq_data_phys = rxq_phys + 4096;

    // Pre-fill 32 RX BDs pointing to data buffers
    for i in 0u32..32 {
        let buf_phys = rxq_data_phys + (i as u64) * 4096;
        host::dma_w32(rxq_dma, i * 8, 4096);          // buf_size=4096, opt=0
        host::dma_w32(rxq_dma, i * 8 + 4, buf_phys as u32); // DMA addr
    }
    host::fence();

    // Program RXQ + RPQ NUM as SEPARATE write16 (Linux rtw89_pci_reset_trx_rings).
    // 0x1020 = RXQ_RXBD_NUM, 0x1022 = RPQ_RXBD_NUM. A combined write32 on 0x1020
    // would stomp on HW-owned fields in the adjacent register.
    host::mmio_w16(mmio, regs::R_AX_RXQ_RXBD_NUM, 32);
    host::mmio_w16(mmio, regs::R_AX_RPQ_RXBD_NUM, 1);

    host::mmio_w32(mmio, regs::R_AX_RXQ_RXBD_DESA_L, rxq_phys as u32);
    host::mmio_w32(mmio, regs::R_AX_RXQ_RXBD_DESA_H, (rxq_phys >> 32) as u32);
    host::mmio_w32(mmio, regs::R_AX_RPQ_RXBD_DESA_L, dummy_phys);
    host::mmio_w32(mmio, regs::R_AX_RPQ_RXBD_DESA_H, 0);

    // Save RXQ handle for mac.rs scan polling
    unsafe { RXQ_DMA = rxq_dma; }

    host::print("  RXQ: 0x"); host::print_hex32(rxq_phys as u32);
    host::print(" (32 bufs)\n");

    // DEBUG: readback DESA immediately after write (before any reset)
    let d1 = host::mmio_r32(mmio, regs::R_AX_RXQ_RXBD_DESA_L);
    let c1 = host::mmio_r32(mmio, regs::R_AX_PCIE_INIT_CFG1);
    host::print("  [dbg pre-BDRAM] DESA_L=0x"); host::print_hex32(d1);
    host::print(" CFG1=0x"); host::print_hex32(c1); host::print("\n");

    // Reset BD_IDX for CH12
    unsafe { BD_IDX = 0; }

    // ── 5h. BDRAM reset ─────────────────────────────────────────
    host::mmio_set32(mmio, regs::R_AX_PCIE_INIT_CFG1, regs::B_AX_RST_BDRAM);
    for _ in 0..200 {
        if host::mmio_r32(mmio, regs::R_AX_PCIE_INIT_CFG1) & regs::B_AX_RST_BDRAM == 0 {
            break;
        }
        host::sleep_ms(1);
    }

    // DEBUG: readback DESA after BDRAM reset
    let d2 = host::mmio_r32(mmio, regs::R_AX_RXQ_RXBD_DESA_L);
    let c2 = host::mmio_r32(mmio, regs::R_AX_PCIE_INIT_CFG1);
    host::print("  [dbg post-BDRAM] DESA_L=0x"); host::print_hex32(d2);
    host::print(" CFG1=0x"); host::print_hex32(c2); host::print("\n");

    // NO RXQ IDX write here. 8852BE has rx_ring_eq_is_full=false in Linux,
    // meaning wp=0 and the IDX register is left alone after BDRAM reset.

    // ── 5i. Stop all TX channels ────────────────────────────────
    host::mmio_set32(mmio, regs::R_AX_PCIE_DMA_STOP1, 0x00070F00); // TX_STOP1_MASK_V1

    // ── 5j. Enable CH12 only ────────────────────────────────────
    host::mmio_clr32(mmio, regs::R_AX_PCIE_DMA_STOP1, regs::B_AX_STOP_CH12);

    // ── 5k. ctrl_dma_all(true): clear PCIEIO + enable TXHCI/RXHCI
    host::mmio_clr32(mmio, regs::R_AX_PCIE_DMA_STOP1, 1 << 20); // clear STOP_PCIEIO
    host::mmio_set32(mmio, regs::R_AX_PCIE_INIT_CFG1,
        regs::B_AX_TXHCI_EN | regs::B_AX_RXHCI_EN);

    Some((ring_dma, data_dma))
}

// H2C header constants for FWDL (from Linux fw.h)
const H2C_CAT_MAC: u32 = 1;
const H2C_CL_MAC_FWDL: u32 = 3;
// H2C_FUNC_MAC_FWHDR_DL = 0

/// H2C sequence counter
static mut H2C_SEQ: u8 = 0;

/// Send firmware HEADER via CH12 with WD + H2C descriptor.
/// Linux: rtw89_pci_fwcmd_submit prepends 24-byte WD body,
///        rtw89_h2c_pkt_set_hdr_fwdl prepends 8-byte H2C header.
/// Buffer layout: [WD 24B][H2C 8B][FW header data]
fn send_fw_header(ring_dma: i32, data_dma: i32, mmio: i32, hdr_len: usize) {
    let data_phys = host::dma_phys(data_dma);
    let h2c_payload = 8 + hdr_len;           // H2C total_len = hdr + payload
    let dma_total = WD_BODY_SIZE + h2c_payload; // WD + H2C + data

    // ── WiFi Descriptor (24 bytes) ─────────────────────────────────
    // dword0: ch_dma=12, fw_dl=0 (header download uses fw_dl=false)
    host::dma_w32(data_dma, 0, WD_DWORD0_FWCMD_HDR);
    host::dma_w32(data_dma, 4, 0);       // dword1 = 0
    host::dma_w32(data_dma, 8, (h2c_payload as u32) & 0x3FFF); // dword2: PKT_SIZE
    host::dma_w32(data_dma, 12, 0);      // dword3 = 0
    host::dma_w32(data_dma, 16, 0);      // dword4 = 0
    host::dma_w32(data_dma, 20, 0);      // dword5 = 0

    // ── H2C descriptor (8 bytes) after WD ──────────────────────────
    let seq = unsafe { H2C_SEQ };
    let hdr0: u32 = (H2C_CAT_MAC)
                   | (H2C_CL_MAC_FWDL << 2)
                   | ((seq as u32) << 24);
    let hdr1: u32 = (h2c_payload as u32) & 0x3FFF; // TOTAL_LEN = H2C hdr + data
    host::dma_w32(data_dma, WD_BODY_SIZE as u32, hdr0);
    host::dma_w32(data_dma, WD_BODY_SIZE as u32 + 4, hdr1);

    // ── FW header data after WD + H2C ──────────────────────────────
    host::dma_write_buf(data_dma, (WD_BODY_SIZE + 8) as u32, &fw_data()[..hdr_len]);
    host::fence();

    unsafe { H2C_SEQ = seq.wrapping_add(1); }

    // Debug: dump first 48 bytes of DMA buffer (WD + H2C + start of FW header)
    host::print("  TX[");
    for i in 0..12u32 {
        if i > 0 && i % 8 == 0 { host::print("\n     "); }
        if i > 0 { host::print(" "); }
        host::print_hex32(host::dma_r32(data_dma, i * 4));
    }
    host::print("]\n");

    // BD length = total DMA buffer size (WD + H2C + data)
    submit_bd(ring_dma, data_dma, data_phys, mmio, dma_total);
}

/// Send a firmware SECTION chunk via CH12 — WD + raw data, NO H2C descriptor.
/// Linux: rtw89_pci_fwcmd_submit prepends 24-byte WD body.
///        Section data uses fw_dl=1 (no H2C header).
/// Buffer layout: [WD 24B][section data]
fn send_fw_section(ring_dma: i32, data_dma: i32, mmio: i32, offset: usize, len: usize) {
    let data_phys = host::dma_phys(data_dma);

    // ── WiFi Descriptor (24 bytes) ─────────────────────────────────
    // dword0: ch_dma=12, fw_dl=1 (section download uses fw_dl=true)
    host::dma_w32(data_dma, 0, WD_DWORD0_FWCMD_BODY);
    host::dma_w32(data_dma, 4, 0);       // dword1 = 0
    host::dma_w32(data_dma, 8, (len as u32) & 0x3FFF); // dword2: PKT_SIZE
    host::dma_w32(data_dma, 12, 0);      // dword3 = 0
    host::dma_w32(data_dma, 16, 0);      // dword4 = 0
    host::dma_w32(data_dma, 20, 0);      // dword5 = 0

    // ── Raw firmware data after WD, no H2C header ──────────────────
    host::dma_write_buf(data_dma, WD_BODY_SIZE as u32, &fw_data()[offset..offset + len]);
    host::fence();

    // BD length = WD + section data
    submit_bd(ring_dma, data_dma, data_phys, mmio, WD_BODY_SIZE + len);
}

/// Submit a TX BD to the CH12 ring, advance write pointer, and wait for DMA completion.
/// Polls CH12_TXBD_IDX until HW_IDX advances, ensuring the DMA engine finished reading
/// our data buffer before we overwrite it for the next chunk.
fn submit_bd(ring_dma: i32, _data_dma: i32, data_phys: u64, mmio: i32, total_len: usize) {
    let bd_idx = unsafe { BD_IDX };
    let bd_offset = (bd_idx as u32) * (BD_SIZE as u32);
    let word0 = (total_len as u32) | ((BD_OPT_LS as u32) << 16);
    host::dma_w32(ring_dma, bd_offset, word0);
    host::dma_w32(ring_dma, bd_offset + 4, data_phys as u32);
    host::fence();

    let new_idx = (bd_idx + 1) % CH12_BD_COUNT;

    // CRITICAL: use 16-bit RMW write to preserve HW_IDX in upper 16 bits!
    // Linux: rtw89_write16(rtwdev, addr.idx, wp) — only writes HOST_IDX.
    // mmio_w32 would zero HW_IDX, causing DMA to reprocess old BDs = data corruption.
    // Use mmio_w16 (RMW) — RTL8852B does NOT ignore upper 16 bits on w32!
    // Linux uses writew (true 16-bit write). RMW race is negligible during FWDL.
    host::mmio_w16(mmio, regs::R_AX_CH12_TXBD_IDX, new_idx);

    // Wait for DMA engine to process this BD (HW_IDX == new HOST_IDX)
    for _ in 0..500u32 {
        let idx = host::mmio_r32(mmio, regs::R_AX_CH12_TXBD_IDX);
        let hw_idx = ((idx >> 16) & 0xFFFF) as u16;
        if hw_idx == new_idx { break; }
    }

    unsafe { BD_IDX = new_idx; }
}

/// Section type constants (from Linux rtw89 fw.h)
const FW_SEC_TYPE_BB: u8 = 9; // RTW89_FW_SEC_TYPE_BB — skip during normal FWDL

/// Send firmware sections individually, skipping BB (baseband) sections.
/// Linux: rtw89_fw_download_main iterates sections, __rtw89_fw_download_main
/// sends each in 2020-byte chunks. BB sections (type=9) are skipped when
/// include_bb=false (which is the normal FWDL path).
fn send_firmware_sections(mmio: i32, ring_dma: i32, data_dma: i32, body_offset: usize) {
    let fw = fw_data();

    // Parse section headers from the FW header (after 32-byte base header)
    let w6 = u32::from_le_bytes([fw[0x18], fw[0x19], fw[0x1A], fw[0x1B]]);
    let section_num = ((w6 >> 8) & 0xFF) as usize;

    let mut data_offset = body_offset; // where section data starts in fw blob
    let mut total_chunks = 0usize;

    host::print("[wifi] Downloading ");
    print_dec(section_num);
    host::print(" sections: ");

    for s in 0..section_num {
        let shdr = 32 + s * 16; // section header offset in fw blob
        let w1 = u32::from_le_bytes([fw[shdr + 4], fw[shdr + 5], fw[shdr + 6], fw[shdr + 7]]);
        let sec_len = (w1 & 0x00FFFFFF) as usize; // bits [23:0]
        let sec_type = ((w1 >> 24) & 0xF) as u8;  // bits [27:24]

        // Note: Linux skips BB (type=9) with include_bb=false, but the boot ROM
        // still expects all section data (STS=1 if BB is skipped). Send everything.

        // Send this section in FW_CHUNK_SIZE chunks
        let mut sent = 0usize;
        while sent < sec_len {
            let remaining = sec_len - sent;
            let chunk_len = if remaining > FW_CHUNK_SIZE { FW_CHUNK_SIZE } else { remaining };
            send_fw_section(ring_dma, data_dma, mmio, data_offset + sent, chunk_len);
            sent += chunk_len;
            total_chunks += 1;
        }
        host::print("[S"); print_dec(s); host::print(":"); print_dec(sec_len); host::print("] ");
        data_offset += sec_len;
    }

    host::print("done (");
    print_dec(total_chunks);
    host::print(" chunks)\n");
}

// ═══════════════════════════════════════════════════════════════════
//  Polling helpers
// ═══════════════════════════════════════════════════════════════════

/// Wait for FWDL path ready (bit 2 in WCPU_FW_CTRL).
fn wait_fwdl_path_ready(mmio: i32) -> bool {
    host::print("[wifi] Waiting FWDL path ready...\n");
    let mut last_sts = 0xFFu32;
    for i in 0..2000u32 {
        let val = host::mmio_r32(mmio, regs::R_AX_WCPU_FW_CTRL);
        if val & regs::B_AX_FWDL_PATH_RDY != 0 {
            host::print("[wifi] FWDL ready (");
            print_dec(i as usize);
            host::print("ms)\n");
            return true;
        }
        let sts = (val >> 5) & 0x7;
        if sts != last_sts || i < 5 || i % 500 == 0 {
            host::print("  ["); print_dec(i as usize);
            host::print("] FW_CTRL=0x"); host::print_hex32(val);
            host::print(" STS="); print_dec(sts as usize);
            host::print("\n");
            last_sts = sts;
        }
        host::sleep_ms(1);
    }
    false
}

/// Parse firmware header length from binary.
/// Linux: base_hdr = sizeof(fw_hdr) + section_num * sizeof(fw_hdr_section)
///      = 32 + section_num * 16
/// section_num is in FW_HDR word 6, bits [15:8].
/// Parse firmware header and return (send_len, body_offset).
/// send_len = base header to send to chip (WITHOUT dynamic header).
/// body_offset = where firmware sections start (AFTER full header).
fn fw_header_info() -> (usize, usize) {
    let fw = fw_data();
    if fw.len() < 0x20 { return (fw.len(), fw.len()); }

    let w6 = u32::from_le_bytes([fw[0x18], fw[0x19], fw[0x1A], fw[0x1B]]);
    let section_num = ((w6 >> 8) & 0xFF) as usize;
    let base_hdr_len = 32 + section_num * 16;

    let w7 = u32::from_le_bytes([fw[0x1C], fw[0x1D], fw[0x1E], fw[0x1F]]);
    let dyn_hdr = (w7 >> 16) & 1;

    let (send_len, body_offset) = if dyn_hdr != 0 {
        let w3 = u32::from_le_bytes([fw[0x0C], fw[0x0D], fw[0x0E], fw[0x0F]]);
        let full_hdr = ((w3 >> 16) & 0xFF) as usize;
        // Linux: sends base_hdr only, body starts after full header
        (base_hdr_len, full_hdr)
    } else {
        (base_hdr_len, base_hdr_len)
    };

    host::print("  FW: ");
    print_dec(section_num);
    host::print(" sections, send=");
    print_dec(send_len);
    host::print(" body@");
    print_dec(body_offset);
    host::print("\n");

    (send_len, body_offset)
}

/// Wait for firmware ready — FWDL_STS in WCPU_FW_CTRL bits [7:5] == 7.
/// Linux: rtw89_fw_check_rdy polls for RTW89_FWDL_WCPU_FW_INIT_RDY (7).
fn wait_fw_ready(mmio: i32) -> bool {
    // Wait for DMA to finish processing all BDs first
    let host_idx = unsafe { BD_IDX };
    for _ in 0..100u32 {
        let idx = host::mmio_r32(mmio, regs::R_AX_CH12_TXBD_IDX);
        let hw_idx = ((idx >> 16) & 0xFFFF) as u16;
        if hw_idx == host_idx { break; }
        host::sleep_ms(1);
    }

    host::print("[wifi] Waiting FW ready...\n");
    let mut last_dbg = 0u32;
    for i in 0..2000u32 {
        let fw_ctrl = host::mmio_r32(mmio, regs::R_AX_WCPU_FW_CTRL);
        let fwdl_sts = (fw_ctrl >> 5) & 0x7;

        if fwdl_sts == 7 { // FWDL_WCPU_FW_INIT_RDY
            host::print("[wifi] FW ready! (");
            print_dec(i as usize);
            host::print("ms) FW_CTRL=0x");
            host::print_hex32(fw_ctrl);
            host::print("\n");
            return true;
        }
        if fwdl_sts >= 2 && fwdl_sts <= 5 { // error states
            host::print("[wifi] FW error STS=");
            print_dec(fwdl_sts as usize);
            host::print(" FW_CTRL=0x"); host::print_hex32(fw_ctrl);
            host::print("\n");
            return false;
        }
        // Track BOOT_DBG progress
        let dbg = host::mmio_r32(mmio, regs::R_AX_BOOT_DBG);
        if dbg != last_dbg || i % 500 == 0 {
            host::print("  ["); print_dec(i as usize);
            host::print("] STS="); print_dec(fwdl_sts as usize);
            host::print(" DBG=0x"); host::print_hex32(dbg);
            host::print("\n");
            last_dbg = dbg;
        }
        host::sleep_ms(1);
    }
    false
}

// ═══════════════════════════════════════════════════════════════════
//  Debug helpers
// ═══════════════════════════════════════════════════════════════════

/// Dump key register state for debugging.
fn dump_state(mmio: i32, label: &str) {
    host::print("  ["); host::print(label); host::print("] ");
    host::print("PW=0x");
    host::print_hex32(host::mmio_r32(mmio, regs::R_AX_SYS_PW_CTRL));
    host::print(" FW=0x");
    host::print_hex32(host::mmio_r32(mmio, regs::R_AX_WCPU_FW_CTRL));
    host::print(" PLAT=0x");
    host::print_hex32(host::mmio_r32(mmio, regs::R_AX_PLATFORM_ENABLE));
    host::print("\n");
}

/// Send an H2C command via CH12 (reuses FWDL ring).
/// Payload is raw H2C body (without WD or H2C header).
/// `rack` = request REC_ACK (auto-forced when seq % 4 == 0 for RTW89_CHIP_AX).
/// `dack` = request DONE_ACK from FW (sets H2C_HDR_DONE_ACK bit 15 in hdr1).
/// Linux rtw89_h2c_pkt_set_hdr (fw.c:1564).
pub fn h2c_send(mmio: i32, cat: u8, class: u8, func: u8, rack: bool, dack: bool, payload: &[u8]) {
    let ring_dma = unsafe { RING_DMA };
    let data_dma = unsafe { DATA_DMA };
    if ring_dma < 0 || data_dma < 0 { return; }
    let data_phys = host::dma_phys(data_dma);

    let h2c_len = 8 + payload.len(); // H2C header + payload
    let total = WD_BODY_SIZE + h2c_len;

    // WiFi Descriptor (24B): ch_dma=12, fw_dl=0
    host::dma_w32(data_dma, 0, WD_DWORD0_FWCMD_HDR);
    host::dma_w32(data_dma, 4, 0);
    host::dma_w32(data_dma, 8, (h2c_len as u32) & 0x3FFF);
    host::dma_w32(data_dma, 12, 0);
    host::dma_w32(data_dma, 16, 0);
    host::dma_w32(data_dma, 20, 0);

    // H2C header (8B) — Linux rtw89_h2c_pkt_set_hdr (fw.c:1564)
    let seq = unsafe { H2C_SEQ };
    // RTW89_CHIP_AX && (seq % 4 == 0) → rack forced true (fw.c:1573)
    let rack = rack || (seq % 4 == 0);
    // hdr0: CAT[1:0] | CLASS[7:2] | FUNC[15:8] | DEL_TYPE[19:16]=0 | SEQ[31:24]
    let hdr0: u32 = (cat as u32 & 0x3)
        | ((class as u32 & 0x3F) << 2)
        | ((func as u32) << 8)
        | ((seq as u32) << 24);
    // hdr1: TOTAL_LEN[13:0] | REC_ACK[14] | DONE_ACK[15]
    let mut hdr1: u32 = (h2c_len as u32) & 0x3FFF;
    if rack { hdr1 |= 1 << 14; }
    if dack { hdr1 |= 1 << 15; }
    host::dma_w32(data_dma, WD_BODY_SIZE as u32, hdr0);
    host::dma_w32(data_dma, WD_BODY_SIZE as u32 + 4, hdr1);

    // Payload
    if !payload.is_empty() {
        host::dma_write_buf(data_dma, (WD_BODY_SIZE + 8) as u32, payload);
    }
    host::fence();

    unsafe { H2C_SEQ = seq.wrapping_add(1); }

    submit_bd(ring_dma, data_dma, data_phys, mmio, total);
}

pub fn print_dec(n: usize) {
    if n >= 10 { print_dec(n / 10); }
    let d = (n % 10) as u8 + b'0';
    let s = [d];
    host::print(unsafe { core::str::from_utf8_unchecked(&s) });
}

// ═══════════════════════════════════════════════════════════════════
//  h2c_fw_log — 1:1 Linux rtw89_fw_h2c_fw_log (fw.c:2787).
//  Enables FW firmware log with level=LOUD routed to C2H, with
//  INIT/TASK/PS/ERROR/MLO/SCAN components. Without this the FW is
//  silent and we can't diagnose init failures from its side.
// ═══════════════════════════════════════════════════════════════════

const H2C_CL_FW_INFO:    u8 = 0x0;
const H2C_FUNC_LOG_CFG:  u8 = 0x0;

/// Send LOG_CFG H2C to enable FW trace log via C2H channel.
/// `enable=false` → COMP=0 (effectively off), `enable=true` → default comp set.
pub fn h2c_fw_log(mmio: i32, enable: bool) {
    // Linux: RTW89_FW_LOG_COMP_{INIT=1, TASK=2, PS=11, ERROR=12, MLO=26, SCAN=28}
    let comp: u32 = if enable {
        (1 << 1) | (1 << 2) | (1 << 11) | (1 << 12) | (1 << 26) | (1 << 28)
    } else { 0 };

    // w0[7:0]  = LEVEL  = RTW89_FW_LOG_LEVEL_LOUD (4)
    // w0[15:8] = PATH   = BIT(RTW89_FW_LOG_LEVEL_C2H=1) = 0x02
    // w1[31:0] = COMP
    // w2[31:0] = COMP_EXT = 0
    let w0: u32 = 4u32 | (0x02u32 << 8);
    let w1: u32 = comp;
    let w2: u32 = 0;

    let mut payload = [0u8; 12];
    payload[0..4].copy_from_slice(&w0.to_le_bytes());
    payload[4..8].copy_from_slice(&w1.to_le_bytes());
    payload[8..12].copy_from_slice(&w2.to_le_bytes());

    // Linux: rtw89_h2c_pkt_set_hdr(..., rack=0, dack=0, ...)
    h2c_send(mmio, H2C_CAT_MAC as u8, H2C_CL_FW_INFO, H2C_FUNC_LOG_CFG,
             false, false, &payload);
}
