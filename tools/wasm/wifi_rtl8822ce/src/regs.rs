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

// ── Stufe 2c: efuse und hw_feature ───────────────────────────────
pub const REG_EFUSE_CTRL: u32 = 0x0030; // reg.h:48
pub const BIT_EF_FLAG: u32 = 1 << 31;
pub const BIT_SHIFT_EF_ADDR: u32 = 8;
pub const BIT_MASK_EF_ADDR: u32 = 0x3ff;
pub const BIT_MASK_EF_DATA: u32 = 0xff;
pub const BITS_EF_ADDR: u32 = BIT_MASK_EF_ADDR << BIT_SHIFT_EF_ADDR;

pub const REG_LDO_EFUSE_CTRL: u32 = 0x0034; // reg.h:62
pub const BIT_MASK_EFUSE_BANK_SEL: u32 = (1 << 8) | (1 << 9);

pub const REG_C2HEVT: u32 = 0x01A0; // reg.h:287
/// fw.h:63 — der Ausloeser, VOR dem Firmware-Download geschrieben.
pub const C2H_HW_FEATURE_DUMP: u8 = 0xfd;
/// fw.h:57 — die Antwort der Firmware.
pub const C2H_HW_FEATURE_REPORT: u8 = 0x19;
/// main.h:39
pub const HW_FEATURE_LEN: usize = 13;

pub const REG_ANAPARLDO_POW_MAC: u32 = 0x0029; // rtw8822c.h:181
pub const BIT_LDOE25_PON: u8 = 1 << 0; // rtw8822c.h:182

// efuse.h:8-11
pub const EFUSE_HW_CAP_IGNORE: u8 = 0;
pub const EFUSE_HW_CAP_SUPP_BW80: u8 = 7;
pub const EFUSE_HW_CAP_SUPP_BW40: u8 = 6;

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

// ════════════════════════════════════════════════════════════════
// Stufe 3a: rtw_mac_init
// ════════════════════════════════════════════════════════════════

// ── mac.c `txdma_queue_mapping` (reg.h:207-260) ──────────────────
pub const BIT_RXDMA_EN: u8 = 1 << 3; // reg.h:215
pub const BIT_HCI_RXDMA_EN: u8 = 1 << 1; // reg.h:217
pub const BIT_PROTOCOL_EN: u8 = 1 << 4; // reg.h:214
pub const BIT_SCHEDULE_EN: u8 = 1 << 5; // reg.h:213
pub const BIT_MACTXEN: u8 = 1 << 6; // reg.h:212
pub const BIT_MACRXEN: u8 = 1 << 7; // reg.h:211
/// reg.h:219 — alle acht Bits, also 0xff. Der Name steht hier, weil Linux
/// ihn schreibt; die Zahl daneben ist kein Kommentar, sondern das Ergebnis.
pub const MAC_TRX_ENABLE: u8 = BIT_HCI_TXDMA_EN | BIT_HCI_RXDMA_EN | BIT_TXDMA_EN
    | BIT_RXDMA_EN | BIT_PROTOCOL_EN | BIT_SCHEDULE_EN | BIT_MACTXEN | BIT_MACRXEN;

// Die sechs Queue-Abbildungen sind Feldmakros: zwei Bit je Queue im
// 16-Bit-Wort REG_TXDMA_PQ_MAP (reg.h:233-258).
pub const BIT_SHIFT_TXDMA_VOQ_MAP: u32 = 4; // reg.h:231
pub const BIT_SHIFT_TXDMA_VIQ_MAP: u32 = 6; // reg.h:235
pub const BIT_SHIFT_TXDMA_BEQ_MAP: u32 = 8; // reg.h:243
pub const BIT_SHIFT_TXDMA_BKQ_MAP: u32 = 10; // reg.h:247
pub const BIT_SHIFT_TXDMA_MGQ_MAP: u32 = 12; // reg.h:251
pub const BIT_SHIFT_TXDMA_HIQ_MAP: u32 = 14; // reg.h:255
pub const BIT_MASK_TXDMA_QUEUE_MAP: u16 = 0x3; // reg.h:232 u.a., fuer alle sechs

/// main.h:1010-1013 — die vier Zielprioritaeten.
pub const RTW_DMA_MAPPING_EXTRA: u8 = 0; // main.h:1010
pub const RTW_DMA_MAPPING_LOW: u8 = 1; // main.h:1011
pub const RTW_DMA_MAPPING_NORMAL: u8 = 2; // main.h:1012

// ── mac.c `rtw_set_trx_fifo_info` (mac.h:24-29, main.h:34-35) ────
pub const TX_PAGE_SIZE_SHIFT: u32 = 7; // main.h:34
pub const TX_PAGE_SIZE: u32 = 1 << TX_PAGE_SIZE_SHIFT; // main.h:35
pub const RSVD_PG_DRV_NUM: u16 = 16; // mac.h:24
pub const RSVD_PG_H2C_EXTRAINFO_NUM: u16 = 24; // mac.h:25
pub const RSVD_PG_H2C_STATICINFO_NUM: u16 = 8; // mac.h:26
pub const RSVD_PG_H2CQ_NUM: u16 = 8; // mac.h:27
pub const RSVD_PG_CPU_INSTRUCTION_NUM: u16 = 0; // mac.h:28
pub const RSVD_PG_FW_TXBUF_NUM: u16 = 4; // mac.h:29
pub const C2H_PKT_BUF: u32 = 256; // mac.h:11
pub const PHY_STATUS_SIZE: u8 = 4; // mac.h:13

/// rtw8822c.c `rtw8822c_hw_spec` — die drei Zahlen, aus denen der Seitenplan
/// faellt. `page_size` ist `TX_PAGE_SIZE`.
pub const TXFF_SIZE_8822C: u32 = 262144; // rtw8822c.c:5345
pub const RXFF_SIZE_8822C: u32 = 24576; // rtw8822c.c:5346
pub const RSVD_DRV_PG_NUM_8822C: u16 = 16; // rtw8822c.c:5348
pub const CSI_BUF_PG_NUM_8822C: u16 = 50; // rtw8822c.c:5352

// ── mac.c `__priority_queue_cfg` ─────────────────────────────────
pub const REG_FIFOPAGE_INFO_2: u32 = 0x0234; // reg.h:351
pub const REG_FIFOPAGE_INFO_3: u32 = 0x0238; // reg.h:352
pub const REG_FIFOPAGE_INFO_4: u32 = 0x023C; // reg.h:353
pub const REG_FIFOPAGE_INFO_5: u32 = 0x0240; // reg.h:354
pub const BIT_EN_WR_FREE_TAIL: u32 = 1 << 20; // reg.h:389
pub const REG_BCNQ_BDNY_V1: u32 = 0x0424; // reg.h:392
pub const REG_BCNQ1_BDNY_V1: u32 = 0x0456; // reg.h:411
pub const REG_RXFF_BNDY: u32 = 0x011C; // reg.h:276
pub const REG_AUTO_LLT_V1: u32 = 0x0208; // reg.h:325
pub const BIT_AUTO_INIT_LLT_V1: u32 = 1 << 0; // reg.h:326

// ── mac.c `init_h2c` ─────────────────────────────────────────────
pub const REG_H2C_HEAD: u32 = 0x0244; // reg.h:355
pub const REG_H2C_TAIL: u32 = 0x0248; // reg.h:356
pub const REG_H2C_READ_ADDR: u32 = 0x024C; // reg.h:357
pub const REG_H2C_INFO: u32 = 0x0254; // reg.h:358
pub const REG_TXDMA_OFFSET_CHK: u32 = 0x020C; // reg.h:330
pub const REG_H2C_PKT_READADDR: u32 = 0x10D0; // reg.h:800
pub const REG_H2C_PKT_WRITEADDR: u32 = 0x10D4; // reg.h:801

// ── mac.c `rtw_drv_info_cfg` ─────────────────────────────────────
pub const REG_RX_DRVINFO_SZ: u32 = 0x060F; // reg.h:536
pub const REG_TRXFF_BNDY: u32 = 0x0114; // reg.h:275
pub const REG_RCR: u32 = 0x0608; // reg.h:502
pub const BIT_APP_PHYSTS: u32 = 1 << 28; // reg.h:506
pub const REG_WMAC_OPTION_FUNCTION: u32 = 0x07D0; // reg.h:595

// ── pci.c `rtw_pci_interface_cfg` ────────────────────────────────
pub const REG_HCI_MIX_CFG: u32 = 0x03FC; // reg.h:381
pub const BIT_PCIE_EMAC_PDN_AUX_TO_FAST_CLK: u32 = 1 << 26; // reg.h:382

// ── rtw8822c.c `rtw8822c_mac_init` — Register ────────────────────
pub const REG_SPEC_SIFS: u32 = 0x0428; // reg.h:397
pub const REG_SIFS: u32 = 0x0514; // reg.h:457
pub const REG_RESP_SIFS_CCK: u32 = 0x063C; // reg.h:542
pub const REG_RESP_SIFS_OFDM: u32 = 0x063E; // reg.h:543
pub const REG_DARFRC: u32 = 0x0430; // reg.h:399
pub const REG_DARFRCH: u32 = 0x0434; // reg.h:400
pub const REG_RARFRCH: u32 = 0x043C; // reg.h:401
pub const REG_ARFR0: u32 = 0x0444; // reg.h:404
pub const REG_ARFRH0: u32 = 0x0448; // reg.h:405
pub const REG_ARFR1_V1: u32 = 0x044C; // reg.h:406
pub const REG_ARFRH1_V1: u32 = 0x0450; // reg.h:407
pub const REG_ARFR4: u32 = 0x049C; // reg.h:425
pub const REG_ARFRH4: u32 = 0x04A0; // reg.h:427
pub const REG_ARFR5: u32 = 0x04A4; // reg.h:428
pub const REG_ARFRH5: u32 = 0x04A8; // reg.h:429
pub const REG_AMPDU_MAX_TIME_V1: u32 = 0x0455; // reg.h:410
pub const REG_TX_HANG_CTRL: u32 = 0x045E; // reg.h:415
pub const BIT_EN_EOF_V1: u8 = 1 << 2; // reg.h:417
pub const REG_PRECNT_CTRL: u32 = 0x04E5; // reg.h:440
pub const BIT_EN_PRECNT: u16 = 1 << 11; // reg.h:442
pub const REG_PROT_MODE_CTRL: u32 = 0x04C8; // reg.h:437
pub const REG_BAR_MODE_CTRL: u32 = 0x04CC; // reg.h:439
pub const REG_FAST_EDCA_VOVI_SETTING: u32 = 0x1448; // reg.h:825
pub const REG_FAST_EDCA_BEBK_SETTING: u32 = 0x144C; // reg.h:826
pub const REG_LIFETIME_EN: u32 = 0x0426; // reg.h:395
pub const BIT_BA_PARSER_EN: u8 = 1 << 5; // reg.h:396
pub const REG_RRSR: u32 = 0x0440; // reg.h:402
pub const BITS_RRSR_RSC: u32 = 0x60_0000; // reg.h:403  GENMASK(22, 21)
pub const REG_EDCA_VO_PARAM: u32 = 0x0500; // reg.h:447
pub const REG_EDCA_VI_PARAM: u32 = 0x0504; // reg.h:448
pub const REG_EDCA_BE_PARAM: u32 = 0x0508; // reg.h:449
pub const REG_EDCA_BK_PARAM: u32 = 0x050C; // reg.h:450
pub const REG_PIFS: u32 = 0x0512; // reg.h:456
pub const REG_TX_PTCL_CTRL: u32 = 0x0520; // reg.h:463
pub const BIT_SIFS_BK_EN: u32 = 1 << 12; // reg.h:465
pub const REG_RD_CTRL: u32 = 0x0524; // reg.h:469
pub const BIT_DIS_TXOP_CFE: u32 = 1 << 10; // reg.h:471
pub const BIT_DIS_LSIG_CFE: u32 = 1 << 9; // reg.h:472
pub const BIT_DIS_STBC_CFE: u32 = 1 << 8; // reg.h:473
pub const REG_AFE_CTRL1: u32 = 0x0024; // reg.h:46
pub const BIT_MAC_CLK_SEL: u32 = (1 << 20) | (1 << 21); // reg.h:47
pub const REG_USTIME_TSF: u32 = 0x055C; // reg.h:486
pub const REG_USTIME_EDCA: u32 = 0x0638; // reg.h:539
pub const REG_MISC_CTRL: u32 = 0x0577; // reg.h:489
pub const BIT_EN_FREE_CNT: u8 = 1 << 3; // reg.h:490
pub const BIT_DIS_SECOND_CCA: u8 = (1 << 0) | (1 << 1); // reg.h:491
pub const REG_TIMER0_SRC_SEL: u32 = 0x05B4; // reg.h:495
pub const BIT_TSFT_SEL_TIMER0: u8 = (1 << 4) | (1 << 5) | (1 << 6); // reg.h:496
pub const REG_TXPAUSE: u32 = 0x0522; // reg.h:466
pub const REG_SLOT: u32 = 0x051B; // reg.h:462
pub const REG_RD_NAV_NXT: u32 = 0x0544; // reg.h:476
pub const REG_RXTSF_OFFSET_CCK: u32 = 0x055E; // reg.h:488
pub const REG_TBTT_PROHIBIT: u32 = 0x0540; // reg.h:474
pub const REG_DRVERLYINT: u32 = 0x0558; // reg.h:483
pub const REG_BCN_CTRL_CLINT0: u32 = 0x0551; // reg.h:482
pub const REG_BCNDMATIM: u32 = 0x0559; // reg.h:484
pub const REG_BCN_MAX_ERR: u32 = 0x055D; // reg.h:487
pub const REG_MAR: u32 = 0x0620; // reg.h:538
pub const REG_BBPSF_CTRL: u32 = 0x06DC; // reg.h:577
pub const REG_ACKTO: u32 = 0x0640; // reg.h:544
pub const REG_ACKTO_CCK: u32 = 0x0639; // reg.h:540
pub const REG_EIFS: u32 = 0x0642; // reg.h:545
pub const REG_NAV_CTRL: u32 = 0x0650; // reg.h:546
pub const REG_WMAC_TRXPTCL_CTL_H: u32 = 0x066C; // reg.h:551
pub const REG_RXFLTMAP0: u32 = 0x06A0; // reg.h:566
pub const REG_RXFLTMAP2: u32 = 0x06A4; // reg.h:568
pub const REG_RX_PKT_LIMIT: u32 = 0x060C; // reg.h:535
pub const REG_TCR: u32 = 0x0604; // reg.h:498
pub const REG_GENERAL_OPTION: u32 = 0x1664; // reg.h:894
pub const BIT_DUMMY_FCS_READY_MASK_EN: u32 = 1 << 9; // reg.h:895
pub const REG_WMAC_OPTION_FUNCTION_1: u32 = 0x07D4; // reg.h:596
pub const REG_RXPSF_CTRL: u32 = 0x1610; // reg.h:828
pub const REG_RXPSF_TYPE_CTRL: u32 = 0x1614; // reg.h:893
pub const REG_INT_MIG: u32 = 0x0304; // reg.h:380
/// `REG_SND_PTCL_CTRL` und sein Bit stehen in **bf.h**, nicht in reg.h —
/// Beamforming hat dort seinen eigenen Registerblock.
pub const REG_SND_PTCL_CTRL: u32 = 0x0718; // bf.h:15
pub const BIT_DIS_CHK_VHTSIGB_CRC: u8 = 1 << 6; // bf.h:16

