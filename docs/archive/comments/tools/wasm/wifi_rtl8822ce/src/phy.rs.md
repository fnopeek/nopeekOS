# `tools/wasm/wifi_rtl8822ce/src/phy.rs` @ 5e0102684

## L1-13 · `#![allow(dead_code)]`

```
//! `phy.c` aus Linux 6.18.26 rtw88 — Stufe 3b: die Parametertabellen.
//!
//! Portiert: `rtw_phy_setup_phy_cond` · `check_positive` ·
//! `rtw_parse_tbl_phy_cond` · `rtw_phy_cfg_mac` · `rtw_phy_cfg_agc` ·
//! `rtw_phy_cfg_bb` · `rtw_phy_cfg_rf` · `rtw_load_rfk_table` ·
//! `rtw_phy_load_tables` · `rtw_phy_read_rf` · `rtw_phy_write_rf_reg` ·
//! `rtw_phy_write_rf_reg_sipi` · `rtw_phy_write_rf_reg_mix`.
//!
//! **`rtw_phy_read_rf_sipi` ist NICHT portiert, und das ist keine Luecke:**
//! es liest `chip->rf_sipi_read_addr`, und der 8822C setzt das Feld nicht.
//! In Linux endet die Funktion fuer diesen Chip in `rf_sipi_read_addr isn't
//! defined` und gibt `INV_RF_DATA` zurueck. Eine Portierung waere Code fuer
//! einen Zweig, den dieser Chip nicht hat.
```

## L20 · `pub const RFREG_MASK: u32 = 0xfffff; // phy.h:179`

```
// ── main.h:32-33, phy.h:179-197 ──────────────────────────────────
```

## L21 · `pub const RFREG_MASK: u32 = 0xfffff; // phy.h:179`

```
// phy.h:179
```

## L22 · `pub const INV_RF_DATA: u32 = 0xffffffff; // main.h:33`

```
// main.h:33
```

## L23 · `pub const LSSI_READ_ADDR_MASK: u32 = 0x7f800000; // phy.h:195`

```
// phy.h:195
```

## L24 · `pub const LSSI_READ_EDGE_MASK: u32 = 0x80000000; // phy.h:196`

```
// phy.h:196
```

## L25 · `pub const LSSI_READ_DATA_MASK: u32 = 0xfffff; // phy.h:197`

```
// phy.h:197
```

## L27 · `pub const RF_PATH_A: usize = 0; // main.h:135`

```
// main.h:135
```

## L28 · `pub const RF_PATH_B: usize = 1; // main.h:136`

```
// main.h:136
```

## L30 · `const RF_BASE_ADDR: [u32; 2] = [0x3c00, 0x4c00];`

```
/// rtw8822c.c:5375 `.rf_base_addr` — der direkte Fensterzugang je Pfad.
```

## L32 · `const RF_SIPI_ADDR: [u32; 2] = [0x1808, 0x4108];`

```
/// rtw8822c.c:5376 `.rf_sipi_addr` — der serielle Weg, nur fuer Register 0.
```

## L35 · `const INTF_PCIE: u32 = 1 << 0;`

```
// main.h:1862-1869 — die Felder von `struct rtw_phy_cond`.
```

## L42-46 · `#[derive(Clone, Copy, Default)]`

```
/// main.h:1839-1860 `struct rtw_phy_cond`, als das Wort, das es ist.
///
/// In C ist es ein Bitfeld ueber `u32`; hier steht es als Zahl da, weil die
/// Tabelle genau diese Zahl enthaelt. Die Zerlegung ist Little-Endian —
/// `rfe` liegt unten, `pos` ganz oben.
```

## L61-69 · `pub fn setup_phy_cond(cut_version: u8, rfe_option: u8) -> PhyCond {`

