# `tools/wasm/wifi/src/phy.rs` @ 5e0102684

## L1-22 · `use crate::host;`

```
//! PHY initialization — strict 1:1 port of Linux rtw89_phy_init_bb_reg +
//! rtw89_phy_init_rf_reg + rtw89_phy_init_rf_nctl.
//!
//! Structure:
//!   1. BB table (MMIO writes via config_bb_reg)
//!   2. bb_reset
//!   3. BB gain table — NOT MMIO. Linux parses each entry's "addr" as a
//!      struct (type/path/gain_band/cfg_type) and stores values in RAM
//!      for later per-channel RSSI calibration. Writing them as MMIO
//!      clobbers SYS_ISO_CTRL / SYS_PW_CTRL (addrs 0x000..0x002 overlap
//!      PCIe power regs) and kills the chip. We skip the table for now.
//!   4. RF table per path (path A: base 0xE000; path B: base 0xF000)
//!      Each entry routed via rtw89_phy_write_rf_v1:
//!         - bit 16 of addr set (ad_sel=1) → direct MMIO at base+((addr&0xff)<<2)
//!         - else                         → SWSI at R_SWSI_DATA_V1 (0x0370)
//!   5. preinit_rf_nctl_ax (mandatory before NCTL, polls 0x8080 == 0x4)
//!   6. NCTL table (MMIO writes, addrs 0x8000+)
//!
//! Conditional state machine (PHY_COND_BRANCH_IF/ELIF/ELSE/END/CHECK) is
//! implemented per Linux rtw89_phy_init_reg. Our flat .bin tables trigger
//! the parser with headline_size=0 (no headers), so only the unconditional
//! pre-IF entries exist — functional for basic init.
```

## L41-46 · `pub const PHY_CR_BASE: u32 = 0x10000;`

```
// Linux rtw89_phy_gen_ax.cr_base — ALL PHY/BB/RF/NCTL register addresses
// in the Linux phy table constants and in rtw89_phy_write32() helpers are
// relative to this base. MAC registers (R_AX_* with cr_base=0) stay at
// their raw offset, but everything going through rtw89_phy_write32/_mask
// lands at addr + CR_BASE. Missing this offset was writing the BB table
// into SYS/PCIe/MAC registers, killing the chip.
```

## L49 · `const R_SWSI_DATA_V1: u32 = 0x0370;`

```
// SWSI RF indirect write (Linux reg.h) — PHY-space addresses, add CR_BASE
```

## L51 · `const R_SWSI_V1:        u32 = 0x174C;`

```
// SWSI busy status register (Linux rtw89_phy_check_swsi_busy)
```

## L56 · `const RF_BASE_ADDR_A: u32 = 0xE000;`

```
// 8852B RF base addresses (Linux rtw8852b.c: .rf_base_addr = {0xe000, 0xf000})
```

## L62 · `const R_IOQ_IQK_DPK:   u32 = 0x0C60;`

```
// preinit_rf_nctl_ax register addresses (Linux reg.h)
```

## L70-73 · `const RFE: u8 = 1;`

```
// 8852BE: rfe/cv values. cv=2 confirmed from SYS_CFG1 register dump.
// v0.63.1 with full Linux tables showed rfe=0 fails all 4 sel_headline
// cases (RF_A headlines have rfe ∈ {1..8, 0x29, 0x2B}). 1 is most common
// for 8852BE consumer cards. If still aborts, try 2.
```

## L83-85 · `pub fn init(mmio: i32) {`

```
// ═══════════════════════════════════════════════════════════════════
//  Entry point — mirrors Linux chip->ops->bb_cfg + rfk_init sequence
// ═══════════════════════════════════════════════════════════════════
```

## L94-97 · `bb_reset(mmio);`

```
// bb_reset IMMEDIATELY after last BB write, before any serial prints.
// Linux phy_init_bb_reg runs BB table → init_txpwr_unit → bb_gain →
// bb_reset, all in tight sequence. Our serial prints take 10-15ms
// and the BB subsystem destabilises during that gap.
```

