# `tools/wasm/wifi/src/efuse.rs` @ 5e0102684

## L1-27 · `use crate::host;`

```
//! Efuse (OTP) parser — 1:1 port of Linux rtw89_parse_efuse_map_ax +
//! rtw8852bx_efuse parsing.
//!
//! Efuse = one-time-programmable ROM on the chip. Holds per-chip
//! calibration data that HW defaults cannot supply:
//!   - path-A/B thermal meters (needed by TSSI _tssi_set_tmeter_tbl)
//!   - TSSI CCK/MCS per-channel offsets (needed by TSSI set_efuse_to_de)
//!   - RX gain offsets per path/band (needed by set_gain_offset)
//!   - RFE type (external PA presence → DPK bypass decision)
//!   - chip MAC address (replaces our pseudo 00:11:22:33:44:55)
//!   - country code, crystal cap, coex type, etc.
//!
//! Without efuse, TSSI runs with thermal=0xff fallback (zero thermal
//! offset table), set_txpwr uses uniform hardcoded dBm, and we send
//! Probe Requests with a pseudo MAC. All three of those are broken.
//!
//! Read path (8852BE is AX, non-DAV): direct MMIO via R_AX_EFUSE_CTRL.
//!   1. enable_efuse_pwr_cut_ddv (SYS_ISO_CTRL bit sequence, +1ms)
//!   2. for each byte in physical range:
//!        write R_AX_EFUSE_CTRL = addr << 16 (clears RDY)
//!        poll until bit 29 (RDY) = 1
//!        read data from low 8 bits
//!   3. disable_efuse_pwr_cut_ddv
//!
//! Decode: physical map is sparse (block header + 4 word-enable bits
//! per 2-byte word); decode_logical expands it into a flat 2048-byte
//! logical map addressed by field offset.
```

## L31 · `const R_AX_SYS_WL_EFUSE_CTRL: u32  = 0x000A;`

```
// ── Register addresses ──────────────────────────────────────────
```

## L36 · `const R_AX_SYS_ISO_CTRL: u32       = 0x0000; // 16-bit at offset, ISO bits in upper half`

```
// 16-bit at offset, ISO bits in upper half
```

## L37-38 · `const B_AX_PWC_EV2EF_B14: u16      = 1 << 14;`

```
// B_AX_PWC_EV2EF_B14 = bit 14, B_AX_PWC_EV2EF_B15 = bit 15,
// B_AX_ISO_EB2CORE = bit 8 — all within the 16-bit ISO_CTRL reg.
```

## L47 · `const B_AX_EF_ADDR_MASK: u32       = 0x07FF_0000; // GENMASK(26,16)`

```
// GENMASK(26,16)
```

## L50 · `pub const PHY_EFUSE_SIZE: u32  = 1216;`

```
// ── Chip constants (8852BE) ─────────────────────────────────────
```

## L57-62 · `const SEC_CTRL_SIZE: u32 = 4;`

```
// Sec-ctrl size: first + last N bytes are secure-control, not scanned.
// chip->sec_ctrl_efuse_size for RTL8852BE = 4 (rtw8852b.c:993).
// My first version used 2 — the logical decode started 2 bytes too
// early, read sec-ctrl bytes as headers, got 0xFFFF on the first one,
// and aborted before decoding any actual block. Result: log_map stays
// all-0xFF, every parsed field reads 0xFF.
```

## L65-66 · `const OFF_PATH_A_TSSI:   usize = 0x210;`

```
// Struct rtw8852bx_efuse field offsets (logical space).
// Derived from rtw8852b_common.h struct layout with __packed semantics.
```

## L80 · `const OFF_PCIE_MAC_ADDR:   usize = 0x400; // rtw8852bx_e_efuse (PCIe variant)`

```
// rtw8852bx_e_efuse (PCIe variant)
```

## L82 · `const TSSI_CCK_CH_GROUP_NUM:    usize = 6;`

