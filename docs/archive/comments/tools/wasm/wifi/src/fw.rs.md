# `tools/wasm/wifi/src/fw.rs` @ 5e0102684

## L1-9 · `use crate::host;`

```
//! RTL8852BE firmware download
//!
//! Full sequence based on Linux rtw89 driver (rtw8852b.c + mac.c):
//!   1. pwr_off  — clean UEFI state (disable CPU, XTAL SI restore, OFFMAC)
//!   2. pwr_on   — full power-on (SPS, XTAL SI, ISO, LDO, DMAC/CMAC func_en)
//!   3. PCIe DMA pre-init (stop DMA, reset BDRAM, enable HCI)
//!   4. disable_cpu — clean CPU state
//!   5. enable_cpu  — FWDL mode (WCPU_EN + FWDL_EN)
//!   6. H2C_PATH_RDY → CH12 ring → header → FWDL_PATH_RDY → body → FW ready
```

## L14-15 · `static MFW_DATA: &[u8] = include_bytes!("rtw8852b_fw.bin");`

```
/// Embedded firmware blob (rtw8852b_fw-1.bin from linux-firmware)
/// This is an MFW container (sig=0xFF). The actual FW is extracted at runtime.
```

## L18 · `static mut FW_OFFSET: usize = 0;`

```
/// Actual firmware slice (set by mfw_find_fw)
```

## L26-28 · `fn mfw_find_fw(mmio: i32) -> bool {`

```
/// Parse MFW container and find the NORMAL firmware (type=5) matching chip cv.
/// Sets FW_OFFSET and FW_SIZE for the rest of the download.
/// Linux: rtw89_mfw_recognize — picks highest cv <= chip_cv with mp=0.
```

## L31 · `unsafe { FW_OFFSET = 0; FW_SIZE = MFW_DATA.len(); }`

```
// Not MFW, use entire blob as firmware
```

## L41 · `let sys_cfg1 = host::mmio_r32(mmio, regs::R_AX_SYS_CFG1);`

```
// Read chip version from SYS_CFG1[15:12] — determines which firmware to use
```

## L48 · `let mut best_idx: Option<usize> = None;`

```
// Find best matching firmware: highest cv <= chip_cv, type=5 (NORMAL), mp=0
```

## L107-108 · `const BD_SIZE: usize = 8;`

```
// ── TX Buffer Descriptor (BD) format ─────────────────────────────
// 8 bytes: length(u16) | option(u16) | dma_addr(u32)
```

## L110 · `const BD_OPT_LS: u16 = 1 << 14; // Last Segment`

```
// Last Segment
```

## L112-115 · `const WD_BODY_SIZE: usize = 24;`

```
// ── WiFi Descriptor (WD) body ────────────────────────────────────
// 24 bytes (6 dwords), prepended to ALL CH12 DMA transfers.
// The PCIe DMA engine reads the WD to know how to process the packet.
// Linux: struct rtw89_txwd_body, pushed by rtw89_pci_fwcmd_submit.
```

## L118-119 · `const WD_DWORD0_FWCMD_HDR: u32 = 12 << 16;`

```
// WD dword0: CHANNEL_DMA = 12 (CH12 = H2C/FWCMD), FW_DL = 0
// Used for FW HEADER download (H2C descriptor follows WD).
```

## L122-123 · `const WD_DWORD0_FWCMD_BODY: u32 = (1 << 20) | (12 << 16);`

```
// WD dword0: CHANNEL_DMA = 12, FW_DL = 1
// Used for FW SECTION download (raw data follows WD, no H2C descriptor).
```

## L126 · `const FW_CHUNK_SIZE: usize = 2020;`

```
// WD dword2: PKT_SIZE in bits [13:0] — data length after WD (set per-packet)
```

## L128 · `const FW_CHUNK_SIZE: usize = 2020;`

```
// Firmware download chunk size — must match Linux FWDL_SECTION_PER_PKT_LEN
```

## L131 · `const CH12_BD_COUNT: u16 = 16;`

```
// CH12 ring: 16 buffer descriptors
```

## L134 · `pub static mut BD_IDX: u16 = 0;`

```
/// BD ring state — tracks the current write index across multiple sends
```

## L137 · `pub static mut RING_DMA: i32 = -1;`

```
/// DMA handles (set during FWDL pre-init, reused after init)
```

## L140 · `pub static mut RXQ_DMA: i32 = -1; // RXQ: page 0=BD ring, pages 1-32=data`

```
// RXQ: page 0=BD ring, pages 1-32=data
```

## L142-144 · `pub fn download(mmio: i32) -> bool {`

```
// ═══════════════════════════════════════════════════════════════════
//  Main entry point
// ═══════════════════════════════════════════════════════════════════
```

## L146 · `pub fn download(mmio: i32) -> bool {`

```
/// Run the full firmware download sequence.
```

## L148 · `if !mfw_find_fw(mmio) {`

```
// Parse MFW container to find actual firmware matching chip version
```

## L159-161 · `host::print("[wifi] Power off...\n");`

```
// ── Step 1: Power OFF (soft MAC reset, no FLR!) ─────────────
// FLR is forbidden: it desynchronizes DDIE↔ADIE (kills XTAL SI).
// Soft pwr_off properly shuts down the MAC while keeping analog die intact.
```

## L168 · `host::print("[wifi] Power on...\n");`

```
// ── Step 2: Power ON (full sequence with XTAL SI) ───────────
```

## L175-177 · `host::mmio_set32(mmio, regs::R_AX_HCI_FUNC_EN, 0x03); // TXDMA_EN | RXDMA_EN`

```
// ── Step 3: Enable HCI DMA (rtw89_mac_ctrl_hci_dma_trx) ─────
// MUST come before dmac_pre_init — enables HCI TX/RX DMA engines.
// Without this, H2C path can never become ready.
```

## L178 · `host::mmio_set32(mmio, regs::R_AX_HCI_FUNC_EN, 0x03); // TXDMA_EN | RXDMA_EN`

```
// TXDMA_EN | RXDMA_EN
```

## L180 · `if !dmac_pre_init_dlfw(mmio) {`

```
// ── Step 4: DMAC/DLE/HFC pre-init for FWDL ─────────────────
```

## L185 · `let (ring_dma, data_dma) = match pcie_dma_pre_init(mmio) {`

```
// ── Step 5: PCIe DMA pre-init (includes ALL ring setup) ─────
```

## L193 · `unsafe { RING_DMA = ring_dma; DATA_DMA = data_dma; }`

```
// Save DMA handles for post-FWDL H2C commands
```

## L196 · `disable_cpu(mmio);`

```
// ── Step 6: Disable + Enable CPU in FWDL mode ───────────────
```

## L200-201 · `host::sleep_ms(5);`

```
// Note: rtw89_fwdl_secure_idmem_share_mode_ax is a NO-OP without secure boot.
// SEC_CTRL[17:16] stays at 2 from enable_cpu (correct for 8852B).
```

