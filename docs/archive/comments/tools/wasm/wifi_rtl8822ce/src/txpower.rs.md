# `tools/wasm/wifi_rtl8822ce/src/txpower.rs` @ 5e0102684

## L1-18 · `#![allow(dead_code)]`

```
//! `phy.c` aus Linux 6.18.26 rtw88 — die Sendeleistung.
//!
//! **Warum das VOR Stufe 3 steht:** in Linux laeuft `rtw_chip_board_info_setup`
//! (main.c:2064) zur Probe-Zeit, direkt nach `rtw_chip_efuse_info_setup` und
//! LANGE vor `rtw_power_on`. Es schreibt kein einziges Register — es fuellt
//! nur die Tabellen in `hal`, aus denen `rtw_set_channel` spaeter die
//! Sendeleistung je Kanal, Rate und Pfad ausrechnet.
//!
//! Portiert: `rtw_phy_init_tx_power` · `rtw_phy_init_tx_power_limit` ·
//! `bcd_to_dec_pwr_by_rate` · `tbl_to_dec_pwr_by_rate` ·
//! `rtw_phy_get_rate_values_of_txpwr_by_rate` · `rtw_phy_store_tx_power_by_rate` ·
//! `rtw_parse_tbl_bb_pg` · `rtw_channel_to_idx` · `rtw_phy_set_tx_power_limit` ·
//! `rtw_regd_has_alt` · `__cfg_txpwr_lmt_by_alt` · `rtw_cfg_txpwr_lmt_by_alt` ·
//! `rtw_xref_5g_txpwr_lmt` · `rtw_xref_txpwr_lmt_by_rs` ·
//! `rtw_xref_5g_txpwr_lmt_by_ch` · `rtw_xref_txpwr_lmt_by_bw` ·
//! `rtw_xref_txpwr_lmt` · `rtw_parse_tbl_txpwr_lmt` ·
//! `rtw_phy_tx_power_by_rate_config_by_path` · `rtw_phy_tx_power_by_rate_config` ·
//! `__rtw_phy_tx_power_limit_config` · `rtw_phy_tx_power_limit_config`.
```

## L25 · `pub const RTW_RF_PATH_MAX: usize = 4; // main.h:38`

```
// ── Masse aus main.h ─────────────────────────────────────────────
```

## L26 · `pub const RTW_RF_PATH_MAX: usize = 4; // main.h:38`

```
// main.h:38
```

## L27 · `pub const DESC_RATE_MAX: usize = 0x54; // main.h:341, folgt auf 0x53`

```
// main.h:341, folgt auf 0x53
```

## L28 · `pub const RTW_RATE_SECTION_NUM: usize = 10; // main.h:176`

```
// main.h:176
```

## L29 · `pub const RTW_REGD_MAX: usize = 13; // main.h:359, folgt auf RTW_REGD_WW`

```
// main.h:359, folgt auf RTW_REGD_WW
```

## L30 · `pub const RTW_REGD_WW: usize = 12; // main.h:358`

```
// main.h:358
```

## L31 · `pub const RTW_CHANNEL_WIDTH_MAX: usize = 3; // main.h:37`

```
// main.h:37
```

## L32 · `pub const RTW_MAX_CHANNEL_NUM_2G: usize = 14; // main.h:49`

```
// main.h:49
```

## L33 · `pub const RTW_MAX_CHANNEL_NUM_5G: usize = 49; // main.h:50`

```
// main.h:50
```

## L34 · `pub const PHY_BAND_2G: u8 = 0; // main.h, enum rtw_phy_band_type`

```
// main.h, enum rtw_phy_band_type
```

## L37 · `pub const MAX_POWER_INDEX: i8 = 0x7f;`

```
/// rtw8822c.c:5351 `.max_power_index = 0x7f`
```

## L40-42 · `const REGD_ALT: [Option<usize>; RTW_REGD_MAX] = [`

```
/// regd.c:522-533 `rtw_regd_alt[]` — welche Regulierungszone von welcher
/// abschreibt, wenn die Tabelle fuer sie nichts sagt. `None` heisst
/// „keine Ersatzzone", und dann gilt WW.
```