```
// TSSI offset sub-struct sizes (from common.h)
```

## L89 · `pub const TSSI_TRIM_CH_GROUP_NUM: usize = 8;`

```
// ── Parsed output struct ────────────────────────────────────────
```

## L101 · `pub tssi_trim:      [[i8; TSSI_TRIM_CH_GROUP_NUM]; 2],`

```
// From phycap (0x580 range, separate from logical efuse).
```

## L104-106 · `pub pa_bias_trim:   [u8; 2],`

```
// PA bias trim (phycap 0x5DE path-A, 0x5DB path-B).
// Low nibble = 2G bias, high nibble = 5G bias. Applied to
// RR_BIASA.TXG/TXA per RF path by pwr_trim::run.
```

## L109-110 · `pub thermal_trim:   [u8; 2],`

```
// Thermal trim (phycap 0x5DF path-A, 0x5DC path-B).
// Applied to RR_TM2.OFF per path by pwr_trim::run.
```

## L152 · `fn print_u8(v: u8) {`

```
// ── Power-cut sequence (enable_efuse_pwr_cut_ddv / disable_*) ───
```

## L173 · `let aligned = off & !0x3;`

```
// 8-bit write via set/clr in the containing 32-bit word.
```

## L182 · `w8(mmio, R_AX_PMC_DBG_CTRL2, r8(mmio, R_AX_PMC_DBG_CTRL2) | B_AX_SYSON_DIS_PMCR_AX_WRMSK);`

```
// PMC_DBG_CTRL2 |= DIS_PMCR_AX_WRMSK
```

## L184 · `w16(mmio, R_AX_SYS_ISO_CTRL, r16(mmio, R_AX_SYS_ISO_CTRL) | B_AX_PWC_EV2EF_B14);`

```
// SYS_ISO_CTRL |= PWC_EV2EF_B14
```

## L186 · `host::sleep_ms(1); // fsleep(1000 us) — at least 1 ms`

```
// fsleep(1000 us) — at least 1 ms
```

## L187 · `w16(mmio, R_AX_SYS_ISO_CTRL, r16(mmio, R_AX_SYS_ISO_CTRL) | B_AX_PWC_EV2EF_B15);`

```
// SYS_ISO_CTRL |= PWC_EV2EF_B15
```

## L189 · `w16(mmio, R_AX_SYS_ISO_CTRL, r16(mmio, R_AX_SYS_ISO_CTRL) & !B_AX_ISO_EB2CORE);`

```
// SYS_ISO_CTRL &= ~ISO_EB2CORE
```

## L201-202 · `fn read_one(mmio: i32, addr: u32) -> Option<u8> {`

```
/// Read one byte from physical efuse at `addr` via R_AX_EFUSE_CTRL.
/// Returns None if RDY doesn't come up within the poll window.
```

## L204 · `let req = (addr << 16) & B_AX_EF_ADDR_MASK; // RDY=0 implicit`

```
// RDY=0 implicit
```

## L207 · `for _ in 0..10_000u32 {`

```
// Poll up to ~1 ms for RDY.
```

## L218-219 · `fn dump_physical(mmio: i32, addr: u32, size: usize, out: &mut [u8]) -> bool {`

```
/// Dump `size` bytes of physical efuse starting at `addr` into `out`.
/// Returns false on timeout. Caller must have enabled pwr-cut first.
```

## L230 · `fn invalid_header(h1: u8, h2: u8) -> bool { h1 == 0xFF || h2 == 0xFF }`

```
// ── Physical → logical decode (1:1 rtw89_dump_logical_efuse_map) ─
```

## L236-239 · `fn decode_logical(phy: &[u8], log: &mut [u8]) {`

```
/// Expand the sparse physical map into a flat logical map.
/// Walks 2-byte headers; each header declares a block-idx and word-
/// enable bits. For every word whose enable bit is clear, 2 payload
/// bytes follow. Stops on 0xFFFF header (unwritten region).
```