// REG_RXPSF_CTRL-Felder (reg.h:831-889)
pub const BIT_SHIFT_RXGCK_VHT_FIFOTHR: u32 = 26; // reg.h:831
pub const BIT_SHIFT_RXGCK_HT_FIFOTHR: u32 = 24; // reg.h:838
pub const BIT_SHIFT_RXGCK_OFDM_FIFOTHR: u32 = 22; // reg.h:845
pub const BIT_SHIFT_RXGCK_CCK_FIFOTHR: u32 = 20; // reg.h:852
pub const BIT_SHIFT_RXPSF_PKTLENTHR: u32 = 13; // reg.h:861
pub const BIT_MASK_RXPSF_PKTLENTHR: u16 = 0x7; // reg.h:862
pub const BIT_RXPSF_CTRLEN: u16 = 1 << 12; // reg.h:871
pub const BIT_RXPSF_VHTCHKEN: u16 = 1 << 11; // reg.h:872
pub const BIT_RXPSF_HTCHKEN: u16 = 1 << 10; // reg.h:873
pub const BIT_RXPSF_OFDMCHKEN: u16 = 1 << 9; // reg.h:874
pub const BIT_RXPSF_CCKCHKEN: u16 = 1 << 8; // reg.h:875
pub const BIT_RXPSF_OFDMRST: u16 = 1 << 7; // reg.h:876
pub const BIT_RXPSF_CCKRST: u16 = 1 << 6; // reg.h:877
pub const BIT_RXPSF_MHCHKEN: u16 = 1 << 5; // reg.h:878
pub const BIT_RXPSF_CONT_ERRCHKEN: u16 = 1 << 4; // reg.h:879
pub const BIT_SHIFT_RXPSF_ERRTHR: u32 = 0; // reg.h:882
pub const BIT_MASK_RXPSF_ERRTHR: u16 = 0x7; // reg.h:883

// ── rtw8822c.c `rtw8822c_mac_init` — Werte ───────────────────────
pub const WLAN_TXQ_RPT_EN: u8 = 0x1F; // rtw8822c.c:1915
pub const WLAN_SLOT_TIME: u8 = 0x09; // rtw8822c.c:1916
pub const WLAN_PIFS_TIME: u8 = 0x1C; // rtw8822c.c:1917
pub const WLAN_NAV_MAX: u8 = 0xC8; // rtw8822c.c:1922
pub const WLAN_DRV_EARLY_INT: u8 = 0x04; // rtw8822c.c:1929
pub const WLAN_BCN_CTRL_CLT0: u8 = 0x10; // rtw8822c.c:1930
pub const WLAN_BCN_DMA_TIME: u8 = 0x02; // rtw8822c.c:1931
pub const WLAN_BCN_MAX_ERR: u8 = 0xFF; // rtw8822c.c:1932
pub const WLAN_SIFS_CCK_CTX: u16 = 0x0A; // rtw8822c.c:1935
pub const WLAN_SIFS_CCK_IRX: u16 = 0x0A; // rtw8822c.c:1936
pub const WLAN_SIFS_OFDM_CTX: u16 = 0x0E; // rtw8822c.c:1937
pub const WLAN_SIFS_OFDM_IRX: u16 = 0x0E; // rtw8822c.c:1938
pub const WLAN_EIFS_DUR_TUNE: u16 = 0x40; // rtw8822c.c:1939
pub const WLAN_EDCA_VO_PARAM: u32 = 0x002F_A226; // rtw8822c.c:1940
pub const WLAN_EDCA_VI_PARAM: u32 = 0x005E_A328; // rtw8822c.c:1941
pub const WLAN_EDCA_BE_PARAM: u32 = 0x005E_A42B; // rtw8822c.c:1942
pub const WLAN_EDCA_BK_PARAM: u32 = 0x0000_A44F; // rtw8822c.c:1943
pub const WLAN_RX_FILTER0: u32 = 0xFFFF_FFFF; // rtw8822c.c:1945
pub const WLAN_RX_FILTER2: u16 = 0xFFFF; // rtw8822c.c:1946
pub const WLAN_RCR_CFG: u32 = 0xE400_220E; // rtw8822c.c:1947
pub const WLAN_RXPKT_MAX_SZ_512: u8 = 24; // rtw8822c.c:1949  (12288 >> 9)
pub const WLAN_AMPDU_MAX_TIME: u8 = 0x70; // rtw8822c.c:1951
pub const WLAN_RTS_LEN_TH: u32 = 0xFF; // rtw8822c.c:1952
pub const WLAN_RTS_TX_TIME_TH: u32 = 0x08; // rtw8822c.c:1953
pub const WLAN_MAX_AGG_PKT_LIMIT: u32 = 0x3f; // rtw8822c.c:1954
pub const WLAN_RTS_MAX_AGG_PKT_LIMIT: u32 = 0x3f; // rtw8822c.c:1955
pub const WLAN_PRE_TXCNT_TIME_TH: u16 = 0x1E0; // rtw8822c.c:1956
pub const FAST_EDCA_VO_TH: u8 = 0x06; // rtw8822c.c:1957
pub const FAST_EDCA_VI_TH: u8 = 0x06; // rtw8822c.c:1958
pub const FAST_EDCA_BE_TH: u8 = 0x06; // rtw8822c.c:1959
pub const FAST_EDCA_BK_TH: u8 = 0x06; // rtw8822c.c:1960
pub const WLAN_BAR_RETRY_LIMIT: u16 = 0x01; // rtw8822c.c:1961
pub const WLAN_BAR_ACK_TYPE: u8 = 0x05; // rtw8822c.c:1962
pub const WLAN_RA_TRY_RATE_AGG_LIMIT: u16 = 0x08; // rtw8822c.c:1963
pub const WLAN_RESP_TXRATE: u8 = 0x84; // rtw8822c.c:1964
pub const WLAN_ACK_TO: u8 = 0x21; // rtw8822c.c:1965
pub const WLAN_ACK_TO_CCK: u8 = 0x6A; // rtw8822c.c:1966
pub const WLAN_DATA_RATE_FB_CNT_1_4: u32 = 0x0100_0000; // rtw8822c.c:1967
pub const WLAN_DATA_RATE_FB_CNT_5_8: u32 = 0x0807_0504; // rtw8822c.c:1968
pub const WLAN_RTS_RATE_FB_CNT_5_8: u32 = 0x0807_0504; // rtw8822c.c:1969
pub const WLAN_DATA_RATE_FB_RATE0: u32 = 0xFE01_F010; // rtw8822c.c:1970
pub const WLAN_DATA_RATE_FB_RATE0_H: u32 = 0x4000_0000; // rtw8822c.c:1971
pub const WLAN_RTS_RATE_FB_RATE1: u32 = 0x003F_F010; // rtw8822c.c:1972
pub const WLAN_RTS_RATE_FB_RATE1_H: u32 = 0x4000_0000; // rtw8822c.c:1973
pub const WLAN_RTS_RATE_FB_RATE4: u32 = 0x0600_F010; // rtw8822c.c:1974
pub const WLAN_RTS_RATE_FB_RATE4_H: u32 = 0x4000_03E0; // rtw8822c.c:1975
pub const WLAN_RTS_RATE_FB_RATE5: u32 = 0x0600_F015; // rtw8822c.c:1976
pub const WLAN_RTS_RATE_FB_RATE5_H: u32 = 0x0000_00E0; // rtw8822c.c:1977
pub const WLAN_MULTI_ADDR: u32 = 0xFFFF_FFFF; // rtw8822c.c:1978
pub const WLAN_TX_FUNC_CFG1: u8 = 0x30; // rtw8822c.c:1980
pub const WLAN_TX_FUNC_CFG2: u8 = 0x30; // rtw8822c.c:1981
pub const WLAN_MAC_OPT_NORM_FUNC1: u8 = 0x98; // rtw8822c.c:1982
pub const WLAN_MAC_OPT_FUNC2: u32 = 0xb081_0041; // rtw8822c.c:1984
pub const WLAN_MAC_INT_MIG_CFG: u32 = 0x3333_0000; // rtw8822c.c:1985
/// rtw8822c.c:1987 — zusammengesetzt aus CCK_CONT_TX 0x0A, OFDM_CONT_TX 0x0E,
/// CCK_TRX 0x0A und OFDM_TRX 0x10 an ihren Schiebestellen: 0x100A0E0A.
pub const WLAN_SIFS_CFG: u32 = 0x100A_0E0A; // rtw8822c.c:1987
/// rtw8822c.c:1992 — CCK_DUR_TUNE 0x0A | OFDM_DUR_TUNE 0x10 << 8.
pub const WLAN_SIFS_DUR_TUNE: u16 = 0x100A; // rtw8822c.c:1992
/// rtw8822c.c:1995 — TBTT_PROHIBIT 0x04 | TBTT_HOLD_TIME 0x64 << 8.
pub const WLAN_TBTT_TIME: u32 = 0x0000_6404; // rtw8822c.c:1995
pub const WLAN_NAV_CFG: u32 = 0x001B_0005; // rtw8822c.c:1998
pub const WLAN_RX_TSF_CFG: u16 = 0x3030; // rtw8822c.c:1999
pub const MAC_CLK_SPEED: u8 = 80; // rtw8822c.c:2001

// ── Feldmakros aus reg.h, als Funktionen ─────────────────────────
// In C sind das `#define NAME(x) (((x) & MASK) << SHIFT)`. Als Funktion
// bleibt die Maske stehen, wo sie in Linux steht — ein direkt geschriebener
// Zahlenwert waere die Rechnung von HEUTE und nicht die Regel.

/// reg.h:233-258 — dieselbe Form fuer alle sechs Sendequeues.
#[inline]
pub fn bit_txdma_queue_map(x: u8, shift: u32) -> u16 {
    ((x as u16) & BIT_MASK_TXDMA_QUEUE_MAP) << shift
}

