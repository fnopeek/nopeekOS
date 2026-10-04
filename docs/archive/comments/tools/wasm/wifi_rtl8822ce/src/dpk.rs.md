# `tools/wasm/wifi_rtl8822ce/src/dpk.rs` @ 5e0102684

## L1-12 · `#![allow(dead_code)]`

```
//! `rtw8822c_do_dpk` — die digitale Vorverzerrung (Stufe 5d),
//! rtw8822c.c:3171-4186.
//!
//! DPK misst die Kennlinie des Leistungsverstaerkers und legt eine
//! Umkehrfunktion davor. Ohne sie sendet der Chip linear nur bis zu einem
//! Pegel sauber; darueber verzerrt er, und die hohen Modulationen fallen
//! als erstes aus.
//!
//! **Der Einstieg haengt an `is_dpk_pwr_on`**, und das setzt
//! `rtw_load_rfk_table` (phy.c:1847) am Ende des RFK-Tabellenladens. Ohne
//! geladene RFK-Tabelle gibt es keine DPK — das ist kein Sonderfall,
//! sondern die Reihenfolge.
```

## L23 · `const PATHS: usize = 4; // RTW_RF_PATH_MAX`

```
// RTW_RF_PATH_MAX
```

## L25-28 · `pub struct DpkInfo {`

```
/// main.h:1591-1614 `struct rtw_dpk_info`.
///
/// `avg_thermal` ist der gleitende Mittelwert fuer `rtw8822c_dpk_track`
/// — seit 0.26.0 hat er einen Leser (der Watchdog alle zwei Sekunden).
```

## L32 · `pub dpk_path_ok: u8, // DECLARE_BITMAP(…, DPK_RF_PATH_NUM)`

```
// DECLARE_BITMAP(…, DPK_RF_PATH_NUM)
```

## L34 · `pub avg_thermal: [crate::dm::Ewma; DPK_RF_PATH_NUM_U],`

```
/// main.h:1598 `struct ewma_thermal avg_thermal[DPK_RF_PATH_NUM]`
```

## L92 · `#[derive(Clone, Copy, Default)]`

```
/// util.c `rtw_backup_info` — Adresse und Wert, alle vier Byte breit.
```

## L99-103 · `fn set_gnt_wl(h: i32, d: &mut DpkInfo, is_before_k: bool) {`

```
/// rtw8822c.c:3171-3185 `rtw8822c_dpk_set_gnt_wl`.
///
/// Waehrend der Kalibrierung gehoert die Antenne dem WLAN allein. Der
/// vorige Zustand wird gemerkt und hinterher zurueckgegeben — sonst nimmt
/// die naechste Koexistenz-Entscheidung eine Stellung an, die nicht steht.
```

## L116 · `fn restore_registers(h: i32, bckp: &[Backup]) {`

```
/// rtw8822c.c:3187-3194 `rtw8822c_dpk_restore_registers`
```

## L125 · `fn backup_registers(h: i32, reg: &[u32], bckp: &mut [Backup]) {`

```
/// rtw8822c.c:3196-3207 `rtw8822c_dpk_backup_registers`
```

## L133 · `fn backup_rf_registers(h: i32, rf_reg: &[u32],`

```
/// rtw8822c.c:3209-3221 `rtw8822c_dpk_backup_rf_registers`
```

## L144 · `fn reload_rf_registers(h: i32, rf_reg: &[u32],`

```
/// rtw8822c.c:3223-3235 `rtw8822c_dpk_reload_rf_registers`
```

## L155 · `fn information(h: i32, d: &mut DpkInfo) {`

```
/// rtw8822c.c:3237-3249 `rtw8822c_dpk_information`
```

## L164 · `pub fn rxbb_dc_cal(h: i32, path: usize) {`

```
/// rtw8822c.c:3251-3258 `rtw8822c_dpk_rxbb_dc_cal`
```

## L173 · `fn dc_corr_check(h: i32, _path: usize) -> u8 {`

