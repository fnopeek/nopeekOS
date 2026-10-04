# `tools/wasm/wifi_rtl8822ce/src/chip.rs` @ 5e0102684

## L1-8 · `use crate::host;`

```
//! `rtw8822c.c` aus Linux 6.18.26 — was NUR dieser Chip tut.
//!
//! Der Schnitt ist derselbe wie in Linux: `mac.c` ist fuer alle rtw88-Chips
//! gleich und ruft an genau zwei Stellen in den Chip hinein
//! (`chip->ops->mac_init`, `chip->page_table`/`rqpn_table`). Was hier steht,
//! ist diese Hinein-Haelfte fuer den 8822C.
//!
//! Portiert: `page_table_8822c` · `rqpn_table_8822c` · `rtw8822c_mac_init`.
```

## L13-16 · `pub struct PageTable {`

```
/// main.h:1038-1044 `struct rtw_page_table`. **Die Reihenfolge der Felder
/// ist nicht die der Namen im Initialisierer** — `{64, 64, 64, 64, 1}` ist
/// hq, nq, lq, exq, gapq, und `__priority_queue_cfg` schreibt sie in einer
/// ANDEREN Reihenfolge in die Register (hq, lq, nq, exq).
```

## L25 · `pub const PAGE_TABLE: [PageTable; 5] = [`

```
/// rtw8822c.c:4917-4923 `page_table_8822c[]`. Index 1 = PCIe.
```

## L34-35 · `pub struct Rqpn {`

```
/// main.h:1019-1026 `struct rtw_rqpn` — wohin jede der sechs Sendequeues
/// im DMA-Prioritaetsraum zeigt.
```

## L45-50 · `pub const RQPN_PCIE: Rqpn = Rqpn {`

```
/// rtw8822c.c:4925-4938 `rqpn_table_8822c[]`. Index 1 = PCIe.
///
/// Gebaut sind nur die beiden Eintraege, die es auf PCIe geben kann: Linux
/// waehlt [1] fuer PCIe, [0] fuer SDIO und [2..4] nach der Zahl der
/// USB-Bulkout-Endpunkte. Ein Bus, den dieser Treiber nicht hat, braucht
/// keine Zeile — und eine Zeile, die niemand liest, ist eine Behauptung.
```

## L60-65 · `pub fn mac_init(h: i32) -> bool {`

```
/// rtw8822c.c:2004-2130 `rtw8822c_mac_init`.
///
/// Eine gerade Liste von Schreibzugriffen — SIFS, Ratenrueckfall, EDCA,
/// Beacon, WMAC, Empfangsfilter. Sie steht hier Zeile fuer Zeile in Linux'
/// Reihenfolge und mit Linux' Schreibbreiten; jede Umgruppierung waere eine
/// Abweichung ohne Gewinn.
```

## L67 · `let mut value8 = host::r8(h, REG_FWHW_TXQ_CTRL);`

```
// txq control
```

## L69-72 · `value8 |= (1 << 7) & !(1 << 1) & !(1u8 << 2);`

```
// Linux schreibt `BIT(7) & ~BIT(1) & ~BIT(2)`. Das ist 0x80 — die zwei
// Ausmaskierungen treffen ein Bit, das gar nicht gesetzt ist. Der
// Ausdruck bleibt stehen, damit sichtbar ist, dass hier NICHTS
// geloescht wird, obwohl es so aussieht.
```

## L77 · `host::w16(h, REG_SPEC_SIFS, WLAN_SIFS_DUR_TUNE);`

```
// sifs control
```

## L83 · `host::w32(h, REG_DARFRC, WLAN_DATA_RATE_FB_CNT_1_4);`

```
// rate fallback control
```

## L96 · `host::w8(h, REG_AMPDU_MAX_TIME_V1, WLAN_AMPDU_MAX_TIME);`

```
// protocol configuration
```

## L114 · `host::clr8(h, REG_LIFETIME_EN, BIT_BA_PARSER_EN);`

```
// close BA parser
```

