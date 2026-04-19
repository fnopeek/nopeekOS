//! RTL8852BE register definitions (from rtw89 Linux driver)

// ── System / Power ───────────────────────────────────────────────

pub const R_AX_SYS_ISO_CTRL: u32      = 0x0000;
pub const R_AX_SYS_FUNC_EN: u32       = 0x0002;
pub const R_AX_SYS_PW_CTRL: u32       = 0x0004;
pub const R_AX_SYS_CLK_CTRL: u32      = 0x0008;
pub const B_AX_CPU_CLK_EN: u32        = 1 << 14;
pub const R_AX_SYS_SWR_CTRL1: u32     = 0x0010;
pub const R_AX_SYS_ADIE_PAD_PWR_CTRL: u32 = 0x0018;
pub const R_AX_SYS_AFE_LDO_CTRL: u32  = 0x0020;
pub const R_AX_GPIO_MUXCFG: u32       = 0x0040;
pub const R_AX_SYS_SDIO_CTRL: u32     = 0x0070;
pub const R_AX_PLATFORM_ENABLE: u32   = 0x0088;
pub const R_AX_WLLPS_CTRL: u32        = 0x0090;
pub const R_AX_PMC_DBG_CTRL2: u32     = 0x00CC;
pub const R_AX_SYS_CFG1: u32          = 0x00F0;
pub const R_AX_SYS_STATUS1: u32       = 0x00F4;
pub const R_AX_SPS_DIG_ON_CTRL0: u32  = 0x0200;
pub const R_AX_WLAN_XTAL_SI_CTRL: u32 = 0x0270;
pub const R_AX_EECS_EESK_FUNC_SEL: u32 = 0x02D8;
pub const R_AX_WLRF_CTRL: u32         = 0x02F0;
pub const R_AX_SPS_DIG_OFF_CTRL0: u32 = 0x0400;

// ── Firmware / CPU Control ───────────────────────────────────────

pub const R_AX_HALT_H2C_CTRL: u32     = 0x0160;
pub const R_AX_HALT_C2H_CTRL: u32     = 0x0164;
pub const R_AX_HALT_H2C: u32          = 0x0168;
pub const R_AX_HALT_C2H: u32          = 0x016C;
pub const R_AX_WCPU_FW_CTRL: u32      = 0x01E0;
pub const R_AX_BOOT_REASON: u32       = 0x01E6;
pub const RTW89_FW_DLFW_RESUME: u32   = 3; // firmware download boot reason

// Additional registers from rtw89_mac_enable_cpu_ax
pub const R_AX_UDM1: u32              = 0x01F4;
pub const R_AX_UDM2: u32              = 0x01F8;
// H2C/C2H counter byte at R_AX_UDM1+1 (= 0x01F5):
//   [3:0] = HALMAC_H2C_DEQ_CNT   (GENMASK(11,8) in the 32-bit UDM1 reg)
//   [7:4] = HALMAC_C2H_ENQ_CNT   (GENMASK(15,12))
// Linux updates the counter on every h2creg / c2hreg transaction (chip info
// rtw8852b.c:1031 .h2c_counter_reg / .c2h_counter_reg).
pub const R_AX_HALMAC_CNT_BYTE: u32   = 0x01F5;
pub const B_HALMAC_H2C_DEQ_CNT: u8    = 0x0F;
pub const B_HALMAC_C2H_ENQ_CNT: u8    = 0xF0;

// H2CREG / C2HREG — fast register-based H2C channel to FW (used by
// sch_tx_en, get_feature, etc.; not the CH12 DMA H2C).
pub const R_AX_H2CREG_DATA0: u32      = 0x8140;
pub const R_AX_H2CREG_CTRL:  u32      = 0x8160;
pub const B_AX_H2CREG_TRIGGER: u8     = 1 << 0;
pub const R_AX_C2HREG_DATA0: u32      = 0x8150;
pub const R_AX_C2HREG_CTRL:  u32      = 0x8164;

// CMAC TX enable register — written by rtw89_mac_stop_sch_tx /
// rtw89_mac_resume_sch_tx via H2CREG SCH_TX_EN when FW is ready.
pub const R_AX_CTN_TXEN: u32          = 0xC348;
pub const B_AX_CTN_TXEN_ALL_MASK: u16 = 0xFFFF;

