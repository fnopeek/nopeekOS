# `tools/wasm/wifi_rtl8822ce/src/txgapk.rs` @ 5e0102684

## L1-11 · `#![allow(dead_code)]`

```
//! `rtw8822c_txgapk` — die Sendeverstaerkungs-Kalibrierung (Stufe 5d),
//! rtw8822c.c:1191-1823.
//!
//! Sie misst je Verstaerkungsstufe den Abstand zwischen dem, was die
//! Verstaerkungstabelle des RF verspricht, und dem, was herauskommt, und
//! schreibt die Tabelle danach korrigiert zurueck.
//!
//! **Zwei Ausstiege stehen ganz vorn**, und beide gehoeren zur Funktion:
//! ohne gelesene Verstaerkungstabelle (`read_txgain == 0`) gibt es nichts
//! zu korrigieren, und bei `power_track_type` 4..7 regelt der Chip seine
//! Sendeleistung ueber TSSI — dann waere die Korrektur doppelt.
```

## L21 · `const PATHS: usize = 4; // RTW_RF_PATH_MAX`

```
// RTW_RF_PATH_MAX
```

## L23-25 · `pub struct GapkInfo {`

```
/// main.h:1663-1671 `struct rtw_gapk_info`. Der Tippfehler `fianl_offset`
/// steht in Linux so da und bleibt hier stehen — wer danach sucht, sucht
/// mit dem Namen, den die Quelle traegt.
```

## L61 · `fn backup_bb_reg(h: i32, reg: &[u32], backup: &mut [u32]) {`

```
/// rtw8822c.c:1191-1202 `rtw8822c_txgapk_backup_bb_reg`
```

## L68 · `fn reload_bb_reg(h: i32, reg: &[u32], backup: &[u32]) {`

```
/// rtw8822c.c:1204-1215 `rtw8822c_txgapk_reload_bb_reg`
```

## L75-78 · `fn check_rf_status(h: i32, status: u32) -> bool {`

```
/// rtw8822c.c:1217-1229 `check_rf_status`.
///
/// Wahr, wenn KEIN Pfad mehr im gefragten Zustand steht — der Name klingt
/// nach dem Gegenteil, und so steht er in Linux.
```

## L85 · `fn tx_pause(h: i32) -> bool {`

```
/// rtw8822c.c:1232-1246 `rtw8822c_txgapk_tx_pause`
```

## L90 · `let t0 = host::now_us();`

```
// `read_poll_timeout_atomic(check_rf_status, status, status, 2, 5000, …)`
```

## L104 · `fn bb_dpk(h: i32, path: usize) {`

```
/// rtw8822c.c:1248-1278 `rtw8822c_txgapk_bb_dpk`
```

## L127-130 · `fn afe_dpk(h: i32, path: usize) {`

```
/// rtw8822c.c:1280-1314 `rtw8822c_txgapk_afe_dpk`.
///
/// Achtzehn Schreibzugriffe, und der erste Wert steht ZWEIMAL da. Das ist
/// kein Kopierfehler von mir — es steht in Linux so, und der letzte ebenso.
```

## L151 · `fn afe_dpk_restore(h: i32, path: usize) {`

```
/// rtw8822c.c:1316-1347 `rtw8822c_txgapk_afe_dpk_restore`
```

## L172 · `fn bb_dpk_restore(h: i32, path: usize) {`

```
/// rtw8822c.c:1349-1387 `rtw8822c_txgapk_bb_dpk_restore`
```

## L205 · `fn gain_valid(gain: u32) -> bool {`

```
/// rtw8822c.c:1389-1396 `_rtw8822c_txgapk_gain_valid`
```

## L210 · `fn write_gain_bb_table_one(h: i32, g: &GapkInfo, band: usize, path: usize) {`

```
/// rtw8822c.c:1398-1450 `_rtw8822c_txgapk_write_gain_bb_table`
```

## L224-225 · `let mut tmp_3f = 0u32;`

```
// `tmp_3f` wird NICHT je Durchlauf zurueckgesetzt: ist eine Stufe
// gueltig, behaelt sie den Wert der letzten ungueltigen. So steht es da.
```

## L246 · `fn write_gain_bb_table(h: i32, g: &GapkInfo, rf_path_num: u8) {`

```
/// rtw8822c.c:1452-1465 `rtw8822c_txgapk_write_gain_bb_table`
```

## L255 · `fn read_offset(h: i32, g: &mut GapkInfo, path: usize) {`

```
/// rtw8822c.c:1467-1542 `rtw8822c_txgapk_read_offset`
```

## L291-292 · `let t0 = host::now_us();`

```
// `read_poll_timeout(…, val == 0x55, 1000, 100000, …)` — Linux prueft
// den Rueckgabewert NICHT; die Zeit ist der ganze Zweck.
```

## L323 · `for i in 0..RF_HW_OFFSET_NUM_U {`

```
// Vorzeichen aus vier Bit: Bit 3 gesetzt heisst negativ.
```

## L331 · `fn calculate_offset(h: i32, g: &mut GapkInfo, path: usize) {`

```
/// rtw8822c.c:1544-1616 `rtw8822c_txgapk_calculate_offset`
```

## L390 · `fn rf_restore(h: i32, path: usize, rf_path_num: u8) {`

```
/// rtw8822c.c:1618-1628 `rtw8822c_txgapk_rf_restore`
```

## L400 · `fn cal_gain(gain: u32, offset: i8) -> u32 {`

```
/// rtw8822c.c:1630-1652 `rtw8822c_txgapk_cal_gain`
```

## L405-406 · `let gain_x2 = (gain << 1).wrapping_add(offset as i32 as u32);`

```
// `(gain << 1) + offset` in u32-Arithmetik, wie in C: ein negatives
// `offset` wird zur grossen Zahl und zieht beim Addieren ab.
```

## L411 · `fn write_tx_gain(h: i32, g: &mut GapkInfo, rf_path_num: u8) {`

```
/// rtw8822c.c:1654-1722 `rtw8822c_txgapk_write_tx_gain`
```

## L455 · `fn save_all_tx_gain_table(h: i32, g: &mut GapkInfo, rf_path_num: u8,`

```
/// rtw8822c.c:1724-1781 `rtw8822c_txgapk_save_all_tx_gain_table`
```

## L463-464 · `if dm_flags & (1 << RTW_DM_CAP_TXGAPK) != 0 {`

```
// `BIT(RTW_DM_CAP_TXGAPK)` GESETZT heisst ABGESCHALTET — die Pruefung
// ist umgekehrt, als der Name vermuten laesst.
```

## L503 · `#[derive(Clone, Copy, PartialEq)]`

```
/// Warum TXGAPK nicht gelaufen ist — oder dass es lief.
```

## L512 · `pub fn txgapk(h: i32, g: &mut GapkInfo, rf_path_num: u8, dm_flags: u32,`

```
/// rtw8822c.c:1783-1823 `rtw8822c_txgapk`
```

## L528-529 · `if (4..=7).contains(&power_track_type) {`

```
// „Normal Mode in TSSI mode. return!!!" — der Chip regelt seine
// Sendeleistung dann selbst, eine zweite Korrektur waere doppelt.
```

