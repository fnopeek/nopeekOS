# `kernel/src/shade/widgets/check_abi.rs` @ 5e0102684

## L1-20 · `#![allow(dead_code)]`

```
//! Compile-time ABI ordering lock.
//!
//! Postcard serializes enum variants by **declaration order**, not by
//! name. Inserting a variant in the middle of an ABI enum breaks every
//! serialized tree written before the change. Removing one does the
//! same.
//!
//! This module pins the ordering with two mechanisms:
//!
//!   1. `const _: () = assert!(...)` on `#[repr(u8/u16)]` enums — a
//!      reordering changes a discriminant, the compile fails.
//!
//!   2. Exhaustive `match` functions over field-carrying enums (`Widget`,
//!      `Modifier`, `Event`, …). Adding a variant without updating the
//!      match produces a non-exhaustive-match error. Within the defining
//!      crate, `#[non_exhaustive]` does not suppress this check.
//!
//! If you land here after a compile error: the ABI changed. Either
//! revert (variants must be appended only) or bump `WIRE_VERSION` and
//! update this file intentionally.
```

## L26 · `const _: () = {`

```
// ── Integer-discriminant locks ────────────────────────────────────────
```

## L29 · `assert!(Token::Surface         as u8 == 0);`

```
// Token — values frozen forever, appended only.
```

## L58 · `assert!(Role::None      as u8 == 0);`

```
// Role — values frozen, appended only.
```

## L71 · `assert!(TextStyle::Body    as u8 == 0);`

```
// TextStyle — values frozen.
```

## L79 · `assert!(IconId::None.0              == 0);`

```
// IconId — u16, P10.9 Phosphor set frozen.
```

## L119 · `assert!(Align::Start   as u8 == 0);`

```
// Align / Axis — used inside Widget struct variants, positions frozen.
```

## L129 · `assert!(Density::Compact  as u8 == 0);`

```
// Density — vocab v2 container-query buckets.
```

## L134 · `assert!(MouseButton::Left   as u8 == 0);`

```
// MouseButton — Event payload, position frozen.
```

## L139 · `assert!(WIRE_VERSION == 0x01);`

```
// Wire version — must be 0x01 for v1.
```

## L143 · `fn _widget_wire_position(w: &Widget) -> usize {`

```
// ── Exhaustive-match locks (fieldful enums) ───────────────────────────
```

## L145-150 · `fn _widget_wire_position(w: &Widget) -> usize {`

```
/// Lock `Widget` variant order. Returns the wire position postcard will
/// write for each variant. If a variant is added, inserted, or removed,
/// this match becomes non-exhaustive (or the expected constant below
/// drifts) and compilation fails.
///
/// Never called — the match is evaluated at type-check time.
```

## L153 · `Widget::Column   { .. } => 0,`

```
// Containers
```

## L158 · `Widget::Text     { .. } => 4,`

```
// Leaves
```

## L167 · `Widget::Popover  { .. } => 12,`

```
// Reserved (v2+)
```

## L176 · `fn _modifier_wire_position(m: &Modifier) -> usize {`

```
/// Lock `Modifier` variant order. Same mechanism as above.
```

## L179 · `Modifier::Padding(_)        => 0,`

```
// Active in v1
```

## L188 · `Modifier::Blur(_)           => 8,`

```
// Reserved (v2+ effects, role)
```

## L194 · `Modifier::Hover(_)          => 13,`

```
// Vocab v2 — Tailwind-style additions.
```

## L219 · `fn _event_wire_position(e: &Event) -> usize {`

```
/// Lock `Event` variant order.
```

## L241 · `fn _action_wire_position(a: &Action) -> usize {`

```
/// Lock `Action` variant order.
```

## L250 · `fn _transition_wire_position(t: &Transition) -> usize {`

```
/// Lock `Transition` variant order.
```

## L258 · `fn _fill_wire_position(f: &Fill) -> usize {`

```
/// Lock `Fill` variant order.
```

