# `tools/wasm/wifi_rtl8822ce/src/efuse.rs` @ 5e0102684

## L1-7 · `#![allow(dead_code)]`

```
//! `efuse.c` aus Linux 6.18.26 rtw88, plus `rtw8822c_read_efuse` und
//! `rtw_dump_hw_feature` — Stufe 2c.
//!
//! Portiert: `switch_efuse_bank` · `rtw_dump_physical_efuse_map` ·
//! `rtw_dump_logical_efuse_map` · `rtw_parse_efuse_map` ·
//! `rtw8822c_read_efuse` · `rtw8822ce_efuse_parsing` · `rtw_dump_hw_feature` ·
//! `rtw_check_supported_rfe` · `rtw8822c_cfg_ldo25`.
```

## L13 · `const RTW_EFUSE_BANK_WIFI: u32 = 0x0;`

```
// efuse.c:12
```

## L16 · `pub const PHYSICAL_SIZE: usize = 512;`

```
/// rtw8822c.c `rtw8822c_hw_spec`
```

## L21-29 · `const OFF_USB_MODE: usize = 0x06;`

```
// ── Feldlagen in `struct rtw8822c_efuse` (rtw8822c.h) ────────────
//
// Die Struktur ist `__packed`, die Offsets also die Summe der Felder davor.
// Linux kommentiert DREI davon ausdruecklich (0xb8, 0xc9, 0xd0, 0x120) —
// die dienen hier als Anker, gegen die die Rechnung geprueft wird.
//
//   rtl_id 2 + res0 4 + usb_mode 1 + res1 9            = 0x10
//   txpwr_idx_table[4], je 42 Byte                     = 0xa8
//   -> channel_plan bei 0x10 + 0xa8                    = 0xb8  ✓ Anker
```

## L49 · `const OFF_MAC_ADDR: usize = 0x120;`

```
/// `struct rtw8822ce_efuse.mac_addr` — der vierte Anker.
```

## L52-54 · `const _: () = assert!(OFF_TXPWR_IDX_TABLE + 4 * TXPWR_IDX_SIZE == OFF_CHANNEL_PLAN);`

```
// Die Anker aus der Linux-Quelle, zur Bauzeit geprueft. Stimmt die
// Rechnung nicht mehr, baut es nicht — statt still die falschen Bytes
// zu lesen.
```

## L60 · `const XCAP_MASK: u8 = 0x7f;`

```
/// rtw8822c.h:183
```

## L63 · `pub struct Efuse {`

```
/// `struct rtw_efuse`, nur die Felder, die `rtw8822c_read_efuse` setzt.
```

## L79-81 · `pub ext_pa_2g: u8,`

```
// `rtw8822c_read_efuse` setzt pa_type/lna_type NICHT — sie bleiben 0,
// damit bleiben auch die vier ext_*-Fahnen 0. Das ist Linux, nicht
// Bequemlichkeit.
```

## L86 · `pub hw_cap_hci: u8,`

```
// aus rtw_dump_hw_feature
```

## L92-94 · `pub txpwr_idx: [[u8; TXPWR_IDX_SIZE]; 4],`

```
/// rtw8822c.c:74-75 `efuse->txpwr_idx_table[i] = map->txpwr_idx_table[i]`
/// — vier Pfade zu 42 Byte, ROH aus der logischen efuse. Die 4-Bit-
/// Felder darin liest `txpower::TxPwrIdx`.
```

## L98 · `fn cfg_ldo25(h: i32, enable: bool) {`

```
/// rtw8822c.c `rtw8822c_cfg_ldo25`
```

## L109 · `fn switch_efuse_bank(h: i32) {`

```
/// efuse.c `switch_efuse_bank`
```

## L118-126 · `fn dump_physical_efuse_map(h: i32, map: &mut [u8]) -> bool {`

```
/// efuse.c `rtw_dump_physical_efuse_map`.
///
/// `rtw_chip_efuse_grant_on/off` sind fuer den 8822C ohne Wirkung — seine
/// `rtw_chip_ops` fuehren kein `efuse_grant`, und der Wrapper prueft das.
///
/// Linux pollt je Byte mit `udelay(1)` bis zu 1000000-mal, die Frist ist
/// also EINE SEKUNDE je Byte. Gehalten wird hier die Frist, nicht die
/// Rundenzahl — derselbe Fehler wie in `check_hw_ready` soll sich nicht
/// wiederholen.
```

## L130 · `cfg_ldo25(h, false);`

```
// disable 2.5V LDO
```

## L158 · `#[inline]`

```
// efuse.c:20-29 — die vier Kopfmakros, Zeichen fuer Zeichen.
```

