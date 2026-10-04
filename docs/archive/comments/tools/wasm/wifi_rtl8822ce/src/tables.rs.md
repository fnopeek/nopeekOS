# `tools/wasm/wifi_rtl8822ce/src/tables.rs` @ 5e0102684

## L1-8 · `#![allow(dead_code)]`

```
//! ERZEUGT von gen_tables.py aus Linux 6.18.26 rtw8822c_table.c — nicht
//! von Hand aendern.
//!
//! Die sechs Tabellen, die `rtw_phy_load_tables` laedt. Jede ist ein flaches
//! `u32`-Feld aus Paaren; was ein Paar BEDEUTET, entscheidet
//! `rtw_parse_tbl_phy_cond` (`phy.rs`): ist im ersten Wort Bit 31 gesetzt,
//! ist es eine Bedingung, ist Bit 30 gesetzt, endet ein Bedingungsblock —
//! sonst sind es Adresse und Wert.
```

## L11 · `pub static MAC: [u32; 0] = [`

```
/// rtw8822c_table.c `rtw8822c_mac` — 0 Woerter = 0 Kacheln · rtw_phy_cfg_mac — leer beim 8822C
```

## L15 · `pub static BB: [u32; 3006] = [`

```
/// rtw8822c_table.c `rtw8822c_bb` — 3006 Woerter = 1503 Kacheln · rtw_phy_cfg_bb
```

## L395 · `pub static AGC: [u32; 3728] = [`

```
/// rtw8822c_table.c `rtw8822c_agc` — 3728 Woerter = 1864 Kacheln · rtw_phy_cfg_agc
```

## L865 · `pub static RFK_INIT: [u32; 4920] = [`

```
/// rtw8822c_table.c `rtw8822c_array_mp_cal_init` — 4920 Woerter = 2460 Kacheln · rtw_phy_cfg_bb, ueber rtw_load_rfk_table
```

## L1484 · `pub static RF_A: [u32; 40070] = [`

```
/// rtw8822c_table.c `rtw8822c_rf_a` — 40070 Woerter = 20035 Kacheln · rtw_phy_cfg_rf, RF_PATH_A
```

## L6497 · `pub static RF_B: [u32; 40706] = [`

```
/// rtw8822c_table.c `rtw8822c_rf_b` — 40706 Woerter = 20353 Kacheln · rtw_phy_cfg_rf, RF_PATH_B
```

## L11590-11594 · `pub const EXPECTED_WRITES_CUT_D_RFE1: [(&str, u32); 6] = [`

```
/// Wieviele Schreibzugriffe jede Tabelle auf UNSEREM Geraet abgibt
/// (cut 3 = CUT_D · rfe_option 1 · PCIe · pkg 15), vom Erzeuger
/// nachgerechnet. Der Lader haelt seine eigene Zahl dagegen: stimmt sie
/// nicht, laeuft der Bedingungslaeufer anders als Linux' — und das ist ein
/// Fehler, den man sonst erst als stummen Funkausfall sieht.
```

## L11604 · `pub static COEX_TABLE_SANT: [[u32; 2]; 35] = [`

```
/// rtw8822c.c `table_sant_8822c` — 35 Faelle
```

## L11643 · `pub static COEX_TABLE_NSANT: [[u32; 2]; 24] = [`

```
/// rtw8822c.c `table_nsant_8822c` — 24 Faelle
```

## L11671 · `pub static COEX_TDMA_SANT: [[u8; 5]; 28] = [`

```
/// rtw8822c.c `tdma_sant_8822c` — 28 Faelle
```

## L11703 · `pub static COEX_TDMA_NSANT: [[u8; 5]; 22] = [`

```
/// rtw8822c.c `tdma_nsant_8822c` — 22 Faelle
```

## L11729-11735 · `pub static TXPWR_BY_RATE_MAP: [(u32, &[u8]); 81] = [`

```
/// phy.c `rtw_phy_get_rate_values_of_txpwr_by_rate` — der Teil, der
/// ZUORDNET. Eine Registeradresse aus `bb_pg` nennt eine Gruppe von Raten;
/// die Werte selbst kommen byteweise aus dem Tabelleneintrag.
///
/// **Nicht enthalten sind die zwei Faelle, die RECHNEN** (0xE08 mit
/// `bcd_to_dec_pwr_by_rate`, 0x86C je nach Maske). Die stehen als Code in
/// `phy.rs` — hier waeren sie eine Zuordnung, die keine ist.
```

