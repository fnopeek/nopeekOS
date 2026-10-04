# `tools/wasm/wifi_rtl8822ce/src/rfk.rs` @ 5e0102684

## L1-24 · `#![allow(dead_code)]`

```
//! `rtw8822c.c`, Zeilen 108-1010 — die DAC-Kalibrierung (DACK).
//!
//! Portiert, in Quellreihenfolge: `rtw8822c_dac_backup_reg` ·
//! `rtw8822c_dac_restore_reg` · `rtw8822c_rf_minmax_cmp` ·
//! `__rtw8822c_dac_iq_sort` · `rtw8822c_dac_iq_sort` ·
//! `rtw8822c_dac_iq_offset` · `rtw8822c_get_path_write_addr` ·
//! `rtw8822c_get_path_read_addr` · `rtw8822c_dac_iq_check` ·
//! `rtw8822c_dac_cal_iq_sample` · `rtw8822c_dac_cal_iq_search` ·
//! `rtw8822c_dac_cal_rf_mode` · `rtw8822c_dac_bb_setting` ·
//! `rtw8822c_dac_cal_adc` · `rtw8822c_dac_cal_step1..4` ·
//! `rtw8822c_dac_cal_backup_vec/_path/_dck/_backup` ·
//! `rtw8822c_dac_cal_restore_dck/_prepare/_wait/_path` ·
//! `__rtw8822c_dac_cal_restore` · `rtw8822c_dac_cal_restore` ·
//! `rtw8822c_rf_dac_cal`.
//!
//! **Was die Kalibrierung tut:** sie misst den Gleichspannungsversatz von
//! ADC und DAC beider Pfade und traegt den Ausgleich in die Hardware ein.
//! Ohne sie steht auf jedem Empfangspfad ein konstanter Fehler.
//!
//! **`dac_cal_restore` greift bei uns nie.** Es rettet das Ergebnis eines
//! FRUEHEREN Laufs ueber ein Aus- und Wiedereinschalten — `dack_msbk` ist
//! beim ersten Lauf null, und die Funktion steigt genau daran aus. Portiert
//! ist sie trotzdem vollstaendig: sobald der Treiber den Chip ein zweites
//! Mal anwirft, spart sie die ganze Messung.
```

## L32 · `fn wrf(h: i32, path: usize, addr: u32, val: u32) {`

```
/// `rtw_write_rf(..., RFREG_MASK, v)` — die Form, in der die DACK schreibt.
```

## L37-38 · `#[derive(Clone, Copy, Default)]`

```
/// `struct rtw_backup_info` (main.h), wie in `mac.rs` — aber hier sind alle
/// Eintraege 4 Byte breit, also braucht es kein `len`.
```

## L45 · `const DACK_ADDRS: [u32; DACK_REG_8822C] = [`

```
/// rtw8822c.c:115-120 — die sechzehn BB-Register, die die DACK verstellt.
```

## L53 · `const DACK_RF_ADDRS: [u32; DACK_RF_8822C] = [0x8f];`

```
/// rtw8822c.c:115 — `u32 rf_addr[DACK_RF_8822C] = {0x8f};`
```

## L56-64 · `fn dac_backup_reg(h: i32) -> ([Backup; DACK_REG_8822C],`

```
/// rtw8822c.c:108-135 `rtw8822c_dac_backup_reg`.
///
/// **Der Index `backup_rf[path * i + i]` ist Linux' eigener und er ist
/// schraeg** — mit `DACK_RF_8822C == 1` laeuft `i` nur ueber 0, also ist
/// `path * 0 + 0` fuer BEIDE Pfade die 0. Der zweite Pfad ueberschreibt den
/// ersten, und `restore_reg` liest mit demselben Ausdruck zurueck. Der
/// Ausdruck steht hier unveraendert: er ist harmlos, solange
/// `DACK_RF_8822C` 1 ist, und ihn stillschweigend zu „reparieren" hiesse,
/// eine andere Reihenfolge zu schreiben als Linux fuehrt.
```

## L86 · `fn dac_restore_reg(h: i32, backup: &[Backup; DACK_REG_8822C],`

