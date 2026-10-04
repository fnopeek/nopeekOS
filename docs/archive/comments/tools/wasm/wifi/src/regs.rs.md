# `tools/wasm/wifi/src/regs.rs` @ 5e0102684

## L1 · `pub const R_AX_SYS_ISO_CTRL: u32      = 0x0000;`

```
//! RTL8852BE register definitions (from rtw89 Linux driver)
```

## L3 · `pub const R_AX_SYS_ISO_CTRL: u32      = 0x0000;`

```
// ── System / Power ───────────────────────────────────────────────
```

## L26 · `pub const R_AX_HALT_H2C_CTRL: u32     = 0x0160;`

```
// ── Firmware / CPU Control ───────────────────────────────────────
```

## L34 · `pub const RTW89_FW_DLFW_RESUME: u32   = 3; // firmware download boot reason`

```
// firmware download boot reason
```

## L36 · `pub const R_AX_UDM1: u32              = 0x01F4;`

```
// Additional registers from rtw89_mac_enable_cpu_ax
```

## L39-43 · `pub const R_AX_HALMAC_CNT_BYTE: u32   = 0x01F5;`

```
// H2C/C2H counter byte at R_AX_UDM1+1 (= 0x01F5):
//   [3:0] = HALMAC_H2C_DEQ_CNT   (GENMASK(11,8) in the 32-bit UDM1 reg)
//   [7:4] = HALMAC_C2H_ENQ_CNT   (GENMASK(15,12))
// Linux updates the counter on every h2creg / c2hreg transaction (chip info
// rtw8852b.c:1031 .h2c_counter_reg / .c2h_counter_reg).
```

## L48-49 · `pub const R_AX_H2CREG_DATA0: u32      = 0x8140;`

```
// H2CREG / C2HREG — fast register-based H2C channel to FW (used by
// sch_tx_en, get_feature, etc.; not the CH12 DMA H2C).
```

## L56-57 · `pub const R_AX_CTN_TXEN: u32          = 0xC348;`

```
// CMAC TX enable register — written by rtw89_mac_stop_sch_tx /
// rtw89_mac_resume_sch_tx via H2CREG SCH_TX_EN when FW is ready.
```

## L61-64 · `pub const R_AX_HIMR0:        u32 = 0x01A0;`

```
// PCIe interrupt mask registers — rtw89_pci_enable_intr (pci.c:853).
// Writing the IMRs unmasks the matching IRQ sources; required as part of
// rtw89_hci_start / rtw89_pci_ops_start (pci.c:1922) at the end of
// rtw89_core_start. Without this, the RX side may remain gated.
```

## L72 · `pub const B_AX_HALT_C2H_INT_EN:     u32 = 1 << 21;`

```
// HIMR0 bits (pci.h:160)
```

## L75 · `pub const B_AX_HS0ISR_IND_INT_EN:   u32 = 1 << 24;`

```
// PCIE_HIMR00 bits (pci.h:180)
```

## L85 · `pub const B_AX_HC10ISR_IND_INT_EN:  u32 = 1 << 28;`

```
// PCIE_HIMR10 bits
```

## L89 · `pub const B_AX_BOOT_REASON_MASK: u32  = 0x7; // bits [2:0] at offset 0x01E6`

```
// bits [2:0] at offset 0x01E6
```

## L91 · `pub const R_AX_PCIE_INIT_CFG1: u32    = 0x1000;`

```
// ── PCIe / DMA ───────────────────────────────────────────────────
```

## L98-102 · `pub const R_AX_ACH0_TXBD_NUM: u32     = 0x1024;`

```
// TX BD ring addresses — from Linux rtw89_pci_ch_dma_addr_set
// Format per channel: NUM, IDX, BDRAM_CTRL, DESA_L, DESA_H
//
// 8852B unmasked TX channels: ACH0-ACH3, CH8, CH9, CH12
// (ACH4-ACH7, CH10, CH11 are masked via tx_dma_ch_mask)
```

## L123 · `pub const R_AX_CH12_TXBD_NUM: u32     = 0x1038;`

```
// CH12 = FWCMD queue — correct addresses from Linux!
```

## L130 · `pub const R_AX_RXQ_RXBD_NUM: u32      = 0x1020;`

```
// RX BD ring addresses (Linux rtw89 reg.h + rtw89_pci_ch_dma_addr_set)
```

## L140 · `pub const R_AX_PCIE_INIT_CFG2: u32     = 0x1004;`

```
// ── PCIe Configuration ──────────────────────────────────────────
```

## L144 · `pub const B_AX_MAX_TAG_NUM_MASK: u32   = 0x7 << 16; // GENMASK(18,16)`

```
// GENMASK(18,16)
```

## L148 · `pub const R_AX_LTR_DEC_CTRL: u32      = 0x1600;`

```
// ── LTR / Power Management ──────────────────────────────────────
```

## L154 · `pub const R_AX_HCI_FUNC_EN: u32       = 0x8380;`

