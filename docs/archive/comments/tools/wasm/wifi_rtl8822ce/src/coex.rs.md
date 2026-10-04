# `tools/wasm/wifi_rtl8822ce/src/coex.rs` @ 5e0102684

## L1-20 · `#![allow(dead_code)]`

```
//! `coex.c` aus Linux 6.18.26 rtw88 — der Teil, den `rtw_power_on` faehrt.
//!
//! **Warum das ueberhaupt hier steht:** unsere efuse meldet `btcoex JA` und
//! `share_ant JA`. Auf diesem Board teilen WLAN und Bluetooth EINE Antenne,
//! und wer sie bekommt, entscheidet dieser Block. Linux' allererste
//! Coex-Handlung nach dem Einschalten ist woertlich „set antenna path to BT"
//! (`rtw_coex_power_on_setting`), und erst `rtw_coex_init_hw_config` holt sie
//! zurueck. Ohne diese Kette steht der Schalter dort, wo ihn das BIOS
//! gelassen hat — und der Empfaenger hoert nichts.
//!
//! Portiert, in Aufrufreihenfolge: `rtw_coex_power_on_setting` ·
//! `rtw_coex_read_scbd` · `rtw_coex_write_scbd` · `rtw_coex_monitor_bt_enable` ·
//! `rtw_coex_check_rfk` · `rtw_coex_coex_ctrl_owner` · `rtw_coex_set_gnt_bt` ·
//! `rtw_coex_set_gnt_wl` · `rtw_coex_set_ant_path` · `rtw_coex_set_table` ·
//! `rtw_btc_wltoggle_table_a` · `rtw_coex_table` · `rtw_coex_init_coex_var` ·
//! `rtw_coex_wl_slot_extend` · `rtw_coex_wl_ccklock_action` ·
//! `rtw_coex_set_wl_pri_mask` · `rtw_coex_power_save_state` ·
//! `rtw_coex_set_tdma` · `rtw_coex_tdma_timer_base` · `rtw_coex_tdma` ·
//! `rtw_coex_query_bt_info` · `__rtw_coex_init_hw_config` ·
//! `rtw_coex_read_indirect_reg` · `rtw_coex_write_indirect_reg`.
```

## L29-32 · `#[derive(Clone, Copy)]`

```
/// main.h:1587-1620 `struct rtw_coex_stat` und `rtw_coex_dm` — die Felder,
/// die der Anlaufweg liest oder schreibt. Alles andere gehoert zum
/// LAUFENDEN Betrieb (Verkehrsmessung, BT-Profile, RSSI-Zustaende) und
/// kommt mit der Stufe, die einen Kanal hat.
```

## L35 · `pub stop_dm: bool,`

```
// rtw_coex
```

## L42 · `pub bt_disabled: bool,`

```
// rtw_coex_stat
```

## L44-46 · `pub hi_pri_tx: u16,`

```
/// coex.c:454-475 `rtw_coex_monitor_bt_ctr` — die vier Zaehler, die
/// der Watchdog alle zwei Sekunden abholt. Sie sind der einzige
/// Hinweis auf BT-Verkehr, wenn das Scoreboard nichts sagt.
```

## L52-53 · `pub wl_under_ips: bool,`

```
/// `wl_under_ips` / `wl_under_lps` — bei uns immer falsch: wir fahren
/// weder IPS noch LPS (der Plan fuehrt beides als offenen Posten).
```

## L71 · `pub cur_table: u8,`

```
// rtw_coex_dm
```

## L79 · `pub rfe_module_type: u8,`

```
// rtw_coex_rfe (rtw8822c_coex_cfg_rfe_type)
```

## L88-90 · `const COEX_WLINK_MAX: u8 = 8; // coex.h:177`

```
/// coex.h:173-178 `enum coex_wl_link_mode` — die letzte Marke folgt auf
/// 0x7, ist also **8**. `rtw_coex_init_coex_var` setzt sie als „noch kein
/// Modus" ein, und `rtw8822c_coex_cfg_gnt_fix` vergleicht dagegen.
```

## L91 · `const COEX_WLINK_MAX: u8 = 8; // coex.h:177`

```
// coex.h:177
```

## L92 · `const COEX_RSN_LPS: u8 = 13;`

```
/// coex.h:101 `COEX_RSN_LPS = 13`
```