```
/// rtw8822c.c:137-152 `rtw8822c_dac_restore_reg`
```

## L89 · `for b in backup.iter() {`

```
// util.c `rtw_restore_reg`, hier durchgehend 4 Byte.
```

## L102-106 · `fn rf_minmax_cmp(value: u32, min: &mut u32, max: &mut u32) {`

```
/// rtw8822c.c:154-183 `rtw8822c_rf_minmax_cmp`.
///
/// Die Werte sind Zehn-Bit-Zweierkomplement: alles ab 0x200 ist negativ.
/// Deshalb ist „kleiner" hier nicht die Zahlenordnung, und deshalb sieht
/// diese Funktion so aus, wie sie aussieht.
```

## L131 · `fn dac_iq_sort_pair(v1: &mut u32, v2: &mut u32) {`

```
/// rtw8822c.c:185-196 `__rtw8822c_dac_iq_sort`
```

## L142 · `fn dac_iq_sort(iv: &mut [u32; DACK_SN_8822C], qv: &mut [u32; DACK_SN_8822C]) {`

```
/// rtw8822c.c:198-209 `rtw8822c_dac_iq_sort` — Bubblesort ueber beide Felder.
```

## L154-155 · `fn dac_iq_offset(vec: &[u32; DACK_SN_8822C]) -> u32 {`

```
/// rtw8822c.c:211-234 `rtw8822c_dac_iq_offset` — der Mittelwert der
/// mittleren 80 von 100 Proben, wieder im Zehn-Bit-Zweierkomplement.
```

## L175 · `fn path_write_addr(path: usize) -> u32 {`

```
/// rtw8822c.c:236-253 `rtw8822c_get_path_write_addr`
```

## L184 · `fn path_read_addr(path: usize) -> u32 {`

```
/// rtw8822c.c:255-272 `rtw8822c_get_path_read_addr`
```

## L193-194 · `fn dac_iq_check(value: u32) -> bool {`

```
/// rtw8822c.c:274-285 `rtw8822c_dac_iq_check` — eine Probe, deren Betrag
/// ueber 0x64 liegt, ist ein Ueberlauf und wird verworfen.
```

## L199-202 · `fn dac_cal_iq_sample(h: i32, iv: &mut [u32; DACK_SN_8822C],`

```
/// rtw8822c.c:287-302 `rtw8822c_dac_cal_iq_sample`.
///
/// Der Deckel von 10000 ist Linux'; ohne ihn haengt die Schleife, wenn die
/// Hardware nur Ueberlaeufe liefert.
```

## L217-219 · `let (mut imin, mut imax, mut qmin, mut qmax) = (iv[0], iv[0], qv[0], qv[0]);`

```
// Die ROHEN Proben, bevor irgendetwas daraus gerechnet wird. Ein
// Feld aus 100 gleichen Zahlen ist eine eingefrorene Messung; eine
// echte streut ([[feedback_dump_the_raw_input_before_debugging_the_interpretation]]).
```

## L241 · `fn dac_cal_iq_search(h: i32, iv: &mut [u32; DACK_SN_8822C],`

```
/// rtw8822c.c:304-360 `rtw8822c_dac_cal_iq_search`
```

## L283 · `if cnt >= 100 {`

```
// `while (cnt++ < 100)` — nachgestellte Erhoehung, also 101 Runden.
```

## L293-297 · `fn dac_cal_rf_mode(h: i32, verbose: bool) -> (u32, u32) {`

```
/// rtw8822c.c:362-376 `rtw8822c_dac_cal_rf_mode`.
///
/// Die zwei `rtw_read_rf` am Anfang stehen in Linux nur fuer die Debugzeile
/// dahinter — aber ein Lesezugriff auf ein RF-Register ist bei diesem Chip
/// ein echter Buszugriff, und weglassen hiesse, die Reihenfolge zu aendern.
```

## L308 · `fn dac_bb_setting(h: i32) {`

```
/// rtw8822c.c:378-392 `rtw8822c_dac_bb_setting`
```

## L324 · `fn dac_cal_adc(h: i32, dm: &mut DmInfo, path: usize) -> (u32, u32) {`

