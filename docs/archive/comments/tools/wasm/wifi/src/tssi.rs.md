# `tools/wasm/wifi/src/tssi.rs` @ 5e0102684

## L1-17 · `use crate::host;`

```
//! TSSI — Transmit Signal Strength Indicator calibration.
//!
//! 1:1 Port of Linux rtw8852b_rfk.c TSSI functions. Without TSSI the
//! PA runs open-loop and real output power is unknown — frames leave
//! the chip at undefined amplitude, APs see noise.
//!
//! Linux entry: rtw8852b_tssi(phy_idx, hwtx_en=true, chan).
//! We skip _tssi_alimentk (the TX-loop cal with sch_tx pause) for
//! Phase 1 — it requires hwtx_en and is a heavy multi-ms auto-cal.
//! Setup-only run lets the TSSI tracking hardware become live without
//! the per-level alignment sweep.
//!
//! Efuse dependencies: _tssi_set_tmeter_tbl reads per-path thermal from
//! efuse; with thermal=0xff (our default) it falls back to writing
//! zero offsets — HW uses the default delta table.
//! _tssi_set_efuse_to_de reads per-channel DE values from efuse;
//! without efuse we skip it (HW keeps defaults from the tables above).
```

## L27-29 · `const DELTA_SWINGIDX_SIZE: usize = 30;`

```
// DELTA_SWINGIDX tables for 2G path A/B (rtw8852b_table.c 14637-14651).
// 30 entries each (DELTA_SWINGIDX_SIZE = 30). Used by set_tmeter_tbl to
// build the 64-byte thermal offset table when efuse thermal is valid.
```

## L49 · `const RR_TXPOW: u32       = 0x7F;`

```
// ── Register addresses (reg.h) ──────────────────────────────────
```

## L57 · `const R_P0_TMETER: u32    = 0x5810;`

```
// Path A PHY registers
```

## L59 · `const B_P0_TMETER: u32    = 0xFC00;      // GENMASK(15,10)`

```
// GENMASK(15,10)
```

## L65 · `const B_P0_TSSI_RFC: u32  = 0x18000000;  // GENMASK(28,27)`

```
// GENMASK(28,27)
```

## L66 · `const B_P0_TSSI_OFT: u32  = 0xFF;         // GENMASK(7,0)`

```
// GENMASK(7,0)
```

## L71 · `const B_P0_RFCTM_VAL: u32 = 0x03F00000;   // GENMASK(25,20)`

```
// GENMASK(25,20)
```

## L74 · `const B_P0_TSSI_MV_MIX: u32 = 0x000FF800; // GENMASK(19,11)`

```
// GENMASK(19,11)
```

## L78 · `const R_P1_TMETER: u32    = 0x7810;`

```
// Path B PHY registers
```

## L96 · `const B_P1_RFCTM_DEL: u32 = 0x000FF800;   // same layout as P0`

```
// same layout as P0
```

## L99 · `fn pwm(mmio: i32, addr: u32, mask: u32, val: u32) {`

```
// ── Helpers ─────────────────────────────────────────────────────
```

## L110 · `fn rf_setting(mmio: i32, path: u8, band: u8) {`

```
// ── TSSI sub-functions (rtw8852b_rfk.c 2719-3104) ───────────────
```

## L112 · `fn rf_setting(mmio: i32, path: u8, band: u8) {`

```
/// _tssi_rf_setting — rfk.c:2719. Enables TX PA chain for current band.
```

## L121 · `fn set_sys(mmio: i32, path: u8, band: u8) {`

```
/// _tssi_set_sys — rfk.c:2730. Applies sys-wide defs + per-path/band defs.
```

## L139 · `fn ini_txpwr_ctrl_bb(mmio: i32, path: u8) {`

```
/// _tssi_ini_txpwr_ctrl_bb — rfk.c:2747. BB txpwr ctrl init per path.
```

## L148 · `fn ini_txpwr_ctrl_bb_he_tb(mmio: i32, path: u8) {`

```
/// _tssi_ini_txpwr_ctrl_bb_he_tb — rfk.c:2756. HE-TB BB txpwr ctrl.
```

## L157 · `fn set_dck(mmio: i32, path: u8) {`

```
/// _tssi_set_dck — rfk.c:2765. DC-K per path.
```