## L118 · `host::w32(h, REG_EDCA_VO_PARAM, WLAN_EDCA_VO_PARAM);`

```
// EDCA configuration
```

## L128 · `host::clr32(h, REG_AFE_CTRL1, BIT_MAC_CLK_SEL);`

```
// MAC clock configuration
```

## L140 · `host::set8(h, REG_BCN_CTRL, BIT_EN_BCN_FUNCTION);`

```
// Set beacon control - enable TSF and other related functions
```

## L143 · `host::w32(h, REG_TBTT_PROHIBIT, WLAN_TBTT_TIME);`

```
// Set send beacon related registers
```

## L150 · `host::w32(h, REG_MAR, WLAN_MULTI_ADDR);`

```
// WMAC configuration
```

## L169 · `let mut value16 = host::r16(h, REG_RXPSF_CTRL + 2) & 0xF00F;`

```
// init low power
```

## L182 · `let mut value16 = host::r16(h, REG_RXPSF_CTRL);`

```
// rx ignore configuration
```

## L189 · `host::w32(h, REG_INT_MIG, WLAN_MAC_INT_MIG_CFG);`

```
// Interrupt migration configuration
```

## L195-197 · `use crate::dm::{DmInfo, PathDiv};`

```
// ════════════════════════════════════════════════════════════════
// Stufe 3c: rtw8822c_phy_set_param (rtw8822c.c:1862-1913)
// ════════════════════════════════════════════════════════════════
```

## L203 · `pub const DEFAULT_1SS_TX_PATH: u8 = BB_PATH_A;`

```
/// rtw8822c.c:5361 `.default_1ss_tx_path = BB_PATH_A`
```

## L206 · `fn header_file_init(h: i32, pre: bool) {`

```
/// rtw8822c.c:88-99 `rtw8822c_header_file_init`
```

## L220-221 · `fn bb_reset(h: i32) {`

```
/// rtw8822c.c:101-106 `rtw8822c_bb_reset` — aus, an, aus? Nein: AN, aus, AN.
/// Der mittlere Schritt ist der Reset, die zwei aeusseren halten ihn.
```

## L228 · `fn config_cck_rx_path(h: i32, rx_path: u8) {`

```
/// rtw8822c.c:2444-2459 `rtw8822c_config_cck_rx_path`
```

## L247 · `fn config_ofdm_rx_path(h: i32, rx_path: u8) {`

```
/// rtw8822c.c:2461-2479 `rtw8822c_config_ofdm_rx_path`
```

## L267 · `fn config_cck_tx_path(h: i32, tx_path: u8, is_tx2_path: bool) {`

```
/// rtw8822c.c:2481-2497 `rtw8822c_config_cck_tx_path`
```

## L281 · `fn config_ofdm_tx_path(h: i32, tx_path: u8, tx_path_sel_1ss: u8) {`

```
/// rtw8822c.c:2499-2521 `rtw8822c_config_ofdm_tx_path`
```

## L302-303 · `fn toggle_igi(h: i32) {`

```
/// rtw8822c.c:2436-2442 `rtw8822c_toggle_igi` — den IGI zwei Schritte
/// herunter und wieder zurueck. Das stoesst die Verstaerkungsregelung an.
```

## L306-308 · `let lower = igi.wrapping_sub(2);`

```
// `igi - 2` laeuft in C um, wenn igi unter 2 liegt, und die Maske
// schneidet das Ergebnis danach ohnehin auf sieben Bit. In Rust waere
// dasselbe je nach Bauart eine PANIK — also ausdruecklich umlaufend.
```

## L316 · `fn config_trx_mode(h: i32, tx_path: u8, rx_path: u8, is_tx2_path: bool) {`

```
/// rtw8822c.c:2529-2546 `rtw8822c_config_trx_mode`
```

## L339 · `fn rf_x2_check(h: i32) {`

```
// ── rtw8822c_rf_init (rtw8822c.c:1838-1845) ──────────────────────
```

## L341 · `fn rf_x2_check(h: i32) {`

```
/// rtw8822c.c:1010-1022 `rtw8822c_rf_x2_check`
```

