# `tools/wasm/wifi/src/chan.rs` @ 5e0102684

## L1-11 · `use crate::host;`

```
//! set_channel(ch=1, 2.4GHz, 20MHz) — minimal 1:1 port of Linux
//! rtw8852b_set_channel for the default scan start channel.
//!
//! Linux entry: rtw8852b.c:580 rtw8852b_set_channel →
//!   __rtw8852bx_set_channel_mac  (rtw8852b_common.c:451)
//!   __rtw8852bx_set_channel_bb   (rtw8852b_common.c:1167)
//!   rtw8852b_set_channel_rf      (rtw8852b_rfk.c:4160 → ctrl_bw_ch)
//!
//! We hardcode ch=1, 20MHz, 2G — minimum needed so FW scan_offload has a
//! baseline channel to work from. Without it the RF is in post-table
//! default state (no frequency tuned) and the receiver is physically deaf.
```

## L16 · `const CR: u32 = 0x10000; // PHY_CR_BASE for rtw89_phy_gen_ax`

```
// PHY_CR_BASE for rtw89_phy_gen_ax
```

## L18-20 · `const RF_BASE_A: u32 = 0xE000;`

```
// RF path write via direct PHY access (8852b uses AX mechanism, not SWSI).
// base_addr[path] + (addr << 2) + CR_BASE.
// Linux phy.c:901 rtw89_phy_read_rf / phy.c:889 write_rf.
```

## L38 · `const RR_CFGCH: u8         = 0x18;`

```
// ── RF register bits (reg.h around 8400) ────────────────────────
```

## L44 · `const RR_CFGCH_CH: u32     = 0xFF;        // [7:0]`

```
// bits for CFGCH
```

## L45 · `const RR_CFGCH_CH: u32     = 0xFF;        // [7:0]`

```
// [7:0]
```

## L46 · `const RR_CFGCH_BW: u32     = 0x3 << 10;   // [11:10]`

```
// [11:10]
```

## L55 · `const RR_LUTWA_M2: u32     = 0x1F;        // [4:0]`

```
// [4:0]
```

## L56 · `const RR_LUTWD0_LB: u32    = 0x3F;        // [5:0]`

```
// [5:0]
```

## L59 · `const R_AX_WMAC_RFMOD: u32          = 0xCC1C;`

```
// ── MAC registers (reg.h) ───────────────────────────────────────
```

## L68 · `const R_PATH0_BAND_SEL_V1: u32  = 0x4738;`

```
// ── PHY registers for BB channel config ─────────────────────────
```

## L73 · `const B_FC0_BW_INV: u32         = 0x7F;         // [6:0]`

```
// [6:0]
```

## L74 · `const B_FC0_BW_SET: u32         = 0x3 << 30;    // [31:30]`

```
// [31:30]
```

## L80 · `const B_RFMODE_ORI_RX_ALL: u32  = 0xFFF << 12;  // [23:12]`

```
// [23:12]
```

## L90 · `const B_RXSCO_TH: u32           = 0x7_FFFF;   // [18:0]`

```
// [18:0]
```

## L92-93 · `const BW_ADC_SEL_A: u32   = 0xC0EC;`

```
// BW setting per-path ADC/WBADC (rtw8852b_bw_setting table):
// For 20MHz: adc_sel = 0 (mask 0x6000), wbadc_sel = 2 (mask 0x30)
```

## L99-103 · `fn sco_mapping_2g(ch: u8) -> u32 {`

```
/// SCO mapping table (rtw8852bx_sco_mapping, rtw8852b_common.c:519):
///   ch 1 → 109
///   ch 2..6 → 108
///   ch 7..10 → 107
///   ch 11..14 → 106
```

## L114 · `const SCO_BARKER: [u32; 14] = [`

```
/// SCO Barker thresholds (rtw8852bx_sco_barker_threshold, ch 1..14)
```

## L120 · `const SCO_CCK: [u32; 14] = [`

```
/// SCO CCK thresholds (rtw8852bx_sco_cck_threshold, ch 1..14)
```

## L126-128 · `pub fn set_channel_1_2g(mmio: i32) { set_channel_2g(mmio, 1); }`

```
// ═══════════════════════════════════════════════════════════════════
//  set_channel(channel=ch, 2.4GHz, 20MHz)  — ch ∈ 1..13
// ═══════════════════════════════════════════════════════════════════
```

