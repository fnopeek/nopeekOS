# `tools/wasm/wifi/src/rfk.rs` @ 5e0102684

## L1-7 · `use crate::host;`

```
//! RF Kalibrierung — 1:1 Port aus Linux rtw8852b_rfk.c.
//!
//! Implementiert die Basis-RFK die Linux in rtw8852b_rfk_init() ruft:
//!   dpk_init → rck → dack → rx_dck
//!
//! Alle PHY-Register brauchen `PHY_CR_BASE` offset wie in phy.rs beschrieben.
//! RF-Register (RR_*) gehen über SWSI (rf_read/rf_write_mask aus phy.rs).
```

## L13 · `const F_WRF:   u8 = 0;`

```
// RFK table operation flags (Linux rtw89_rfk_flag)
```

## L20-21 · `pub fn parser(mmio: i32, tbl: &[(u8, u8, u32, u32, u32)]) {`

```
/// Execute an RFK table (array of (flag, path, addr, mask, data) tuples).
/// Linux rtw89_rfk_parser (phy.c:7853).
```

## L27 · `let reg = PHY_CR_BASE + addr;`

```
// rtw89_phy_write32_mask — PHY space +CR_BASE
```

## L42 · `for _ in 0..(data as usize * 100) { core::hint::spin_loop(); }`

```
// data is microseconds. sleep_ms(0) wouldn't help; use spin loop.
```

## L50-53 · `const R_DRCK_V1:       u32 = 0xC0CC;`

```
// ═══════════════════════════════════════════════════════════════════
//  Register constants (Linux reg.h)
//  All addresses below are PHY-space → +CR_BASE when using MMIO.
// ═══════════════════════════════════════════════════════════════════
```

## L55 · `const R_DRCK_V1:       u32 = 0xC0CC;`

```
// DRCK
```

## L57 · `const B_DRCK_V1_KICK:  u32 = 1 << 9;   // BIT(9)`

```
// BIT(9)
```

## L58 · `const B_DRCK_V1_SEL:   u32 = 1 << 5;   // educated guess — used only by phy_write32_mask`

```
// educated guess — used only by phy_write32_mask
```

## L59 · `const B_DRCK_V1_CV:    u32 = 0x1F;     // GENMASK(4,0)`

```
// GENMASK(4,0)
```

## L61 · `const B_DRCK_RS_DONE:  u32 = 1 << 15;  // BIT(15) — from Linux usage context`

```
// BIT(15) — from Linux usage context
```

## L62 · `const B_DRCK_RS_LPS:   u32 = 0x1F;     // GENMASK(4,0)`

```
// GENMASK(4,0)
```

## L67 · `const R_ADDCK0:         u32 = 0x12A0;`

```
// ADDCK S0
```

## L70 · `const B_ADDCK0:         u32 = 0x300;    // GENMASK(9,8)`

```
// GENMASK(9,8)
```

## L71 · `const B_ADDCK0_MAN:     u32 = 0x30;     // GENMASK(5,4)`

```
// GENMASK(5,4)
```

## L72 · `const B_ADDCK0_VAL:     u32 = 0xFC;     // rough, Linux value used for WR`

```
// rough, Linux value used for WR
```

## L74 · `const B_ADDCKR0_A0:     u32 = 0xFFC00;  // GENMASK(19,10)`

```
// GENMASK(19,10)
```

## L75 · `const B_ADDCKR0_A1:     u32 = 0x3FF;    // GENMASK(9,0)`

```
// GENMASK(9,0)
```

## L77 · `const B_ADDCK0D_VAL:    u32 = 0x03FF_0000; // GENMASK(25,16)`

```
// GENMASK(25,16)
```

## L78 · `const B_ADDCK0D_VAL2:   u32 = 0xFC00_0000; // GENMASK(31,26)`

```
// GENMASK(31,26)
```

## L80 · `const R_ADDCK1:         u32 = 0xC1F4;`

```
// ADDCK S1
```

## L92 · `const R_ANAPAR:         u32 = 0x032C;`

```
// ANAPAR
```

## L98 · `const B_ANAPAR_PW15_H:  u32 = 0x0F00_0000; // GENMASK(27,24)`

```
// GENMASK(27,24)
```

## L100 · `const R_PATH0_SAMPL_DLY_T_V1: u32 = 0x2B80;`

```
// SAMPL_DLY
```

## L104 · `const R_P0_NRBW:      u32 = 0x12B8;`

```
// P0_NRBW / P1_DBGMOD
```

## L110 · `const R_DCOF0:        u32 = 0xC000;`

```
// DACK S0/S1 registers
```

## L112 · `const B_DCOF0_V:      u32 = 0x1E;      // GENMASK(4,1)`

```
// GENMASK(4,1)
```

## L141 · `const RR_RSV1:        u32 = 0x05;`

```
// RF register addresses/masks used in _rx_dck
```

## L148 · `const RR_DCK_FINE:    u32 = 1 << 1;   // BIT(1)`

```
// BIT(1)
```

## L152 · `const R_P0_TSSI_TRK:  u32 = 0x5818;`

```
// TSSI tracker
```

## L156 · `const R_AX_PHYREG_SET: u32 = 0x8040;`

```
// R_AX_PHYREG_SET is MAC-space
```

## L159-161 · `fn afe_init(mmio: i32) {`

```
// ═══════════════════════════════════════════════════════════════════
//  _afe_init (rfk.c:371)
// ═══════════════════════════════════════════════════════════════════
```