## L204 · `let (si_ok, si_fail) = unsafe { (XTAL_SI_OK, XTAL_SI_FAIL) };`

```
// ── Summary ─────────────────────────────────────────────────
```

## L210 · `host::print("[wifi] Waiting H2C path ready...\n");`

```
// ── Step 8: Wait for H2C_PATH_RDY ───────────────────────────
```

## L234 · `let (hdr_send_len, body_offset) = fw_header_info();`

```
// ── Step 9: Send FW header ──────────────────────────────────
```

## L242 · `if VERBOSE {`

```
// ── DMA diagnostic after header send (verbose-only) ─────────
```

## L257 · `if !wait_fwdl_path_ready(mmio) {`

```
// ── Step 10: Wait FWDL_PATH_RDY ─────────────────────────────
```

## L263 · `host::mmio_w32(mmio, regs::R_AX_HALT_H2C_CTRL, 0);`

```
// ── Step 10: Clear halt channels ────────────────────────────
```

## L267 · `send_firmware_sections(mmio, ring_dma, data_dma, body_offset);`

```
// ── Step 11: Send firmware body (section by section, skip BB) ─
```

## L270 · `if !wait_fw_ready(mmio) {`

```
// ── Step 12: Wait FW ready ──────────────────────────────────
```

## L285-287 · `pub fn pcie_flr() {`

```
// ═══════════════════════════════════════════════════════════════════
//  XTAL SI indirect register access
// ═══════════════════════════════════════════════════════════════════
```

## L289-290 · `pub fn pcie_flr() {`

```
/// PCIe Function Level Reset — hard-resets the device via config space.
/// BARs are cleared but kernel auto-assigns them in mmio_map_bar.
```

## L292 · `let mut cap_ptr = (host::pci_read_config(0x34) & 0xFF) as u8;`

```
// Walk PCIe capability list to find Express Capability (ID=0x10)
```

## L297 · `let dev_cap = host::pci_read_config(cap_ptr + 4);`

```
// Check FLR support (DevCap bit 28)
```

## L303 · `let mut dev_ctrl = host::pci_read_config(cap_ptr + 8);`

```
// Trigger FLR (DevCtl bit 15)
```

## L314 · `static mut XTAL_SI_OK: u32 = 0;`

```
/// XTAL SI success counter
```

## L318 · `pub fn write_xtal_si(mmio: i32, offset: u8, val: u8, mask: u8) -> bool {`

```
/// Write to an XTAL SI register via the indirect interface at 0x0270.
```

## L338-340 · `fn pwr_off(mmio: i32) -> bool {`

```
// ═══════════════════════════════════════════════════════════════════
//  Power OFF — rtw8852b_pwr_off_func()
// ═══════════════════════════════════════════════════════════════════
```

## L342-343 · `fn pwr_off(mmio: i32) -> bool {`

```
/// Full power-off sequence (based on Linux rtw8852b_pwr_off_func).
/// Cleans UEFI state so we can re-initialize from scratch.
```

## L345 · `host::mmio_clr32(mmio, regs::R_AX_PLATFORM_ENABLE, regs::B_AX_WCPU_EN);`

```
// ── Disable CPU first ───────────────────────────────────────
```

## L350 · `write_xtal_si(mmio, regs::XTAL_SI_ANAPAR_WL, regs::XTAL_SI_RFC2RF, regs::XTAL_SI_RFC2RF);`

```
// ── XTAL SI: restore power-off defaults ─────────────────────
```

## L360 · `host::mmio_set32(mmio, regs::R_AX_SYS_PW_CTRL, regs::B_AX_EN_WLON);`

```
// ── System power down ───────────────────────────────────────
```

## L375 · `host::mmio_set32(mmio, regs::R_AX_SYS_PW_CTRL, regs::B_AX_APFM_OFFMAC);`

```
// ── Request MAC power off ───────────────────────────────────
```

## L378 · `let mut ok = false;`

```
// Poll OFFMAC cleared (auto-clears when done)
```

## L392 · `host::mmio_w32(mmio, regs::R_AX_WLLPS_CTRL, regs::SW_LPS_OPTION);`

```
// ── PCIe post-off config ────────────────────────────────────
```

## L402-404 · `fn pwr_on(mmio: i32) -> bool {`

```
// ═══════════════════════════════════════════════════════════════════
//  Power ON — rtw8852b_pwr_on_func()
// ═══════════════════════════════════════════════════════════════════
```

## L406-407 · `fn pwr_on(mmio: i32) -> bool {`

```
/// Full power-on sequence (based on Linux rtw8852b_pwr_on_func).
/// Initializes XTAL, LDO, ISO, SPS, then enables DMAC + CMAC.
```

## L409 · `host::mmio_clr32(mmio, regs::R_AX_SYS_PW_CTRL,`

```
// ── Wake from low-power ─────────────────────────────────────
```

## L419 · `let mut ok = false;`

```
// Poll RDY_SYSPWR (up to 100ms after FLR)
```

## L430 · `host::mmio_set32(mmio, regs::R_AX_SYS_AFE_LDO_CTRL, regs::B_AX_AON_OFF_PC_EN);`

```
// ── AFE LDO ─────────────────────────────────────────────────
```

## L442 · `host::mmio_w32_mask(mmio, regs::R_AX_SPS_DIG_OFF_CTRL0,`

```
// ── SPS dig off config (default non-RFE5 path) ──────────────
```

## L448 · `host::mmio_set32(mmio, regs::R_AX_SYS_PW_CTRL, regs::B_AX_EN_WLON);`

```
// ── Enable WLAN + request MAC power on ──────────────────────
```

## L452 · `ok = false;`

```
// Poll ONMAC cleared (auto-clears when done, up to 100ms)
```

## L463-464 · `host::mmio_set8(mmio, regs::R_AX_PLATFORM_ENABLE, regs::B_AX_PLATFORM_EN as u8);`

```
// ── Platform enable reset dance ─────────────────────────────
// rtw89 toggles PLATFORM_EN via write8 five times (set-clr-set-clr-set)
```

## L471 · `host::mmio_clr32(mmio, regs::R_AX_SYS_SDIO_CTRL, regs::B_AX_PCIE_CALIB_EN_V1);`

```
// ── PCIe: disable calibration ───────────────────────────────
```

## L474-475 · `host::mmio_set32(mmio, regs::R_AX_SYS_ADIE_PAD_PWR_CTRL,`

```
// ── ADIE PAD power + XTAL SI crystal init ───────────────────
// Without FLR, DDIE↔ADIE sync is intact → XTAL SI should work.
```

## L508 · `host::mmio_set32(mmio, regs::R_AX_PMC_DBG_CTRL2,`

```
// ── ISO control ─────────────────────────────────────────────
```

## L520-521 · `host::mmio_set32(mmio, regs::R_AX_DMAC_FUNC_EN,`

