# `tools/wasm/wifi_rtl8822ce/src/regs.rs` @ 5e0102684

## L1-6 · `#![allow(dead_code)]`

```
//! Register und Bits, 1:1 aus Linux 6.18.26
//! `drivers/net/wireless/realtek/rtw88/reg.h` (und `pci.h`, `main.h`).
//!
//! Jede Zeile traegt ihre Quelle. Wer hier einen Wert aendert, ohne die
//! genannte Zeile gelesen zu haben, hat geraten — und genau das ist die
//! Regel, die bei diesem Chip nicht verhandelbar ist.
```

## L9-10 · `pub const RTL_VENDOR: u16 = 0x10ec;`

```
// ── PCI-Identitaet ───────────────────────────────────────────────
// rtw8822ce.c: PCI_DEVICE(PCI_VENDOR_ID_REALTEK, 0xC822) und 0xC82F.
```

## L15 · `pub const BAR_REG: u8 = 2;`

```
/// pci.c `rtw_pci_io_mapping`: `u8 bar_id = 2` — NICHT BAR0.
```

## L17-19 · `pub const BAR_PAGES: u16 = 16;`

```
/// 16 Seiten = 64 KiB. Die hoechste PCIe-Registeradresse, die rtw88 anfasst,
/// ist `RTK_PCI_TXBD_H2CQ_CSR` 0x1330; BB/RF liegen direkt im Fenster bis
/// ~0x5000. Der Kernel klemmt ohnehin auf die echte BAR-Groesse.
```

## L22 · `pub const REG_SYS_FUNC_EN: u32 = 0x0002; // reg.h:8`

```
// ── Systemregister (reg.h) ───────────────────────────────────────
```

## L23 · `pub const REG_SYS_FUNC_EN: u32 = 0x0002; // reg.h:8`

```
// reg.h:8
```

## L24 · `pub const REG_SYS_PW_CTRL: u32 = 0x0004; // reg.h:18`

```
// reg.h:18
```

## L25 · `pub const REG_RSV_CTRL: u32 = 0x001C; // reg.h:33`

```
// reg.h:33
```

## L26 · `pub const REG_MCUFW_CTRL: u32 = 0x0080; // reg.h:131`

```
// reg.h:131
```

## L27 · `pub const REG_SYS_CFG1: u32 = 0x00F0; // reg.h:186`

```
// reg.h:186
```

## L28 · `pub const REG_SYS_STATUS1: u32 = 0x00F4; // reg.h:202`

```
// reg.h:202
```

## L29 · `pub const REG_SYS_CFG2: u32 = 0x00FC; // reg.h:204`

```
// reg.h:204
```

## L30 · `pub const REG_CR: u32 = 0x0100; // reg.h:207`

```
// reg.h:207
```

## L32 · `pub const BIT_RTL_ID: u32 = 1 << 23;`

```
// REG_SYS_CFG1-Felder, reg.h:187-201
```

## L41 · `#[inline]`

```
/// reg.h:201 — `BIT_GET_CHIP_VER(x)`
```

## L47 · `#[inline]`

```
/// reg.h:195 — `BIT_GET_VENDOR_ID(x)`
```

## L53-56 · `#[inline]`

```
/// mac.h:9 — `cut_version_to_mask(cut)`. Waehlt in der Power-Sequenz aus,
/// welche Kommandos fuer DIESEN Chipschnitt gelten.
/// In C ist das ein int-Shift, der erst bei der Zuweisung auf 8 Bit
/// verkuerzt wird — `1u8 << n` waere ab cut 7 etwas anderes.
```

## L62 · `pub const REG_SYS_CLKR: u32 = 0x0008; // reg.h:28`

```
// ── Stufe 1: Power-Sequenz (reg.h, Zeilen wie angegeben) ─────────
```

## L63 · `pub const REG_SYS_CLKR: u32 = 0x0008; // reg.h:28`

```
// reg.h:28
```

## L68 · `pub const REG_RF_CTRL: u32 = 0x001F; // reg.h:38`

```
// reg.h:38
```

## L73 · `pub const BIT_FEN_BB_GLB_RST: u8 = 1 << 1; // reg.h:14`

```
// reg.h:14
```

## L74 · `pub const BIT_FEN_BB_RSTB: u8 = 1 << 0; // reg.h:15`

```
// reg.h:15
```

## L76 · `pub const REG_GPIO_MUXCFG: u32 = 0x0040; // reg.h:72`

```
// reg.h:72
```

## L77 · `pub const BIT_FSPI_EN: u32 = 1 << 19; // reg.h:73`

```
// reg.h:73
```

## L78 · `pub const BIT_WLRFE_4_5_EN: u32 = 1 << 2; // reg.h:78`

```
// reg.h:78
```

## L80 · `pub const REG_LED_CFG: u32 = 0x004C; // reg.h:82`

```
// reg.h:82
```

## L84 · `pub const REG_PAD_CTRL1: u32 = 0x0064; // reg.h:100`

```
// reg.h:100
```

## L88 · `pub const REG_HCI_OPT_CTRL: u32 = 0x0074; // reg.h:114`

```
// reg.h:114
```

## L89 · `pub const BIT_USB_SUS_DIS: u32 = 1 << 8; // reg.h:115`

```
// reg.h:115
```

## L91 · `pub const REG_WLRF1: u32 = 0x00EC; // reg.h:205`

```
// reg.h:205
```

## L94 · `pub const REG_CPU_DMEM_CON: u32 = 0x1080; // reg.h:793`

```
// reg.h:793
```

## L95 · `pub const BIT_WL_PLATFORM_RST: u32 = 1 << 16; // reg.h:794`

```
// reg.h:794
```

## L96 · `pub const BIT_DDMA_EN: u32 = 1 << 8; // reg.h:796`

```
// reg.h:796
```

## L98 · `pub const REG_CR_EXT: u32 = 0x1100; // reg.h:806`

```
// reg.h:806
```

## L100 · `pub const BIT_PFM_WOWL: u8 = 1 << 3;`

```
// REG_SYS_PW_CTRL / REG_APS_FSMCO teilen sich 0x0004 (reg.h:18-24).
```

## L106 · `pub const BIT_BOOT_FSPI_EN: u32 = 1 << 20;`

```
// REG_MCUFW_CTRL-Felder (reg.h:131-158)
```

## L111 · `pub const REG_SYS_CLK_CTRL: u32 = 0x0008; // reg.h:25`

```
// ── Stufe 2b: Firmware-Download (reg.h/mac.h, Zeilen wie angegeben) ──
```

## L112 · `pub const REG_SYS_CLK_CTRL: u32 = 0x0008; // reg.h:25`

```
// reg.h:25
```

## L114 · `pub const BIT_WLMCU_IOIF: u8 = 1 << 0; // reg.h:37`

```
// reg.h:37
```

## L115 · `pub const BIT_FEN_CPUEN: u8 = 1 << 2; // reg.h:12`

```
// reg.h:12
```

## L117 · `pub const REG_TXDMA_PQ_MAP: u32 = 0x010C; // reg.h:239`

```
// reg.h:239
```

## L118 · `pub const RTW_DMA_MAPPING_HIGH: u8 = 3; // main.h:1013`

```
// main.h:1013
```

## L120 · `pub const BIT_TXDMA_EN: u8 = 1 << 2; // reg.h:216`

```
// reg.h:216
```

## L121 · `pub const BIT_HCI_TXDMA_EN: u8 = 1 << 0; // reg.h:218`

```
// reg.h:218
```

## L122 · `pub const BIT_ENSWBCN: u32 = 1 << 8; // reg.h:210 (im 16-Bit-CR)`

```
// reg.h:210 (im 16-Bit-CR)
```

## L124 · `pub const REG_FIFOPAGE_CTRL_2: u32 = 0x0204; // reg.h:322`

```
// reg.h:322
```

## L128 · `pub const REG_TXDMA_STATUS: u32 = 0x0210; // reg.h:332`

```
// reg.h:332
```

## L131 · `pub const REG_RQPN_CTRL_2: u32 = 0x022C; // reg.h:348`

```
// reg.h:348
```

## L133 · `pub const REG_FIFOPAGE_INFO_1: u32 = 0x0230; // reg.h:350`

```
// reg.h:350
```

## L135 · `pub const REG_FWHW_TXQ_CTRL: u32 = 0x0420; // reg.h:387`

```
// reg.h:387
```

## L138 · `pub const REG_BCN_CTRL: u32 = 0x0550; // reg.h:478`

```
// reg.h:478
```

## L142 · `pub const REG_FW_DBG7: u32 = 0x10FC; // reg.h:803`

```
// reg.h:803
```

## L144 · `pub const ILLEGAL_KEY_GROUP: u32 = 0xFAAAAA00; // mac.h:14`

```
// mac.h:14
```

## L146 · `pub const REG_DDMA_CH0SA: u32 = 0x1200; // reg.h:812`

```
// reg.h:812
```

## L156 · `pub const REG_H2CQ_CSR: u32 = 0x1330; // reg.h:823`

```
// reg.h:823
```

## L159 · `pub const OCPBASE_TXBUF_88XX: u32 = 0x18780000;`

```
// mac.h:17-22
```

## L163 · `pub const BIT_FW_INIT_RDY: u32 = 1 << 15;`

```
// reg.h:135-153 — die Statusbits des Downloads
```

## L177 · `pub const LTECOEX_ACCESS_CTRL: u32 = 0x1700;`

```
// reg.h:897-903 — LTE-Koexistenz, indirekter Zugriff
```

## L183 · `pub const FW_HDR_SIZE: usize = 64;`

```
// fw.h:12-13
```

## L187 · `pub const REG_EFUSE_CTRL: u32 = 0x0030; // reg.h:48`

```
// ── Stufe 2c: efuse und hw_feature ───────────────────────────────
```

## L188 · `pub const REG_EFUSE_CTRL: u32 = 0x0030; // reg.h:48`

```
// reg.h:48
```

## L195 · `pub const REG_LDO_EFUSE_CTRL: u32 = 0x0034; // reg.h:62`

```
// reg.h:62
```

## L198 · `pub const REG_C2HEVT: u32 = 0x01A0; // reg.h:287`

```
// reg.h:287
```

## L199 · `pub const C2H_HW_FEATURE_DUMP: u8 = 0xfd;`

```
/// fw.h:63 — der Ausloeser, VOR dem Firmware-Download geschrieben.
```

## L201 · `pub const C2H_HW_FEATURE_REPORT: u8 = 0x19;`

```
/// fw.h:57 — die Antwort der Firmware.
```

## L203 · `pub const HW_FEATURE_LEN: usize = 13;`

```
/// main.h:39
```

## L206 · `pub const REG_ANAPARLDO_POW_MAC: u32 = 0x0029; // rtw8822c.h:181`

```
// rtw8822c.h:181
```

## L207 · `pub const BIT_LDOE25_PON: u8 = 1 << 0; // rtw8822c.h:182`

```
// rtw8822c.h:182
```

## L209 · `pub const EFUSE_HW_CAP_IGNORE: u8 = 0;`

```
// efuse.h:8-11
```

## L214 · `pub const SYS_FUNC_EN_8822C: u8 = 0xD8;`

```
/// rtw8822c.c `rtw8822c_hw_spec.sys_func_en`
```

## L217 · `pub const RTW_CHIP_VER_CUT_D: u8 = 0x03;`

```
/// main.h:967-973 — unser Geraet meldet 3.
```

## L220-222 · `pub const CR_POWER_OFF: u8 = 0xea;`

```
// ── Zustaende, die Linux als Klartextvergleich liest ─────────────
/// mac.c `rtw_mac_power_switch`: `rtw_read8(rtwdev, REG_CR) == 0xea`
/// heisst „MAC ist AUS". Ein 8-Bit-Lesezugriff, ausdruecklich.
```

## L224-225 · `pub const MCUFW_CTRL_FW_ALIVE: u16 = 0xc078;`

```
/// mac.c `rtw_mac_power_switch`: `rtw_read16(rtwdev, REG_MCUFW_CTRL) == 0xC078`
/// heisst „die Firmware laeuft noch".
```

## L228 · `pub const PCIE_RPWM_ADDR: u32 = 0x03d9;`

```
// ── HCI-Parameter, main.c `rtw_chip_parameter_setup` ─────────────
```

## L232 · `pub const TX_PKT_DESC_SZ: u32 = 48;`

```
// ── Erwartung fuer den 8822C (rtw8822c.c `rtw8822c_hw_spec`) ─────
```

## L241-243 · `pub const BIT_RXDMA_EN: u8 = 1 << 3; // reg.h:215`

```
// ════════════════════════════════════════════════════════════════
// Stufe 3a: rtw_mac_init
// ════════════════════════════════════════════════════════════════
```

## L245 · `pub const BIT_RXDMA_EN: u8 = 1 << 3; // reg.h:215`

```
// ── mac.c `txdma_queue_mapping` (reg.h:207-260) ──────────────────
```

## L246 · `pub const BIT_RXDMA_EN: u8 = 1 << 3; // reg.h:215`

```
// reg.h:215
```

## L247 · `pub const BIT_HCI_RXDMA_EN: u8 = 1 << 1; // reg.h:217`

```
// reg.h:217
```

## L248 · `pub const BIT_PROTOCOL_EN: u8 = 1 << 4; // reg.h:214`

```
// reg.h:214
```

## L249 · `pub const BIT_SCHEDULE_EN: u8 = 1 << 5; // reg.h:213`

```
// reg.h:213
```

## L250 · `pub const BIT_MACTXEN: u8 = 1 << 6; // reg.h:212`

```
// reg.h:212
```

## L251 · `pub const BIT_MACRXEN: u8 = 1 << 7; // reg.h:211`

```
// reg.h:211
```

## L252-253 · `pub const MAC_TRX_ENABLE: u8 = BIT_HCI_TXDMA_EN | BIT_HCI_RXDMA_EN | BIT_TXDMA_EN`

```
/// reg.h:219 — alle acht Bits, also 0xff. Der Name steht hier, weil Linux
/// ihn schreibt; die Zahl daneben ist kein Kommentar, sondern das Ergebnis.
```

## L257-258 · `pub const BIT_SHIFT_TXDMA_VOQ_MAP: u32 = 4; // reg.h:231`

```
// Die sechs Queue-Abbildungen sind Feldmakros: zwei Bit je Queue im
// 16-Bit-Wort REG_TXDMA_PQ_MAP (reg.h:233-258).
```

## L259 · `pub const BIT_SHIFT_TXDMA_VOQ_MAP: u32 = 4; // reg.h:231`

```
// reg.h:231
```

## L260 · `pub const BIT_SHIFT_TXDMA_VIQ_MAP: u32 = 6; // reg.h:235`

```
// reg.h:235
```

## L261 · `pub const BIT_SHIFT_TXDMA_BEQ_MAP: u32 = 8; // reg.h:243`

```
// reg.h:243
```

## L262 · `pub const BIT_SHIFT_TXDMA_BKQ_MAP: u32 = 10; // reg.h:247`

```
// reg.h:247
```

## L263 · `pub const BIT_SHIFT_TXDMA_MGQ_MAP: u32 = 12; // reg.h:251`

```
// reg.h:251
```

## L264 · `pub const BIT_SHIFT_TXDMA_HIQ_MAP: u32 = 14; // reg.h:255`

```
// reg.h:255
```

## L265 · `pub const BIT_MASK_TXDMA_QUEUE_MAP: u16 = 0x3; // reg.h:232 u.a., fuer alle sechs`

```
// reg.h:232 u.a., fuer alle sechs
```

## L267 · `pub const RTW_DMA_MAPPING_EXTRA: u8 = 0; // main.h:1010`

```
/// main.h:1010-1013 — die vier Zielprioritaeten.
```

## L268 · `pub const RTW_DMA_MAPPING_EXTRA: u8 = 0; // main.h:1010`

```
// main.h:1010
```

## L269 · `pub const RTW_DMA_MAPPING_LOW: u8 = 1; // main.h:1011`

```
// main.h:1011
```

## L270 · `pub const RTW_DMA_MAPPING_NORMAL: u8 = 2; // main.h:1012`

```
// main.h:1012
```

## L272 · `pub const TX_PAGE_SIZE_SHIFT: u32 = 7; // main.h:34`

```
// ── mac.c `rtw_set_trx_fifo_info` (mac.h:24-29, main.h:34-35) ────
```

## L273 · `pub const TX_PAGE_SIZE_SHIFT: u32 = 7; // main.h:34`

```
// main.h:34
```

## L274 · `pub const TX_PAGE_SIZE: u32 = 1 << TX_PAGE_SIZE_SHIFT; // main.h:35`

```
// main.h:35
```

## L275 · `pub const RSVD_PG_DRV_NUM: u16 = 16; // mac.h:24`

```
// mac.h:24
```

## L276 · `pub const RSVD_PG_H2C_EXTRAINFO_NUM: u16 = 24; // mac.h:25`

```
// mac.h:25
```

## L277 · `pub const RSVD_PG_H2C_STATICINFO_NUM: u16 = 8; // mac.h:26`

```
// mac.h:26
```

## L278 · `pub const RSVD_PG_H2CQ_NUM: u16 = 8; // mac.h:27`

```
// mac.h:27
```

## L279 · `pub const RSVD_PG_CPU_INSTRUCTION_NUM: u16 = 0; // mac.h:28`

```
// mac.h:28
```

## L280 · `pub const RSVD_PG_FW_TXBUF_NUM: u16 = 4; // mac.h:29`

```
// mac.h:29
```

## L281 · `pub const C2H_PKT_BUF: u32 = 256; // mac.h:11`

```
// mac.h:11
```

## L282 · `pub const PHY_STATUS_SIZE: u8 = 4; // mac.h:13`

```
// mac.h:13
```

## L284-285 · `pub const TXFF_SIZE_8822C: u32 = 262144; // rtw8822c.c:5345`

```
/// rtw8822c.c `rtw8822c_hw_spec` — die drei Zahlen, aus denen der Seitenplan
/// faellt. `page_size` ist `TX_PAGE_SIZE`.
```

## L286 · `pub const TXFF_SIZE_8822C: u32 = 262144; // rtw8822c.c:5345`

```
// rtw8822c.c:5345
```

## L287 · `pub const RXFF_SIZE_8822C: u32 = 24576; // rtw8822c.c:5346`

```
// rtw8822c.c:5346
```

## L288 · `pub const RSVD_DRV_PG_NUM_8822C: u16 = 16; // rtw8822c.c:5348`

```
// rtw8822c.c:5348
```

## L289 · `pub const CSI_BUF_PG_NUM_8822C: u16 = 50; // rtw8822c.c:5352`

```
// rtw8822c.c:5352
```

## L291 · `pub const REG_FIFOPAGE_INFO_2: u32 = 0x0234; // reg.h:351`

```
// ── mac.c `__priority_queue_cfg` ─────────────────────────────────
```

## L292 · `pub const REG_FIFOPAGE_INFO_2: u32 = 0x0234; // reg.h:351`

```
// reg.h:351
```

## L293 · `pub const REG_FIFOPAGE_INFO_3: u32 = 0x0238; // reg.h:352`

```
// reg.h:352
```

## L294 · `pub const REG_FIFOPAGE_INFO_4: u32 = 0x023C; // reg.h:353`

```
// reg.h:353
```

## L295 · `pub const REG_FIFOPAGE_INFO_5: u32 = 0x0240; // reg.h:354`

```
// reg.h:354
```

## L296 · `pub const BIT_EN_WR_FREE_TAIL: u32 = 1 << 20; // reg.h:389`

```
// reg.h:389
```

## L297 · `pub const REG_BCNQ_BDNY_V1: u32 = 0x0424; // reg.h:392`

```
// reg.h:392
```

## L298 · `pub const REG_BCNQ1_BDNY_V1: u32 = 0x0456; // reg.h:411`

```
// reg.h:411
```

## L299 · `pub const REG_RXFF_BNDY: u32 = 0x011C; // reg.h:276`

```
// reg.h:276
```

## L300 · `pub const REG_AUTO_LLT_V1: u32 = 0x0208; // reg.h:325`

```
// reg.h:325
```

## L301 · `pub const BIT_AUTO_INIT_LLT_V1: u32 = 1 << 0; // reg.h:326`

```
// reg.h:326
```

## L303 · `pub const REG_H2C_HEAD: u32 = 0x0244; // reg.h:355`

```
// ── mac.c `init_h2c` ─────────────────────────────────────────────
```

## L304 · `pub const REG_H2C_HEAD: u32 = 0x0244; // reg.h:355`

```
// reg.h:355
```

## L305 · `pub const REG_H2C_TAIL: u32 = 0x0248; // reg.h:356`

```
// reg.h:356
```

## L306 · `pub const REG_H2C_READ_ADDR: u32 = 0x024C; // reg.h:357`

```
// reg.h:357
```

## L307 · `pub const REG_H2C_INFO: u32 = 0x0254; // reg.h:358`

```
// reg.h:358
```

## L308 · `pub const REG_TXDMA_OFFSET_CHK: u32 = 0x020C; // reg.h:330`

```
// reg.h:330
```

## L309 · `pub const REG_H2C_PKT_READADDR: u32 = 0x10D0; // reg.h:800`

```
// reg.h:800
```

