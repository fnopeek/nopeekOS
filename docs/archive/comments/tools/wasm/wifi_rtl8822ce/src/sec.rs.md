# `tools/wasm/wifi_rtl8822ce/src/sec.rs` @ 5e0102684

## L1-5 · `#![allow(dead_code)]`

```
//! `sec.c` aus Linux 6.18.26 rtw88 — nur `rtw_sec_enable_sec_engine`.
//!
//! Der Rest von sec.c (Schluessel in die CAM schreiben, `rtw_sec_write_cam`,
//! `rtw_sec_clear_cam`) haengt an einer VERBINDUNG und gehoert zu der Stufe,
//! die eine aufbaut. Hier steht, was `rtw_core_start` beim Anlaufen tut.
```

## L11-14 · `pub fn enable_sec_engine(h: i32) {`

```
/// sec.c `rtw_sec_enable_sec_engine`.
///
/// `sec->default_key_search` wird in derselben Funktion auf `true` gesetzt
/// („default use default key search for now"), also gilt der Zweig immer.
```

## L21 · `sec_config |= RTW_SEC_TX_UNI_USE_DK | RTW_SEC_RX_UNI_USE_DK`

```
// default_key_search == true
```

## L27-29 · `#[derive(Clone, Copy)]`

```
// ════════════════════════════════════════════════════════════════
// Stufe 6a: der Schluesselspeicher (sec.c:24-100)
// ════════════════════════════════════════════════════════════════
```

## L31 · `#[derive(Clone, Copy)]`

```
/// sec.h:20-27 `struct rtw_cam_entry` — ein Platz im Schluesselspeicher.
```

## L49-54 · `pub fn write_cam(h: i32, cam: &mut CamEntry, hw_key_idx: u8,`

```
/// sec.c:24-84 `rtw_sec_write_cam`.
///
/// Acht Worte je Platz, RUECKWAERTS geschrieben (`for i = 7; i >= 0`) —
/// Wort 0 traegt das Gueltig-Bit und geht damit zuletzt hinaus. Wer
/// vorwaerts schriebe, machte den Platz gueltig, bevor der Schluessel
/// drin steht.
```

## L101 · `pub fn clear_cam(h: i32, cam: &mut CamEntry, hw_key_idx: u8) {`

```
/// sec.c:86-104 `rtw_sec_clear_cam`
```