## L126-131 · `const R_TX_COLLISION_T2R_ST: u32 = 0x0C70;`

```
// edcca_init — Linux rtw89_phy_edcca_init for 8852B is 1 register write.
// Without proper EDCCA threshold, the MAC's CCA engine thinks the channel
// is always busy → RX is blocked for real frames.
//   R_TX_COLLISION_T2R_ST = 0x0C70
//   B_TX_COLLISION_T2R_ST_M = GENMASK(25, 20)
//   value = 0x29
```

## L140-141 · `rfk_init(mmio);`

```
// RFK baseline — Linux rtw8852b_rfk_init (rtw8852b.c:649)
// Part 1: dpk_init + rck (inline in phy.rs)
```

## L144 · `crate::rfk::init(mmio);`

```
// Part 2: dack + rx_dck (rfk.rs module)
```

## L153-156 · `const VERBOSE: bool = false;`

```
/// Gated by `VERBOSE` — dumps CFG1 at every PHY-init milestone. Was
/// critical while tracking down "BB table kills PCIe" regressions in
/// v0.80-era; now that init passes cleanly these 15+ lines are pure
/// noise. Keep code to re-enable when a future chip change breaks CFG1.
```

## L167-169 · `fn preinit_rf_nctl_ax(mmio: i32) {`

```
// ═══════════════════════════════════════════════════════════════════
//  preinit_rf_nctl_ax — 1:1 port of Linux rtw89_phy_preinit_rf_nctl_ax
// ═══════════════════════════════════════════════════════════════════
```

## L173 · `host::mmio_set32(mmio, PHY_CR_BASE + R_IOQ_IQK_DPK,   0x3);`

```
// All addresses here are PHY-space (Linux uses rtw89_phy_write32_*) → +CR_BASE
```

## L195-197 · `#[derive(Copy, Clone)]`

```
// ═══════════════════════════════════════════════════════════════════
//  rtw89_phy_init_reg state machine (1:1 Linux)
// ═══════════════════════════════════════════════════════════════════
```

## L236 · `None => return stats, // Linux behavior: "invalid PHY package" → skip table`

```
// Linux behavior: "invalid PHY package" → skip table
```

## L241 · `let hdr_off = 4 + headline_idx * 8;`

```
// cfg_target from the chosen headline entry (or entry 0 if no headlines).
```

## L296 · `let _ = last_addr; let _ = last_data; let _ = last_written_at; // used below`

```
// used below
```

## L297 · `let check_freq = 16u32;`

```
// Periodic liveness check — tight window for killer.
```

## L319-321 · `if stats.written > 0 {`

```
// Final kill check — catches killers in the last <16 writes that never
// hit a periodic checkpoint. Reports the last written entry as the
// prime suspect (it's within 0..check_freq writes of the actual death).
```

## L357 · `let compare = cfg_compare(rfe, cv);`

```
// case 1: RFE match, CV match
```

## L363 · `let compare = cfg_compare(rfe, PHY_COND_DONT_CARE);`

```
// case 2: RFE match, CV don't care
```

## L369 · `let mut cv_max: u8 = 0;`

```
// case 3: RFE match, CV max
```

## L381 · `let mut cv_max: u8 = 0;`

```
// case 4: RFE don't care, CV max
```

## L396-398 · `fn write_entry(mmio: i32, kind: WriteKind, addr: u32, data: u32) {`

```
// ═══════════════════════════════════════════════════════════════════
//  write_entry — dispatches delay vs MMIO/SWSI
// ═══════════════════════════════════════════════════════════════════
```

## L401-402 · `match addr {`

```
// Delay encodings — same for BB and RF paths (rtw89_phy_config_bb_reg
// and rtw89_phy_config_rf_reg check these explicitly).
```

## L413 · `host::mmio_w32(mmio, PHY_CR_BASE + addr, data);`

```
// Linux rtw89_phy_config_bb_reg → rtw89_phy_write32(addr+cr_base, data)
```

## L417-421 · `if addr & RTW89_RF_ADDR_ADSEL_MASK != 0 {`