## L310 · `pub const REG_H2C_PKT_WRITEADDR: u32 = 0x10D4; // reg.h:801`

```
// reg.h:801
```

## L312 · `pub const REG_RX_DRVINFO_SZ: u32 = 0x060F; // reg.h:536`

```
// ── mac.c `rtw_drv_info_cfg` ─────────────────────────────────────
```

## L313 · `pub const REG_RX_DRVINFO_SZ: u32 = 0x060F; // reg.h:536`

```
// reg.h:536
```

## L314 · `pub const REG_TRXFF_BNDY: u32 = 0x0114; // reg.h:275`

```
// reg.h:275
```

## L315 · `pub const REG_RCR: u32 = 0x0608; // reg.h:502`

```
// reg.h:502
```

## L316 · `pub const BIT_APP_PHYSTS: u32 = 1 << 28; // reg.h:506`

```
// reg.h:506
```

## L317 · `pub const REG_WMAC_OPTION_FUNCTION: u32 = 0x07D0; // reg.h:595`

```
// reg.h:595
```

## L319 · `pub const REG_HCI_MIX_CFG: u32 = 0x03FC; // reg.h:381`

```
// ── pci.c `rtw_pci_interface_cfg` ────────────────────────────────
```

## L320 · `pub const REG_HCI_MIX_CFG: u32 = 0x03FC; // reg.h:381`

```
// reg.h:381
```

## L321 · `pub const BIT_PCIE_EMAC_PDN_AUX_TO_FAST_CLK: u32 = 1 << 26; // reg.h:382`

```
// reg.h:382
```

## L323 · `pub const REG_SPEC_SIFS: u32 = 0x0428; // reg.h:397`

```
// ── rtw8822c.c `rtw8822c_mac_init` — Register ────────────────────
```

## L324 · `pub const REG_SPEC_SIFS: u32 = 0x0428; // reg.h:397`

```
// reg.h:397
```

## L325 · `pub const REG_SIFS: u32 = 0x0514; // reg.h:457`

```
// reg.h:457
```

## L326 · `pub const REG_RESP_SIFS_CCK: u32 = 0x063C; // reg.h:542`

```
// reg.h:542
```

## L327 · `pub const REG_RESP_SIFS_OFDM: u32 = 0x063E; // reg.h:543`

```
// reg.h:543
```

## L328 · `pub const REG_DARFRC: u32 = 0x0430; // reg.h:399`

```
// reg.h:399
```

## L329 · `pub const REG_DARFRCH: u32 = 0x0434; // reg.h:400`

```
// reg.h:400
```

## L330 · `pub const REG_RARFRCH: u32 = 0x043C; // reg.h:401`

```
// reg.h:401
```

## L331 · `pub const REG_ARFR0: u32 = 0x0444; // reg.h:404`

```
// reg.h:404
```

## L332 · `pub const REG_ARFRH0: u32 = 0x0448; // reg.h:405`

```
// reg.h:405
```

## L333 · `pub const REG_ARFR1_V1: u32 = 0x044C; // reg.h:406`

```
// reg.h:406
```

## L334 · `pub const REG_ARFRH1_V1: u32 = 0x0450; // reg.h:407`

```
// reg.h:407
```

## L335 · `pub const REG_ARFR4: u32 = 0x049C; // reg.h:425`

```
// reg.h:425
```

## L336 · `pub const REG_ARFRH4: u32 = 0x04A0; // reg.h:427`

```
// reg.h:427
```

## L337 · `pub const REG_ARFR5: u32 = 0x04A4; // reg.h:428`

```
// reg.h:428
```

## L338 · `pub const REG_ARFRH5: u32 = 0x04A8; // reg.h:429`

```
// reg.h:429
```

## L339 · `pub const REG_AMPDU_MAX_TIME_V1: u32 = 0x0455; // reg.h:410`

```
// reg.h:410
```

## L340 · `pub const REG_TX_HANG_CTRL: u32 = 0x045E; // reg.h:415`

```
// reg.h:415
```

## L341 · `pub const BIT_EN_EOF_V1: u8 = 1 << 2; // reg.h:417`

```
// reg.h:417
```

## L342 · `pub const REG_PRECNT_CTRL: u32 = 0x04E5; // reg.h:440`

```
// reg.h:440
```

## L343 · `pub const BIT_EN_PRECNT: u16 = 1 << 11; // reg.h:442`

```
// reg.h:442
```

## L344 · `pub const REG_PROT_MODE_CTRL: u32 = 0x04C8; // reg.h:437`

```
// reg.h:437
```

## L345 · `pub const REG_BAR_MODE_CTRL: u32 = 0x04CC; // reg.h:439`

```
// reg.h:439
```

## L346 · `pub const REG_FAST_EDCA_VOVI_SETTING: u32 = 0x1448; // reg.h:825`

```
// reg.h:825
```

## L347 · `pub const REG_FAST_EDCA_BEBK_SETTING: u32 = 0x144C; // reg.h:826`

```
// reg.h:826
```

## L348 · `pub const REG_LIFETIME_EN: u32 = 0x0426; // reg.h:395`

```
// reg.h:395
```

## L349 · `pub const BIT_BA_PARSER_EN: u8 = 1 << 5; // reg.h:396`

```
// reg.h:396
```

## L350 · `pub const REG_RRSR: u32 = 0x0440; // reg.h:402`

```
// reg.h:402
```

## L351 · `pub const BITS_RRSR_RSC: u32 = 0x60_0000; // reg.h:403  GENMASK(22, 21)`

```
// reg.h:403  GENMASK(22, 21)
```

## L352 · `pub const REG_EDCA_VO_PARAM: u32 = 0x0500; // reg.h:447`

```
// reg.h:447
```

## L353 · `pub const REG_EDCA_VI_PARAM: u32 = 0x0504; // reg.h:448`

```
// reg.h:448
```

## L354 · `pub const REG_EDCA_BE_PARAM: u32 = 0x0508; // reg.h:449`

```
// reg.h:449
```

## L355 · `pub const REG_EDCA_BK_PARAM: u32 = 0x050C; // reg.h:450`

```
// reg.h:450
```

## L356 · `pub const REG_PIFS: u32 = 0x0512; // reg.h:456`

```
// reg.h:456
```

## L357 · `pub const REG_TX_PTCL_CTRL: u32 = 0x0520; // reg.h:463`

```
// reg.h:463
```

## L358 · `pub const BIT_SIFS_BK_EN: u32 = 1 << 12; // reg.h:465`

```
// reg.h:465
```

## L359 · `pub const REG_RD_CTRL: u32 = 0x0524; // reg.h:469`

```
// reg.h:469
```

## L360 · `pub const BIT_DIS_TXOP_CFE: u32 = 1 << 10; // reg.h:471`

```
// reg.h:471
```

## L361 · `pub const BIT_DIS_LSIG_CFE: u32 = 1 << 9; // reg.h:472`

```
// reg.h:472
```

## L362 · `pub const BIT_DIS_STBC_CFE: u32 = 1 << 8; // reg.h:473`

```
// reg.h:473
```

## L363 · `pub const REG_AFE_CTRL1: u32 = 0x0024; // reg.h:46`

```
// reg.h:46
```

## L364 · `pub const BIT_MAC_CLK_SEL: u32 = (1 << 20) | (1 << 21); // reg.h:47`

```
// reg.h:47
```

## L365 · `pub const REG_USTIME_TSF: u32 = 0x055C; // reg.h:486`

```
// reg.h:486
```

## L366 · `pub const REG_USTIME_EDCA: u32 = 0x0638; // reg.h:539`

```
// reg.h:539
```

## L367 · `pub const REG_MISC_CTRL: u32 = 0x0577; // reg.h:489`

```
// reg.h:489
```

## L368 · `pub const BIT_EN_FREE_CNT: u8 = 1 << 3; // reg.h:490`

```
// reg.h:490
```

## L369 · `pub const BIT_DIS_SECOND_CCA: u8 = (1 << 0) | (1 << 1); // reg.h:491`

```
// reg.h:491
```

## L370 · `pub const REG_TIMER0_SRC_SEL: u32 = 0x05B4; // reg.h:495`

```
// reg.h:495
```

## L371 · `pub const BIT_TSFT_SEL_TIMER0: u8 = (1 << 4) | (1 << 5) | (1 << 6); // reg.h:496`

```
// reg.h:496
```

## L372 · `pub const REG_TXPAUSE: u32 = 0x0522; // reg.h:466`

```
// reg.h:466
```

## L373 · `pub const REG_SLOT: u32 = 0x051B; // reg.h:462`

```
// reg.h:462
```

## L374 · `pub const REG_RD_NAV_NXT: u32 = 0x0544; // reg.h:476`

```
// reg.h:476
```

## L375 · `pub const REG_RXTSF_OFFSET_CCK: u32 = 0x055E; // reg.h:488`

```
// reg.h:488
```

## L376 · `pub const REG_TBTT_PROHIBIT: u32 = 0x0540; // reg.h:474`

```
// reg.h:474
```

## L377 · `pub const REG_DRVERLYINT: u32 = 0x0558; // reg.h:483`

```
// reg.h:483
```

## L378 · `pub const REG_BCN_CTRL_CLINT0: u32 = 0x0551; // reg.h:482`

```
// reg.h:482
```

## L379 · `pub const REG_BCNDMATIM: u32 = 0x0559; // reg.h:484`

```
// reg.h:484
```

## L380 · `pub const REG_BCN_MAX_ERR: u32 = 0x055D; // reg.h:487`

```
// reg.h:487
```

## L381 · `pub const REG_MAR: u32 = 0x0620; // reg.h:538`

```
// reg.h:538
```

## L382 · `pub const REG_BBPSF_CTRL: u32 = 0x06DC; // reg.h:577`

```
// reg.h:577
```

## L383 · `pub const REG_ACKTO: u32 = 0x0640; // reg.h:544`

```
// reg.h:544
```

## L384 · `pub const REG_ACKTO_CCK: u32 = 0x0639; // reg.h:540`

```
// reg.h:540
```

## L385 · `pub const REG_EIFS: u32 = 0x0642; // reg.h:545`

```
// reg.h:545
```

## L386 · `pub const REG_NAV_CTRL: u32 = 0x0650; // reg.h:546`

```
// reg.h:546
```

## L387 · `pub const REG_WMAC_TRXPTCL_CTL_H: u32 = 0x066C; // reg.h:551`

```
// reg.h:551
```

## L388 · `pub const REG_RXFLTMAP0: u32 = 0x06A0; // reg.h:566`

```
// reg.h:566
```

## L389 · `pub const REG_RXFLTMAP2: u32 = 0x06A4; // reg.h:568`

```
// reg.h:568
```

## L390 · `pub const REG_RX_PKT_LIMIT: u32 = 0x060C; // reg.h:535`

```
// reg.h:535
```

## L391 · `pub const REG_TCR: u32 = 0x0604; // reg.h:498`

```
// reg.h:498
```

## L392 · `pub const REG_GENERAL_OPTION: u32 = 0x1664; // reg.h:894`

```
// reg.h:894
```

## L393 · `pub const BIT_DUMMY_FCS_READY_MASK_EN: u32 = 1 << 9; // reg.h:895`

```
// reg.h:895
```

## L394 · `pub const REG_WMAC_OPTION_FUNCTION_1: u32 = 0x07D4; // reg.h:596`

```
// reg.h:596
```

## L395 · `pub const REG_RXPSF_CTRL: u32 = 0x1610; // reg.h:828`

```
// reg.h:828
```

## L396 · `pub const REG_RXPSF_TYPE_CTRL: u32 = 0x1614; // reg.h:893`

```
// reg.h:893
```

## L397 · `pub const REG_INT_MIG: u32 = 0x0304; // reg.h:380`

```
// reg.h:380
```

## L398-399 · `pub const REG_SND_PTCL_CTRL: u32 = 0x0718; // bf.h:15`

```
/// `REG_SND_PTCL_CTRL` und sein Bit stehen in **bf.h**, nicht in reg.h —
/// Beamforming hat dort seinen eigenen Registerblock.
```

## L400 · `pub const REG_SND_PTCL_CTRL: u32 = 0x0718; // bf.h:15`

```
// bf.h:15
```

## L401 · `pub const BIT_DIS_CHK_VHTSIGB_CRC: u8 = 1 << 6; // bf.h:16`

```
// bf.h:16
```

## L403 · `pub const BIT_SHIFT_RXGCK_VHT_FIFOTHR: u32 = 26; // reg.h:831`

```
// REG_RXPSF_CTRL-Felder (reg.h:831-889)
```

## L404 · `pub const BIT_SHIFT_RXGCK_VHT_FIFOTHR: u32 = 26; // reg.h:831`

```
// reg.h:831
```

## L405 · `pub const BIT_SHIFT_RXGCK_HT_FIFOTHR: u32 = 24; // reg.h:838`

```
// reg.h:838
```

## L406 · `pub const BIT_SHIFT_RXGCK_OFDM_FIFOTHR: u32 = 22; // reg.h:845`

```
// reg.h:845
```

## L407 · `pub const BIT_SHIFT_RXGCK_CCK_FIFOTHR: u32 = 20; // reg.h:852`

```
// reg.h:852
```

## L408 · `pub const BIT_SHIFT_RXPSF_PKTLENTHR: u32 = 13; // reg.h:861`

```
// reg.h:861
```

## L409 · `pub const BIT_MASK_RXPSF_PKTLENTHR: u16 = 0x7; // reg.h:862`

```
// reg.h:862
```

## L410 · `pub const BIT_RXPSF_CTRLEN: u16 = 1 << 12; // reg.h:871`

```
// reg.h:871
```

## L411 · `pub const BIT_RXPSF_VHTCHKEN: u16 = 1 << 11; // reg.h:872`

```
// reg.h:872
```

## L412 · `pub const BIT_RXPSF_HTCHKEN: u16 = 1 << 10; // reg.h:873`

```
// reg.h:873
```

## L413 · `pub const BIT_RXPSF_OFDMCHKEN: u16 = 1 << 9; // reg.h:874`

```
// reg.h:874
```

## L414 · `pub const BIT_RXPSF_CCKCHKEN: u16 = 1 << 8; // reg.h:875`

```
// reg.h:875
```

## L415 · `pub const BIT_RXPSF_OFDMRST: u16 = 1 << 7; // reg.h:876`

```
// reg.h:876
```

## L416 · `pub const BIT_RXPSF_CCKRST: u16 = 1 << 6; // reg.h:877`

```
// reg.h:877
```

## L417 · `pub const BIT_RXPSF_MHCHKEN: u16 = 1 << 5; // reg.h:878`

```
// reg.h:878
```

## L418 · `pub const BIT_RXPSF_CONT_ERRCHKEN: u16 = 1 << 4; // reg.h:879`

```
// reg.h:879
```

## L419 · `pub const BIT_SHIFT_RXPSF_ERRTHR: u32 = 0; // reg.h:882`

```
// reg.h:882
```

## L420 · `pub const BIT_MASK_RXPSF_ERRTHR: u16 = 0x7; // reg.h:883`

```
// reg.h:883
```

## L422 · `pub const WLAN_TXQ_RPT_EN: u8 = 0x1F; // rtw8822c.c:1915`

```
// ── rtw8822c.c `rtw8822c_mac_init` — Werte ───────────────────────
```

## L423 · `pub const WLAN_TXQ_RPT_EN: u8 = 0x1F; // rtw8822c.c:1915`

```
// rtw8822c.c:1915
```

## L424 · `pub const WLAN_SLOT_TIME: u8 = 0x09; // rtw8822c.c:1916`

```
// rtw8822c.c:1916
```

## L425 · `pub const WLAN_PIFS_TIME: u8 = 0x1C; // rtw8822c.c:1917`

```
// rtw8822c.c:1917
```

## L426 · `pub const WLAN_NAV_MAX: u8 = 0xC8; // rtw8822c.c:1922`

```
// rtw8822c.c:1922
```

## L427 · `pub const WLAN_DRV_EARLY_INT: u8 = 0x04; // rtw8822c.c:1929`

```
// rtw8822c.c:1929
```

## L428 · `pub const WLAN_BCN_CTRL_CLT0: u8 = 0x10; // rtw8822c.c:1930`

```
// rtw8822c.c:1930
```

## L429 · `pub const WLAN_BCN_DMA_TIME: u8 = 0x02; // rtw8822c.c:1931`

```
// rtw8822c.c:1931
```

## L430 · `pub const WLAN_BCN_MAX_ERR: u8 = 0xFF; // rtw8822c.c:1932`

```
// rtw8822c.c:1932
```

## L431 · `pub const WLAN_SIFS_CCK_CTX: u16 = 0x0A; // rtw8822c.c:1935`

```
// rtw8822c.c:1935
```

## L432 · `pub const WLAN_SIFS_CCK_IRX: u16 = 0x0A; // rtw8822c.c:1936`

```
// rtw8822c.c:1936
```

## L433 · `pub const WLAN_SIFS_OFDM_CTX: u16 = 0x0E; // rtw8822c.c:1937`

```
// rtw8822c.c:1937
```

## L434 · `pub const WLAN_SIFS_OFDM_IRX: u16 = 0x0E; // rtw8822c.c:1938`

```
// rtw8822c.c:1938
```

## L435 · `pub const WLAN_EIFS_DUR_TUNE: u16 = 0x40; // rtw8822c.c:1939`

```
// rtw8822c.c:1939
```

## L436 · `pub const WLAN_EDCA_VO_PARAM: u32 = 0x002F_A226; // rtw8822c.c:1940`

```
// rtw8822c.c:1940
```

## L437 · `pub const WLAN_EDCA_VI_PARAM: u32 = 0x005E_A328; // rtw8822c.c:1941`

```
// rtw8822c.c:1941
```

## L438 · `pub const WLAN_EDCA_BE_PARAM: u32 = 0x005E_A42B; // rtw8822c.c:1942`

```
// rtw8822c.c:1942
```

## L439 · `pub const WLAN_EDCA_BK_PARAM: u32 = 0x0000_A44F; // rtw8822c.c:1943`

```
// rtw8822c.c:1943
```

## L440 · `pub const WLAN_RX_FILTER0: u32 = 0xFFFF_FFFF; // rtw8822c.c:1945`

```
// rtw8822c.c:1945
```

## L441 · `pub const WLAN_RX_FILTER2: u16 = 0xFFFF; // rtw8822c.c:1946`

```
// rtw8822c.c:1946
```

## L442 · `pub const WLAN_RCR_CFG: u32 = 0xE400_220E; // rtw8822c.c:1947`

```
// rtw8822c.c:1947
```

## L443 · `pub const WLAN_RXPKT_MAX_SZ_512: u8 = 24; // rtw8822c.c:1949  (12288 >> 9)`

```
// rtw8822c.c:1949  (12288 >> 9)
```

## L444 · `pub const WLAN_AMPDU_MAX_TIME: u8 = 0x70; // rtw8822c.c:1951`

```
// rtw8822c.c:1951
```

## L445 · `pub const WLAN_RTS_LEN_TH: u32 = 0xFF; // rtw8822c.c:1952`

```
// rtw8822c.c:1952
```

## L446 · `pub const WLAN_RTS_TX_TIME_TH: u32 = 0x08; // rtw8822c.c:1953`

```
// rtw8822c.c:1953
```

## L447 · `pub const WLAN_MAX_AGG_PKT_LIMIT: u32 = 0x3f; // rtw8822c.c:1954`

```
// rtw8822c.c:1954
```

## L448 · `pub const WLAN_RTS_MAX_AGG_PKT_LIMIT: u32 = 0x3f; // rtw8822c.c:1955`

```
// rtw8822c.c:1955
```

## L449 · `pub const WLAN_PRE_TXCNT_TIME_TH: u16 = 0x1E0; // rtw8822c.c:1956`

```
// rtw8822c.c:1956
```

## L450 · `pub const FAST_EDCA_VO_TH: u8 = 0x06; // rtw8822c.c:1957`

```
// rtw8822c.c:1957
```

## L451 · `pub const FAST_EDCA_VI_TH: u8 = 0x06; // rtw8822c.c:1958`

```
// rtw8822c.c:1958
```

## L452 · `pub const FAST_EDCA_BE_TH: u8 = 0x06; // rtw8822c.c:1959`

```
// rtw8822c.c:1959
```

## L453 · `pub const FAST_EDCA_BK_TH: u8 = 0x06; // rtw8822c.c:1960`

```
// rtw8822c.c:1960
```

## L454 · `pub const WLAN_BAR_RETRY_LIMIT: u16 = 0x01; // rtw8822c.c:1961`

```
// rtw8822c.c:1961
```

## L455 · `pub const WLAN_BAR_ACK_TYPE: u8 = 0x05; // rtw8822c.c:1962`

```
// rtw8822c.c:1962
```

## L456 · `pub const WLAN_RA_TRY_RATE_AGG_LIMIT: u16 = 0x08; // rtw8822c.c:1963`

```
// rtw8822c.c:1963
```

## L457 · `pub const WLAN_RESP_TXRATE: u8 = 0x84; // rtw8822c.c:1964`

```
// rtw8822c.c:1964
```

## L458 · `pub const WLAN_ACK_TO: u8 = 0x21; // rtw8822c.c:1965`