## L130 · `pub fn set_channel_1_2g(mmio: i32) { set_channel_2g(mmio, 1); }`

```
/// Convenience: default scan baseline = channel 1.
```

## L138-140 · `host::mmio_clr32(mmio, R_AX_WMAC_RFMOD, B_AX_WMAC_RFMOD_MASK);`

```
// ── 1. set_channel_mac (__rtw8852bx_set_channel_mac) ─────────
//   BW=20MHz → RFMOD mask clr (0 = 20MHz), TX_SUB_CARRIER = 0
//   ch=1 is 2G (<=14) → TXRATE_CHK: set BAND_MODE, clear CCK_EN + RTS_LIMIT
```

## L148-149 · `host::mmio_w32_mask(mmio, CR + R_RXSCOBC, B_RXSCO_TH, SCO_BARKER[ch_idx]);`

```
// ── 2. set_channel_bb ────────────────────────────────────────
//   2a. SCO CCK thresholds per channel (rtw8852bx_ctrl_sco_cck)
```

## L153 · `host::mmio_w32_mask(mmio, CR + R_PATH0_BAND_SEL_V1, B_PATH_BAND_SEL_V1, 1);`

```
//   2b. ctrl_ch: path A/B band_sel = 1 (2G), SCO comp per channel
```

## L158-159 · `let cck_fir: [(u32, u32); 8] = [`

```
//   2c. CCK TX FIR coefficients for ch != 14 (ch 1 case)
//   From rtw8852b_common.c:788-795
```

## L174-179 · `crate::phy::apply_gain_error_2g(mmio, 0);`

```
//   2c.5 set_gain_error(2G, path A+B) — 1:1 Linux rtw8852bx_ctrl_ch.
//   Reads LNA/TIA gain values stored in phy::BB_GAIN during BB-gain
//   table parse and writes them into the per-path LNA/TIA gain
//   registers. Without this step the RX analog front-end has no gain
//   → zero frames reach the MAC. set_gain_offset/set_rxsc_rpl_comp
//   need efuse data (not parsed yet), so they stay out for now.
```

## L183-186 · `host::mmio_w32_mask(mmio, CR + R_FC0_BW_V1, B_FC0_BW_SET, 0);`

```
//   2d. ctrl_bw (20MHz, pri_ch=0):
//       FC0_BW_SET = 0, CHBW_MOD_SBW = 0, CHBW_MOD_PRICH = 0
//       RFMODE_ORI_RX both paths = 0x333
//       bw_setting(20MHz, A+B): adc_sel = 0, wbadc_sel = 2
```

## L192 · `host::mmio_w32_mask(mmio, CR + BW_ADC_SEL_A,   0x6000, 0);`

```
// bw_setting per path (rtw8852b_bw_setting, 20MHz):
```

## L198 · `host::mmio_set32(mmio, CR + R_UPD_CLK_ADC, B_ENABLE_CCK);`

```
//   2e. ctrl_cck_en(true): UPD_CLK_ADC.ENABLE_CCK = 1, RXCCA.DIS = 0
```

## L202 · `host::mmio_w32_mask(mmio, CR + R_MAC_PIN_SEL, B_CH_IDX_SEG0, ch as u32);`

```
//   2f. chan_idx encoding for 2G: BASE_IDX_2G(0)<<4 | ch = ch for 2G
```

## L205-210 · `const R_PATH0_5MDET_V1: u32 = 0x46F8;`

```
//   2g. rtw8852bx_5m_mask (common.c:1022) — for BW=20 MHz the function
//   sets mask_5m_en=false and clears three enable bits. Without these
//   clears stale 5M-detect state from a previous BW setting can bias
//   adjacent-channel interference detection.
//   R_PATH0_5MDET_V1 = 0x46F8 bit 12, R_PATH1_5MDET_V1 = 0x47B8 bit 12,
//   R_ASSIGN_SBD_OPT_V1 = 0x4440 bit 31.
```

## L220-221 · `for path in 0u8..2 {`

```
//   2h. rtw8852bx_bb_set_pop (common.c:1115) — clears POP-EN only in
//   monitor mode. We're always STATION → NO-OP.
```

## L223-228 · `for path in 0u8..2 {`

