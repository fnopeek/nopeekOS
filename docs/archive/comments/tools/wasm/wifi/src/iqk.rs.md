# `tools/wasm/wifi/src/iqk.rs` @ 5e0102684

## L1-26 · `use crate::host;`

```
//! IQK — IQ Imbalance Calibration — full 1:1 port of Linux rtw8852b_rfk.c.
//!
//! Linux entry: rtw8852b_rfk.c:3757 rtw8852b_iqk → _iqk → _doiqk → _iqk_by_path
//!
//! Coverage (1:1 with Linux, 2G + 5G branches both present):
//!   _iqk_init           (rfk.c:1538)
//!   _iqk_macbb_setting  (rfk.c:1514) — apply RTW8852B_SET_NONDBCC_PATH01
//!   _iqk_preset         (rfk.c:1493)
//!   _iqk_txclk_setting  (rfk.c:1303)
//!   _iqk_rxclk_setting  (rfk.c:977)
//!   _iqk_rxk_setting    (rfk.c:792)
//!   _iqk_txk_setting    (rfk.c:1273)
//!   _lok_res_table      (rfk.c:1127)
//!   _lok_finetune_check (rfk.c:1147)
//!   _iqk_lok            (rfk.c:1191)  — 3x retry via _iqk_by_path
//!   _txk_group_sel      (rfk.c:1016)
//!   _rxk_group_sel      (rfk.c:870)
//!   _iqk_nbtxk          (rfk.c:1079)
//!   _iqk_nbrxk          (rfk.c:928)
//!   _iqk_one_shot       (rfk.c:815)   — all ktypes
//!   _iqk_check_cal      (rfk.c:253)
//!   _iqk_restore        (rfk.c:1439)
//!   _iqk_afebb_restore  (rfk.c:1467) — apply RTW8852B_RESTORE_NONDBCC_PATH01
//!
//! Linux is_nbiqk defaults to false → we run _txk_group_sel + _rxk_group_sel
//! (wide-band, 4 group iterations each), matching Linux behaviour.
```

## L36 · `const CR: u32 = 0x10000; // PHY_CR_BASE`

```
// PHY_CR_BASE
```

## L38 · `const R_IQKINF: u32        = 0x9FE0;`

```
// ── PHY register addresses (reg.h — verified 1:1) ─────────────────
```

## L54 · `const R_NCTL_N1: u32       = 0x8010; // was WRONG (0x8004) in previous version`

```
// was WRONG (0x8004) in previous version
```

## L61 · `const R_KIP_SYSCFG: u32    = 0x8088; // was WRONG (0x8240) in previous version`

```
// was WRONG (0x8240) in previous version
```

## L62 · `const R_COEF_SEL: u32      = 0x8104;       // + path<<8`

```
// + path<<8
```

## L67 · `const R_IQK_RES: u32       = 0x8124;       // + path<<8`

```
// + path<<8
```

## L71 · `const R_TXIQC: u32         = 0x8138;       // + path<<8 — was WRONG (0x81D8)`

```
// + path<<8 — was WRONG (0x81D8)
```

## L72 · `const R_RXIQC: u32         = 0x813C;       // + path<<8 — was WRONG (0x8220)`

```
// + path<<8 — was WRONG (0x8220)
```

## L74 · `const R_CFIR_LUT: u32      = 0x8154;       // + path<<8`

```
// + path<<8
```

## L77 · `const B_CFIR_LUT_G3: u32   = 1 << 3;       // was WRONG (1<<20)`

```
// was WRONG (1<<20)
```

## L79 · `const B_CFIR_LUT_GP_V1: u32 = 0x7 << 0;    // 3-bit (RXK uses this)`

```
// 3-bit (RXK uses this)
```

## L80 · `const B_CFIR_LUT_GP: u32   = 0x3 << 0;     // 2-bit (TXK uses this)`

```
// 2-bit (TXK uses this)
```

## L82 · `const R_KIP_IQP: u32       = 0x81CC;       // + path<<8`

```
// + path<<8
```

## L107 · `const R_RFK_ST: u32        = 0xBFF8;       // cal status poll`

```
// cal status poll
```

## L109 · `const RR_MOD: u32          = 0x00;`

```
// ── RF register addresses (reg.h — u32 with high bank bits) ───────
```

## L111 · `const RR_MOD_IQK: u32      = 0xFFFF0;      // GENMASK(19,4)`

```
// GENMASK(19,4)
```

## L112 · `const RR_MOD_MASK: u32     = 0xF0000;      // GENMASK(19,16)`

```
// GENMASK(19,16)
```

## L113 · `const RR_MOD_RGM: u32      = 0x3FF0;       // GENMASK(13,4)`

```
// GENMASK(13,4)
```