```
// ── HCI / DMAC / CMAC Function Enable ────────────────────────────
```

## L166 · `pub const R_AX_WDE_PKTBUF_CFG: u32    = 0x8C08;`

```
// Memory management
```

## L172 · `pub const R_AX_CMAC_FUNC_EN: u32      = 0xC000;`

```
// CMAC
```

## L176 · `pub const B_AX_APFN_ONMAC: u32       = 1 << 8;`

```
// ── Register Bit Definitions ─────────────────────────────────────
```

## L178 · `pub const B_AX_APFN_ONMAC: u32       = 1 << 8;`

```
// R_AX_SYS_PW_CTRL (0x0004) bits
```

## L189 · `pub const B_AX_ISO_EB2CORE: u32      = 1 << 8;`

```
// R_AX_SYS_ISO_CTRL (0x0000) bits
```

## L194 · `pub const B_AX_FEN_BBRSTB: u8        = 1 << 0;`

```
// R_AX_SYS_FUNC_EN (0x0002) bits (byte access)
```

## L198 · `pub const B_AX_DIS_WLBT_LPSEN_LOPC: u32 = 1 << 1;`

```
// R_AX_WLLPS_CTRL (0x0090) bits
```

## L201 · `pub const B_AX_AON_OFF_PC_EN: u32    = 1 << 23;`

```
// R_AX_SYS_AFE_LDO_CTRL (0x0020) bits
```

## L204 · `pub const B_AX_SYM_PADPDN_WL_RFC_1P3: u32 = 1 << 5;`

```
// R_AX_SYS_ADIE_PAD_PWR_CTRL (0x0018) bits
```

## L208 · `pub const B_AX_SYSON_DIS_PMCR_AX_WRMSK: u32 = 1 << 2;`

```
// R_AX_PMC_DBG_CTRL2 (0x00CC) bits
```

## L211 · `pub const B_AX_PCIE_CALIB_EN_V1: u32 = 1 << 12;`

```
// R_AX_SYS_SDIO_CTRL (0x0070) bits
```

## L214 · `pub const B_AX_AFC_AFEDIG: u32       = 1 << 17;`

```
// R_AX_WLRF_CTRL (0x02F0) bits
```

## L217 · `pub const B_AX_SYM_CTRL_SPS_PWMFREQ: u32 = 1 << 10;`

```
// R_AX_SYS_SWR_CTRL1 (0x0010) bits
```

## L220 · `pub const B_AX_C1_L1_MASK: u32       = 0x3;       // GENMASK(1,0)`

```
// R_AX_SPS_DIG_OFF_CTRL0 (0x0400) field masks
```

## L221 · `pub const B_AX_C1_L1_MASK: u32       = 0x3;       // GENMASK(1,0)`

```
// GENMASK(1,0)
```

## L222 · `pub const B_AX_C3_L1_MASK: u32       = 0x30;      // GENMASK(5,4)`

```
// GENMASK(5,4)
```

## L224 · `pub const B_AX_REG_ZCDC_H_MASK: u32  = 0x3 << 17; // GENMASK(18,17)`

```
// R_AX_SPS_DIG_ON_CTRL0 (0x0200) field masks
```

## L225 · `pub const B_AX_REG_ZCDC_H_MASK: u32  = 0x3 << 17; // GENMASK(18,17)`

```
// GENMASK(18,17)
```

## L227 · `pub const B_AX_PINMUX_EESK_FUNC_SEL_MASK: u32 = 0xF0; // GENMASK(7,4)`

```
// R_AX_EECS_EESK_FUNC_SEL (0x02D8) field masks
```

## L228 · `pub const B_AX_PINMUX_EESK_FUNC_SEL_MASK: u32 = 0xF0; // GENMASK(7,4)`

```
// GENMASK(7,4)
```

## L230 · `pub const SW_LPS_OPTION: u32         = 0x0001A0B2;`

```
// Power-off constants
```

## L233 · `pub const B_AX_PLATFORM_EN: u32  = 1 << 0;`

```
// R_AX_PLATFORM_ENABLE (0x0088) bits
```

## L236 · `pub const B_AX_APB_WRAP_EN: u32  = 1 << 2;  // firmware watchdog control`

```
// firmware watchdog control
```

## L240 · `pub const B_AX_WCPU_FWDL_EN: u32    = 1 << 0;`

```
// R_AX_WCPU_FW_CTRL (0x01E0) bits
```

## L245 · `pub const B_AX_MAC_FUNC_EN: u32     = 1 << 30;`

```
// R_AX_DMAC_FUNC_EN (0x8400) bits — full set from rtw8852b_pwr_on_func
```

## L263 · `pub const B_AX_DMAC_CRPRT: u32      = 1 << 31;`

```
// R_AX_DMAC_FUNC_EN extra (Linux has this, we missed it)
```

## L266-268 · `pub const B_AX_WD_RLS_CLK_EN: u32      = 1 << 27;`