```
// ── 3. set_channel_rf (rtw8852b_ctrl_bw_ch) ──────────────────
//   3a. _ctrl_ch: _ch_setting for path A/B with dav=true and false
//       8852b maps RR_CFGCH and RR_CFGCH_V1 to same RF reg 0x18
//       via direct addressing (addr &= 0xff). So both pairs update
//       the same register — we only need one write per path.
//       Set CH=1, clear BAND bits (2G), set BW2, clear POW_LCK/TRX_AH/BCN.
```

## L233 · `v |= ch as u32;  // CH = ch (2G, no BAND bits)`

```
// CH = ch (2G, no BAND bits)
```

## L236 · `rf_write(mmio, path, RR_LCKST, RR_LCKST_BIN, 0);`

```
// Trigger LCK: toggle LCKST.BIN 0 → 1
```

## L241 · `for path in 0u8..2 {`

```
//   3b. _ctrl_bw: set BW bits = 20M (3) in CFGCH
```

## L251-252 · `for path in 0u8..2 {`

```
//   3c. _rxbb_bw(20MHz): 4 RF writes per path
//       LUTWE2.RTXBW = 1, LUTWA.M2 = 0x12, LUTWD0.LB = 0x1B, LUTWE2.RTXBW = 0
```

## L265-269 · `const R_ADC_FIFO: u32       = 0x20FC;`

```
// ═══════════════════════════════════════════════════════════════════
//  set_channel_help — 1:1 port of rtw8852b_set_channel_help
//  Wraps set_channel with ENTER (quiesce) and EXIT (re-enable) blocks.
//  Linux rtw8852b.c:627.
// ═══════════════════════════════════════════════════════════════════
```

## L271 · `const R_ADC_FIFO: u32       = 0x20FC;`

```
// Register addresses used by help_enter/exit + bb_reset_en + adc_en + tssi_cont_en.
```

## L296 · `fn tssi_cont_en(mmio: i32, en: bool) {`

```
/// _tssi_cont_en / rtw8852b_tssi_cont_en_phyidx with en=false/true.
```

## L305 · `fn adc_en(mmio: i32, en: bool) {`

```
/// rtw8852b_adc_en — en=false: ADC_FIFO_RST=0xf (reset), en=true: =0 (run).
```

## L311 · `fn bb_reset_en(mmio: i32, en: bool) {`

```
/// rtw8852b_bb_reset_en (Linux rtw8852b.c:542, 2G branch only).
```

## L317 · `host::mmio_w32_mask(mmio, CR + R_RXCCA,   B_RXCCA_DIS,   0x0);`

```
// 2G: clear RXCCA_DIS
```

## L325 · `host::sleep_ms(1); // fsleep(1) in Linux`

```
// fsleep(1) in Linux
```

## L330 · `fn mac_cfg_ppdu_status(mmio: i32, enable: bool) {`

```
/// rtw89_mac_cfg_ppdu_status — Linux mac.c:6155.
```

## L343-354 · `pub fn set_channel_help_enter(mmio: i32) -> u16 {`

```
/// 1:1 port of rtw8852b_set_channel_help(enter=true).
/// Linux order (rtw8852b.c:633):
///   1. stop_sch_tx(ALL) → saves tx_en bits
///   2. cfg_ppdu_status(false)
///   3. tssi_cont_en(false)
///   4. adc_en(false)
///   5. fsleep(40 µs)
///   6. bb_reset_en(band, false)
/// Returns the saved `tx_en` so set_channel_help_exit can restore it.
/// Without the sch_tx stop, TX slots keep firing during the channel
/// switch and the PHY sees stale energy — IQK/calibration fails and
/// mgmt frames queued during the switch may go out on the wrong freq.
```

## L360 · `host::sleep_ms(1); // fsleep(40 µs)`

```
// fsleep(40 µs)
```

## L365-367 · `pub fn set_channel_help_exit(mmio: i32, tx_en: u16) {`

```
/// 1:1 port of rtw8852b_set_channel_help(enter=false).
/// Linux order: ppdu_status ON → adc ON → tssi ON → bb_reset ON →
/// resume_sch_tx with the tx_en saved by the matching enter call.
```

## L376-389 · `const R_AX_PWR_RATE_CTRL: u32 = 0xD200;`