## L117 · `const RR_LOKVB_COI: u32    = 0x3F << 14;   // GENMASK(19,14)`

```
// GENMASK(19,14)
```

## L118 · `const RR_LOKVB_COQ: u32    = 0x3F << 4;    // GENMASK(9,4)`

```
// GENMASK(9,4)
```

## L120 · `const RR_TXIG_GR0: u32     = 0x3;          // [1:0]`

```
// [1:0]
```

## L121 · `const RR_TXIG_GR1: u32     = 0x7 << 4;     // [6:4]`

```
// [6:4]
```

## L122 · `const RR_TXIG_TG: u32      = 0x1F << 12;   // [16:12]`

```
// [16:12]
```

## L138 · `const RR_TXGA_LOK_EXT: u32 = 0x1F << 0;    // GENMASK(4,0)`

```
// GENMASK(4,0)
```

## L140 · `const RR_TXMO_COI: u32     = 0x1F << 15;   // GENMASK(19,15)`

```
// GENMASK(19,15)
```

## L141 · `const RR_TXMO_COQ: u32     = 0x1F << 10;   // GENMASK(14,10)`

```
// GENMASK(14,10)
```

## L143 · `const RR_BIASA_A: u32      = 0x7 << 0;     // GENMASK(2,0)`

```
// GENMASK(2,0)
```

## L147 · `const RR_RXBB_C2G: u32     = 0x7F << 10;   // GENMASK(16,10)`

```
// GENMASK(16,10)
```

## L148 · `const RR_RXBB_C1G: u32     = 0x3 << 8;     // GENMASK(9,8)`

```
// GENMASK(9,8)
```

## L150 · `const RR_XGLNA2_SW: u32    = 0x3 << 0;     // GENMASK(1,0)`

```
// GENMASK(1,0)
```

## L152 · `const RR_RXA2_HATT: u32    = 0x7F;         // GENMASK(6,0)`

```
// GENMASK(6,0)
```

## L153 · `const RR_RXA2_CC2: u32     = 0x3 << 7;     // GENMASK(8,7)`

```
// GENMASK(8,7)
```

## L155 · `const RR_XALNA2_SW2: u32   = 0x3 << 8;     // GENMASK(9,8)`

```
// GENMASK(9,8)
```

## L158 · `const RR_BBDC: u32         = 0x10005;      // bit16 = ad_sel=1 (direct MMIO)`

```
// bit16 = ad_sel=1 (direct MMIO)
```

## L163 · `const ID_FLOK_COARSE:  u8  = 0x1;`

```
// ── IQK one-shot command IDs (Linux rtw8852b_iqk_type enum) ───────
```

## L172 · `const RXK_GROUP_NR: usize = 4;`

```
// ── Group constants (Linux rtw8852b_rfk.c:98..111) ────────────────
```

## L194-195 · `struct IqkState {`

```
// ── IQK state (per path) ──────────────────────────────────────────
// Mirrors the subset of rtw89_iqk_info that _iqk_by_path + sub-funcs read/write.
```

## L197 · `band: [u8; 2],   // iqk_band[path]  (BAND_2G/5G)`

```
// iqk_band[path]  (BAND_2G/5G)
```

## L198 · `bw: [u8; 2],     // iqk_bw[path]    (BW_20M etc.)`

```
// iqk_bw[path]    (BW_20M etc.)
```

## L199 · `ch: [u8; 2],     // iqk_ch[path]`

```
// iqk_ch[path]
```

## L210-213 · `const BACKUP_BB_REGS: [u32; 3] = [0x2344, 0x5800, 0x7800];`

```
// ── BB/RF backup tables (Linux rtw8852b_backup_bb_regs / _rf_regs) ─
// IQK corrupts these; Linux _doiqk backs them up before and restores after.
// Skipping this step leaves TXPW_RSTB + R_RXCCA + per-path RF in IQK state
// after IQK finishes → RX pipe never recovers. rfk.c:113.
```

## L215 · `const BACKUP_RF_REGS: [u32; 11] = [`

```
// 11 RF registers backed up per path. 0x10005 has bit16 (ad_sel=1).
```

## L220-222 · `fn udelay_1() { for _ in 0..1000 { core::hint::spin_loop(); } }`

```
// ═══════════════════════════════════════════════════════════════════
//  Helpers
// ═══════════════════════════════════════════════════════════════════
```

## L224-227 · `fn udelay_1() { for _ in 0..1000 { core::hint::spin_loop(); } }`

```
// Each iteration includes an MMIO read (~300-500ns on PCIe) plus spin_loops.
// Linux uses udelay(1) = real 1 microsecond. Our old 100×spin_loop was
// ~100-300ns — 5-10× too short. Bumped to 1000 spin_loops for ~1-3µs floor;
// combined with the MMIO read this stays >= 1µs per iteration.
```

