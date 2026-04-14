//! RTL8852BE register definitions (from rtw89 Linux driver)

// ── System / Power ───────────────────────────────────────────────

pub const R_AX_SYS_ISO_CTRL: u32      = 0x0000;
pub const R_AX_SYS_FUNC_EN: u32       = 0x0002;
pub const R_AX_SYS_PW_CTRL: u32       = 0x0004;
pub const R_AX_SYS_CLK_CTRL: u32      = 0x0008;
pub const B_AX_CPU_CLK_EN: u32        = 1 << 14;
pub const R_AX_SYS_AFE_LDO_CTRL: u32  = 0x0020;
pub const R_AX_GPIO_MUXCFG: u32       = 0x0040;
pub const R_AX_PLATFORM_ENABLE: u32   = 0x0088;
pub const R_AX_SYS_CFG1: u32          = 0x00F0;
pub const R_AX_SYS_STATUS1: u32       = 0x00F4;

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
pub const R_AX_SEC_CTRL: u32          = 0x0C00;
pub const B_AX_SEC_IDMEM_MASK: u32    = 0x3 << 16;
pub const B_AX_BOOT_REASON_MASK: u32  = 0x7; // bits [2:0] at offset 0x01E6

// ── PCIe / DMA ───────────────────────────────────────────────────

pub const R_AX_PCIE_INIT_CFG1: u32    = 0x1000;
pub const R_AX_HAXI_INIT_CFG1: u32    = 0x1000;
pub const R_AX_HAXI_DMA_STOP1: u32    = 0x1010;
pub const R_AX_HAXI_DMA_BUSY1: u32    = 0x101C;

// TX BD ring addresses (CH12 = FW command channel)
pub const R_AX_CH12_TXBD_DESA_L: u32  = 0x1130;
pub const R_AX_CH12_TXBD_DESA_H: u32  = 0x1134;
pub const R_AX_CH12_TXBD_NUM: u32     = 0x1138;
pub const R_AX_CH12_TXBD_IDX: u32     = 0x113C;

// RX BD ring addresses
pub const R_AX_RXQ_RXBD_DESA_L: u32   = 0x1100;
pub const R_AX_RXQ_RXBD_DESA_H: u32   = 0x1104;
pub const R_AX_RXQ_RXBD_NUM: u32      = 0x1108;
pub const R_AX_RXQ_RXBD_IDX: u32      = 0x110C;

pub const R_AX_RPQ_RXBD_DESA_L: u32   = 0x1110;
pub const R_AX_RPQ_RXBD_DESA_H: u32   = 0x1114;
pub const R_AX_RPQ_RXBD_NUM: u32      = 0x1118;
pub const R_AX_RPQ_RXBD_IDX: u32      = 0x111C;

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

// R_AX_PLATFORM_ENABLE (0x0088) bits
pub const B_AX_PLATFORM_EN: u32  = 1 << 0;
pub const B_AX_WCPU_EN: u32      = 1 << 1;
pub const B_AX_AXIDMA_EN: u32    = 1 << 3;
pub const B_AX_H_AXIDMA_EN: u32  = 1 << 14;

// R_AX_WCPU_FW_CTRL (0x01E0) bits
pub const B_AX_WCPU_FWDL_EN: u32    = 1 << 0;
pub const B_AX_H2C_PATH_RDY: u32    = 1 << 1;
pub const B_AX_FWDL_PATH_RDY: u32   = 1 << 2;

// R_AX_DMAC_FUNC_EN (0x8400) bits
pub const B_AX_MAC_FUNC_EN: u32     = 1 << 30;
pub const B_AX_DMAC_FUNC_EN: u32    = 1 << 29;
pub const B_AX_DISPATCHER_EN: u32   = 1 << 18;

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

// ── Chip Constants ───────────────────────────────────────────────

pub const RTL8852B_VENDOR: u16 = 0x10EC;
pub const RTL8852B_DEVICE: u16 = 0xB852;