// PCIe interrupt mask registers — rtw89_pci_enable_intr (pci.c:853).
// Writing the IMRs unmasks the matching IRQ sources; required as part of
// rtw89_hci_start / rtw89_pci_ops_start (pci.c:1922) at the end of
// rtw89_core_start. Without this, the RX side may remain gated.
pub const R_AX_HIMR0:        u32 = 0x01A0;
pub const R_AX_HISR0:        u32 = 0x01A4;
pub const R_AX_PCIE_HIMR00:  u32 = 0x10B0;
pub const R_AX_PCIE_HISR00:  u32 = 0x10B4;
pub const R_AX_PCIE_HIMR10:  u32 = 0x13B0;
pub const R_AX_PCIE_HISR10:  u32 = 0x13B4;

// HIMR0 bits (pci.h:160)
pub const B_AX_HALT_C2H_INT_EN:     u32 = 1 << 21;

// PCIE_HIMR00 bits (pci.h:180)
pub const B_AX_HS0ISR_IND_INT_EN:   u32 = 1 << 24;
pub const B_AX_RPQBD_FULL_INT_EN:   u32 = 1 << 20;
pub const B_AX_RDU_INT_EN:          u32 = 1 << 19;
pub const B_AX_RXDMA_STUCK_INT_EN:  u32 = 1 << 18;
pub const B_AX_TXDMA_STUCK_INT_EN:  u32 = 1 << 17;
pub const B_AX_RPQDMA_INT_EN:       u32 = 1 << 2;
pub const B_AX_RXP1DMA_INT_EN:      u32 = 1 << 1;
pub const B_AX_RXDMA_INT_EN:        u32 = 1 << 0;

// PCIE_HIMR10 bits
pub const B_AX_HC10ISR_IND_INT_EN:  u32 = 1 << 28;
pub const R_AX_SEC_CTRL: u32          = 0x0C00;
pub const B_AX_SEC_IDMEM_MASK: u32    = 0x3 << 16;
pub const B_AX_BOOT_REASON_MASK: u32  = 0x7; // bits [2:0] at offset 0x01E6

// ── PCIe / DMA ───────────────────────────────────────────────────

pub const R_AX_PCIE_INIT_CFG1: u32    = 0x1000;
pub const R_AX_HAXI_INIT_CFG1: u32    = 0x1000;
pub const R_AX_HAXI_DMA_STOP1: u32    = 0x1010;
pub const R_AX_HAXI_DMA_BUSY1: u32    = 0x101C;

// TX BD ring addresses — from Linux rtw89_pci_ch_dma_addr_set
// Format per channel: NUM, IDX, BDRAM_CTRL, DESA_L, DESA_H
//
// 8852B unmasked TX channels: ACH0-ACH3, CH8, CH9, CH12
// (ACH4-ACH7, CH10, CH11 are masked via tx_dma_ch_mask)
pub const R_AX_ACH0_TXBD_NUM: u32     = 0x1024;
pub const R_AX_ACH0_BDRAM_CTRL: u32   = 0x1200;
pub const R_AX_ACH0_TXBD_DESA_L: u32  = 0x1110;
pub const R_AX_ACH0_TXBD_DESA_H: u32  = 0x1114;

pub const R_AX_ACH1_BDRAM_CTRL: u32   = 0x1204;
pub const R_AX_ACH1_TXBD_DESA_L: u32  = 0x1118;

pub const R_AX_ACH2_BDRAM_CTRL: u32   = 0x1208;
pub const R_AX_ACH2_TXBD_DESA_L: u32  = 0x1120;

pub const R_AX_ACH3_BDRAM_CTRL: u32   = 0x120C;
pub const R_AX_ACH3_TXBD_DESA_L: u32  = 0x1128;

pub const R_AX_CH8_BDRAM_CTRL: u32    = 0x1220;
pub const R_AX_CH8_TXBD_DESA_L: u32   = 0x1150;

pub const R_AX_CH9_BDRAM_CTRL: u32    = 0x1224;
pub const R_AX_CH9_TXBD_DESA_L: u32   = 0x1158;

