# `tools/wasm/sdk/widgets/src/check_abi.rs` @ 5e0102684

## L1-7 · `#![allow(dead_code)]`

```
//! Compile-time ABI ordering lock — SDK-side mirror of the kernel's
//! `kernel/src/shade/widgets/check_abi.rs`. Same mechanisms, same
//! expected constants. If these two files disagree, deserialization on
//! either side will silently misinterpret variants.
//!
//! If you land here after a compile error: the ABI changed. Revert
//! (append-only) or bump `WIRE_VERSION` on both sides intentionally.
```

## L15 · `assert!(Token::Surface         as u8 == 0);`

```
// Token
```

## L44 · `assert!(Role::None      as u8 == 0);`

```
// Role
```

## L57 · `assert!(TextStyle::Body    as u8 == 0);`

```
// TextStyle
```

## L65 · `assert!(IconId::None.0              == 0);`

```
// IconId
```

## L104 · `assert!(Align::Start   as u8 == 0);`

```
// Align / Axis
```

## L114 · `assert!(Density::Compact  as u8 == 0);`

```
// Density — vocab v2 container-query buckets.
```

## L119 · `assert!(MouseButton::Left   as u8 == 0);`

```
// MouseButton
```

## L124 · `assert!(WIRE_VERSION == 0x01);`

```
// Wire version
```

## L127 · `assert!(crate::app_meta::APP_META_WIRE == 0x01);`

```
// AppMeta wire version (tracked separately from widget wire).
```

## L137 · `fn _widget_wire_position(w: &Widget) -> usize {`

```
// ── Exhaustive-match locks (fieldful enums) ───────────────────────────
```

## L176 · `Modifier::Hover(_)          => 13,`

```
// Vocab v2 — Tailwind-style additions.
```