```
// rtw8822c.c:1965
```

## L459 · `pub const WLAN_ACK_TO_CCK: u8 = 0x6A; // rtw8822c.c:1966`

```
// rtw8822c.c:1966
```

## L460 · `pub const WLAN_DATA_RATE_FB_CNT_1_4: u32 = 0x0100_0000; // rtw8822c.c:1967`

```
// rtw8822c.c:1967
```

## L461 · `pub const WLAN_DATA_RATE_FB_CNT_5_8: u32 = 0x0807_0504; // rtw8822c.c:1968`

```
// rtw8822c.c:1968
```

## L462 · `pub const WLAN_RTS_RATE_FB_CNT_5_8: u32 = 0x0807_0504; // rtw8822c.c:1969`

```
// rtw8822c.c:1969
```

## L463 · `pub const WLAN_DATA_RATE_FB_RATE0: u32 = 0xFE01_F010; // rtw8822c.c:1970`

```
// rtw8822c.c:1970
```

## L464 · `pub const WLAN_DATA_RATE_FB_RATE0_H: u32 = 0x4000_0000; // rtw8822c.c:1971`

```
// rtw8822c.c:1971
```

## L465 · `pub const WLAN_RTS_RATE_FB_RATE1: u32 = 0x003F_F010; // rtw8822c.c:1972`

```
// rtw8822c.c:1972
```

## L466 · `pub const WLAN_RTS_RATE_FB_RATE1_H: u32 = 0x4000_0000; // rtw8822c.c:1973`

```
// rtw8822c.c:1973
```

## L467 · `pub const WLAN_RTS_RATE_FB_RATE4: u32 = 0x0600_F010; // rtw8822c.c:1974`

```
// rtw8822c.c:1974
```

## L468 · `pub const WLAN_RTS_RATE_FB_RATE4_H: u32 = 0x4000_03E0; // rtw8822c.c:1975`

```
// rtw8822c.c:1975
```

## L469 · `pub const WLAN_RTS_RATE_FB_RATE5: u32 = 0x0600_F015; // rtw8822c.c:1976`

```
// rtw8822c.c:1976
```

## L470 · `pub const WLAN_RTS_RATE_FB_RATE5_H: u32 = 0x0000_00E0; // rtw8822c.c:1977`

```
// rtw8822c.c:1977
```

## L471 · `pub const WLAN_MULTI_ADDR: u32 = 0xFFFF_FFFF; // rtw8822c.c:1978`

```
// rtw8822c.c:1978
```

## L472 · `pub const WLAN_TX_FUNC_CFG1: u8 = 0x30; // rtw8822c.c:1980`

```
// rtw8822c.c:1980
```

## L473 · `pub const WLAN_TX_FUNC_CFG2: u8 = 0x30; // rtw8822c.c:1981`

```
// rtw8822c.c:1981
```

## L474 · `pub const WLAN_MAC_OPT_NORM_FUNC1: u8 = 0x98; // rtw8822c.c:1982`

```
// rtw8822c.c:1982
```

## L475 · `pub const WLAN_MAC_OPT_FUNC2: u32 = 0xb081_0041; // rtw8822c.c:1984`

```
// rtw8822c.c:1984
```

## L476 · `pub const WLAN_MAC_INT_MIG_CFG: u32 = 0x3333_0000; // rtw8822c.c:1985`

```
// rtw8822c.c:1985
```

## L477-478 · `pub const WLAN_SIFS_CFG: u32 = 0x100A_0E0A; // rtw8822c.c:1987`

```
/// rtw8822c.c:1987 — zusammengesetzt aus CCK_CONT_TX 0x0A, OFDM_CONT_TX 0x0E,
/// CCK_TRX 0x0A und OFDM_TRX 0x10 an ihren Schiebestellen: 0x100A0E0A.
```

## L479 · `pub const WLAN_SIFS_CFG: u32 = 0x100A_0E0A; // rtw8822c.c:1987`

```
// rtw8822c.c:1987
```

## L480 · `pub const WLAN_SIFS_DUR_TUNE: u16 = 0x100A; // rtw8822c.c:1992`

```
/// rtw8822c.c:1992 — CCK_DUR_TUNE 0x0A | OFDM_DUR_TUNE 0x10 << 8.
```

## L481 · `pub const WLAN_SIFS_DUR_TUNE: u16 = 0x100A; // rtw8822c.c:1992`

```
// rtw8822c.c:1992
```

## L482 · `pub const WLAN_TBTT_TIME: u32 = 0x0000_6404; // rtw8822c.c:1995`

```
/// rtw8822c.c:1995 — TBTT_PROHIBIT 0x04 | TBTT_HOLD_TIME 0x64 << 8.
```

## L483 · `pub const WLAN_TBTT_TIME: u32 = 0x0000_6404; // rtw8822c.c:1995`

```
// rtw8822c.c:1995
```

## L484 · `pub const WLAN_NAV_CFG: u32 = 0x001B_0005; // rtw8822c.c:1998`

```
// rtw8822c.c:1998
```

## L485 · `pub const WLAN_RX_TSF_CFG: u16 = 0x3030; // rtw8822c.c:1999`

```
// rtw8822c.c:1999
```

## L486 · `pub const MAC_CLK_SPEED: u8 = 80; // rtw8822c.c:2001`

```
// rtw8822c.c:2001
```

## L488-491 · `#[inline]`

```
// ── Feldmakros aus reg.h, als Funktionen ─────────────────────────
// In C sind das `#define NAME(x) (((x) & MASK) << SHIFT)`. Als Funktion
// bleibt die Maske stehen, wo sie in Linux steht — ein direkt geschriebener
// Zahlenwert waere die Rechnung von HEUTE und nicht die Regel.
```

## L493 · `#[inline]`

```
/// reg.h:233-258 — dieselbe Form fuer alle sechs Sendequeues.
```

## L499 · `#[inline]`

```
/// reg.h:833 `BIT_RXGCK_VHT_FIFOTHR(x)`
```

## L504 · `#[inline]`

```
/// reg.h:840 `BIT_RXGCK_HT_FIFOTHR(x)`
```

## L509 · `#[inline]`

```
/// reg.h:847 `BIT_RXGCK_OFDM_FIFOTHR(x)`
```

## L514 · `#[inline]`

```
/// reg.h:854 `BIT_RXGCK_CCK_FIFOTHR(x)`
```

## L520 · `#[inline]`

```
/// reg.h:869 `BIT_SET_RXPSF_PKTLENTHR(x, v)` — Feld loeschen, dann setzen.
```

## L527 · `#[inline]`

```
/// reg.h:889 `BIT_SET_RXPSF_ERRTHR(x, v)`
```

## L534-536 · `pub const REG_3WIRE: u32 = 0x180C; // rtw8822c.h:214`

```
// ════════════════════════════════════════════════════════════════
// Stufe 3c: rtw8822c_phy_set_param
// ════════════════════════════════════════════════════════════════
```

## L538 · `pub const REG_3WIRE: u32 = 0x180C; // rtw8822c.h:214`

```
// ── rtw8822c_header_file_init (rtw8822c.h:214-219, 312-314) ──────
```

## L539 · `pub const REG_3WIRE: u32 = 0x180C; // rtw8822c.h:214`

```
// rtw8822c.h:214
```

## L540 · `pub const REG_3WIRE2: u32 = 0x410C; // rtw8822c.h:353`

```
// rtw8822c.h:353
```

## L541 · `pub const BIT_3WIRE_TX_EN: u32 = 0x1; // rtw8822c.h:216  GENMASK(0, 0)`

```
// rtw8822c.h:216  GENMASK(0, 0)
```

## L542 · `pub const BIT_3WIRE_RX_EN: u32 = 0x2; // rtw8822c.h:217  GENMASK(1, 1)`

```
// rtw8822c.h:217  GENMASK(1, 1)
```

## L543 · `pub const BIT_3WIRE_PI_ON: u32 = 1 << 28; // rtw8822c.h:219`

```
// rtw8822c.h:219
```

## L544 · `pub const REG_ENCCK: u32 = 0x1C3C; // rtw8822c.h:312`

```
// rtw8822c.h:312
```

## L545 · `pub const BIT_CCK_BLK_EN: u32 = 1 << 1; // rtw8822c.h:313`

```
// rtw8822c.h:313
```

## L546 · `pub const BIT_CCK_OFDM_BLK_EN: u32 = 0x3; // rtw8822c.h:314  GENMASK(1, 0)`

```
// rtw8822c.h:314  GENMASK(1, 0)
```

## L548 · `pub const BB_PATH_A: u8 = 1 << 0; // main.h:142`

```
// ── Sende-/Empfangspfade (rtw8822c.h, main.h:141-147) ────────────
```

## L549 · `pub const BB_PATH_A: u8 = 1 << 0; // main.h:142`

```
// main.h:142
```

## L550 · `pub const BB_PATH_B: u8 = 1 << 1; // main.h:143`

```
// main.h:143
```

## L551 · `pub const BB_PATH_AB: u8 = BB_PATH_A | BB_PATH_B; // main.h:147`

```
// main.h:147
```

## L552 · `pub const REG_ORITXCODE: u32 = 0x1800; // rtw8822c.h:212`

```
// rtw8822c.h:212
```

## L553 · `pub const REG_ORITXCODE2: u32 = 0x4100; // rtw8822c.h:352`

```
// rtw8822c.h:352
```

## L554 · `pub const MASK20BITS: u32 = 0xfffff; // phy.h:184`

```
// phy.h:184
```

## L555 · `pub const REG_CCANRX: u32 = 0x1A2C; // rtw8822c.h:242`

```
// rtw8822c.h:242
```

## L556 · `pub const REG_RXCCKSEL: u32 = 0x1A04; // rtw8822c.h:236`

```
// rtw8822c.h:236
```

## L557 · `pub const REG_RXFNCTL: u32 = 0x1D30; // rtw8822c.h:326`

```
// rtw8822c.h:326
```

## L558 · `pub const REG_AGCSWSH: u32 = 0x0C44; // rtw8822c.h:207`

```
// rtw8822c.h:207
```

## L559 · `pub const REG_ANTWTPD: u32 = 0x0C54; // rtw8822c.h:208`

```
// rtw8822c.h:208
```

## L560 · `pub const REG_MRCM: u32 = 0x0C38; // rtw8822c.h:206`

```
// rtw8822c.h:206
```

## L561 · `pub const REG_ANTMAP0: u32 = 0x0820; // rtw8822c.h:190`

```
// rtw8822c.h:190
```

## L562 · `pub const REG_TXLGMAP: u32 = 0x1E2C; // rtw8822c.h:335`

```
// rtw8822c.h:335
```

## L563 · `pub const REG_RXIGI: u32 = 0x1D70; // rtw8822c.h:329`

```
// rtw8822c.h:329
```

## L565 · `pub const REG_DIS_DPD: u32 = 0x0A70; // reg.h:657`

```
// ── DPD, Quarz (reg.h) ───────────────────────────────────────────
```

## L566 · `pub const REG_DIS_DPD: u32 = 0x0A70; // reg.h:657`

```
// reg.h:657
```

## L567 · `pub const DIS_DPD_MASK: u32 = 0x3ff; // reg.h:658  GENMASK(9, 0)`

```
// reg.h:658  GENMASK(9, 0)
```

## L568 · `pub const DIS_DPD_RATEALL: u32 = 0x3ff; // reg.h:669`

```
// reg.h:669
```

## L569 · `pub const REG_ANAPAR_XTAL_0: u32 = 0x1040; // reg.h:791`

```
// reg.h:791
```

## L570 · `pub const XCAP_MASK: u32 = 0x7f; // rtw8822c.h:183  GENMASK(6, 0)`

```
// rtw8822c.h:183  GENMASK(6, 0)
```

## L572 · `pub const DACK_PATH_8822C: usize = 2; // rtw8822c.h:139`

```
// ── DACK (rtw8822c.h:139-142, main.h:1625-1626, rtw8822c.h:229-232) ──
```

## L573 · `pub const DACK_PATH_8822C: usize = 2; // rtw8822c.h:139`

```
// rtw8822c.h:139
```

## L574 · `pub const DACK_REG_8822C: usize = 16; // rtw8822c.h:140`

```
// rtw8822c.h:140
```

## L575 · `pub const DACK_RF_8822C: usize = 1; // rtw8822c.h:141`

```
// rtw8822c.h:141
```

## L576 · `pub const DACK_SN_8822C: usize = 100; // rtw8822c.h:142`

```
// rtw8822c.h:142
```

## L577 · `pub const DACK_MSBK_BACKUP_NUM: usize = 15; // main.h:1625`

```
// main.h:1625
```

## L578 · `pub const DACK_DCK_BACKUP_NUM: usize = 2; // main.h:1626`

```
// main.h:1626
```

## L579 · `pub const REG_DCKA_I_0: u32 = 0x18BC; // rtw8822c.h:229`

```
// rtw8822c.h:229
```

## L580 · `pub const REG_DCKA_I_1: u32 = 0x18C0; // rtw8822c.h:230`

```
// rtw8822c.h:230
```

## L581 · `pub const REG_DCKA_Q_0: u32 = 0x18D8; // rtw8822c.h:231`

```
// rtw8822c.h:231
```

## L582 · `pub const REG_DCKA_Q_1: u32 = 0x18DC; // rtw8822c.h:232`

```
// rtw8822c.h:232
```

## L583 · `pub const REG_DCKB_I_0: u32 = 0x41BC; // rtw8822c.h:359`

```
// rtw8822c.h:359
```

## L584 · `pub const REG_DCKB_I_1: u32 = 0x41C0; // rtw8822c.h:360`

```
// rtw8822c.h:360
```

## L585 · `pub const REG_DCKB_Q_0: u32 = 0x41D8; // rtw8822c.h:361`

```
// rtw8822c.h:361
```

## L586 · `pub const REG_DCKB_Q_1: u32 = 0x41DC; // rtw8822c.h:362`

```
// rtw8822c.h:362
```

## L588 · `pub const RF_PA: u32 = 0x60; // rtw8822c.h:384`

```
// ── RF-Register und ihre Felder (rtw8822c.h:384-406) ─────────────
```

## L589 · `pub const RF_PA: u32 = 0x60; // rtw8822c.h:384`

```
// rtw8822c.h:384
```

## L590 · `pub const RF_PABIAS_2G_MASK: u32 = 0xf000; // rtw8822c.h:385  GENMASK(15, 12)`

```
// rtw8822c.h:385  GENMASK(15, 12)
```

## L591 · `pub const RF_PABIAS_5G_MASK: u32 = 0xf0000; // rtw8822c.h:386  GENMASK(19, 16)`

```
// rtw8822c.h:386  GENMASK(19, 16)
```

## L592 · `pub const RF_THEMAL_MASK: u32 = 0xf0000; // rtw8822c.h:406  GENMASK(19, 16)`

```
// rtw8822c.h:406  GENMASK(19, 16)
```

## L594 · `pub const PPG_THERMAL_B: u16 = 0x1B0; // rtw8822c.h:405`

```
// ── Werkskalibrierung in der PHYSISCHEN efuse (rtw8822c.h:405-428) ──
```

## L595 · `pub const PPG_THERMAL_B: u16 = 0x1B0; // rtw8822c.h:405`

```
// rtw8822c.h:405
```

## L596 · `pub const PPG_2GH_TXAB: u16 = 0x1D2; // rtw8822c.h:407`

```
// rtw8822c.h:407
```

## L597 · `pub const PPG_2G_A_MASK: u8 = 0x0f; // rtw8822c.h:408  GENMASK(3, 0)`

```
// rtw8822c.h:408  GENMASK(3, 0)
```

## L598 · `pub const PPG_2G_B_MASK: u8 = 0xf0; // rtw8822c.h:409  GENMASK(7, 4)`

```
// rtw8822c.h:409  GENMASK(7, 4)
```

## L599 · `pub const PPG_2GL_TXAB: u16 = 0x1D4; // rtw8822c.h:410`

```
// rtw8822c.h:410
```

## L600 · `pub const PPG_PABIAS_2GB: u16 = 0x1D5; // rtw8822c.h:411`

```
// rtw8822c.h:411
```

## L601 · `pub const PPG_PABIAS_2GA: u16 = 0x1D6; // rtw8822c.h:412`

```
// rtw8822c.h:412
```

## L602 · `pub const PPG_PABIAS_MASK: u8 = 0x0f; // rtw8822c.h:413  GENMASK(3, 0)`

```
// rtw8822c.h:413  GENMASK(3, 0)
```

## L603 · `pub const PPG_PABIAS_5GB: u16 = 0x1D7; // rtw8822c.h:414`

```
// rtw8822c.h:414
```

## L604 · `pub const PPG_PABIAS_5GA: u16 = 0x1D8; // rtw8822c.h:415`

```
// rtw8822c.h:415
```

## L605 · `pub const PPG_5G_MASK: u8 = 0x1f; // rtw8822c.h:416  GENMASK(4, 0)`

```
// rtw8822c.h:416  GENMASK(4, 0)
```

## L606 · `pub const PPG_5GH1_TXB: u16 = 0x1DB; // rtw8822c.h:417`

```
// rtw8822c.h:417
```

## L607 · `pub const PPG_5GH1_TXA: u16 = 0x1DC; // rtw8822c.h:418`

```
// rtw8822c.h:418
```

## L608 · `pub const PPG_5GM2_TXB: u16 = 0x1DF; // rtw8822c.h:419`

```
// rtw8822c.h:419
```

## L609 · `pub const PPG_5GM2_TXA: u16 = 0x1E0; // rtw8822c.h:420`

```
// rtw8822c.h:420
```

## L610 · `pub const PPG_5GM1_TXB: u16 = 0x1E3; // rtw8822c.h:421`

```
// rtw8822c.h:421
```

## L611 · `pub const PPG_5GM1_TXA: u16 = 0x1E4; // rtw8822c.h:422`

```
// rtw8822c.h:422
```

## L612 · `pub const PPG_5GL2_TXB: u16 = 0x1E7; // rtw8822c.h:423`

```
// rtw8822c.h:423
```

## L613 · `pub const PPG_5GL2_TXA: u16 = 0x1E8; // rtw8822c.h:424`

```
// rtw8822c.h:424
```

## L614 · `pub const PPG_5GL1_TXB: u16 = 0x1EB; // rtw8822c.h:425`

```
// rtw8822c.h:425
```

## L615 · `pub const PPG_5GL1_TXA: u16 = 0x1EC; // rtw8822c.h:426`

```
// rtw8822c.h:426
```

## L616 · `pub const PPG_2GM_TXAB: u16 = 0x1EE; // rtw8822c.h:427`

```
// rtw8822c.h:427
```

## L617 · `pub const PPG_THERMAL_A: u16 = 0x1EF; // rtw8822c.h:428`

```
// rtw8822c.h:428
```

## L618 · `pub const EFUSE_READ_FAIL: u8 = 0xff;`

```
/// efuse.h:13 — und zugleich der Wert, den ein LEERES efuse-Byte hat.
```

## L621 · `pub const RTW8822C_EDCCA_MAX: u8 = 0x7f; // rtw8822c.h:180`

```
// ── Adaptivity / EDCCA ───────────────────────────────────────────
```

## L622 · `pub const RTW8822C_EDCCA_MAX: u8 = 0x7f; // rtw8822c.h:180`

```
// rtw8822c.h:180
```

## L623 · `pub const BIT_DIS_EDCCA: u32 = 1 << 15; // reg.h:464`

```
// reg.h:464
```

## L624 · `pub const BIT_EDCCA_MSK_CNTDOWN_EN: u32 = 1 << 11; // reg.h:470`

```
// reg.h:470
```

## L625 · `pub const REG_EDCCA_DECISION: u32 = 0x0844; // rtw8822c.h:193`

```
// rtw8822c.h:193
```

## L626 · `pub const BIT_EDCCA_OPTION: u32 = 0x6000_0000; // rtw8822c.h:194  GENMASK(30, 29)`

```
// rtw8822c.h:194  GENMASK(30, 29)
```

## L627-628 · `pub const EDCCA_TH_L2H: (u32, u32, u8) = (0x84c, 0x00ff_0000, 0x80);`

```
/// rtw8822c.c:5287-5294 `rtw8822c_edcca_th` — Adresse, Maske und der
/// Versatz, der auf den Schwellwert addiert wird.
```

## L632 · `pub const REG_TXBF_CTRL: u32 = 0x042C; // bf.h:8`

```
// ── Beamforming-Grundeinstellung (bf.h) ──────────────────────────
```

## L633 · `pub const REG_TXBF_CTRL: u32 = 0x042C; // bf.h:8`

```
// bf.h:8
```

## L634 · `pub const REG_NDPA_OPT_CTRL: u32 = 0x045F; // bf.h:10`

```
// bf.h:10
```

## L635 · `pub const REG_MU_TX_CTL: u32 = 0x14C0; // bf.h:19`

```
// bf.h:19
```

## L636 · `pub const REG_WMAC_MU_BF_OPTION: u32 = 0x167C; // bf.h:23`

```
// bf.h:23
```

## L637 · `pub const REG_WMAC_MU_BF_CTL: u32 = 0x1680; // bf.h:24`

```
// bf.h:24
```