```
// Linux rtw89_phy_write_rf_v1: ad_sel = addr & BIT(16)
//   ad_sel=1: rtw89_phy_write_rf → rtw89_phy_write32_mask(direct, mask, data)
//             direct = base_addr[path] + ((addr&0xff)<<2), MASKED by RFREG_MASK.
//             All via rtw89_phy_write32_mask → +cr_base.
//   ad_sel=0: rtw89_phy_write_rf_a → SWSI (also via rtw89_phy_write32_mask → +cr_base)
```

## L433-434 · `cfg_bb_gain(addr, data);`

```
// No MMIO. addr is a packed arg (type/path/gband/cfg_type);
// dispatch to cfg_bb_gain which stores into BB_GAIN software state.
```

## L440-443 · `const R_SWSI_READ_ADDR_V1: u32 = 0x0378;`

```
// ═══════════════════════════════════════════════════════════════════
//  RF register read/write helpers — used by RFK functions
//  Linux rtw89_phy_{read,write}_rf_v1 — route via ad_sel bit.
// ═══════════════════════════════════════════════════════════════════
```

## L446 · `const B_SWSI_R_DATA_DONE_V1: u32 = 1 << 26;`

```
// B_SWSI_READ_ADDR_ADDR_V1 = GENMASK(7,0), B_SWSI_READ_ADDR_PATH_V1 = GENMASK(10,8)
```

## L450-452 · `pub fn rf_read(mmio: i32, path: u8, addr: u32) -> u32 {`

```
/// Read an RF register. Returns the full 20-bit value (unmasked).
/// Linux rtw89_phy_read_rf_v1 — dispatches ad_sel=1 to direct MMIO,
/// ad_sel=0 to SWSI read via R_SWSI_READ_ADDR_V1.
```

## L455 · `let base = if path == 0 { RF_BASE_ADDR_A } else { RF_BASE_ADDR_B };`

```
// Direct MMIO read (Linux rtw89_phy_read_rf): base + (addr & 0xff) << 2
```

## L460-461 · `for _ in 0..30u32 {`

```
// SWSI read (Linux rtw89_phy_read_rf_a)
// Wait busy clear
```

## L467 · `let req = ((path as u32 & 0x7) << 8) | (addr & 0xFF);`

```
// Trigger read: path[10:8] | addr[7:0]
```

## L470 · `for _ in 0..200 { core::hint::spin_loop(); } // ~2us`

```
// ~2us
```

## L471 · `for _ in 0..30u32 {`

```
// Poll B_SWSI_R_DATA_DONE
```

## L479 · `0xFFFFFFFF // timeout marker`

```
// timeout marker
```

## L482 · `pub fn rf_write_mask(mmio: i32, path: u8, addr: u32, mask: u32, data: u32) {`

```
/// Write an RF register with a bit mask. Read-modify-write internally.
```

## L487 · `rf_write_full(mmio, path, addr, data & RFREG_MASK);`

```
// Full register write
```

## L496 · `pub fn rf_write_full(mmio: i32, path: u8, addr: u32, data: u32) {`

```
/// Write full RF register (unmasked) — RFREG_MASK = 0xFFFFF.
```

## L508-513 · `const RR_MOD: u32      = 0x00;`

```
// ═══════════════════════════════════════════════════════════════════
//  RFK: baseline — 1:1 Linux rtw8852b_rfk_init
//  rtw8852b.c:649 → dpk_init + rck + dack + rx_dck
//  We implement dpk_init + rck for now (minimal baseline);
//  dack/rx_dck are much larger and can be added if still needed.
// ═══════════════════════════════════════════════════════════════════
```

## L515 · `const RR_MOD: u32      = 0x00;`

```
// RF register addresses (Linux reg.h)
```

## L517 · `const RR_MOD_MASK: u32 = 0xF0000;     // GENMASK(19,16)`

```
// GENMASK(19,16)
```

## L520 · `const RR_RSV1_RST: u32 = 0x1;         // BIT(0)`

```
// BIT(0)
```

