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
static FW_DATA: &[u8] = include_bytes!("rtw8852b_fw.bin");

// ── TX Buffer Descriptor (BD) format ─────────────────────────────
// 8 bytes: length(u16) | option(u16) | dma_addr(u32)
const BD_SIZE: usize = 8;
const BD_OPT_LS: u16 = 1 << 14; // Last Segment

// Firmware download chunk size (4KB - 48 bytes header room)
const FW_CHUNK_SIZE: usize = 4000;

// CH12 ring: 16 buffer descriptors
const CH12_BD_COUNT: u16 = 16;

// TX descriptor size for AX generation (RTL8852B): 8 bytes
const TXDESC_SIZE: usize = 8;

/// BD ring state — tracks the current write index across multiple sends
static mut BD_IDX: u16 = 0;

// ═══════════════════════════════════════════════════════════════════
//  Main entry point
// ═══════════════════════════════════════════════════════════════════

/// Run the full firmware download sequence.
pub fn download(mmio: i32) -> bool {
    host::print("[wifi] Firmware: ");
    print_dec(FW_DATA.len());
    host::print(" bytes\n");

    dump_state(mmio, "initial");

    // ── Step 1: Power OFF (clean UEFI state) ────────────────────
    host::print("[wifi] Power off...\n");
    if !pwr_off(mmio) {
        host::print("[wifi] WARNING: pwr_off incomplete\n");
    }
    dump_state(mmio, "pwr-off");

    // ── Step 2: Power ON (full XTAL/LDO/ISO/func_en) ───────────
    host::print("[wifi] Power on...\n");
    if !pwr_on(mmio) {
        host::print("[wifi] WARNING: pwr_on incomplete\n");
    }
    dump_state(mmio, "pwr-on");

    // ── Step 3: PCIe DMA pre-init ───────────────────────────────
    pcie_dma_pre_init(mmio);

    // ── Step 4: Disable CPU (clean state before FWDL) ───────────
    disable_cpu(mmio);

    // ── Step 5: Enable CPU in FWDL mode ─────────────────────────
    enable_cpu_fwdl(mmio);
    host::sleep_ms(50);

    dump_state(mmio, "cpu-on");

    // ── Step 6: Wait for H2C_PATH_RDY ───────────────────────────
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
            host::print("] FW_CTRL=0x"); host::print_hex32(val);
            host::print("\n");
        }
        host::sleep_ms(1);
    }
    if !h2c_ready {
        host::print("[wifi] H2C path NOT READY\n");
        dump_state(mmio, "h2c-fail");
        return false;
    }

    // ── Step 7: Setup CH12 ring ─────────────────────────────────
    let (ring_dma, data_dma) = match setup_ch12_ring(mmio) {
        Some(r) => r,
        None => {
            host::print("[wifi] CH12 ring setup failed\n");
            return false;
        }
    };

    // ── Step 8: Send FW header ──────────────────────────────────
    let hdr_len = fw_header_len();
    host::print("[wifi] Sending header (");
    print_dec(hdr_len);
    host::print(" bytes)...\n");
    send_fw_chunk(ring_dma, data_dma, mmio, 0, hdr_len);
    host::sleep_ms(20);

    // ── Step 9: Wait FWDL_PATH_RDY ──────────────────────────────
    if !wait_fwdl_path_ready(mmio) {
        host::print("[wifi] FWDL path not ready after header\n");
        return false;
    }

    // ── Step 10: Clear halt channels ────────────────────────────
    host::mmio_w32(mmio, regs::R_AX_HALT_H2C_CTRL, 0);
    host::mmio_w32(mmio, regs::R_AX_HALT_C2H_CTRL, 0);

    // ── Step 11: Send firmware body ─────────────────────────────
    send_firmware_body(mmio, ring_dma, data_dma, hdr_len);

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

