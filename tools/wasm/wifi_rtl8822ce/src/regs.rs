//! Register und Bits, 1:1 aus Linux 6.18.26
//! `drivers/net/wireless/realtek/rtw88/reg.h` (und `pci.h`, `main.h`).
//!
//! Jede Zeile traegt ihre Quelle. Wer hier einen Wert aendert, ohne die
//! genannte Zeile gelesen zu haben, hat geraten — und genau das ist die
//! Regel, die bei diesem Chip nicht verhandelbar ist.
#![allow(dead_code)]

// ── PCI-Identitaet ───────────────────────────────────────────────
// rtw8822ce.c: PCI_DEVICE(PCI_VENDOR_ID_REALTEK, 0xC822) und 0xC82F.
pub const RTL_VENDOR: u16 = 0x10ec;
pub const RTL8822CE_DEVICE: u16 = 0xc822;
pub const RTL8822CE_DEVICE_ALT: u16 = 0xc82f;

/// pci.c `rtw_pci_io_mapping`: `u8 bar_id = 2` — NICHT BAR0.
pub const BAR_REG: u8 = 2;
/// 16 Seiten = 64 KiB. Die hoechste PCIe-Registeradresse, die rtw88 anfasst,
/// ist `RTK_PCI_TXBD_H2CQ_CSR` 0x1330; BB/RF liegen direkt im Fenster bis
/// ~0x5000. Der Kernel klemmt ohnehin auf die echte BAR-Groesse.
pub const BAR_PAGES: u16 = 16;

// ── Systemregister (reg.h) ───────────────────────────────────────
pub const REG_SYS_FUNC_EN: u32 = 0x0002; // reg.h:8
pub const REG_SYS_PW_CTRL: u32 = 0x0004; // reg.h:18
pub const REG_RSV_CTRL: u32 = 0x001C; // reg.h:33
pub const REG_MCUFW_CTRL: u32 = 0x0080; // reg.h:131
pub const REG_SYS_CFG1: u32 = 0x00F0; // reg.h:186
pub const REG_SYS_STATUS1: u32 = 0x00F4; // reg.h:202
pub const REG_SYS_CFG2: u32 = 0x00FC; // reg.h:204
pub const REG_CR: u32 = 0x0100; // reg.h:207

// REG_SYS_CFG1-Felder, reg.h:187-201
pub const BIT_RTL_ID: u32 = 1 << 23;
pub const BIT_LDO: u32 = 1 << 24;
pub const BIT_RF_TYPE_ID: u32 = 1 << 27;
pub const BIT_SHIFT_VENDOR_ID: u32 = 16;
pub const BIT_MASK_VENDOR_ID: u32 = 0xf;
pub const BIT_SHIFT_CHIP_VER: u32 = 12;
pub const BIT_MASK_CHIP_VER: u32 = 0xf;

/// reg.h:201 — `BIT_GET_CHIP_VER(x)`
#[inline]
pub fn bit_get_chip_ver(x: u32) -> u8 {
    ((x >> BIT_SHIFT_CHIP_VER) & BIT_MASK_CHIP_VER) as u8
}

/// reg.h:195 — `BIT_GET_VENDOR_ID(x)`
#[inline]
pub fn bit_get_vendor_id(x: u32) -> u8 {
    ((x >> BIT_SHIFT_VENDOR_ID) & BIT_MASK_VENDOR_ID) as u8
}

/// mac.h:9 — `cut_version_to_mask(cut)`. Waehlt in der Power-Sequenz aus,
/// welche Kommandos fuer DIESEN Chipschnitt gelten.
/// In C ist das ein int-Shift, der erst bei der Zuweisung auf 8 Bit
/// verkuerzt wird — `1u8 << n` waere ab cut 7 etwas anderes.
#[inline]
pub fn cut_version_to_mask(cut: u8) -> u8 {
    (1u32 << (cut as u32 + 1)) as u8
}

// ── Stufe 1: Power-Sequenz (reg.h, Zeilen wie angegeben) ─────────
pub const REG_SYS_CLKR: u32 = 0x0008; // reg.h:28
pub const BIT_ANA8M: u32 = 1 << 1;
pub const BIT_WAKEPAD_EN: u32 = 1 << 3;
pub const BIT_LOADER_CLK_EN: u32 = 1 << 5;

pub const REG_RF_CTRL: u32 = 0x001F; // reg.h:38
pub const BIT_RF_SDM_RSTB: u8 = 1 << 2;
pub const BIT_RF_RSTB: u8 = 1 << 1;
pub const BIT_RF_EN: u8 = 1 << 0;