## L44 · `None,       // FCC`

```
// FCC
```

## L45 · `None,       // MKK`

```
// MKK
```

## L46 · `None,       // ETSI`

```
// ETSI
```

## L47 · `Some(0),    // IC      -> FCC`

```
// IC      -> FCC
```

## L48 · `Some(2),    // KCC     -> ETSI`

```
// KCC     -> ETSI
```

## L49 · `Some(2),    // ACMA    -> ETSI`

```
// ACMA    -> ETSI
```

## L50 · `Some(0),    // CHILE   -> FCC`

```
// CHILE   -> FCC
```

## L51 · `Some(2),    // UKRAINE -> ETSI`

```
// UKRAINE -> ETSI
```

## L52 · `Some(0),    // MEXICO  -> FCC`

```
// MEXICO  -> FCC
```

## L53 · `Some(2),    // CN      -> ETSI`

```
// CN      -> ETSI
```

## L54 · `Some(2),    // QATAR   -> ETSI`

```
// QATAR   -> ETSI
```

## L55 · `Some(2),    // UK      -> ETSI`

```
// UK      -> ETSI
```

## L56 · `None,       // WW`

```
// WW
```

## L59-62 · `pub struct TxPower {`

```
/// main.h:1993-2011 `struct rtw_hal`, der Sendeleistungsteil.
///
/// Rund 25 KiB. Das ist kein Versehen: `tx_pwr_limit_5g` allein ist
/// 13 Zonen x 3 Bandbreiten x 10 Ratenabschnitte x 49 Kanaele.
```

## L76-78 · `pub fn new() -> Self {`

```
/// phy.c:2427-2446 `rtw_phy_init_tx_power` — samt
/// `rtw_phy_init_tx_power_limit`, das nichts tut, als jede Zelle auf
/// `max_power_index` zu setzen. Die Null davor kommt vom Anlegen.
```

## L94 · `for ch in 0..RTW_MAX_CHANNEL_NUM_2G {`

```
// phy.c:2411-2425 `rtw_phy_init_tx_power_limit`
```

## L108 · `fn bcd_to_dec_pwr_by_rate(val: u32, i: u32) -> i8 {`

```
// ── Die Leistung JE RATE (bb_pg) ─────────────────────────────────
```

## L110-113 · `fn bcd_to_dec_pwr_by_rate(val: u32, i: u32) -> i8 {`