## L118-123 · `fn init_coex_var(&mut self) {`

```
/// coex.c `rtw_coex_init_coex_var`.
///
/// Linux `memset`t `coex_dm` und `coex_stat` und setzt danach drei
/// Felder. Die RSSI-Zustaende auf `COEX_RSSI_STATE_LOW` = 0 fallen mit
/// dem Nullen zusammen, `wl_rx_rate`/`wl_rts_rx_rate` gehoeren zum
/// laufenden Betrieb.
```

## L130 · `self.stop_dm = stop_dm;`

```
// `coex` selbst wird NICHT genullt — nur `coex_dm` und `coex_stat`.
```

## L140 · `pub fn read_scbd(h: i32) -> u16 {`

```
// ── Score-Board: das Zwei-Byte-Gespraech mit dem BT-Kern ─────────
```

## L142 · `pub fn read_scbd(h: i32) -> u16 {`

```
/// coex.c `rtw_coex_read_scbd`. `chip->scbd_support` ist beim 8822C `true`.
```

## L147-151 · `pub fn write_scbd(h: i32, c: &mut Coex, bitpos: u16, set: bool) {`

```
/// coex.c `rtw_coex_write_scbd`.
///
/// **`new_scbd10_def` ist beim 8822C `true`** (rtw8822c.c:5407), also gilt
/// der EINFACHE Zweig: `FIX2M` wird wie jedes andere Bit gesetzt. Beim
/// 8822B ist es umgekehrt herum, und genau dafuer steht das Feld.
```

## L156 · `if set {`

```
// new_scbd10_def == true -> der else-Zweig
```

## L169-177 · `pub fn monitor_bt_enable(h: i32, c: &mut Coex) {`

```
/// coex.c `rtw_coex_monitor_bt_enable`, Zweig mit `scbd_support`.
///
/// Der `bt_reenable_work`-Zeitgeber (15 s) haengt an mac80211's Arbeitswarte-
/// schlange; wir merken nur die Fahne, die er dort setzt.
/// **Das ist die Zahl, an der die Quarznachfuehrung haengt.**
/// `rtw8822c_cfo_need_adjust` stellt sie ab, solange Bluetooth NICHT
/// abgeschaltet ist — wer den Riegel portiert und diese Funktion nicht
/// laufen laesst, bekommt einen Riegel, der nie aufgeht. Bis 0.26.0 rief
/// sie nur die Initialisierung.
```

## L196 · `pub fn read_indirect_reg(h: i32, addr: u16) -> u32 {`

```
// ── Der indirekte LTE-Registerraum ───────────────────────────────
```

## L198 · `pub fn read_indirect_reg(h: i32, addr: u16) -> u32 {`

```
/// coex.c `rtw_coex_read_indirect_reg`
```

## L209 · `pub fn write_indirect_reg(h: i32, addr: u16, mask: u32, val: u32) {`

```
/// coex.c `rtw_coex_write_indirect_reg`
```

## L219 · `fn set_gnt_bt(h: i32, state: u32) {`

```
// ── Wer bekommt die Antenne ──────────────────────────────────────
```

## L221 · `fn set_gnt_bt(h: i32, state: u32) {`

```
/// coex.c `rtw_coex_set_gnt_bt`
```

## L227 · `fn set_gnt_wl(h: i32, state: u32) {`

```
/// coex.c `rtw_coex_set_gnt_wl`
```

## L233-236 · `fn coex_ctrl_owner(h: i32, wifi_control: bool) {`

```
/// coex.c `rtw_coex_coex_ctrl_owner`.
///
/// `chip->btg_reg` ist beim 8822C NICHT gesetzt (kein Treffer in
/// rtw8822c.c), der zweite Schreibzugriff entfaellt also.
```

## L245-249 · `fn check_rfk(h: i32, c: &mut Coex) {`

```
/// coex.c `rtw_coex_check_rfk`.
///
/// Wartet, bis weder BT noch WLAN kalibrieren, bevor der Besitzer des
/// Antennenpfads wechselt. `wlg_at_btg` ist bei gemeinsamer Antenne wahr,
/// `scbd_support` ist es immer — der Zweig gilt also bei uns.
```

## L271-277 · `pub fn set_ant_path(h: i32, c: &mut Coex, force: bool, phase: u8) {`