// CH12 = FWCMD queue — correct addresses from Linux!
pub const R_AX_CH12_TXBD_NUM: u32     = 0x1038;
pub const R_AX_CH12_TXBD_IDX: u32     = 0x1080;
pub const R_AX_CH12_BDRAM_CTRL: u32   = 0x1228;
pub const R_AX_CH12_TXBD_DESA_L: u32  = 0x1160;
pub const R_AX_CH12_TXBD_DESA_H: u32  = 0x1164;

// RX BD ring addresses (Linux rtw89 reg.h + rtw89_pci_ch_dma_addr_set)
pub const R_AX_RXQ_RXBD_NUM: u32      = 0x1020;
pub const R_AX_RPQ_RXBD_NUM: u32      = 0x1022;
pub const R_AX_RXQ_RXBD_IDX: u32      = 0x1050;
pub const R_AX_RPQ_RXBD_IDX: u32      = 0x1054;
pub const R_AX_RXQ_RXBD_DESA_L: u32   = 0x1100;
pub const R_AX_RXQ_RXBD_DESA_H: u32   = 0x1104;
pub const R_AX_RPQ_RXBD_DESA_L: u32   = 0x1108;
pub const R_AX_RPQ_RXBD_DESA_H: u32   = 0x110C;

// ── PCIe Configuration ──────────────────────────────────────────

pub const R_AX_PCIE_INIT_CFG2: u32     = 0x1004;
pub const R_AX_PCIE_EXP_CTRL: u32     = 0x13F0;
pub const B_AX_MAX_TAG_NUM_MASK: u32   = 0x7 << 16; // GENMASK(18,16)
pub const R_AX_TX_ADDR_INFO_MODE: u32 = 0x8810;
pub const R_AX_PKTIN_SETTING: u32     = 0x9A00;

// ── LTR / Power Management ──────────────────────────────────────

pub const R_AX_LTR_DEC_CTRL: u32      = 0x1600;
pub const R_AX_LTR_CTRL_0: u32        = 0x8410;
pub const R_AX_LTR_CTRL_1: u32        = 0x8414;

// ── HCI / DMAC / CMAC Function Enable ────────────────────────────

pub const R_AX_HCI_FUNC_EN: u32       = 0x8380;
pub const R_AX_DMAC_FUNC_EN: u32      = 0x8400;
pub const R_AX_DMAC_CLK_EN: u32       = 0x8404;
pub const R_AX_HD0IMR: u32            = 0x8110;
pub const R_AX_HD0ISR: u32            = 0x8114;
pub const R_AX_DMAC_ERR_IMR: u32      = 0x8520;
pub const R_AX_DMAC_ERR_ISR: u32      = 0x8524;

pub const R_AX_BOOT_DBG: u32          = 0x83F0;

// Memory management
pub const R_AX_WDE_PKTBUF_CFG: u32    = 0x8C08;
pub const R_AX_PLE_PKTBUF_CFG: u32    = 0x9008;
pub const R_AX_DLE_EMPTY0: u32        = 0x8430;
pub const R_AX_DLE_EMPTY1: u32        = 0x8434;

// CMAC
pub const R_AX_CMAC_FUNC_EN: u32      = 0xC000;
pub const R_AX_CK_EN: u32             = 0xC004;

// ── Register Bit Definitions ─────────────────────────────────────

// R_AX_SYS_PW_CTRL (0x0004) bits
pub const B_AX_APFN_ONMAC: u32       = 1 << 8;
pub const B_AX_APFM_OFFMAC: u32      = 1 << 9;
pub const B_AX_APFM_SWLPS: u32       = 1 << 10;
pub const B_AX_AFSM_WLSUS_EN: u32    = 1 << 11;
pub const B_AX_AFSM_PCIE_SUS_EN: u32 = 1 << 12;
pub const B_AX_APDM_HPDN: u32        = 1 << 15;
pub const B_AX_EN_WLON: u32           = 1 << 16;
pub const B_AX_RDY_SYSPWR: u32        = 1 << 17;
pub const B_AX_DIS_WLBT_PDNSUSEN_SOPC: u32 = 1 << 18;

// R_AX_SYS_ISO_CTRL (0x0000) bits
pub const B_AX_ISO_EB2CORE: u32      = 1 << 8;
pub const B_AX_PWC_EV2EF_B14: u32    = 1 << 14;
pub const B_AX_PWC_EV2EF_B15: u32    = 1 << 15;