## L353-358 · `const POWER_TRIM_SEQ: [(u32, usize); 15] = [`

```
/// rtw8822c.c:1024-1053 `rtw8822c_set_power_trim`.
///
/// Die fuenfzehn Zeilen des Makros `RF_SET_POWER_TRIM(path, seq, idx)` — die
/// Reihenfolge der Indizes ist NICHT fortlaufend (2 kommt zweimal, dann 3-7,
/// dann noch einmal 3-7 und 7): sie bildet die Sendekanalgruppen auf die
/// acht gemessenen Verstaerkungen ab.
```

## L370-373 · `phy::write_rf_reg_mix(h, path, 0x3f, RFREG_MASK,`

```
// `bb_gain` ist in C `s8`; der Wert geht als Rohbitmuster in ein
// 20-Bit-RF-Register, also wird vorzeichenerhaltend erweitert
// und dann geklemmt — genau das tut die implizite Umwandlung
// `s8 -> u32` in C.
```

## L381 · `fn power_trim(h: i32, rf_path_num: u8) {`

```
/// rtw8822c.c:1056-1091 `rtw8822c_power_trim`
```

## L419-422 · `fn thermal_trim(h: i32, rf_path_num: u8) {`

```
/// rtw8822c.c:1093-1109 `rtw8822c_thermal_trim`.
///
/// Der Kommentar in Linux erklaert die schraege Umsortierung: Bit 0 der
/// efuse wandert auf Bit 3, und die Bits 1-3 ruecken eines nach unten.
```

## L430 · `let mut thermal = (pg_therm & 0x0e) >> 1; // FIELD_GET(GENMASK(3, 1))`

```
// FIELD_GET(GENMASK(3, 1))
```

## L431 · `thermal |= (pg_therm & 0x01) << 3; // FIELD_PREP(BIT(3), pg & BIT(0))`

```
// FIELD_PREP(BIT(3), pg & BIT(0))
```

## L436-440 · `fn pa_bias(h: i32, rf_path_num: u8) {`

```
/// rtw8822c.c:1111-1132 `rtw8822c_pa_bias`.
///
/// **Die zweite Schleife prueft `EFUSE_READ_FAIL` NICHT** — sie schreibt
/// auch einen Fehlwert weiter. Das steht so in Linux; ein 0xff wird von
/// `PPG_PABIAS_MASK` ohnehin auf 0xf beschnitten.
```

## L460 · `fn rf_init(h: i32, dm: &mut DmInfo, rf_path_num: u8) -> bool {`

```
/// rtw8822c.c:1838-1845 `rtw8822c_rf_init`
```

## L475-480 · `fn pwrtrack_init(dm: &mut DmInfo, thermal_meter_k: u8) {`

```
/// rtw8822c.c:1847-1860 `rtw8822c_pwrtrack_init`. Reiner Treiberzustand.
///
/// `ewma_thermal_init` legt einen gleitenden Mittelwert an — bei uns ist das
/// die 0 in `thermal_avg`, die `rtw_phy_pwrtrack_avg` beim ersten Wert
/// ersetzt. Die Mittelung selbst gehoert zum Nachfuehren der Sendeleistung
/// und damit zur Stufe, die sendet.
```

## L491-496 · `pub fn read_cck_gi_bnd(h: i32, dm: &mut crate::dm::DmInfo) {`

```
/// rtw8822c.c:1961-1968, ein Stueck aus `rtw8822c_phy_set_param`.
///
/// Herausgeloest, weil der EMPFANGSweg (`query_phy_status_page0`) dieselben
/// zwei Zahlen braucht: sie sind der Massstab, an dem ein CCK-Paket seine
/// RSSI bekommt. In Linux stehen sie in `dm_info` und werden genau hier
/// einmal gefuellt; sie gehoeren dem TREIBER, nicht dem Paket.
```

## L508-514 · `#[allow(clippy::too_many_arguments)]`