```
// ── DMAC Function Enable (full set from rtw8852b_pwr_on_func +
//    sys_init_ax dmac_func_en_ax, including B_AX_DMAC_CRPRT) ──
```

## L533-535 · `host::mmio_set32(mmio, regs::R_AX_DMAC_CLK_EN,`

```
// ── DMAC Clock Enable — Linux sys_init_ax:1664 ────────────────
// Enables clocks for all DMAC sub-blocks. Without these DMA/packet-
// in/dispatcher/wd_rls do not tick → RX DMA never completes.
```

## L542-546 · `host::mmio_set32(mmio, regs::R_AX_CK_EN,`

```
// ── CMAC Clock Enable — Linux cmac_func_en_ax:1624 ────────────
// Must come BEFORE CMAC_FUNC_EN. Enables clocks for RMAC, TMAC,
// PHYINTF, CMAC_DMA, Scheduler, PTCLTOP, CMAC. Without RMAC_CKEN
// and CMAC_DMA_CKEN the RX path from PHY → MAC → DMA is dead:
// frames may enter the radio but never reach the host ring.
```

## L553 · `host::mmio_set32(mmio, regs::R_AX_CMAC_FUNC_EN,`

```
// ── CMAC Function Enable (with B_AX_CMAC_CRPRT) ───────────────
```

## L561 · `host::mmio_w32_mask(mmio, regs::R_AX_EECS_EESK_FUNC_SEL,`

```
// ── Pinmux: EESK func = BT_LOG ─────────────────────────────
```

## L568-570 · `fn disable_cpu(mmio: i32) {`

```
// ═══════════════════════════════════════════════════════════════════
//  CPU control
// ═══════════════════════════════════════════════════════════════════
```

## L572-573 · `fn disable_cpu(mmio: i32) {`

```
/// Disable CPU — clean state before FWDL.
/// Based on rtw89_mac_disable_cpu / disable_cpu_ax.
```

## L575 · `host::mmio_clr32(mmio, regs::R_AX_PLATFORM_ENABLE, regs::B_AX_WCPU_EN);`

```
// Stop WCPU
```

## L578 · `host::mmio_clr32(mmio, regs::R_AX_WCPU_FW_CTRL,`

```
// Clear FW control state
```

## L582 · `host::mmio_clr32(mmio, regs::R_AX_SYS_CLK_CTRL, regs::B_AX_CPU_CLK_EN);`

```
// Stop CPU clock
```

## L585 · `host::mmio_clr32(mmio, regs::R_AX_PLATFORM_ENABLE, regs::B_AX_APB_WRAP_EN);`

```
// Toggle APB_WRAP (watchdog reset)
```

## L589 · `host::mmio_clr32(mmio, regs::R_AX_PLATFORM_ENABLE, regs::B_AX_PLATFORM_EN);`

```
// Toggle PLATFORM_EN
```

## L594 · `fn enable_cpu_fwdl(mmio: i32) {`

```
/// Enable CPU in FWDL mode — rtw89_mac_enable_cpu_ax(fwdl=true).
```

## L596 · `host::mmio_w32(mmio, regs::R_AX_UDM1, 0);`

```
// Clear UDM registers
```

## L600 · `host::mmio_w32(mmio, regs::R_AX_HALT_H2C_CTRL, 0);`

```
// Clear halt channels
```

## L606 · `host::mmio_set32(mmio, regs::R_AX_SYS_CLK_CTRL, regs::B_AX_CPU_CLK_EN);`

```
// Enable CPU clock
```

## L609 · `let mut val = host::mmio_r32(mmio, regs::R_AX_WCPU_FW_CTRL);`

```
// Set FW_CTRL: clear state, set FWDL_EN
```

## L612 · `val &= !((0x7u32) << 5); // clear FWDL_STS field`

```
// clear FWDL_STS field
```

## L616 · `val = host::mmio_r32(mmio, regs::R_AX_SEC_CTRL);`

```
// Set SEC_IDMEM_SIZE_CONFIG = 2
```

## L622-623 · `let aligned = regs::R_AX_BOOT_REASON & !0x3; // 0x01E4`

```
// Write boot reason = 0 (initial FW download, NOT 3=DLFW_RESUME)
// Linux: mac->fwdl_enable_wcpu(rtwdev, 0, true, false)
```

## L624 · `let aligned = regs::R_AX_BOOT_REASON & !0x3; // 0x01E4`

```
// 0x01E4
```

## L625 · `let shift = (regs::R_AX_BOOT_REASON & 0x2) * 8; // 16`

```
// 16
```

## L627 · `br &= !((0x7u32) << shift); // clear bits [2:0] → boot_reason = 0`

```
// clear bits [2:0] → boot_reason = 0
```

## L630 · `host::mmio_set32(mmio, regs::R_AX_PLATFORM_ENABLE, regs::B_AX_WCPU_EN);`

```
// Enable WCPU — boot ROM starts in FWDL mode
```

## L634-636 · `fn dmac_pre_init_dlfw(mmio: i32) -> bool {`

```
// ═══════════════════════════════════════════════════════════════════
//  PCIe DMA pre-init
// ═══════════════════════════════════════════════════════════════════
```

## L638-639 · `fn dmac_pre_init_dlfw(mmio: i32) -> bool {`

```
/// DLE + HFC initialization for firmware download mode.
/// Based on rtw89_mac_dmac_pre_init → hci_func_en + dmac_func_pre_en + dle_init(DLFW) + hfc_init.
```

## L643 · `let val = regs::B_AX_MAC_FUNC_EN | regs::B_AX_DMAC_FUNC_EN`

```
// ── hci_func_en_ax: write (NOT set) basic DMAC enables ──────
```

## L648 · `const B_AX_DISPATCHER_CLK_EN: u32 = 1 << 18;`

```
// ── dmac_func_pre_en_ax: enable dispatcher clock ────────────
```

## L652 · `host::mmio_clr32(mmio, regs::R_AX_DMAC_FUNC_EN,`

```
// ── dle_init(RTW89_QTA_DLFW) ────────────────────────────────
```

## L654 · `host::mmio_clr32(mmio, regs::R_AX_DMAC_FUNC_EN,`

```
// 1. Disable DLE
```

## L658 · `const B_AX_DLE_WDE_CLK_EN: u32 = 1 << 26;`

```
// 2. Enable DLE clocks
```

## L664-665 · `let mut wde = host::mmio_r32(mmio, regs::R_AX_WDE_PKTBUF_CFG);`

```
// 3. Configure WDE buffer: wde_size9 = {PG_64, lnk=0, unlnk=1024}
//    page_sel=0(64B), bound=0, free_page_num=0
```

## L668 · `host::mmio_w32(mmio, regs::R_AX_WDE_PKTBUF_CFG, wde);`

```
// page_sel=0, bound=0, free_pages=0
```

