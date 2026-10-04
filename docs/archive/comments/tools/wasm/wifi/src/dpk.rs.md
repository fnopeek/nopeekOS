# `tools/wasm/wifi/src/dpk.rs` @ 5e0102684

## L1-13 · `use crate::host;`

```
//! DPK — Digital Pre-Distortion, full 1:1 port of Linux rtw8852b_rfk.c
//! (rfk.c:1649..2587, the complete _dpk flow).
//!
//! Linux entry: rtw8852b_dpk → _dpk → _dpk_bypass_check ? _dpk_force_bypass
//! : _dpk_cal_select → for each path: _dpk_main → _dpk_agc → _dpk_idl_mpa
//! → _dpk_fill_result.
//!
//! State: rtw89_dpk_info + bp[path][kidx] backup entries, cur_idx[path],
//! dpk_gs[phy], corr_idx/corr_val/dc_i/dc_q arrays.
//!
//! The cal loop sends PMAC test-TX bursts and reads feedback via the
//! DPK sync/gainloss/PAS channels. Each one-shot writes a cmd-ID to
//! R_NCTL_CFG and polls 0xBFF8 for 0x55 (same path as IQK).
```

## L24 · `const BKUP_NUM: usize = 2;        // RTW89_DPK_BKUP_NUM`

```
// ── State ─────────────────────────────────────────────────────────
```

## L25 · `const BKUP_NUM: usize = 2;        // RTW89_DPK_BKUP_NUM`

```
// RTW89_DPK_BKUP_NUM
```

## L26 · `const DPK_RF_PATH: usize = 2;     // RTW8852B_DPK_RF_PATH`

```
// RTW8852B_DPK_RF_PATH
```

## L27 · `const KIP_REG_NUM: usize = 3;     // RTW8852B_DPK_KIP_REG_NUM`

```
// RTW8852B_DPK_KIP_REG_NUM
```

## L73 · `const R_DPD_BF:            u32 = 0x44A0;`

```
// ── BB register addresses (reg.h) ──────────────────────────────────
```

## L138 · `const R_NCTL_CFG:          u32 = 0x8000;`

```
// Shared with iqk.rs:
```

## L155 · `const RR_MOD:              u32 = 0x00;`

```
// ── RF register defs ───────────────────────────────────────────────
```

## L172 · `const RR_TXG1:             u32 = 0x51;         // (not used here but kept for parity)`

```
// (not used here but kept for parity)
```

## L205 · `const LBK_RXIQK:     u32 = 0x06;`

```
// ── One-shot IDs (rtw8852b_dpk_id) ─────────────────────────────────
```

## L217 · `const DPK_SYNC_TH_DC_I: u16 = 200;`

```
// Sync thresholds
```

## L222 · `const AGC_SYNC_DGAIN: u8       = 0;`

```
// AGC FSM steps
```

## L230 · `fn pr(mmio: i32, a: u32) -> u32 { host::mmio_r32(mmio, PHY_CR_BASE + a) }`

```
// ── I/O helpers ────────────────────────────────────────────────────
```

## L263-265 · `fn set_rx_dck(mmio: i32, path: u8) {`

```
// ═══════════════════════════════════════════════════════════════════
//  _set_rx_dck (rfk.c:295) — shared by IQK/DPK
// ═══════════════════════════════════════════════════════════════════
```

## L273-275 · `fn order_convert(mmio: i32) -> u32 {`

```
// ═══════════════════════════════════════════════════════════════════
//  _dpk_order_convert (rfk.c:1675)
// ═══════════════════════════════════════════════════════════════════
```

## L281-283 · `fn onoff(mmio: i32, path: u8, off: bool) {`

```
// ═══════════════════════════════════════════════════════════════════
//  _dpk_onoff (rfk.c:1688)
// ═══════════════════════════════════════════════════════════════════
```

## L288-289 · `let reg = R_DPD_CH0A + ((path as u32) << 8) + (kidx << 2);`

```
// MASKBYTE3 of (R_DPD_CH0A + path*0x100 + kidx*4) = bits[31:24].
// Byte value = (order << 1) | val
```

## L295-297 · `fn one_shot(mmio: i32, path: u8, id: u32) {`

```
// ═══════════════════════════════════════════════════════════════════
//  _dpk_one_shot (rfk.c:1702)
// ═══════════════════════════════════════════════════════════════════
```

## L302 · `let mut ok = false;`

```
// Wait up to 20ms for 0xBFF8 byte0 == 0x55
```

## L316 · `pw(mmio, R_KIP_RPT1, 0x00030000);`

```
// Secondary poll on R_RPT_COM low16 == 0x8000 (2ms)
```

## L327-329 · `fn rx_dck(mmio: i32, path: u8) {`

```
// ═══════════════════════════════════════════════════════════════════
//  _dpk_rx_dck (rfk.c:1744)
// ═══════════════════════════════════════════════════════════════════
```