```
/// coex.c `rtw_coex_set_ant_path`.
///
/// **Der Aufruf, um den es geht.** `ant_switch_exist` ist beim 8822C
/// `false` (rtw8822c_coex_cfg_rfe_type) und `coex_set_ant_switch` ist
/// ohnehin `NULL` — der Schalter haengt also allein an GNT_BT/GNT_WL und
/// am Besitzer des Pfads. `ctrl_type`/`pos_type` werden trotzdem gesetzt,
/// weil sie in Linux gesetzt werden.
```

## L284 · `check_rfk(h, c);`

```
// avoid switch coex_ctrl_owner during BT IQK
```

## L290 · `coex_ctrl_owner(h, c.bt_disabled);`

```
// set path control owner to BT at power-on
```

## L293 · `pos_type = 0; // COEX_SWITCH_TO_BT`

```
// COEX_SWITCH_TO_BT
```

## L303 · `coex_ctrl_owner(h, true);`

```
// set path control owner to wl at initial step
```

## L306 · `pos_type = 0; // COEX_SWITCH_TO_BT`

```
// COEX_SWITCH_TO_BT
```

## L313 · `pos_type = 1; // COEX_SWITCH_TO_WLG`

```
// COEX_SWITCH_TO_WLG
```

## L318 · `pos_type = 4; // COEX_SWITCH_TO_NOCARE`

```
// COEX_SWITCH_TO_NOCARE
```

## L325 · `pos_type = 4; // COEX_SWITCH_TO_NOCARE`

```
// COEX_SWITCH_TO_NOCARE
```

## L332 · `pos_type = 2; // COEX_SWITCH_TO_WLA`

```
// COEX_SWITCH_TO_WLA
```

## L339 · `pos_type = 3; // COEX_SWITCH_TO_WLG_BT`

```
// COEX_SWITCH_TO_WLG_BT
```

## L346 · `pos_type = 3; // COEX_SWITCH_TO_WLG_BT`

```
// COEX_SWITCH_TO_WLG_BT
```

## L354-356 · `let _ = (ctrl_type, pos_type, COEX_SWITCH_CTRL_MAX, COEX_SWITCH_TO_MAX);`

```
// `rtw_coex_set_ant_switch` — `chip->ops->coex_set_ant_switch` ist beim
// 8822C NULL (rtw8822c.c:4993) und `ant_switch_exist` ist false. Beide
// Gruende einzeln reichen; der Aufruf kann nie stattfinden.
```

## L360 · `fn set_table(h: i32, c: &Coex, force: bool, table0: u32, table1: u32) {`

```
// ── Die Koexistenz-Tabelle ───────────────────────────────────────
```

## L362 · `fn set_table(h: i32, c: &Coex, force: bool, table0: u32, table1: u32) {`

```
/// coex.c `rtw_coex_set_table`
```

## L376 · `fn wltoggle_table_a(h: i32, st: &mut H2cState, share_ant: bool, table_case: u8) {`

```
/// coex.c `rtw_btc_wltoggle_table_a`
```

## L387 · `let data = [`

```
// h2c_para[1] = 0x1 ("no definition"), dann die vier Bytes von table_wl
```

## L398-402 · `pub fn table(h: i32, c: &mut Coex, st: &mut H2cState, share_ant: bool,`

```
/// coex.c `rtw_coex_table`.
///
/// **Die Tabellen sind ERZEUGT** (`gen_tables.py` aus rtw8822c.c), und die
/// Zahl der Faelle ist `ARRAY_SIZE` — also genau die Laenge des erzeugten
/// Feldes, nicht eine hingeschriebene Konstante.
```

## L424 · `fn wl_slot_extend(h: i32, c: &mut Coex, st: &mut H2cState, enable: bool) {`

```
// ── TDMA ─────────────────────────────────────────────────────────
```

## L426 · `fn wl_slot_extend(h: i32, c: &mut Coex, st: &mut H2cState, enable: bool) {`

```
/// coex.c `rtw_coex_wl_slot_extend`
```

## L439-444 · `fn wl_ccklock_action(h: i32, c: &mut Coex, st: &mut H2cState) {`