```
/// rtw8822c.c:3260-3283 `rtw8822c_dpk_dc_corr_check`
```

## L188-189 · `let _ = host::r32_mask(h, REG_STAT_RPT, 0xff00);`

```
// Der zweite Lesezugriff steht in Linux ohne Empfaenger da. Er bleibt,
// weil ein Lesezugriff auf diesem Bus eine WIRKUNG haben kann.
```

## L195 · `fn tx_pause(h: i32) {`

```
/// rtw8822c.c:3285-3299 `rtw8822c_dpk_tx_pause`
```

## L212 · `fn load_dpk_table(h: i32, tbl: &[u32]) {`

```
/// `rtw8822c_parse_tbl_dpk` — Tripel aus Adresse, Maske und Wert.
```

## L219 · `fn mac_bb_setting(h: i32) {`

```
/// rtw8822c.c:3301-3305 `rtw8822c_dpk_mac_bb_setting`
```

## L225 · `fn afe_setting(h: i32, is_do_dpk: bool) {`

```
/// rtw8822c.c:3307-3313 `rtw8822c_dpk_afe_setting`
```

## L234 · `fn pre_setting(h: i32, d: &DpkInfo, rf_path_num: u8) {`

```
/// rtw8822c.c:3315-3332 `rtw8822c_dpk_pre_setting`
```

## L253 · `fn rf_setting(h: i32, d: &DpkInfo, path: usize) -> u32 {`

```
/// rtw8822c.c:3334-3370 `rtw8822c_dpk_rf_setting`
```

## L290 · `fn get_cmd(d: &DpkInfo, action: u32, path: usize) -> u32 {`

```
/// rtw8822c.c:3372-3395 `rtw8822c_dpk_get_cmd`
```

## L303 · `fn check_hw_ready(h: i32, addr: u32, mask: u32, target: u32) -> bool {`

```
/// mac.c `check_hw_ready` — lesen, maskieren, gegen einen Wert, 20 ms.
```

## L314 · `fn one_shot(h: i32, d: &mut DpkInfo, path: usize, action: u32) -> u8 {`

```
/// rtw8822c.c:3397-3436 `rtw8822c_dpk_one_shot`
```

## L350 · `fn dgain_read(h: i32) -> u16 {`

```
/// rtw8822c.c:3438-3448 `rtw8822c_dpk_dgain_read`
```

## L357 · `fn thermal_read(h: i32, path: usize) -> u8 {`

```
/// rtw8822c.c:3450-3458 `rtw8822c_dpk_thermal_read`
```

## L366 · `fn pas_read(h: i32, path: usize) -> u32 {`

```
/// rtw8822c.c:3460-3484 `rtw8822c_dpk_pas_read`
```

## L389 · `fn psd_log2base(val: u32) -> u32 {`

```
/// rtw8822c.c:3486-3507 `rtw8822c_psd_log2base`
```

## L398 · `let val_integerd_b = 32 - val.leading_zeros();`

```
// `__fls(val) + 1` — Stelle des hoechsten gesetzten Bits, von eins an.
```

## L408 · `fn gainloss_result(h: i32, path: usize) -> u8 {`

```
/// rtw8822c.c:3509-3522 `rtw8822c_dpk_gainloss_result`
```

## L418 · `fn agc_gain_chk(h: i32, d: &mut DpkInfo, path: usize, limited_pga: u8) -> u32 {`

```
/// rtw8822c.c:3524-3539 `rtw8822c_dpk_agc_gain_chk`
```

## L428 · `0 // = RTW_DPK_GAIN_CHECK`

```
// = RTW_DPK_GAIN_CHECK
```

## L432 · `fn agc_loss_chk(h: i32, path: usize) -> u32 {`

```
/// rtw8822c.c:3541-3556 `rtw8822c_dpk_agc_loss_chk`
```

## L438-439 · `let loss_db = (3u32.wrapping_mul(psd_log2base(loss >> 13)))`

```
// `3 * log2base(loss >> 13) - 3870` in VORZEICHENLOSER Arithmetik, wie
// in C: wird es negativ, laeuft es um und ist damit gross.
```

