# `tools/wasm/wifi_rtl8822ce/src/dm.rs` @ 5e0102684

## L1-6 · `#![allow(dead_code)]`

```
//! `struct rtw_dm_info` (main.h:1687-1780) — der Zustand, den die
//! PHY-Schicht zwischen ihren Funktionen traegt.
//!
//! Hier stehen nur die Felder, die Stufe 3 wirklich liest oder schreibt.
//! Ein Feld, das kein portierter Code anfasst, waere eine Behauptung ueber
//! den naechsten Posten und keine Portierung.
```

## L11-26 · `#[derive(Clone, Copy, Default)]`

```
/// `DECLARE_EWMA(name, precision, weight_rcp)` aus
/// `include/linux/average.h`.
///
/// **Die einzige Stelle dieses Treibers, deren Linux-Quelle NICHT in
/// unserem Teilbaum liegt** (`include/linux/average.h` fehlt in
/// `~/.cache/nopeekos/linux-src`). Deshalb steht die Rechnung hier
/// ausgeschrieben, damit sie nachschlagbar ist statt geglaubt:
///
/// `​``text
/// add:  internal = internal
///           ? (((internal << w) - internal) + (val << p)) >> w
///           : (val << p)
/// read: internal >> p            w = ilog2(weight_rcp)
/// `​``
///
/// `DECLARE_EWMA(thermal, 10, 4)` heisst also p = 10, w = 2.
```

## L37-38 · `pub fn add(&mut self, val: u32, precision: u32, weight_rcp: u32) {`

```
/// `ewma_*_add`. `precision` und `weight_rcp` kommen von der
/// Deklarationsstelle — beim Thermometer 10 und 4.
```

## L40 · `let w = weight_rcp.trailing_zeros(); // ilog2, weight_rcp ist 2^n`

```
// ilog2, weight_rcp ist 2^n
```

## L48 · `pub fn read(&self, precision: u32) -> u32 {`

```
/// `ewma_*_read`
```

## L54 · `pub const EWMA_THERMAL_PRECISION: u32 = 10;`

```
/// main.h:1592 `DECLARE_EWMA(thermal, 10, 4)`
```

## L57 · `pub const EWMA_TP_PRECISION: u32 = 10;`

```
/// main.h:667 `DECLARE_EWMA(tp, 10, 2)`
```

## L60 · `pub const EWMA_RSSI_PRECISION: u32 = 10;`

```
/// main.h:760 `DECLARE_EWMA(rssi, 10, 16)`
```

## L64-69 · `#[derive(Clone, Copy)]`

```
/// main.h:1650-1657 `struct rtw_cfo_track`.
///
/// **Der Akkumulator, nicht die Momentaufnahme.** `DmInfo::cfo_tail`
/// daneben ist der letzte Empfangsstatus „for debug"; hier laufen die
/// Summen, aus denen `rtw8822c_cfo_track` alle zwei Sekunden den Quarz
/// nachdreht.
```

## L93-95 · `#[derive(Clone, Copy)]`

```
/// main.h:1633-1636 `struct rtw_pkt_count` — was in einem Watchdog-Takt
/// hereinkam, nach Rate aufgeschluesselt. `rtw_phy_stat_rate_cnt`
/// schiebt es nach `last_pkt_count` und faengt von vorn an.
```

## L108-109 · `#[derive(Clone, Copy)]`

```
/// main.h:1621-1631 `struct rtw_dack_info` — der Teil, der die DAC-
/// Kalibrierung ueber einen zweiten Lauf rettet.
```

## L112-113 · `pub dack_adck: [u32; DACK_PATH_8822C],`

```
/// `dack_adck[path]` — von `dac_cal_adc` gerechnet, von `dac_cal_step1`
/// wieder eingesetzt.
```

## L115 · `pub dack_msbk: [[[u16; DACK_MSBK_BACKUP_NUM]; 2]; DACK_PATH_8822C],`

```
/// `dack_msbk[path][vec][i]` · vec 0 = I, vec 1 = Q
```

## L117 · `pub dack_dck: [[[u8; DACK_DCK_BACKUP_NUM]; 2]; DACK_PATH_8822C],`

```
/// `dack_dck[path][vec][i]`
```

## L120 · `pub cck_gi_u_bnd: u8,`

```
/// aus `rtw8822c_phy_set_param`, gelesen von `query_phy_status_page0`
```

## L124 · `pub rx_snr: [i8; 4],`

```
// main.h:1766-1770 — was der EMPFANGSweg zurueckschreibt (Stufe 5a).
```

## L131-135 · `pub fa_history: [u16; 4],`

```
// rtw_phy_init
/// **u16 wie in Linux** (main.h `u16 fa_history[4]`) — die Breite ist
/// Semantik: `rtw_phy_dig_recorder` schreibt einen u32-Zaehler hinein
/// und schneidet ihn dabei ab, und `dig_check_damping` vergleicht
/// GEGEN diesen abgeschnittenen Wert.
```

## L139 · `pub cck_pd_lv: [[u8; 4]; 2],`

```
/// `cck_pd_lv[bw][path]`, bw laeuft bis `RTW_CHANNEL_WIDTH_40` = 1.
```

## L146 · `pub cfo_track: CfoTrack,`

```
// rtw8822c_cfo_init / rtw8822c_cfo_track
```

## L149 · `pub delta_power_index: [i8; 4],`

```
// rtw8822c_pwrtrack_init / rtw8822c_pwr_track
```

## L163 · `pub min_rssi: u8,`

```
// rtw_phy_stat_rssi / rtw_phy_dig
```

## L170 · `pub cur_pkt_count: PktCount,`

```
// rtw_phy_stat_rate_cnt
```

## L174 · `pub fix_rate: u8,`

```
// rtw_phy_ra_track / rtw_phy_rrsr_update
```

## L180 · `pub cck_pd_default: u8,`

```
// rtw_phy_cck_pd
```

## L183 · `pub scan_density: u8,`

```
/// main.h:1780 `u8 scan_density` — Eingabe von `rtw_fw_adaptivity`.
```

## L186 · `pub cck_fa_cnt: u32,`

```
// rtw8822c_false_alarm_statistics
```

## L269 · `#[derive(Clone, Copy, Default)]`

```
/// main.h:1782-1790 `struct rtw_path_div`
```