## L671-673 · `let mut ple = host::mmio_r32(mmio, regs::R_AX_PLE_PKTBUF_CFG);`

```
// 4. Configure PLE buffer: ple_size8 = {PG_128, lnk=64, unlnk=960}
//    WDE total = (0+1024)*64 = 65536, bound = 65536/8192 = 8
//    page_sel=1(128B), bound=8, free_page_num=64
```

## L676 · `ple |= 1;           // page_sel = 1 (128-byte pages)`

```
// page_sel = 1 (128-byte pages)
```

## L677 · `ple |= 8 << 8;      // start_bound = 8`

```
// start_bound = 8
```

## L678 · `ple |= 64 << 16;    // free_page_num = 64`

```
// free_page_num = 64
```

## L681-682 · `for i in 0u32..4 {`

```
// 5. Set quotas (DLFW mode: wde_qt4 all zeros, ple_qt13 minimal)
//    WDE: all 4 channels = {min=0, max=0}
```

## L684 · `host::mmio_w32(mmio, 0x8C40 + i * 4, 0); // R_AX_WDE_QTAn_CFG`

```
// R_AX_WDE_QTAn_CFG
```

## L686-687 · `let ple_qt: [u32; 11] = [`

```
//    PLE: qt13 = {0,0,16,48, 0,0,0,0, 0,0,0}
//    Format: min[11:0] | max[27:16]
```

## L689 · `0,                          // QTA0: mpdu  {min=0, max=0}`

```
// QTA0: mpdu  {min=0, max=0}
```

## L690 · `0,                          // QTA1: qtv   {min=0, max=0}`

```
// QTA1: qtv   {min=0, max=0}
```

## L691 · `(16 << 16) | 16,           // QTA2: cpuio {min=16, max=16}`

```
// QTA2: cpuio {min=16, max=16}
```

## L692 · `(48 << 16) | 48,           // QTA3: wcpu  {min=48, max=48}`

```
// QTA3: wcpu  {min=48, max=48}
```

## L693 · `0, 0, 0, 0, 0, 0, 0,      // QTA4-QTA10: all zero`

```
// QTA4-QTA10: all zero
```

## L699 · `host::mmio_set32(mmio, regs::R_AX_DMAC_FUNC_EN,`

```
// 6. Enable DLE
```

## L703 · `let mut wde_ok = false;`

```
// 7. Poll WDE ready (R_AX_WDE_INI_STATUS bits [1:0] = 0x3)
```

## L718 · `let mut ple_ok = false;`

```
// 8. Poll PLE ready (R_AX_PLE_INI_STATUS bits [1:0] = 0x3)
```

## L733-734 · `let mut hfc = host::mmio_r32(mmio, 0x8A00); // R_AX_HCI_FC_CTRL`

```
// ── hfc_init(reset=true, en=false, h2c_en=true) ────────────
// For DLFW: only enable CH12 (H2C) flow control, not full HFC
```

## L735 · `let mut hfc = host::mmio_r32(mmio, 0x8A00); // R_AX_HCI_FC_CTRL`

```
// R_AX_HCI_FC_CTRL
```

## L736 · `hfc &= !(1u32); // clear HCI_FC_EN`

```
// clear HCI_FC_EN
```

## L737 · `hfc |= 1 << 3;  // set HCI_FC_CH12_EN`

```
// set HCI_FC_CH12_EN
```

## L749-751 · `fn pcie_dma_pre_init(mmio: i32) -> Option<(i32, i32)> {`

```
/// Complete PCIe DMA pre-init matching Linux rtw89_pci_ops_mac_pre_init_ax.
/// Includes: PCIe helpers, DMA stop, mode_op, ALL ring setup + BDRAM, DMA enable.
/// Returns (ch12_ring_dma, ch12_data_dma) handles for firmware transfer.
```

## L755 · `host::mmio_clr32(mmio, 0x1008, 1 << 5);           // l1off_pwroff`

```
// ── PCIe pre-init helpers (Linux: called before DMA stop) ────
```

## L756 · `host::mmio_clr32(mmio, 0x1008, 1 << 5);           // l1off_pwroff`

```
// l1off_pwroff
```

## L757 · `host::mmio_clr32(mmio, regs::R_AX_SYS_PW_CTRL, 1 << 14); // aphy_pwrcut`

```
// aphy_pwrcut
```

## L758 · `host::mmio_set32(mmio, regs::R_AX_SYS_SDIO_CTRL, 1 << 15); // hci_ldo set`

```
// hci_ldo set
```

## L759 · `host::mmio_clr32(mmio, regs::R_AX_SYS_SDIO_CTRL, 1 << 14); // hci_ldo clr`

```
// hci_ldo clr
```

## L760 · `host::mmio_set32(mmio, 0x0074, 1 << 5);           // power_wake_ax`

```
// power_wake_ax
```

## L761 · `host::mmio_clr32(mmio, regs::R_AX_PCIE_EXP_CTRL, 1 << 4); // set_sic`

```
// set_sic
```

## L762 · `let mut lbc = host::mmio_r32(mmio, 0x11D8);       // set_lbc`

```
// set_lbc
```

## L765 · `host::mmio_set32(mmio, 0x11C0, 0x3);               // set_dbg`

```
// set_dbg
```

## L767 · `host::mmio_set32(mmio, regs::R_AX_PCIE_INIT_CFG1, (1 << 23) | (1 << 22)); // set_keep_reg`

```
// set_keep_reg
```

## L769 · `host::mmio_set32(mmio, regs::R_AX_PCIE_DMA_STOP1, 1 << 19);`

```
// ── 5b. Stop WPDMA ──────────────────────────────────────────
```

## L772 · `host::mmio_set32(mmio, regs::R_AX_PCIE_DMA_STOP1, 1 << 20);`

```
// ── 5c. ctrl_dma_all(false): stop PCIEIO + disable TXHCI/RXHCI
```

## L777 · `for i in 0..100 {`

```
// ── 5d. Poll DMA idle ───────────────────────────────────────
```

## L785 · `host::mmio_set32(mmio, regs::R_AX_TXBD_RWPTR_CLR1, 0x070F); // ACH0-3,CH8,CH9,CH12`

```
// ── 5e. Clear all ring indices ──────────────────────────────
```

## L786 · `host::mmio_set32(mmio, regs::R_AX_TXBD_RWPTR_CLR1, 0x070F); // ACH0-3,CH8,CH9,CH12`

```
// ACH0-3,CH8,CH9,CH12
```

## L787 · `host::mmio_set32(mmio, regs::R_AX_RXBD_RWPTR_CLR, 0x03);    // RXQ + RPQ`

```
// RXQ + RPQ
```

## L789 · `{`

```
// ── 5f. mode_op ─────────────────────────────────────────────
```

## L792 · `cfg1 &= !(1 << 18);     // clear RXBD_MODE`

```
// clear RXBD_MODE
```