/// Write to an XTAL SI register via the indirect interface at 0x0270.
/// `offset` = XTAL SI register address, `val` = data, `mask` = which bits to write.
fn write_xtal_si(mmio: i32, offset: u8, val: u8, mask: u8) -> bool {
    // Build command: CMD_POLL | mode=write(0) | bitmask | data | addr
    let cmd: u32 = (1u32 << 31)
        | ((mask as u32) << 16)
        | ((val as u32) << 8)
        | (offset as u32);
    host::mmio_w32(mmio, regs::R_AX_WLAN_XTAL_SI_CTRL, cmd);

    // Poll until CMD_POLL clears (up to 50ms)
    for _ in 0..50 {
        let v = host::mmio_r32(mmio, regs::R_AX_WLAN_XTAL_SI_CTRL);
        if v & (1u32 << 31) == 0 {
            return true;
        }
        host::sleep_ms(1);
    }
    host::print("[wifi] XTAL SI timeout @0x");
    host::print_hex32(offset as u32);
    host::print("\n");
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

    // Poll RDY_SYSPWR
    let mut ok = false;
    for _ in 0..20 {
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
    for _ in 0..20 {
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

    // Poll ONMAC cleared (auto-clears when done)
    ok = false;
    for _ in 0..20 {
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
    host::mmio_set32(mmio, regs::R_AX_SYS_ADIE_PAD_PWR_CTRL,
        regs::B_AX_SYM_PADPDN_WL_PTA_1P3);

    write_xtal_si(mmio, regs::XTAL_SI_ANAPAR_WL,
        regs::XTAL_SI_GND_SHDN_WL, regs::XTAL_SI_GND_SHDN_WL);

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

    // ── DMAC Function Enable (full set from rtw8852b_pwr_on_func) ─
    host::mmio_set32(mmio, regs::R_AX_DMAC_FUNC_EN,
        regs::B_AX_MAC_FUNC_EN | regs::B_AX_DMAC_FUNC_EN
        | regs::B_AX_MPDU_PROC_EN | regs::B_AX_WD_RLS_EN
        | regs::B_AX_DLE_WDE_EN | regs::B_AX_TXPKT_CTRL_EN
        | regs::B_AX_STA_SCH_EN | regs::B_AX_DLE_PLE_EN
        | regs::B_AX_PKT_BUF_EN | regs::B_AX_DMAC_TBL_EN
        | regs::B_AX_PKT_IN_EN | regs::B_AX_DLE_CPUIO_EN
        | regs::B_AX_DISPATCHER_EN | regs::B_AX_BBRPT_EN
        | regs::B_AX_MAC_SEC_EN | regs::B_AX_DMACREG_GCKEN);

    // ── CMAC Function Enable ────────────────────────────────────
    host::mmio_set32(mmio, regs::R_AX_CMAC_FUNC_EN,
        regs::B_AX_CMAC_EN | regs::B_AX_CMAC_TXEN | regs::B_AX_CMAC_RXEN
        | regs::B_AX_FORCE_CMACREG_GCKEN | regs::B_AX_PHYINTF_EN
        | regs::B_AX_CMAC_DMA_EN | regs::B_AX_PTCLTOP_EN
        | regs::B_AX_SCHEDULER_EN | regs::B_AX_TMAC_EN | regs::B_AX_RMAC_EN);

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

    // Write boot reason = FWDL_RESUME (3)
    host::mmio_w16(mmio, regs::R_AX_BOOT_REASON, regs::RTW89_FW_DLFW_RESUME as u16);

    // Enable WCPU — boot ROM starts in FWDL mode
    host::mmio_set32(mmio, regs::R_AX_PLATFORM_ENABLE, regs::B_AX_WCPU_EN);
}

// ═══════════════════════════════════════════════════════════════════
//  PCIe DMA pre-init
// ═══════════════════════════════════════════════════════════════════

/// Setup PCIe DMA: stop all channels, reset BDRAM, re-enable HCI for CH12.
fn pcie_dma_pre_init(mmio: i32) {
    host::print("[wifi] PCIe DMA init...\n");

    // Enable AXIDMA
    host::mmio_set32(mmio, regs::R_AX_PLATFORM_ENABLE, regs::B_AX_AXIDMA_EN);

    // Hard-stop ALL DMA
    host::mmio_w32(mmio, regs::R_AX_PCIE_DMA_STOP1, 0xFFFFFFFF);
    host::sleep_ms(2);

    // Disable HCI TX/RX
    host::mmio_clr32(mmio, regs::R_AX_PCIE_INIT_CFG1,
        regs::B_AX_TXHCI_EN | regs::B_AX_RXHCI_EN);
    host::sleep_ms(2);

    // Wait DMA idle (up to 500ms)
    for i in 0..50 {
        let busy = host::mmio_r32(mmio, regs::R_AX_PCIE_DMA_BUSY1);
        if busy == 0 { break; }
        if i == 49 {
            host::print("  DMA busy: 0x"); host::print_hex32(busy); host::print("\n");
        }
        host::sleep_ms(10);
    }

    // Clear ALL TX/RX ring indices
    host::mmio_w32(mmio, regs::R_AX_TXBD_RWPTR_CLR1, 0xFFFFFFFF);
    host::mmio_w32(mmio, regs::R_AX_RXBD_RWPTR_CLR, 0xFFFFFFFF);
    host::sleep_ms(1);

    // Reset BDRAM (Realtek does NOT auto-clear this bit)
    host::mmio_set32(mmio, regs::R_AX_PCIE_INIT_CFG1, regs::B_AX_RST_BDRAM);
    host::sleep_ms(2);
    host::mmio_clr32(mmio, regs::R_AX_PCIE_INIT_CFG1, regs::B_AX_RST_BDRAM);
    host::sleep_ms(2);

    // Stop all channels EXCEPT CH12
    let stop = 0x0007FFFF & !regs::B_AX_STOP_CH12;
    host::mmio_w32(mmio, regs::R_AX_PCIE_DMA_STOP1, stop);
    host::sleep_ms(1);

    // Re-enable HCI TX/RX DMA
    host::mmio_set32(mmio, regs::R_AX_PCIE_INIT_CFG1,
        regs::B_AX_TXHCI_EN | regs::B_AX_RXHCI_EN);
}

// ═══════════════════════════════════════════════════════════════════
//  CH12 ring setup + firmware transfer
// ═══════════════════════════════════════════════════════════════════

/// Setup CH12 (FW command) DMA ring.
fn setup_ch12_ring(mmio: i32) -> Option<(i32, i32)> {
    // Allocate 1 page for ring descriptors (16 BDs x 8 bytes = 128 bytes)
    let ring_dma = host::dma_alloc(1);
    if ring_dma < 0 { return None; }
    let ring_phys = host::dma_phys(ring_dma);

    // Allocate 2 pages for data buffer (8KB, enough for fw chunks)
    let data_dma = host::dma_alloc(2);
    if data_dma < 0 { return None; }

    // Program CH12 ring base address
    host::mmio_w32(mmio, regs::R_AX_CH12_TXBD_DESA_L, ring_phys as u32);
    host::mmio_w32(mmio, regs::R_AX_CH12_TXBD_DESA_H, (ring_phys >> 32) as u32);

    // Set ring size — MUST be 16-bit write
    host::mmio_w16(mmio, regs::R_AX_CH12_TXBD_NUM, CH12_BD_COUNT);

    // Reset ring index
    host::mmio_w16(mmio, regs::R_AX_CH12_TXBD_IDX, 0);

    // Reset BD_IDX
    unsafe { BD_IDX = 0; }

    host::print("[wifi] CH12 ring @ 0x");
    host::print_hex32((ring_phys >> 32) as u32);
    host::print_hex32(ring_phys as u32);
    host::print("\n");

    Some((ring_dma, data_dma))
}

/// Send a single firmware chunk via CH12 DMA ring.
fn send_fw_chunk(ring_dma: i32, data_dma: i32, mmio: i32, offset: usize, len: usize) {
    let data_phys = host::dma_phys(data_dma);
    let total_len = TXDESC_SIZE + len;

    // Write 8-byte TX descriptor (zeroed for fwcmd)
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

    // Advance ring write pointer — MUST be 16-bit write
    let new_idx = (bd_idx + 1) % CH12_BD_COUNT;
    host::mmio_w16(mmio, regs::R_AX_CH12_TXBD_IDX, new_idx);

    host::sleep_ms(5);
    unsafe { BD_IDX = new_idx; }
}

/// Send firmware body (everything after header) in chunks via CH12.
fn send_firmware_body(mmio: i32, ring_dma: i32, data_dma: i32, start_offset: usize) {
    let total = FW_DATA.len();
    let mut offset = start_offset;
    let mut chunk_num = 0usize;

    host::print("[wifi] Downloading body: ");

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

// ═══════════════════════════════════════════════════════════════════
//  Polling helpers
// ═══════════════════════════════════════════════════════════════════

/// Wait for FWDL path ready (bit 2 in WCPU_FW_CTRL).
fn wait_fwdl_path_ready(mmio: i32) -> bool {
    host::print("[wifi] Waiting FWDL path ready...\n");
    for i in 0..2000u32 {
        let val = host::mmio_r32(mmio, regs::R_AX_WCPU_FW_CTRL);
        if val & regs::B_AX_FWDL_PATH_RDY != 0 {
            host::print("[wifi] FWDL ready (");
            print_dec(i as usize);
            host::print("ms)\n");
            return true;
        }
        host::sleep_ms(1);
    }
    false
}

/// Firmware header length — first 0x60 bytes (section table).
fn fw_header_len() -> usize {
    if FW_DATA.len() < 0x60 { return FW_DATA.len(); }
    0x60
}

/// Wait for firmware ready — SYS_STATUS1 bit 0.
fn wait_fw_ready(mmio: i32) -> bool {
    host::print("[wifi] Waiting FW ready...\n");
    for i in 0..500u32 {
        let status = host::mmio_r32(mmio, regs::R_AX_SYS_STATUS1);
        let fw_ctrl = host::mmio_r32(mmio, regs::R_AX_WCPU_FW_CTRL);

        if status & 1 != 0 && fw_ctrl & regs::FWDL_CHECKSUM_FAIL == 0 {
            host::print("[wifi] FW ready (");
            print_dec((i * 10) as usize);
            host::print("ms)\n");
            return true;
        }
        if status & 1 != 0 && fw_ctrl & regs::FWDL_CHECKSUM_FAIL != 0 && i > 100 {
            host::print("[wifi] FW ready but checksum fail\n");
            return false;
        }
        host::sleep_ms(10);
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

fn print_dec(n: usize) {
    if n >= 10 { print_dec(n / 10); }
    let d = (n % 10) as u8 + b'0';
    let s = [d];
    host::print(unsafe { core::str::from_utf8_unchecked(&s) });
}