## L638 · `pub const BIT_WMAC_TXMU_ACKPOLICY_EN: u8 = 1 << 6; // bf.h:27`

```
// bf.h:27
```

## L639 · `pub const BIT_USE_NDPA_PARAMETER: u32 = 1 << 30; // bf.h:28`

```
// bf.h:28
```

## L640 · `pub const BIT_MU_P1_WAIT_STATE_EN: u32 = 1 << 16; // bf.h:29`

```
// bf.h:29
```

## L641 · `pub const BIT_EN_MU_MIMO: u32 = 1 << 7; // bf.h:30`

```
// bf.h:30
```

## L642 · `pub const BIT_SHIFT_R_MU_RL: u32 = 12; // bf.h:33`

```
// bf.h:33
```

## L643 · `pub const BIT_SHIFT_WMAC_TXMU_ACKPOLICY: u32 = 4; // bf.h:34`

```
// bf.h:34
```

## L644 · `pub const BIT_MASK_R_MU_RL: u32 = 0xf000; // bf.h:37  GENMASK(15, 12)`

```
// bf.h:37  GENMASK(15, 12)
```

## L645 · `pub const BIT_MASK_R_MU_TABLE_VALID: u32 = 0x3f; // bf.h:38  GENMASK(5, 0)`

```
// bf.h:38  GENMASK(5, 0)
```

## L646 · `pub const BIT_MASK_CSI_RATE: u32 = 0x3f00_0000; // bf.h:40  GENMASK(29, 24)`

```
// bf.h:40  GENMASK(29, 24)
```

## L647 · `pub const DESC_RATE6M: u32 = 0x04; // main.h:255`

```
// main.h:255
```

## L649 · `pub const REG_CCK_FACNT: u32 = 0x1A5C; // rtw8822c.h:245`

```
// ── Falschalarm-Zaehler (rtw8822c.h:243-347) ─────────────────────
```

## L650 · `pub const REG_CCK_FACNT: u32 = 0x1A5C; // rtw8822c.h:245`

```
// rtw8822c.h:245
```

## L651 · `pub const REG_OFDM_FACNT1: u32 = 0x2D04; // rtw8822c.h:343`

```
// rtw8822c.h:343
```

## L652 · `pub const REG_OFDM_FACNT2: u32 = 0x2D08; // rtw8822c.h:344`

```
// rtw8822c.h:344
```

## L653 · `pub const REG_OFDM_FACNT3: u32 = 0x2D0C; // rtw8822c.h:345`

```
// rtw8822c.h:345
```

## L654 · `pub const REG_OFDM_FACNT4: u32 = 0x2D10; // rtw8822c.h:346`

```
// rtw8822c.h:346
```

## L655 · `pub const REG_OFDM_FACNT5: u32 = 0x2D20; // rtw8822c.h:347`

```
// rtw8822c.h:347
```

## L656 · `pub const BIT_CCK_FA_RST: u32 = 0xc000; // rtw8822c.h:243  GENMASK(15, 14)`

```
// rtw8822c.h:243  GENMASK(15, 14)
```

## L657 · `pub const BIT_OFDM_FA_RST: u32 = 0x3000; // rtw8822c.h:244  GENMASK(13, 12)`

```
// rtw8822c.h:244  GENMASK(13, 12)
```

## L658 · `pub const REG_RX_BREAK: u32 = 0x1D2C; // rtw8822c.h:324`

```
// rtw8822c.h:324
```

## L659 · `pub const BIT_COM_RX_GCK_EN: u32 = 1 << 31; // rtw8822c.h:325`

```
// rtw8822c.h:325
```

## L660 · `pub const REG_CNT_CTRL: u32 = 0x1EB4; // rtw8822c.h:339`

```
// rtw8822c.h:339
```

## L661 · `pub const BIT_ALL_CNT_RST: u32 = 1 << 25; // rtw8822c.h:340`

```
// rtw8822c.h:340
```

## L663-665 · `pub const REG_HMETFR: u32 = 0x01CC; // reg.h:291`

```
// ════════════════════════════════════════════════════════════════
// Stufe 4a: der Rest von rtw_power_on und rtw_core_start
// ════════════════════════════════════════════════════════════════
```

## L667-669 · `pub const REG_HMETFR: u32 = 0x01CC; // reg.h:291`

```
// ── H2C-MAILBOX (reg.h:291-307) ──────────────────────────────────
// Der eine von ZWEI H2C-Wegen: acht Byte je Kommando, vier Postfaecher
// im Umlauf. Die Koexistenz redet hierueber mit der Firmware.
```

## L670 · `pub const REG_HMETFR: u32 = 0x01CC; // reg.h:291`

```
// reg.h:291
```

## L671 · `pub const REG_HMEBOX0: u32 = 0x01D0; // reg.h:298`

```
// reg.h:298
```

## L672 · `pub const REG_HMEBOX1: u32 = 0x01D4; // reg.h:299`

```
// reg.h:299
```

## L673 · `pub const REG_HMEBOX2: u32 = 0x01D8; // reg.h:300`

```
// reg.h:300
```

## L674 · `pub const REG_HMEBOX3: u32 = 0x01DC; // reg.h:301`

```
// reg.h:301
```

## L675 · `pub const REG_HMEBOX0_EX: u32 = 0x01F0; // reg.h:304`

```
// reg.h:304
```

## L676 · `pub const REG_HMEBOX1_EX: u32 = 0x01F4; // reg.h:305`

```
// reg.h:305
```

## L677 · `pub const REG_HMEBOX2_EX: u32 = 0x01F8; // reg.h:306`

```
// reg.h:306
```

## L678 · `pub const REG_HMEBOX3_EX: u32 = 0x01FC; // reg.h:307`

```
// reg.h:307
```

## L680-683 · `pub const H2C_PKT_SIZE: usize = 32; // fw.h:8`

```
// ── H2C-PAKET (fw.h) ─────────────────────────────────────────────
// Der ANDERE Weg: 32 Byte durch die H2C-Queue, also durch den Ring,
// dessen Adresse `init_h2c` in Stufe 3a gesetzt hat. General- und
// PHYDM-Info gehen hier durch.
```

## L684 · `pub const H2C_PKT_SIZE: usize = 32; // fw.h:8`

```
// fw.h:8
```

## L685 · `pub const H2C_PKT_HDR_SIZE: u16 = 8; // fw.h:9`

```
// fw.h:9
```

## L686 · `pub const H2C_PKT_CMD_ID: u32 = 0xFF; // fw.h:386`

```
// fw.h:386
```

## L687 · `pub const H2C_PKT_CATEGORY: u32 = 0x01; // fw.h:387`

```
// fw.h:387
```

## L688 · `pub const H2C_PKT_GENERAL_INFO: u32 = 0x0D; // fw.h:389`

```
// fw.h:389
```

## L689 · `pub const H2C_PKT_PHYDM_INFO: u32 = 0x11; // fw.h:390`

```
// fw.h:390
```

## L690 · `pub const FW_RF_2T2R: u8 = 0x2; // fw.h:134`

```
// fw.h:134
```

## L691 · `pub const FW_RF_1T1R: u8 = 0x4; // fw.h:136`

```
// fw.h:136
```

## L693 · `pub const RTW_SEC_CONFIG: u32 = 0x0680; // sec.h:11`

```
// ── Sicherheits-Engine (sec.h) ───────────────────────────────────
```

## L694 · `pub const RTW_SEC_CONFIG: u32 = 0x0680; // sec.h:11`

```
// sec.h:11
```

## L695 · `pub const RTW_SEC_TX_UNI_USE_DK: u16 = 1 << 0; // sec.h:19`

```
// sec.h:19
```

## L696 · `pub const RTW_SEC_RX_UNI_USE_DK: u16 = 1 << 1; // sec.h:20`

```
// sec.h:20
```

## L697 · `pub const RTW_SEC_TX_DEC_EN: u16 = 1 << 2; // sec.h:21`

```
// sec.h:21
```

## L698 · `pub const RTW_SEC_RX_DEC_EN: u16 = 1 << 3; // sec.h:22`

```
// sec.h:22
```

## L699 · `pub const RTW_SEC_TX_BC_USE_DK: u16 = 1 << 6; // sec.h:23`

```
// sec.h:23
```

## L700 · `pub const RTW_SEC_RX_BC_USE_DK: u16 = 1 << 7; // sec.h:24`

```
// sec.h:24
```

## L701 · `pub const RTW_SEC_ENGINE_EN: u16 = 1 << 9; // sec.h:26`

```
// sec.h:26
```

## L703-707 · `pub const BIT_APP_FCS: u32 = 1 << 31; // reg.h:503`

```
// ── Empfangsfilter (reg.h:502-533) ───────────────────────────────
// `hal->rcr` wird in main.c:2183 gesetzt und in `rtw_core_start`
// (main.c:1526) NOCH EINMAL ins Register geschrieben — nach allem, was
// `rtw8822c_mac_init` und `rtw_drv_info_cfg` dort hinterlassen haben.
// Der Kommentar dort lautet „rcr reset after powered on".
```

## L708 · `pub const BIT_APP_FCS: u32 = 1 << 31; // reg.h:503`

```
// reg.h:503
```

## L709 · `pub const BIT_APP_MIC: u32 = 1 << 30; // reg.h:504`

```
// reg.h:504
```

## L710 · `pub const BIT_APP_ICV: u32 = 1 << 29; // reg.h:505`

```
// reg.h:505
```

## L711 · `pub const BIT_VHT_DACK: u32 = 1 << 26; // reg.h:508`

```
// reg.h:508
```

## L712 · `pub const BIT_PKTCTL_DLEN: u32 = 1 << 20; // reg.h:514`

```
// reg.h:514
```

## L713 · `pub const BIT_HTC_LOC_CTRL: u32 = 1 << 14; // reg.h:520`

```
// reg.h:520
```

## L714 · `pub const BIT_AB: u32 = 1 << 3; // reg.h:531`

```
// reg.h:531
```

## L715 · `pub const BIT_AM: u32 = 1 << 2; // reg.h:532`

```
// reg.h:532
```

## L716 · `pub const BIT_APM: u32 = 1 << 1; // reg.h:533`

```
// reg.h:533
```

## L718 · `pub const REG_WIFI_BT_INFO: u32 = 0x00AA; // reg.h:184`

```
// ── Koexistenz (reg.h) ───────────────────────────────────────────
```

## L719 · `pub const REG_WIFI_BT_INFO: u32 = 0x00AA; // reg.h:184`

```
// reg.h:184
```

## L720 · `pub const BIT_BT_INT_EN: u16 = 1 << 15; // reg.h:185`

```
// reg.h:185
```

## L721 · `pub const REG_BT_COEX_TABLE_H: u32 = 0x06CC; // reg.h:573`

```
// reg.h:573
```

## L722 · `pub const H2C_CMD_QUERY_BT_INFO: u32 = 0x61; // fw.h:568`

```
// fw.h:568
```

## L723 · `pub const H2C_CMD_BT_WIFI_CONTROL: u32 = 0x69; // fw.h:573`

```
// fw.h:573
```

## L725-728 · `pub const COEX_SET_ANT_INIT: u8 = 0; // coex.h:75`

```
// ── Koexistenz: Zustaende (coex.h, Aufzaehlungen ohne Zahlen) ────
// Sie stehen in `enum` ohne Wert, also zaehlt die POSITION. Abgezaehlt
// aus coex.h:74-85 bzw. 129-138 — und darum ist die Quellzeile hier
// wichtiger als sonst.
```

## L729 · `pub const COEX_SET_ANT_INIT: u8 = 0; // coex.h:75`

```
// coex.h:75
```

## L730 · `pub const COEX_SET_ANT_WONLY: u8 = 1; // coex.h:76`

```
// coex.h:76
```

## L731 · `pub const COEX_SET_ANT_WOFF: u8 = 2; // coex.h:77`

```
// coex.h:77
```

## L732 · `pub const COEX_SET_ANT_2G: u8 = 3; // coex.h:78`

```
// coex.h:78
```

## L733 · `pub const COEX_SET_ANT_5G: u8 = 4; // coex.h:79`

```
// coex.h:79
```

## L734 · `pub const COEX_SET_ANT_POWERON: u8 = 5; // coex.h:80`

```
// coex.h:80
```

## L735 · `pub const COEX_SET_ANT_2G_WLBT: u8 = 6; // coex.h:81`

```
// coex.h:81
```

## L736 · `pub const COEX_SET_ANT_2G_FREERUN: u8 = 7; // coex.h:82`

```
// coex.h:82
```

## L737 · `pub const COEX_SWITCH_CTRL_BY_BBSW: u8 = 0; // coex.h:130`

```
// coex.h:130
```

## L738 · `pub const COEX_SWITCH_CTRL_BY_PTA: u8 = 1; // coex.h:131`

```
// coex.h:131
```

## L739 · `pub const COEX_SWITCH_CTRL_BY_BT: u8 = 4; // coex.h:134`

```
// coex.h:134
```

## L740 · `pub const COEX_SWITCH_CTRL_MAX: u8 = 6; // coex.h:137`

```
// coex.h:137
```

## L741 · `pub const COEX_SWITCH_TO_MAX: u8 = 5; // coex.h:126 — fuenf Eintraege, nicht sieben`

```
// coex.h:126 — fuenf Eintraege, nicht sieben
```

## L743 · `pub const COEX_GNT_SET_HW_PTA: u32 = 0x0; // coex.h:114`

```
// coex.h:114
```

## L744 · `pub const COEX_GNT_SET_SW_LOW: u32 = 0x1; // coex.h:115`

```
// coex.h:115
```

## L745 · `pub const COEX_GNT_SET_SW_HIGH: u32 = 0x3; // coex.h:116`

```
// coex.h:116
```

## L747 · `pub const COEX_SCBD_ACTIVE: u16 = 0x0001; // coex.h:181`

```
// coex.h:181
```

## L748 · `pub const COEX_SCBD_ONOFF: u16 = 0x0002; // coex.h:182`

```
// coex.h:182
```

## L749 · `pub const COEX_SCBD_BT_RFK: u16 = 0x0020; // coex.h:186`

```
// coex.h:186
```

## L750 · `pub const COEX_SCBD_TDMA: u16 = 1 << 9; // coex.h:189`

```
// coex.h:189
```

## L751 · `pub const COEX_SCBD_FIX2M: u16 = 1 << 10; // coex.h:190`

```
// coex.h:190
```

## L752 · `pub const COEX_SCBD_ALL: u16 = 0xffff; // coex.h:191  GENMASK(15, 0)`

```
// coex.h:191  GENMASK(15, 0)
```

## L754 · `pub const COEX_H2C69_TDMA_SLOT: u8 = 0x0B; // coex.h:23`

```
// coex.h:23
```

## L755 · `pub const PARA1_H2C69_TDMA_4SLOT: u8 = 0xC1; // coex.h:24`

```
// coex.h:24
```

## L756 · `pub const PARA1_H2C69_TDMA_2SLOT: u8 = 0x01; // coex.h:25`

```
// coex.h:25
```

## L757 · `pub const COEX_H2C69_WL_LEAKAP: u8 = 0x0C; // coex.h:19`

```
// coex.h:19
```

## L758 · `pub const PARA1_H2C69_EN_5MS: u8 = 0x00; // coex.h:21`

```
// coex.h:21
```

## L759 · `pub const PARA1_H2C69_DIS_5MS: u8 = 0x01; // coex.h:20`

```
// coex.h:20
```

## L760 · `pub const COEX_H2C69_TOGGLE_TABLE_A: u8 = 0x0D; // coex.h:26`

```
// coex.h:26
```

## L762 · `pub const TDMA_4SLOT: u32 = 0x100; // coex.h:32`

```
// coex.h:32
```

## L763 · `pub const TDMA_TIMER_TYPE_2SLOT: u8 = 0; // coex.h:34`

```
// coex.h:34
```

## L764 · `pub const TDMA_TIMER_TYPE_4SLOT: u8 = 3; // coex.h:35`

```
// coex.h:35
```

## L765 · `pub const COEX_MIN_DELAY: u32 = 10; // coex.h:12`

```
// coex.h:12
```

## L766 · `pub const COEX_RFK_TIMEOUT: u32 = 600; // coex.h:13`

```
// coex.h:13
```

## L767 · `pub const COEX_WLPRI_TX_RSP: u8 = 3; // coex.h:265`

```
// coex.h:265
```

## L768 · `pub const COEX_WLPRI_TX_BEACON: u8 = 4; // coex.h:266`

```
// coex.h:266
```

## L769 · `pub const COEX_WLPRI_TX_BEACONQ: u8 = 27; // coex.h:269`

```
// coex.h:269
```

## L771 · `pub const REG_BT_COEX_TABLE0: u32 = 0x06C0; // reg.h:570`

```
// ── Koexistenz: Register ─────────────────────────────────────────
```

## L772 · `pub const REG_BT_COEX_TABLE0: u32 = 0x06C0; // reg.h:570`

```
// reg.h:570
```

## L773 · `pub const REG_BT_COEX_TABLE1: u32 = 0x06C4; // reg.h:571`

```
// reg.h:571
```

## L774 · `pub const REG_BT_COEX_BRK_TABLE: u32 = 0x06C8; // reg.h:572`

```
// reg.h:572
```

## L775 · `pub const REG_BT_STAT_CTRL: u32 = 0x0778; // reg.h:589`

```
// reg.h:589
```

## L776 · `pub const REG_BT_TDMA_TIME: u32 = 0x0790; // reg.h:590`

```
// reg.h:590
```

## L777 · `pub const BIT_MASK_SAMPLE_RATE: u32 = 0x3f; // reg.h:591`

```
// reg.h:591
```

## L778 · `pub const BIT_BT_PTA_EN: u32 = 1 << 5; // reg.h:77`

```
// reg.h:77
```

## L779 · `pub const BIT_PO_BT_PTA_PINS: u32 = 1 << 9; // reg.h:76`

```
// reg.h:76
```

## L780 · `pub const REG_QUEUE_CTRL: u32 = 0x04C6; // reg.h:432`

```
// reg.h:432
```

## L781 · `pub const BIT_PTA_WL_TX_EN: u8 = 1 << 4; // reg.h:433`

```
// reg.h:433
```

## L782 · `pub const BIT_PTA_EDCCA_EN: u8 = 1 << 5; // reg.h:434`

```
// reg.h:434
```

## L783 · `pub const REG_BT_COEX_V2: u32 = 0x0762; // reg.h:579`

```
// reg.h:579
```

## L784 · `pub const BIT_GNT_BT_POLARITY: u16 = 1 << 12; // reg.h:580`

```
// reg.h:580
```

## L785 · `pub const REG_DUMMY_PAGE4_V1: u32 = 0x04FC; // reg.h:445`

```
// reg.h:445
```

## L786 · `pub const BIT_BTCCA_CTRL: u8 = 0x3; // reg.h:441  GENMASK(1, 0)`

```
// reg.h:441  GENMASK(1, 0)
```

## L787 · `pub const RF_MODOPT: u32 = 0x01; // reg.h:957`

```
// reg.h:957
```

## L788 · `pub const REG_SYS_SDIO_CTRL: u32 = 0x0070; // reg.h:111`

```
// reg.h:111
```

## L789 · `pub const BIT_DBG_GNT_WL_BT: u32 = 1 << 27; // reg.h:112`

```
// reg.h:112
```

## L790 · `pub const BIT_LTE_MUX_CTRL_PATH: u32 = 1 << 26; // reg.h:113`

```
// reg.h:113
```

## L791 · `pub const BIT_BTGP_JTAG_EN: u32 = 1 << 24; // reg.h:104`

```
// reg.h:104
```

## L792 · `pub const BIT_BTGP_SPI_EN: u32 = 1 << 20; // reg.h:105`

```
// reg.h:105
```

## L793 · `pub const BIT_LED1DIS: u32 = 1 << 15; // reg.h:106`

```
// reg.h:106
```

## L794 · `pub const BIT_WL_RFK: u8 = 1 << 0; // reg.h:426`

```
// reg.h:426
```

## L795 · `pub const BIT_LTE_COEX_EN: u32 = 1 << 7; // reg.h:581`

```
// reg.h:581
```

## L796 · `pub const LTE_COEX_CTRL: u16 = 0x38; // reg.h:1000`

```
// reg.h:1000
```

## L797 · `pub const LTE_WL_TRX_CTRL: u16 = 0xa0; // reg.h:1001`

```
// reg.h:1001
```

## L798 · `pub const LTE_BT_TRX_CTRL: u16 = 0xa4; // reg.h:1002`

```
// reg.h:1002
```

## L799 · `pub const MASKLWORD: u32 = 0x0000ffff; // phy.h:177`

```
// phy.h:177
```

## L800 · `pub const H2C_CMD_COEX_TDMA_TYPE: u32 = 0x60; // fw.h:567`

```
// fw.h:567
```

## L801 · `pub const REG_IGN_GNT_BT1: u32 = 0x1860; // reg.h:910`

```
// reg.h:910
```

## L802 · `pub const REG_NOMASK_TXBT: u32 = 0x1CA7; // reg.h:927`

```
// reg.h:927
```

## L803 · `pub const REG_ANAPAR: u32 = 0x1C30; // reg.h:928`

```
// reg.h:928
```