```
/// phy.c:1219 `#define bcd_to_dec_pwr_by_rate(val, i) bcd2bin(val >> (i * 8))`
///
/// `bcd2bin(x)` ist `(x & 0x0f) + (x >> 4) * 10` — eine Zahl, deren zwei
/// Nibbles DEZIMALziffern sind.
```

## L119-122 · `fn tbl_to_dec_pwr_by_rate(hex: u32, i: u32) -> i8 {`

```
/// phy.c:1221-1227 `tbl_to_dec_pwr_by_rate`.
///
/// `chip->is_pwr_by_rate_dec` ist beim 8822C **false** (rtw8822c.c:5350),
/// also gilt der einfache Zweig: das Byte, wie es dasteht.
```

## L127-134 · `fn rate_values_of_txpwr_by_rate(addr: u32, mask: u32, val: u32)`

```
/// phy.c:1229-1532 `rtw_phy_get_rate_values_of_txpwr_by_rate`.
///
/// Die ZUORDNUNG steht in `tables::TXPWR_BY_RATE_MAP`, erzeugt aus genau
/// diesem `switch`. Hier stehen nur die zwei Faelle, die RECHNEN statt
/// zuzuordnen — und dass sie hier stehen, ist der Grund, warum sie nicht
/// in der Tabelle sind.
///
/// Gibt `(raten, werte, anzahl)` zurueck.
```

## L141 · `if addr == 0xE08 {`

```
// phy.c:1259 `case 0xE08:` — EIN Wert, und er kommt aus BCD, Byte 1.
```

## L143 · `rate[0] = 0x00; // DESC_RATE1M`

```
// DESC_RATE1M
```

## L148 · `if addr == 0x86C {`

```
// phy.c:1264-1277 `case 0x86C:` — haengt an der MASKE.
```

## L151 · `rate[0] = 0x01; // DESC_RATE2M`

```
// DESC_RATE2M
```

## L152 · `rate[1] = 0x02; // DESC_RATE5_5M`

```
// DESC_RATE5_5M
```

## L153 · `rate[2] = 0x03; // DESC_RATE11M`

```
// DESC_RATE11M
```

## L159 · `rate[0] = 0x03; // DESC_RATE11M`

```
// DESC_RATE11M
```

## L163 · `return (rate, pwr, 0);`

```
// Linux faellt hier ohne `rate_num` heraus, also mit 0.
```

## L177 · `(rate, pwr, 0)`

```
// `default:` in Linux: nur eine Fehlermeldung, `rate_num` bleibt 0.
```

## L181 · `fn store_tx_power_by_rate(t: &mut TxPower, band: u32, rfpath: usize,`

```
/// phy.c:1534-1562 `rtw_phy_store_tx_power_by_rate`
```

## L206 · `pub fn parse_tbl_bb_pg(t: &mut TxPower) {`

```
/// phy.c:1564-1579 `rtw_parse_tbl_bb_pg`
```

## L219 · `fn channel_to_idx(band: u8, channel: u8) -> Option<usize> {`

```
// ── Die GRENZE je Zone, Bandbreite, Ratenabschnitt und Kanal ─────
```

## L221 · `fn channel_to_idx(band: u8, channel: u8) -> Option<usize> {`

```
/// phy.c:1590-1611 `rtw_channel_to_idx`
```

## L236-241 · `fn set_tx_power_limit(t: &mut TxPower, regd: usize, band: u8, bw: usize,`

```
/// phy.c:1613-1645 `rtw_phy_set_tx_power_limit`.
///
/// **Jeder Wert geht zweimal hinein:** einmal in seine Zone und einmal als
/// MINIMUM in die Welt-Zone `RTW_REGD_WW`. Die ist damit am Ende die
/// strengste aller Zonen — und der Rueckfall fuer alles, was die Tabelle
/// gar nicht nennt.
```

## L266 · `fn cfg_txpwr_lmt_by_alt_one(t: &mut TxPower, regd: usize, regd_alt: usize,`

```
/// phy.c:1716-1728 `__cfg_txpwr_lmt_by_alt`
```

## L277 · `fn cfg_txpwr_lmt_by_alt(t: &mut TxPower, regd: usize, regd_alt: usize) {`

```
/// phy.c:1730-1738 `rtw_cfg_txpwr_lmt_by_alt`
```

## L286 · `const RS_HT_1S: usize = 2;`

```
// phy.c:1662-1666 — die Paare, ueber die quergeglichen wird.
```

## L296-300 · `fn xref_5g_txpwr_lmt(t: &mut TxPower, regd: usize, bw: usize, ch_idx: usize,`

```
/// phy.c:1647-1664 `rtw_xref_5g_txpwr_lmt`.
///
/// Sagt die Tabelle fuer HT etwas und fuer VHT nichts (oder umgekehrt),
/// bekommt der stumme den Wert des anderen. „Nichts gesagt" heisst hier
/// `max_power_index` — der Wert, mit dem `init_tx_power_limit` gefuellt hat.
```

## L316 · `fn xref_txpwr_lmt_by_rs(t: &mut TxPower, regd: usize, bw: usize, ch_idx: usize) {`

```
/// phy.c:1667-1684 `rtw_xref_txpwr_lmt_by_rs`
```

## L329 · `fn xref_5g_txpwr_lmt_by_ch(t: &mut TxPower, regd: usize, bw: usize) {`

```
/// phy.c:1686-1694 `rtw_xref_5g_txpwr_lmt_by_ch`
```

## L336 · `fn xref_txpwr_lmt_by_bw(t: &mut TxPower, regd: usize) {`

```
/// phy.c:1696-1704 `rtw_xref_txpwr_lmt_by_bw` — nur 20 und 40 MHz.
```

## L343 · `fn xref_txpwr_lmt(t: &mut TxPower) {`

```
/// phy.c:1706-1713 `rtw_xref_txpwr_lmt`
```

## L350-354 · `pub fn parse_tbl_txpwr_lmt(t: &mut TxPower, tbl: &[(u8, u8, u8, u8, u8, i8)]) {`

```
/// phy.c:1740-1780 `rtw_parse_tbl_txpwr_lmt`.
///
/// Danach die zwei Aufraeumschritte, die Linux gleich mitmacht: eine Zone,
/// die in der Tabelle gar nicht vorkommt, schreibt von ihrer Ersatzzone ab
/// (`rtw_regd_has_alt`) — und wenn es keine gibt, von der Welt-Zone.
```

## L383 · `fn tx_power_by_rate_config_by_path(t: &mut TxPower, path: usize, rs: usize,`

```
// ── Aus absoluten Werten werden Abweichungen ─────────────────────
```

## L385-389 · `fn tx_power_by_rate_config_by_path(t: &mut TxPower, path: usize, rs: usize,`

```
/// phy.c:2348-2369 `rtw_phy_tx_power_by_rate_config_by_path`.
///
/// **Die Basisrate ist bei VHT die DRITTLETZTE**, sonst die letzte — bei
/// zehn VHT-Raten also MCS7 und nicht MCS9. Alles wird danach relativ zu
/// ihr ausgedrueckt.
```

## L409 · `pub fn tx_power_by_rate_config(t: &mut TxPower) {`

```
/// phy.c:2371-2380 `rtw_phy_tx_power_by_rate_config`
```

## L419 · `fn tx_power_limit_config_one(t: &mut TxPower, regd: usize, bw: usize, rs: usize) {`

```
/// phy.c:2382-2396 `__rtw_phy_tx_power_limit_config`
```

## L421 · `let base_2g = t.by_rate_base_2g[0][rs];`

```
// **Pfad 0**, auch fuer Pfad 1 — so steht es in Linux.
```

## L432 · `pub fn tx_power_limit_config(t: &mut TxPower) {`

```
/// phy.c:2398-2409 `rtw_phy_tx_power_limit_config`
```

## L434 · `t.cch_by_bw[0] = 1;`

```
// default at channel 1
```

## L446-451 · `pub fn txpwr_lmt_tbl(rfe_option: u8) -> Option<&'static [(u8, u8, u8, u8, u8, i8)]> {`

```
/// phy.h:119-136 `rtw_get_rfe_def` — welcher RFE-Satz fuer dieses Board.
///
/// rtw8822c.c:5277-5285: sieben Eintraege, und nur `[5]` weicht ab (er
/// nimmt `txpwr_lmt_type5`). `rfe_option` ausserhalb 0..6 hat keinen
/// Eintrag; Linux gibt dann NULL und `rtw_chip_board_info_setup`
/// scheitert.
```

## L460-467 · `pub fn board_info_setup(rfe_option: u8) -> Option<TxPower> {`

```
/// main.c:2064-2081 `rtw_chip_board_info_setup`, ohne
/// `rtw_phy_setup_phy_cond` — das steht in `phy.rs`, weil es die
/// Bedingung fuer die PARAMETERTABELLEN rechnet und nicht fuer die
/// Sendeleistung.
///
/// **Kein einziger Registerzugriff.** Diese Stufe fuellt nur die Tabellen,
/// aus denen `rtw_set_channel` spaeter rechnet — und genau deshalb laeuft
/// sie in Linux zur Probe-Zeit und nicht im Anlaufweg.
```

## L478-480 · `pub fn checksums(t: &TxPower) -> [u32; 6] {`

```
/// Die eigenen Summen, in derselben Reihenfolge wie
/// `tables::EXPECTED_TXPWR_SUMS`. Byteweise, ohne Vorzeichen — genau wie
/// die Nachrechnung im Erzeuger.
```

## L524-526 · `pub struct TxPwrIdx<'a>(pub &'a [u8; 42]);`

```
// ════════════════════════════════════════════════════════════════
// Stufe 4c: aus den Tabellen wird ein Leistungsindex je Rate
// ════════════════════════════════════════════════════════════════
```

## L528-532 · `pub struct TxPwrIdx<'a>(pub &'a [u8; 42]);`

```
/// main.h:438-531 `struct rtw_txpwr_idx`, wie er in der efuse liegt:
/// 42 Byte je Pfad, gepackt, mit 4-Bit-Feldern.
///
///     2G: cck_base[6] · bw40_base[5] · ht_1s_diff · ht_2s/3s/4s_diff
///     5G: bw40_base[14] · ht_1s · ht_2s/3s/4s · ofdm · vht_1s..4s
```

## L537-538 · `let v = if hi { b >> 4 } else { b & 0x0f };`

```
// Ein 4-Bit-Bitfeld mit Vorzeichen, little-endian: das UNTERE
// Nibble ist das erste Feld.
```

## L544 · `pub fn g2_ht1s_ofdm(&self) -> i8 { Self::n4(self.0[11], false) }`

```
// 2G ht_1s_diff @11: ofdm (unten), bw20 (oben)
```

## L547-548 · `pub fn g2_ns_bw20(&self, n: usize) -> i8 { Self::n4(self.0[12 + (n - 2) * 2], false) }`

```
// 2G ht_2s/3s/4s_diff @12,13,14: bw20 (unten), bw40 (oben) im ersten
// Byte — `rtw_2g_ns_pwr_idx_diff` ist ZWEI Byte: bw20,bw40 | cck,ofdm
```

## L551 · `pub fn bw40_base_5g(&self, g: usize) -> u8 { self.0[18 + g] }`

```
// 5G beginnt bei 18: bw40_base[14]
```

## L555-557 · `pub fn g5_ns_bw20(&self, n: usize) -> i8 { Self::n4(self.0[33 + (n - 2)], false) }`

```
// **`rtw_5g_ht_ns_pwr_idx_diff` ist EIN Byte** (bw20:4, bw40:4) — das
// 2G-Gegenstueck daneben ist zwei, und genau dieser Schritt stand hier
// zuerst. @33,34,35 fuer 2s/3s/4s.
```

## L560-562 · `pub fn g5_vht_bw80(&self, n: usize) -> i8 { Self::n4(self.0[38 + (n - 1)], true) }`

```
// ofdm_diff @36,37 (zwei Byte) liest `rtw_phy_get_5g_tx_power_index`
// nicht — dahinter kommen vht_1s..4s @38,39,40,41, und in
// `rtw_5g_vht_ns_pwr_idx_diff` steht bw160 UNTEN, bw80 OBEN.
```

## L566-579 · `const DESC_RATE11M: u8 = crate::regs::DESC_RATE11M as u8;`

```
// Ratengrenzen aus main.h:249-340, gebraucht fuer die Abschnitts- und
// Streamzuordnung.
//
// **Sie werden aus `regs.rs` ABGELEITET und nicht daneben geschrieben.**
// Hier stand eine zweite Liste von Hand, und ab `DESC_RATEMCS31` war sie
// ganz um eins zu hoch — MCS31 als 0x2c statt 0x2b, und damit jeder
// VHT-Anfang eine Stelle daneben. Der Fehler kippt genau den ERSTEN
// Eintrag jedes Abschnitts, deshalb ist er nie aufgefallen:
// `rate_to_rate_section(0x2c)` gab 7 (HT 4SS) statt 4 (VHT 1SS), und
// `above_2ss(0x36)` sagte NEIN zur ersten 2SS-Rate. VHT-MCS0 ist die
// Rate, die man nur am Rand der Zelle sieht.
//
// Die Enden (`_MCS7/9/15/23/31`) stehen als Rechnung da, weil die Reihen
// in `main.h` lueckenlos sind: acht MCS je HT-Abschnitt, zehn je VHT.
```

## L600-601 · `pub fn rate_to_rate_section(rate: u8) -> usize {`

```
/// phy.c:1962-1990 `rtw_phy_rate_to_rate_section`.
/// `RTW_RATE_SECTION_NUM` heisst „Rate ungueltig".
```

## L618-619 · `fn channel_group(channel: u8, rate: u8) -> u8 {`

```
/// phy.c:1872-1960 `rtw_get_channel_group` — die Zuordnung ist erzeugt,
/// der rechnende Fall (Kanal 14) steht hier.
```

## L630 · `0`

```
// Linux: `default: WARN_ON(1); fallthrough;` -> Gruppe 0
```

## L649 · `fn get_2g_tx_power_index(p: &TxPwrIdx, bw: usize, rate: u8, group: u8) -> u8 {`

```
/// phy.c:1993-2050 `rtw_phy_get_2g_tx_power_index`
```

## L667 · `if above_2ss(rate) { tx += p.g2_ns_bw40(2) as i16 * f; }`

```
// bw40 ist die Basis
```

## L673 · `tx += p.g2_ht1s_bw20() as i16 * f;`

```
// RTW_CHANNEL_WIDTH_20 und Linux' `default: WARN_ON(1)`
```

## L683 · `fn get_5g_tx_power_index(p: &TxPwrIdx, bw: usize, rate: u8, group: u8) -> u8 {`

```
/// phy.c:2052-2120 `rtw_phy_get_5g_tx_power_index`
```

## L700 · `let lower = p.bw40_base_5g(g) as i16;`

```
// die Basis von 80 MHz ist der Mittelwert aus bw40+ und bw40-
```

## L719-723 · `fn get_tx_power_limit(t: &TxPower, band: u8, bw: usize, rate: u8,`

```
/// phy.c:2149-2196 `rtw_phy_get_tx_power_limit`.
///
/// **Es wird das MINIMUM ueber alle Bandbreiten von 20 MHz bis zur
/// aktuellen genommen** — nicht nur die aktuelle. Und CCK/OFDM kennen nur
/// 20 MHz, HT hoechstens 40.
```

## L737 · `bw = 0; // nur 20 MHz bei CCK und OFDM`

```
// nur 20 MHz bei CCK und OFDM
```

## L740 · `bw = bw.min(1); // HT hoechstens 40 MHz`

```
// HT hoechstens 40 MHz
```

## L759-760 · `fn dis_dpd_by_rate_diff(rate: u8) -> i16 {`

```
/// phy.c:2122-2147 `rtw_phy_get_dis_dpd_by_rate_diff`.
/// `en_dis_dpd` ist beim 8822C true, `dpd_ratemask` ist `DIS_DPD_RATEALL`.
```

## L781-787 · `#[allow(clippy::too_many_arguments)]`

```
/// phy.c:2219-2256 `rtw_get_tx_power_params` + phy.c:2258-2283
/// `rtw_phy_get_tx_power_index`, zusammengezogen.
///
/// **`pwr_sar` ist `max_power_index`**: `hal->sar.src` ist
/// `RTW_SAR_SOURCE_NONE`, solange kein ACPI-SAR-Block da ist, und
/// `rtw_query_sar` gibt dann genau das zurueck. **`pwr_remnant` ist 0**:
/// `txagc_remnant_*` setzt die Leistungsnachfuehrung im laufenden Betrieb.
```

## L812-813 · `(tx_power as i32 as u32 & 0xff) as u8`

```
// Linux rechnet in u8 und laesst es umlaufen; der Wert wird danach
// ohnehin auf sieben Bit beschnitten.
```

## L820-825 · `pub fn set_tx_power_level(t: &TxPower, p: &[TxPwrIdx], tbl: &mut [[u8; DESC_RATE_MAX]; RTW_RF_PATH_MAX],`

```
/// phy.c:2285-2330 `rtw_phy_set_tx_power_index_by_rs` +
/// `rtw_phy_set_tx_power_level_by_path`. Fuellt `tx_pwr_tbl`.
///
/// `regd` kommt in Linux aus `rtw_regd_get` — der Regulierungszone, die
/// `rtw_regd_init` aus dem Laenderkuerzel setzt. Solange die nicht steht,
/// ist es die efuse-Zone.
```

## L829 · `let start = if band == PHY_BAND_2G { 0 } else { 1 };`

```
// Ohne 2,4 GHz keine CCK-Raten.
```