## L793 · `cfg1 &= !(0x7 << 8);  cfg1 |= 7 << 8;   // TX burst = 2048B`

```
// TX burst = 2048B
```

## L794 · `cfg1 &= !(0x7 << 14); cfg1 |= 3 << 14;  // RX burst = 128B`

```
// RX burst = 128B
```

## L795 · `cfg1 |= 1 << 12;      // LATENCY_CONTROL`

```
// LATENCY_CONTROL
```

## L802 · `cfg2 &= !(0xF << 24); cfg2 |= 1 << 24;  // WD idle = 256ns`

```
// WD idle = 256ns
```

## L803 · `cfg2 &= !(0xF << 16); cfg2 |= 1 << 16;  // WD active = 256ns`

```
// WD active = 256ns
```

## L810-811 · `let dummy_dma = host::dma_alloc(1);`

```
// ── 5g. ops_reset: program ALL rings + BDRAM ────────────────
// Allocate DMA buffers: 1 shared page for dummy rings, 1 for CH12 ring, 2 for data
```

## L828-836 · `host::mmio_w32(mmio, regs::R_AX_ACH0_BDRAM_CTRL, 0x00020500);`

```
// TX rings: ACH0-ACH3, CH8, CH9 (dummy), CH12 (real)
// BDRAM table (rtw89_bd_ram_table_single):
//   ACH0: start=0, max=5, min=2  → 0x00020500
//   ACH1: start=5, max=5, min=2  → 0x00020505
//   ACH2: start=10, max=5, min=2 → 0x0002050A
//   ACH3: start=15, max=5, min=2 → 0x0002050F
//   CH8:  start=20, max=4, min=1 → 0x00010414
//   CH9:  start=24, max=4, min=1 → 0x00010418
//   CH12: start=28, max=4, min=1 → 0x0001041C
```

## L838 · `host::mmio_w32(mmio, regs::R_AX_ACH0_BDRAM_CTRL, 0x00020500);`

```
// ACH0
```

## L842 · `host::mmio_w32(mmio, regs::R_AX_ACH1_BDRAM_CTRL, 0x00020505);`

```
// ACH1
```

## L846 · `host::mmio_w32(mmio, regs::R_AX_ACH2_BDRAM_CTRL, 0x0002050A);`

```
// ACH2
```

## L850 · `host::mmio_w32(mmio, regs::R_AX_ACH3_BDRAM_CTRL, 0x0002050F);`

```
// ACH3
```

## L854 · `host::mmio_w32(mmio, regs::R_AX_CH8_BDRAM_CTRL, 0x00010414);`

```
// CH8
```

## L858 · `host::mmio_w32(mmio, regs::R_AX_CH9_BDRAM_CTRL, 0x00010418);`

```
// CH9
```

## L863 · `host::mmio_w32(mmio, regs::R_AX_CH12_BDRAM_CTRL, 0x0001041C);`

```
// CH12 — FWCMD queue (the one we actually use)
```

## L869-871 · `let rxq_dma = host::dma_alloc(33);`

```
// ── RXQ: allocate REAL ring (not dummy!) ───────────────────────
// Linux allocates rings during probe, BEFORE BDRAM reset.
// 33 pages: page 0 = BD ring, pages 1-32 = data buffers
```

## L877 · `for i in 0u32..32 {`

```
// Pre-fill 32 RX BDs pointing to data buffers
```

## L880 · `host::dma_w32(rxq_dma, i * 8, 4096);          // buf_size=4096, opt=0`

```
// buf_size=4096, opt=0
```

## L881 · `host::dma_w32(rxq_dma, i * 8 + 4, buf_phys as u32); // DMA addr`

```
// DMA addr
```

## L885-887 · `host::mmio_w16(mmio, regs::R_AX_RXQ_RXBD_NUM, 32);`

```
// Program RXQ + RPQ NUM as SEPARATE write16 (Linux rtw89_pci_reset_trx_rings).
// 0x1020 = RXQ_RXBD_NUM, 0x1022 = RPQ_RXBD_NUM. A combined write32 on 0x1020
// would stomp on HW-owned fields in the adjacent register.
```

## L896 · `unsafe { RXQ_DMA = rxq_dma; }`

```
// Save RXQ handle for mac.rs scan polling
```

## L902 · `if VERBOSE {`

```
// DEBUG: readback DESA immediately after write (before any reset)
```

## L910 · `unsafe { BD_IDX = 0; }`

```
// Reset BD_IDX for CH12
```

## L913 · `host::mmio_set32(mmio, regs::R_AX_PCIE_INIT_CFG1, regs::B_AX_RST_BDRAM);`

```
// ── 5h. BDRAM reset ─────────────────────────────────────────
```

## L922 · `if VERBOSE {`

```
// DEBUG: readback DESA after BDRAM reset
```

## L930-931 · `host::mmio_set32(mmio, regs::R_AX_PCIE_DMA_STOP1, 0x00070F00); // TX_STOP1_MASK_V1`

```
// NO RXQ IDX write here. 8852BE has rx_ring_eq_is_full=false in Linux,
// meaning wp=0 and the IDX register is left alone after BDRAM reset.
```

## L933 · `host::mmio_set32(mmio, regs::R_AX_PCIE_DMA_STOP1, 0x00070F00); // TX_STOP1_MASK_V1`

```
// ── 5i. Stop all TX channels ────────────────────────────────
```

## L934 · `host::mmio_set32(mmio, regs::R_AX_PCIE_DMA_STOP1, 0x00070F00); // TX_STOP1_MASK_V1`

```
// TX_STOP1_MASK_V1
```

## L936 · `host::mmio_clr32(mmio, regs::R_AX_PCIE_DMA_STOP1, regs::B_AX_STOP_CH12);`

```
// ── 5j. Enable CH12 only ────────────────────────────────────
```

## L939 · `host::mmio_clr32(mmio, regs::R_AX_PCIE_DMA_STOP1, 1 << 20); // clear STOP_PCIEIO`

```
// ── 5k. ctrl_dma_all(true): clear PCIEIO + enable TXHCI/RXHCI
```

## L940 · `host::mmio_clr32(mmio, regs::R_AX_PCIE_DMA_STOP1, 1 << 20); // clear STOP_PCIEIO`

```
// clear STOP_PCIEIO
```

## L947 · `const H2C_CAT_MAC: u32 = 1;`

```
// H2C header constants for FWDL (from Linux fw.h)
```

## L950 · `static mut H2C_SEQ: u8 = 0;`

```
// H2C_FUNC_MAC_FWHDR_DL = 0
```

## L952 · `static mut H2C_SEQ: u8 = 0;`

```
/// H2C sequence counter
```

## L955-958 · `fn send_fw_header(ring_dma: i32, data_dma: i32, mmio: i32, hdr_len: usize) {`