## L166-169 · `fn build_thm_ofst(up: &[i8; DELTA_SWINGIDX_SIZE], down: &[i8; DELTA_SWINGIDX_SIZE])`

```
/// Build the 64-byte thermal offset table per Linux _tssi_set_tmeter_tbl:
///   thm_ofst[0..32]  = -thm_down[i], clamped to last entry
///   thm_ofst[32..64] =  thm_up[i],   in reverse from index 63 downwards
/// (Linux rfk.c:2853-2863.)
```

## L193 · `fn pack4(t: &[i8; 64], idx: usize) -> u32 {`

```
/// Pack 4 s8 bytes into a little-endian u32 (Linux RTW8852B_TSSI_GET_VAL).
```

## L201-205 · `fn set_tmeter_tbl(mmio: i32, path: u8, thermal: u8) {`

```
/// _tssi_set_tmeter_tbl — rfk.c:2773. Efuse thermal drives the delta
/// tables. thermal=0xff → fallback (zero offsets, TMETER=32). With a
/// real thermal value, writes the thermal into TMETER + RFCTM_VAL and
/// fills the 64-byte offset table from the DELTA_SWINGIDX_2G{A,B}_{N,P}
/// constants above.
```

## L221 · `pwm(mmio, r_tmeter, b_tmeter,    32);`

```
// Fallback: TMETER = 32, all offsets = 0.
```

## L228 · `pwm(mmio, r_tmeter, b_tmeter,    thermal as u32);`

```
// Real thermal: program it + build delta table.
```

## L247 · `fn set_dac_gain_tbl(mmio: i32, path: u8) {`

```
/// _tssi_set_dac_gain_tbl — rfk.c:2930.
```

## L256 · `fn slope_cal_org(mmio: i32, path: u8, band: u8) {`

```
/// _tssi_slope_cal_org — rfk.c:2938.
```

## L267-268 · `fn alignment_default(mmio: i32, path: u8, band: u8, ch: u8) {`

```
/// _tssi_alignment_default — rfk.c:2953. For 2G ch 1-14 pick _2g_ tables.
/// `all=true` for full alignment apply (what rtw8852b_tssi calls).
```

## L270 · `if band == BAND_2G && ch >= 1 && ch <= 14 {`

```
// 2G ch 1..14
```

## L276 · `if band == BAND_5G {`

```
// 5G ch ranges
```

## L291 · `fn set_tssi_slope(mmio: i32, path: u8) {`

```
/// _tssi_set_tssi_slope — rfk.c:3011.
```

## L300-301 · `fn set_tssi_track(mmio: i32, path: u8) {`

```
/// _tssi_set_tssi_track — rfk.c:3019. Clear TSSIC_BYPASS so tracking
/// feedback loop runs.
```

## L310 · `fn set_txagc_offset_mv_avg(mmio: i32, path: u8) {`

```
/// _tssi_set_txagc_offset_mv_avg — rfk.c:3028.
```

## L319-333 · `const TSSI_DE_MASK: u32 = 0x003F_F000; // GENMASK(21,12)`

```
// ── _tssi_set_efuse_to_de (rfk.c:3296) ─────────────────────────
//
// Writes per-channel CCK and MCS DE values into the BB from the
// efuse-derived tssi_cck / tssi_mcs / tssi_trim arrays.
//
// The BB target registers are per-path + per-bandwidth:
//   CCK long:   path A 0x5858 / path B 0x7858
//   CCK short:  path A 0x5860 / path B 0x7860
//   MCS  5m:    path A 0x5828 / path B 0x7828
//   MCS 10m:    path A 0x5830 / path B 0x7830
//   MCS 20m:    path A 0x5838 / path B 0x7838
//   MCS 40m:    path A 0x5840 / path B 0x7840
//   MCS 80m:    path A 0x5848 / path B 0x7848
//   MCS 80_80m: path A 0x5850 / path B 0x7850
// Each register bit field: _TSSI_DE_MASK = GENMASK(21,12).
```

## L335 · `const TSSI_DE_MASK: u32 = 0x003F_F000; // GENMASK(21,12)`

```
// GENMASK(21,12)
```

## L346 · `fn cck_group(ch: u8) -> usize {`

```
/// _tssi_get_cck_group — rfk.c:3106. 2G ch 1..14 → group 0..5.
```

