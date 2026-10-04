# `tools/wasm/wifi/src/pwr_trim.rs` @ 5e0102684

## L1-12 · `use crate::efuse::EfuseData;`

```
//! rtw89_chip_power_trim — 1:1 port of rtw8852bx_thermal_trim
//! (rtw8852b_common.c:335) + rtw8852bx_pa_bias_trim (common.c:383).
//!
//! Called once from phy_dm_init right after set_txpwr_ctrl (phy.c:7706).
//! Applies per-chip PA-bias and thermal offsets loaded from efuse's
//! phycap region. Without these applied, PA bias stays at the HW
//! default which on some chips leaves the amplifier inactive, and
//! thermal correction is zero — neither is catastrophic on its own
//! but together they can keep TX physically silent.
//!
//! Both steps are gated by PG flags: if efuse byte is 0xFF, no PG,
//! skip (= NO-OP).
```

## L17-18 · `const RR_TM2: u32 = 0x43;`

```
// RF register addresses + masks (reg.h:8543+8574). Non-V1 variants
// apply for 8852B (V1 = 8852C).
```

## L20 · `const RR_TM2_OFF: u32 = 0x000F_0000; // GENMASK(19,16)`

```
// GENMASK(19,16)
```

## L23 · `const RR_BIASA_TXA: u32 = 0x000F_0000; // GENMASK(19,16)  — 5G`

```
// GENMASK(19,16)  — 5G
```

## L24 · `const RR_BIASA_TXG: u32 = 0x0000_F000; // GENMASK(15,12)  — 2G`

```
// GENMASK(15,12)  — 2G
```

## L26-29 · `fn thermal_trim(mmio: i32, e: &EfuseData) {`

```
/// rtw8852bx_thermal_trim (common.c:335).
/// For each RF path, if pg_thermal_trim is set, compute:
///   val = ((raw & 0x1) << 3) | ((raw & 0x1f) >> 1)
/// and write to RR_TM2[19:16].
```

## L43-47 · `fn pa_bias_trim(mmio: i32, e: &EfuseData) {`

```
/// rtw8852bx_pa_bias_trim (common.c:383).
/// For each RF path, if pg_pa_bias_trim is set:
///   pabias_2g = raw & 0xF
///   pabias_5g = (raw >> 4) & 0xF
/// write both into RR_BIASA (RR_BIASA_TXG = 2G, RR_BIASA_TXA = 5G).
```

## L63 · `pub fn run(mmio: i32, e: &EfuseData) {`

```
/// rtw89_chip_power_trim → __rtw8852bx_power_trim (common.c:445).
```