```
/// Send firmware HEADER via CH12 with WD + H2C descriptor.
/// Linux: rtw89_pci_fwcmd_submit prepends 24-byte WD body,
///        rtw89_h2c_pkt_set_hdr_fwdl prepends 8-byte H2C header.
/// Buffer layout: [WD 24B][H2C 8B][FW header data]
```

## L961 · `let h2c_payload = 8 + hdr_len;           // H2C total_len = hdr + payload`

```
// H2C total_len = hdr + payload
```

## L962 · `let dma_total = WD_BODY_SIZE + h2c_payload; // WD + H2C + data`

```
// WD + H2C + data
```

## L964-965 · `host::dma_w32(data_dma, 0, WD_DWORD0_FWCMD_HDR);`

```
// ── WiFi Descriptor (24 bytes) ─────────────────────────────────
// dword0: ch_dma=12, fw_dl=0 (header download uses fw_dl=false)
```

## L967 · `host::dma_w32(data_dma, 4, 0);       // dword1 = 0`

```
// dword1 = 0
```

## L968 · `host::dma_w32(data_dma, 8, (h2c_payload as u32) & 0x3FFF); // dword2: PKT_SIZE`

```
// dword2: PKT_SIZE
```

## L969 · `host::dma_w32(data_dma, 12, 0);      // dword3 = 0`

```
// dword3 = 0
```

## L970 · `host::dma_w32(data_dma, 16, 0);      // dword4 = 0`

```
// dword4 = 0
```

## L971 · `host::dma_w32(data_dma, 20, 0);      // dword5 = 0`

```
// dword5 = 0
```

## L973 · `let seq = unsafe { H2C_SEQ };`

```
// ── H2C descriptor (8 bytes) after WD ──────────────────────────
```

## L978 · `let hdr1: u32 = (h2c_payload as u32) & 0x3FFF; // TOTAL_LEN = H2C hdr + data`

```
// TOTAL_LEN = H2C hdr + data
```

## L982 · `host::dma_write_buf(data_dma, (WD_BODY_SIZE + 8) as u32, &fw_data()[..hdr_len]);`

```
// ── FW header data after WD + H2C ──────────────────────────────
```

## L988 · `if VERBOSE {`

```
// Debug: dump first 48 bytes of DMA buffer (WD + H2C + start of FW header)
```

## L999 · `submit_bd(ring_dma, data_dma, data_phys, mmio, dma_total);`

```
// BD length = total DMA buffer size (WD + H2C + data)
```

## L1003-1006 · `fn send_fw_section(ring_dma: i32, data_dma: i32, mmio: i32, offset: usize, len: usize) {`

```
/// Send a firmware SECTION chunk via CH12 — WD + raw data, NO H2C descriptor.
/// Linux: rtw89_pci_fwcmd_submit prepends 24-byte WD body.
///        Section data uses fw_dl=1 (no H2C header).
/// Buffer layout: [WD 24B][section data]
```

## L1010-1011 · `host::dma_w32(data_dma, 0, WD_DWORD0_FWCMD_BODY);`

```
// ── WiFi Descriptor (24 bytes) ─────────────────────────────────
// dword0: ch_dma=12, fw_dl=1 (section download uses fw_dl=true)
```

## L1013 · `host::dma_w32(data_dma, 4, 0);       // dword1 = 0`

```
// dword1 = 0
```

## L1014 · `host::dma_w32(data_dma, 8, (len as u32) & 0x3FFF); // dword2: PKT_SIZE`

```
// dword2: PKT_SIZE
```

## L1015 · `host::dma_w32(data_dma, 12, 0);      // dword3 = 0`

```
// dword3 = 0
```

## L1016 · `host::dma_w32(data_dma, 16, 0);      // dword4 = 0`

```
// dword4 = 0
```

## L1017 · `host::dma_w32(data_dma, 20, 0);      // dword5 = 0`

```
// dword5 = 0
```

## L1019 · `host::dma_write_buf(data_dma, WD_BODY_SIZE as u32, &fw_data()[offset..offset + len]);`

```
// ── Raw firmware data after WD, no H2C header ──────────────────
```

## L1023 · `submit_bd(ring_dma, data_dma, data_phys, mmio, WD_BODY_SIZE + len);`

```
// BD length = WD + section data
```

## L1027-1029 · `fn submit_bd(ring_dma: i32, _data_dma: i32, data_phys: u64, mmio: i32, total_len: usize) {`

```
/// Submit a TX BD to the CH12 ring, advance write pointer, and wait for DMA completion.
/// Polls CH12_TXBD_IDX until HW_IDX advances, ensuring the DMA engine finished reading
/// our data buffer before we overwrite it for the next chunk.
```

## L1040-1044 · `host::mmio_w16(mmio, regs::R_AX_CH12_TXBD_IDX, new_idx);`

```
// CRITICAL: use 16-bit RMW write to preserve HW_IDX in upper 16 bits!
// Linux: rtw89_write16(rtwdev, addr.idx, wp) — only writes HOST_IDX.
// mmio_w32 would zero HW_IDX, causing DMA to reprocess old BDs = data corruption.
// Use mmio_w16 (RMW) — RTL8852B does NOT ignore upper 16 bits on w32!
// Linux uses writew (true 16-bit write). RMW race is negligible during FWDL.
```

## L1047 · `for _ in 0..500u32 {`

```
// Wait for DMA engine to process this BD (HW_IDX == new HOST_IDX)
```

## L1057 · `const FW_SEC_TYPE_BB: u8 = 9; // RTW89_FW_SEC_TYPE_BB — skip during normal FWDL`

```
/// Section type constants (from Linux rtw89 fw.h)
```

## L1058 · `const FW_SEC_TYPE_BB: u8 = 9; // RTW89_FW_SEC_TYPE_BB — skip during normal FWDL`

```
// RTW89_FW_SEC_TYPE_BB — skip during normal FWDL
```

## L1060-1063 · `fn send_firmware_sections(mmio: i32, ring_dma: i32, data_dma: i32, body_offset: usize) {`

```
/// Send firmware sections individually, skipping BB (baseband) sections.
/// Linux: rtw89_fw_download_main iterates sections, __rtw89_fw_download_main
/// sends each in 2020-byte chunks. BB sections (type=9) are skipped when
/// include_bb=false (which is the normal FWDL path).
```

## L1067 · `let w6 = u32::from_le_bytes([fw[0x18], fw[0x19], fw[0x1A], fw[0x1B]]);`

```
// Parse section headers from the FW header (after 32-byte base header)
```

## L1071 · `let mut data_offset = body_offset; // where section data starts in fw blob`

```
// where section data starts in fw blob
```

## L1079 · `let shdr = 32 + s * 16; // section header offset in fw blob`

```
// section header offset in fw blob
```

## L1081 · `let sec_len = (w1 & 0x00FFFFFF) as usize; // bits [23:0]`

```
// bits [23:0]
```

