# `tools/wasm/wifi_rtl8822ce/src/bf.rs` @ 5e0102684

## L1-6 · `#![allow(dead_code)]`

```
//! `bf.c` aus Linux 6.18.26 rtw88 — nur `rtw_bf_phy_init`.
//!
//! Der Rest von bf.c (Beamformer/Beamformee anmelden, CSI-Raten, die
//! Gruppentabelle) haengt an einer VERBINDUNG und gehoert zu der Stufe, die
//! eine aufbaut. Hier steht die Grundeinstellung, die `phy_set_param` zum
//! Schluss setzt.
```

## L12 · `pub fn phy_init(h: i32) {`

```
/// bf.c:344-377 `rtw_bf_phy_init`
```

## L19 · `tmp32 |= BIT_MU_P1_WAIT_STATE_EN;`

```
// Enable P1 aggr new packet according to P0 transfer time
```

## L21 · `tmp32 &= !BIT_MASK_R_MU_RL;`

```
// MU Retry Limit
```

## L24 · `tmp32 &= !BIT_EN_MU_MIMO;`

```
// Disable Tx MU-MIMO until sounding done
```

## L26 · `tmp32 &= !BIT_MASK_R_MU_TABLE_VALID;`

```
// Clear validity of MU STAs
```

## L30 · `let tmp8 = (ack_policy << BIT_SHIFT_WMAC_TXMU_ACKPOLICY)`

```
// MU-MIMO Option as default value
```

## L35 · `host::w16(h, REG_WMAC_MU_BF_CTL, 0);`

```
// MU-MIMO Control as default value
```

## L37 · `host::set32(h, REG_TXBF_CTRL, BIT_USE_NDPA_PARAMETER);`

```
// Set MU NDPA rate & BW source
```

## L39 · `host::w8(h, REG_NDPA_OPT_CTRL, ndpa_rate);`

```
// Set NDPA Rate
```