/// reg.h:833 `BIT_RXGCK_VHT_FIFOTHR(x)`
#[inline]
pub fn bit_rxgck_vht_fifothr(x: u32) -> u32 {
    (x & 0x3) << BIT_SHIFT_RXGCK_VHT_FIFOTHR
}
/// reg.h:840 `BIT_RXGCK_HT_FIFOTHR(x)`
#[inline]
pub fn bit_rxgck_ht_fifothr(x: u32) -> u32 {
    (x & 0x3) << BIT_SHIFT_RXGCK_HT_FIFOTHR
}
/// reg.h:847 `BIT_RXGCK_OFDM_FIFOTHR(x)`
#[inline]
pub fn bit_rxgck_ofdm_fifothr(x: u32) -> u32 {
    (x & 0x3) << BIT_SHIFT_RXGCK_OFDM_FIFOTHR
}
/// reg.h:854 `BIT_RXGCK_CCK_FIFOTHR(x)`
#[inline]
pub fn bit_rxgck_cck_fifothr(x: u32) -> u32 {
    (x & 0x3) << BIT_SHIFT_RXGCK_CCK_FIFOTHR
}

/// reg.h:869 `BIT_SET_RXPSF_PKTLENTHR(x, v)` — Feld loeschen, dann setzen.
#[inline]
pub fn bit_set_rxpsf_pktlenthr(x: u16, v: u16) -> u16 {
    (x & !(BIT_MASK_RXPSF_PKTLENTHR << BIT_SHIFT_RXPSF_PKTLENTHR))
        | ((v & BIT_MASK_RXPSF_PKTLENTHR) << BIT_SHIFT_RXPSF_PKTLENTHR)
}

/// reg.h:889 `BIT_SET_RXPSF_ERRTHR(x, v)`
#[inline]
pub fn bit_set_rxpsf_errthr(x: u16, v: u16) -> u16 {
    (x & !(BIT_MASK_RXPSF_ERRTHR << BIT_SHIFT_RXPSF_ERRTHR))
        | ((v & BIT_MASK_RXPSF_ERRTHR) << BIT_SHIFT_RXPSF_ERRTHR)
}

// ════════════════════════════════════════════════════════════════
// Stufe 3c: rtw8822c_phy_set_param
// ════════════════════════════════════════════════════════════════

// ── rtw8822c_header_file_init (rtw8822c.h:214-219, 312-314) ──────
pub const REG_3WIRE: u32 = 0x180C; // rtw8822c.h:214
pub const REG_3WIRE2: u32 = 0x410C; // rtw8822c.h:353
pub const BIT_3WIRE_TX_EN: u32 = 0x1; // rtw8822c.h:216  GENMASK(0, 0)
pub const BIT_3WIRE_RX_EN: u32 = 0x2; // rtw8822c.h:217  GENMASK(1, 1)
pub const BIT_3WIRE_PI_ON: u32 = 1 << 28; // rtw8822c.h:219
pub const REG_ENCCK: u32 = 0x1C3C; // rtw8822c.h:312
pub const BIT_CCK_BLK_EN: u32 = 1 << 1; // rtw8822c.h:313
pub const BIT_CCK_OFDM_BLK_EN: u32 = 0x3; // rtw8822c.h:314  GENMASK(1, 0)

// ── Sende-/Empfangspfade (rtw8822c.h, main.h:141-147) ────────────
pub const BB_PATH_A: u8 = 1 << 0; // main.h:142
pub const BB_PATH_B: u8 = 1 << 1; // main.h:143
pub const BB_PATH_AB: u8 = BB_PATH_A | BB_PATH_B; // main.h:147
pub const REG_ORITXCODE: u32 = 0x1800; // rtw8822c.h:212
pub const REG_ORITXCODE2: u32 = 0x4100; // rtw8822c.h:352
pub const MASK20BITS: u32 = 0xfffff; // phy.h:184
pub const REG_CCANRX: u32 = 0x1A2C; // rtw8822c.h:242
pub const REG_RXCCKSEL: u32 = 0x1A04; // rtw8822c.h:236
pub const REG_RXFNCTL: u32 = 0x1D30; // rtw8822c.h:326
pub const REG_AGCSWSH: u32 = 0x0C44; // rtw8822c.h:207
pub const REG_ANTWTPD: u32 = 0x0C54; // rtw8822c.h:208
pub const REG_MRCM: u32 = 0x0C38; // rtw8822c.h:206
pub const REG_ANTMAP0: u32 = 0x0820; // rtw8822c.h:190
pub const REG_TXLGMAP: u32 = 0x1E2C; // rtw8822c.h:335
pub const REG_RXIGI: u32 = 0x1D70; // rtw8822c.h:329

// ── DPD, Quarz (reg.h) ───────────────────────────────────────────
pub const REG_DIS_DPD: u32 = 0x0A70; // reg.h:657
pub const DIS_DPD_MASK: u32 = 0x3ff; // reg.h:658  GENMASK(9, 0)
pub const DIS_DPD_RATEALL: u32 = 0x3ff; // reg.h:669
pub const REG_ANAPAR_XTAL_0: u32 = 0x1040; // reg.h:791
pub const XCAP_MASK: u32 = 0x7f; // rtw8822c.h:183  GENMASK(6, 0)

// ── DACK (rtw8822c.h:139-142, main.h:1625-1626, rtw8822c.h:229-232) ──
pub const DACK_PATH_8822C: usize = 2; // rtw8822c.h:139
pub const DACK_REG_8822C: usize = 16; // rtw8822c.h:140
pub const DACK_RF_8822C: usize = 1; // rtw8822c.h:141
pub const DACK_SN_8822C: usize = 100; // rtw8822c.h:142
pub const DACK_MSBK_BACKUP_NUM: usize = 15; // main.h:1625
pub const DACK_DCK_BACKUP_NUM: usize = 2; // main.h:1626
pub const REG_DCKA_I_0: u32 = 0x18BC; // rtw8822c.h:229
pub const REG_DCKA_I_1: u32 = 0x18C0; // rtw8822c.h:230
pub const REG_DCKA_Q_0: u32 = 0x18D8; // rtw8822c.h:231
pub const REG_DCKA_Q_1: u32 = 0x18DC; // rtw8822c.h:232
pub const REG_DCKB_I_0: u32 = 0x41BC; // rtw8822c.h:359
pub const REG_DCKB_I_1: u32 = 0x41C0; // rtw8822c.h:360
pub const REG_DCKB_Q_0: u32 = 0x41D8; // rtw8822c.h:361
pub const REG_DCKB_Q_1: u32 = 0x41DC; // rtw8822c.h:362

// ── RF-Register und ihre Felder (rtw8822c.h:384-406) ─────────────
pub const RF_PA: u32 = 0x60; // rtw8822c.h:384
pub const RF_PABIAS_2G_MASK: u32 = 0xf000; // rtw8822c.h:385  GENMASK(15, 12)
pub const RF_PABIAS_5G_MASK: u32 = 0xf0000; // rtw8822c.h:386  GENMASK(19, 16)
pub const RF_THEMAL_MASK: u32 = 0xf0000; // rtw8822c.h:406  GENMASK(19, 16)

// ── Werkskalibrierung in der PHYSISCHEN efuse (rtw8822c.h:405-428) ──
pub const PPG_THERMAL_B: u16 = 0x1B0; // rtw8822c.h:405
pub const PPG_2GH_TXAB: u16 = 0x1D2; // rtw8822c.h:407
pub const PPG_2G_A_MASK: u8 = 0x0f; // rtw8822c.h:408  GENMASK(3, 0)
pub const PPG_2G_B_MASK: u8 = 0xf0; // rtw8822c.h:409  GENMASK(7, 4)
pub const PPG_2GL_TXAB: u16 = 0x1D4; // rtw8822c.h:410
pub const PPG_PABIAS_2GB: u16 = 0x1D5; // rtw8822c.h:411
pub const PPG_PABIAS_2GA: u16 = 0x1D6; // rtw8822c.h:412
pub const PPG_PABIAS_MASK: u8 = 0x0f; // rtw8822c.h:413  GENMASK(3, 0)
pub const PPG_PABIAS_5GB: u16 = 0x1D7; // rtw8822c.h:414
pub const PPG_PABIAS_5GA: u16 = 0x1D8; // rtw8822c.h:415
pub const PPG_5G_MASK: u8 = 0x1f; // rtw8822c.h:416  GENMASK(4, 0)
pub const PPG_5GH1_TXB: u16 = 0x1DB; // rtw8822c.h:417
pub const PPG_5GH1_TXA: u16 = 0x1DC; // rtw8822c.h:418
pub const PPG_5GM2_TXB: u16 = 0x1DF; // rtw8822c.h:419
pub const PPG_5GM2_TXA: u16 = 0x1E0; // rtw8822c.h:420
pub const PPG_5GM1_TXB: u16 = 0x1E3; // rtw8822c.h:421
pub const PPG_5GM1_TXA: u16 = 0x1E4; // rtw8822c.h:422
pub const PPG_5GL2_TXB: u16 = 0x1E7; // rtw8822c.h:423
pub const PPG_5GL2_TXA: u16 = 0x1E8; // rtw8822c.h:424
pub const PPG_5GL1_TXB: u16 = 0x1EB; // rtw8822c.h:425
pub const PPG_5GL1_TXA: u16 = 0x1EC; // rtw8822c.h:426
pub const PPG_2GM_TXAB: u16 = 0x1EE; // rtw8822c.h:427
pub const PPG_THERMAL_A: u16 = 0x1EF; // rtw8822c.h:428
/// efuse.h:13 — und zugleich der Wert, den ein LEERES efuse-Byte hat.
pub const EFUSE_READ_FAIL: u8 = 0xff;

// ── Adaptivity / EDCCA ───────────────────────────────────────────
pub const RTW8822C_EDCCA_MAX: u8 = 0x7f; // rtw8822c.h:180
pub const BIT_DIS_EDCCA: u32 = 1 << 15; // reg.h:464
pub const BIT_EDCCA_MSK_CNTDOWN_EN: u32 = 1 << 11; // reg.h:470
pub const REG_EDCCA_DECISION: u32 = 0x0844; // rtw8822c.h:193
pub const BIT_EDCCA_OPTION: u32 = 0x6000_0000; // rtw8822c.h:194  GENMASK(30, 29)
/// rtw8822c.c:5287-5294 `rtw8822c_edcca_th` — Adresse, Maske und der
/// Versatz, der auf den Schwellwert addiert wird.
pub const EDCCA_TH_L2H: (u32, u32, u8) = (0x84c, 0x00ff_0000, 0x80);
pub const EDCCA_TH_H2L: (u32, u32, u8) = (0x84c, 0xff00_0000, 0x80);

// ── Beamforming-Grundeinstellung (bf.h) ──────────────────────────
pub const REG_TXBF_CTRL: u32 = 0x042C; // bf.h:8
pub const REG_NDPA_OPT_CTRL: u32 = 0x045F; // bf.h:10
pub const REG_MU_TX_CTL: u32 = 0x14C0; // bf.h:19
pub const REG_WMAC_MU_BF_OPTION: u32 = 0x167C; // bf.h:23
pub const REG_WMAC_MU_BF_CTL: u32 = 0x1680; // bf.h:24
pub const BIT_WMAC_TXMU_ACKPOLICY_EN: u8 = 1 << 6; // bf.h:27
pub const BIT_USE_NDPA_PARAMETER: u32 = 1 << 30; // bf.h:28
pub const BIT_MU_P1_WAIT_STATE_EN: u32 = 1 << 16; // bf.h:29
pub const BIT_EN_MU_MIMO: u32 = 1 << 7; // bf.h:30
pub const BIT_SHIFT_R_MU_RL: u32 = 12; // bf.h:33
pub const BIT_SHIFT_WMAC_TXMU_ACKPOLICY: u32 = 4; // bf.h:34
pub const BIT_MASK_R_MU_RL: u32 = 0xf000; // bf.h:37  GENMASK(15, 12)
pub const BIT_MASK_R_MU_TABLE_VALID: u32 = 0x3f; // bf.h:38  GENMASK(5, 0)
pub const BIT_MASK_CSI_RATE: u32 = 0x3f00_0000; // bf.h:40  GENMASK(29, 24)
pub const DESC_RATE6M: u32 = 0x04; // main.h:255