## L522 · `const RR_RCKC_CA: u32  = 0x7C00;      // GENMASK(14,10)`

```
// GENMASK(14,10)
```

## L525 · `const R_DPD_BF: u32      = 0x4CF8;`

```
// DPK backoff registers (PHY space, CR_BASE applies)
```

## L527 · `const B_DPD_BF_OFDM: u32 = 0x1F00;    // GENMASK(12,8)`

```
// GENMASK(12,8)
```

## L528 · `const B_DPD_BF_SCA: u32  = 0x3E;      // GENMASK(5,1) — from Linux bb def`

```
// GENMASK(5,1) — from Linux bb def
```

## L529 · `const R_DPD_CH0A: u32    = 0x5800;    // Per-path base (path 0 = 0x5800, path 1 = 0x5900)`

```
// Per-path base (path 0 = 0x5800, path 1 = 0x5900)
```

## L530 · `const B_DPD_CFG: u32     = 0x00FF_FFFF; // bottom 24 bits`

```
// bottom 24 bits
```

## L542 · `fn set_dpd_backoff(mmio: i32) {`

```
/// 1:1 Linux _set_dpd_backoff for phy=0 (rtw8852b_rfk.c:2692).
```

## L544-545 · `let v = host::mmio_r32(mmio, PHY_CR_BASE + R_DPD_BF);`

```
// phy_read32_mask(R_DPD_BF + (phy << 13), B_DPD_BF_OFDM/SCA)
// For phy=0, offset=0.
```

## L550 · `for path in 0..2u32 {`

```
// Linux: for each path, write B_DPD_CFG=0x7f7f7f at R_DPD_CH0A + (path << 8)
```

## L559 · `fn rck(mmio: i32, path: u8) {`

```
/// 1:1 Linux _rck (rtw8852b_rfk.c:336).
```

## L565 · `rf_write_full(mmio, path, RR_RCKC, 0x00240);`

```
// RCK trigger
```

## L568 · `for _ in 0..30u32 {`

```
// Poll RR_RCKS bit 3 (~30us timeout, Linux: 2us sleep × 30)
```

## L581-583 · `fn write_rf_swsi(mmio: i32, path: u8, addr: u32, data: u32) {`

```
/// SWSI RF write (Linux rtw89_phy_write_rf_a, mask == RFREG_MASK path).
/// R_SWSI_DATA_V1 layout: [31]=bit_mask_en | [30:28]=path | [27:20]=addr | [19:0]=data
/// All SWSI registers are PHY-space → need +PHY_CR_BASE offset.
```

## L596-599 · `fn bb_reset(mmio: i32) {`

```
/// 1:1 Linux rtw8852b_bb_reset (rtw8852b.c:566) + rtw8852bx_bb_reset_all
/// (rtw8852b_common.c:1077). Called at the end of phy_init_bb_reg to
/// commit the BB state. Without this the BB subsystem drifts and kills
/// the PCIe interface some milliseconds after the last BB write.
```

## L601 · `const R_P0_TXPW_RSTB: u32 = 0x58DC;`

```
// Addresses from Linux reg.h
```

## L611 · `const B_HW_SI_DIS_W_R_TRIG_MASK: u32 = 0x7 << 28; // GENMASK(30,28)`

```
// GENMASK(30,28)
```

## L616 · `host::mmio_set32(mmio, PHY_CR_BASE + R_P0_TXPW_RSTB, B_TXPW_RSTB_MANON);`

```
// All addresses are PHY-space (Linux uses rtw89_phy_write32_*) → +CR_BASE
```

## L618 · `host::mmio_set32(mmio, PHY_CR_BASE + R_P0_TXPW_RSTB, B_TXPW_RSTB_MANON);`

```
// Pre-gate
```

## L624 · `let s0 = host::mmio_r32(mmio, PHY_CR_BASE + R_S0_HW_SI_DIS);`

```
// rtw8852bx_bb_reset_all body
```

## L630 · `host::sleep_ms(1); // Linux fsleep(1)`

```
// Linux fsleep(1)
```