```
// ═══════════════════════════════════════════════════════════════════
//  apply_default_txpwr — smoke-test stand-in for rtw8852bx_set_txpwr
//
//  Linux's set_txpwr pipeline reads per-rate dBm values from efuse
//  (set_txpwr_byrate + offset + limit + limit_ru + diff). We don't
//  parse efuse, so for the Phase 7 smoke test we uniformly fill the
//  per-rate table with a safe 20 dBm (= 0x50 in 0.25-dBm units) so
//  HW has *something* to transmit at. Without this the table reads
//  0 and the PA stays at minimum output — AP never sees our frame.
//
//  R_AX_PWR_BY_RATE_TABLE0..10 = 0xD2C0..0xD2E8, 11 dwords.
//  Each byte = one rate's power setting in 0.25-dBm units.
//  0x50 = 80 → 20 dBm = 100 mW (2.4G legal maximum most regions).
// ═══════════════════════════════════════════════════════════════════
```

## L391-411 · `const R_AX_PWR_RATE_CTRL: u32 = 0xD200;`

```
// ═══════════════════════════════════════════════════════════════════
//  apply_txpwr_ctrl — port of __rtw8852bx_set_txpwr_ctrl (common.c:1381)
//
//  Linux phy_dm_init calls chip->ops->set_txpwr_ctrl, which is
//  __rtw8852bx_set_txpwr_ctrl → rtw8852bx_set_txpwr_ref(phy_idx=0,
//  pwr_ofst=0). This sets the *PA reference level* for both OFDM and
//  CCK, on both RF paths. Without it the per-rate txpwr table has no
//  reference to anchor against — TX goes out at undefined power.
//
//  For pwr_ofst=0:
//    ofst_dec[A] = 0, ofst_dec[B] = 0
//    val_ofdm = val_cck = bb_cal_txpwr_ref(ref=0, dec=0)
//
//  bb_cal_txpwr_ref(ref=0, dec=0):
//    pwr_s10_3   = (0<<1) + (0x27<<3) - 0 = 0x138 (312)
//    bb_pwr_cw   = 0x138 & 0x7 = 0
//    rf_pwr_cw   = (0x138>>3) & 0x3F = 0x27 = 39 (clamp 15..63 → 39)
//    pwr_cw      = (39<<3) | 0 = 0x138
//    tssi_ofst_cw = 0x12c + 0 - 128 = 0xAC (172)
//    val = (0xAC<<18) | (0x138<<9) | 0 = 0x02B27000
// ═══════════════════════════════════════════════════════════════════
```

## L414 · `const B_AX_PWR_REF: u32       = 0x0FFF_FC00; // GENMASK(27,10)`

```
// GENMASK(27,10)
```

## L416-417 · `const R_DPD_A: u32 = 0x5800;`

```
// Path A DPD reg = 0x5800 + ofst; Path B = 0x7800 + ofst.
// ofst_ofdm = 0x4, ofst_cck = 0x8.
```

## L420 · `const DPD_MASK: u32 = (0x1FF << 18) | (0x1FF << 9) | 0x1FF; // = 0x07FFFFFF`

```
// = 0x07FFFFFF
```

## L422-423 · `const DPD_VAL_REF0: u32 = 0x02B27000;`

```
/// Pre-computed bb_cal_txpwr_ref(ref=0, dec=0) = 0x02B27000.
/// Same value for OFDM and CCK when pwr_ofst=0.
```

## L426-445 · `pub fn bb_cfg_txrx_path(mmio: i32) {`

```
// ═══════════════════════════════════════════════════════════════════
//  bb_cfg_txrx_path — 1:1 port of __rtw8852bx_bb_cfg_txrx_path
//  (rtw8852b_common.c:1743). THE missing TX-routing setup.
//
//  Linux calls this once from rtw89_phy_dm_init. Without it,
//  R_P0_RFMODE / R_P1_RFMODE (0x12AC / 0x32AC) bits [31:4] stay at
//  reset default — TX routing pattern undefined, chip doesn't know
//  which RF path to send TX through, frame dies silently.
//
//  For our 2G RF_AB case (both paths TX + both paths RX):
//    R_P0_RFMODE[31:4]       = 0x1233312  (TX routing pattern)
//    R_P0_RFMODE_FTM_RX[11:0]= 0x333
//    R_P1_RFMODE[31:4]       = 0x1233312
//    R_P1_RFMODE_FTM_RX[11:0]= 0x333
//    R_CHBW_MOD_V1.ANT_RX_SEG0       = 3 (both paths receive)
//    R_FC0_BW_V1.ANT_RX_1RCCA_SEG0/1 = 3 (both CCA segments)
//    R_RXHT_MCS_LIMIT / R_RXVHT_MCS_LIMIT / R_RXHE flags for nss=2
//    R_MAC_SEL.B_MAC_SEL_MOD = 0
//    R_P0/1_TXPW_RSTB MANON+TSSI toggle 1→3 (release TX-power reset)
// ═══════════════════════════════════════════════════════════════════
```