```
/// rtw8822c.c:1862-1913 `rtw8822c_phy_set_param`.
///
/// `hal->antenna_tx`/`antenna_rx` kommen aus `rtw_chip_parameter_setup`:
/// bei 2T2R beide `BB_PATH_AB`. `is_tx2_path` ist dort fest `false`.
///
/// Gibt zurueck: (Tabellen wie gerechnet, DAC-Kalibrierung konvergiert) —
/// die Gates der Stufen 3b und 3c.
```

## L519 · `host::set8(h, REG_SYS_FUNC_EN, BIT_FEN_BB_GLB_RST | BIT_FEN_BB_RSTB);`

```
// power on BB/RF domain
```

## L524 · `host::w32_mask(h, REG_DIS_DPD, DIS_DPD_MASK, DIS_DPD_RATEALL);`

```
// disable low rate DPD
```

## L527 · `header_file_init(h, true);`

```
// pre init before header files config
```

## L553 · `header_file_init(h, false);`

```
// post init after header files config
```

## L570-574 · `pub fn false_alarm_statistics(h: i32, dm: &mut DmInfo) {`

```
/// rtw8822c.c:2004 `rtw8822c_false_alarm_statistics`.
///
/// Sie ist hier nicht, weil Stufe 3 sie braeuchte, sondern weil sie das
/// GATE ist: zaehlt der Empfaenger Falschalarme und CCA-Ereignisse, hoert
/// er. Ein stiller Zaehler heisst, dass die PHY nicht laeuft.
```

## L630 · `host::clr32(h, REG_RX_BREAK, BIT_COM_RX_GCK_EN);`

```
// disable rx clk gating to reset counters
```

## L637-639 · `use crate::coex::Coex;`

```
// ════════════════════════════════════════════════════════════════
// Stufe 4a: die Koexistenz-Ops des Chips (rtw8822c.c)
// ════════════════════════════════════════════════════════════════
```

## L643 · `pub fn coex_cfg_init(h: i32) {`

```
/// rtw8822c.c `rtw8822c_coex_cfg_init` — `chip->ops->coex_set_init`.
```

## L645 · `host::set8(h, REG_BCN_CTRL, BIT_EN_BCN_FUNCTION);`

```
// enable TBTT interrupt
```

## L648 · `host::w8_mask(h, REG_BT_TDMA_TIME, BIT_MASK_SAMPLE_RATE, 0x5);`

```
// BT report packet sample rate: 0x790[5:0]=0x5
```

## L651 · `host::w8(h, REG_BT_STAT_CTRL, 0x1);`

```
// enable BT counter statistics
```

## L654 · `host::set32(h, REG_GPIO_MUXCFG, BIT_BT_PTA_EN);`

```
// enable PTA (3-wire function form BT side)
```

## L658 · `host::set8(h, REG_QUEUE_CTRL, BIT_PTA_WL_TX_EN);`

```
// enable PTA (tx/rx signal form WiFi side)
```

## L660 · `host::clr8(h, REG_QUEUE_CTRL, BIT_PTA_EDCCA_EN);`

```
// wl tx signal to PTA not case EDCCA
```

## L662 · `host::set16(h, REG_BT_COEX_V2, BIT_GNT_BT_POLARITY);`

```
// GNT_BT=1 while select both
```

## L664 · `host::clr8(h, REG_DUMMY_PAGE4_V1, BIT_BTCCA_CTRL);`

```
// BT_CCA = ~GNT_WL_BB, not or GNT_BT_BB, LTE_Rx
```

## L667 · `phy::write_rf_reg_mix(h, RF_PATH_B, RF_MODOPT, 0xfffff, 0x40000);`

```
// to avoid RF parameter error
```

## L671 · `pub fn coex_cfg_gnt_debug(h: i32) {`

```
/// rtw8822c.c `rtw8822c_coex_cfg_gnt_debug`
```

## L680-685 · `pub fn coex_cfg_rfe_type(h: i32, c: &mut Coex, share_ant: bool, rfe_option: u8) {`