```
/// phy.c:1083-1128 `rtw_phy_setup_phy_cond`, PCIe-Zweig.
///
/// `pkg` kommt in Linux aus `hal->pkg_type` — und **dieses Feld wird
/// nirgends beschrieben**, im ganzen Treiber nicht. Es ist also immer 0, und
/// damit greift `pkg ? pkg : 15` und die Bedingung lautet 15. Das steht hier,
/// weil es sonst wie ein vergessener Wert aussieht.
///
/// Der 8812A/8821A-Zweig (rfe aus ext_lna/ext_pa/btcoex zusammengesetzt,
/// cond2 aus den LNA/PA-Typen) gilt fuer andere Chips.
```

## L72 · `let pkg = 15u32; // hal->pkg_type ist 0, siehe oben`

```
// hal->pkg_type ist 0, siehe oben
```

## L79-84 · `fn check_positive(cond: PhyCond, drv: PhyCond) -> bool {`

```
/// phy.c:1130-1171 `check_positive`, Zweig fuer alles ausser 8812A/8821A.
///
/// `cut`, `pkg` und `intf` gelten nur, wenn die Tabelle sie NENNT (0 heisst
/// „egal"). `rfe` wird dagegen IMMER verglichen — auch auf 0. Das ist keine
/// Unachtsamkeit in Linux, sondern die Regel, mit der `rfe_option 0` von
/// `rfe_option 1` getrennt wird.
```

## L98-100 · `#[derive(Clone, Copy, PartialEq)]`

```
/// Welche der vier `do_cfg`-Funktionen eine Tabelle benutzt.
/// In Linux ein Funktionszeiger in `struct rtw_table`; hier eine Art, weil
/// `rtw_phy_cfg_rf` zusaetzlich den Pfad der Tabelle braucht.
```

## L109-113 · `pub fn parse_tbl_phy_cond(h: i32, data: &[u32], cfg: Cfg, drv: PhyCond) -> u32 {`