## L452 · `#[derive(Default, Clone, Copy)]`

```
/// rtw8822c.c:3558-3566 `struct rtw8822c_dpk_data`
```

## L464 · `fn gain_check_state(h: i32, d: &mut DpkInfo, s: &mut DpkData) -> u32 {`

```
/// rtw8822c.c:3568-3593 `rtw8822c_gain_check_state`
```

## L489 · `fn gain_large_state(h: i32, s: &mut DpkData) -> u32 {`

```
/// rtw8822c.c:3595-3608 `rtw8822c_gain_large_state`
```

## L502 · `fn gain_less_state(h: i32, s: &mut DpkData) -> u32 {`

```
/// rtw8822c.c:3610-3623 `rtw8822c_gain_less_state`
```

## L515 · `fn gl_state(h: i32, s: &mut DpkData, is_large: usize) -> u32 {`

```
/// rtw8822c.c:3625-3640 `rtw8822c_gl_state`
```

## L531 · `fn loss_check_state(h: i32, d: &mut DpkInfo, s: &mut DpkData) -> u32 {`

```
/// rtw8822c.c:3664-3675 `rtw8822c_loss_check_state`
```

## L537-542 · `fn pas_agc(h: i32, d: &mut DpkInfo, path: usize, gain_only: bool,`

```
/// rtw8822c.c:3677-3696 `rtw8822c_dpk_pas_agc`.
///
/// In Linux ist das eine Tabelle von Funktionszeigern (`dpk_state[]`), die
/// sich gegenseitig den naechsten Zustand nennen. Hier steht dieselbe
/// Maschine als `match` — ein Zeigerfeld ueber Funktionen mit Wirkung auf
/// Hardware waere in Rust nur Umstand, und die REIHENFOLGE ist dieselbe.
```

## L564 · `fn coef_iq_check(coef_i: u16, coef_q: u16) -> bool {`

```
/// rtw8822c.c:3698-3706 `rtw8822c_dpk_coef_iq_check`
```

## L569 · `fn coef_transfer(h: i32) -> u32 {`

```
/// rtw8822c.c:3708-3723 `rtw8822c_dpk_coef_transfer`
```

## L571-572 · `let _reg = host::r32(h, REG_STAT_RPT);`

```
// Linux liest erst das ganze Wort und verwirft es wieder; der Zugriff
// bleibt, weil er auf diesem Bus eine Wirkung haben kann.
```

## L582 · `const GET_COEF_TBL: [u32; 20] = [`

```
/// rtw8822c.c:3725-3730 `rtw8822c_dpk_get_coef_tbl`
```

## L590 · `fn coef_tbl_apply(h: i32, d: &mut DpkInfo, path: usize) {`

```
/// rtw8822c.c:3732-3742 `rtw8822c_dpk_coef_tbl_apply`
```

## L598 · `fn get_coef(h: i32, d: &mut DpkInfo, path: usize) {`

```
/// rtw8822c.c:3744-3757 `rtw8822c_dpk_get_coef`
```

## L613 · `fn coef_read(d: &DpkInfo, path: usize) -> u8 {`

```
/// rtw8822c.c:3759-3775 `rtw8822c_dpk_coef_read`
```

## L625 · `fn coef_write(h: i32, d: &DpkInfo, path: usize, result: u8) {`

```
/// rtw8822c.c:3777-3798 `rtw8822c_dpk_coef_write`
```

## L642 · `fn fill_result(h: i32, d: &mut DpkInfo, dpk_txagc: u32, path: usize,`

```
/// rtw8822c.c:3800-3816 `rtw8822c_dpk_fill_result`
```

## L659 · `fn gainloss(h: i32, d: &mut DpkInfo, path: usize) -> u32 {`

```
/// rtw8822c.c:3818-3854 `rtw8822c_dpk_gainloss`
```

## L694 · `fn by_path(h: i32, d: &mut DpkInfo, _tx_agc: u32, path: usize) -> u8 {`