```
// R_AX_DMAC_CLK_EN (0x8404) bits — Linux dmac_func_en_ax writes these
// to enable clocks for all DMAC sub-blocks. Without these the DMAC
// subsystem runs without clocks → RX/TX DMA dead.
```

## L278 · `pub const B_AX_CMAC_CRPRT: u32      = 1 << 31;`

```
// R_AX_CMAC_FUNC_EN (0xC000) bits
```

## L291-292 · `pub const B_AX_CMAC_CKEN: u32       = 1 << 30;`

```
// R_AX_CK_EN (0xC004) bits — CMAC sub-block clocks. Without these the
// CMAC RX pipe is dead: RMAC, PHYINTF, CMAC_DMA all gate receive.
```

## L301 · `pub const FWDL_WCPU_FW_INIT_RDY: u32  = 1 << 0;`

```
// ── Firmware Download Status Bits ────────────────────────────────
```

## L308 · `pub const R_AX_PCIE_DMA_STOP1: u32    = 0x1010;`

```
// ── PCIe DMA Control ─────────────────────────────────────────────
```

## L321 · `pub const B_AX_CLR_ACH0_IDX: u32  = 1 << 0;`

```
// Bits in R_AX_TXBD_RWPTR_CLR1: set to clear corresponding ring index
```

## L329 · `pub const B_AX_CLR_ALL_CH: u32    = 0x7FF; // bits [10:0]`

```
// bits [10:0]
```

## L331 · `pub const B_AX_TXHCI_EN: u32     = 1 << 11;`

```
// R_AX_PCIE_INIT_CFG1 (0x1000) DMA control bits
```

## L336-341 · `pub const R_AX_CH8_TXBD_NUM: u32    = 0x1034; // 16-bit: ring size`

```
// ── CH8 TX Ring (MGMT Band 0) — ring size/idx/dma-high ──────────
//
// pci.h rtw89_pci_ch_dma_addr_set (non-V1, for 8852BE single-band).
// CH8 = RTW89_TXCH_CH8 = MGMT Band 0. QSEL = RTW89_TX_QSEL_B0_MGMT = 0x12.
// BDRAM single-band layout: start=20 max=4 min=1 (pci.c:1716).
// Base addrs (DESA_L, BDRAM_CTRL) are defined above with ACH0..3/CH9.
```

## L342 · `pub const R_AX_CH8_TXBD_NUM: u32    = 0x1034; // 16-bit: ring size`

```
// 16-bit: ring size
```

## L343 · `pub const R_AX_CH8_TXBD_IDX: u32    = 0x1078; // 16-bit: wp write / rp read`

```
// 16-bit: wp write / rp read
```

## L345 · `pub const B_AX_CH8_BUSY: u32        = 1 << 16; // R_AX_PCIE_DMA_BUSY1`

```
// R_AX_PCIE_DMA_BUSY1
```

## L347 · `pub const RTL8852B_VENDOR: u16 = 0x10EC;`

```
// ── Chip Constants ───────────────────────────────────────────────
```

## L352-358 · `pub const XTAL_SI_ANAPAR_WL: u8   = 0x90;`

```
// ── XTAL SI Indirect Access ─────────────────────────────────────
// Written via R_AX_WLAN_XTAL_SI_CTRL (0x0270):
//   BIT(31) = CMD_POLL (set to trigger, clears when done)
//   [25:24] = mode (0=write, 1=read)
//   [23:16] = bitmask
//   [15:8]  = data
//   [7:0]   = address (offset)
```

## L360 · `pub const XTAL_SI_ANAPAR_WL: u8   = 0x90;`

```
// XTAL SI register offsets
```

## L368 · `pub const XTAL_SI_PON_WEI: u8     = 1 << 0;`

```
// XTAL SI bit masks for ANAPAR_WL (offset 0x90)
```

## L378 · `pub const XTAL_SI_SRAM_DIS: u8    = 1 << 1;  // SRAM_CTRL (0xA1)`

```
// XTAL SI bit masks for other offsets
```

## L379 · `pub const XTAL_SI_SRAM_DIS: u8    = 1 << 1;  // SRAM_CTRL (0xA1)`

```
// SRAM_CTRL (0xA1)
```

## L380 · `pub const XTAL_SI_RF00: u8        = 1 << 0;  // WL_RFC_S0 (0x80)`

```
// WL_RFC_S0 (0x80)
```

## L381 · `pub const XTAL_SI_RF10: u8        = 1 << 0;  // WL_RFC_S1 (0x81)`

```
// WL_RFC_S1 (0x81)
```

## L382 · `pub const XTAL_SI_LDO_LPS: u8     = 0x70;    // XTAL_XMD_2 GENMASK(6,4)`

```
// XTAL_XMD_2 GENMASK(6,4)
```

## L383 · `pub const XTAL_SI_LPS_CAP: u8     = 0x0F;    // XTAL_XMD_4 GENMASK(3,0)`

```
// XTAL_XMD_4 GENMASK(3,0)
```