## L804 · `pub const BIT_ANAPAR_BTPS: u32 = 1 << 22; // reg.h:929`

```
// reg.h:929
```

## L805 · `pub const REG_RSTB_SEL: u32 = 0x1C38; // reg.h:930`

```
// reg.h:930
```

## L806 · `pub const BIT_DAC_OFF_ENABLE: u32 = 1 << 4; // reg.h:931`

```
// reg.h:931
```

## L807 · `pub const BIT_PI_IGNORE_GNT_BT: u32 = 1 << 3; // reg.h:932`

```
// reg.h:932
```

## L808 · `pub const BIT_NOMASK_TXBT_ENABLE: u32 = 1 << 3; // reg.h:933`

```
// reg.h:933
```

## L809 · `pub const REG_IGN_GNTBT4: u32 = 0x4160; // reg.h:940`

```
// reg.h:940
```

## L810 · `pub const COEX_WLINK_5G: u8 = 0x3; // coex.h:175`

```
// coex.h:175
```

## L812-814 · `pub const REG_TXDFIR0: u32 = 0x0808; // rtw8822c.h:188`

```
// ════════════════════════════════════════════════════════════════
// Stufe 4c: rtw_set_channel
// ════════════════════════════════════════════════════════════════
```

## L816 · `pub const REG_TXDFIR0: u32 = 0x0808; // rtw8822c.h:188`

```
// ── rtw8822c_set_channel_bb ──────────────────────────────────────
```

## L817 · `pub const REG_TXDFIR0: u32 = 0x0808; // rtw8822c.h:188`

```
// rtw8822c.h:188
```

## L818 · `pub const REG_DFIRBW: u32 = 0x0810; // rtw8822c.h:189`

```
// rtw8822c.h:189
```

## L819 · `pub const REG_SBD: u32 = 0x088C; // rtw8822c.h:198`

```
// rtw8822c.h:198
```

## L820 · `pub const BITS_SUBTUNE: u32 = 0xf000; // rtw8822c.h:199`

```
// rtw8822c.h:199
```

## L821 · `pub const REG_TXBWCTL: u32 = 0x09B0; // rtw8822c.h:202`

```
// rtw8822c.h:202
```

## L822 · `pub const REG_TXCLK: u32 = 0x09B4; // rtw8822c.h:203`

```
// rtw8822c.h:203
```

## L823 · `pub const REG_SCOTRK: u32 = 0x0C30; // rtw8822c.h:205`

```
// rtw8822c.h:205
```

## L824 · `pub const REG_PT_CHSMO: u32 = 0x0CBC; // rtw8822c.h:209`

```
// rtw8822c.h:209
```

## L825 · `pub const BIT_PT_OPT: u32 = 1 << 21; // rtw8822c.h:210`

```
// rtw8822c.h:210
```

## L826 · `pub const REG_RXAGCCTL0: u32 = 0x18AC; // rtw8822c.h:226`

```
// rtw8822c.h:226
```

## L827 · `pub const BITS_RXAGC_CCK: u32 = 0xf000; // rtw8822c.h:227`

```
// rtw8822c.h:227
```

## L828 · `pub const BITS_RXAGC_OFDM: u32 = 0x01f0; // rtw8822c.h:228`

```
// rtw8822c.h:228
```

## L829 · `pub const REG_CCKSB: u32 = 0x1A00; // rtw8822c.h:234`

```
// rtw8822c.h:234
```

## L830 · `pub const REG_BGCTRL: u32 = 0x1A14; // rtw8822c.h:237`

```
// rtw8822c.h:237
```

## L831 · `pub const BITS_RX_IQ_WEIGHT: u32 = 0x300; // rtw8822c.h:238`

```
// rtw8822c.h:238
```

## L832 · `pub const REG_TXF0: u32 = 0x1A20; // rtw8822c.h:239`

```
// rtw8822c.h:239
```

## L833 · `pub const REG_TXF1: u32 = 0x1A24; // rtw8822c.h:240`

```
// rtw8822c.h:240
```

## L834 · `pub const REG_TXF2: u32 = 0x1A28; // rtw8822c.h:241`

```
// rtw8822c.h:241
```

## L835 · `pub const REG_CCKTXONLY: u32 = 0x1A80; // rtw8822c.h:246`

```
// rtw8822c.h:246
```

## L836 · `pub const BIT_BB_CCK_CHECK_EN: u32 = 1 << 18; // rtw8822c.h:247`

```
// rtw8822c.h:247
```

## L837 · `pub const REG_TXF3: u32 = 0x1A98; // rtw8822c.h:248`

```
// rtw8822c.h:248
```

## L838 · `pub const REG_TXF4: u32 = 0x1A9C; // rtw8822c.h:249`

```
// rtw8822c.h:249
```

## L839 · `pub const REG_TXF5: u32 = 0x1AA0; // rtw8822c.h:250`

```
// rtw8822c.h:250
```

## L840 · `pub const REG_TXF6: u32 = 0x1AAC; // rtw8822c.h:251`

```
// rtw8822c.h:251
```

## L841 · `pub const REG_TXF7: u32 = 0x1AB0; // rtw8822c.h:252`

```
// rtw8822c.h:252
```

## L842 · `pub const REG_CCK_SOURCE: u32 = 0x1ABC; // rtw8822c.h:253`

```
// rtw8822c.h:253
```

## L843 · `pub const BIT_NBI_EN: u32 = 1 << 30; // rtw8822c.h:254`

```
// rtw8822c.h:254
```

## L844 · `pub const REG_CCAMSK: u32 = 0x1C80; // rtw8822c.h:315`

```
// rtw8822c.h:315
```

## L845 · `pub const REG_RXAGCCTL: u32 = 0x41AC; // rtw8822c.h:358`

```
// rtw8822c.h:358
```

## L846 · `pub const REG_CCK_CHECK: u32 = 0x0454; // reg.h:408`

```
// reg.h:408
```

## L847 · `pub const BIT_CHECK_CCK_EN: u8 = 1 << 7; // reg.h:409`

```
// reg.h:409
```

## L849 · `pub const REG_ANAPAR_A: u32 = 0x1830; // rtw8822c.h:220`

```
// ── rtw8822c_rstb_3wire / set_channel_rf ─────────────────────────
```

## L850 · `pub const REG_ANAPAR_A: u32 = 0x1830; // rtw8822c.h:220`

```
// rtw8822c.h:220
```

## L851 · `pub const BIT_ANAPAR_UPDATE: u32 = 1 << 29; // rtw8822c.h:221`

```
// rtw8822c.h:221
```

## L852 · `pub const REG_ANAPAR_B: u32 = 0x4130; // rtw8822c.h:354`

```
// rtw8822c.h:354
```

## L853 · `pub const REG_RSTB: u32 = 0x1C90; // rtw8822c.h:316`

```
// rtw8822c.h:316
```

## L854 · `pub const BIT_RSTB_3WIRE: u32 = 1 << 8; // rtw8822c.h:317`

```
// rtw8822c.h:317
```

## L855 · `pub const RF_CFGCH: u32 = 0x18; // reg.h:961`

```
// reg.h:961
```

## L856 · `pub const RF_LUTWA: u32 = 0x33; // reg.h:971`

```
// reg.h:971
```

## L857 · `pub const RF_LUTWD0: u32 = 0x3F; // reg.h:973`

```
// reg.h:973
```

## L858 · `pub const RF_LUTWE2: u32 = 0xEE; // reg.h:997`

```
// reg.h:997
```

## L860 · `pub const REG_DATA_SC: u32 = 0x0483; // reg.h:419`

```
// ── rtw_set_channel_mac ──────────────────────────────────────────
```

## L861 · `pub const REG_DATA_SC: u32 = 0x0483; // reg.h:419`

```
// reg.h:419
```

## L862 · `pub const REG_WMAC_TRXPTCL_CTL: u32 = 0x0668; // reg.h:547`

```
// reg.h:547
```

## L863 · `pub const BIT_RFMOD: u32 = 0x180; // reg.h:548  GENMASK(8, 7)`

```
// reg.h:548  GENMASK(8, 7)
```

## L864 · `pub const BIT_RFMOD_80M: u32 = 1 << 8; // reg.h:549`

```
// reg.h:549
```

## L865 · `pub const BIT_RFMOD_40M: u32 = 1 << 7; // reg.h:550`

```
// reg.h:550
```

## L866 · `pub const MAC_CLK_HW_DEF_80M: u32 = 0; // reg.h:269`

```
// reg.h:269
```

## L867 · `pub const BIT_SHIFT_MAC_CLK_SEL: u32 = 20; // reg.h:268`

```
// reg.h:268
```

## L869 · `pub const RTW_SC_DONT_CARE: u8 = 0; // main.h:106`

```
// ── Unterkanallage (main.h:106-112) ──────────────────────────────
```

## L870 · `pub const RTW_SC_DONT_CARE: u8 = 0; // main.h:106`

```
// main.h:106
```

## L871 · `pub const RTW_SC_20_UPPER: u8 = 1; // main.h:107`

```
// main.h:107
```

## L872 · `pub const RTW_SC_20_LOWER: u8 = 2; // main.h:108`

```
// main.h:108
```

## L873 · `pub const RTW_SC_20_UPMOST: u8 = 3; // main.h:109`

```
// main.h:109
```

## L874 · `pub const RTW_SC_20_LOWEST: u8 = 4; // main.h:110`

```
// main.h:110
```

## L875 · `pub const RTW_SC_40_UPPER: u8 = 9; // main.h:111`

```
// main.h:111
```

## L876 · `pub const RTW_SC_40_LOWER: u8 = 10; // main.h:112`

```
// main.h:112
```

## L878 · `pub const MASKBYTE0: u32 = 0xff; // phy.h:172`

```
// phy.h:172
```

## L879 · `pub const MASKHWORD: u32 = 0xffff0000; // phy.h:176`

```
// phy.h:176
```

## L880 · `pub const MASKDWORD: u32 = 0xffffffff; // phy.h:178`

```
// phy.h:178
```

## L882 · `pub const DIS_DPD_RATE6M: u16 = 1 << 0; // reg.h:659`

```
// ── DPD-Abzug je Rate (reg.h:659-668) ────────────────────────────
```

## L883 · `pub const DIS_DPD_RATE6M: u16 = 1 << 0; // reg.h:659`

```
// reg.h:659
```

## L884 · `pub const DIS_DPD_RATE9M: u16 = 1 << 1; // reg.h:660`

```
// reg.h:660
```

## L885 · `pub const DIS_DPD_RATEMCS0: u16 = 1 << 2; // reg.h:661`

```
// reg.h:661
```

## L886 · `pub const DIS_DPD_RATEMCS1: u16 = 1 << 3; // reg.h:662`

```
// reg.h:662
```

## L887 · `pub const DIS_DPD_RATEMCS8: u16 = 1 << 4; // reg.h:663`

```
// reg.h:663
```

## L888 · `pub const DIS_DPD_RATEMCS9: u16 = 1 << 5; // reg.h:664`

```
// reg.h:664
```

## L889 · `pub const DIS_DPD_RATEVHT1SS_MCS0: u16 = 1 << 6; // reg.h:665`

```
// reg.h:665
```

## L890 · `pub const DIS_DPD_RATEVHT1SS_MCS1: u16 = 1 << 7; // reg.h:666`

```
// reg.h:666
```

## L891 · `pub const DIS_DPD_RATEVHT2SS_MCS0: u16 = 1 << 8; // reg.h:667`

```
// reg.h:667
```

## L892 · `pub const DIS_DPD_RATEVHT2SS_MCS1: u16 = 1 << 9; // reg.h:668`

```
// reg.h:668
```

## L894-895 · `pub const TXGI_FACTOR: i16 = 2;`

```
/// rtw8822c.c:5352 `.txgi_factor = 2` · :5385 `.en_dis_dpd = true`
/// · :5386 `.dpd_ratemask = DIS_DPD_RATEALL`
```

## L898 · `pub const DPD_RATEMASK: u16 = 0x3ff; // = DIS_DPD_RATEALL`

```
// = DIS_DPD_RATEALL
```

## L899 · `pub const BIT_SHIFT_TXSC_40M: u32 = 4; // reg.h:260`

```
// reg.h:260
```

## L900 · `pub const BIT_MASK_TXSC_40M: u8 = 0xf; // reg.h:261`

```
// reg.h:261
```

## L901 · `pub const BIT_SHIFT_TXSC_20M: u32 = 0; // reg.h:264`

```
// reg.h:264
```

## L902 · `pub const BIT_MASK_TXSC_20M: u8 = 0xf; // reg.h:265`

```
// reg.h:265
```

## L904-906 · `pub const PORT0_MAC_ADDR: u32 = 0x0610;`

```
// ── Stufe 5b: der Sendeweg ───────────────────────────────────────
// mac80211.c:108-115 `rtw_vif_port[0]`. Wir fahren nur Port 0 — Linux
// vergibt den ersten freien, und bei EINER Schnittstelle ist das die Null.
```

## L909 · `pub const PORT0_NET_TYPE: u32 = 0x0100; // = REG_CR`

```
// = REG_CR
```

## L913 · `pub const PORT0_BCN_CTRL: u32 = 0x0550; // = REG_BCN_CTRL`

```
// = REG_BCN_CTRL
```

## L916 · `pub const PORT_SET_MAC_ADDR: u32 = 1 << 0;`

```
// main.h:586-592 `enum rtw_vif_port_set`
```

## L923 · `pub const RTW_NET_NO_LINK: u32 = 0;`

```
// main.h:115-120 `enum rtw_net_type`
```

## L929-930 · `pub const H2C_CMD_SCAN: u32 = 0x59;`

```
// reg.h:478-480 stehen schon oben (REG_BCN_CTRL, BIT_DIS_TSF_UDT,
// BIT_EN_BCN_FUNCTION) — dort als u8, weil `rtw_write8_mask` sie schreibt.
```

## L932 · `pub const H2C_CMD_SCAN: u32 = 0x59;`

```
/// fw.h:564 `H2C_CMD_SCAN`
```

## L934 · `pub const FW_FEATURE_NOTIFY_SCAN: u32 = 1 << 6;`

```
/// fw.h:151 `FW_FEATURE_NOTIFY_SCAN`
```

## L937-938 · `pub const IQK_DONE_8822C: u8 = 0xaa;`

```
// ── Stufe 5d: die RF-Kalibrierung ────────────────────────────────
/// rtw8822c.h:21 `IQK_DONE_8822C`
```

## L940 · `pub const REG_NCTL0: u32 = 0x1b00;`

```
/// rtw8822c.h:256-264
```

## L942 · `pub const BIT_SEL_PATH: u32 = 0x6; // GENMASK(2, 1)`

```
// GENMASK(2, 1)
```

## L943 · `pub const BIT_SUBPAGE: u32 = 0xf; // GENMASK(3, 0)`

```
// GENMASK(3, 0)
```

## L950 · `pub const BIT_TX_CFIR: u32 = 0xc000_0000; // GENMASK(31, 30)`

```
// GENMASK(31, 30)
```

## L951 · `pub const REG_RPT_CIP: u32 = 0x2d9c;`

```
/// rtw8822c.h:348-349
```

## L953 · `pub const BIT_RPT_CIP_STATUS: u32 = 0xff; // GENMASK(7, 0)`

```
// GENMASK(7, 0)
```

## L954 · `pub const REG_PMC_DBG_CTRL1: u32 = 0xa8;`

```
/// reg.h:163-164
```

## L956 · `pub const BITS_PMC_BT_IQK_STS: u32 = 0x0060_0000; // GENMASK(22, 21)`

```
// GENMASK(22, 21)
```

## L957-958 · `pub const H2C_CMD_WIFI_CALIBRATION: u32 = 0x6d;`

```
// reg.h:425-426 REG_ARFR4/BIT_WL_RFK stehen schon oben.
/// fw.h:574 `H2C_CMD_WIFI_CALIBRATION`
```

## L960 · `pub const H2C_PKT_IQK: u8 = 0x0E;`

```
/// fw.h:391 `H2C_PKT_IQK`
```

## L963 · `pub const REG_RFTXEN_GCK_A: u32 = 0x1864; // rtw8822c.h:201`

```
// ── Stufe 5d: TXGAPK (rtw8822c.c:1191-1823) ─────────────────────
```

## L965-966 · `pub const REG_RFTXEN_GCK_A: u32 = 0x1864; // rtw8822c.h:201`

```
// NICHT GEFUNDEN — von Hand nachsehen:
//   BITS_RFC_DIRECT (Ausdruck: (BIT(31) | BIT(30)))
```

## L967 · `pub const REG_RFTXEN_GCK_A: u32 = 0x1864; // rtw8822c.h:201`

```
// rtw8822c.h:201
```

## L968 · `pub const REG_RFTXEN_GCK_B: u32 = 0x4164; // rtw8822c.h:334`

```
// rtw8822c.h:334
```

## L969 · `pub const BIT_RFTXEN_GCK_FORCE_ON: u32 = 1 << 31; // rtw8822c.h:202`

```
// rtw8822c.h:202
```

## L970 · `pub const BIT_DIS_SHARERX_TXGAT: u32 = 1 << 27; // rtw8822c.h:194`

```
// rtw8822c.h:194
```

## L971 · `pub const REG_DIS_SHARE_RX_A: u32 = 0x186c; // rtw8822c.h:203`

```
// rtw8822c.h:203
```

## L972 · `pub const REG_DIS_SHARE_RX_B: u32 = 0x416c; // rtw8822c.h:335`

```
// rtw8822c.h:335
```

## L973 · `pub const BIT_TX_SCALE_0DB: u32 = 1 << 7; // rtw8822c.h:204`

```
// rtw8822c.h:204
```

## L974 · `pub const BIT_3WIRE_EN: u32 = 0x00000003; // rtw8822c.h:197  GENMASK(1, 0)`

```
// rtw8822c.h:197  GENMASK(1, 0)
```

## L975 · `pub const BIT_BBMODE: u32 = 0x00000006; // rtw8822c.h:214  GENMASK(2, 1)`

```
// rtw8822c.h:214  GENMASK(2, 1)
```

## L976 · `pub const REG_IQK_CTRL: u32 = 0x1c38; // rtw8822c.h:290`

```
// rtw8822c.h:290
```

## L977 · `pub const RF_DEBUG: u32 = 0xde; // rtw8822c.h:379`

```
// rtw8822c.h:379
```

## L978 · `pub const BIT_DE_TX_GAIN: u32 = 1 << 16; // rtw8822c.h:381`

```
// rtw8822c.h:381
```

## L979 · `pub const RF_DIS_BYPASS_TXBB: u32 = 0x9e; // rtw8822c.h:376`

```
// rtw8822c.h:376
```

## L980 · `pub const BIT_TIA_BYPASS: u32 = 1 << 5; // rtw8822c.h:378`

```
// rtw8822c.h:378
```

## L981 · `pub const BIT_TXBB: u32 = 1 << 10; // rtw8822c.h:377`

```
// rtw8822c.h:377
```

## L982 · `pub const REG_SINGLE_TONE_SW: u32 = 0x1bb8; // rtw8822c.h:263`

```
// rtw8822c.h:263
```

## L983 · `pub const BIT_IRQ_TEST_MODE: u32 = 1 << 20; // rtw8822c.h:264`

```
// rtw8822c.h:264
```

## L984 · `pub const REG_R_CONFIG: u32 = 0x1bcc; // rtw8822c.h:265`

```
// rtw8822c.h:265
```

## L985 · `pub const BIT_CFIR_EN: u32 = 0x07000000; // rtw8822c.h:246  GENMASK(26, 24)`

```
// rtw8822c.h:246  GENMASK(26, 24)
```

## L986 · `pub const BIT_GAIN_TX_PAD_H: u32 = 0x00000f00; // rtw8822c.h:361  GENMASK(11, 8)`

```
// rtw8822c.h:361  GENMASK(11, 8)
```

## L987 · `pub const BIT_GAIN_TX_PAD_L: u32 = 0x000000f0; // rtw8822c.h:362  GENMASK(7, 4)`

```
// rtw8822c.h:362  GENMASK(7, 4)
```

## L988 · `pub const REG_TABLE_SEL: u32 = 0x1b98; // rtw8822c.h:255`

```
// rtw8822c.h:255
```

## L989 · `pub const BIT_Q_GAIN_SEL: u32 = 0x00007000; // rtw8822c.h:258  GENMASK(14, 12)`

```
// rtw8822c.h:258  GENMASK(14, 12)
```

## L990 · `pub const BIT_Q_GAIN: u32 = 0x00000fff; // rtw8822c.h:259  GENMASK(11, 0)`

```
// rtw8822c.h:259  GENMASK(11, 0)
```

## L991 · `pub const BIT_I_GAIN: u32 = 0x000f0000; // rtw8822c.h:256  GENMASK(19, 16)`

```
// rtw8822c.h:256  GENMASK(19, 16)
```

## L992 · `pub const BIT_GAIN_RST: u32 = 1 << 15; // rtw8822c.h:257`

```
// rtw8822c.h:257
```

## L993 · `pub const REG_TX_GAIN_SET: u32 = 0x1b9c; // rtw8822c.h:260`

```
// rtw8822c.h:260
```

## L994 · `pub const RF_GAIN_NUM: u32 = 11; // main.h:1648`