// ── Falschalarm-Zaehler (rtw8822c.h:243-347) ─────────────────────
pub const REG_CCK_FACNT: u32 = 0x1A5C; // rtw8822c.h:245
pub const REG_OFDM_FACNT1: u32 = 0x2D04; // rtw8822c.h:343
pub const REG_OFDM_FACNT2: u32 = 0x2D08; // rtw8822c.h:344
pub const REG_OFDM_FACNT3: u32 = 0x2D0C; // rtw8822c.h:345
pub const REG_OFDM_FACNT4: u32 = 0x2D10; // rtw8822c.h:346
pub const REG_OFDM_FACNT5: u32 = 0x2D20; // rtw8822c.h:347
pub const BIT_CCK_FA_RST: u32 = 0xc000; // rtw8822c.h:243  GENMASK(15, 14)
pub const BIT_OFDM_FA_RST: u32 = 0x3000; // rtw8822c.h:244  GENMASK(13, 12)
pub const REG_RX_BREAK: u32 = 0x1D2C; // rtw8822c.h:324
pub const BIT_COM_RX_GCK_EN: u32 = 1 << 31; // rtw8822c.h:325
pub const REG_CNT_CTRL: u32 = 0x1EB4; // rtw8822c.h:339
pub const BIT_ALL_CNT_RST: u32 = 1 << 25; // rtw8822c.h:340

// ════════════════════════════════════════════════════════════════
// Stufe 4a: der Rest von rtw_power_on und rtw_core_start
// ════════════════════════════════════════════════════════════════

// ── H2C-MAILBOX (reg.h:291-307) ──────────────────────────────────
// Der eine von ZWEI H2C-Wegen: acht Byte je Kommando, vier Postfaecher
// im Umlauf. Die Koexistenz redet hierueber mit der Firmware.
pub const REG_HMETFR: u32 = 0x01CC; // reg.h:291
pub const REG_HMEBOX0: u32 = 0x01D0; // reg.h:298
pub const REG_HMEBOX1: u32 = 0x01D4; // reg.h:299
pub const REG_HMEBOX2: u32 = 0x01D8; // reg.h:300
pub const REG_HMEBOX3: u32 = 0x01DC; // reg.h:301
pub const REG_HMEBOX0_EX: u32 = 0x01F0; // reg.h:304
pub const REG_HMEBOX1_EX: u32 = 0x01F4; // reg.h:305
pub const REG_HMEBOX2_EX: u32 = 0x01F8; // reg.h:306
pub const REG_HMEBOX3_EX: u32 = 0x01FC; // reg.h:307

// ── H2C-PAKET (fw.h) ─────────────────────────────────────────────
// Der ANDERE Weg: 32 Byte durch die H2C-Queue, also durch den Ring,
// dessen Adresse `init_h2c` in Stufe 3a gesetzt hat. General- und
// PHYDM-Info gehen hier durch.
pub const H2C_PKT_SIZE: usize = 32; // fw.h:8
pub const H2C_PKT_HDR_SIZE: u16 = 8; // fw.h:9
pub const H2C_PKT_CMD_ID: u32 = 0xFF; // fw.h:386
pub const H2C_PKT_CATEGORY: u32 = 0x01; // fw.h:387
pub const H2C_PKT_GENERAL_INFO: u32 = 0x0D; // fw.h:389
pub const H2C_PKT_PHYDM_INFO: u32 = 0x11; // fw.h:390
pub const FW_RF_2T2R: u8 = 0x2; // fw.h:134
pub const FW_RF_1T1R: u8 = 0x4; // fw.h:136

// ── Sicherheits-Engine (sec.h) ───────────────────────────────────
pub const RTW_SEC_CONFIG: u32 = 0x0680; // sec.h:11
pub const RTW_SEC_TX_UNI_USE_DK: u16 = 1 << 0; // sec.h:19
pub const RTW_SEC_RX_UNI_USE_DK: u16 = 1 << 1; // sec.h:20
pub const RTW_SEC_TX_DEC_EN: u16 = 1 << 2; // sec.h:21
pub const RTW_SEC_RX_DEC_EN: u16 = 1 << 3; // sec.h:22
pub const RTW_SEC_TX_BC_USE_DK: u16 = 1 << 6; // sec.h:23
pub const RTW_SEC_RX_BC_USE_DK: u16 = 1 << 7; // sec.h:24
pub const RTW_SEC_ENGINE_EN: u16 = 1 << 9; // sec.h:26

// ── Empfangsfilter (reg.h:502-533) ───────────────────────────────
// `hal->rcr` wird in main.c:2183 gesetzt und in `rtw_core_start`
// (main.c:1526) NOCH EINMAL ins Register geschrieben — nach allem, was
// `rtw8822c_mac_init` und `rtw_drv_info_cfg` dort hinterlassen haben.
// Der Kommentar dort lautet „rcr reset after powered on".
pub const BIT_APP_FCS: u32 = 1 << 31; // reg.h:503
pub const BIT_APP_MIC: u32 = 1 << 30; // reg.h:504
pub const BIT_APP_ICV: u32 = 1 << 29; // reg.h:505
pub const BIT_VHT_DACK: u32 = 1 << 26; // reg.h:508
pub const BIT_PKTCTL_DLEN: u32 = 1 << 20; // reg.h:514
pub const BIT_HTC_LOC_CTRL: u32 = 1 << 14; // reg.h:520
pub const BIT_AB: u32 = 1 << 3; // reg.h:531
pub const BIT_AM: u32 = 1 << 2; // reg.h:532
pub const BIT_APM: u32 = 1 << 1; // reg.h:533

// ── Koexistenz (reg.h) ───────────────────────────────────────────
pub const REG_WIFI_BT_INFO: u32 = 0x00AA; // reg.h:184
pub const BIT_BT_INT_EN: u16 = 1 << 15; // reg.h:185
pub const REG_BT_COEX_TABLE_H: u32 = 0x06CC; // reg.h:573
pub const H2C_CMD_QUERY_BT_INFO: u32 = 0x61; // fw.h:568
pub const H2C_CMD_BT_WIFI_CONTROL: u32 = 0x69; // fw.h:573

// ── Koexistenz: Zustaende (coex.h, Aufzaehlungen ohne Zahlen) ────
// Sie stehen in `enum` ohne Wert, also zaehlt die POSITION. Abgezaehlt
// aus coex.h:74-85 bzw. 129-138 — und darum ist die Quellzeile hier
// wichtiger als sonst.
pub const COEX_SET_ANT_INIT: u8 = 0; // coex.h:75
pub const COEX_SET_ANT_WONLY: u8 = 1; // coex.h:76
pub const COEX_SET_ANT_WOFF: u8 = 2; // coex.h:77
pub const COEX_SET_ANT_2G: u8 = 3; // coex.h:78
pub const COEX_SET_ANT_5G: u8 = 4; // coex.h:79
pub const COEX_SET_ANT_POWERON: u8 = 5; // coex.h:80
pub const COEX_SET_ANT_2G_WLBT: u8 = 6; // coex.h:81
pub const COEX_SET_ANT_2G_FREERUN: u8 = 7; // coex.h:82
pub const COEX_SWITCH_CTRL_BY_BBSW: u8 = 0; // coex.h:130
pub const COEX_SWITCH_CTRL_BY_PTA: u8 = 1; // coex.h:131
pub const COEX_SWITCH_CTRL_BY_BT: u8 = 4; // coex.h:134
pub const COEX_SWITCH_CTRL_MAX: u8 = 6; // coex.h:137
pub const COEX_SWITCH_TO_MAX: u8 = 5; // coex.h:126 — fuenf Eintraege, nicht sieben

pub const COEX_GNT_SET_HW_PTA: u32 = 0x0; // coex.h:114
pub const COEX_GNT_SET_SW_LOW: u32 = 0x1; // coex.h:115
pub const COEX_GNT_SET_SW_HIGH: u32 = 0x3; // coex.h:116

pub const COEX_SCBD_ACTIVE: u16 = 0x0001; // coex.h:181
pub const COEX_SCBD_ONOFF: u16 = 0x0002; // coex.h:182
pub const COEX_SCBD_BT_RFK: u16 = 0x0020; // coex.h:186
pub const COEX_SCBD_TDMA: u16 = 1 << 9; // coex.h:189
pub const COEX_SCBD_FIX2M: u16 = 1 << 10; // coex.h:190
pub const COEX_SCBD_ALL: u16 = 0xffff; // coex.h:191  GENMASK(15, 0)

pub const COEX_H2C69_TDMA_SLOT: u8 = 0x0B; // coex.h:23
pub const PARA1_H2C69_TDMA_4SLOT: u8 = 0xC1; // coex.h:24
pub const PARA1_H2C69_TDMA_2SLOT: u8 = 0x01; // coex.h:25
pub const COEX_H2C69_WL_LEAKAP: u8 = 0x0C; // coex.h:19
pub const PARA1_H2C69_EN_5MS: u8 = 0x00; // coex.h:21
pub const PARA1_H2C69_DIS_5MS: u8 = 0x01; // coex.h:20
pub const COEX_H2C69_TOGGLE_TABLE_A: u8 = 0x0D; // coex.h:26

pub const TDMA_4SLOT: u32 = 0x100; // coex.h:32
pub const TDMA_TIMER_TYPE_2SLOT: u8 = 0; // coex.h:34
pub const TDMA_TIMER_TYPE_4SLOT: u8 = 3; // coex.h:35
pub const COEX_MIN_DELAY: u32 = 10; // coex.h:12
pub const COEX_RFK_TIMEOUT: u32 = 600; // coex.h:13
pub const COEX_WLPRI_TX_RSP: u8 = 3; // coex.h:265
pub const COEX_WLPRI_TX_BEACON: u8 = 4; // coex.h:266
pub const COEX_WLPRI_TX_BEACONQ: u8 = 27; // coex.h:269

// ── Koexistenz: Register ─────────────────────────────────────────
pub const REG_BT_COEX_TABLE0: u32 = 0x06C0; // reg.h:570
pub const REG_BT_COEX_TABLE1: u32 = 0x06C4; // reg.h:571
pub const REG_BT_COEX_BRK_TABLE: u32 = 0x06C8; // reg.h:572
pub const REG_BT_STAT_CTRL: u32 = 0x0778; // reg.h:589
pub const REG_BT_TDMA_TIME: u32 = 0x0790; // reg.h:590
pub const BIT_MASK_SAMPLE_RATE: u32 = 0x3f; // reg.h:591
pub const BIT_BT_PTA_EN: u32 = 1 << 5; // reg.h:77
pub const BIT_PO_BT_PTA_PINS: u32 = 1 << 9; // reg.h:76
pub const REG_QUEUE_CTRL: u32 = 0x04C6; // reg.h:432
pub const BIT_PTA_WL_TX_EN: u8 = 1 << 4; // reg.h:433
pub const BIT_PTA_EDCCA_EN: u8 = 1 << 5; // reg.h:434
pub const REG_BT_COEX_V2: u32 = 0x0762; // reg.h:579
pub const BIT_GNT_BT_POLARITY: u16 = 1 << 12; // reg.h:580
pub const REG_DUMMY_PAGE4_V1: u32 = 0x04FC; // reg.h:445
pub const BIT_BTCCA_CTRL: u8 = 0x3; // reg.h:441  GENMASK(1, 0)
pub const RF_MODOPT: u32 = 0x01; // reg.h:957
pub const REG_SYS_SDIO_CTRL: u32 = 0x0070; // reg.h:111
pub const BIT_DBG_GNT_WL_BT: u32 = 1 << 27; // reg.h:112
pub const BIT_LTE_MUX_CTRL_PATH: u32 = 1 << 26; // reg.h:113
pub const BIT_BTGP_JTAG_EN: u32 = 1 << 24; // reg.h:104
pub const BIT_BTGP_SPI_EN: u32 = 1 << 20; // reg.h:105
pub const BIT_LED1DIS: u32 = 1 << 15; // reg.h:106
pub const BIT_WL_RFK: u8 = 1 << 0; // reg.h:426
pub const BIT_LTE_COEX_EN: u32 = 1 << 7; // reg.h:581
pub const LTE_COEX_CTRL: u16 = 0x38; // reg.h:1000
pub const LTE_WL_TRX_CTRL: u16 = 0xa0; // reg.h:1001
pub const LTE_BT_TRX_CTRL: u16 = 0xa4; // reg.h:1002
pub const MASKLWORD: u32 = 0x0000ffff; // phy.h:177
pub const H2C_CMD_COEX_TDMA_TYPE: u32 = 0x60; // fw.h:567
pub const REG_IGN_GNT_BT1: u32 = 0x1860; // reg.h:910
pub const REG_NOMASK_TXBT: u32 = 0x1CA7; // reg.h:927
pub const REG_ANAPAR: u32 = 0x1C30; // reg.h:928
pub const BIT_ANAPAR_BTPS: u32 = 1 << 22; // reg.h:929
pub const REG_RSTB_SEL: u32 = 0x1C38; // reg.h:930
pub const BIT_DAC_OFF_ENABLE: u32 = 1 << 4; // reg.h:931
pub const BIT_PI_IGNORE_GNT_BT: u32 = 1 << 3; // reg.h:932
pub const BIT_NOMASK_TXBT_ENABLE: u32 = 1 << 3; // reg.h:933
pub const REG_IGN_GNTBT4: u32 = 0x4160; // reg.h:940
pub const COEX_WLINK_5G: u8 = 0x3; // coex.h:175