```
/// rtw8822c.c:3856-3871 `rtw8822c_dpk_by_path`
```

## L709 · `fn cal_gs(h: i32, d: &mut DpkInfo, path: usize) {`

```
/// rtw8822c.c:3873-3941 `rtw8822c_dpk_cal_gs`
```

## L763 · `tmp_gs = tmp_gs.div_ceil(10).min((tmp_gs + 5) / 10);`

```
// `DIV_ROUND_CLOSEST(tmp_gs, 10)`
```

## L775 · `fn cal_coef1(h: i32, d: &DpkInfo, rf_path_num: u8) -> bool {`

```
/// rtw8822c.c:3943-3975 `rtw8822c_dpk_cal_coef1`
```

## L793-795 · `let gs = d.dpk_gs[path];`

```
// Linux teilt hier ohne Pruefung; `dpk_gs` ist nach `cal_gs` nie
// null, und wenn doch, waere eine Division durch null bei uns ein
// Trap statt eines Unsinnswerts.
```

## L808 · `fn dpk_on(h: i32, d: &mut DpkInfo, path: usize) {`

```
/// rtw8822c.c:3977-3988 `rtw8822c_dpk_on`
```

## L820 · `fn check_pass(h: i32, d: &mut DpkInfo, is_fail: bool, dpk_txagc: u32,`

```
/// rtw8822c.c:3990-4007 `rtw8822c_dpk_check_pass`
```

## L828 · `fn result_reset(h: i32, d: &mut DpkInfo, rf_path_num: u8) {`

```
/// rtw8822c.c:4009-4027 `rtw8822c_dpk_result_reset`
```

## L845 · `fn calibrate(h: i32, d: &mut DpkInfo, path: usize) -> bool {`

```
/// rtw8822c.c:4029-4048 `rtw8822c_dpk_calibrate`
```

## L859 · `fn path_select(h: i32, d: &mut DpkInfo, rf_path_num: u8) -> bool {`

```
/// rtw8822c.c:4050-4057 `rtw8822c_dpk_path_select`
```

## L868 · `fn enable_disable(h: i32, d: &DpkInfo) {`

```
/// rtw8822c.c:4059-4079 `rtw8822c_dpk_enable_disable`
```

## L886 · `fn reload_data(h: i32, d: &mut DpkInfo, rf_path_num: u8) {`

```
/// rtw8822c.c:4081-4116 `rtw8822c_dpk_reload_data`
```

## L918 · `fn reload(h: i32, d: &mut DpkInfo, rf_path_num: u8) -> bool {`

```
/// rtw8822c.c:4118-4135 `rtw8822c_dpk_reload`
```

## L930 · `#[derive(Clone, Copy, PartialEq)]`

```
/// Warum DPK nicht gelaufen ist — oder wie es ausging.
```

## L938 · `pub fn do_dpk(h: i32, d: &mut DpkInfo, rf_path_num: u8) -> DpkRpt {`

```
/// rtw8822c.c:4137-4186 `rtw8822c_do_dpk`
```

## L954-955 · `information(h, d);`

```
// `ewma_thermal_init` fuellt den gleitenden Mittelwert fuer
// `rtw8822c_dpk_track`; die Nachfuehrung gibt es noch nicht.
```

## L986-991 · `pub fn track(h: i32, d: &mut DpkInfo) {`

```
/// rtw8822c.c:3629-3660 `rtw8822c_dpk_track`.
///
/// **Die Nachfuehrung der Vorverzerrung ueber die Temperatur.** Sie
/// laeuft nur, wenn eine DPK-Kalibrierung ueberhaupt stattgefunden hat
/// (`thermal_dpk` beider Pfade null heisst: es gibt nichts
/// nachzufuehren).
```

## L1003-1004 · `let delta_dpk = (d.thermal_dpk[path] as i8)`

```
// Linux rechnet beides in `s8`, und der Umlauf ist gewollt: die
// Maske 0x7f darunter schneidet ohnehin auf sieben Bit.
```