```
// main.h:1648
```

## L995 · `pub const BIT_ANT_PATH: u32 = 0x00000003; // rtw8822c.h:170  GENMASK(1, 0)`

```
// rtw8822c.h:170  GENMASK(1, 0)
```

## L996 · `pub const REG_TXANTSEG: u32 = 0x1e28; // rtw8822c.h:312`

```
// rtw8822c.h:312
```

## L997 · `pub const BIT_ANTSEG: u32 = 0x0000000f; // rtw8822c.h:313  GENMASK(3, 0)`

```
// rtw8822c.h:313  GENMASK(3, 0)
```

## L998 · `pub const BIT_PATH_EN: u32 = 1 << 31; // rtw8822c.h:192`

```
// rtw8822c.h:192
```

## L999 · `pub const RF_LUTDBG: u32 = 0xdf; // reg.h:965`

```
// reg.h:965
```

## L1000 · `pub const BIT_TXA_TANK: u32 = 1 << 4; // reg.h:966`

```
// reg.h:966
```

## L1001 · `pub const RF_IDAC: u32 = 0x58; // rtw8822c.h:358`

```
// rtw8822c.h:358
```

## L1002 · `pub const BIT_TX_MODE: u32 = 0x000fff00; // rtw8822c.h:359  GENMASK(19, 8)`

```
// rtw8822c.h:359  GENMASK(19, 8)
```

## L1003 · `pub const REG_TX_TONE_IDX: u32 = 0x1b2c; // rtw8822c.h:249`

```
// rtw8822c.h:249
```

## L1004 · `pub const BIT_2G_SWING: u32 = 0x2d; // rtw8822c.h:268`

```
// rtw8822c.h:268
```

## L1005 · `pub const BIT_5G_SWING: u32 = 0x36; // rtw8822c.h:269`

```
// rtw8822c.h:269
```

## L1006 · `pub const REG_RXSRAM_CTL: u32 = 0x1bd4; // rtw8822c.h:270`

```
// rtw8822c.h:270
```

## L1007 · `pub const BIT_RPT_EN: u32 = 1 << 21; // rtw8822c.h:271`

```
// rtw8822c.h:271
```

## L1008 · `pub const BIT_RPT_SEL: u32 = 0x001f0000; // rtw8822c.h:272  GENMASK(20, 16)`

```
// rtw8822c.h:272  GENMASK(20, 16)
```

## L1009 · `pub const BIT_GAPK_RPT_IDX: u32 = 0x00000f00; // rtw8822c.h:261  GENMASK(11, 8)`

```
// rtw8822c.h:261  GENMASK(11, 8)
```

## L1010 · `pub const REG_STAT_RPT: u32 = 0x1bfc; // rtw8822c.h:278`

```
// rtw8822c.h:278
```

## L1011 · `pub const BIT_GAPK_RPT0: u32 = 0x0000000f; // rtw8822c.h:280  GENMASK(3, 0)`

```
// rtw8822c.h:280  GENMASK(3, 0)
```

## L1012 · `pub const BIT_GAPK_RPT1: u32 = 0x000000f0; // rtw8822c.h:281  GENMASK(7, 4)`

```
// rtw8822c.h:281  GENMASK(7, 4)
```

## L1013 · `pub const BIT_GAPK_RPT2: u32 = 0x00000f00; // rtw8822c.h:282  GENMASK(11, 8)`

```
// rtw8822c.h:282  GENMASK(11, 8)
```

## L1014 · `pub const BIT_GAPK_RPT3: u32 = 0x0000f000; // rtw8822c.h:283  GENMASK(15, 12)`

```
// rtw8822c.h:283  GENMASK(15, 12)
```

## L1015 · `pub const BIT_GAPK_RPT4: u32 = 0x000f0000; // rtw8822c.h:284  GENMASK(19, 16)`

```
// rtw8822c.h:284  GENMASK(19, 16)
```

## L1016 · `pub const BIT_GAPK_RPT5: u32 = 0x00f00000; // rtw8822c.h:285  GENMASK(23, 20)`

```
// rtw8822c.h:285  GENMASK(23, 20)
```

## L1017 · `pub const BIT_GAPK_RPT6: u32 = 0x0f000000; // rtw8822c.h:286  GENMASK(27, 24)`

```
// rtw8822c.h:286  GENMASK(27, 24)
```

## L1018 · `pub const BIT_GAPK_RPT7: u32 = 0xf0000000; // rtw8822c.h:287  GENMASK(31, 28)`

```
// rtw8822c.h:287  GENMASK(31, 28)
```

## L1019 · `pub const RF_HW_OFFSET_NUM: u32 = 10; // main.h:1649`

```
// main.h:1649
```

## L1020 · `pub const BIT_IQ_SWITCH: u32 = 0x0000003f; // rtw8822c.h:267  GENMASK(5, 0)`

```
// rtw8822c.h:267  GENMASK(5, 0)
```

## L1021 · `pub const RF_MODE_TRXAGC: u32 = 0x00; // rtw8822c.h:343`

```
// rtw8822c.h:343
```

## L1022 · `pub const RF_TX_GAIN_OFFSET: u32 = 0x55; // rtw8822c.h:353`

```
// rtw8822c.h:353
```

## L1023 · `pub const BIT_RF_GAIN: u32 = 0x0000001c; // rtw8822c.h:355  GENMASK(4, 2)`

```
// rtw8822c.h:355  GENMASK(4, 2)
```

## L1024 · `pub const RF_RXG_GAIN: u32 = 0x87; // rtw8822c.h:370`

```
// rtw8822c.h:370
```

## L1025 · `pub const BIT_RXG_GAIN: u32 = 1 << 18; // rtw8822c.h:371`

```
// rtw8822c.h:371
```

## L1026 · `pub const BIT_RXAGC: u32 = 0x000003e0; // rtw8822c.h:345  GENMASK(9, 5)`

```
// rtw8822c.h:345  GENMASK(9, 5)
```

## L1027 · `pub const BIT_DE_TRXBW: u32 = 1 << 2; // rtw8822c.h:382`

```
// rtw8822c.h:382
```

## L1028 · `pub const RF_BW_TRXBB: u32 = 0x1a; // rtw8822c.h:348`

```
// rtw8822c.h:348
```

## L1029 · `pub const BIT_BW_TXBB: u32 = 0x00007000; // rtw8822c.h:350  GENMASK(14, 12)`

```
// rtw8822c.h:350  GENMASK(14, 12)
```

## L1030 · `pub const BIT_BW_RXBB: u32 = 0x00000c00; // rtw8822c.h:351  GENMASK(11, 10)`

```
// rtw8822c.h:351  GENMASK(11, 10)
```

## L1031 · `pub const RF_EXT_TIA_BW: u32 = 0x8f; // rtw8822c.h:374`

```
// rtw8822c.h:374
```

## L1032 · `pub const BIT_PW_EXT_TIA: u32 = 1 << 1; // rtw8822c.h:375`

```
// rtw8822c.h:375
```

## L1033 · `pub const RF_TXA_LB_SW: u32 = 0x63; // rtw8822c.h:366`

```
// rtw8822c.h:366
```

## L1034 · `pub const BIT_TXA_LB_ATT: u32 = 0x0000c000; // rtw8822c.h:367  GENMASK(15, 14)`

```
// rtw8822c.h:367  GENMASK(15, 14)
```

## L1035 · `pub const BIT_LB_ATT: u32 = 0x0000001c; // rtw8822c.h:369  GENMASK(4, 2)`

```
// rtw8822c.h:369  GENMASK(4, 2)
```

## L1036 · `pub const BIT_LB_SW: u32 = 0x00003000; // rtw8822c.h:368  GENMASK(13, 12)`

```
// rtw8822c.h:368  GENMASK(13, 12)
```

## L1037 · `pub const RF_RXA_MIX_GAIN: u32 = 0x8a; // rtw8822c.h:372`

```
// rtw8822c.h:372
```

## L1038 · `pub const BIT_RXA_MIX_GAIN: u32 = 0x00000018; // rtw8822c.h:373  GENMASK(4, 3)`

```
// rtw8822c.h:373  GENMASK(4, 3)
```

## L1039 · `pub const BIT_RF_MODE: u32 = 0x000f0000; // rtw8822c.h:344  GENMASK(19, 16)`

```
// rtw8822c.h:344  GENMASK(19, 16)
```

## L1040 · `pub const BIT_GAIN_EXT: u32 = 1 << 12; // reg.h:944`

```
// reg.h:944
```

## L1041 · `pub const BIT_DATA_L: u32 = 0x00000fff; // reg.h:945  GENMASK(11, 0)`

```
// reg.h:945  GENMASK(11, 0)
```

## L1042 · `pub const BIT_BAND: u32 = 0x00070000; // reg.h:932  GENMASK(18, 16)`

```
// reg.h:932  GENMASK(18, 16)
```

## L1043 · `pub const BIT_DBG_CCK_CCA: u32 = 1 << 1; // rtw8822c.h:352`

```
// rtw8822c.h:352
```

## L1044 · `pub const BIT_TX_CCK_IND: u32 = 1 << 16; // rtw8822c.h:349`

```
// rtw8822c.h:349
```

## L1045 · `pub const RF_TX_RESULT: u32 = 0x5f; // rtw8822c.h:360`

```
// rtw8822c.h:360
```

## L1046 · `pub const RF_BAND_2G_CCK: u32 = 0; // main.h:1639`

```
// main.h:1639
```

## L1047 · `pub const RF_BAND_2G_OFDM: u32 = 1; // main.h:1639`

```
// main.h:1639
```

## L1048 · `pub const RF_BAND_5G_L: u32 = 2; // main.h:1639`

```
// main.h:1639
```

## L1049 · `pub const RF_BAND_5G_M: u32 = 3; // main.h:1639`

```
// main.h:1639
```

## L1050 · `pub const RF_BAND_5G_H: u32 = 4; // main.h:1639`

```
// main.h:1639
```

## L1051 · `pub const RF_BAND_MAX: u32 = 5; // main.h:1639`

```
// main.h:1639
```

## L1052 · `pub const RTW_DM_CAP_TXGAPK: u32 = 1; // main.h:1687`

```
// main.h:1687
```

## L1053 · `pub const REG_TX_FIFO: u32 = 0x1e70; // rtw8822c.h:316`

```
// rtw8822c.h:316
```

## L1054 · `pub const BIT_STOP_TX: u32 = 0x0000000f; // rtw8822c.h:317  GENMASK(3, 0)`

```
// rtw8822c.h:317  GENMASK(3, 0)
```

## L1055 · `pub const BIT_AC_QUEUE: u32 = 0x000000ff; // reg.h:452  GENMASK(7, 0)`

```
// reg.h:452  GENMASK(7, 0)
```

## L1056 · `pub const REG_ENFN: u32 = 0x1e24; // rtw8822c.h:310`

```
// rtw8822c.h:310
```

## L1057 · `pub const BIT_IQK_DPK_EN: u32 = 1 << 17; // rtw8822c.h:311`

```
// rtw8822c.h:311
```

## L1058 · `pub const REG_CH_DELAY_EXTR2: u32 = 0x1cd0; // rtw8822c.h:297`

```
// rtw8822c.h:297
```

## L1059 · `pub const BIT_IQK_DPK_CLOCK_SRC: u32 = 1 << 28; // rtw8822c.h:301`

```
// rtw8822c.h:301
```

## L1060 · `pub const BIT_IQK_DPK_RESET_SRC: u32 = 1 << 29; // rtw8822c.h:300`

```
// rtw8822c.h:300
```

## L1061 · `pub const BIT_EN_IOQ_IQK_DPK: u32 = 1 << 30; // rtw8822c.h:299`

```
// rtw8822c.h:299
```

## L1062 · `pub const BIT_TST_IQK2SET_SRC: u32 = 1 << 31; // rtw8822c.h:298`

```
// rtw8822c.h:298
```

## L1063 · `pub const REG_CCA_OFF: u32 = 0x1d58; // rtw8822c.h:306`

```
// rtw8822c.h:306
```

## L1064 · `pub const BIT_CCA_ON_BY_PW: u32 = 0x00000ff8; // rtw8822c.h:307  GENMASK(11, 3)`

```
// rtw8822c.h:307  GENMASK(11, 3)
```

## L1065 · `pub const BITS_RFC_DIRECT: u32 = 0xc0000000; // reg.h:36  (BIT(31) | BIT(30))`

```
// reg.h:36  (BIT(31) | BIT(30))
```

## L1067 · `pub const BIT_DPD_CLK: u32 = 0x000000f0; // rtw8822c.h:273  GENMASK(7, 4)`

```
// ── Stufe 5d: DPK (rtw8822c.c:3171-4186) ────────────────────────
```

## L1068 · `pub const BIT_DPD_CLK: u32 = 0x000000f0; // rtw8822c.h:273  GENMASK(7, 4)`

```
// rtw8822c.h:273  GENMASK(7, 4)
```

## L1069 · `pub const DPK_RF_REG_NUM: u32 = 7; // main.h:1575`

```
// main.h:1575
```

## L1070 · `pub const DPK_BB_REG_NUM: u32 = 18; // main.h:1577`

```
// main.h:1577
```

## L1071 · `pub const DPK_RF_PATH_NUM: u32 = 2; // main.h:1576`

```
// main.h:1576
```

## L1072 · `pub const RF_RXAGC_OFFSET: u32 = 0x19; // rtw8822c.h:347`

```
// rtw8822c.h:347
```

## L1073 · `pub const REG_DPD_CTL1_S1: u32 = 0x1b60; // rtw8822c.h:253`

```
// rtw8822c.h:253
```

## L1074 · `pub const REG_DPD_LUT0: u32 = 0x1b44; // rtw8822c.h:250`

```
// rtw8822c.h:250
```

## L1075 · `pub const BIT_GLOSS_DB: u32 = 0x00007000; // rtw8822c.h:251  GENMASK(14, 12)`

```
// rtw8822c.h:251  GENMASK(14, 12)
```

## L1076 · `pub const REG_DPD_CTL11: u32 = 0x1be4; // rtw8822c.h:274`

```
// rtw8822c.h:274
```

## L1077 · `pub const REG_DPD_CTL12: u32 = 0x1be8; // rtw8822c.h:275`

```
// rtw8822c.h:275
```

## L1078 · `pub const RF_TX_GAIN: u32 = 0x56; // rtw8822c.h:356`

```
// rtw8822c.h:356
```

## L1079 · `pub const BIT_DE_PWR_TRIM: u32 = 1 << 19; // rtw8822c.h:380`

```
// rtw8822c.h:380
```

## L1080 · `pub const BIT_BB_GAIN: u32 = 0x0007c000; // rtw8822c.h:354  GENMASK(18, 14)`

```
// rtw8822c.h:354  GENMASK(18, 14)
```

## L1081 · `pub const DPK_CHANNEL_WIDTH_80: u32 = 1; // main.h:1578`

```
// main.h:1578
```

## L1082 · `pub const RTW_DPK_GAIN_LOSS: u32 = 1; // rtw8822c.h:118`

```
// rtw8822c.h:118
```

## L1083 · `pub const RTW_DPK_DO_DPK: u32 = 2; // rtw8822c.h:118`

```
// rtw8822c.h:118
```

## L1084 · `pub const RTW_DPK_DPK_ON: u32 = 3; // rtw8822c.h:118`

```
// rtw8822c.h:118
```

## L1085 · `pub const RTW_DPK_DAGC: u32 = 4; // rtw8822c.h:118`

```
// rtw8822c.h:118
```

## L1086 · `pub const RTW_DPK_CAL_PWR: u32 = 0; // rtw8822c.h:118`

```
// rtw8822c.h:118
```

## L1087 · `pub const REG_DPD_CTL0: u32 = 0x1bb4; // rtw8822c.h:262`

```
// rtw8822c.h:262
```

## L1088 · `pub const RF_T_METER: u32 = 0x42; // reg.h:946`

```
// reg.h:946
```

## L1089 · `pub const RTW_DPK_GAIN_LESS: u32 = 2; // rtw8822c.h:108`

```
// rtw8822c.h:108
```

## L1090 · `pub const RTW_DPK_GAIN_LARGE: u32 = 1; // rtw8822c.h:108`

```
// rtw8822c.h:108
```

## L1091 · `pub const RTW_DPK_GL_LESS: u32 = 4; // rtw8822c.h:108`

```
// rtw8822c.h:108
```

## L1092 · `pub const RTW_DPK_GL_LARGE: u32 = 3; // rtw8822c.h:108`

```
// rtw8822c.h:108
```

## L1093 · `pub const RTW_DPK_AGC_OUT: u32 = 6; // rtw8822c.h:108`

```
// rtw8822c.h:108
```

## L1094 · `pub const RTW_DPK_LOSS_CHECK: u32 = 5; // rtw8822c.h:108`

```
// rtw8822c.h:108
```

## L1095 · `pub const RTW_DPK_GAIN_CHECK: u32 = 0; // rtw8822c.h:108`

```
// rtw8822c.h:108
```

## L1096 · `pub const BIT_GAIN_TXBB: u32 = 0x0000001f; // rtw8822c.h:357  GENMASK(4, 0)`

```
// rtw8822c.h:357  GENMASK(4, 0)
```

## L1097 · `pub const BIT_TXAGC: u32 = 0x0000001f; // rtw8822c.h:346  GENMASK(4, 0)`

```
// rtw8822c.h:346  GENMASK(4, 0)
```

## L1098 · `pub const REG_DPD_CTL0_S1: u32 = 0x1b5c; // rtw8822c.h:252`

```
// rtw8822c.h:252
```

## L1099 · `pub const REG_DPD_AGC: u32 = 0x1b67; // rtw8822c.h:254`

```
// rtw8822c.h:254
```

## L1100 · `pub const BIT_BYPASS_DPD: u32 = 1 << 25; // rtw8822c.h:247`

```
// rtw8822c.h:247
```

## L1101 · `pub const BIT_INNER_LB: u32 = 1 << 21; // rtw8822c.h:266`

```
// rtw8822c.h:266
```

## L1102 · `pub const BIT_GS_PWSF: u32 = 0x0fffffff; // rtw8822c.h:239  GENMASK(27, 0)`

```
// rtw8822c.h:239  GENMASK(27, 0)
```

## L1103 · `pub const REG_DPD_CTL16: u32 = 0x1bf8; // rtw8822c.h:277`

```
// rtw8822c.h:277
```

## L1104 · `pub const REG_DPD_CTL15: u32 = 0x1bf4; // rtw8822c.h:276`

```
// rtw8822c.h:276
```

## L1105 · `pub const MASKBYTE3: u32 = 0xff000000; // phy.h:156`

```
// phy.h:156
```

## L1106 · `pub const BIT_RPT_DGAIN: u32 = 0x0fff0000; // rtw8822c.h:279  GENMASK(27, 16)`

```
// rtw8822c.h:279  GENMASK(27, 16)
```

## L1107 · `pub const MASKBYTE1: u32 = 0xff00; // phy.h:154`

```
// phy.h:154
```

## L1109-1111 · `pub const RTW_BAND_2G_MASK: u32 = 1 << 0;`

```
/// main.h:99 `RTW_BAND_2G = BIT(NL80211_BAND_2GHZ)` — `NL80211_BAND_2GHZ`
/// ist 0, also BIT(0). Der Name steht in einem anderen Baum (cfg80211),
/// deshalb hier ausgerechnet statt erzeugt.
```

## L1114 · `pub const H2C_CMD_MEDIA_STATUS_RPT: u32 = 0x01; // fw.h:484`

```
// ── Stufe 5e: Verbinden ─────────────────────────────────────────
```

## L1115 · `pub const H2C_CMD_MEDIA_STATUS_RPT: u32 = 0x01; // fw.h:484`

```
// fw.h:484
```

## L1116 · `pub const C2H_CCX_TX_RPT: u32 = 0x03; // fw.h:51`

```
// fw.h:51
```

## L1117 · `pub const C2H_BT_INFO: u32 = 0x09; // fw.h:51`

```
// fw.h:51
```

## L1118 · `pub const C2H_BT_MP_INFO: u32 = 0x0b; // fw.h:51`

```
// fw.h:51
```

## L1119 · `pub const C2H_BT_HID_INFO: u32 = 0x45; // fw.h:51`

```
// fw.h:51
```

## L1120 · `pub const C2H_RA_RPT: u32 = 0x0c; // fw.h:51`

```
// fw.h:51
```

## L1121 · `pub const C2H_WLAN_INFO: u32 = 0x27; // fw.h:51`

```
// fw.h:51
```

## L1122 · `pub const C2H_WLAN_RFON: u32 = 0x32; // fw.h:51`

```
// fw.h:51
```

## L1123 · `pub const C2H_BCN_FILTER_NOTIFY: u32 = 0x36; // fw.h:51`

```
// fw.h:51
```

## L1124 · `pub const C2H_ADAPTIVITY: u32 = 0x37; // fw.h:51`

```
// fw.h:51
```

## L1125 · `pub const C2H_SCAN_RESULT: u32 = 0x38; // fw.h:51`

```
// fw.h:51
```

## L1126 · `pub const C2H_HALMAC: u32 = 0xff; // fw.h:51`

```
// fw.h:51
```