## L452 · `const B_TXRX_FTM_TX:       u32 = 0xFFFF_FFF0; // GENMASK(31, 4)`

```
// GENMASK(31, 4)
```

## L453 · `const B_FTM_RX:            u32 = 0x0000_0FFF; // GENMASK(11, 0)`

```
// GENMASK(11, 0)
```

## L456 · `const B_ANT_RX_SEG0:       u32 = 0x0000_000F; // GENMASK(3, 0)`

```
// GENMASK(3, 0)
```

## L458 · `const B_ANT_RX_1RCCA_SEG0: u32 = 0x0003_C000; // GENMASK(17,14)`

```
// GENMASK(17,14)
```

## L459 · `const B_ANT_RX_1RCCA_SEG1: u32 = 0x003C_0000; // GENMASK(21,18)`

```
// GENMASK(21,18)
```

## L462 · `const B_RXHT_MCS_LIMIT:    u32 = 0x3 << 8;    // GENMASK(9,8)`

```
// GENMASK(9,8)
```

## L464 · `const B_RXVHT_MCS_LIMIT:   u32 = 0x3 << 21;   // GENMASK(22,21)`

```
// GENMASK(22,21)
```

## L466 · `const B_RXHE_USER_MAX:     u32 = 0xFF << 6;   // GENMASK(13,6)`

```
// GENMASK(13,6)
```

## L467 · `const B_RXHE_MAX_NSS:      u32 = 0x7 << 14;   // GENMASK(16,14)`

```
// GENMASK(16,14)
```

## L468 · `const B_RXHETB_MAX_NSS:    u32 = 0x7 << 23;   // GENMASK(25,23)`

```
// GENMASK(25,23)
```

## L472 · `const B_TXPW_RSTB:         u32 = 0x3 << 30;   // MANON(30) | TSSI(31)`

```
// MANON(30) | TSSI(31)
```

## L477 · `host::mmio_w32_mask(mmio, CR + R_CHBW_MOD_V1,       B_ANT_RX_SEG0,       3);`

```
// Linux __rtw8852bx_bb_ctrl_rx_path(RF_AB, chan) — RX path side:
```

## L487 · `host::mmio_w32_mask(mmio, CR + R_P0_TXPW_RSTB, B_TXPW_RSTB, 1);`

```
// TXPW_RSTB release — toggle 1 then 3 on both paths
```

## L493-494 · `host::mmio_w32_mask(mmio, CR + R_P0_RFMODE,        B_TXRX_FTM_TX, 0x1233312);`

```
// Linux __rtw8852bx_bb_ctrl_rf_mode_rx_path(RF_AB) — THE critical
// TX routing pattern. 0x1233312 encodes per-band-nibble routing.
```

## L500 · `host::mmio_w32_mask(mmio, CR + R_MAC_SEL, B_MAC_SEL_MOD, 0);`

```
// MAC_SEL.MOD = 0 (last write in Linux cfg_txrx_path)
```

## L507 · `host::mmio_clr32(mmio, R_AX_PWR_RATE_CTRL, B_AX_PWR_REF);`

```
// 1. Clear PWR_REF in R_AX_PWR_RATE_CTRL (leave FORCE_EN + FORCE_VALUE bits alone).
```

## L510-512 · `host::mmio_w32_mask(mmio, CR + R_DPD_A + 0x4, DPD_MASK, DPD_VAL_REF0); // A OFDM`

```
// 2. Write OFDM + CCK ref values, both paths.
//    Path A OFDM at PHY 0x5804, CCK at PHY 0x5808
//    Path B OFDM at PHY 0x7804, CCK at PHY 0x7808
```