## L243 · `for b in log.iter_mut() { *b = 0xFF; }`

```
// fill log with 0xFF first
```

## L257 · `if word_en & (1 << i) != 0 { continue; } // bit set = word NOT present`

```
// bit set = word NOT present
```

## L268 · `fn parse_fields(log: &[u8]) -> EfuseData {`

```
// ── Field extraction (1:1 rtw8852bx_efuse_parsing_*) ────────────
```

## L273 · `for i in 0..6 { e.mac_addr[i] = log[OFF_PCIE_MAC_ADDR + i]; }`

```
// MAC address (PCIe variant)
```

## L276 · `e.thermal[0] = log[OFF_PATH_A_THERM];`

```
// Thermal
```

## L280 · `for path in 0..2usize {`

```
// TSSI offsets per path
```

## L290 · `let b5 = b2 + TSSI_MCS_2G_CH_GROUP_NUM + 7; // +rsvd[7]`

```
// +rsvd[7]
```

## L294 · `let _ = TSSI_OFFSET_STRUCT_SIZE; // silence unused warning in no_std`

```
// silence unused warning in no_std
```

## L297 · `e.rx_gain_2g_cck  = log[OFF_RX_GAIN_2G_CCK];`

```
// RX gain offsets
```

## L304 · `e.rfe_type     = log[OFF_RFE_TYPE];`

```
// RFE / channel_plan / xtal / country
```

## L314 · `pub fn read(mmio: i32) -> EfuseData {`

```
// ── Public entry ────────────────────────────────────────────────
```

## L316-318 · `pub fn read(mmio: i32) -> EfuseData {`

```
/// Read and parse the full efuse. Returns EfuseData::empty() on any
/// failure (the defaults are sentinel 0xFF values that consumers can
/// detect as "not available").
```

## L322 · `let autoload_bits = host::mmio_r16(mmio, R_AX_SYS_WL_EFUSE_CTRL);`

```
// 1. Check autoload status. bit is in upper byte of R_AX_SYS_WL_EFUSE_CTRL.
```

## L332-333 · `let mut phy_map = [0u8; 1216];`

```
// 2. Static buffers — stack-friendly since WASM pages are small.
//    PHY_EFUSE_SIZE = 1216, LOG = 2048.
```

## L346 · `decode_logical(&phy_map, &mut log_map);`

```
// 3. Decode sparse physical → flat logical.
```

## L349 · `let mut e = parse_fields(&log_map);`

```
// 4. Extract fields.
```

## L353-358 · `enable_pwr_cut_ddv(mmio);`

```
// 4b. Dump the 128-byte phycap region at 0x580 (separate from the
//     logical efuse) and pull tssi_trim per path from it.
//     Port of rtw8852bx_phycap_parsing_tssi (common.c:281).
//     Trim addresses are in *descending* order:
//       path A: 0x5D6, 0x5D5, 0x5D4, ...
//       path B: 0x5AB, 0x5AA, 0x5A9, ...
```

## L380 · `e.tssi_trim = [[0; TSSI_TRIM_CH_GROUP_NUM]; 2];`

```
// No valid trim programmed — Linux zeros the whole array.
```

## L384-385 · `let therm_addr = [0x5DFusize, 0x5DCusize];`

```
// Thermal trim (rtw8852bx_phycap_parsing_thermal_trim, common.c:315)
//   path A at 0x5DF, path B at 0x5DC. PG iff any byte != 0xFF.
```

## L393-394 · `let pabias_addr = [0x5DEusize, 0x5DBusize];`

```
// PA bias trim (rtw8852bx_phycap_parsing_pa_bias_trim, common.c:363)
//   path A at 0x5DE, path B at 0x5DB. PG iff any byte != 0xFF.
```

## L403 · `host::print("  EFUSE: MAC=");`

```
// 5. Log highlights (MAC as 6×u8, thermal/rfe as u8).
```