## L252-253 · `pub fn wait_rx_mode_pub(mmio: i32) { wait_rx_mode(mmio); }`

```
/// _wait_rx_mode — poll RR_MOD.MOD_MASK until != 2 (TX). Linux rfk.c:1569.
/// Aborts IQK if the chip is stuck in TX mode at IQK entry.
```

## L282-284 · `fn iqk_check_cal(mmio: i32, path: u8) -> bool {`

```
// ═══════════════════════════════════════════════════════════════════
//  _iqk_check_cal — Linux rfk.c:253
// ═══════════════════════════════════════════════════════════════════
```

## L287-289 · `for _ in 0..20000u32 {`

```
// Linux: read_poll_timeout_atomic(1us, 8200us). Our udelay_1 may still
// undershoot, so give the loop more iterations (20000) to guarantee
// the calibration has at least ~20ms of wall clock to settle.
```

## L306-308 · `fn iqk_one_shot(mmio: i32, state: &IqkState, path: u8, ktype: u8) -> bool {`

```
// ═══════════════════════════════════════════════════════════════════
//  _iqk_one_shot — Linux rfk.c:815, all ktypes
// ═══════════════════════════════════════════════════════════════════
```

## L354-356 · `fn iqk_txclk_setting(mmio: i32, _path: u8) {`

```
// ═══════════════════════════════════════════════════════════════════
//  _iqk_txclk_setting — rfk.c:1303
// ═══════════════════════════════════════════════════════════════════
```

## L369-371 · `fn iqk_rxclk_setting(mmio: i32, _path: u8) {`

```
// ═══════════════════════════════════════════════════════════════════
//  _iqk_rxclk_setting — rfk.c:977 (20M/40M branch — BW_20M here)
// ═══════════════════════════════════════════════════════════════════
```

## L373 · `pwm(mmio, R_P0_NRBW,     B_P0_NRBW_DBG,  1);`

```
// BW != 80MHz branch
```

## L391-393 · `fn iqk_rxk_setting(mmio: i32, state: &IqkState, path: u8) {`

```
// ═══════════════════════════════════════════════════════════════════
//  _iqk_rxk_setting — rfk.c:792
// ═══════════════════════════════════════════════════════════════════
```

## L412-414 · `fn iqk_txk_setting(mmio: i32, state: &IqkState, path: u8) {`

```
// ═══════════════════════════════════════════════════════════════════
//  _iqk_txk_setting — rfk.c:1273
// ═══════════════════════════════════════════════════════════════════
```

## L441-443 · `fn lok_res_table(mmio: i32, state: &IqkState, path: u8, ibias: u8) {`

```
// ═══════════════════════════════════════════════════════════════════
//  _lok_res_table — rfk.c:1127
// ═══════════════════════════════════════════════════════════════════
```

## L456-458 · `fn lok_finetune_check(mmio: i32, path: u8) -> bool {`

```
// ═══════════════════════════════════════════════════════════════════
//  _lok_finetune_check — rfk.c:1147 (returns true = fail)
// ═══════════════════════════════════════════════════════════════════
```

## L473-475 · `fn iqk_lok(mmio: i32, state: &mut IqkState, path: u8) -> bool {`

```
// ═══════════════════════════════════════════════════════════════════
//  _iqk_lok — rfk.c:1191 (both 2G and 5G)
// ═══════════════════════════════════════════════════════════════════
```

## L490 · `rw(mmio, path, RR_TXIG, RR_TXIG_TG, 0x0);  // 2G and 5G both`

```
// 2G and 5G both
```

## L496 · `rw(mmio, path, RR_TXIG, RR_TXIG_TG, 0x12); // both bands`

```
// both bands
```

## L516-518 · `fn txk_group_sel(mmio: i32, state: &mut IqkState, path: u8) -> bool {`

```
// ═══════════════════════════════════════════════════════════════════
//  _txk_group_sel — rfk.c:1016 (wide-band: 4 groups)
// ═══════════════════════════════════════════════════════════════════
```

## L559-561 · `#[allow(dead_code)]`

```
// ═══════════════════════════════════════════════════════════════════
//  _iqk_nbtxk — rfk.c:1079 (narrow-band TX: single group)
// ═══════════════════════════════════════════════════════════════════
```

## L595-597 · `fn rxk_group_sel(mmio: i32, state: &mut IqkState, path: u8) -> bool {`

```
// ═══════════════════════════════════════════════════════════════════
//  _rxk_group_sel — rfk.c:870 (wide-band: 4 groups)
// ═══════════════════════════════════════════════════════════════════
```

## L636-638 · `#[allow(dead_code)]`