## L180 · `fn dump_logical_efuse_map(phy_map: &[u8], log_map: &mut [u8]) -> bool {`

```
/// efuse.c `rtw_dump_logical_efuse_map`
```

## L222 · `fn read_efuse(log_map: &[u8]) -> Efuse {`

```
/// rtw8822c.c `rtw8822c_read_efuse` + `rtw8822ce_efuse_parsing`
```

## L237 · `let mut t = [[0u8; TXPWR_IDX_SIZE]; 4];`

```
// rtw8822c.c:74-75 — vier Pfade zu 42 Byte am Stueck.
```

## L246-248 · `addr: [0; 6],`

```
// Der Rest bleibt null, wie in Linux, wo `rtw_efuse` Teil eines
// kzalloc'ten `rtw_dev` ist. `share_ant`/`btcoex` und die vier
// `ext_*` setzt `efuse_info_setup` danach.
```

## L262 · `e.addr.copy_from_slice(&log_map[OFF_MAC_ADDR..OFF_MAC_ADDR + 6]);`

```
// rtw8822ce_efuse_parsing: ether_addr_copy(efuse->addr, map->e.mac_addr)
```

## L267-271 · `fn dump_hw_feature(h: i32, e: &mut Efuse, rf_path_num: u8) -> bool {`

```
/// main.c `rtw_dump_hw_feature`. `hw_feature_report` ist beim 8822C true.
///
/// Der Ausloeser dafuer steht in `rtw_chip_efuse_enable` und ist ein
/// einzelner Registerschreiber VOR dem Firmware-Download:
/// `rtw_write8(REG_C2HEVT, C2H_HW_FEATURE_DUMP)`.
```

## L287 · `let w1 = u32::from_le_bytes([hw_feature[4], hw_feature[5],`

```
// efuse.h:15-24 — alle Felder aus dem ZWEITEN 32-Bit-Wort.
```

## L303 · `fn hw_bw_cap_to_bitamp(bw_cap: u8) -> u8 {`

```
/// main.c `hw_bw_cap_to_bitamp`
```

## L305 · `let mut bw = 1 << 0;`

```
// BIT(RTW_CHANNEL_WIDTH_20) ist immer dabei; 40 und 80 kommen dazu.
```

## L315-316 · `fn check_supported_rfe(rfe_option: u8) -> bool {`

```
/// phy.h `rtw_check_supported_rfe`. Fuer den 8822C fuehren alle sieben
/// `rfe_defs` gueltige Tabellen, die Pruefung ist also die Reichweite.
```

## L321 · `const RTW8822C_RFE_DEFS_SIZE: usize = 7;`

```
/// rtw8822c.c `rtw8822c_rfe_defs` hat sieben Eintraege.
```

## L324-327 · `pub fn efuse_info_setup(h: i32, rf_path_num: u8) -> Option<Efuse> {`

```
/// main.c `rtw_chip_efuse_info_setup`, ab `rtw_parse_efuse_map`.
///
/// Der Chip muss dafuer AN sein und die Firmware laufen — `hw_feature`
/// kommt von ihr.
```

## L330-332 · `let mut log_map = [0xffu8; LOGICAL_SIZE];`

```
// Linux: `memset(log_map, 0xff, log_size)` VOR dem Auspacken — was der
// Kopf nicht beschreibt, bleibt 0xff und faellt damit in die
// Vorgabe-Zweige weiter unten.
```

## L357 · `if e.crystal_cap == 0xff { e.crystal_cap = 0; }`

```
// main.c:2024-2046 — die Vorgaben und die abgeleiteten Fahnen.
```

## L375-386 · `pub fn read8_physical(h: i32, addr: u16) -> u8 {`

```
/// efuse.c:31-49 `rtw_read8_physical_efuse` — EIN Byte aus der physischen
/// efuse, ohne Bankwechsel und ohne LDO-Schalter.
///
/// Das ist der Weg, ueber den `rtw8822c_rf_init` seine Werkskalibrierung
/// holt (Thermik, Leistungsabgleich, PA-Vorspannung). Linux pollt mit
/// `read_poll_timeout(..., 1000, 100000, ...)`, also alle 1 ms bis
/// **100 ms** — eine andere Frist als der volle Abzug oben, und deshalb
/// steht sie hier eigens.
///
/// Gibt `EFUSE_READ_FAIL` zurueck, wenn die Frist ablaeuft. Der Wert ist
/// 0xff und damit zugleich das, was ein UNBESCHRIEBENES efuse-Byte liefert
/// — die Rufer behandeln beides gleich, und das ist Absicht.
```