```
/// rtw8822c.c `rtw8822c_coex_cfg_rfe_type`.
///
/// Setzt den Beschreibungssatz des Antennen-Frontends — und schaltet dabei
/// die LTE-Koexistenz auf der WLAN-Seite AB. **`ant_switch_exist` bleibt
/// `false`**, und das ist der Grund, warum `rtw_coex_set_ant_switch` auf
/// diesem Chip nie etwas tut.
```

## L694 · `crate::coex::write_indirect_reg(h, LTE_COEX_CTRL, BIT_LTE_COEX_EN, 0x0);`

```
// disable LTE coex in wifi side
```

## L700-705 · `#[allow(dead_code)]`

```
/// rtw8822c.c `rtw8822c_coex_cfg_gnt_fix`.
///
/// Nicht im Anlaufweg — Linux ruft es aus `rtw_coex_run_coex`, also im
/// laufenden Betrieb. Es steht hier, weil es zu den Coex-Ops des Chips
/// gehoert und weil der naechste Posten es braucht; wer es erst dann
/// schreibt, schreibt es unter Zeitdruck.
```

## L708 · `const COEX_WLINK_2GFREE: u8 = 0x7; // coex.h:176`

```
// coex.h:176
```

## L720 · `if share_ant {`

```
// BT at S1 for Shared-Ant
```

## L734 · `host::w8_mask(h, REG_IGN_GNTBT4, BIT_PI_IGNORE_GNT_BT, 1);`

```
// disable WL-S1 BB chage RF mode if GNT_BT, since RF TRx mask can do it
```

## L748 · `host::w8_mask(h, REG_IGN_GNT_BT1, BIT_PI_IGNORE_GNT_BT, 0);`

```
// shared-antenna
```

## L756-758 · `#[inline] fn is_ch_2g(ch: u8) -> bool { ch <= 14 }`

```
// ════════════════════════════════════════════════════════════════
// Stufe 4c: rtw8822c_set_channel (rtw8822c.c:2529)
// ════════════════════════════════════════════════════════════════
```

## L760-761 · `#[inline] fn is_ch_2g(ch: u8) -> bool { ch <= 14 }`

```
/// main.h:73-79 — die Bandpruefungen, die `set_channel_bb` und
/// `set_channel_rf` ueberall benutzen.
```

## L769 · `fn rstb_3wire(h: i32, enable: bool) {`

```
/// rtw8822c.c:2229-2239 `rtw8822c_rstb_3wire`
```

## L780-783 · `fn set_channel_bb(h: i32, channel: u8, bw: usize, primary_ch_idx: u8) {`

```
/// rtw8822c.c:2241-2397 `rtw8822c_set_channel_bb`.
///
/// **Hier stehen AGC und CCA-Maske** — und das ist der Grund, warum die
/// Falschalarmzaehler vor dieser Funktion nichts zaehlen koennen.
```

## L929-932 · `fn set_channel_rf(h: i32, channel: u8, bw: usize) {`

```
/// rtw8822c.c:2399-2434 `rtw8822c_set_channel_rf`.
///
/// Eine einzige Zahl — RF-Register 0x18 — traegt Band, Kanal, RFSI und
/// Bandbreite; sie wird gelesen, feldweise geloescht und neu gesetzt.
```

## L959 · `_ => { rf_reg18 |= RF18_BW_20M; 0x18 }`

```
// RTW_CHANNEL_WIDTH_5/10/20 und Linux' `default:`
```

## L978 · `pub fn set_channel(h: i32, channel: u8, bw: usize, primary_ch_idx: u8) {`

```
/// rtw8822c.c:2529-2546 `rtw8822c_set_channel`
```

## L986-990 · `fn set_write_tx_power_ref(h: i32, rf_path_num: u8,`

```
/// rtw8822c.c:2693-2713 `rtw8822c_set_write_tx_power_ref`.
///
/// Zwei Bezugswerte je Pfad — CCK und OFDM — in vier festen Registern.
/// Vor JEDEM Schreibzugriff wird `0x1c90` Bit 15 geloescht; das ist kein
/// Versehen und keine Schleifeninvariante, es steht so da.
```