```
/// coex.c `rtw_coex_wl_ccklock_action`.
///
/// Erreichbar nur aus `tdma_timer_base` und nur bei Basis 3 — beim Anlauf
/// ist die Basis 0. Sie steht hier vollstaendig, weil sie im Aufrufbaum
/// steht; `wl_fw_dbg_info` kommt aus einem C2H-Bericht, den erst der
/// laufende Betrieb holt, und ist hier 0.
```

## L453-455 · `let wl_cck_lock = false;`

```
// `wl_cck_lock` und `wl_cck_lock_ever` kommen aus der Verkehrsmessung
// des laufenden Betriebs und sind beim Anlauf beide false, der zweite
// Zweig faellt damit ins Leere.
```

## L459 · `c.cnt_wl_5ms_noextend += 1;`

```
// wl_fw_dbg_info[7] ist hier 0, also <= 5
```

## L469-475 · `fn leave_lps(_h: i32) {}`

```
/// ps.c `rtw_leave_lps`.
///
/// `__rtw_leave_lps_deep` und `__rtw_leave_lps` pruefen beide
/// `RTW_FLAG_LEISURE_PS*` und kehren zurueck, wenn die Fahne nicht steht.
/// Wir schalten Leisure-PS nie ein, also ist das hier ein echter Nullweg —
/// und kein weggelassener, sondern einer, den Linux an dieser Stelle
/// genauso nimmt.
```

## L478 · `fn power_save_state(h: i32, c: &mut Coex, ps_type: u8) {`

```
/// coex.c `rtw_coex_power_save_state`
```

## L489 · `leave_lps(h);`

```
// `lps_conf.mode` ist 0, solange kein LPS laeuft.
```

## L496-499 · `#[allow(clippy::too_many_arguments)]`

```
/// coex.c `rtw_coex_set_tdma`.
///
/// `ap_enable` ist in Linux eine lokale `false` — der AP-Zweig ist tot.
/// `chip->pstdma_type` ist `COEX_PSTDMA_FORCE_LPSOFF` (rtw8822c.c:5410).
```

## L505 · `const COEX_WLINK_2GFREE: u8 = 0x7; // coex.h:176`

```
// coex.h:176
```

## L509 · `power_save_state(h, c, COEX_PS_WIFI_NATIVE);`

```
// Unerreichbar: `ap_enable` ist in Linux eine lokale Konstante.
```

## L514 · `power_save_state(h, c, COEX_PS_LPS_OFF);`

```
// pstdma_type == COEX_PSTDMA_FORCE_LPSOFF
```

## L532 · `fn tdma_timer_base(h: i32, c: &mut Coex, st: &mut H2cState, ty: u8) {`

```
/// coex.c `rtw_coex_tdma_timer_base`
```

## L547 · `(times as u8) & 0x3f`

```
// FIELD_PREP(PARA1_H2C69_TBTT_TIMES, times), coex.h:28 = GENMASK(5,0)
```

## L554 · `((times as u8) & 0x3f) | (1 << 7)`

```
// dazu PARA1_H2C69_TBTT_DIV100 = BIT(7), coex.h:29
```

## L563 · `if c.tdma_timer_base == 3 {`

```
// no 5ms_wl_slot_extend for 4-slot mode
```

## L569 · `pub fn tdma(h: i32, c: &mut Coex, st: &mut H2cState, share_ant: bool,`

```
/// coex.c `rtw_coex_tdma`
```

## L585-587 · `let wl_busy = false;`

```
// `wl_busy` ist `RTW_FLAG_BUSY_TRAFFIC` und beim Anlauf false; der erste
// Zweig gilt also, und `bt_a2dp_exist` braucht gar nicht geprueft zu
// werden.
```

## L615 · `pub fn query_bt_info(h: i32, c: &Coex, st: &mut H2cState) {`

```
/// coex.c `rtw_coex_query_bt_info`
```

## L623 · `fn set_wl_pri_mask(h: i32, bitmap: u8, data: u8) {`

```
/// coex.c `rtw_coex_set_wl_pri_mask`
```

## L630 · `pub fn power_on_setting(h: i32, c: &mut Coex, st: &mut H2cState,`

```
// ── Die zwei Einstiege, die `rtw_power_on` ruft ──────────────────
```

## L632 · `pub fn power_on_setting(h: i32, c: &mut Coex, st: &mut H2cState,`

```
/// coex.c `rtw_coex_power_on_setting`
```