## L1128 · `pub const IEEE80211_HT_CAP_SGI_20: u32 = 0x0020; // ../../../../../include/linux/ieee80211.h:1917`

```
// ── Stufe 5f: Ratenanpassung (main.c:1117-1240) ─────────────────
```

## L1129 · `pub const IEEE80211_HT_CAP_SGI_20: u32 = 0x0020; // ../../../../../include/linux/ieee80211.h:1917`

```
// ../../../../../include/linux/ieee80211.h:1917
```

## L1130 · `pub const IEEE80211_HT_CAP_SGI_40: u32 = 0x0040; // ../../../../../include/linux/ieee80211.h:1918`

```
// ../../../../../include/linux/ieee80211.h:1918
```

## L1131 · `pub const IEEE80211_HT_CAP_RX_STBC: u32 = 0x0300; // ../../../../../include/linux/ieee80211.h:1920`

```
// ../../../../../include/linux/ieee80211.h:1920
```

## L1132 · `pub const IEEE80211_HT_CAP_LDPC_CODING: u32 = 0x0001; // ../../../../../include/linux/ieee80211.h:1912`

```
// ../../../../../include/linux/ieee80211.h:1912
```

## L1133 · `pub const IEEE80211_HT_CAP_SUP_WIDTH_20_40: u32 = 0x0002; // ../../../../../include/linux/ieee80211.h:1913`

```
// ../../../../../include/linux/ieee80211.h:1913
```

## L1134 · `pub const IEEE80211_HT_CAP_SM_PS: u32 = 0x000C; // ../../../../../include/linux/ieee80211.h:1915`

```
// ../../../../../include/linux/ieee80211.h:1915
```

## L1135 · `pub const IEEE80211_HT_CAP_SM_PS_SHIFT: u32 = 2; // ../../../../../include/linux/ieee80211.h:1916`

```
// ../../../../../include/linux/ieee80211.h:1916
```

## L1136 · `pub const WLAN_HT_CAP_SM_PS_DISABLED: u32 = 3; // ../../../../../include/linux/ieee80211.h:2055`

```
// ../../../../../include/linux/ieee80211.h:2055
```

## L1137 · `pub const IEEE80211_VHT_CAP_SHORT_GI_80: u32 = 0x00000020; // ../../../../../include/linux/ieee80211.h:2438`

```
// ../../../../../include/linux/ieee80211.h:2438
```

## L1138 · `pub const IEEE80211_VHT_CAP_RXSTBC_MASK: u32 = 0x00000700; // ../../../../../include/linux/ieee80211.h:2445`

```
// ../../../../../include/linux/ieee80211.h:2445
```

## L1139 · `pub const IEEE80211_VHT_CAP_RXLDPC: u32 = 0x00000010; // ../../../../../include/linux/ieee80211.h:2437`

```
// ../../../../../include/linux/ieee80211.h:2437
```

## L1140 · `pub const IEEE80211_VHT_CAP_SUPP_CHAN_WIDTH_MASK: u32 = 0x0000000C; // ../../../../../include/linux/ieee80211.h:2435`

```
// ../../../../../include/linux/ieee80211.h:2435
```

## L1141 · `pub const WLAN_EID_HT_CAPABILITY: u32 = 45; // ../../../../../include/linux/ieee80211.h:3659`

```
// ../../../../../include/linux/ieee80211.h:3659
```

## L1142 · `pub const WLAN_EID_VHT_CAPABILITY: u32 = 191; // ../../../../../include/linux/ieee80211.h:3659`

```
// ../../../../../include/linux/ieee80211.h:3659
```

## L1143 · `pub const WLAN_EID_HT_OPERATION: u32 = 61; // ../../../../../include/linux/ieee80211.h:3659`

```
// ../../../../../include/linux/ieee80211.h:3659
```

## L1144 · `pub const WLAN_EID_VHT_OPERATION: u32 = 192; // ../../../../../include/linux/ieee80211.h:3659`

```
// ../../../../../include/linux/ieee80211.h:3659
```

## L1145 · `pub const WLAN_EID_SUPP_RATES: u32 = 1; // ../../../../../include/linux/ieee80211.h:3659`

```
// ../../../../../include/linux/ieee80211.h:3659
```

## L1146 · `pub const WLAN_EID_EXT_SUPP_RATES: u32 = 50; // ../../../../../include/linux/ieee80211.h:3659`

```
// ../../../../../include/linux/ieee80211.h:3659
```

## L1147 · `pub const WLAN_EID_SSID: u32 = 0; // ../../../../../include/linux/ieee80211.h:3659`

```
// ../../../../../include/linux/ieee80211.h:3659
```

## L1148 · `pub const WLAN_EID_RSN: u32 = 48; // ../../../../../include/linux/ieee80211.h:3659`

```
// ../../../../../include/linux/ieee80211.h:3659
```

## L1149 · `pub const WLAN_EID_DS_PARAMS: u32 = 3; // ../../../../../include/linux/ieee80211.h:3659`

```
// ../../../../../include/linux/ieee80211.h:3659
```

## L1150 · `pub const RA_MASK_CCK_RATES: u32 = 0x0000f; // main.c:1117`

```
// main.c:1117
```

## L1151 · `pub const RA_MASK_OFDM_RATES: u32 = 0x00ff0; // main.c:1118`

```
// main.c:1118
```

## L1152 · `pub const RA_MASK_HT_RATES_1SS: u32 = 0xff000; // main.c:1119  (0xff000ULL << 0)`

```
// main.c:1119  (0xff000ULL << 0)
```

## L1153 · `pub const RA_MASK_HT_RATES_2SS: u32 = 0xff00000; // main.c:1120  (0xff000ULL << 8)`

```
// main.c:1120  (0xff000ULL << 8)
```

## L1154 · `pub const RA_MASK_HT_RATES_3SS: u64 = 0xff0000000; // main.c:1121  (0xff000ULL << 16)`

```
// main.c:1121  (0xff000ULL << 16)
```

## L1155 · `pub const RA_MASK_HT_RATES: u64 = 0xffffff000; // main.c:1122  (RA_MASK_HT_RATES_1SS |  				 RA_MASK_HT_RATES_2SS |  			`

```
// main.c:1122  (RA_MASK_HT_RATES_1SS |  				 RA_MASK_HT_RATES_2SS |  				 RA_MASK_HT_RATES_3SS)
```

## L1156 · `pub const RA_MASK_VHT_RATES_1SS: u32 = 0x3ff000; // main.c:1123  (0x3ff000ULL << 0)`

```
// main.c:1123  (0x3ff000ULL << 0)
```

## L1157 · `pub const RA_MASK_VHT_RATES_2SS: u32 = 0xffc00000; // main.c:1124  (0x3ff000ULL << 10)`

```
// main.c:1124  (0x3ff000ULL << 10)
```

## L1158 · `pub const RA_MASK_VHT_RATES_3SS: u64 = 0x3ff00000000; // main.c:1125  (0x3ff000ULL << 20)`

```
// main.c:1125  (0x3ff000ULL << 20)
```

## L1159 · `pub const RA_MASK_VHT_RATES: u64 = 0x3fffffff000; // main.c:1126  (RA_MASK_VHT_RATES_1SS |  				 RA_MASK_VHT_RATES_2SS |`

```
// main.c:1126  (RA_MASK_VHT_RATES_1SS |  				 RA_MASK_VHT_RATES_2SS |  				 RA_MASK_VHT_RATES_3SS)
```

## L1160 · `pub const RA_MASK_CCK_IN_BG: u32 = 0x00005; // main.c:1127`

```
// main.c:1127
```

## L1161 · `pub const RA_MASK_CCK_IN_HT: u32 = 0x00005; // main.c:1128`

```
// main.c:1128
```

## L1162 · `pub const RA_MASK_CCK_IN_VHT: u32 = 0x00005; // main.c:1129`

```
// main.c:1129
```

## L1163 · `pub const RA_MASK_OFDM_IN_VHT: u32 = 0x00010; // main.c:1130`

```
// main.c:1130
```

## L1164 · `pub const RA_MASK_OFDM_IN_HT_2G: u32 = 0x00010; // main.c:1131`

```
// main.c:1131
```

## L1165 · `pub const RA_MASK_OFDM_IN_HT_5G: u32 = 0x00030; // main.c:1132`

```
// main.c:1132
```

## L1166 · `pub const WIRELESS_CCK: u32 = 0x00000001; // main.h:176`

```
// main.h:176
```

## L1167 · `pub const WIRELESS_OFDM: u32 = 0x00000002; // main.h:176`

```
// main.h:176
```

## L1168 · `pub const WIRELESS_HT: u32 = 0x00000004; // main.h:176`

```
// main.h:176
```

## L1169 · `pub const WIRELESS_VHT: u32 = 0x00000008; // main.h:176`

```
// main.h:176
```

## L1170 · `pub const RRSR_INIT_2G: u32 = 0x15f; // main.h:1684`

```
// main.h:1684
```

## L1171 · `pub const RRSR_INIT_5G: u32 = 0x150; // main.h:1685`

```
// main.h:1685
```

## L1172 · `pub const RTW_RATEID_BG: u32 = 6; // main.h:226`

```
// main.h:226
```

## L1173 · `pub const RTW_RATEID_GN_N1SS: u32 = 5; // main.h:226`

```
// main.h:226
```

## L1174 · `pub const RTW_RATEID_GN_N2SS: u32 = 4; // main.h:226`

```
// main.h:226
```

## L1175 · `pub const RTW_RATEID_BGN_20M_1SS: u32 = 3; // main.h:226`

```
// main.h:226
```

## L1176 · `pub const RTW_RATEID_BGN_20M_2SS: u32 = 2; // main.h:226`

```
// main.h:226
```

## L1177 · `pub const RTW_RATEID_BGN_40M_1SS: u32 = 1; // main.h:226`

```
// main.h:226
```

## L1178 · `pub const RTW_RATEID_BGN_40M_2SS: u32 = 0; // main.h:226`

```
// main.h:226
```

## L1179 · `pub const RTW_RATEID_ARFR1_AC_1SS: u32 = 10; // main.h:226`

```
// main.h:226
```

## L1180 · `pub const RTW_RATEID_ARFR0_AC_2SS: u32 = 9; // main.h:226`

```
// main.h:226
```

## L1181 · `pub const RTW_RATEID_ARFR2_AC_2G_1SS: u32 = 11; // main.h:226`

```
// main.h:226
```

## L1182 · `pub const RTW_RATEID_ARFR3_AC_2G_2SS: u32 = 12; // main.h:226`

```
// main.h:226
```

## L1183 · `pub const H2C_CMD_RA_INFO: u32 = 0x40; // fw.h:488`

```
// fw.h:488
```

## L1184 · `pub const RTW_C2H_RA_RPT_RATE: u32 = 0x0000007f; // fw.h:98  GENMASK(6, 0)`

```
// fw.h:98  GENMASK(6, 0)
```

## L1185 · `pub const RTW_C2H_RA_RPT_SGI: u32 = 1 << 7; // fw.h:99`

```
// fw.h:99
```

## L1186 · `pub const VHT_STBC_EN: u8 = 1 << 1; // main.h:184`

```
// main.h:184
```

## L1187 · `pub const VHT_LDPC_EN: u8 = 1 << 1; // main.h:186`

```
// main.h:186
```

## L1188 · `pub const HT_STBC_EN: u8 = 1 << 0; // main.h:183`

```
// main.h:183
```

## L1189 · `pub const HT_LDPC_EN: u8 = 1 << 0; // main.h:185`

```
// main.h:185
```

## L1190 · `pub const H2C_CMD_DEFAULT_PORT: u32 = 0x2c; // fw.h:487`

```
// fw.h:487
```

## L1191 · `pub const RTW_H2C_DEFAULT_PORT_W0_PORTID: u32 = 0x0000ff00; // fw.h:109  GENMASK(15, 8)`

```
// fw.h:109  GENMASK(15, 8)
```

## L1192 · `pub const RTW_H2C_DEFAULT_PORT_W0_MACID: u32 = 0x00ff0000; // fw.h:110  GENMASK(23, 16)`

```
// fw.h:110  GENMASK(23, 16)
```

## L1194 · `pub const IEEE80211_HT_CAP_MAX_AMSDU: u32 = 0x0800; // ../../../../../include/linux/ieee80211.h:1923`

```
// ── Stufe 5f.1: unsere EIGENEN Faehigkeiten (main.c:1580-1640) ──
```

## L1195 · `pub const IEEE80211_HT_CAP_MAX_AMSDU: u32 = 0x0800; // ../../../../../include/linux/ieee80211.h:1923`

```
// ../../../../../include/linux/ieee80211.h:1923
```

## L1196 · `pub const IEEE80211_HT_CAP_RX_STBC_SHIFT: u32 = 8; // ../../../../../include/linux/ieee80211.h:1921`

```
// ../../../../../include/linux/ieee80211.h:1921
```

## L1197 · `pub const IEEE80211_HT_CAP_TX_STBC: u32 = 0x0080; // ../../../../../include/linux/ieee80211.h:1919`

```
// ../../../../../include/linux/ieee80211.h:1919
```

## L1198 · `pub const IEEE80211_HT_CAP_DSSSCCK40: u32 = 0x1000; // ../../../../../include/linux/ieee80211.h:1924`

```
// ../../../../../include/linux/ieee80211.h:1924
```

## L1199 · `pub const IEEE80211_HT_MAX_AMPDU_64K: u32 = 3; // ../../../../../include/linux/ieee80211.h:1947`

```
// ../../../../../include/linux/ieee80211.h:1947
```

## L1200 · `pub const IEEE80211_HT_MPDU_DENSITY_2: u32 = 4; // ../../../../../include/linux/ieee80211.h:1972`

```
// ../../../../../include/linux/ieee80211.h:1972
```

## L1201 · `pub const IEEE80211_HT_MCS_TX_DEFINED: u32 = 0x01; // ../../../../../include/linux/ieee80211.h:1867`

```
// ../../../../../include/linux/ieee80211.h:1867
```

## L1202 · `pub const IEEE80211_VHT_CAP_MAX_MPDU_LENGTH_11454: u32 = 0x00000002; // ../../../../../include/linux/ieee80211.h:2431`

```
// ../../../../../include/linux/ieee80211.h:2431
```

## L1203 · `pub const IEEE80211_VHT_CAP_RXSTBC_1: u32 = 0x00000100; // ../../../../../include/linux/ieee80211.h:2441`

```
// ../../../../../include/linux/ieee80211.h:2441
```

## L1204 · `pub const IEEE80211_VHT_CAP_HTC_VHT: u32 = 0x00400000; // ../../../../../include/linux/ieee80211.h:2456`

```
// ../../../../../include/linux/ieee80211.h:2456
```

## L1205 · `pub const IEEE80211_VHT_CAP_MAX_A_MPDU_LENGTH_EXPONENT_MASK: u32 = 0x3800000; // ../../../../../include/linux/ieee80211.`

```
// ../../../../../include/linux/ieee80211.h:2458  (7 << IEEE80211_VHT_CAP_MAX_A_MPDU_LENGTH_EXPONENT_SHIFT)
```

## L1206 · `pub const IEEE80211_VHT_CAP_TXSTBC: u32 = 0x00000080; // ../../../../../include/linux/ieee80211.h:2440`

```
// ../../../../../include/linux/ieee80211.h:2440
```

## L1207 · `pub const IEEE80211_VHT_CAP_MU_BEAMFORMEE_CAPABLE: u32 = 0x00100000; // ../../../../../include/linux/ieee80211.h:2454`

```
// ../../../../../include/linux/ieee80211.h:2454
```

## L1208 · `pub const IEEE80211_VHT_CAP_SU_BEAMFORMEE_CAPABLE: u32 = 0x00001000; // ../../../../../include/linux/ieee80211.h:2448`

```
// ../../../../../include/linux/ieee80211.h:2448
```

## L1209 · `pub const IEEE80211_VHT_CAP_BEAMFORMEE_STS_SHIFT: u32 = 13; // ../../../../../include/linux/ieee80211.h:2449`

```
// ../../../../../include/linux/ieee80211.h:2449
```

## L1210 · `pub const IEEE80211_VHT_CAP_SU_BEAMFORMER_CAPABLE: u32 = 0x00000800; // ../../../../../include/linux/ieee80211.h:2447`

```
// ../../../../../include/linux/ieee80211.h:2447
```

## L1211 · `pub const IEEE80211_VHT_CAP_MU_BEAMFORMER_CAPABLE: u32 = 0x00080000; // ../../../../../include/linux/ieee80211.h:2453`

```
// ../../../../../include/linux/ieee80211.h:2453
```

## L1212 · `pub const IEEE80211_VHT_CAP_BEAMFORMEE_STS_MASK: u32 = 0xe000; // ../../../../../include/linux/ieee80211.h:2450  (7 << I`

```
// ../../../../../include/linux/ieee80211.h:2450  (7 << IEEE80211_VHT_CAP_BEAMFORMEE_STS_SHIFT)
```

## L1213 · `pub const IEEE80211_VHT_MCS_SUPPORT_0_9: u32 = 2; // ../../../../../include/linux/ieee80211.h:2107`

```
// ../../../../../include/linux/ieee80211.h:2107
```

## L1214 · `pub const IEEE80211_VHT_MCS_NOT_SUPPORTED: u32 = 3; // ../../../../../include/linux/ieee80211.h:2107`

```
// ../../../../../include/linux/ieee80211.h:2107
```

## L1215 · `pub const EFUSE_HW_CAP_PTCL_VHT: u32 = 3; // efuse.h:9`

```
// efuse.h:9
```

## L1217-1218 · `pub const CMD_SCAN: u8 = 0x01;`

```
// ── Stufe 6a: der Steuerkanal (docs/spec/WIFI_CLASS_ABI.md §4) ───
// Abwaerts (Manager -> Treiber)
```

## L1227 · `pub const EV_SCAN_AP: u8 = 0x81;`

```
// Aufwaerts (Treiber -> Manager)
```

## L1237 · `pub const LLC_SNAP_HDR: [u8; 6] = [0xaa, 0xaa, 0x03, 0x00, 0x00, 0x00];`

```
/// 802.2 LLC/SNAP-Kopf vor jedem 802.11-Datenrahmen (RFC 1042).
```

## L1240 · `pub const DOT11_FC_TYPE_DATA: u8 = 0x08;`

```
/// `fc[0] & 0x0c == 0x08` — Datenrahmen.
```

## L1242 · `pub const DOT11_FC_PROTECTED: u8 = 0x40;`

```
/// `fc[1]` — die Nutzlast ist verschluesselt.
```

## L1244 · `pub const DOT11_STYPE_QOS: u8 = 0x08;`

```
/// Subtyp-Bit: QoS-Daten, zwei Byte QoS-Control hinter dem Kopf.
```

## L1246 · `pub const DOT11_STYPE_NODATA: u8 = 0x04;`

```
/// Subtyp-Bit: Null und QoS-Null tragen KEINEN Rumpf.
```

## L1248-1252 · `pub const DOT11_FC_DEAUTH: u8 = 0xc0;`

```
/// 802.11 §9.2.4.1 — das ganze erste Byte: Protokollfassung 0, Typ
/// VERWALTUNG (00), Subtyp 12 bzw. 10. Ein Deauth ist deshalb GENAU
/// `0xc0` und nicht eine Maske: `rx_to_8023` filtert in seiner ersten
/// Zeile auf Daten, und ohne diese zwei Werte faellt ein Rauswurf
/// lautlos durch.
```

## L1255-1256 · `pub const LINK_DOWN_REQUESTED: u8 = 0;`

```
/// docs/spec/WIFI_CLASS_ABI.md §4b — `EV_LINK_DOWN` traegt einen Grund:
/// 0 = angefordert, 1 = Deauth, 2 = verloren.
```

## L1260 · `pub const RTW_SEC_CMD_REG: u32 = 0x670; // sec.h:8`

```
// sec.h:8
```

## L1261 · `pub const RTW_SEC_WRITE_REG: u32 = 0x674; // sec.h:9`

```
// sec.h:9
```

## L1262 · `pub const RTW_SEC_CAM_ENTRY_SHIFT: u32 = 3; // sec.h:13`

```
// sec.h:13
```

## L1263 · `pub const RTW_SEC_CMD_WRITE_ENABLE: u32 = 1 << 16; // sec.h:15`

```
// sec.h:15
```

## L1264 · `pub const RTW_SEC_CMD_POLLING: u32 = 1 << 31; // sec.h:17`

```
// sec.h:17
```

## L1265 · `pub const RTW_CAM_AES: u32 = 4; // main.h:713`

```
// main.h:713
```

## L1266 · `pub const RTW_CAM_TKIP: u32 = 2; // main.h:713`

```
// main.h:713
```

## L1267 · `pub const RTW_CAM_WEP40: u32 = 1; // main.h:713`

```
// main.h:713
```

## L1268 · `pub const RTW_CAM_WEP104: u32 = 5; // main.h:713`

```
// main.h:713
```

## L1269 · `pub const RTW_CAM_NONE: u32 = 0; // main.h:713`

```
// main.h:713
```

## L1270 · `pub const DESC_RATE11M: u32 = 0x03; // main.h:246`

```
// main.h:246
```