## L513 · `host::mmio_w32_mask(mmio, CR + R_DPD_A + 0x4, DPD_MASK, DPD_VAL_REF0); // A OFDM`

```
// A OFDM
```

## L514 · `host::mmio_w32_mask(mmio, CR + R_DPD_A + 0x8, DPD_MASK, DPD_VAL_REF0); // A CCK`

```
// A CCK
```

## L515 · `host::mmio_w32_mask(mmio, CR + R_DPD_B + 0x4, DPD_MASK, DPD_VAL_REF0); // B OFDM`

```
// B OFDM
```

## L516 · `host::mmio_w32_mask(mmio, CR + R_DPD_B + 0x8, DPD_MASK, DPD_VAL_REF0); // B CCK`

```
// B CCK
```

## L522-528 · `pub fn set_txpwr(mmio: i32, _ch: u8) {`

```
// R_AX_PWR_RATE_CTRL (0xD200):
//   bits 27..10 = B_AX_PWR_REF (signed s18, 0.25 dBm units)
//   bit  9      = B_AX_FORCE_PWR_BY_RATE_EN
//   bits 8..0   = B_AX_FORCE_PWR_BY_RATE_VALUE_MASK
// When FORCE_PWR_BY_RATE_EN=1, HW ignores the per-rate table and
// transmits every frame at VALUE. Useful smoke-test override until
// we port the full rtw8852bx_set_txpwr_ref/offset/limit pipeline.
```

## L530-554 · `pub fn set_txpwr(mmio: i32, _ch: u8) {`

```
// ═══════════════════════════════════════════════════════════════════
//  set_txpwr — full Linux pipeline, 2 G only, FCC approximation.
//
//  Linux __rtw8852bx_set_txpwr (common.c:1369) does six sub-steps:
//    1. rtw89_phy_set_txpwr_byrate_ax  (phy.c:3055)
//    2. rtw89_phy_set_txpwr_offset_ax  (phy.c:3112)
//    3. rtw8852bx_set_tx_shape         (common.c:1320)
//    4. rtw89_phy_set_txpwr_limit_ax   (phy.c:3140)
//    5. rtw89_phy_set_txpwr_limit_ru_ax(phy.c:3175)
//    6. rtw8852bx_set_txpwr_diff       (common.c:1358) → set_txpwr_ref
//
//  This is the 1:1 port. The only simplification is the regulatory
//  domain: we use permissive FCC-2G values (0x50 = 20 dBm) rather than
//  parsing Linux's per-country-per-channel regulatory arrays
//  (>10000 lines of table data). For our AUTH on ch 7 that's fine —
//  FCC allows 30 dBm on 2.4 G and our PA won't do more than 20.
//
//  The important difference from the old apply_default_txpwr:
//    - NO FORCE_PWR_BY_RATE — FORCE is a debug override, never set by
//      Linux in production. Leaving it on might lock the PA into a
//      rate-blind mode that mis-configures the RF path.
//    - Real per-rate byrate values (Linux rtw89_8852b_txpwr_byrate
//      table row for 2 G: high MCS get lower dBm).
//    - tx_shape CCK + OFDM triangular = 0 (FCC default).
// ═══════════════════════════════════════════════════════════════════
```

## L562-564 · `apply_txpwr_ctrl(mmio);`

```
// set_txpwr_diff / set_txpwr_ref is already applied once as
// apply_txpwr_ctrl in lib.rs after MAC init, with pwr_ofst=0.
// Re-apply here so each channel switch programs the reference.
```

## L569-578 · `fn set_txpwr_byrate(mmio: i32) {`

```
/// Step 1: byrate table — 1:1 values from Linux rtw89_8852b_txpwr_byrate
/// (rtw8852b_table.c:14574..14599), 2 G band rows only.
///
/// Each dword packs four s8 per-rate values (0.25 dBm units).
/// Layout of R_AX_PWR_BY_RATE_TABLE0..10 (11 dwords, 0xD2C0..0xD2E8):
///   nss 0: CCK[0..3] | OFDM[0..3] | OFDM[4..7]
///          MCS [0..3] | MCS [4..7] | MCS [8..11]
///          HEDCM[0..3]
///   nss 1: MCS [0..3] | MCS [4..7] | MCS [8..11]
///          HEDCM[0..3]
```

