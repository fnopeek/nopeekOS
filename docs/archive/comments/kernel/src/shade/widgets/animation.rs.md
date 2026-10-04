# `kernel/src/shade/widgets/animation.rs` @ 5e0102684

## L1-16 · `#![allow(dead_code)]`

```
//! Widget animation — Q16.16 fixed-point spring + linear tween.
//!
//! Per docs/archive/PHASE10_WIDGETS.md: interpolation lives in the compositor,
//! apps only declare intent via `Modifier::Transition(..)`. Math is
//! deterministic fixed-point (floats drift across cores + variable
//! wakeup latency).
//!
//! **Status (v0.61.0):** infra + math primitives only. No active
//! consumers yet — proper wiring needs the tree-diff path (queued
//! for the post-P10.9 cleanup). Once we know the delta between
//! successive commits, each animatable modifier (Background,
//! Opacity, Padding, x/y position) becomes a tween entry here.
//!
//! Self-scheduling tick: `tick()` is called by shade's poll_render
//! at 60 Hz while `any_active()` is true. Returns to dirty-driven
//! (event → render → idle) when all tweens have settled.
```

## L20-21 · `pub type Q16_16 = i32;`

```
/// Q16.16 fixed-point: high 16 bits = integer part (signed),
/// low 16 bits = fraction (1/65536 unit).
```

## L24 · `pub const TICK_HZ: u32 = 60;`

```
/// Frames per second the compositor animates at.
```

## L35-38 · `pub fn spring_step(`

```
/// Spring physics — one step towards `target` from `current` given
/// `velocity`, using `stiffness` (Q16.16) and `damping` (Q16.16).
/// Returns (new_current, new_velocity). Standard critically-damped
/// spring at default values.
```

## L43 · `let delta = target.wrapping_sub(current);`

```
// accel = stiffness * (target - current) - damping * velocity
```

## L48 · `let dt: Q16_16 = (1 << 16) / TICK_HZ as i32;`

```
// dt = 1 / TICK_HZ, approximated as Q16.16 = 65536 / 60 ≈ 1092
```

## L55-56 · `pub fn linear_step(current: Q16_16, target: Q16_16, remaining_ticks: u32) -> Q16_16 {`

```
/// Linear interpolation towards `target` over `remaining_ticks`.
/// Returns new value; caller decrements ticks externally.
```

## L64 · `pub fn qmul(a: Q16_16, b: Q16_16) -> Q16_16 {`

```
/// Q16.16 multiply. Shifts after 64-bit extension to avoid overflow.
```

## L69-77 · `pub fn tick() {`

```
// ── Tick scheduler (stub) ─────────────────────────────────────────────
//
// Real implementation: walks every scene's in-flight tween list,
// advances one step, rebuilds that scene's pixel buffer if any
// value changed, marks the window dirty.
//
// For now no tween list is populated (no diff → no transitions),
// so tick() is a no-op. Shade can already call it — when diff
// lands this lights up without further integration.
```

## L80-81 · `}`

```
// No-op until tree diff feeds `Transition`-modifier deltas into
// per-scene tween state.
```