// ════════════════════════════════════════════════════════════════
// Stufe 4c: rtw_set_channel
// ════════════════════════════════════════════════════════════════

// ── rtw8822c_set_channel_bb ──────────────────────────────────────
pub const REG_TXDFIR0: u32 = 0x0808; // rtw8822c.h:188
pub const REG_DFIRBW: u32 = 0x0810; // rtw8822c.h:189
pub const REG_SBD: u32 = 0x088C; // rtw8822c.h:198
pub const BITS_SUBTUNE: u32 = 0xf000; // rtw8822c.h:199
pub const REG_TXBWCTL: u32 = 0x09B0; // rtw8822c.h:202
pub const REG_TXCLK: u32 = 0x09B4; // rtw8822c.h:203
pub const REG_SCOTRK: u32 = 0x0C30; // rtw8822c.h:205
pub const REG_PT_CHSMO: u32 = 0x0CBC; // rtw8822c.h:209
pub const BIT_PT_OPT: u32 = 1 << 21; // rtw8822c.h:210
pub const REG_RXAGCCTL0: u32 = 0x18AC; // rtw8822c.h:226
pub const BITS_RXAGC_CCK: u32 = 0xf000; // rtw8822c.h:227
pub const BITS_RXAGC_OFDM: u32 = 0x01f0; // rtw8822c.h:228
pub const REG_CCKSB: u32 = 0x1A00; // rtw8822c.h:234
pub const REG_BGCTRL: u32 = 0x1A14; // rtw8822c.h:237
pub const BITS_RX_IQ_WEIGHT: u32 = 0x300; // rtw8822c.h:238
pub const REG_TXF0: u32 = 0x1A20; // rtw8822c.h:239
pub const REG_TXF1: u32 = 0x1A24; // rtw8822c.h:240
pub const REG_TXF2: u32 = 0x1A28; // rtw8822c.h:241
pub const REG_CCKTXONLY: u32 = 0x1A80; // rtw8822c.h:246
pub const BIT_BB_CCK_CHECK_EN: u32 = 1 << 18; // rtw8822c.h:247
pub const REG_TXF3: u32 = 0x1A98; // rtw8822c.h:248
pub const REG_TXF4: u32 = 0x1A9C; // rtw8822c.h:249
pub const REG_TXF5: u32 = 0x1AA0; // rtw8822c.h:250
pub const REG_TXF6: u32 = 0x1AAC; // rtw8822c.h:251
pub const REG_TXF7: u32 = 0x1AB0; // rtw8822c.h:252
pub const REG_CCK_SOURCE: u32 = 0x1ABC; // rtw8822c.h:253
pub const BIT_NBI_EN: u32 = 1 << 30; // rtw8822c.h:254
pub const REG_CCAMSK: u32 = 0x1C80; // rtw8822c.h:315
pub const REG_RXAGCCTL: u32 = 0x41AC; // rtw8822c.h:358
pub const REG_CCK_CHECK: u32 = 0x0454; // reg.h:408
pub const BIT_CHECK_CCK_EN: u8 = 1 << 7; // reg.h:409

// ── rtw8822c_rstb_3wire / set_channel_rf ─────────────────────────
pub const REG_ANAPAR_A: u32 = 0x1830; // rtw8822c.h:220
pub const BIT_ANAPAR_UPDATE: u32 = 1 << 29; // rtw8822c.h:221
pub const REG_ANAPAR_B: u32 = 0x4130; // rtw8822c.h:354
pub const REG_RSTB: u32 = 0x1C90; // rtw8822c.h:316
pub const BIT_RSTB_3WIRE: u32 = 1 << 8; // rtw8822c.h:317
pub const RF_CFGCH: u32 = 0x18; // reg.h:961
pub const RF_LUTWA: u32 = 0x33; // reg.h:971
pub const RF_LUTWD0: u32 = 0x3F; // reg.h:973
pub const RF_LUTWE2: u32 = 0xEE; // reg.h:997

// ── rtw_set_channel_mac ──────────────────────────────────────────
pub const REG_DATA_SC: u32 = 0x0483; // reg.h:419
pub const REG_WMAC_TRXPTCL_CTL: u32 = 0x0668; // reg.h:547
pub const BIT_RFMOD: u32 = 0x180; // reg.h:548  GENMASK(8, 7)
pub const BIT_RFMOD_80M: u32 = 1 << 8; // reg.h:549
pub const BIT_RFMOD_40M: u32 = 1 << 7; // reg.h:550
pub const MAC_CLK_HW_DEF_80M: u32 = 0; // reg.h:269
pub const BIT_SHIFT_MAC_CLK_SEL: u32 = 20; // reg.h:268

// ── Unterkanallage (main.h:106-112) ──────────────────────────────
pub const RTW_SC_DONT_CARE: u8 = 0; // main.h:106
pub const RTW_SC_20_UPPER: u8 = 1; // main.h:107
pub const RTW_SC_20_LOWER: u8 = 2; // main.h:108
pub const RTW_SC_20_UPMOST: u8 = 3; // main.h:109
pub const RTW_SC_20_LOWEST: u8 = 4; // main.h:110
pub const RTW_SC_40_UPPER: u8 = 9; // main.h:111
pub const RTW_SC_40_LOWER: u8 = 10; // main.h:112

pub const MASKBYTE0: u32 = 0xff; // phy.h:172
pub const MASKHWORD: u32 = 0xffff0000; // phy.h:176
pub const MASKDWORD: u32 = 0xffffffff; // phy.h:178

// ── DPD-Abzug je Rate (reg.h:659-668) ────────────────────────────
pub const DIS_DPD_RATE6M: u16 = 1 << 0; // reg.h:659
pub const DIS_DPD_RATE9M: u16 = 1 << 1; // reg.h:660
pub const DIS_DPD_RATEMCS0: u16 = 1 << 2; // reg.h:661
pub const DIS_DPD_RATEMCS1: u16 = 1 << 3; // reg.h:662
pub const DIS_DPD_RATEMCS8: u16 = 1 << 4; // reg.h:663
pub const DIS_DPD_RATEMCS9: u16 = 1 << 5; // reg.h:664
pub const DIS_DPD_RATEVHT1SS_MCS0: u16 = 1 << 6; // reg.h:665
pub const DIS_DPD_RATEVHT1SS_MCS1: u16 = 1 << 7; // reg.h:666
pub const DIS_DPD_RATEVHT2SS_MCS0: u16 = 1 << 8; // reg.h:667
pub const DIS_DPD_RATEVHT2SS_MCS1: u16 = 1 << 9; // reg.h:668

/// rtw8822c.c:5352 `.txgi_factor = 2` · :5385 `.en_dis_dpd = true`
/// · :5386 `.dpd_ratemask = DIS_DPD_RATEALL`
pub const TXGI_FACTOR: i16 = 2;
pub const EN_DIS_DPD: bool = true;
pub const DPD_RATEMASK: u16 = 0x3ff; // = DIS_DPD_RATEALL
pub const BIT_SHIFT_TXSC_40M: u32 = 4; // reg.h:260
pub const BIT_MASK_TXSC_40M: u8 = 0xf; // reg.h:261
pub const BIT_SHIFT_TXSC_20M: u32 = 0; // reg.h:264
pub const BIT_MASK_TXSC_20M: u8 = 0xf; // reg.h:265

// ── Stufe 5b: der Sendeweg ───────────────────────────────────────
// mac80211.c:108-115 `rtw_vif_port[0]`. Wir fahren nur Port 0 — Linux
// vergibt den ersten freien, und bei EINER Schnittstelle ist das die Null.
pub const PORT0_MAC_ADDR: u32 = 0x0610;
pub const PORT0_BSSID: u32 = 0x0618;
pub const PORT0_NET_TYPE: u32 = 0x0100; // = REG_CR
pub const PORT0_NET_TYPE_MASK: u32 = 0x30000;
pub const PORT0_AID: u32 = 0x06a8;
pub const PORT0_AID_MASK: u32 = 0x7ff;
pub const PORT0_BCN_CTRL: u32 = 0x0550; // = REG_BCN_CTRL
pub const PORT0_BCN_CTRL_MASK: u32 = 0xff;

// main.h:586-592 `enum rtw_vif_port_set`
pub const PORT_SET_MAC_ADDR: u32 = 1 << 0;
pub const PORT_SET_BSSID: u32 = 1 << 1;
pub const PORT_SET_NET_TYPE: u32 = 1 << 2;
pub const PORT_SET_AID: u32 = 1 << 3;
pub const PORT_SET_BCN_CTRL: u32 = 1 << 4;

// main.h:115-120 `enum rtw_net_type`
pub const RTW_NET_NO_LINK: u32 = 0;
pub const RTW_NET_AD_HOC: u32 = 1;
pub const RTW_NET_MGD_LINKED: u32 = 2;
pub const RTW_NET_AP_MODE: u32 = 3;

// reg.h:478-480 stehen schon oben (REG_BCN_CTRL, BIT_DIS_TSF_UDT,
// BIT_EN_BCN_FUNCTION) — dort als u8, weil `rtw_write8_mask` sie schreibt.

/// fw.h:564 `H2C_CMD_SCAN`
pub const H2C_CMD_SCAN: u32 = 0x59;
/// fw.h:151 `FW_FEATURE_NOTIFY_SCAN`
pub const FW_FEATURE_NOTIFY_SCAN: u32 = 1 << 6;

// ── Stufe 5d: die RF-Kalibrierung ────────────────────────────────
/// rtw8822c.h:21 `IQK_DONE_8822C`
pub const IQK_DONE_8822C: u8 = 0xaa;
/// rtw8822c.h:256-264
pub const REG_NCTL0: u32 = 0x1b00;
pub const BIT_SEL_PATH: u32 = 0x6; // GENMASK(2, 1)
pub const BIT_SUBPAGE: u32 = 0xf; // GENMASK(3, 0)
pub const REG_DPD_CTL0_S0: u32 = 0x1b04;
pub const REG_DPD_CTL1_S0: u32 = 0x1b08;
pub const BIT_DPD_EN: u32 = 1 << 31;
pub const BIT_PS_EN: u32 = 1 << 7;
pub const REG_IQKSTAT: u32 = 0x1b10;
pub const REG_IQK_CTL1: u32 = 0x1b20;
pub const BIT_TX_CFIR: u32 = 0xc000_0000; // GENMASK(31, 30)
/// rtw8822c.h:348-349
pub const REG_RPT_CIP: u32 = 0x2d9c;
pub const BIT_RPT_CIP_STATUS: u32 = 0xff; // GENMASK(7, 0)
/// reg.h:163-164
pub const REG_PMC_DBG_CTRL1: u32 = 0xa8;
pub const BITS_PMC_BT_IQK_STS: u32 = 0x0060_0000; // GENMASK(22, 21)
// reg.h:425-426 REG_ARFR4/BIT_WL_RFK stehen schon oben.
/// fw.h:574 `H2C_CMD_WIFI_CALIBRATION`
pub const H2C_CMD_WIFI_CALIBRATION: u32 = 0x6d;
/// fw.h:391 `H2C_PKT_IQK`
pub const H2C_PKT_IQK: u8 = 0x0E;

// ── Stufe 5d: TXGAPK (rtw8822c.c:1191-1823) ─────────────────────