## L335-337 · `fn information(mmio: i32, path: u8, band: u8, ch: u8, bw: u8) {`

```
// ═══════════════════════════════════════════════════════════════════
//  _dpk_information (rfk.c:1751) — bookkeeping only
// ═══════════════════════════════════════════════════════════════════
```

## L346-348 · `fn bb_afe_setting(mmio: i32, bw: u8) {`

```
// ═══════════════════════════════════════════════════════════════════
//  _dpk_bb_afe_setting / _restore (rfk.c:1775/1793)
// ═══════════════════════════════════════════════════════════════════
```

## L351 · `if bw == 2 /* RTW89_CHANNEL_WIDTH_80 */ {`

```
/* RTW89_CHANNEL_WIDTH_80 */
```

## L365-367 · `fn tssi_pause(mmio: i32, path: u8, is_pause: bool) {`

```
// ═══════════════════════════════════════════════════════════════════
//  _dpk_tssi_pause (rfk.c:1811) — pause/resume TSSI tracking per path
// ═══════════════════════════════════════════════════════════════════
```

## L374-376 · `fn kip_restore(mmio: i32, path: u8) {`

```
// ═══════════════════════════════════════════════════════════════════
//  _dpk_kip_restore (rfk.c:1821)
// ═══════════════════════════════════════════════════════════════════
```

## L379 · `pwm(mmio, R_DPD_COM + ((path as u32) << 8), B_DPD_COM_OF, 0x1);`

```
// cv > CHIP_CAV: our hal reports cv=2 so always apply.
```

## L383-385 · `fn lbk_rxiqk(mmio: i32, path: u8) {`

```
// ═══════════════════════════════════════════════════════════════════
//  _dpk_lbk_rxiqk (rfk.c:1832)
// ═══════════════════════════════════════════════════════════════════
```

## L424-426 · `fn get_thermal(mmio: i32, path: u8, kidx: u8) {`

```
// ═══════════════════════════════════════════════════════════════════
//  _dpk_get_thermal (rfk.c:1876)
// ═══════════════════════════════════════════════════════════════════
```

## L436-438 · `fn rf_setting(mmio: i32, path: u8, kidx: u8) {`

```
// ═══════════════════════════════════════════════════════════════════
//  _dpk_rf_setting (rfk.c:1892)
// ═══════════════════════════════════════════════════════════════════
```

## L442 · `if band == 0 /* 2G */ {`

```
/* 2G */
```

## L461-463 · `fn bypass_rxcfir(mmio: i32, path: u8, enable: bool) {`

```
// ═══════════════════════════════════════════════════════════════════
//  _dpk_bypass_rxcfir (rfk.c:1923)
// ═══════════════════════════════════════════════════════════════════
```

## L475-477 · `fn tpg_sel(mmio: i32, path: u8, kidx: u8) {`

```
// ═══════════════════════════════════════════════════════════════════
//  _dpk_tpg_sel (rfk.c:1946)
// ═══════════════════════════════════════════════════════════════════
```

## L484-486 · `fn table_select(mmio: i32, path: u8, kidx: u8, gain: u8) {`

```
// ═══════════════════════════════════════════════════════════════════
//  _dpk_table_select (rfk.c:1962)
// ═══════════════════════════════════════════════════════════════════
```

## L492-494 · `fn sync_check(mmio: i32, path: u8, kidx: u8) -> bool {`

```
// ═══════════════════════════════════════════════════════════════════
//  _dpk_sync_check / _dpk_sync (rfk.c:1974/2016)
// ═══════════════════════════════════════════════════════════════════
```

## L518-520 · `fn dgain_read(mmio: i32) -> u16 {`

```
// ═══════════════════════════════════════════════════════════════════
//  DGain read + mapping (rfk.c:2024/2037)
// ═══════════════════════════════════════════════════════════════════
```

## L549-551 · `fn gainloss_read(mmio: i32) -> u8 {`

```
// ═══════════════════════════════════════════════════════════════════
//  Gain loss (rfk.c:2085/2093)
// ═══════════════════════════════════════════════════════════════════
```

## L563-565 · `fn kip_preset(mmio: i32, path: u8, kidx: u8) {`

```
// ═══════════════════════════════════════════════════════════════════
//  KIP preset / pwr_clk / set_txagc / set_rxagc (rfk.c:2100..2144)
// ═══════════════════════════════════════════════════════════════════
```

## L593-595 · `fn set_offset(mmio: i32, path: u8, gain_offset: i8) -> u8 {`

```
// ═══════════════════════════════════════════════════════════════════
//  _dpk_set_offset (rfk.c:2146)
// ═══════════════════════════════════════════════════════════════════
```

## L610-612 · `fn pas_read(mmio: i32, is_check: bool) -> bool {`