## L359-361 · `const EXTRA: u32 = 1 << 31;`

```
/// _tssi_get_ofdm_group — rfk.c:3132. Full table ported. High bit 31
/// marks an "extra" group where the DE is averaged between two
/// adjacent groups. Returns packed u32 exactly as Linux does.
```

## L399 · `fn trim_group(ch: u8) -> usize {`

```
/// _tssi_get_trim_group — rfk.c:3200. 2G ch 1..14 → 0..1, 5G → 2..7.
```

## L414-415 · `fn get_mcs_de(e: &crate::efuse::EfuseData, path: usize, ch: u8) -> i8 {`

```
/// Look up MCS DE from efuse tssi_mcs arrays (2G or 5G depending on
/// group idx). Handles "EXTRA" averaged groups like Linux.
```

## L441-442 · `pub fn set_efuse_to_de(mmio: i32, e: &crate::efuse::EfuseData, ch: u8) {`

```
/// _tssi_set_efuse_to_de — rfk.c:3296. Writes CCK + MCS DE values for
/// the current channel into 8 BB registers per path (16 total).
```

## L451 · `let cck_val = ((cck_base as i32) + (trim as i32)) as u32 & 0x3FF;`

```
// Linux: val = (s32)cck_base + trim_de (_TSSI_DE_MASK keeps it in 10 bits).
```

## L467-468 · `fn enable(mmio: i32) {`

```
/// _tssi_enable — rfk.c:3041. Turns on TSSI hardware tracking per path.
/// This is the "TSSI mode ON" step.
```

## L496 · `fn disable(mmio: i32) {`

```
/// _tssi_disable — rfk.c:3093. Called at TSSI entry to clear state.
```

## L506-512 · `const ALIM_REG_P0: [u32; 4] = [0x5630, 0x5634, 0x563C, 0x5640]; // ALIM1/3/2/4 (Linux order)`

```
// ═══════════════════════════════════════════════════════════════════
//  Public entry — rtw8852b_tssi (phase 1: setup only, no alimentk)
//
//  Channel parameters: band (2G=0, 5G=1), channel number.
//  hwtx_en is accepted for future alimentk integration; currently
//  ignored (we never run the TX alignment loop).
// ═══════════════════════════════════════════════════════════════════
```

## L514-524 · `const ALIM_REG_P0: [u32; 4] = [0x5630, 0x5634, 0x563C, 0x5640]; // ALIM1/3/2/4 (Linux order)`

```
// ── alimentk (rfk.c:3559): the actual TX-loop auto-calibration ───
//
// Sends 4 test-TX bursts at decreasing power levels, reads the TSSI
// feedback ADC CW value for each, and computes per-path alignment
// offsets that get written into the TSSI_ALIM1/2/3/4 BB registers.
// These offsets are what normal (non-PMAC) TX uses to know how to
// drive the PA for a target output power.
//
// Without alimentk the TSSI loop tracks against defaults, so actual
// output power for a given target dBm is unknown. Linux always runs
// this per channel; it's why every AP replies to their Probe Reqs.
```

## L526 · `const ALIM_REG_P0: [u32; 4] = [0x5630, 0x5634, 0x563C, 0x5640]; // ALIM1/3/2/4 (Linux order)`

```
// ALIM1/3/2/4 (Linux order)
```

## L529 · `const CW_DEFAULT_ADDR: [[u32; 4]; 2] = [`

```
// _tssi_cw_default_addr per path × 4 (rfk.c:85).
```

## L536 · `const B_P0_TSSI_ALIM11: u32 = 0x3FF0_0000; // GENMASK(29,20)`

```
// ALIM write fields (reg.h).
```

## L537 · `const B_P0_TSSI_ALIM11: u32 = 0x3FF0_0000; // GENMASK(29,20)`

```
// GENMASK(29,20)
```

## L538 · `const B_P0_TSSI_ALIM12: u32 = 0x000F_FC00; // GENMASK(19,10)`

```
// GENMASK(19,10)
```

## L539 · `const B_P0_TSSI_ALIM13: u32 = 0x0000_03FF; // GENMASK(9,0)`

```
// GENMASK(9,0)
```

## L540 · `const B_P0_TSSI_ALIM1:  u32 = 0x3FFF_FFFF; // GENMASK(29,0)`