// NICHT GEFUNDEN — von Hand nachsehen:
//   BITS_RFC_DIRECT (Ausdruck: (BIT(31) | BIT(30)))
pub const REG_RFTXEN_GCK_A: u32 = 0x1864; // rtw8822c.h:201
pub const REG_RFTXEN_GCK_B: u32 = 0x4164; // rtw8822c.h:334
pub const BIT_RFTXEN_GCK_FORCE_ON: u32 = 1 << 31; // rtw8822c.h:202
pub const BIT_DIS_SHARERX_TXGAT: u32 = 1 << 27; // rtw8822c.h:194
pub const REG_DIS_SHARE_RX_A: u32 = 0x186c; // rtw8822c.h:203
pub const REG_DIS_SHARE_RX_B: u32 = 0x416c; // rtw8822c.h:335
pub const BIT_TX_SCALE_0DB: u32 = 1 << 7; // rtw8822c.h:204
pub const BIT_3WIRE_EN: u32 = 0x00000003; // rtw8822c.h:197  GENMASK(1, 0)
pub const BIT_BBMODE: u32 = 0x00000006; // rtw8822c.h:214  GENMASK(2, 1)
pub const REG_IQK_CTRL: u32 = 0x1c38; // rtw8822c.h:290
pub const RF_DEBUG: u32 = 0xde; // rtw8822c.h:379
pub const BIT_DE_TX_GAIN: u32 = 1 << 16; // rtw8822c.h:381
pub const RF_DIS_BYPASS_TXBB: u32 = 0x9e; // rtw8822c.h:376
pub const BIT_TIA_BYPASS: u32 = 1 << 5; // rtw8822c.h:378
pub const BIT_TXBB: u32 = 1 << 10; // rtw8822c.h:377
pub const REG_SINGLE_TONE_SW: u32 = 0x1bb8; // rtw8822c.h:263
pub const BIT_IRQ_TEST_MODE: u32 = 1 << 20; // rtw8822c.h:264
pub const REG_R_CONFIG: u32 = 0x1bcc; // rtw8822c.h:265
pub const BIT_CFIR_EN: u32 = 0x07000000; // rtw8822c.h:246  GENMASK(26, 24)
pub const BIT_GAIN_TX_PAD_H: u32 = 0x00000f00; // rtw8822c.h:361  GENMASK(11, 8)
pub const BIT_GAIN_TX_PAD_L: u32 = 0x000000f0; // rtw8822c.h:362  GENMASK(7, 4)
pub const REG_TABLE_SEL: u32 = 0x1b98; // rtw8822c.h:255
pub const BIT_Q_GAIN_SEL: u32 = 0x00007000; // rtw8822c.h:258  GENMASK(14, 12)
pub const BIT_Q_GAIN: u32 = 0x00000fff; // rtw8822c.h:259  GENMASK(11, 0)
pub const BIT_I_GAIN: u32 = 0x000f0000; // rtw8822c.h:256  GENMASK(19, 16)
pub const BIT_GAIN_RST: u32 = 1 << 15; // rtw8822c.h:257
pub const REG_TX_GAIN_SET: u32 = 0x1b9c; // rtw8822c.h:260
pub const RF_GAIN_NUM: u32 = 11; // main.h:1648
pub const BIT_ANT_PATH: u32 = 0x00000003; // rtw8822c.h:170  GENMASK(1, 0)
pub const REG_TXANTSEG: u32 = 0x1e28; // rtw8822c.h:312
pub const BIT_ANTSEG: u32 = 0x0000000f; // rtw8822c.h:313  GENMASK(3, 0)
pub const BIT_PATH_EN: u32 = 1 << 31; // rtw8822c.h:192
pub const RF_LUTDBG: u32 = 0xdf; // reg.h:965
pub const BIT_TXA_TANK: u32 = 1 << 4; // reg.h:966
pub const RF_IDAC: u32 = 0x58; // rtw8822c.h:358
pub const BIT_TX_MODE: u32 = 0x000fff00; // rtw8822c.h:359  GENMASK(19, 8)
pub const REG_TX_TONE_IDX: u32 = 0x1b2c; // rtw8822c.h:249
pub const BIT_2G_SWING: u32 = 0x2d; // rtw8822c.h:268
pub const BIT_5G_SWING: u32 = 0x36; // rtw8822c.h:269
pub const REG_RXSRAM_CTL: u32 = 0x1bd4; // rtw8822c.h:270
pub const BIT_RPT_EN: u32 = 1 << 21; // rtw8822c.h:271
pub const BIT_RPT_SEL: u32 = 0x001f0000; // rtw8822c.h:272  GENMASK(20, 16)
pub const BIT_GAPK_RPT_IDX: u32 = 0x00000f00; // rtw8822c.h:261  GENMASK(11, 8)
pub const REG_STAT_RPT: u32 = 0x1bfc; // rtw8822c.h:278
pub const BIT_GAPK_RPT0: u32 = 0x0000000f; // rtw8822c.h:280  GENMASK(3, 0)
pub const BIT_GAPK_RPT1: u32 = 0x000000f0; // rtw8822c.h:281  GENMASK(7, 4)
pub const BIT_GAPK_RPT2: u32 = 0x00000f00; // rtw8822c.h:282  GENMASK(11, 8)
pub const BIT_GAPK_RPT3: u32 = 0x0000f000; // rtw8822c.h:283  GENMASK(15, 12)
pub const BIT_GAPK_RPT4: u32 = 0x000f0000; // rtw8822c.h:284  GENMASK(19, 16)
pub const BIT_GAPK_RPT5: u32 = 0x00f00000; // rtw8822c.h:285  GENMASK(23, 20)
pub const BIT_GAPK_RPT6: u32 = 0x0f000000; // rtw8822c.h:286  GENMASK(27, 24)
pub const BIT_GAPK_RPT7: u32 = 0xf0000000; // rtw8822c.h:287  GENMASK(31, 28)
pub const RF_HW_OFFSET_NUM: u32 = 10; // main.h:1649
pub const BIT_IQ_SWITCH: u32 = 0x0000003f; // rtw8822c.h:267  GENMASK(5, 0)
pub const RF_MODE_TRXAGC: u32 = 0x00; // rtw8822c.h:343
pub const RF_TX_GAIN_OFFSET: u32 = 0x55; // rtw8822c.h:353
pub const BIT_RF_GAIN: u32 = 0x0000001c; // rtw8822c.h:355  GENMASK(4, 2)
pub const RF_RXG_GAIN: u32 = 0x87; // rtw8822c.h:370
pub const BIT_RXG_GAIN: u32 = 1 << 18; // rtw8822c.h:371
pub const BIT_RXAGC: u32 = 0x000003e0; // rtw8822c.h:345  GENMASK(9, 5)
pub const BIT_DE_TRXBW: u32 = 1 << 2; // rtw8822c.h:382
pub const RF_BW_TRXBB: u32 = 0x1a; // rtw8822c.h:348
pub const BIT_BW_TXBB: u32 = 0x00007000; // rtw8822c.h:350  GENMASK(14, 12)
pub const BIT_BW_RXBB: u32 = 0x00000c00; // rtw8822c.h:351  GENMASK(11, 10)
pub const RF_EXT_TIA_BW: u32 = 0x8f; // rtw8822c.h:374
pub const BIT_PW_EXT_TIA: u32 = 1 << 1; // rtw8822c.h:375
pub const RF_TXA_LB_SW: u32 = 0x63; // rtw8822c.h:366
pub const BIT_TXA_LB_ATT: u32 = 0x0000c000; // rtw8822c.h:367  GENMASK(15, 14)
pub const BIT_LB_ATT: u32 = 0x0000001c; // rtw8822c.h:369  GENMASK(4, 2)
pub const BIT_LB_SW: u32 = 0x00003000; // rtw8822c.h:368  GENMASK(13, 12)
pub const RF_RXA_MIX_GAIN: u32 = 0x8a; // rtw8822c.h:372
pub const BIT_RXA_MIX_GAIN: u32 = 0x00000018; // rtw8822c.h:373  GENMASK(4, 3)
pub const BIT_RF_MODE: u32 = 0x000f0000; // rtw8822c.h:344  GENMASK(19, 16)
pub const BIT_GAIN_EXT: u32 = 1 << 12; // reg.h:944
pub const BIT_DATA_L: u32 = 0x00000fff; // reg.h:945  GENMASK(11, 0)
pub const BIT_BAND: u32 = 0x00070000; // reg.h:932  GENMASK(18, 16)
pub const BIT_DBG_CCK_CCA: u32 = 1 << 1; // rtw8822c.h:352
pub const BIT_TX_CCK_IND: u32 = 1 << 16; // rtw8822c.h:349
pub const RF_TX_RESULT: u32 = 0x5f; // rtw8822c.h:360
pub const RF_BAND_2G_CCK: u32 = 0; // main.h:1639
pub const RF_BAND_2G_OFDM: u32 = 1; // main.h:1639
pub const RF_BAND_5G_L: u32 = 2; // main.h:1639
pub const RF_BAND_5G_M: u32 = 3; // main.h:1639
pub const RF_BAND_5G_H: u32 = 4; // main.h:1639
pub const RF_BAND_MAX: u32 = 5; // main.h:1639
pub const RTW_DM_CAP_TXGAPK: u32 = 1; // main.h:1687
pub const REG_TX_FIFO: u32 = 0x1e70; // rtw8822c.h:316
pub const BIT_STOP_TX: u32 = 0x0000000f; // rtw8822c.h:317  GENMASK(3, 0)
pub const BIT_AC_QUEUE: u32 = 0x000000ff; // reg.h:452  GENMASK(7, 0)
pub const REG_ENFN: u32 = 0x1e24; // rtw8822c.h:310
pub const BIT_IQK_DPK_EN: u32 = 1 << 17; // rtw8822c.h:311
pub const REG_CH_DELAY_EXTR2: u32 = 0x1cd0; // rtw8822c.h:297
pub const BIT_IQK_DPK_CLOCK_SRC: u32 = 1 << 28; // rtw8822c.h:301
pub const BIT_IQK_DPK_RESET_SRC: u32 = 1 << 29; // rtw8822c.h:300
pub const BIT_EN_IOQ_IQK_DPK: u32 = 1 << 30; // rtw8822c.h:299
pub const BIT_TST_IQK2SET_SRC: u32 = 1 << 31; // rtw8822c.h:298
pub const REG_CCA_OFF: u32 = 0x1d58; // rtw8822c.h:306
pub const BIT_CCA_ON_BY_PW: u32 = 0x00000ff8; // rtw8822c.h:307  GENMASK(11, 3)
pub const BITS_RFC_DIRECT: u32 = 0xc0000000; // reg.h:36  (BIT(31) | BIT(30))

// ── Stufe 5d: DPK (rtw8822c.c:3171-4186) ────────────────────────
pub const BIT_DPD_CLK: u32 = 0x000000f0; // rtw8822c.h:273  GENMASK(7, 4)
pub const DPK_RF_REG_NUM: u32 = 7; // main.h:1575
pub const DPK_BB_REG_NUM: u32 = 18; // main.h:1577
pub const DPK_RF_PATH_NUM: u32 = 2; // main.h:1576
pub const RF_RXAGC_OFFSET: u32 = 0x19; // rtw8822c.h:347
pub const REG_DPD_CTL1_S1: u32 = 0x1b60; // rtw8822c.h:253
pub const REG_DPD_LUT0: u32 = 0x1b44; // rtw8822c.h:250
pub const BIT_GLOSS_DB: u32 = 0x00007000; // rtw8822c.h:251  GENMASK(14, 12)
pub const REG_DPD_CTL11: u32 = 0x1be4; // rtw8822c.h:274
pub const REG_DPD_CTL12: u32 = 0x1be8; // rtw8822c.h:275
pub const RF_TX_GAIN: u32 = 0x56; // rtw8822c.h:356
pub const BIT_DE_PWR_TRIM: u32 = 1 << 19; // rtw8822c.h:380
pub const BIT_BB_GAIN: u32 = 0x0007c000; // rtw8822c.h:354  GENMASK(18, 14)
pub const DPK_CHANNEL_WIDTH_80: u32 = 1; // main.h:1578
pub const RTW_DPK_GAIN_LOSS: u32 = 1; // rtw8822c.h:118
pub const RTW_DPK_DO_DPK: u32 = 2; // rtw8822c.h:118
pub const RTW_DPK_DPK_ON: u32 = 3; // rtw8822c.h:118
pub const RTW_DPK_DAGC: u32 = 4; // rtw8822c.h:118
pub const RTW_DPK_CAL_PWR: u32 = 0; // rtw8822c.h:118
pub const REG_DPD_CTL0: u32 = 0x1bb4; // rtw8822c.h:262
pub const RF_T_METER: u32 = 0x42; // reg.h:946
pub const RTW_DPK_GAIN_LESS: u32 = 2; // rtw8822c.h:108
pub const RTW_DPK_GAIN_LARGE: u32 = 1; // rtw8822c.h:108
pub const RTW_DPK_GL_LESS: u32 = 4; // rtw8822c.h:108
pub const RTW_DPK_GL_LARGE: u32 = 3; // rtw8822c.h:108
pub const RTW_DPK_AGC_OUT: u32 = 6; // rtw8822c.h:108
pub const RTW_DPK_LOSS_CHECK: u32 = 5; // rtw8822c.h:108
pub const RTW_DPK_GAIN_CHECK: u32 = 0; // rtw8822c.h:108
pub const BIT_GAIN_TXBB: u32 = 0x0000001f; // rtw8822c.h:357  GENMASK(4, 0)
pub const BIT_TXAGC: u32 = 0x0000001f; // rtw8822c.h:346  GENMASK(4, 0)
pub const REG_DPD_CTL0_S1: u32 = 0x1b5c; // rtw8822c.h:252
pub const REG_DPD_AGC: u32 = 0x1b67; // rtw8822c.h:254
pub const BIT_BYPASS_DPD: u32 = 1 << 25; // rtw8822c.h:247
pub const BIT_INNER_LB: u32 = 1 << 21; // rtw8822c.h:266
pub const BIT_GS_PWSF: u32 = 0x0fffffff; // rtw8822c.h:239  GENMASK(27, 0)
pub const REG_DPD_CTL16: u32 = 0x1bf8; // rtw8822c.h:277
pub const REG_DPD_CTL15: u32 = 0x1bf4; // rtw8822c.h:276
pub const MASKBYTE3: u32 = 0xff000000; // phy.h:156
pub const BIT_RPT_DGAIN: u32 = 0x0fff0000; // rtw8822c.h:279  GENMASK(27, 16)
pub const MASKBYTE1: u32 = 0xff00; // phy.h:154