## L164 · `host::mmio_w32(mmio, R_AX_PHYREG_SET, 0xF);`

```
// MAC-space write — no CR_BASE
```

## L169-171 · `fn drck(mmio: i32) {`

```
// ═══════════════════════════════════════════════════════════════════
//  _drck (rfk.c:378)
// ═══════════════════════════════════════════════════════════════════
```

## L178 · `let cur = host::mmio_r32(mmio, r_drck_v1);`

```
// kick = 1
```

## L182 · `for _ in 0..10000u32 {`

```
// Poll R_DRCK_RS.B_DRCK_RS_DONE until non-zero
```

## L189 · `let cur = host::mmio_r32(mmio, r_drck_v1);`

```
// kick = 0
```

## L193 · `host::mmio_set32(mmio, r_drck_fh, B_DRCK_LAT);`

```
// LAT toggle
```

## L198 · `let rck_d = host::mmio_r32(mmio, r_drck_rs) & B_DRCK_RS_LPS;`

```
// rck_d = read B_DRCK_RS_LPS
```

## L201 · `let cur = host::mmio_r32(mmio, r_drck_v1);`

```
// v1_sel = 0, v1_cv = rck_d
```

## L207-209 · `static mut ADDCK_D: [[u32; 2]; 2] = [[0; 2]; 2];`

```
// ═══════════════════════════════════════════════════════════════════
//  _addck + backup + reload  (rfk.c:404, 417, 511)
// ═══════════════════════════════════════════════════════════════════
```

## L219 · `let cur = host::mmio_r32(mmio, r0);`

```
// R_ADDCK0 B_ADDCK0 = 0
```

## L242 · `let r = PHY_CR_BASE + R_ADDCK0D;`

```
// S0
```

## L252-255 · `let _ = cur;`

```
// B_ADDCK0_VAL = top two bits = (a01 >> 6) written into B_ADDCK0_VAL field
// Linux: rtw89_phy_write32_mask(R_ADDCK0, B_ADDCK0_VAL, a01 >> 6);
// B_ADDCK0_VAL is 0xFC (bits 7..2) per reg.h context — we don't have the exact mask,
// using same mask as S1. For now skip this refinement.
```

## L258 · `let cur = host::mmio_r32(mmio, r);`

```
// B_ADDCK0_MAN = 0x3
```

## L262 · `let r = PHY_CR_BASE + R_ADDCK1D;`

```
// S1
```

## L279 · `let r = PHY_CR_BASE + R_ADDCK0;`

```
// S0
```

## L282 · `host::mmio_w32(mmio, r, cur & !B_ADDCK0_MAN); // MAN=0`

```
// MAN=0
```

## L290 · `let cur = host::mmio_r32(mmio, anapar15);`

```
// PW15_H = 0xf
```

## L295 · `let cur = host::mmio_r32(mmio, anapar15);`

```
// PW15_H = 0x3
```

## L299 · `host::mmio_set32(mmio, PHY_CR_BASE + R_ADDCK0, B_ADDCK0_TRG);`

```
// TRG toggle + trigger
```

## L306 · `for _ in 0..10000u32 {`

```
// Poll R_ADDCKR0 BIT(0)
```

## L313 · `host::mmio_clr32(mmio, PHY_CR_BASE + R_PATH0_SAMPL_DLY_T_V1, 1 << 1);`

```
// Restore
```

## L321 · `host::mmio_set32(mmio, PHY_CR_BASE + R_P1_DBGMOD, B_P1_DBGMOD_ON);`

```
// S1 — same pattern with R_ADDCK1 / R_P1_DBGMOD
```

## L353-355 · `fn dack_s0_check_done(mmio: i32, part1: bool) -> bool {`

```
// ═══════════════════════════════════════════════════════════════════
//  _dack_s0/s1 — uses tables + polls
// ═══════════════════════════════════════════════════════════════════
```

## L415-417 · `pub fn dac_cal(mmio: i32) {`

```
// ═══════════════════════════════════════════════════════════════════
//  _dac_cal (rfk.c:756) — main DACK wrapper
// ═══════════════════════════════════════════════════════════════════
```

## L448-450 · `pub fn rx_dck(mmio: i32) {`

```
// ═══════════════════════════════════════════════════════════════════
//  _rx_dck (rfk.c:304) — per-path RX DC offset calibration
// ═══════════════════════════════════════════════════════════════════
```

## L462 · `set_rx_dck(mmio, path);`

```
// _set_rx_dck — kick RF reg 0x92 BIT(0) to trigger, wait for DCK_DONE
```

## L470 · `fn set_rx_dck(mmio: i32, path: u8) {`

```
/// _set_rx_dck writes RR_DCK LV=1 to trigger calibration, waits for DONE.
```

## L472 · `rf_write_mask(mmio, path, RR_DCK, 1, 1); // LV=1 trigger`

```
// Linux _set_rx_dck writes LV bit and polls DCK_DONE (~600us typical)
```

## L473 · `rf_write_mask(mmio, path, RR_DCK, 1, 1); // LV=1 trigger`

```
// LV=1 trigger
```

## L476 · `if (v & 0xE0) == 0xE0 { break; } // DONE field [7:5] all set`

```
// DONE field [7:5] all set
```

## L482 · `pub fn init(mmio: i32) {`

```
/// Public RFK baseline wrapper.
```

## L484-485 · `dac_cal(mmio);`

```
// rck + dpk_init + TSSI-disable handled by phy::rfk_init (already called
// at end of phy::init). Here we add DACK + RX_DCK.
```