## L1006 · `fn set_tx_power_diff(h: i32, rate: u8, diff_idx: &[i8; 4]) {`

```
/// rtw8822c.c:2566-2586 `rtw8822c_set_tx_power_diff`
```

## L1017 · `host::w32_mask(h, 0x1c90, 1 << 15, 0x0);`

```
// rtw8822c.c:2578 — `0x1c90` ist REG_RSTB, Bit 15.
```

## L1022-1026 · `pub fn set_tx_power_index(h: i32, rf_path_num: u8,`

```
/// rtw8822c.c:2588-2620 `rtw8822c_set_tx_power_index`.
///
/// Schreibt nicht die Indizes selbst, sondern zwei BEZUGSwerte (CCK und
/// MCS7) und je Vierergruppe von Raten die ABWEICHUNG davon — und zwar das
/// Minimum beider Pfade.
```

## L1032 · `const RATE_SECTION_2SS_MAX: usize = 5; // __RTW_RATE_SECTION_2SS_MAX`

```
// __RTW_RATE_SECTION_2SS_MAX
```

## L1059-1062 · `pub fn config_tx_path(h: i32, tx_path: u8, tx_path_sel_1ss: u8,`

```
// ═══════════════════════════════════════════════════════════════════
// Was `rtw_watch_dog_work` alle zwei Sekunden an DIESEM Chip tut.
// rtw8822c.c — Quarz, Sendeleistung, CCK-Schwelle, Sendepfad.
// ═══════════════════════════════════════════════════════════════════
```

## L1064 · `pub fn config_tx_path(h: i32, tx_path: u8, tx_path_sel_1ss: u8,`

```
/// rtw8822c.c:2523-2527 `rtw8822c_config_tx_path`
```

## L1072-1073 · `const CCK_PD_REG: [[(u32, u32, u32, u32); 2]; 2] = [`

```
/// rtw8822c.c:4343-4352 `rtw8822c_cck_pd_reg[bw][nrx]` —
/// `(reg_pd, mask_pd, reg_cs, mask_cs)`.
```

## L1081 · `fn phy_cck_pd_set_reg(h: i32, pd_diff: i8, cs_diff: i8, bw: usize, nrx: usize) {`

```
/// rtw8822c.c:4359-4390 `rtw8822c_phy_cck_pd_set_reg`
```

## L1084 · `return; // Linux: WARN_ON und zurueck`

```
// Linux: WARN_ON und zurueck
```

## L1104 · `pub fn phy_cck_pd_set(h: i32, dm: &mut DmInfo, new_lvl: u8) {`

```
/// rtw8822c.c:4392-4413 `rtw8822c_phy_cck_pd_set`
```

## L1120 · `dm.cck_fa_avg = CCK_FA_AVG_RESET;`

```
// „update cck pd info"
```

## L1130-1136 · `fn xcap_extend(v: u8) -> u32 {`

```
// ── Der Quarz: rtw8822c_cfo_track und seine vier Helfer ──────────
//
// **Das ist die Sendeseite der Temperatur.** Ein Empfaenger rastet sich
// an jeder Praeambel neu auf die Frequenz des Gegenuebers ein; ein
// Sender laeuft auf dem eigenen Quarz. Waermt der Chip ueber Minuten
// auf, wandert der — und die Leitung wird einseitig, ohne dass
// irgendwo ein Fehler steht.
```

## L1138 · `fn xcap_extend(v: u8) -> u32 {`