/// main.h:99 `RTW_BAND_2G = BIT(NL80211_BAND_2GHZ)` — `NL80211_BAND_2GHZ`
/// ist 0, also BIT(0). Der Name steht in einem anderen Baum (cfg80211),
/// deshalb hier ausgerechnet statt erzeugt.
pub const RTW_BAND_2G_MASK: u32 = 1 << 0;

// ── Stufe 5e: Verbinden ─────────────────────────────────────────
pub const H2C_CMD_MEDIA_STATUS_RPT: u32 = 0x01; // fw.h:484
pub const C2H_CCX_TX_RPT: u32 = 0x03; // fw.h:51
pub const C2H_BT_INFO: u32 = 0x09; // fw.h:51
pub const C2H_BT_MP_INFO: u32 = 0x0b; // fw.h:51
pub const C2H_BT_HID_INFO: u32 = 0x45; // fw.h:51
pub const C2H_RA_RPT: u32 = 0x0c; // fw.h:51
pub const C2H_WLAN_INFO: u32 = 0x27; // fw.h:51
pub const C2H_WLAN_RFON: u32 = 0x32; // fw.h:51
pub const C2H_BCN_FILTER_NOTIFY: u32 = 0x36; // fw.h:51
pub const C2H_ADAPTIVITY: u32 = 0x37; // fw.h:51
pub const C2H_SCAN_RESULT: u32 = 0x38; // fw.h:51
pub const C2H_HALMAC: u32 = 0xff; // fw.h:51

// ── Stufe 5f: Ratenanpassung (main.c:1117-1240) ─────────────────
pub const IEEE80211_HT_CAP_SGI_20: u32 = 0x0020; // ../../../../../include/linux/ieee80211.h:1917
pub const IEEE80211_HT_CAP_SGI_40: u32 = 0x0040; // ../../../../../include/linux/ieee80211.h:1918
pub const IEEE80211_HT_CAP_RX_STBC: u32 = 0x0300; // ../../../../../include/linux/ieee80211.h:1920
pub const IEEE80211_HT_CAP_LDPC_CODING: u32 = 0x0001; // ../../../../../include/linux/ieee80211.h:1912
pub const IEEE80211_HT_CAP_SUP_WIDTH_20_40: u32 = 0x0002; // ../../../../../include/linux/ieee80211.h:1913
pub const IEEE80211_VHT_CAP_SHORT_GI_80: u32 = 0x00000020; // ../../../../../include/linux/ieee80211.h:2438
pub const IEEE80211_VHT_CAP_RXSTBC_MASK: u32 = 0x00000700; // ../../../../../include/linux/ieee80211.h:2445
pub const IEEE80211_VHT_CAP_RXLDPC: u32 = 0x00000010; // ../../../../../include/linux/ieee80211.h:2437
pub const IEEE80211_VHT_CAP_SUPP_CHAN_WIDTH_MASK: u32 = 0x0000000C; // ../../../../../include/linux/ieee80211.h:2435
pub const WLAN_EID_HT_CAPABILITY: u32 = 45; // ../../../../../include/linux/ieee80211.h:3659
pub const WLAN_EID_VHT_CAPABILITY: u32 = 191; // ../../../../../include/linux/ieee80211.h:3659
pub const WLAN_EID_SUPP_RATES: u32 = 1; // ../../../../../include/linux/ieee80211.h:3659
pub const WLAN_EID_EXT_SUPP_RATES: u32 = 50; // ../../../../../include/linux/ieee80211.h:3659
pub const WLAN_EID_SSID: u32 = 0; // ../../../../../include/linux/ieee80211.h:3659
pub const WLAN_EID_RSN: u32 = 48; // ../../../../../include/linux/ieee80211.h:3659
pub const WLAN_EID_DS_PARAMS: u32 = 3; // ../../../../../include/linux/ieee80211.h:3659
pub const RA_MASK_CCK_RATES: u32 = 0x0000f; // main.c:1117
pub const RA_MASK_OFDM_RATES: u32 = 0x00ff0; // main.c:1118
pub const RA_MASK_HT_RATES_1SS: u32 = 0xff000; // main.c:1119  (0xff000ULL << 0)
pub const RA_MASK_HT_RATES_2SS: u32 = 0xff00000; // main.c:1120  (0xff000ULL << 8)
pub const RA_MASK_HT_RATES_3SS: u64 = 0xff0000000; // main.c:1121  (0xff000ULL << 16)
pub const RA_MASK_HT_RATES: u64 = 0xffffff000; // main.c:1122  (RA_MASK_HT_RATES_1SS |  				 RA_MASK_HT_RATES_2SS |  				 RA_MASK_HT_RATES_3SS)
pub const RA_MASK_VHT_RATES_1SS: u32 = 0x3ff000; // main.c:1123  (0x3ff000ULL << 0)
pub const RA_MASK_VHT_RATES_2SS: u32 = 0xffc00000; // main.c:1124  (0x3ff000ULL << 10)
pub const RA_MASK_VHT_RATES_3SS: u64 = 0x3ff00000000; // main.c:1125  (0x3ff000ULL << 20)
pub const RA_MASK_VHT_RATES: u64 = 0x3fffffff000; // main.c:1126  (RA_MASK_VHT_RATES_1SS |  				 RA_MASK_VHT_RATES_2SS |  				 RA_MASK_VHT_RATES_3SS)
pub const RA_MASK_CCK_IN_BG: u32 = 0x00005; // main.c:1127
pub const RA_MASK_CCK_IN_HT: u32 = 0x00005; // main.c:1128
pub const RA_MASK_CCK_IN_VHT: u32 = 0x00005; // main.c:1129
pub const RA_MASK_OFDM_IN_VHT: u32 = 0x00010; // main.c:1130
pub const RA_MASK_OFDM_IN_HT_2G: u32 = 0x00010; // main.c:1131
pub const RA_MASK_OFDM_IN_HT_5G: u32 = 0x00030; // main.c:1132
pub const WIRELESS_CCK: u32 = 0x00000001; // main.h:176
pub const WIRELESS_OFDM: u32 = 0x00000002; // main.h:176
pub const WIRELESS_HT: u32 = 0x00000004; // main.h:176
pub const WIRELESS_VHT: u32 = 0x00000008; // main.h:176
pub const RRSR_INIT_2G: u32 = 0x15f; // main.h:1684
pub const RRSR_INIT_5G: u32 = 0x150; // main.h:1685
pub const RTW_RATEID_BG: u32 = 6; // main.h:226
pub const RTW_RATEID_GN_N1SS: u32 = 5; // main.h:226
pub const RTW_RATEID_GN_N2SS: u32 = 4; // main.h:226
pub const RTW_RATEID_BGN_20M_1SS: u32 = 3; // main.h:226
pub const RTW_RATEID_BGN_20M_2SS: u32 = 2; // main.h:226
pub const RTW_RATEID_BGN_40M_1SS: u32 = 1; // main.h:226
pub const RTW_RATEID_BGN_40M_2SS: u32 = 0; // main.h:226
pub const RTW_RATEID_ARFR1_AC_1SS: u32 = 10; // main.h:226
pub const RTW_RATEID_ARFR0_AC_2SS: u32 = 9; // main.h:226
pub const RTW_RATEID_ARFR2_AC_2G_1SS: u32 = 11; // main.h:226
pub const RTW_RATEID_ARFR3_AC_2G_2SS: u32 = 12; // main.h:226
pub const H2C_CMD_RA_INFO: u32 = 0x40; // fw.h:488
pub const RTW_C2H_RA_RPT_RATE: u32 = 0x0000007f; // fw.h:98  GENMASK(6, 0)
pub const RTW_C2H_RA_RPT_SGI: u32 = 1 << 7; // fw.h:99
pub const VHT_STBC_EN: u8 = 1 << 1; // main.h:184
pub const VHT_LDPC_EN: u8 = 1 << 1; // main.h:186
pub const HT_STBC_EN: u8 = 1 << 0; // main.h:183
pub const HT_LDPC_EN: u8 = 1 << 0; // main.h:185
pub const H2C_CMD_DEFAULT_PORT: u32 = 0x2c; // fw.h:487
pub const RTW_H2C_DEFAULT_PORT_W0_PORTID: u32 = 0x0000ff00; // fw.h:109  GENMASK(15, 8)
pub const RTW_H2C_DEFAULT_PORT_W0_MACID: u32 = 0x00ff0000; // fw.h:110  GENMASK(23, 16)

// ── Stufe 5f.1: unsere EIGENEN Faehigkeiten (main.c:1580-1640) ──
pub const IEEE80211_HT_CAP_MAX_AMSDU: u32 = 0x0800; // ../../../../../include/linux/ieee80211.h:1923
pub const IEEE80211_HT_CAP_RX_STBC_SHIFT: u32 = 8; // ../../../../../include/linux/ieee80211.h:1921
pub const IEEE80211_HT_CAP_TX_STBC: u32 = 0x0080; // ../../../../../include/linux/ieee80211.h:1919
pub const IEEE80211_HT_CAP_DSSSCCK40: u32 = 0x1000; // ../../../../../include/linux/ieee80211.h:1924
pub const IEEE80211_HT_MAX_AMPDU_64K: u32 = 3; // ../../../../../include/linux/ieee80211.h:1947
pub const IEEE80211_HT_MPDU_DENSITY_2: u32 = 4; // ../../../../../include/linux/ieee80211.h:1972
pub const IEEE80211_HT_MCS_TX_DEFINED: u32 = 0x01; // ../../../../../include/linux/ieee80211.h:1867
pub const IEEE80211_VHT_CAP_MAX_MPDU_LENGTH_11454: u32 = 0x00000002; // ../../../../../include/linux/ieee80211.h:2431
pub const IEEE80211_VHT_CAP_RXSTBC_1: u32 = 0x00000100; // ../../../../../include/linux/ieee80211.h:2441
pub const IEEE80211_VHT_CAP_HTC_VHT: u32 = 0x00400000; // ../../../../../include/linux/ieee80211.h:2456
pub const IEEE80211_VHT_CAP_MAX_A_MPDU_LENGTH_EXPONENT_MASK: u32 = 0x3800000; // ../../../../../include/linux/ieee80211.h:2458  (7 << IEEE80211_VHT_CAP_MAX_A_MPDU_LENGTH_EXPONENT_SHIFT)
pub const IEEE80211_VHT_CAP_TXSTBC: u32 = 0x00000080; // ../../../../../include/linux/ieee80211.h:2440
pub const IEEE80211_VHT_CAP_MU_BEAMFORMEE_CAPABLE: u32 = 0x00100000; // ../../../../../include/linux/ieee80211.h:2454
pub const IEEE80211_VHT_CAP_SU_BEAMFORMEE_CAPABLE: u32 = 0x00001000; // ../../../../../include/linux/ieee80211.h:2448
pub const IEEE80211_VHT_CAP_BEAMFORMEE_STS_SHIFT: u32 = 13; // ../../../../../include/linux/ieee80211.h:2449
pub const IEEE80211_VHT_MCS_SUPPORT_0_9: u32 = 2; // ../../../../../include/linux/ieee80211.h:2107
pub const IEEE80211_VHT_MCS_NOT_SUPPORTED: u32 = 3; // ../../../../../include/linux/ieee80211.h:2107
pub const EFUSE_HW_CAP_PTCL_VHT: u32 = 3; // efuse.h:9

// ── Stufe 6a: der Steuerkanal (docs/spec/WIFI_CLASS_ABI.md §4) ───
// Abwaerts (Manager -> Treiber)
pub const CMD_SCAN: u8 = 0x01;
pub const CMD_CONNECT: u8 = 0x02;
pub const CMD_DISCONNECT: u8 = 0x03;
pub const CMD_SET_KEY: u8 = 0x04;
pub const CMD_TX_EAPOL: u8 = 0x05;
pub const CMD_TX_MGMT: u8 = 0x06;
pub const CMD_ASSOCIATED: u8 = 0x07;
pub const CMD_AUTHORIZED: u8 = 0x08;
// Aufwaerts (Treiber -> Manager)
pub const EV_SCAN_AP: u8 = 0x81;
pub const EV_SCAN_DONE: u8 = 0x82;
pub const EV_READY: u8 = 0x83;
pub const EV_EAPOL_RX: u8 = 0x84;
pub const EV_LINK_UP: u8 = 0x85;
pub const EV_LINK_DOWN: u8 = 0x86;
pub const EV_CONNECT_FAILED: u8 = 0x87;
pub const EV_RX_MGMT: u8 = 0x88;

