# `tools/wasm/wifi/src/dpk_tables.rs` @ 5e0102684

## L1-4 · `#![allow(dead_code)]`

```
//! DPK tables — 1:1 port from Linux rtw8852b_rfk_table.c.
//!
//! Each entry: (addr, mask, value). PHY-space registers get PHY_CR_BASE
//! (0x10000) added when accessed.
```

## L8-9 · `pub static RTW8852B_DPK_AFE_DEFS: &[(u32, u32, u32)] = &[`

```
/// Linux rtw8852b_dpk_afe_defs (rtw8852b_rfk_table.c:169) — 29 entries.
/// Applied at entry to DPK cal (before per-path measurement loop).
```

## L42-43 · `pub static RTW8852B_DPK_AFE_RESTORE_DEFS: &[(u32, u32, u32)] = &[`

```
/// Linux rtw8852b_dpk_afe_restore_defs (rtw8852b_rfk_table.c:203) — 24 entries.
/// Applied at exit from DPK cal.
```

## L71-72 · `pub static RTW8852B_DPK_KIP_DEFS: &[(u32, u32, u32)] = &[`

```
/// Linux rtw8852b_dpk_kip_defs (rtw8852b_rfk_table.c:232) — 2 entries.
/// Applied in _dpk_kip_restore.
```