## L642 · `host::mmio_clr32(mmio, PHY_CR_BASE + R_P0_TXPW_RSTB, B_TXPW_RSTB_MANON);`

```
// Post-gate
```

## L649-665 · `const GAIN_BAND_NR: usize = 8;   // 2G + 5G L/M/H + 6G L/M/H/UH`

```
// ═══════════════════════════════════════════════════════════════════
//  BB gain parser + HW apply  — 1:1 Linux rtw89_phy_config_bb_gain_ax
//  + rtw8852bx_set_gain_error (rtw8852b_common.c:577).
//
//  BB gain entries in the bb_gain table do NOT write to MMIO. Instead
//  the table addr field is a packed `rtw89_phy_bb_gain_arg` union:
//      [ 7: 0] type   (or rxsc_start[3:0] | bw[7:4] for rpl_ofst)
//      [15: 8] path
//      [23:16] gain_band  (0=2G, 1..3=5G, 4..7=6G)
//      [31:24] cfg_type   (0=error, 1=rpl_ofst, 2=bypass, 3=op1db, 4=eFEM)
//  The data field carries 4 s8 values packed LE — unpacked by cfg_*.
//
//  Linux stores results in rtwdev->bb_gain.ax and uses them in
//  rtw8852bx_ctrl_ch → set_gain_error to write LNA/TIA gain registers
//  per channel band. Skipping this leaves LNA/TIA gain = 0 on every
//  channel switch → RX front-end has no gain → zero packets received.
// ═══════════════════════════════════════════════════════════════════
```

## L667 · `const GAIN_BAND_NR: usize = 8;   // 2G + 5G L/M/H + 6G L/M/H/UH`

```
// 2G + 5G L/M/H + 6G L/M/H/UH
```

## L668 · `const PATH_NR: usize      = 2;   // 8852B uses RF_PATH_A + RF_PATH_B`

```
// 8852B uses RF_PATH_A + RF_PATH_B
```

## L671 · `const TIA_LNA_OP1DB_NUM: usize = 8;  // LNA_GAIN_NUM + 1 per Linux core.h`

```
// LNA_GAIN_NUM + 1 per Linux core.h
```

## L700 · `const RXSC_IDX_FULL:    u8 = 0;`

```
// Linux enum rtw89_phy_bb_rxsc_start_idx
```

## L707 · `const BW_20:  u8 = 0;`

```
// Linux enum rtw89_channel_width
```

## L714 · `let addr_lo = addr & 0xFF;`

```
// Reject flow-ctrl/delay addrs — Linux: "bb gain table with flow ctrl".
```

## L718 · `let typ       = (addr & 0xFF) as u8;                 // also rxsc_start[3:0]|bw[7:4]`

```
// also rxsc_start[3:0]|bw[7:4]
```

## L731 · `_ => { /* 4 = eFEM (needs efuse rfe_type>=50), ignore */ }`

```
/* 4 = eFEM (needs efuse rfe_type>=50), ignore */
```

## L772 · `let rxsc_start = typ & 0xF;`

```
// typ = rxsc_start[3:0] | bw[7:4]
```

## L841-843 · `const BB_GAIN_LNA_G: [(u32, u32, u32); LNA_NUM] = [`

```
// 8852B bb_gain_lna/tia register tables — 1:1 Linux
// rtw8852b_common.c:553 (bb_gain_lna, 2G only: .gain_g).
// PHY-space, + PHY_CR_BASE when writing.
```

## L859-862 · `pub fn apply_gain_error_2g(mmio: i32, path: u8) {`

```
/// 1:1 Linux rtw8852bx_set_gain_error for subband == RTW89_CH_2G.
/// Writes LNA + TIA gain values stored in BB_GAIN (populated by
/// cfg_bb_gain during BB gain table load) into per-path HW registers.
/// Must be called on each 2G channel change (Linux: inside rtw8852bx_ctrl_ch).
```

## L865 · `let gband = 0usize; // RTW89_BB_GAIN_BAND_2G`

```
// RTW89_BB_GAIN_BAND_2G
```