pub const BIT_FEN_BB_GLB_RST: u8 = 1 << 1; // reg.h:14
pub const BIT_FEN_BB_RSTB: u8 = 1 << 0; // reg.h:15

pub const REG_GPIO_MUXCFG: u32 = 0x0040; // reg.h:72
pub const BIT_FSPI_EN: u32 = 1 << 19; // reg.h:73
pub const BIT_WLRFE_4_5_EN: u32 = 1 << 2; // reg.h:78

pub const REG_LED_CFG: u32 = 0x004C; // reg.h:82
pub const BIT_LNAON_SEL_EN: u32 = 1 << 26;
pub const BIT_PAPE_SEL_EN: u32 = 1 << 25;

pub const REG_PAD_CTRL1: u32 = 0x0064; // reg.h:100
pub const BIT_PAPE_WLBT_SEL: u32 = 1 << 29;
pub const BIT_LNAON_WLBT_SEL: u32 = 1 << 28;

pub const REG_HCI_OPT_CTRL: u32 = 0x0074; // reg.h:114
pub const BIT_USB_SUS_DIS: u32 = 1 << 8; // reg.h:115

pub const REG_WLRF1: u32 = 0x00EC; // reg.h:205
pub const BIT_WLRF1_BBRF_EN: u32 = (1 << 24) | (1 << 25) | (1 << 26);

pub const REG_CPU_DMEM_CON: u32 = 0x1080; // reg.h:793
pub const BIT_WL_PLATFORM_RST: u32 = 1 << 16; // reg.h:794
pub const BIT_DDMA_EN: u32 = 1 << 8; // reg.h:796

pub const REG_CR_EXT: u32 = 0x1100; // reg.h:806

// REG_SYS_PW_CTRL / REG_APS_FSMCO teilen sich 0x0004 (reg.h:18-24).
pub const BIT_PFM_WOWL: u8 = 1 << 3;
pub const APS_FSMCO_MAC_ENABLE: u32 = 1 << 8;
pub const APS_FSMCO_MAC_OFF: u32 = 1 << 9;
pub const APS_FSMCO_HW_POWERDOWN: u32 = 1 << 15;

// REG_MCUFW_CTRL-Felder (reg.h:131-158)
pub const BIT_BOOT_FSPI_EN: u32 = 1 << 20;
pub const BIT_RPWM_TOGGLE: u8 = 1 << 7;
pub const BIT_MCUFWDL_EN: u8 = 1 << 0;

// ── Stufe 2b: Firmware-Download (reg.h/mac.h, Zeilen wie angegeben) ──
pub const REG_SYS_CLK_CTRL: u32 = 0x0008; // reg.h:25
pub const BIT_CPU_CLK_EN: u32 = 1 << 14;
pub const BIT_WLMCU_IOIF: u8 = 1 << 0; // reg.h:37
pub const BIT_FEN_CPUEN: u8 = 1 << 2; // reg.h:12

pub const REG_TXDMA_PQ_MAP: u32 = 0x010C; // reg.h:239
pub const RTW_DMA_MAPPING_HIGH: u8 = 3; // main.h:1013

pub const BIT_TXDMA_EN: u8 = 1 << 2; // reg.h:216
pub const BIT_HCI_TXDMA_EN: u8 = 1 << 0; // reg.h:218
pub const BIT_ENSWBCN: u32 = 1 << 8; // reg.h:210 (im 16-Bit-CR)

pub const REG_FIFOPAGE_CTRL_2: u32 = 0x0204; // reg.h:322
pub const BIT_BCN_VALID_V1: u32 = 1 << 15;
pub const BIT_MASK_BCN_HEAD_1_V1: u16 = 0xfff;

pub const REG_TXDMA_STATUS: u32 = 0x0210; // reg.h:332
pub const BTI_PAGE_OVF: u32 = 1 << 2;

pub const REG_RQPN_CTRL_2: u32 = 0x022C; // reg.h:348
pub const BIT_LD_RQPN: u32 = 1 << 31;
pub const REG_FIFOPAGE_INFO_1: u32 = 0x0230; // reg.h:350

pub const REG_FWHW_TXQ_CTRL: u32 = 0x0420; // reg.h:387
pub const BIT_EN_BCNQ_DL: u32 = 1 << 22;

pub const REG_BCN_CTRL: u32 = 0x0550; // reg.h:478
pub const BIT_DIS_TSF_UDT: u8 = 1 << 4;
pub const BIT_EN_BCN_FUNCTION: u8 = 1 << 3;

pub const REG_FW_DBG7: u32 = 0x10FC; // reg.h:803
pub const FW_KEY_MASK: u32 = 0xffffff00;
pub const ILLEGAL_KEY_GROUP: u32 = 0xFAAAAA00; // mac.h:14