/// 802.2 LLC/SNAP-Kopf vor jedem 802.11-Datenrahmen (RFC 1042).
pub const LLC_SNAP_HDR: [u8; 6] = [0xaa, 0xaa, 0x03, 0x00, 0x00, 0x00];
pub const ETHERTYPE_EAPOL: u16 = 0x888e;
/// `fc[0] & 0x0c == 0x08` — Datenrahmen.
pub const DOT11_FC_TYPE_DATA: u8 = 0x08;
/// `fc[1]` — die Nutzlast ist verschluesselt.
pub const DOT11_FC_PROTECTED: u8 = 0x40;
/// Subtyp-Bit: QoS-Daten, zwei Byte QoS-Control hinter dem Kopf.
pub const DOT11_STYPE_QOS: u8 = 0x08;
/// Subtyp-Bit: Null und QoS-Null tragen KEINEN Rumpf.
pub const DOT11_STYPE_NODATA: u8 = 0x04;
/// 802.11 §9.2.4.1 — das ganze erste Byte: Protokollfassung 0, Typ
/// VERWALTUNG (00), Subtyp 12 bzw. 10. Ein Deauth ist deshalb GENAU
/// `0xc0` und nicht eine Maske: `rx_to_8023` filtert in seiner ersten
/// Zeile auf Daten, und ohne diese zwei Werte faellt ein Rauswurf
/// lautlos durch.
pub const DOT11_FC_DEAUTH: u8 = 0xc0;
pub const DOT11_FC_DISASSOC: u8 = 0xa0;
/// docs/spec/WIFI_CLASS_ABI.md §4b — `EV_LINK_DOWN` traegt einen Grund:
/// 0 = angefordert, 1 = Deauth, 2 = verloren.
pub const LINK_DOWN_REQUESTED: u8 = 0;
pub const LINK_DOWN_DEAUTH: u8 = 1;
pub const LINK_DOWN_LOST: u8 = 2;
pub const RTW_SEC_CMD_REG: u32 = 0x670; // sec.h:8
pub const RTW_SEC_WRITE_REG: u32 = 0x674; // sec.h:9
pub const RTW_SEC_CAM_ENTRY_SHIFT: u32 = 3; // sec.h:13
pub const RTW_SEC_CMD_WRITE_ENABLE: u32 = 1 << 16; // sec.h:15
pub const RTW_SEC_CMD_POLLING: u32 = 1 << 31; // sec.h:17
pub const RTW_CAM_AES: u32 = 4; // main.h:713
pub const RTW_CAM_TKIP: u32 = 2; // main.h:713
pub const RTW_CAM_WEP40: u32 = 1; // main.h:713
pub const RTW_CAM_WEP104: u32 = 5; // main.h:713
pub const RTW_CAM_NONE: u32 = 0; // main.h:713
pub const DESC_RATE11M: u32 = 0x03; // main.h:246
pub const DESC_RATE54M: u32 = 0x0b; // main.h:246
pub const DESC_RATEMCS7: u32 = 0x13; // main.h:246
pub const DESC_RATEMCS15: u32 = 0x1b; // main.h:246

// ── rtw_watch_dog_work: die laufende Haelfte (main.c:224-310) ────
// Linux fuehrt sie ALLE 2 SEKUNDEN, das ganze Leben einer Verbindung
// lang. Bis 0.26.0 gab es sie bei uns nicht — und drei ihrer Posten
// sind Sendeseite und haengen an der Temperatur.
pub const RTW_WATCH_DOG_DELAY_MS: u64 = 2000; // main.h:30  HZ * 2
pub const RTW_TP_SHIFT: u32 = 18; // main.h:41  bytes/2s --> Mbps
pub const RTW_LPS_THRESHOLD: u32 = 50; // ps.h:8
pub const RTW_BUSY_TRAFFIC_THRESHOLD: u64 = 100; // main.c:241-244

// ── rtw_phy_dig (phy.c:373-437) ──────────────────────────────────
pub const DIG_PERF_FA_TH_LOW: u32 = 250; // phy.c:360
pub const DIG_PERF_FA_TH_HIGH: u32 = 500; // phy.c:361
pub const DIG_PERF_FA_TH_EXTRA_HIGH: u32 = 750; // phy.c:362
pub const DIG_PERF_MAX: u32 = 0x5a; // phy.c:363
pub const DIG_PERF_MID: u32 = 0x40; // phy.c:364
pub const DIG_CVRG_FA_TH_LOW: u32 = 2000; // phy.c:365
pub const DIG_CVRG_FA_TH_HIGH: u32 = 4000; // phy.c:366
pub const DIG_CVRG_FA_TH_EXTRA_HIGH: u32 = 5000; // phy.c:367
pub const DIG_CVRG_MAX: u32 = 0x2a; // phy.c:368
pub const DIG_CVRG_MID: u32 = 0x26; // phy.c:369
pub const DIG_CVRG_MIN: u32 = 0x1c; // phy.c:370
pub const DIG_RSSI_GAIN_OFFSET: u32 = 15; // phy.c:371
/// rtw8822c.c:5355 `.dig_min` — die Untergrenze DIESES Chips.
pub const RTW8822C_DIG_MIN: u8 = 0x20; // rtw8822c.c:5355

// ── rtw_phy_get_rssi_level (phy.c:292-309) ───────────────────────
pub const RA_FLOOR_TABLE_SIZE: usize = 7; // phy.c:289
pub const RA_FLOOR_UP_GAP: u8 = 3; // phy.c:290

// ── rtw_phy_cck_pd (phy.c:736-766) ───────────────────────────────
pub const CCK_PD_FA_LV1_MIN: u32 = 1000; // phy.c:711
pub const CCK_PD_FA_LV0_MAX: u32 = 500; // phy.c:712
pub const CCK_PD_IGI_LV4_VAL: u8 = 0x38; // phy.c:728
pub const CCK_PD_IGI_LV3_VAL: u8 = 0x2a; // phy.c:729
pub const CCK_PD_IGI_LV2_VAL: u8 = 0x24; // phy.c:730
pub const CCK_PD_RSSI_LV4_VAL: u8 = 32; // phy.c:731
pub const CCK_PD_RSSI_LV3_VAL: u8 = 32; // phy.c:732
pub const CCK_PD_RSSI_LV2_VAL: u8 = 24; // phy.c:733
pub const CCK_FA_AVG_RESET: u32 = 0xffffffff; // phy.h:174
pub const CCK_PD_LV0: u8 = 0; // phy.h:164
pub const CCK_PD_LV1: u8 = 1; // phy.h:165
pub const CCK_PD_LV2: u8 = 2; // phy.h:166
pub const CCK_PD_LV3: u8 = 3; // phy.h:167
pub const CCK_PD_LV4: u8 = 4; // phy.h:168
pub const CCK_PD_LV_MAX: u8 = 5; // phy.h:169
pub const RTW_CCK_PD_MAX: u32 = 255; // rtw8822c.c:4342
pub const RTW_CCK_CS_MAX: u32 = 31; // rtw8822c.c:4343
pub const RTW_CCK_CS_ERR1: u32 = 27; // rtw8822c.c:4344
pub const RTW_CCK_CS_ERR2: u32 = 29; // rtw8822c.c:4345

// ── rtw_phy_rrsr_update (phy.c:1062-1069) ────────────────────────
pub const RRSR_RATE_ORDER_MAX: u32 = 0xfffff; // phy.h:180
pub const RRSR_RATE_ORDER_CCK_LEN: u32 = 4; // phy.h:181

// ── rtw8822c_cfo_track (rtw8822c.c:4265-4330) ────────────────────
pub const CFO_TRK_ENABLE_TH: i32 = 20; // rtw8822c.h:163
pub const CFO_TRK_STOP_TH: i32 = 10; // rtw8822c.h:164
pub const CFO_TRK_ADJ_TH: i32 = 10; // rtw8822c.h:165

// ── rtw_phy_pwr_track (phy.c) / rtw8822c_pwr_track ───────────────
pub const RTW_PWR_TRK_TBL_SZ: usize = 30; // main.h:1127
pub const RTW_PWR_TRK_5G_NUM: usize = 3; // main.h:1125
pub const PWR_TRACK_MASK: u32 = 0x7f; // rtw8822c.c:4413
/// rtw8822c.c:5387-5388 — beide 8.
pub const RTW8822C_IQK_THRESHOLD: u8 = 8; // rtw8822c.c:5387
pub const RTW8822C_LCK_THRESHOLD: u8 = 8; // rtw8822c.c:5388
/// rtw8822c.c:5362 `.path_div_supported = true`
pub const RTW8822C_PATH_DIV_SUPPORTED: bool = true; // rtw8822c.c:5362
pub const DESC_RATE_MAX: usize = 84; // main.h:246

// ── rtw_phy_get_rrsr_mask (phy.c:1021-1049) ──────────────────────
pub const DESC_RATE1M: u32 = 0x00; // main.h:246
pub const DESC_RATEMCS0: u32 = 0x0c; // main.h:246
pub const DESC_RATEMCS8: u32 = 0x14; // main.h:246
pub const DESC_RATEMCS16: u32 = 0x1c; // main.h:246
pub const DESC_RATEMCS24: u32 = 0x24; // main.h:246
pub const DESC_RATEVHT1SS_MCS0: u32 = 0x2c; // main.h:246
pub const DESC_RATEVHT2SS_MCS0: u32 = 0x36; // main.h:246
pub const DESC_RATEVHT3SS_MCS0: u32 = 0x40; // main.h:246
pub const DESC_RATEVHT4SS_MCS0: u32 = 0x4a; // main.h:246

// ── rtw8822c_cfo_track / rtw8822c_do_lck (rtw8822c.c) ────────────
pub const BIT_XCAP_0: u32 = 0x00fffc00; // reg.h:776  GENMASK(23, 10)
pub const RF_SYN_CTRL: u32 = 0xbb; // reg.h:958
pub const RF_SYN_PFD: u32 = 0xb0; // reg.h:955
pub const RF_SYN_AAC: u32 = 0xc9; // reg.h:960
pub const RF_AAC_CTRL: u32 = 0xca; // reg.h:961
pub const RF_FAST_LCK: u32 = 0xcc; // reg.h:962

// ── rtw_coex_monitor_bt_ctr (coex.c:454-475) ─────────────────────
pub const REG_BT_ACT_STATISTICS: u32 = 0x0770; // reg.h:571
pub const REG_BT_ACT_STATISTICS_1: u32 = 0x0774; // reg.h:572
pub const REG_BT_COEX_ENH_INTR_CTRL: u32 = 0x76E; // reg.h:568
pub const BIT_R_GRANTALL_WLMASK: u32 = 1 << 3; // reg.h:569
pub const BIT_STATIS_BT_EN: u32 = 1 << 2; // reg.h:570

// ── rtw_phy_ra_track / rtw_fw_adaptivity (fw.c) ──────────────────
pub const H2C_CMD_RSSI_MONITOR: u32 = 0x42; // fw.h:559
pub const H2C_CMD_WL_PHY_INFO: u32 = 0x58; // fw.h:563
pub const H2C_CMD_ADAPTIVITY: u32 = 0x5A; // fw.h:565

// ── rtw8822c_adaptivity (rtw8822c.c:2178-2196) ───────────────────
pub const EDCCA_TH_L2H_LB: i8 = 48; // main.h:1663
pub const EDCCA_ADC_BACKOFF: i8 = 12; // main.h:1664
pub const EDCCA_L2H_H2L_DIFF: i8 = 7; // main.h:1667
pub const EDCCA_L2H_H2L_DIFF_NORMAL: i8 = 8; // main.h:1668
pub const EDCCA_IGI_L2H_DIFF: i8 = 8; // main.h:1666
pub const FW_FEATURE_BCN_FILTER: u32 = 1 << 5; // fw.h:144
pub const FW_FEATURE_ADAPTIVITY: u32 = 1 << 7; // fw.h:144

// ── rtw_fw_ra_report_handle (fw.c:265-323) ───────────────────────
/// rtw8822c.c:5359 `.c2h_ra_report_size = 7`
pub const C2H_RA_REPORT_SIZE: usize = 7; // rtw8822c.c:5359