```
// ═══════════════════════════════════════════════════════════════════
//  _iqk_nbrxk — rfk.c:928 (narrow-band RX: single group 0x3)
// ═══════════════════════════════════════════════════════════════════
```

## L674-676 · `fn iqk_preset(mmio: i32, path: u8) {`

```
// ═══════════════════════════════════════════════════════════════════
//  _iqk_preset — rfk.c:1493
// ═══════════════════════════════════════════════════════════════════
```

## L678 · `let idx: u32 = 0; // rfk_mcc.table_idx — always 0 for us`

```
// rfk_mcc.table_idx — always 0 for us
```

## L688-690 · `fn apply_reg3(mmio: i32, table: &[(u32, u32, u32)]) {`

```
// ═══════════════════════════════════════════════════════════════════
//  _iqk_macbb_setting + _iqk_afebb_restore — rfk.c:1514/1467
// ═══════════════════════════════════════════════════════════════════
```

## L697-699 · `fn iqk_restore(mmio: i32, state: &IqkState, path: u8) {`

```
// ═══════════════════════════════════════════════════════════════════
//  _iqk_restore — rfk.c:1439
// ═══════════════════════════════════════════════════════════════════
```

## L720-722 · `fn iqk_by_path(mmio: i32, state: &mut IqkState, path: u8) {`

```
// ═══════════════════════════════════════════════════════════════════
//  _iqk_by_path — rfk.c:1347 (main dispatcher: LOK + TXK + RXK)
// ═══════════════════════════════════════════════════════════════════
```

## L726 · `let mut ibias: u8 = 0x1;`

```
// LOK with 3x retry (ibias starts at 1, increments each try)
```

## L742 · `state.tx_fail[path as usize] = txk_group_sel(mmio, state, path);`

```
// TXK — wide-band (is_nbiqk=false default)
```

## L745 · `iqk_rxclk_setting(mmio, path);`

```
// RX
```

## L750 · `let off4 = (path as u32) * 4;`

```
// _iqk_info_iqk (debug only — write fail flags into R_IQKINF)
```

## L758-761 · `pub fn run(mmio: i32) {`

```
// ═══════════════════════════════════════════════════════════════════
//  Public entry — runs the complete IQK flow for path A+B
//  Mirrors Linux _doiqk (rfk.c:1596) for the RF_AB kpath.
// ═══════════════════════════════════════════════════════════════════
```

## L765-769 · `let tx_en_saved = fw::stop_sch_tx(mmio, 0);`

```
// ── TX scheduler pause — 1:1 Linux rtw8852b_iqk (rtw8852b_rfk.c:3764).
// IQK measures TX LO leakage / I-Q mismatch; if the CMAC scheduler is
// still firing TX slots (even NOP) the cal engine sees stale TX energy
// and LOK produces values outside [0x02..0x1D] → cor/fin fail.
// FW is running, so Linux routes this through H2CREG SCH_TX_EN.
```

## L775-776 · `let mut state = IqkState {`

```
// State — defaults match rtw89_iqk_info after _iqk_init.
// Channel is 2G ch 7 @ 20 MHz (set by chan::set_channel_2g(mmio, 7)).
```

## L791-792 · `wait_rx_mode(mmio);`

```
// _wait_rx_mode — make sure the radio is in RX (not TX) before IQK.
// Linux rtw8852b_iqk calls this under chip_stop_sch_tx.
```

## L795 · `pw(mmio, R_IQKINF, 0);`

```
// _iqk_init — R_IQKINF = 0
```

## L798 · `pwm(mmio, R_IQKINF, B_IQKINF_VER, IQK_VER);`

```
// _iqk_get_ch_info — write version/band/bw/ch into R_IQKINF/R_IQKCH
```

## L807-810 · `for path in 0u8..2 {`

```
// Per-path: backup BB+RF, run full IQK path, restore BB+RF.
// Linux _doiqk pattern — without backup/restore, IQK permanently
// corrupts 3 BB regs (0x2344, 0x5800, 0x7800) and 11 RF regs per
// path, leaving the RX pipe in IQK state after IQK finishes.
```

## L817 · `apply_reg3(mmio, RTW8852B_SET_NONDBCC_PATH01);`

```
// _iqk_macbb_setting — apply SET_NONDBCC table per path
```

## L827 · `apply_reg3(mmio, RTW8852B_RESTORE_NONDBCC_PATH01);`

```
// _iqk_afebb_restore — apply RESTORE_NONDBCC
```

## L835 · `fw::resume_sch_tx(mmio, 0, tx_en_saved);`

```
// Resume TX scheduler — Linux rtw8852b_iqk line 3770.
```

## L839 · `host::print("  IQK: done | A: cor=");`

```
// Final status report
```