## L1082 · `let sec_type = ((w1 >> 24) & 0xF) as u8;  // bits [27:24]`

```
// bits [27:24]
```

## L1084-1085 · `let mut sent = 0usize;`

```
// Note: Linux skips BB (type=9) with include_bb=false, but the boot ROM
// still expects all section data (STS=1 if BB is skipped). Send everything.
```

## L1087 · `let mut sent = 0usize;`

```
// Send this section in FW_CHUNK_SIZE chunks
```

## L1105-1107 · `fn wait_fwdl_path_ready(mmio: i32) -> bool {`

```
// ═══════════════════════════════════════════════════════════════════
//  Polling helpers
// ═══════════════════════════════════════════════════════════════════
```

## L1109 · `fn wait_fwdl_path_ready(mmio: i32) -> bool {`

```
/// Wait for FWDL path ready (bit 2 in WCPU_FW_CTRL).
```

## L1134-1140 · `fn fw_header_info() -> (usize, usize) {`

```
/// Parse firmware header length from binary.
/// Linux: base_hdr = sizeof(fw_hdr) + section_num * sizeof(fw_hdr_section)
///      = 32 + section_num * 16
/// section_num is in FW_HDR word 6, bits [15:8].
/// Parse firmware header and return (send_len, body_offset).
/// send_len = base header to send to chip (WITHOUT dynamic header).
/// body_offset = where firmware sections start (AFTER full header).
```

## L1155 · `(base_hdr_len, full_hdr)`

```
// Linux: sends base_hdr only, body starts after full header
```

## L1172-1173 · `fn wait_fw_ready(mmio: i32) -> bool {`

```
/// Wait for firmware ready — FWDL_STS in WCPU_FW_CTRL bits [7:5] == 7.
/// Linux: rtw89_fw_check_rdy polls for RTW89_FWDL_WCPU_FW_INIT_RDY (7).
```

## L1175 · `let host_idx = unsafe { BD_IDX };`

```
// Wait for DMA to finish processing all BDs first
```

## L1190 · `if fwdl_sts == 7 { // FWDL_WCPU_FW_INIT_RDY`

```
// FWDL_WCPU_FW_INIT_RDY
```

## L1198 · `if fwdl_sts >= 2 && fwdl_sts <= 5 { // error states`

```
// error states
```

## L1205-1207 · `if VERBOSE {`

```
// Track BOOT_DBG progress — verbose-only. In steady-state the
// same 6 "STS=6 DBG=0x..." lines appear every boot and are
// uninteresting; keep for debugging boot regressions.
```

## L1225-1227 · `const VERBOSE: bool = false;`

```
// ═══════════════════════════════════════════════════════════════════
//  Debug helpers
// ═══════════════════════════════════════════════════════════════════
```

## L1229-1231 · `const VERBOSE: bool = false;`

```
/// Dump key register state for debugging. Gated by `VERBOSE` — these
/// PW/FW/PLAT checkpoints were useful during the v0.85-era pwr_off/
/// pwr_on bring-up; once the sequence is stable they are noise.
```

## L1246-1250 · `pub fn h2c_send(mmio: i32, cat: u8, class: u8, func: u8, rack: bool, dack: bool, payload: &[u8]) {`

```
/// Send an H2C command via CH12 (reuses FWDL ring).
/// Payload is raw H2C body (without WD or H2C header).
/// `rack` = request REC_ACK (auto-forced when seq % 4 == 0 for RTW89_CHIP_AX).
/// `dack` = request DONE_ACK from FW (sets H2C_HDR_DONE_ACK bit 15 in hdr1).
/// Linux rtw89_h2c_pkt_set_hdr (fw.c:1564).
```

## L1257 · `let h2c_len = 8 + payload.len(); // H2C header + payload`

```
// H2C header + payload
```

## L1260 · `host::dma_w32(data_dma, 0, WD_DWORD0_FWCMD_HDR);`

```
// WiFi Descriptor (24B): ch_dma=12, fw_dl=0
```

## L1268 · `let seq = unsafe { H2C_SEQ };`

```
// H2C header (8B) — Linux rtw89_h2c_pkt_set_hdr (fw.c:1564)
```

## L1270 · `let rack = rack || (seq % 4 == 0);`

```
// RTW89_CHIP_AX && (seq % 4 == 0) → rack forced true (fw.c:1573)
```

## L1272 · `let hdr0: u32 = (cat as u32 & 0x3)`

```
// hdr0: CAT[1:0] | CLASS[7:2] | FUNC[15:8] | DEL_TYPE[19:16]=0 | SEQ[31:24]
```

## L1277 · `let mut hdr1: u32 = (h2c_len as u32) & 0x3FFF;`

```
// hdr1: TOTAL_LEN[13:0] | REC_ACK[14] | DONE_ACK[15]
```

## L1284 · `if !payload.is_empty() {`

```
// Payload
```

## L1302-1307 · `const H2C_CL_FW_INFO:    u8 = 0x0;`

```
// ═══════════════════════════════════════════════════════════════════
//  h2c_fw_log — 1:1 Linux rtw89_fw_h2c_fw_log (fw.c:2787).
//  Enables FW firmware log with level=LOUD routed to C2H, with
//  INIT/TASK/PS/ERROR/MLO/SCAN components. Without this the FW is
//  silent and we can't diagnose init failures from its side.
// ═══════════════════════════════════════════════════════════════════
```

## L1312-1318 · `const H2CREG_FUNC_SCH_TX_EN: u8 = 5;`

```
// ═══════════════════════════════════════════════════════════════════
//  H2CREG / C2HREG — fast register-based FW channel.
//  1:1 port of Linux rtw89_fw_write_h2c_reg + rtw89_fw_read_c2h_reg
//  (fw.c:8048, 8082). Used by stop_sch_tx / resume_sch_tx and similar
//  commands where the FW is expected to answer synchronously via the
//  C2HREG DATA registers. Distinct from the CH12 DMA H2C path.
// ═══════════════════════════════════════════════════════════════════
```

## L1320 · `const H2CREG_FUNC_SCH_TX_EN: u8 = 5;`

```
/// H2CREG function IDs (Linux enum rtw89_mac_h2c_type, fw.h:146).
```

## L1323 · `const C2HREG_FUNC_TX_PAUSE_RPT: u8 = 4;`

```
/// C2HREG function IDs (Linux enum rtw89_mac_c2h_type, fw.h:160).
```

## L1326-1330 · `fn h2creg_write(mmio: i32, id: u8, content_len: u8,`

```
/// Write 4 × u32 H2CREG DATA, with header word already packed into w0.
/// Header packing (Linux fw.c:8068):
///   w0[6:0]  = id
///   w0[11:8] = len in dwords = DIV_ROUND_UP(content_len + 2, 4)
/// Returns false on timeout waiting for the previous H2CREG to drain.
```

## L1333-1335 · `let mut ready = false;`