```
// GENMASK(29,0)
```

## L542 · `const B_TSSI_CWRPT:     u32 = 0x0000_01FF;`

```
// TSSI trigger / CW report.
```

## L547 · `const B_P0_TSSI_AVG_F: u32 = 0x0000_F000; // GENMASK(15,12)`

```
// GENMASK(15,12)
```

## L548 · `const B_P0_TSSI_MV_AVG_F: u32 = 0x0000_3800; // GENMASK(13,11)`

```
// GENMASK(13,11)
```

## L557 · `fn sext(v: u32, bits: u32) -> i32 {`

```
/// Sign-extend a `bits`-bit value to i32.
```

## L563 · `fn hw_tx(mmio: i32, path: u8, pwr_dbm: i16, enable: bool) {`

```
/// _tssi_hw_tx wrapper — enable or disable PMAC test-TX for one path.
```

## L568 · `crate::bb::ctrl_rx_path(mmio, crate::bb::RF_PATH_AB);`

```
// Simplified: always route RX to path AB during alimentk.
```

## L575-577 · `fn get_cw_report(mmio: i32, path: u8, power: &[i16]) -> Option<[u32; 2]> {`

```
/// _tssi_get_cw_report — sweeps 2 power levels, waits for CW_RDY,
/// records the CW report per slot. Returns Some([cw0, cw1]) or None
/// on timeout.
```

## L581 · `let (trig_reg, en_mask) = if path == 0 {`

```
// Re-arm TSSI_EN
```

## L590 · `hw_tx(mmio, path, power[j], true);`

```
// Start test-TX at this power
```

## L593 · `let rpt_addr = TSSI_CW_RPT[path as usize];`

```
// Poll CW_RPT_RDY — Linux: 100 × 30µs = 3ms max
```

## L603 · `for _ in 0..3000u32 { core::hint::spin_loop(); }`

```
// ~30 µs
```

## L607 · `hw_tx(mmio, path, power[j], false);`

```
// Stop test-TX
```

## L615-616 · `pub fn alimentk(mmio: i32, path: u8, ch: u8) {`

```
/// alimentk — per-path cal run. Writes alignment offsets into
/// R_P0_TSSI_ALIM1..4 (or P1_*) so regular TX uses calibrated power.
```

## L624 · `let _ = ch; // channel is already tuned`

```
// channel is already tuned
```

## L626 · `let power: [i16; 4] = [48, 20, 4, 4];`

```
// 4 test power levels for 2G (Linux rfk.c:3564).
```

## L629 · `let bak = crate::bb::backup_tssi(mmio);`

```
// Save BB state so we can restore cleanly.
```

## L635 · `pwm(mmio, R_P0_TSSI_AVG,    B_P0_TSSI_AVG_F,    0x8);`

```
// Configure TSSI averaging for the sweep.
```

## L645 · `for i in 0..8 {`

```
// Restore (incl. bb_tx_mode_switch to hand BB back to CMAC)
```

## L655 · `let p = path as usize;`

```
// Compute offsets (rfk.c:3641).
```

## L679-680 · `let packed = (((offset_1 as u32) & 0x3FF) << 20)`

```
// Pack into 30-bit ALIM value: offset_1 in bits 29..20, offset_2 in
// 19..10, offset_3 in 9..0 (all 10-bit signed).
```

## L685 · `let alim1 = if path == 0 { ALIM_REG_P0[0] } else { ALIM_REG_P1[0] };`

```
// Write ALIM1 + ALIM2 per path.
```

## L691 · `for i in 0..8 {`

```
// Restore BB state.
```

## L723-727 · `let tx_en = crate::fw::stop_sch_tx(mmio, 0);`

```
// alimentk re-enabled in v1.42 — v1.28/v1.29 timed out on CW_RPT
// because IQK was running with ADC disabled (v1.38 fixed that),
// so PMAC test-TX had no feedback path. With IQK now clean
// (cor=0 fin=0 tx=0 rx=0) the TSSI alignment loop should see
// real CW reports and calibrate PA output per channel.
```

## L736-738 · `set_efuse_to_de(mmio, e, ch);`

```
// Efuse→DE: per-channel CCK + MCS power correction. The reason TSSI
// runs at all: without this the thermal loop has nothing to correct
// *against*. 16 BB register writes.
```