```
/// rtw8822c.c:4220 `#define XCAP_EXTEND(val) (val | val << 7)`
```

## L1144 · `fn set_crystal_cap_reg(h: i32, dm: &mut DmInfo, crystal_cap: u8) {`

```
/// rtw8822c.c:4222-4231 `rtw8822c_set_crystal_cap_reg`
```

## L1151 · `fn set_crystal_cap(h: i32, dm: &mut DmInfo, crystal_cap: u8) {`

```
/// rtw8822c.c:4233-4241 `rtw8822c_set_crystal_cap`
```

## L1159 · `fn cfo_tracking_reset(h: i32, dm: &mut DmInfo, efuse_crystal_cap: u8) {`

```
/// rtw8822c.c:4243-4255 `rtw8822c_cfo_tracking_reset`
```

## L1172 · `fn report_to_khz(v: i32) -> i32 {`

```
/// rtw8822c.c:4265 `#define REPORT_TO_KHZ(val) ((val << 1) + (val >> 1))`
```

## L1177-1178 · `fn cfo_calc_avg(dm: &mut DmInfo, path_num: u8) -> i32 {`

```
/// rtw8822c.c:4267-4289 `rtw8822c_cfo_calc_avg` — und sie LEERT die
/// Summen, ist also nicht wiederholbar.
```

## L1200-1206 · `fn cfo_need_adjust(h: i32, dm: &mut DmInfo, cfo_avg: i32,`

```
/// rtw8822c.c:4291-4308 `rtw8822c_cfo_need_adjust`.
///
/// **Der Riegel am Ende ist kein Detail:** laeuft Bluetooth im selben
/// Chip (`!rtw_coex_disabled`), stellt Linux die Nachfuehrung AB und
/// setzt den Quarz auf den Wert der efuse zurueck. Wer nur diese
/// Funktion portiert und die Koexistenz nicht, baut den Riegel mit —
/// und `bt_disabled` kommt aus dem Zustand, den der Watchdog pflegt.
```

## L1223-1227 · `pub fn cfo_track(h: i32, dm: &mut DmInfo, path_num: u8,`

```
/// rtw8822c.c:4310-4336 `rtw8822c_cfo_track`.
///
/// `sta_cnt != 1` heisst bei uns: keine Verbindung. Ohne genau EINE
/// Gegenstelle ist ein gemittelter Frequenzversatz sinnlos, und Linux
/// faehrt dann die Nachfuehrung schrittweise auf die efuse zurueck.
```

## L1255 · `fn pwrtrack_set(h: i32, dm: &DmInfo, rf_path: usize) {`

```
// ── Die Sendeleistung ueber die Temperatur ───────────────────────
```

## L1257 · `fn pwrtrack_set(h: i32, dm: &DmInfo, rf_path: usize) {`

```
/// rtw8822c.c:4405-4420 `rtw8822c_pwrtrack_set`
```

## L1268-1272 · `fn pwr_track_stats(h: i32, dm: &mut DmInfo, thermal_meter: &[u8],`

```
/// rtw8822c.c:4422-4431 `rtw8822c_pwr_track_stats`.
///
/// `0xff` in der efuse heisst „kein Thermometer fuer diesen Pfad" — dann
/// gibt es nichts zu mitteln, und ein Mittelwert aus 0xff waere eine
/// erfundene Temperatur.
```

## L1282 · `fn pwr_track_path(h: i32, dm: &mut DmInfo, swing: &phy::SwingTable,`

```
/// rtw8822c.c:4433-4444 `rtw8822c_pwr_track_path`
```

## L1291 · `fn pwr_track_inner(h: i32, dm: &mut DmInfo, thermal_meter: &[u8],`

```
/// rtw8822c.c:4446-4459 `__rtw8822c_pwr_track`
```

## L1307-1311 · `pub fn pwr_track(h: i32, dm: &mut DmInfo, power_track_type: u8,`

```
/// rtw8822c.c:4461-4481 `rtw8822c_pwr_track`.
///
/// **Zwei Takte, nicht einer.** Im ersten wird das Thermometer nur
/// ANGESTOSSEN, im zweiten gelesen — die Wandlung braucht Zeit, und ein
/// Wert, der im selben Takt gelesen wird, ist der alte.
```

## L1335-1336 · `fn do_lck(h: i32) {`

```
/// rtw8822c.c:2136-2153 `rtw8822c_do_lck` — den Synthesizer neu
/// einrasten lassen, wenn die Temperatur weit genug gewandert ist.
```

## L1343-1344 · `let t0 = host::now_us();`

```
// read_poll_timeout(…, val != 0x1, 1000, 100000, …): 100 ms Frist,
// alle 1000 us nachsehen.
```