```
// Step 1: poll ctrl == 0 (FW consumed previous command).
// Linux: read_poll_timeout(rtw89_read8, val, val == 0, 1000, 5000, ...).
// Our sleep granularity is ms; do 5 × sleep_ms(1) approx the 5ms budget.
```

## L1344-1345 · `}`

```
// Linux also warns + returns error; we proceed to match the
// effect of the Linux warning path (does not abort the caller).
```

## L1348 · `let total = content_len as u32 + 2;`

```
// Step 2: header bits. len = DIV_ROUND_UP(content_len + HDR_LEN(2), 4).
```

## L1353 · `host::mmio_w32(mmio, regs::R_AX_H2CREG_DATA0,      w0);`

```
// Step 3: write all 4 data registers.
```

## L1359-1360 · `unsafe {`

```
// Step 4: bump HALMAC_H2C_DEQ_CNT byte counter (Linux fw.c:8075).
//   byte @ 0x01F5 bits [3:0]. RMW 32-bit word (aligned at 0x01F4).
```

## L1364 · `let aligned = regs::R_AX_UDM1;        // 0x01F4`

```
// 0x01F4
```

## L1365 · `let shift = (regs::R_AX_HALMAC_CNT_BYTE - aligned) * 8; // 8`

```
// 8
```

## L1372 · `host::mmio_set8(mmio, regs::R_AX_H2CREG_CTRL, regs::B_AX_H2CREG_TRIGGER);`

```
// Step 5: trigger — write byte 0x01 to R_AX_H2CREG_CTRL.
```

## L1377-1379 · `fn c2hreg_read(mmio: i32, timeout_ms: u32) -> Option<(u8, u32, u32, u32, u32)> {`

```
/// Read a C2HREG reply. Returns `Some((id, w0, w1, w2, w3))` on success.
/// Linux fw.c:8082 rtw89_fw_read_c2h_reg — poll ctrl != 0, drain 4 dwords,
/// ACK by writing 0 to ctrl, parse id from hdr.w0[6:0].
```

## L1396 · `host::mmio_clr8(mmio, regs::R_AX_C2HREG_CTRL, 0xFF);`

```
// ACK: clear ctrl byte so FW can send next reply.
```

## L1401 · `unsafe {`

```
// Bump C2H counter byte (upper nibble @ 0x01F5).
```

## L1417-1426 · `pub fn h2creg_sch_tx_en(mmio: i32, band: u8, tx_en: u16, mask: u16) -> bool {`

```
/// 1:1 Linux rtw89_hw_sch_tx_en_h2c (mac.c:3238).
/// Sends SCH_TX_EN via H2CREG — FW updates R_AX_CTN_TXEN on the target band
/// and replies via C2HREG with FUNC_TX_PAUSE_RPT. Must be used whenever FW
/// is ready (Linux uses direct reg write only for pre-FW path).
///
/// Struct `rtw89_h2creg_sch_tx_en`:
///   w0[31:16] = EN    (tx_en bits to set/clear)
///   w1[15:0]  = MASK  (which bits of EN are valid)
///   w1[16]    = BAND  (mac_idx 0 or 1)
/// content_len = 6 (8 struct bytes - 2 header bytes).
```

## L1433 · `match c2hreg_read(mmio, 1000) {`

```
// Linux waits up to RTW89_C2H_TIMEOUT = 1_000_000us (1 s); we match.
```

## L1449-1451 · `pub fn stop_sch_tx(mmio: i32, band: u8) -> u16 {`

```
/// Save current R_AX_CTN_TXEN bits and clear all via SCH_TX_EN H2CREG.
/// Mirrors Linux rtw89_mac_stop_sch_tx (mac.c:3303) for SCH_TX_SEL_ALL.
/// Returns the saved tx_en bits to be passed back into resume_sch_tx.
```

## L1453 · `let aligned = regs::R_AX_CTN_TXEN & !0x3;`

```
// Linux reads R_AX_CTN_TXEN directly for the caller's save buffer.
```

## L1459 · `h2creg_sch_tx_en(mmio, band, 0, regs::B_AX_CTN_TXEN_ALL_MASK);`

```
// FW path: always when chip is running (our case).
```

## L1464 · `pub fn resume_sch_tx(mmio: i32, band: u8, tx_en: u16) {`

```
/// Restore previously-saved tx_en bits. Linux rtw89_mac_resume_sch_tx.
```

## L1469-1483 · `pub fn h2c_add_pkt_offload(mmio: i32, pkt_id: u8, frame: &[u8]) {`

```
/// Register a packet in the FW's offload pool. 1:1 Linux fw.c:6307
/// rtw89_fw_h2c_add_pkt_offload.
///
/// The FW stores the packet body keyed by `pkt_id`. The scan channel-list
/// H2C then references this ID in its `pkt_id[]` array — whenever the FW
/// lands on that channel during an active scan, it transmits the stored
/// frame on air (via its own TX path, bypassing our CH8 DMA entirely).
///
/// Payload layout (H2C_LEN_PKT_OFLD = 4 bytes + frame):
///   w0[7:0]   = PKT_IDX  (caller-allocated slot, we use 0)
///   w0[10:8]  = PKT_OP   = RTW89_PKT_OFLD_OP_ADD (1)
///   w0[31:16] = PKT_LENGTH (frame.len())
///   bytes 4..   = raw packet
///
/// CAT=1 (MAC), CLASS=9 (MAC_FW_OFLD), FUNC=1 (PACKET_OFLD), rack=1, dack=1.
```

## L1489 · `| (1u32 << 8)                      // PKT_OP = OP_ADD`

```
// PKT_OP = OP_ADD
```

## L1490 · `| ((n as u32) << 16);              // PKT_LENGTH`

```
// PKT_LENGTH
```

## L1496-1497 · `pub fn h2c_fw_log(mmio: i32, enable: bool) {`

```
/// Send LOG_CFG H2C to enable FW trace log via C2H channel.
/// `enable=false` → COMP=0 (effectively off), `enable=true` → default comp set.
```

## L1499 · `let comp: u32 = if enable {`

```
// Linux: RTW89_FW_LOG_COMP_{INIT=1, TASK=2, PS=11, ERROR=12, MLO=26, SCAN=28}
```

## L1504-1507 · `let w0: u32 = 4u32 | (0x02u32 << 8);`

```
// w0[7:0]  = LEVEL  = RTW89_FW_LOG_LEVEL_LOUD (4)
// w0[15:8] = PATH   = BIT(RTW89_FW_LOG_LEVEL_C2H=1) = 0x02
// w1[31:0] = COMP
// w2[31:0] = COMP_EXT = 0
```

## L1517 · `h2c_send(mmio, H2C_CAT_MAC as u8, H2C_CL_FW_INFO, H2C_FUNC_LOG_CFG,`

```
// Linux: rtw89_h2c_pkt_set_hdr(..., rack=0, dack=0, ...)
```