```
/// phy.c:1169-1220 `rtw_parse_tbl_phy_cond`.
///
/// Die Tabelle ist eine Folge von Kacheln zu zwei Woertern. Ein Wort mit
/// Bit 31 ist der KOPF eines Bedingungsblocks (`#if`/`#elif`/`#else`/
/// `#endif`), eines mit Bit 30 sein Ende, alles andere ist Adresse und Wert.
```

## L135 · `_ => {`

```
// BRANCH_IF, BRANCH_ELIF und alles andere
```

## L137-139 · `let _ = BRANCH_IF;`

```
// `cond2` (das zweite Wort) traegt nur fuer 8812A/8821A
// Inhalt — die LNA/PA-Typen. Hier ist es ungenutzt, und
// das ist der Grund, warum es hier nicht mitgefuehrt wird.
```

## L165 · `fn do_cfg(h: i32, cfg: Cfg, addr: u32, data: u32) {`

```
/// phy.c:1782-1830 — die vier `rtw_phy_cfg_*`, hinter EINER Verzweigung.
```

## L168 · `Cfg::Mac => host::w8(h, addr, data as u8),`

```
// phy.c:1782 `rtw_phy_cfg_mac`
```

## L170 · `Cfg::Agc => host::w32(h, addr, data),`

```
// phy.c:1789 `rtw_phy_cfg_agc`
```

## L172-173 · `Cfg::Bb => match addr {`

```
// phy.c:1796 `rtw_phy_cfg_bb` — sechs Adressen sind Pausen, keine
// Register. Unter einer Millisekunde wird gedreht statt geschlafen.
```

## L183 · `Cfg::Rf(path) => match addr {`

```
// phy.c:1815 `rtw_phy_cfg_rf`
```

## L195 · `pub fn read_rf(h: i32, rf_path: usize, addr: u32, mask: u32) -> u32 {`

```
// ── RF-Zugriff (phy.c:937-1081) ──────────────────────────────────
```

## L197-198 · `pub fn read_rf(h: i32, rf_path: usize, addr: u32, mask: u32) -> u32 {`

```
/// phy.c:937-987 `rtw_phy_read_rf` — der 8822C liest DIREKT, ueber ein
/// Fenster je Pfad (`chip->ops->read_rf = rtw_phy_read_rf`).
```

## L208 · `fn write_rf_reg(h: i32, rf_path: usize, addr: u32, mask: u32, data: u32) -> bool {`

```
/// phy.c:1047-1070 `rtw_phy_write_rf_reg`
```

## L220-224 · `fn write_rf_reg_sipi(h: i32, rf_path: usize, addr: u32, mask: u32, data: u32) -> bool {`

```
/// phy.c:1009-1046 `rtw_phy_write_rf_reg_sipi`.
///
/// Beim 8822C nur fuer Register 0 erreichbar (siehe `write_rf_reg_mix`).
/// `mask != RFREG_MASK` fuehrt vorher einen Lesezugriff — der geht ueber
/// `chip->ops->read_rf`, also ueber den DIREKTEN Weg, nicht ueber SIPI.
```

## L249 · `pub fn write_rf_reg_mix(h: i32, rf_path: usize, addr: u32, mask: u32, data: u32) -> bool {`

```
/// phy.c:1072-1081 `rtw_phy_write_rf_reg_mix` — der Schreibweg des 8822C.
```

## L257 · `fn load_rfk_table(h: i32, drv: PhyCond) -> u32 {`

```
// ── Die Tabellen laden (phy.c:1832-1871) ─────────────────────────
```

## L259-262 · `fn load_rfk_table(h: i32, drv: PhyCond) -> u32 {`

```
/// phy.c:1832-1848 `rtw_load_rfk_table`.
///
/// Die fuenf Schreibzugriffe davor stehen ohne Kommentar in Linux; sie
/// schalten den DPK-Block an, bevor seine Initialtabelle laeuft.
```

## L270-271 · `parse_tbl_phy_cond(h, &tables::RFK_INIT, Cfg::Bb, drv)`

```
// `dpk_info->is_dpk_pwr_on = true` ist reiner Treiberzustand und wird
// erst von der DPK-Kalibrierung gelesen — ein Posten der Stufe 4.
```

## L275-284 · `pub fn load_tables(h: i32, rf_path_num: u8, drv: PhyCond) -> bool {`

```
/// phy.c:1850-1871 `rtw_phy_load_tables`.
///
/// **Die Reihenfolge der RF-Tabellen ist umgekehrt zu ihrem Namen** und das
/// ist kein Tippfehler: `rtw8822c_hw_spec` sagt
/// `.rf_tbl = {&rtw8822c_rf_b_tbl, &rtw8822c_rf_a_tbl}` (rtw8822c.c:5382),
/// die Schleife laeuft ueber den Index, und jede Tabelle traegt ihren Pfad
/// selbst. Geladen wird also erst B, dann A.
///
/// Gibt zurueck, ob jede Tabelle so viele Schreibzugriffe abgegeben hat, wie
/// `gen_tables.py` fuer DIESEN Chipzustand vorausgerechnet hat.
```

## L286-289 · `let reference = setup_phy_cond(crate::regs::RTW_CHIP_VER_CUT_D, 1);`

```
// Die Vorausrechnung gilt fuer cut D und rfe_option 1 — das Geraet, an
// dem gemessen wurde. Steht dort etwas anderes, wird nur gezaehlt und
// nicht verglichen; eine Zahl gegen die falsche Erwartung zu halten
// waere schlimmer als gar keine.
```

## L327-329 · `check("rfk_init", load_rfk_table(h, drv));`

```
// `rfe_def->agc_btg_tbl` — der 8822C benutzt `RTW_DEF_RFE` ohne btg
// (rtw8822c.c:5277-5285), das Feld ist also NULL und Linux ueberspringt
// den Aufruf. Es gibt hier nichts zu laden.
```

## L333 · `let rf_tbl: [(&[u32], usize, &str); 2] = [`

```
// rf_tbl[0] = rf_b, rf_tbl[1] = rf_a — siehe oben.
```

## L345 · `use crate::dm::{DmInfo, PathDiv, EWMA_THERMAL_PRECISION, EWMA_THERMAL_WEIGHT_RCP};`

```
// ── Stufe 3c: rtw_phy_init (phy.c:236-261) ───────────────────────
```

## L349-350 · `const DIG: [(u32, u32); 2] = [(0x1d70, 0x7f), (0x1d70, 0x7f00)];`

```
/// phy.c:1673-1674 · rtw8822c.c:4906-4909 `rtw8822c_dig` — Adresse und
/// Maske des IGI-Felds je Pfad. Beide liegen in DEMSELBEN Register.
```

## L353 · `fn cck_pd_init(dm: &mut DmInfo) {`

```
/// phy.c:1600-1611 `rtw_phy_cck_pd_init`. Reiner Treiberzustand.
```

## L355 · `for i in 0..=1usize {`

```
// `i <= RTW_CHANNEL_WIDTH_40` sind die Breiten 20 und 40, also zwei.
```

## L358 · `dm.cck_pd_lv[i][j] = 0; // CCK_PD_LV0 (phy.h:164)`

```
// CCK_PD_LV0 (phy.h:164)
```

## L363 · `fn adaptivity_set_mode(dm: &mut DmInfo) {`

```
/// phy.h:193
```

## L365-371 · `fn adaptivity_set_mode(dm: &mut DmInfo) {`

```
/// phy.c:175-200 `rtw_phy_adaptivity_set_mode`.
///
/// `rtwdev->regd.dfs_region` steht vor der ersten Kanalwahl auf
/// `NL80211_DFS_UNSET`, und der `default`-Zweig setzt dann `RTW_EDCCA_NORMAL`
/// ohne `l2h_th_ini`. Die zwei Sonderfaelle (ETSI, Japan) brauchen eine
/// Regulierungszone, die es hier noch nicht gibt — sie kommen mit
/// `rtw_regd_init`, und das ist die Stufe, die einen Kanal setzt.
```

## L373 · `dm.edcca_mode = 0; // RTW_EDCCA_NORMAL (main.h:1683)`

```
// RTW_EDCCA_NORMAL (main.h:1683)
```

## L376 · `pub fn set_edcca_th(h: i32, l2h: u8, h2l: u8) {`

```
/// phy.c:2245-2257 `rtw_phy_set_edcca_th`
```

## L384-390 · `pub fn adaptivity(h: i32, dm: &DmInfo) {`

```
/// rtw8822c.c:2178-2196 `rtw8822c_adaptivity` — der Chip-Zweig von
/// `rtw_phy_adaptivity`, alle zwei Sekunden.
///
/// `set_edcca_th` nimmt vorzeichenlose Werte, Linux rechnet in `s8`.
/// Beide Schwellen sind nach der Rechnung positiv (`EDCCA_TH_L2H_LB` = 48
/// ist die Untergrenze, `h2l` liegt sieben bis acht darunter), also ist
/// die Umdeutung hier ein Wechsel der Darstellung und keine Klemmung.
```

## L395 · `l2h = igi.saturating_add(EDCCA_IGI_L2H_DIFF).max(EDCCA_TH_L2H_LB);`

```
// RTW_EDCCA_NORMAL
```

## L409-410 · `fn adaptivity_init(h: i32, dm: &mut DmInfo) {`

```
/// phy.c:202-209 `rtw_phy_adaptivity_init` + `chip->ops->adaptivity_init`
/// = rtw8822c.c `rtw8822c_adaptivity_init`.
```

## L415 · `host::clr32(h, REG_TX_PTCL_CTRL, BIT_DIS_EDCCA);`

```
// mac edcca state setting
```

## L418 · `host::clr32(h, REG_EDCCA_DECISION, BIT_EDCCA_OPTION);`

```
// edcca decistion opt
```

## L422-425 · `pub fn phy_init(h: i32, dm: &mut DmInfo, path_div: &mut PathDiv,`

```
/// phy.c:236-261 `rtw_phy_init`.
///
/// Die Verzweigungen ueber `chip->ops` sind hier ausgeschrieben: der 8822C
/// fuehrt `adaptivity_init` und `cfo_init`, also gelten beide.
```

## L441 · `dm.cfo_track.crystal_cap = crystal_cap;`

```
// rtw8822c.c:4257-4263 `rtw8822c_cfo_init`
```

## L445 · `path_div.current_tx_path = default_1ss_tx_path;`

```
// phy.c:222-234 `rtw_phy_tx_path_div_init`
```

## L453-461 · `pub fn get_rssi_level(old_level: u8, rssi: u8) -> u8 {`

```
// ═══════════════════════════════════════════════════════════════════
// rtw_watch_dog_work — die LAUFENDE Haelfte (main.c:224-310)
//
// Linux fuehrt sie alle zwei Sekunden, das ganze Leben einer Verbindung
// lang. Bis 0.26.0 gab es sie hier nicht: gebaut war der Aufbau, und der
// Chip blieb danach sich selbst ueberlassen. Drei ihrer Posten sind
// SENDEseite und haengen an der Temperatur — und ein Empfaenger rastet
// sich an jeder Praeambel neu ein, ein Sender nicht.
// ═══════════════════════════════════════════════════════════════════
```

## L463 · `pub fn get_rssi_level(old_level: u8, rssi: u8) -> u8 {`

```
/// phy.c:292-309 `rtw_phy_get_rssi_level`
```

## L484 · `pub fn stat_rate_cnt(dm: &mut DmInfo) {`

```
/// phy.c:318-321 `rtw_phy_stat_rate_cnt`
```

## L490-494 · `fn dig_check_damping(dm: &mut DmInfo) -> bool {`

```
/// phy.c:335-371 `rtw_phy_dig_check_damping`.
///
/// **Die Daempfungsbremse.** Sie erkennt, dass die Verstaerkungsregelung
/// zwischen zwei Werten hin und her springt, und laesst sie dann in Ruhe
/// — `igi_bitmap` traegt die Richtung der letzten vier Schritte als Bits.
```

## L508-509 · `let cnt = dm.damping_cnt;`

```
// Linux: `dm_info->damping_cnt++ > 20` — NACHzaehlend, der
// Vergleich sieht also den Wert VOR dem Erhoehen.
```

## L522 · `5 => {`

```
// runter -> rauf -> runter -> rauf
```

## L532 · `9 => {`

```
// rauf -> runter -> runter -> rauf
```

## L554 · `fn dig_get_boundary(dm: &DmInfo, linked: bool) -> (u8, u8) {`

```
/// phy.c:373-395 `rtw_phy_dig_get_boundary` — gibt `(upper, lower)`.
```

## L569 · `dig_max = dig_max.min(min_rssi.saturating_add(DIG_RSSI_GAIN_OFFSET as u8));`

```
// „DIG MAX should be bounded by minimum RSSI with offset +15"
```

## L578 · `fn dig_get_threshold(dm: &DmInfo, linked: bool) -> ([u16; 3], [u8; 3]) {`

```
/// phy.c:397-417 `rtw_phy_dig_get_threshold` — gibt `(fa_th, step)`.
```

## L599 · `fn dig_recorder(dm: &mut DmInfo, igi: u8, fa: u16) {`

```
/// phy.c:419-441 `rtw_phy_dig_recorder`
```

## L618-622 · `pub fn dig_write(h: i32, rf_path_num: u8, igi: u8) {`

```
/// phy.c:443-459 `rtw_phy_dig_write`.
///
/// `rtw8822c` fuehrt `.dig_cck = NULL` (rtw8822c.c:5374), der CCK-Zweig
/// entfaellt also — er steht hier als Kommentar und nicht als Code, weil
/// ein toter Zweig sonst wie eine Auslassung aussieht.
```

## L630-633 · `pub fn dig(h: i32, dm: &mut DmInfo, rf_path_num: u8, linked: bool) {`

```
/// phy.c:461-518 `rtw_phy_dig`.
///
/// `linked` ist Linux' `!!rtwdev->sta_cnt`. Der 8812A-Sonderfall am Ende
/// gilt fuer einen anderen Chip und steht deshalb nicht hier.
```

## L635 · `if dig_check_damping(dm) {`

```
// `RTW_FLAG_DIG_DISABLE` setzt bei uns niemand — der Test entfaellt.
```

## L645-647 · `let mut cur_igi = pre_igi;`

```
// „test the false alarm count from the highest threshold level first,
//  and increase it by corresponding step size — note that the step
//  size is offset by -2, compensate it afterall"
```

## L667 · `fn cck_pd_lv_unlink(dm: &DmInfo) -> u8 {`

```
/// phy.c:705-717 `rtw_phy_cck_pd_lv_unlink`
```

## L678 · `fn cck_pd_lv_link(dm: &DmInfo) -> u8 {`

```
/// phy.c:719-735 `rtw_phy_cck_pd_lv_link`
```

## L701 · `fn cck_pd_lv(dm: &DmInfo, linked: bool) -> u8 {`

```
/// phy.c:737-743 `rtw_phy_cck_pd_lv`
```

## L710-713 · `pub fn cck_pd(h: i32, dm: &mut DmInfo, band_2g: bool, linked: bool) {`

```
/// phy.c:745-772 `rtw_phy_cck_pd`.
///
/// Nur auf 2,4 GHz — auf 5 GHz gibt es kein CCK, und der Zweig steht in
/// Linux genauso weit oben.
```

## L734 · `pub fn get_rrsr_mask(rate_idx: u8) -> u32 {`

```
/// phy.c:1021-1049 `rtw_phy_get_rrsr_mask`
```

## L764 · `let hi = rate_order as u32 + RRSR_RATE_ORDER_CCK_LEN - 1;`

```
// GENMASK(rate_order + RRSR_RATE_ORDER_CCK_LEN - 1, 0)
```

## L769-772 · `pub fn rrsr_update(h: i32, dm: &mut DmInfo, sta_rate: Option<u8>) {`

```
/// phy.c:1062-1069 `rtw_phy_rrsr_update`.
///
/// Mit EINER Station ist der Iterator ein Aufruf; `rate` ist ihr
/// `ra_report.desc_rate`.
```

## L784 · `fn set_tx_path_by_reg(h: i32, path_div: &mut PathDiv, antenna_tx: u8,`

```
/// phy.c:1918-1930 `rtw_phy_set_tx_path_by_reg`
```

## L791-792 · `crate::chip::config_tx_path(h, antenna_tx, tx_path_sel_1ss,`

```
// `chip->ops->config_tx_path(…, tx_path_sel_1ss, tx_path_sel_cck, false)`
// — beide Auswahlen sind derselbe Wert (phy.c:1921).
```

## L797 · `fn tx_path_div_select(h: i32, path_div: &mut PathDiv, antenna_tx: u8) {`

```
/// phy.c:1932-1955 `rtw_phy_tx_path_div_select`
```

## L823-826 · `pub fn tx_path_diversity(h: i32, path_div: &mut PathDiv, antenna_tx: u8,`

```
/// phy.c:1957-1972 `rtw_phy_tx_path_diversity_2ss` + phy.c:1974-1982
/// `rtw_phy_tx_path_diversity`.
///
/// `rtw8822c` fuehrt `.path_div_supported = true` (rtw8822c.c:5362).
```

## L841 · `pub fn pwrtrack_avg(dm: &mut DmInfo, thermal: u8, path: usize) {`

```
// ── rtw_phy_pwr_track und seine Helfer ───────────────────────────
```

## L843 · `pub fn pwrtrack_avg(dm: &mut DmInfo, thermal: u8, path: usize) {`

```
/// phy.c `rtw_phy_pwrtrack_avg`
```

## L851 · `pub fn pwrtrack_thermal_changed(dm: &DmInfo, thermal: u8, path: usize) -> bool {`

```
/// phy.c `rtw_phy_pwrtrack_thermal_changed`
```

## L856-857 · `pub fn pwrtrack_get_delta(dm: &DmInfo, thermal_meter: &[u8],`

```
/// phy.c `rtw_phy_pwrtrack_get_delta` — der Abstand zur efuse, gedeckelt
/// auf die Tabellenlaenge.
```

## L870 · `pub fn pwrtrack_need_lck(dm: &mut DmInfo) -> bool {`

```
/// phy.c `rtw_phy_pwrtrack_need_lck`
```

## L882 · `pub fn pwrtrack_need_iqk(dm: &mut DmInfo) -> bool {`

```
/// phy.c `rtw_phy_pwrtrack_need_iqk`
```

## L894-896 · `pub struct SwingTable {`

```
/// Die vier Kurvenpaare, die `rtw_phy_config_swing_table` fuer den
/// aktuellen Kanal und die aktuelle Rate auswaehlt: je Pfad ein `p` und
/// ein `n`.
```

## L902-906 · `pub fn config_swing_table(channel: u8, tx_rate: u8) -> SwingTable {`

```
/// phy.c `rtw_phy_config_swing_table`.
///
/// **Fuer den 8822C gibt es genau EINE Tabelle**: alle sieben
/// RFE-Varianten zeigen auf `type0` (rtw8822c.c:5277-5285). Eine Auswahl
/// nach RFE waere hier eine erfundene Verzweigung.
```

## L908 · `if channel <= 14 {`

```
// IS_CH_2G_BAND(channel) — Kanal 1..14
```

## L922 · `let i = if channel <= 64 { 0 } else if channel <= 144 { 1 } else { 2 };`

```
// IS_CH_5G_BAND_1/2 -> Index 0, BAND_3 -> 1, BAND_4 -> 2.
```

## L931-932 · `pub fn pwrtrack_get_pwridx(dm: &DmInfo, swing: &SwingTable,`

```
/// phy.c `rtw_phy_pwrtrack_get_pwridx` — waermer als die efuse heisst
/// nach oben, kaelter nach unten.
```

## L946-950 · `pub fn statistics(h: i32, dm: &mut DmInfo, st: &mut crate::fw::H2cState,`

```
/// phy.c:311-316 `rtw_phy_statistics`.
///
/// Der `rssi`-Zweig ist bei Linux ein Stationen-Iterator; wir fahren
/// EINE Gegenstelle, also ist er ein `Option`. Ohne Verbindung bleibt
/// `min_rssi` auf `U8_MAX` stehen — genau wie dort.
```

## L953 · `let mut min_rssi = u8::MAX;`

```
// rtw_phy_stat_rssi
```

## L964 · `crate::chip::false_alarm_statistics(h, dm);`

```
// rtw_phy_stat_false_alarm -> chip->ops->false_alarm_statistics
```

## L967 · `stat_rate_cnt(dm);`

```
// rtw_phy_stat_rate_cnt
```

## L971-972 · `fn ra_info_update(h: i32, st: &mut crate::fw::H2cState, watch_dog_cnt: u32,`

```
/// phy.c:1051-1060 `rtw_phy_ra_info_update` — **nur jeden VIERTEN Takt**
/// (`watch_dog_cnt & 0x3`), also alle acht Sekunden.
```

## L980 · `crate::sta::update_sta_info(si, caps, nss, band_2g);`

```
// `rtw_update_sta_info(rtwdev, si, false)`
```

## L986 · `#[allow(clippy::too_many_arguments)]`

```
/// phy.c:1071-1076 `rtw_phy_ra_track`
```

## L994-995 · `if let Some((_, _, _, band_2g)) = &si {`

```
// main.c:1266/1286 — der Teil von `rtw_update_sta_info`, der `dm`
// schreibt: die Grundmenge der Antwortraten je Band.
```

## L1003-1008 · `#[allow(clippy::too_many_arguments)]`

```
/// phy.c:791-806 `rtw_phy_dynamic_mechanism` — die neun Posten in
/// Linux' Reihenfolge.
///
/// Alles, was hier `dm` liest, hat der Empfangsweg zwischen zwei Takten
/// gefuellt (`rx::watchdog_feed`). Laeuft der nicht, rechnet der ganze
/// Block auf Nullen und meldet nichts.
```