## L1271 · `pub const DESC_RATE54M: u32 = 0x0b; // main.h:246`

```
// main.h:246
```

## L1272 · `pub const DESC_RATEMCS7: u32 = 0x13; // main.h:246`

```
// main.h:246
```

## L1273 · `pub const DESC_RATEMCS15: u32 = 0x1b; // main.h:246`

```
// main.h:246
```

## L1275-1278 · `pub const RTW_WATCH_DOG_DELAY_MS: u64 = 2000; // main.h:30  HZ * 2`

```
// ── rtw_watch_dog_work: die laufende Haelfte (main.c:224-310) ────
// Linux fuehrt sie ALLE 2 SEKUNDEN, das ganze Leben einer Verbindung
// lang. Bis 0.26.0 gab es sie bei uns nicht — und drei ihrer Posten
// sind Sendeseite und haengen an der Temperatur.
```

## L1279 · `pub const RTW_WATCH_DOG_DELAY_MS: u64 = 2000; // main.h:30  HZ * 2`

```
// main.h:30  HZ * 2
```

## L1280 · `pub const RTW_TP_SHIFT: u32 = 18; // main.h:41  bytes/2s --> Mbps`

```
// main.h:41  bytes/2s --> Mbps
```

## L1281 · `pub const RTW_LPS_THRESHOLD: u32 = 50; // ps.h:8`

```
// ps.h:8
```

## L1282 · `pub const RTW_BUSY_TRAFFIC_THRESHOLD: u64 = 100; // main.c:241-244`

```
// main.c:241-244
```

## L1284 · `pub const DIG_PERF_FA_TH_LOW: u32 = 250; // phy.c:360`

```
// ── rtw_phy_dig (phy.c:373-437) ──────────────────────────────────
```

## L1285 · `pub const DIG_PERF_FA_TH_LOW: u32 = 250; // phy.c:360`

```
// phy.c:360
```

## L1286 · `pub const DIG_PERF_FA_TH_HIGH: u32 = 500; // phy.c:361`

```
// phy.c:361
```

## L1287 · `pub const DIG_PERF_FA_TH_EXTRA_HIGH: u32 = 750; // phy.c:362`

```
// phy.c:362
```

## L1288 · `pub const DIG_PERF_MAX: u32 = 0x5a; // phy.c:363`

```
// phy.c:363
```

## L1289 · `pub const DIG_PERF_MID: u32 = 0x40; // phy.c:364`

```
// phy.c:364
```

## L1290 · `pub const DIG_CVRG_FA_TH_LOW: u32 = 2000; // phy.c:365`

```
// phy.c:365
```

## L1291 · `pub const DIG_CVRG_FA_TH_HIGH: u32 = 4000; // phy.c:366`

```
// phy.c:366
```

## L1292 · `pub const DIG_CVRG_FA_TH_EXTRA_HIGH: u32 = 5000; // phy.c:367`

```
// phy.c:367
```

## L1293 · `pub const DIG_CVRG_MAX: u32 = 0x2a; // phy.c:368`

```
// phy.c:368
```

## L1294 · `pub const DIG_CVRG_MID: u32 = 0x26; // phy.c:369`

```
// phy.c:369
```

## L1295 · `pub const DIG_CVRG_MIN: u32 = 0x1c; // phy.c:370`

```
// phy.c:370
```

## L1296 · `pub const DIG_RSSI_GAIN_OFFSET: u32 = 15; // phy.c:371`

```
// phy.c:371
```

## L1297 · `pub const RTW8822C_DIG_MIN: u8 = 0x20; // rtw8822c.c:5355`

```
/// rtw8822c.c:5355 `.dig_min` — die Untergrenze DIESES Chips.
```

## L1298 · `pub const RTW8822C_DIG_MIN: u8 = 0x20; // rtw8822c.c:5355`

```
// rtw8822c.c:5355
```

## L1300 · `pub const RA_FLOOR_TABLE_SIZE: usize = 7; // phy.c:289`

```
// ── rtw_phy_get_rssi_level (phy.c:292-309) ───────────────────────
```

## L1301 · `pub const RA_FLOOR_TABLE_SIZE: usize = 7; // phy.c:289`

```
// phy.c:289
```

## L1302 · `pub const RA_FLOOR_UP_GAP: u8 = 3; // phy.c:290`

```
// phy.c:290
```

## L1304 · `pub const CCK_PD_FA_LV1_MIN: u32 = 1000; // phy.c:711`

```
// ── rtw_phy_cck_pd (phy.c:736-766) ───────────────────────────────
```

## L1305 · `pub const CCK_PD_FA_LV1_MIN: u32 = 1000; // phy.c:711`

```
// phy.c:711
```

## L1306 · `pub const CCK_PD_FA_LV0_MAX: u32 = 500; // phy.c:712`

```
// phy.c:712
```

## L1307 · `pub const CCK_PD_IGI_LV4_VAL: u8 = 0x38; // phy.c:728`

```
// phy.c:728
```

## L1308 · `pub const CCK_PD_IGI_LV3_VAL: u8 = 0x2a; // phy.c:729`

```
// phy.c:729
```

## L1309 · `pub const CCK_PD_IGI_LV2_VAL: u8 = 0x24; // phy.c:730`

```
// phy.c:730
```

## L1310 · `pub const CCK_PD_RSSI_LV4_VAL: u8 = 32; // phy.c:731`

```
// phy.c:731
```

## L1311 · `pub const CCK_PD_RSSI_LV3_VAL: u8 = 32; // phy.c:732`

```
// phy.c:732
```

## L1312 · `pub const CCK_PD_RSSI_LV2_VAL: u8 = 24; // phy.c:733`

```
// phy.c:733
```

## L1313 · `pub const CCK_FA_AVG_RESET: u32 = 0xffffffff; // phy.h:174`

```
// phy.h:174
```

## L1314 · `pub const CCK_PD_LV0: u8 = 0; // phy.h:164`

```
// phy.h:164
```

## L1315 · `pub const CCK_PD_LV1: u8 = 1; // phy.h:165`

```
// phy.h:165
```

## L1316 · `pub const CCK_PD_LV2: u8 = 2; // phy.h:166`

```
// phy.h:166
```

## L1317 · `pub const CCK_PD_LV3: u8 = 3; // phy.h:167`

```
// phy.h:167
```

## L1318 · `pub const CCK_PD_LV4: u8 = 4; // phy.h:168`

```
// phy.h:168
```

## L1319 · `pub const CCK_PD_LV_MAX: u8 = 5; // phy.h:169`

```
// phy.h:169
```

## L1320 · `pub const RTW_CCK_PD_MAX: u32 = 255; // rtw8822c.c:4342`

```
// rtw8822c.c:4342
```

## L1321 · `pub const RTW_CCK_CS_MAX: u32 = 31; // rtw8822c.c:4343`

```
// rtw8822c.c:4343
```

## L1322 · `pub const RTW_CCK_CS_ERR1: u32 = 27; // rtw8822c.c:4344`

```
// rtw8822c.c:4344
```

## L1323 · `pub const RTW_CCK_CS_ERR2: u32 = 29; // rtw8822c.c:4345`

```
// rtw8822c.c:4345
```

## L1325 · `pub const RRSR_RATE_ORDER_MAX: u32 = 0xfffff; // phy.h:180`

```
// ── rtw_phy_rrsr_update (phy.c:1062-1069) ────────────────────────
```

## L1326 · `pub const RRSR_RATE_ORDER_MAX: u32 = 0xfffff; // phy.h:180`

```
// phy.h:180
```

## L1327 · `pub const RRSR_RATE_ORDER_CCK_LEN: u32 = 4; // phy.h:181`

```
// phy.h:181
```

## L1329 · `pub const CFO_TRK_ENABLE_TH: i32 = 20; // rtw8822c.h:163`

```
// ── rtw8822c_cfo_track (rtw8822c.c:4265-4330) ────────────────────
```

## L1330 · `pub const CFO_TRK_ENABLE_TH: i32 = 20; // rtw8822c.h:163`

```
// rtw8822c.h:163
```

## L1331 · `pub const CFO_TRK_STOP_TH: i32 = 10; // rtw8822c.h:164`

```
// rtw8822c.h:164
```

## L1332 · `pub const CFO_TRK_ADJ_TH: i32 = 10; // rtw8822c.h:165`

```
// rtw8822c.h:165
```

## L1334 · `pub const RTW_PWR_TRK_TBL_SZ: usize = 30; // main.h:1127`

```
// ── rtw_phy_pwr_track (phy.c) / rtw8822c_pwr_track ───────────────
```

## L1335 · `pub const RTW_PWR_TRK_TBL_SZ: usize = 30; // main.h:1127`

```
// main.h:1127
```

## L1336 · `pub const RTW_PWR_TRK_5G_NUM: usize = 3; // main.h:1125`

```
// main.h:1125
```

## L1337 · `pub const PWR_TRACK_MASK: u32 = 0x7f; // rtw8822c.c:4413`

```
// rtw8822c.c:4413
```

## L1338 · `pub const RTW8822C_IQK_THRESHOLD: u8 = 8; // rtw8822c.c:5387`

```
/// rtw8822c.c:5387-5388 — beide 8.
```

## L1339 · `pub const RTW8822C_IQK_THRESHOLD: u8 = 8; // rtw8822c.c:5387`

```
// rtw8822c.c:5387
```

## L1340 · `pub const RTW8822C_LCK_THRESHOLD: u8 = 8; // rtw8822c.c:5388`

```
// rtw8822c.c:5388
```

## L1341 · `pub const RTW8822C_PATH_DIV_SUPPORTED: bool = true; // rtw8822c.c:5362`

```
/// rtw8822c.c:5362 `.path_div_supported = true`
```

## L1342 · `pub const RTW8822C_PATH_DIV_SUPPORTED: bool = true; // rtw8822c.c:5362`

```
// rtw8822c.c:5362
```

## L1343 · `pub const DESC_RATE_MAX: usize = 84; // main.h:246`

```
// main.h:246
```

## L1345 · `pub const DESC_RATE1M: u32 = 0x00; // main.h:246`

```
// ── rtw_phy_get_rrsr_mask (phy.c:1021-1049) ──────────────────────
```

## L1346 · `pub const DESC_RATE1M: u32 = 0x00; // main.h:246`

```
// main.h:246
```

## L1347 · `pub const DESC_RATEMCS0: u32 = 0x0c; // main.h:246`

```
// main.h:246
```

## L1348 · `pub const DESC_RATEMCS8: u32 = 0x14; // main.h:246`

```
// main.h:246
```

## L1349 · `pub const DESC_RATEMCS16: u32 = 0x1c; // main.h:246`

```
// main.h:246
```

## L1350 · `pub const DESC_RATEMCS24: u32 = 0x24; // main.h:246`

```
// main.h:246
```

## L1351 · `pub const DESC_RATEVHT1SS_MCS0: u32 = 0x2c; // main.h:246`

```
// main.h:246
```

## L1352 · `pub const DESC_RATEVHT2SS_MCS0: u32 = 0x36; // main.h:246`

```
// main.h:246
```

## L1353 · `pub const DESC_RATEVHT3SS_MCS0: u32 = 0x40; // main.h:246`

```
// main.h:246
```

## L1354 · `pub const DESC_RATEVHT4SS_MCS0: u32 = 0x4a; // main.h:246`

```
// main.h:246
```

## L1356 · `pub const BIT_XCAP_0: u32 = 0x00fffc00; // reg.h:776  GENMASK(23, 10)`

```
// ── rtw8822c_cfo_track / rtw8822c_do_lck (rtw8822c.c) ────────────
```

## L1357 · `pub const BIT_XCAP_0: u32 = 0x00fffc00; // reg.h:776  GENMASK(23, 10)`

```
// reg.h:776  GENMASK(23, 10)
```

## L1358 · `pub const RF_SYN_CTRL: u32 = 0xbb; // reg.h:958`

```
// reg.h:958
```

## L1359 · `pub const RF_SYN_PFD: u32 = 0xb0; // reg.h:955`

```
// reg.h:955
```

## L1360 · `pub const RF_SYN_AAC: u32 = 0xc9; // reg.h:960`

```
// reg.h:960
```

## L1361 · `pub const RF_AAC_CTRL: u32 = 0xca; // reg.h:961`

```
// reg.h:961
```

## L1362 · `pub const RF_FAST_LCK: u32 = 0xcc; // reg.h:962`

```
// reg.h:962
```

## L1364 · `pub const REG_BT_ACT_STATISTICS: u32 = 0x0770; // reg.h:571`

```
// ── rtw_coex_monitor_bt_ctr (coex.c:454-475) ─────────────────────
```

## L1365 · `pub const REG_BT_ACT_STATISTICS: u32 = 0x0770; // reg.h:571`

```
// reg.h:571
```

## L1366 · `pub const REG_BT_ACT_STATISTICS_1: u32 = 0x0774; // reg.h:572`

```
// reg.h:572
```

## L1367 · `pub const REG_BT_COEX_ENH_INTR_CTRL: u32 = 0x76E; // reg.h:568`

```
// reg.h:568
```

## L1368 · `pub const BIT_R_GRANTALL_WLMASK: u32 = 1 << 3; // reg.h:569`

```
// reg.h:569
```

## L1369 · `pub const BIT_STATIS_BT_EN: u32 = 1 << 2; // reg.h:570`

```
// reg.h:570
```

## L1371 · `pub const H2C_CMD_RSSI_MONITOR: u32 = 0x42; // fw.h:559`

```
// ── rtw_phy_ra_track / rtw_fw_adaptivity (fw.c) ──────────────────
```

## L1372 · `pub const H2C_CMD_RSSI_MONITOR: u32 = 0x42; // fw.h:559`

```
// fw.h:559
```

## L1373 · `pub const H2C_CMD_WL_PHY_INFO: u32 = 0x58; // fw.h:563`

```
// fw.h:563
```

## L1374 · `pub const H2C_CMD_ADAPTIVITY: u32 = 0x5A; // fw.h:565`

```
// fw.h:565
```

## L1376 · `pub const EDCCA_TH_L2H_LB: i8 = 48; // main.h:1663`

```
// ── rtw8822c_adaptivity (rtw8822c.c:2178-2196) ───────────────────
```

## L1377 · `pub const EDCCA_TH_L2H_LB: i8 = 48; // main.h:1663`

```
// main.h:1663
```

## L1378 · `pub const EDCCA_ADC_BACKOFF: i8 = 12; // main.h:1664`

```
// main.h:1664
```

## L1379 · `pub const EDCCA_L2H_H2L_DIFF: i8 = 7; // main.h:1667`

```
// main.h:1667
```

## L1380 · `pub const EDCCA_L2H_H2L_DIFF_NORMAL: i8 = 8; // main.h:1668`

```
// main.h:1668
```

## L1381 · `pub const EDCCA_IGI_L2H_DIFF: i8 = 8; // main.h:1666`

```
// main.h:1666
```

## L1382 · `pub const FW_FEATURE_BCN_FILTER: u32 = 1 << 5; // fw.h:144`

```
// fw.h:144
```

## L1383 · `pub const FW_FEATURE_ADAPTIVITY: u32 = 1 << 7; // fw.h:144`

```
// fw.h:144
```

## L1385-1386 · `pub const C2H_RA_REPORT_SIZE: usize = 7; // rtw8822c.c:5359`

```
// ── rtw_fw_ra_report_handle (fw.c:265-323) ───────────────────────
/// rtw8822c.c:5359 `.c2h_ra_report_size = 7`
```

## L1387 · `pub const C2H_RA_REPORT_SIZE: usize = 7; // rtw8822c.c:5359`

```
// rtw8822c.c:5359
```

## L1389-1390 · `pub const RTW_TX_PROBE_TIMEOUT_MS: u64 = 500; // tx.h:10`

```
// ── TX-Report (tx.c:166-260) und Firmware-Absturz (fw.c:383-390) ──
/// tx.h:10 `RTW_TX_PROBE_TIMEOUT msecs_to_jiffies(500)`
```

## L1391 · `pub const RTW_TX_PROBE_TIMEOUT_MS: u64 = 500; // tx.h:10`

```
// tx.h:10
```

## L1392 · `pub const RTW_TX_DESC_W2_SPE_RPT: u32 = 1 << 19; // tx.h:37`

```
/// tx.h:37 `RTW_TX_DESC_W2_SPE_RPT BIT(19)`
```

## L1393 · `pub const RTW_TX_DESC_W2_SPE_RPT: u32 = 1 << 19; // tx.h:37`

```
// tx.h:37
```

## L1394 · `pub const RTW_TX_DESC_W6_SW_DEFINE: u32 = 0x0000_0fff; // tx.h:54`

```
/// tx.h:54 `RTW_TX_DESC_W6_SW_DEFINE GENMASK(11, 0)`
```

## L1395 · `pub const RTW_TX_DESC_W6_SW_DEFINE: u32 = 0x0000_0fff; // tx.h:54`

```
// tx.h:54
```

## L1396 · `pub const CCX_REPORT_V0_SEQNUM_OFF: usize = 6; // fw.h:370`

```
/// fw.h:370-371 `GET_CCX_REPORT_SEQNUM_V0` / `_STATUS_V0`
```

## L1397 · `pub const CCX_REPORT_V0_SEQNUM_OFF: usize = 6; // fw.h:370`

```
// fw.h:370
```

## L1398 · `pub const CCX_REPORT_V0_SEQNUM_MASK: u8 = 0xfc; // fw.h:370`

```
// fw.h:370
```

## L1399 · `pub const CCX_REPORT_V0_STATUS_OFF: usize = 0; // fw.h:371`

```
// fw.h:371
```

## L1400 · `pub const CCX_REPORT_V0_STATUS_MASK: u8 = 0xc0; // fw.h:371`

```
// fw.h:371
```

## L1401 · `pub const REG_MCU_TST_CFG: u32 = 0x84; // reg.h:157`

```
// reg.h:157
```

## L1402 · `pub const VAL_FW_TRIGGER: u32 = 0x1; // reg.h:158`

```
// reg.h:158
```

## L1404-1406 · `pub const C2H_CCX_RPT: u32 = 0x0f; // fw.h:67`

```
/// fw.h:67 `C2H_CCX_RPT` — die Sendequittung als UNTERkommando von
/// `C2H_HALMAC`, mit der V1-Aufteilung. rtw88 hat ZWEI Wege dafuer, und
/// welchen eine Firmware nimmt, sagt nur der Geraetelauf.
```

## L1407 · `pub const C2H_CCX_RPT: u32 = 0x0f; // fw.h:67`

```
// fw.h:67
```

## L1408 · `pub const CCX_REPORT_V1_SEQNUM_OFF: usize = 8; // fw.h:372`

```
// fw.h:372
```

## L1409 · `pub const CCX_REPORT_V1_STATUS_OFF: usize = 9; // fw.h:373`

```
// fw.h:373
```

## L1411-1412 · `pub const DOT11_FC_TYPE_MGMT: u8 = 0x00;`

```
// ── Der Zensus der Verwaltungsrahmen ─────────────────────────────
// 802.11 §9.2.4.1: Typ 00 = Verwaltung, der Subtyp steht in Bit 7:4.
```

## L1415-1418 · `pub const DOT11_ACTION_CAT_BA: u8 = 3;`

```
/// §9.4.1.11 Kategorie 3 = Block Ack, Aktion 0 = ADDBA Request.
/// **Das ist der Rahmen, mit dem ein AP eine Aggregation ERBITTET** —
/// und in rtw88 beantwortet ihn mac80211, nicht der Treiber
/// (`IEEE80211_AMPDU_RX_START` ist dort ein leeres `break`).
```

## L1424-1425 · `pub const ADDBA_PARAM_AMSDU_MASK: u16 = 0x0001; // ieee80211.h:2032`

```
// ── ADDBA (802.11 §9.6.7.2-3, include/linux/ieee80211.h) ─────────
/// `IEEE80211_ADDBA_PARAM_*_MASK`, ieee80211.h:2032-2035
```

## L1426 · `pub const ADDBA_PARAM_AMSDU_MASK: u16 = 0x0001; // ieee80211.h:2032`

```
// ieee80211.h:2032
```

## L1427 · `pub const ADDBA_PARAM_POLICY_MASK: u16 = 0x0002; // ieee80211.h:2033`

```
// ieee80211.h:2033
```

## L1428 · `pub const ADDBA_PARAM_TID_MASK: u16 = 0x003C; // ieee80211.h:2034`

```
// ieee80211.h:2034
```

## L1429 · `pub const ADDBA_PARAM_BUF_SIZE_MASK: u16 = 0xFFC0; // ieee80211.h:2035`

```
// ieee80211.h:2035
```

## L1430 · `pub const WLAN_STATUS_SUCCESS: u16 = 0; // ieee80211.h:3536`

```
/// `WLAN_STATUS_SUCCESS`, ieee80211.h:3536
```

## L1431 · `pub const WLAN_STATUS_SUCCESS: u16 = 0; // ieee80211.h:3536`

```
// ieee80211.h:3536
```

## L1432-1433 · `pub const DOT11_FC_ACTION: u8 = 0xd0;`

```
/// Der Verwaltungsrahmen-Subtyp 13 (Action) im ersten Byte:
/// Protokollfassung 0, Typ 00, Subtyp 1101.
```