```
/// rtw8822c.c:394-470 `rtw8822c_dac_cal_adc`
```

## L336 · `host::w32_mask(h, base_addr + 0x30, 1 << 30, 0x0);`

```
// ADCK step1
```

## L357 · `if ic != 0x0 {`

```
// compensation value
```

## L370 · `host::w32(h, 0x1c3c, path_sel + 0x8103);`

```
// check ADC DC offset
```

## L397 · `host::w32(h, 0x1c3c, 0x0000_0003);`

```
// ADCK step2
```

## L402 · `write_rf_reg_mix(h, path, 0x8f, 1 << 13, 0x1);`

```
// release pull low switch on IQ path
```

## L408 · `fn dac_cal_step1(h: i32, dm: &DmInfo, path: usize) {`

```
/// rtw8822c.c:472-515 `rtw8822c_dac_cal_step1`
```

## L451 · `fn dac_cal_step2(h: i32, path: usize) -> (u32, u32) {`

```
/// rtw8822c.c:517-564 `rtw8822c_dac_cal_step2`
```

## L469 · `if ic != 0x0 {`

```
// compensation value
```

## L476-482 · `if ic < 0x300 {`

```
// `0x7f - ic` LAEUFT UM, sobald der gemessene Versatz gross genug ist:
// `(0x400 - ic) * 12 / 5` kann bis 614 werden, und 0x7f ist 127. In C
// ist das ein u32-Umlauf, der danach von `& 0xf` und `check_hw_ready`
// ohnehin als „passt nicht" endet. Hier steht es ausdruecklich als
// Umlauf, weil ein Rust-Bau mit Ueberlaufpruefung sonst PANISCH endet —
// und ein Treiber, der an einer Messung stirbt, ist schlimmer als einer,
// der dieselbe Fehlermeldung wie Linux ausgibt.
```

## L501-504 · `fn dac_cal_step3(h: i32, path: usize, adc_ic: u32, adc_qc: u32,`

```
/// rtw8822c.c:566-641 `rtw8822c_dac_cal_step3`.
///
/// Gibt zurueck: die zwei Werte fuer die Abbruchpruefung (`ic`, `qc` als
/// BETRAG) und die zwei Rohwerte fuer die Debugzeile (`i_out`, `q_out`).
```

## L545 · `let temp = ((adc_ic + 0x10) & 0x3ff) | (((adc_qc + 0x10) & 0x3ff) << 10);`

```
// check DAC DC offset
```

## L568 · `fn dac_cal_step4(h: i32, path: usize) {`

```
/// rtw8822c.c:643-651 `rtw8822c_dac_cal_step4`
```

## L577 · `fn dac_cal_backup_vec(h: i32, dm: &mut DmInfo, path: usize, vec: usize,`

```
/// rtw8822c.c:653-668 `rtw8822c_dac_cal_backup_vec`
```

## L590 · `fn dac_cal_backup_path(h: i32, dm: &mut DmInfo, path: usize) {`

```
/// rtw8822c.c:670-688 `rtw8822c_dac_cal_backup_path`
```

## L597 · `let w_addr = path_write_addr(path) + 0xb0;`

```
// backup I vector
```

## L602 · `let w_addr = path_write_addr(path) + 0xb0 + W_OFF;`

```
// backup Q vector
```

## L608-613 · `fn dac_cal_backup_dck(h: i32, dm: &mut DmInfo) {`

```
/// rtw8822c.c:690-712 `rtw8822c_dac_cal_backup_dck`.
///
/// **Die Indizes von Pfad B sind vertauscht gegenueber Pfad A** — bei A
/// steht `[0][0] [0][1] [1][0] [1][1]`, bei B `[0][0] [1][0] [0][1] [1][1]`.
/// Das steht so in Linux, und `restore_dck` liest in DERSELBEN Verdrehung
/// zurueck, also hebt es sich auf. Geradegezogen waere es eine Abweichung.
```

## L626 · `fn dac_cal_backup(h: i32, dm: &mut DmInfo) {`

```
/// rtw8822c.c:714-742 `rtw8822c_dac_cal_backup`
```

