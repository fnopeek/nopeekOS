# `tools/wasm/wifi_rtl8822ce/src/pwrseq.rs` @ 5e0102684

## L1-5 · `#![allow(dead_code)]`

```
//! ERZEUGT von gen_pwrseq.py aus Linux 6.18.26 rtw8822c.c — nicht von Hand
//! aendern. Die vier Power-Sequenz-Tabellen des RTL8822C, Feld fuer Feld.
//!
//! Reihenfolge der Felder wie `struct rtw_pwr_seq_cmd` (main.h:955):
//! offset, cut_mask, intf_mask, base, cmd, mask, value.
```

## L8-9 · `#[derive(Clone, Copy)]`

```
/// `struct rtw_pwr_seq_cmd` — `base` und `cmd` sind in C 4-Bit-Bitfelder in
/// EINEM Byte; hier zwei Felder, weil der Interpreter sie einzeln liest.
```

## L35 · `pub const RTW_PWR_POLLING_CNT: u32 = 20000;`

```
/// main.h:922
```

## L38 · `pub static TRANS_CARDDIS_TO_CARDEMU: [PwrCmd; 8] = [`

```
/// rtw8822c.c `trans_carddis_to_cardemu_8822c` — 8 Kommandos
```

## L50 · `pub static TRANS_CARDEMU_TO_ACT: [PwrCmd; 22] = [`

```
/// rtw8822c.c `trans_cardemu_to_act_8822c` — 22 Kommandos
```

## L76 · `pub static TRANS_ACT_TO_CARDEMU: [PwrCmd; 12] = [`

```
/// rtw8822c.c `trans_act_to_cardemu_8822c` — 12 Kommandos
```

## L92 · `pub static TRANS_CARDEMU_TO_CARDDIS: [PwrCmd; 12] = [`

```
/// rtw8822c.c `trans_cardemu_to_carddis_8822c` — 12 Kommandos
```

## L108 · `pub static CARD_ENABLE_FLOW: [&[PwrCmd]; 2] =`

```
/// rtw8822c.c `card_enable_flow_8822c`
```

## L112 · `pub static CARD_DISABLE_FLOW: [&[PwrCmd]; 2] =`

```
/// rtw8822c.c `card_disable_flow_8822c`
```