// R_AX_SYS_FUNC_EN (0x0002) bits (byte access)
pub const B_AX_FEN_BBRSTB: u8        = 1 << 0;
pub const B_AX_FEN_BB_GLB_RSTN: u8   = 1 << 1;

// R_AX_WLLPS_CTRL (0x0090) bits
pub const B_AX_DIS_WLBT_LPSEN_LOPC: u32 = 1 << 1;

// R_AX_SYS_AFE_LDO_CTRL (0x0020) bits
pub const B_AX_AON_OFF_PC_EN: u32    = 1 << 23;

// R_AX_SYS_ADIE_PAD_PWR_CTRL (0x0018) bits
pub const B_AX_SYM_PADPDN_WL_RFC_1P3: u32 = 1 << 5;
pub const B_AX_SYM_PADPDN_WL_PTA_1P3: u32 = 1 << 6;

// R_AX_PMC_DBG_CTRL2 (0x00CC) bits
pub const B_AX_SYSON_DIS_PMCR_AX_WRMSK: u32 = 1 << 2;

// R_AX_SYS_SDIO_CTRL (0x0070) bits
pub const B_AX_PCIE_CALIB_EN_V1: u32 = 1 << 12;

// R_AX_WLRF_CTRL (0x02F0) bits
pub const B_AX_AFC_AFEDIG: u32       = 1 << 17;

// R_AX_SYS_SWR_CTRL1 (0x0010) bits
pub const B_AX_SYM_CTRL_SPS_PWMFREQ: u32 = 1 << 10;

// R_AX_SPS_DIG_OFF_CTRL0 (0x0400) field masks
pub const B_AX_C1_L1_MASK: u32       = 0x3;       // GENMASK(1,0)
pub const B_AX_C3_L1_MASK: u32       = 0x30;      // GENMASK(5,4)

// R_AX_SPS_DIG_ON_CTRL0 (0x0200) field masks
pub const B_AX_REG_ZCDC_H_MASK: u32  = 0x3 << 17; // GENMASK(18,17)

// R_AX_EECS_EESK_FUNC_SEL (0x02D8) field masks
pub const B_AX_PINMUX_EESK_FUNC_SEL_MASK: u32 = 0xF0; // GENMASK(7,4)

// Power-off constants
pub const SW_LPS_OPTION: u32         = 0x0001A0B2;

// R_AX_PLATFORM_ENABLE (0x0088) bits
pub const B_AX_PLATFORM_EN: u32  = 1 << 0;
pub const B_AX_WCPU_EN: u32      = 1 << 1;
pub const B_AX_APB_WRAP_EN: u32  = 1 << 2;  // firmware watchdog control
pub const B_AX_AXIDMA_EN: u32    = 1 << 3;
pub const B_AX_H_AXIDMA_EN: u32  = 1 << 14;

// R_AX_WCPU_FW_CTRL (0x01E0) bits
pub const B_AX_WCPU_FWDL_EN: u32    = 1 << 0;
pub const B_AX_H2C_PATH_RDY: u32    = 1 << 1;
pub const B_AX_FWDL_PATH_RDY: u32   = 1 << 2;

// R_AX_DMAC_FUNC_EN (0x8400) bits — full set from rtw8852b_pwr_on_func
pub const B_AX_MAC_FUNC_EN: u32     = 1 << 30;
pub const B_AX_DMAC_FUNC_EN: u32    = 1 << 29;
pub const B_AX_MPDU_PROC_EN: u32    = 1 << 28;
pub const B_AX_WD_RLS_EN: u32       = 1 << 27;
pub const B_AX_DLE_WDE_EN: u32      = 1 << 26;
pub const B_AX_TXPKT_CTRL_EN: u32   = 1 << 25;
pub const B_AX_STA_SCH_EN: u32      = 1 << 24;
pub const B_AX_DLE_PLE_EN: u32      = 1 << 23;
pub const B_AX_PKT_BUF_EN: u32      = 1 << 22;
pub const B_AX_DMAC_TBL_EN: u32     = 1 << 21;
pub const B_AX_PKT_IN_EN: u32       = 1 << 20;
pub const B_AX_DLE_CPUIO_EN: u32    = 1 << 19;
pub const B_AX_DISPATCHER_EN: u32   = 1 << 18;
pub const B_AX_BBRPT_EN: u32        = 1 << 17;
pub const B_AX_MAC_SEC_EN: u32      = 1 << 16;
pub const B_AX_DMACREG_GCKEN: u32   = 1 << 15;