## L11820 · `pub static BB_PG_TYPE0: [[u32; 6]; 46] = [`

```
/// rtw8822c_table.c `rtw8822c_bb_pg_type0` — 46 Zeilen
```

## L11870 · `pub static TXPWR_LMT_TYPE0: [(u8, u8, u8, u8, u8, i8); 2340] = [`

```
/// rtw8822c_table.c `rtw8822c_txpwr_lmt_type0` — 2340 Zeilen
```

## L14214 · `pub static TXPWR_LMT_TYPE5: [(u8, u8, u8, u8, u8, i8); 1365] = [`

```
/// rtw8822c_table.c `rtw8822c_txpwr_lmt_type5` — 1365 Zeilen
```

## L15583-15585 · `pub static RATE_SECTION: [&[u8]; 10] = [`

```
/// phy.c:118-124 `rtw_rate_section[]` — die zehn Ratenabschnitte in
/// der Reihenfolge von `enum rtw_rate_section`. Die LAENGEN stehen in
/// `rtw_rate_size[]` und sind hier die Laenge des Scheibchens.
```

## L15599 · `pub static CHANNEL_IDX_5G: [u8; 49] = [`

```
/// phy.c:1581-1588 `rtw_channel_idx_5g[]`
```

## L15610-15615 · `pub static CHANNEL_GROUP: [u8; 178] = [`

```
/// phy.c:1872-1960 `rtw_get_channel_group` — Kanal auf Leistungsgruppe.
/// Index ist die Kanalnummer; 0xff heisst „kein Eintrag" (Linux warnt dort
/// und faellt auf Gruppe 0).
///
/// **Kanal 2 rechnet** — CCK gibt 0, alles andere 1 — und steht deshalb
/// hier mit dem NICHT-CCK-Wert; den Sonderfall macht `txpower.rs`.
```

## L15631 · `pub static CHANNEL_GROUP_CCK: [(u8, u8, u8); 1] = [`

```
/// Die rechnenden Faelle: (Kanal, Gruppe fuer CCK, sonst)
```

## L15636 · `pub static DB_INVERT_TABLE: [[u32; 8]; 12] = [`

```
/// phy.c:28-53 `db_invert_table[12][8]`
```

## L15652-15658 · `pub static EXPECTED_TXPWR_SUMS: [u32; 6] = [`

```
/// `rtw_chip_board_info_setup` fuer rfe_option 1, vom Erzeuger
/// NACHGERECHNET. Der abgeleitete Zustand ist 25 KiB gross — einzeln
/// vergleichen geht nicht, eine Summe ueber alle Zellen schon, und die
/// faellt bei jedem einzelnen falschen Byte auf.
///
/// Reihenfolge: by_rate_offset_2g · _5g · by_rate_base_2g · _5g ·
/// limit_2g · limit_5g. Summiert wird byteweise, ohne Vorzeichen.
```

## L15664-15666 · `pub static PWRTRK_2GA_N: [u8; 30] = [`

```
/// rtw8822c.c `rtw8822c_pwr_track_type0_tbl` — die Kurven der
/// Sendeleistungs-Nachfuehrung, 30 Stuetzstellen je Kurve
/// (`RTW_PWR_TRK_TBL_SZ`). Index = |Thermometer - efuse|.
```

## L15715 · `pub static DPK_MAC_BB: [u32; 42] = [`

```
/// rtw8822c_table.c `rtw8822c_dpk_mac_bb` — 14 Tripel (Adresse, Maske, Wert) · rtw8822c_dpk_mac_bb_setting
```

## L15733 · `pub static DPK_AFE_IS_DPK: [u32; 117] = [`

```
/// rtw8822c_table.c `rtw8822c_dpk_afe_is_dpk` — 39 Tripel (Adresse, Maske, Wert) · rtw8822c_dpk_afe_setting(true)
```

## L15776 · `pub static DPK_AFE_NO_DPK: [u32; 105] = [`

```
/// rtw8822c_table.c `rtw8822c_dpk_afe_no_dpk` — 35 Tripel (Adresse, Maske, Wert) · rtw8822c_dpk_afe_setting(false)
```

