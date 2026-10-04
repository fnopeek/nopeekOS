# `tools/wasm/wifi/src/bb.rs` @ 5e0102684

## L1-15 · `use crate::host;`

```
//! BB (BaseBand) helpers used by TSSI alimentk.
//!
//! Port of the rtw8852bx BB helper functions from
//! drivers/net/wireless/realtek/rtw89/rtw8852b_common.c:
//!
//!   __rtw8852bx_bb_set_plcp_tx       — load PMAC HT20 MCS7 table
//!   __rtw8852bx_bb_cfg_tx_path       — route TX chain to path A/B/AB
//!   __rtw8852bx_bb_ctrl_rx_path      — route RX chain (minimal variant)
//!   __rtw8852bx_bb_set_power         — set test-TX power in R_TXPWR
//!   __rtw8852bx_bb_set_pmac_pkt_tx   — start/stop PMAC test-TX
//!   __rtw8852bx_bb_backup_tssi       — snapshot 7 BB registers
//!   __rtw8852bx_bb_restore_tssi      — restore them
//!
//! These are only used by the TSSI alimentk cal loop; normal traffic TX
//! uses the CH8 ring path and doesn't touch these registers.
```

## L21 · `pub const R_RSTB_ASYNC:       u32 = 0x0704;`

```
// ── Register addresses (reg.h, PHY space, +PHY_CR_BASE at access) ─
```

## L28 · `pub const B_PMAC_GNT_RXEN:    u32 = 1 << 16; // reg.h: BIT(16) — was wrongly 1<<4`

```
// reg.h: BIT(16) — was wrongly 1<<4
```

## L34 · `pub const B_MAC_SEL_DPD_EN:   u32 = 1 << 10; // reg.h:8841 BIT(10)`

```
// Extra field in R_MAC_SEL (0x09A4) used by tx_mode_switch.
```

## L35 · `pub const B_MAC_SEL_DPD_EN:   u32 = 1 << 10; // reg.h:8841 BIT(10)`

```
// reg.h:8841 BIT(10)
```

## L78 · `pub const B_MAC_SEL_PWR_EN:   u32 = 1 << 16; // reg.h:8840 BIT(16) — was wrongly 1<<7`

```
// reg.h:8840 BIT(16) — was wrongly 1<<7
```

## L79 · `pub const B_MAC_SEL_MOD:      u32 = 0x0000_001C; // reg.h:8842 GENMASK(4,2) — was wrongly 0xE0`

```
// reg.h:8842 GENMASK(4,2) — was wrongly 0xE0
```

## L90 · `pub const RF_PATH_A:  u8 = 0;`

```
// RF path selector (passed to bb_cfg_tx_path / ctrl_rx_path).
```

## L95 · `fn pw(mmio: i32, addr: u32, val: u32) {`

```
// ── Helpers ─────────────────────────────────────────────────────
```

## L113 · `pub fn set_plcp_tx(mmio: i32) {`

```
// ── Public API ──────────────────────────────────────────────────
```

## L115-117 · `pub fn set_plcp_tx(mmio: i32) {`

```
/// __rtw8852bx_bb_set_plcp_tx (common.c:1433).
/// Applies 120-entry HT20 MCS7 preset table — sets up the PMAC PLCP
/// generator for the test TX sweeps done by TSSI alimentk.
```

## L124-125 · `pub fn cfg_tx_path(mmio: i32, path: u8) {`

```
/// __rtw8852bx_bb_cfg_tx_path (common.c:1531). MAC_SEL_MOD=7 enables
/// PMAC TX. Path=A/B/AB sets the TX routing.
```

## L145-147 · `pub fn ctrl_rx_path(mmio: i32, rx_path: u8) {`

```
/// __rtw8852bx_bb_ctrl_rx_path (common.c:1655). Minimal variant —
/// sets ANT_RX_SEG0 + 1RCCA_SEG0/1 + MCS_LIMIT + RXHE fields per
/// rx_path (RF_A=1, RF_B=2, RF_AB=3).
```

## L156 · `pwm(mmio, 0x49C0, 0x0000_0300, seg);  // R_FC0_BW_V1.B_ANT_RX_1RCCA_SEG0`

```
// R_FC0_BW_V1.B_ANT_RX_1RCCA_SEG0
```

## L157 · `pwm(mmio, 0x49C0, 0x0000_0C00, seg);  // B_ANT_RX_1RCCA_SEG1`

```
// B_ANT_RX_1RCCA_SEG1
```

## L165-166 · `pub fn set_power(mmio: i32, pwr_dbm: i16) {`

```
/// __rtw8852bx_bb_set_power (common.c:1521). Writes pwr_dbm into
/// R_TXPWR. Used by alimentk to sweep power levels.
```

## L172-173 · `pub fn set_pmac_pkt_tx(mmio: i32, enable: bool, cnt: u16, period: u16) {`

```
/// __rtw8852bx_bb_set_pmac_pkt_tx (common.c:1504). Enable/disable the
/// PMAC test-TX packet generator with cnt packets per period.
```

## L176 · `pwm(mmio, R_PMAC_TX_PRD, B_PMAC_PTX_EN, 0);`

```
// Stop: clear PTX_EN (PKTS_TX mode always in alimentk).
```

## L183 · `pwm(mmio, R_PMAC_GNT,   B_PMAC_GNT_TXEN,  1);`

```
// Enable path: prepare MAC, pause PD, set TX-only CCA, then kick off.
```

## L192 · `pwm(mmio, R_PMAC_TX_PRD, B_PMAC_PTX_EN, 1);`

```
// Start PKTS_TX mode
```

## L197 · `pwm(mmio, R_PMAC_TX_CTRL, B_PMAC_TXEN_DIS, 1);`

```
// Pulse TXEN_DIS to force packet gen start
```

## L202-205 · `pub fn tx_mode_switch_off(mmio: i32) {`

```
/// __rtw8852bx_bb_tx_mode_switch(mode=0) — common.c:1552. Hands BB TX
/// grant back from PMAC (test/cal) to CMAC (normal packets). Must be
/// called at the end of alimentk, otherwise PMAC keeps the grant and
/// every real frame silently dies before the BB TX counter.
```

## L216 · `#[derive(Copy, Clone, Default)]`

```
/// Snapshot of BB registers touched by alimentk for save/restore.
```

## L225 · `pub tx_pwr:        i32,  // sign-extended s9`

```
// sign-extended s9
```

## L228 · `pub fn backup_tssi(mmio: i32) -> TssiBak {`

```
/// __rtw8852bx_bb_backup_tssi (common.c:1570).
```

## L237 · `let raw = ((pr(mmio, R_TXPWR) & B_TXPWR_MSK) >> 8) & 0x1FF;`

```
// Sign-extend 9-bit signed power
```

## L243 · `pub fn restore_tssi(mmio: i32, b: &TssiBak) {`

```
/// __rtw8852bx_bb_restore_tssi (common.c:1586).
```

## L246 · `if b.tx_path == 3 /* RF_AB */ {`

```
/* RF_AB */
```