// R_AX_DMAC_FUNC_EN extra (Linux has this, we missed it)
pub const B_AX_DMAC_CRPRT: u32      = 1 << 31;

// R_AX_DMAC_CLK_EN (0x8404) bits — Linux dmac_func_en_ax writes these
// to enable clocks for all DMAC sub-blocks. Without these the DMAC
// subsystem runs without clocks → RX/TX DMA dead.
pub const B_AX_WD_RLS_CLK_EN: u32      = 1 << 27;
pub const B_AX_TXPKT_CTRL_CLK_EN: u32  = 1 << 25;
pub const B_AX_STA_SCH_CLK_EN: u32     = 1 << 24;
pub const B_AX_PKT_IN_CLK_EN: u32      = 1 << 20;
pub const B_AX_DLE_CPUIO_CLK_EN: u32   = 1 << 19;
pub const B_AX_DISPATCHER_CLK_EN: u32  = 1 << 18;
pub const B_AX_BBRPT_CLK_EN: u32       = 1 << 17;
pub const B_AX_MAC_SEC_CLK_EN: u32     = 1 << 16;

// R_AX_CMAC_FUNC_EN (0xC000) bits
pub const B_AX_CMAC_CRPRT: u32      = 1 << 31;
pub const B_AX_CMAC_EN: u32         = 1 << 30;
pub const B_AX_CMAC_TXEN: u32       = 1 << 29;
pub const B_AX_CMAC_RXEN: u32       = 1 << 28;
pub const B_AX_FORCE_CMACREG_GCKEN: u32 = 1 << 15;
pub const B_AX_PHYINTF_EN: u32      = 1 << 5;
pub const B_AX_CMAC_DMA_EN: u32     = 1 << 4;
pub const B_AX_PTCLTOP_EN: u32      = 1 << 3;
pub const B_AX_SCHEDULER_EN: u32    = 1 << 2;
pub const B_AX_TMAC_EN: u32         = 1 << 1;
pub const B_AX_RMAC_EN: u32         = 1 << 0;

// R_AX_CK_EN (0xC004) bits — CMAC sub-block clocks. Without these the
// CMAC RX pipe is dead: RMAC, PHYINTF, CMAC_DMA all gate receive.
pub const B_AX_CMAC_CKEN: u32       = 1 << 30;
pub const B_AX_PHYINTF_CKEN: u32    = 1 << 5;
pub const B_AX_CMAC_DMA_CKEN: u32   = 1 << 4;
pub const B_AX_PTCLTOP_CKEN: u32    = 1 << 3;
pub const B_AX_SCHEDULER_CKEN: u32  = 1 << 2;
pub const B_AX_TMAC_CKEN: u32       = 1 << 1;
pub const B_AX_RMAC_CKEN: u32       = 1 << 0;

// ── Firmware Download Status Bits ────────────────────────────────

pub const FWDL_WCPU_FW_INIT_RDY: u32  = 1 << 0;
pub const FWDL_CHECKSUM_FAIL: u32     = 1 << 4;
pub const FWDL_SECURITY_FAIL: u32     = 1 << 5;
pub const FWDL_CV_NOT_MATCH: u32      = 1 << 6;

// ── PCIe DMA Control ─────────────────────────────────────────────

pub const R_AX_PCIE_DMA_STOP1: u32    = 0x1010;
pub const R_AX_TXBD_RWPTR_CLR1: u32   = 0x1014;
pub const R_AX_RXBD_RWPTR_CLR: u32    = 0x1018;
pub const R_AX_PCIE_DMA_BUSY1: u32    = 0x101C;

pub const B_AX_STOP_CH12: u32    = 1 << 18;
pub const B_AX_STOP_WPDMA: u32   = 1 << 19;
pub const B_AX_STOP_PCIEIO: u32  = 1 << 20;

