# `tools/wasm/wifi_rtl8822ce/src/rfkcal.rs` @ 5e0102684

## L1-11 · `#![allow(dead_code)]`

```
//! Die RF-Kalibrierung des 8822C — `rtw8822c_phy_calibration` und was sie
//! umgibt (Stufe 5d).
//!
//! **Wann sie laeuft, entscheidet Linux und nicht wir:** `rtw_set_channel`
//! setzt nur `need_rfk = true`, und `rtw_chip_prepare_tx` fuehrt sie aus,
//! wenn mac80211 `mgd_prepare_tx` ruft — also VOR dem Anmelden. Auf jedem
//! Kanal eines Suchlaufs zu kalibrieren dauert zu lange; der Kommentar in
//! `main.c` sagt genau das.
//!
//! Portiert: `rtw8822c_rfk_power_save` · `rtw8822c_rfk_handshake` ·
//! `rtw8822c_do_iqk` · `rtw8822c_phy_calibration`.
```

## L18 · `pub fn power_save(h: i32, rf_path_num: u8, is_power_save: bool) {`

```
/// rtw8822c.c:1231-1242 `rtw8822c_rfk_power_save`
```

## L27-29 · `#[derive(Default, Clone, Copy)]`

```
/// Was der Handschlag gemeldet hat. Drei Wartezeiten, drei Ausgaenge —
/// Linux schreibt sie in die Debugausgabe, wir behalten sie, weil sie die
/// einzige Auskunft darueber sind, ob die Firmware mitspielt.
```

## L40-45 · `pub fn handshake(h: i32, is_before_k: bool, is_bt_iqk_timeout: &mut bool,`

```
/// rtw8822c.c:1134-1178 `rtw8822c_rfk_handshake`.
///
/// **`is_bt_iqk_timeout` merkt sich einen Fehlschlag fuer immer** — nach
/// einer Zeitueberschreitung wird auf die BT-IQK nie wieder gewartet. Das
/// ist Absicht: eine BT-Seite, die einmal nicht antwortet, kostet sonst bei
/// JEDER Kalibrierung 600 ms.
```

## L52 · `let t0 = host::now_us();`

```
// `read_poll_timeout(rtw_read32_mask, …, 20, 600000, …)`
```

## L81-83 · `fn wait_rfk_ack(h: i32) -> (bool, u64) {`

```
/// `read_poll_timeout(rtw_read8_mask, u1b_tmp, u1b_tmp == 1, 20, 100000, …,
/// REG_ARFR4, BIT_WL_RFK)` — beide Zweige des Handschlags warten damit auf
/// dieselbe Quittung.
```

## L97-102 · `pub fn do_iqk(h: i32, trx: &mut Trx, stage: i32, st: &mut crate::fw::H2cState)`

```
/// rtw8822c.c:1836-1851 `rtw8822c_do_iqk`.
///
/// **Die IQK rechnet die Firmware.** Der Treiber schickt ein H2C-Paket und
/// wartet bis zu 300 ms darauf, dass `REG_RPT_CIP` den Wert `0xaa` traegt.
/// Danach wird `REG_IQKSTAT` geloescht — auch dann, wenn die Wartezeit
/// abgelaufen ist; Linux macht dazwischen keinen Unterschied.
```

## L106 · `crate::fw::do_iqk(h, trx, stage, st, true, false);`

```
// `para.clear = 1`, `segment_iqk` bleibt 0 (`{0}`-Initialisierung).
```

## L126-131 · `pub fn do_gapk(h: i32, g: &mut crate::txgapk::GapkInfo, rf_path_num: u8,`

```
/// rtw8822c.c:1825-1834 `rtw8822c_do_gapk`.
///
/// **Die Pruefung ist umgekehrt, als der Name vermuten laesst:** ist das
/// Bit GESETZT, ist TXGAPK ABGESCHALTET. `dm_flags` wird in Linux nur aus
/// debugfs beschrieben und ist beim Start null — TXGAPK laeuft also im
/// Normalfall mit.
```