## L630 · `host::w32(h, 0x9b4, 0xdb66_db00);`

```
// set clock
```

## L633 · `host::clr32(h, 0x1830, 1 << 30);`

```
// backup path-A I/Q
```

## L638 · `host::clr32(h, 0x4130, 1 << 30);`

```
// backup path-B I/Q
```

## L652 · `fn dac_cal_restore_dck(h: i32, dm: &DmInfo) {`

```
/// rtw8822c.c:744-772 `rtw8822c_dac_cal_restore_dck`
```

## L671 · `fn dac_cal_restore_prepare(h: i32, dm: &DmInfo) {`

```
/// rtw8822c.c:774-828 `rtw8822c_dac_cal_restore_prepare`
```

## L724 · `fn dac_cal_restore_wait(h: i32, target_addr: u32, toggle_addr: u32) -> bool {`

```
/// rtw8822c.c:830-845 `rtw8822c_dac_cal_restore_wait`
```

## L740 · `fn dac_cal_restore_path(h: i32, dm: &DmInfo, path: usize) -> bool {`

```
/// rtw8822c.c:847-891 `rtw8822c_dac_cal_restore_path`
```

## L782 · `fn dac_cal_restore_both(h: i32, dm: &DmInfo) -> bool {`

```
/// rtw8822c.c:893-902 `__rtw8822c_dac_cal_restore`
```

## L787 · `fn dac_cal_restore(h: i32, dm: &DmInfo) -> bool {`

```
/// rtw8822c.c:904-941 `rtw8822c_dac_cal_restore`
```

## L789 · `if dm.dack_msbk[RF_PATH_A][0][0] == 0`

```
// sample the first element for both path's IQ vector
```

## L827 · `pub fn rf_dac_cal(h: i32, dm: &mut DmInfo) -> bool {`

```
/// rtw8822c.c:943-1008 `rtw8822c_rf_dac_cal`
```

## L829-833 · `host::print("    RF 0x3e (Tabelle: A=0x3, B=0x20):  A=0x");`

```
// Die Tabellen schreiben auf RF 0x3e als EINZIGES Register, das danach
// niemand mehr anfasst, verschiedene Werte je Pfad: A=0x3, B=0x20
// (rtw8822c_table.c, gegen den Bedingungslaeufer nachgerechnet). Lesen
// beide Pfade dasselbe, ist der Pfadzugriff selbst falsch — und dann
// ist jede Aussage ueber "Pfad B" wertlos.
```

## L845 · `let (backup, backup_rf) = dac_backup_reg(h);`

```
// not able to restore, do it
```

## L858 · `let (adc_ic_a, adc_qc_a) = dac_cal_adc(h, dm, RF_PATH_A);`

```
// path-A
```

## L864 · `let (adc_ic_b, adc_qc_b) = dac_cal_adc(h, dm, RF_PATH_B);`

```
// path-B
```

## L878 · `dac_cal_backup(h, dm);`

```
// backup results to restore, saving a lot of time
```

## L881-883 · `host::print("    DACK A: ic=0x");`

```
// Linux gibt diese vier Zahlen als Debugzeilen aus. Sie sind die einzige
// Auskunft darueber, ob die Kalibrierung konvergiert ist — ein `ic`/`qc`
// unter 5 heisst ja.
```

## L905-913 · `#[allow(clippy::too_many_arguments)]`

```
/// Die zehn Runden aus `rtw8822c_rf_dac_cal`, fuer EINEN Pfad.
///
/// In Linux steht diese Schleife zweimal ausgeschrieben da und sagt NICHTS
/// darueber, wie sie ausgegangen ist — sie laeuft zehnmal und geht weiter.
/// Hier meldet sie jede Runde, und zwar den Wert, an dem der Abbruch haengt
/// (`ic`/`qc` aus step3, der BETRAG des restlichen Versatzes). Der erste
/// Geraetelauf sagte fuer Pfad B 36/40 statt unter 5 — und aus einer
/// Endzahl allein ist nicht zu sehen, ob es schwingt, feststeht oder
/// langsam faellt ([[feedback_dump_the_raw_input_before_debugging_the_interpretation]]).
```