// Bits in R_AX_TXBD_RWPTR_CLR1: set to clear corresponding ring index
pub const B_AX_CLR_ACH0_IDX: u32  = 1 << 0;
pub const B_AX_CLR_ACH1_IDX: u32  = 1 << 1;
pub const B_AX_CLR_ACH2_IDX: u32  = 1 << 2;
pub const B_AX_CLR_ACH3_IDX: u32  = 1 << 3;
pub const B_AX_CLR_CH8_IDX: u32   = 1 << 8;
pub const B_AX_CLR_CH9_IDX: u32   = 1 << 9;
pub const B_AX_CLR_CH12_IDX: u32  = 1 << 10;
pub const B_AX_CLR_ALL_CH: u32    = 0x7FF; // bits [10:0]

// R_AX_PCIE_INIT_CFG1 (0x1000) DMA control bits
pub const B_AX_TXHCI_EN: u32     = 1 << 11;
pub const B_AX_RXHCI_EN: u32     = 1 << 13;
pub const B_AX_RST_BDRAM: u32    = 1 << 3;

// ── CH8 TX Ring (MGMT Band 0) — ring size/idx/dma-high ──────────
//
// pci.h rtw89_pci_ch_dma_addr_set (non-V1, for 8852BE single-band).
// CH8 = RTW89_TXCH_CH8 = MGMT Band 0. QSEL = RTW89_TX_QSEL_B0_MGMT = 0x12.
// BDRAM single-band layout: start=20 max=4 min=1 (pci.c:1716).
// Base addrs (DESA_L, BDRAM_CTRL) are defined above with ACH0..3/CH9.
pub const R_AX_CH8_TXBD_NUM: u32    = 0x1034; // 16-bit: ring size
pub const R_AX_CH8_TXBD_IDX: u32    = 0x1078; // 16-bit: wp write / rp read
pub const R_AX_CH8_TXBD_DESA_H: u32 = 0x1154;
pub const B_AX_CH8_BUSY: u32        = 1 << 16; // R_AX_PCIE_DMA_BUSY1

// ── Chip Constants ───────────────────────────────────────────────

pub const RTL8852B_VENDOR: u16 = 0x10EC;
pub const RTL8852B_DEVICE: u16 = 0xB852;

// ── XTAL SI Indirect Access ─────────────────────────────────────
// Written via R_AX_WLAN_XTAL_SI_CTRL (0x0270):
//   BIT(31) = CMD_POLL (set to trigger, clears when done)
//   [25:24] = mode (0=write, 1=read)
//   [23:16] = bitmask
//   [15:8]  = data
//   [7:0]   = address (offset)

// XTAL SI register offsets
pub const XTAL_SI_ANAPAR_WL: u8   = 0x90;
pub const XTAL_SI_WL_RFC_S0: u8   = 0x80;
pub const XTAL_SI_WL_RFC_S1: u8   = 0x81;
pub const XTAL_SI_SRAM_CTRL: u8   = 0xA1;
pub const XTAL_SI_XTAL_XMD_2: u8  = 0x24;
pub const XTAL_SI_XTAL_XMD_4: u8  = 0x26;

// XTAL SI bit masks for ANAPAR_WL (offset 0x90)
pub const XTAL_SI_PON_WEI: u8     = 1 << 0;
pub const XTAL_SI_PON_EI: u8      = 1 << 1;
pub const XTAL_SI_OFF_WEI: u8     = 1 << 2;
pub const XTAL_SI_OFF_EI: u8      = 1 << 3;
pub const XTAL_SI_RFC2RF: u8      = 1 << 4;
pub const XTAL_SI_SHDN_WL: u8     = 1 << 5;
pub const XTAL_SI_GND_SHDN_WL: u8 = 1 << 6;
pub const XTAL_SI_SRAM2RFC: u8    = 1 << 7;

// XTAL SI bit masks for other offsets
pub const XTAL_SI_SRAM_DIS: u8    = 1 << 1;  // SRAM_CTRL (0xA1)
pub const XTAL_SI_RF00: u8        = 1 << 0;  // WL_RFC_S0 (0x80)
pub const XTAL_SI_RF10: u8        = 1 << 0;  // WL_RFC_S1 (0x81)
pub const XTAL_SI_LDO_LPS: u8     = 0x70;    // XTAL_XMD_2 GENMASK(6,4)
pub const XTAL_SI_LPS_CAP: u8     = 0x0F;    // XTAL_XMD_4 GENMASK(3,0)