pub const REG_DDMA_CH0SA: u32 = 0x1200; // reg.h:812
pub const REG_DDMA_CH0DA: u32 = 0x1204;
pub const REG_DDMA_CH0CTRL: u32 = 0x1208;
pub const BIT_DDMACH0_OWN: u32 = 1 << 31;
pub const BIT_DDMACH0_CHKSUM_EN: u32 = 1 << 29;
pub const BIT_DDMACH0_CHKSUM_STS: u32 = 1 << 27;
pub const BIT_DDMACH0_RESET_CHKSUM_STS: u32 = 1 << 25;
pub const BIT_DDMACH0_CHKSUM_CONT: u32 = 1 << 24;
pub const BIT_MASK_DDMACH0_DLEN: u32 = 0x3ffff;

pub const REG_H2CQ_CSR: u32 = 0x1330; // reg.h:823
pub const BIT_H2CQ_FULL: u32 = 1 << 31;

// mac.h:17-22
pub const OCPBASE_TXBUF_88XX: u32 = 0x18780000;
pub const OCPBASE_DMEM_88XX: u32 = 0x00200000;

// reg.h:135-153 — die Statusbits des Downloads
pub const BIT_FW_INIT_RDY: u32 = 1 << 15;
pub const BIT_FW_DW_RDY: u32 = 1 << 14;
pub const BIT_CPU_CLK_SEL: u32 = (1 << 12) | (1 << 13);
pub const BIT_DMEM_CHKSUM_OK: u32 = 1 << 6;
pub const BIT_DMEM_DW_OK: u32 = 1 << 5;
pub const BIT_IMEM_CHKSUM_OK: u32 = 1 << 4;
pub const BIT_IMEM_DW_OK: u32 = 1 << 3;
pub const BIT_BOOT_FSPI_EN_U32: u32 = 1 << 20;
pub const BIT_CHECK_SUM_OK: u32 = (1 << 4) | (1 << 6);
pub const FW_READY: u32 = BIT_FW_INIT_RDY | BIT_FW_DW_RDY
    | BIT_IMEM_DW_OK | BIT_DMEM_DW_OK | BIT_CHECK_SUM_OK;
pub const FW_READY_MASK: u32 = 0xffff & !BIT_CPU_CLK_SEL;

// reg.h:897-903 — LTE-Koexistenz, indirekter Zugriff
pub const LTECOEX_ACCESS_CTRL: u32 = 0x1700;
pub const LTECOEX_WRITE_DATA: u32 = 0x1704;
pub const LTECOEX_READ_DATA: u32 = 0x1708;
pub const LTECOEX_READY: u32 = 1 << 29;

// fw.h:12-13
pub const FW_HDR_SIZE: usize = 64;
pub const FW_HDR_CHKSUM_SIZE: u32 = 8;

/// rtw8822c.c `rtw8822c_hw_spec.sys_func_en`
pub const SYS_FUNC_EN_8822C: u8 = 0xD8;

/// main.h:967-973 — unser Geraet meldet 3.
pub const RTW_CHIP_VER_CUT_D: u8 = 0x03;

// ── Zustaende, die Linux als Klartextvergleich liest ─────────────
/// mac.c `rtw_mac_power_switch`: `rtw_read8(rtwdev, REG_CR) == 0xea`
/// heisst „MAC ist AUS". Ein 8-Bit-Lesezugriff, ausdruecklich.
pub const CR_POWER_OFF: u8 = 0xea;
/// mac.c `rtw_mac_power_switch`: `rtw_read16(rtwdev, REG_MCUFW_CTRL) == 0xC078`
/// heisst „die Firmware laeuft noch".
pub const MCUFW_CTRL_FW_ALIVE: u16 = 0xc078;

// ── HCI-Parameter, main.c `rtw_chip_parameter_setup` ─────────────
pub const PCIE_RPWM_ADDR: u32 = 0x03d9;
pub const PCIE_CPWM_ADDR: u32 = 0x03da;

// ── Erwartung fuer den 8822C (rtw8822c.c `rtw8822c_hw_spec`) ─────
pub const TX_PKT_DESC_SZ: u32 = 48;
pub const TX_BUF_DESC_SZ: u32 = 16;
pub const RX_PKT_DESC_SZ: u32 = 24;
pub const RX_BUF_DESC_SZ: u32 = 8;
pub const PHY_EFUSE_SIZE: u32 = 512;
pub const LOG_EFUSE_SIZE: u32 = 768;
pub const PTCT_EFUSE_SIZE: u32 = 124;