## L640 · `host::set8(h, REG_SYS_FUNC_EN, BIT_FEN_BB_GLB_RST | BIT_FEN_BB_RSTB);`

```
// enable BB, we can write 0x948
```

## L646 · `set_ant_path(h, c, true, COEX_SET_ANT_POWERON);`

```
// set antenna path to BT
```

## L650 · `host::w8(h, 0xff1a, 0x0);`

```
// red x issue
```

## L655 · `pub fn init_hw_config(h: i32, c: &mut Coex, st: &mut H2cState,`

```
/// coex.c `__rtw_coex_init_hw_config`
```

## L660 · `c.kt_ver = (host::r8(h, 0xf1) >> 4) & 0xf; // u8_get_bits(..., GENMASK(7,4))`

```
// u8_get_bits(..., GENMASK(7,4))
```

## L670 · `set_wl_pri_mask(h, COEX_WLPRI_TX_RSP, 1);`

```
// set Tx response = Hi-Pri (ex: Transmitting ACK,BA,CTS)
```

## L672 · `set_wl_pri_mask(h, COEX_WLPRI_TX_BEACON, 1);`

```
// set Tx beacon = Hi-Pri
```

## L674 · `set_wl_pri_mask(h, COEX_WLPRI_TX_BEACONQ, 1);`

```
// set Tx beacon queue = Hi-Pri
```

## L677 · `if c.wl_rf_off {`

```
// antenna config
```

## L693 · `table(h, c, st, share_ant, true, 1);`

```
// PTA parameter
```

## L699 · `pub struct AntState {`

```
// ── Zurueckgelesen: wo steht die Antenne wirklich? ───────────────
```

## L701-706 · `pub struct AntState {`

```
/// Der Zustand, den `set_ant_path` in der Hardware hinterlaesst.
///
/// **Das ist die Sache selbst, nicht ihr Nebeneffekt.** GNT_WL und GNT_BT
/// stehen im indirekten LTE-Registerraum, jedes zweimal (Bits 13:12 und 9:8
/// bzw. 15:14 und 11:10) — `set_gnt_wl`/`set_gnt_bt` schreiben beide Paare.
/// Der Besitzer des Pfads steht in `REG_SYS_SDIO_CTRL+3`.
```

## L725-728 · `pub fn read_scbd_raw(h: i32) -> u16 {`

```
/// Die ROHE Zahl im Postfach, ohne die Maske, die `read_scbd` anlegt.
/// `read_scbd` schneidet `BIT_BT_INT_EN` weg — und wer nur das Ergebnis
/// sieht, kann eine 0 nicht von „unser eigener Schreibzugriff kam nie an"
/// unterscheiden.
```

## L733-741 · `pub fn monitor_bt_ctr(h: i32, c: &mut Coex) {`

```
// ═══════════════════════════════════════════════════════════════════
// Was `rtw_watch_dog_work` alle zwei Sekunden an der Koexistenz tut.
//
// **Der Entscheidungsbaum von `rtw_coex_run_coex` gehoert NICHT hierher**
// — er ist L6 des Plans (111 Funktionen, eigene Stufe) und entscheidet
// Antenne und TDMA, WENN Bluetooth aktiv ist. Hier steht die
// Beobachtung: die vier Verkehrszaehler und der eine Zustand, den die
// Quarznachfuehrung braucht.
// ═══════════════════════════════════════════════════════════════════
```

## L743 · `pub fn monitor_bt_ctr(h: i32, c: &mut Coex) {`

```
/// coex.c:454-475 `rtw_coex_monitor_bt_ctr`
```

## L757 · `pub fn wl_status_check(h: i32, c: &mut Coex) {`

```
/// coex.c:3941-3950 `rtw_coex_wl_status_check`
```

## L765-771 · `pub fn active_query_bt_info(_h: i32, _c: &mut Coex) {}`

```
/// coex.h:423-432 `rtw_coex_active_query_bt_info`.
///
/// **Fuer DIESEN Chip ein Nichts, und das ist kein Weglassen:** die
/// Funktion fragt nur beim RTL8821AU nach, dessen Firmware bei
/// getrennten BT-Kopfhoerern kein `C2H_BT_INFO` von sich aus schickt.
/// Der Zweig steht hier als Kommentar, damit niemand ihn fuer vergessen
/// haelt.
```