```
// ═══════════════════════════════════════════════════════════════════
//  _dpk_pas_read (rfk.c:2167)
// ═══════════════════════════════════════════════════════════════════
```

## L632-634 · `fn agc(mmio: i32, path: u8, kidx: u8, init_txagc: u8, loss_only: bool) -> u8 {`

```
// ═══════════════════════════════════════════════════════════════════
//  _dpk_agc (rfk.c:2208) — the FSM
// ═══════════════════════════════════════════════════════════════════
```

## L665 · `bypass_rxcfir(mmio, path, true);`

```
// bw < 80 → bypass; else lbk_rxiqk. Our chan is 20M so bypass.
```

## L705-707 · `fn set_mdpd_para(mmio: i32, order: u32) {`

```
// ═══════════════════════════════════════════════════════════════════
//  _dpk_set_mdpd_para + _dpk_idl_mpa (rfk.c:2327/2355)
// ═══════════════════════════════════════════════════════════════════
```

## L727 · `if bw < 2 /* <80M */ && band == 1 /* 5G */ {`

```
/* <80M */
```

## L727 · `if bw < 2 /* <80M */ && band == 1 /* 5G */ {`

```
/* 5G */
```

## L735-737 · `fn fill_result(mmio: i32, path: u8, kidx: u8, gain: u8, txagc: u8) {`

```
// ═══════════════════════════════════════════════════════════════════
//  _dpk_fill_result (rfk.c:2369)
// ═══════════════════════════════════════════════════════════════════
```

## L740 · `let gs = st().dpk_gs[0]; // phy 0`

```
// phy 0
```

## L765-767 · `fn reload_check(mmio: i32, path: u8, band: u8, ch: u8) -> bool {`

```
// ═══════════════════════════════════════════════════════════════════
//  _dpk_reload_check (rfk.c:2408)
// ═══════════════════════════════════════════════════════════════════
```

## L780-782 · `fn dpk_main(mmio: i32, path: u8, gain: u8) -> bool {`

```
// ═══════════════════════════════════════════════════════════════════
//  _dpk_main (rfk.c:2435) — per-path cal
// ═══════════════════════════════════════════════════════════════════
```

## L787 · `rw(mmio, path, RR_RSV1, RR_RSV1_RST, 0x0);`

```
// _rfk_rf_direct_cntrl(path, false) → RR_RSV1 RST = 0
```

## L789 · `rw(mmio, path, RR_BBDC, RR_BBDC_SEL, 0x0);`

```
// _rfk_drf_direct_cntrl(path, false) → RR_BBDC SEL = 0
```

## L820-822 · `fn cal_select(mmio: i32, band: u8, ch: u8, bw: u8) {`

```
// ═══════════════════════════════════════════════════════════════════
//  _dpk_cal_select (rfk.c:2484)
// ═══════════════════════════════════════════════════════════════════
```

## L824 · `let kip_reg: [u32; KIP_REG_NUM] = [0x813C, 0x8124, 0x8120];`

```
// KIP backup (3 regs × 2 paths)
```

## L830 · `let mut reloaded = [false; DPK_RF_PATH];`

```
// reload_check
```

## L845 · `const BB_REGS: [u32; 3] = [0x2344, 0x5800, 0x7800];`

```
// Backup BB + RF regs (same as IQK — rtw8852b_backup_bb_regs/rf_regs)
```

## L851 · `for i in 0..KIP_REG_NUM {`

```
// backup KIP
```

## L855 · `for i in 0..11 { rf_bak[path as usize][i] = rr(mmio, path, RF_REGS[i]); }`

```
// backup RF
```

## L858 · `tssi_pause(mmio, path, true);`

```
// TSSI pause (is_tssi_mode is true after TSSI setup)
```

## L885-887 · `pub fn init(mmio: i32) {`

```
// ═══════════════════════════════════════════════════════════════════
//  _set_dpd_backoff (rfk.c:2692) → dpk_init
// ═══════════════════════════════════════════════════════════════════
```

## L902-904 · `pub fn force_bypass(mmio: i32) {`

```
// ═══════════════════════════════════════════════════════════════════
//  _dpk_force_bypass (rfk.c:2562)
// ═══════════════════════════════════════════════════════════════════
```

## L910-912 · `pub fn run(mmio: i32, band: u8, ch: u8, bw: u8) {`

```
// ═══════════════════════════════════════════════════════════════════
//  rtw8852b_dpk (rfk.c:3790) — public per-channel entry
// ═══════════════════════════════════════════════════════════════════
```

## L920-923 · `let tx_en = fw::stop_sch_tx(mmio, 0);`

```
// No eFEM for 8852B (non-ePA) → always run cal.
// Linux: rtw89_chip_stop_sch_tx + _wait_rx_mode before _dpk, and
// rtw89_chip_resume_sch_tx after. Handled by our iqk::wait_rx_mode_pub
// and fw::stop_sch_tx / resume_sch_tx wrappers.
```