## L580 · `const VALS: [u32; 11] = [`

```
// values taken directly from Linux 8852b table for band=0 (2 G)
```

## L582 · `0x50505050,  // nss 0 CCK        0..3`

```
// nss 0 CCK        0..3
```

## L583 · `0x50505050,  // nss 0 OFDM       0..3  (6/9/12/18 Mbps)`

```
// nss 0 OFDM       0..3  (6/9/12/18 Mbps)
```

## L584 · `0x484C5050,  // nss 0 OFDM       4..7  (24/36/48/54 Mbps)`

```
// nss 0 OFDM       4..7  (24/36/48/54 Mbps)
```

## L585 · `0x50505050,  // nss 0 MCS        0..3`

```
// nss 0 MCS        0..3
```

## L586 · `0x44484C50,  // nss 0 MCS        4..7`

```
// nss 0 MCS        4..7
```

## L587 · `0x34383C40,  // nss 0 MCS        8..11`

```
// nss 0 MCS        8..11
```

## L588 · `0x50505050,  // nss 0 HEDCM      0..3`

```
// nss 0 HEDCM      0..3
```

## L589 · `0x50505050,  // nss 1 MCS        0..3`

```
// nss 1 MCS        0..3
```

## L590 · `0x44484C50,  // nss 1 MCS        4..7`

```
// nss 1 MCS        4..7
```

## L591 · `0x34383C40,  // nss 1 MCS        8..11`

```
// nss 1 MCS        8..11
```

## L592 · `0x50505050,  // nss 1 HEDCM      0..3`

```
// nss 1 HEDCM      0..3
```

## L600-603 · `host::mmio_w32(mmio, R_AX_PWR_RATE_CTRL, 0);`

```
// IMPORTANT: clear FORCE_PWR_BY_RATE_EN. Linux never sets this.
// A 1 here locks the PA into a single-rate test mode and may mis-
// route RF paths for normal TX. Also clear the whole REF field —
// we write it separately via apply_txpwr_ctrl.
```

## L607 · `fn set_txpwr_offset(mmio: i32) {`

```
/// Step 2: txpwr offset — zero for FCC default.
```

## L609-610 · `host::mmio_w32_mask(mmio, 0xD204 /* R_AX_PWR_RATE_OFST_CTRL */,`

```
// Linux rtw89_phy_set_txpwr_offset_ax: 5 × 4-bit per-rate offsets,
// bits[19:0]. FCC-2G default = all 0.
```

## L611 · `host::mmio_w32_mask(mmio, 0xD204 /* R_AX_PWR_RATE_OFST_CTRL */,`

```
/* R_AX_PWR_RATE_OFST_CTRL */
```

## L615 · `fn set_tx_shape(mmio: i32) {`

```
/// Step 3: TX shape — CCK DFIR + OFDM triangular both 0 for FCC 2 G.
```

## L617-624 · `const R_DCFO_OPT: u32 = 0x1_4494;`

```
// Linux rtw8852bx_set_tx_shape: for 2 G we'd call
// rtw8852bx_bb_set_tx_shape_dfir(tx_shape_cck=0) which writes the
// CCK DFIR coefficient bank — that is a ~40-entry table lookup
// on chan; for FCC cck=0 and ofdm=0 are defaults.
//
// The OFDM "triangular" shaping is a single BB register write:
//   R_DCFO_OPT = 0x4494 + PHY_CR_BASE (0x10000)
//   B_TXSHAPE_TRIANGULAR_CFG = GENMASK(25,24)
```

## L630-633 · `fn set_txpwr_limit(mmio: i32) {`

```
/// Step 4: txpwr limit — 20 dwords for 2 paths.
/// Per-band/regd/ch/bw in Linux, but for FCC 2 G 20 MHz the limit is
/// 30 dBm ≈ 0x78. We use 0x50 (20 dBm) which is always below the FCC
/// ceiling and above our PA's actual output, so it doesn't clamp.
```

## L641 · `fn set_txpwr_limit_ru(mmio: i32) {`

```
/// Step 5: txpwr RU limit — 12 dwords for 2 paths, OFDMA RU limits.
```

## L649 · `pub fn apply_default_txpwr(mmio: i32) { set_txpwr(mmio, 0); }`

```
// Legacy alias so callers of apply_default_txpwr keep working.
```

