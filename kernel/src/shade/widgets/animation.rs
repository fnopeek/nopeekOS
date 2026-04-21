//! Widget animation — Q16.16 fixed-point spring + linear tween.
//!
//! Per PHASE10_WIDGETS.md: interpolation lives in the compositor,
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

#![allow(dead_code)]

/// Q16.16 fixed-point: high 16 bits = integer part (signed),
/// low 16 bits = fraction (1/65536 unit).
pub type Q16_16 = i32;

/// Frames per second the compositor animates at.
pub const TICK_HZ: u32 = 60;

pub const fn from_i32(v: i32) -> Q16_16 {
    v.saturating_mul(1 << 16)
}

pub const fn to_i32_round(v: Q16_16) -> i32 {
    (v.wrapping_add(1 << 15)) >> 16
}

/// Spring physics — one step towards `target` from `current` given
/// `velocity`, using `stiffness` (Q16.16) and `damping` (Q16.16).
/// Returns (new_current, new_velocity). Standard critically-damped
/// spring at default values.
pub fn spring_step(
    current: Q16_16, velocity: Q16_16, target: Q16_16,
    stiffness: Q16_16, damping: Q16_16,
) -> (Q16_16, Q16_16) {
    // accel = stiffness * (target - current) - damping * velocity
    let delta = target.wrapping_sub(current);
    let spring_force  = qmul(stiffness, delta);
    let damping_force = qmul(damping, velocity);
    let accel = spring_force.wrapping_sub(damping_force);
    // dt = 1 / TICK_HZ, approximated as Q16.16 = 65536 / 60 ≈ 1092
    let dt: Q16_16 = (1 << 16) / TICK_HZ as i32;
    let new_v = velocity.wrapping_add(qmul(accel, dt));
    let new_c = current.wrapping_add(qmul(new_v, dt));
    (new_c, new_v)
}

/// Linear interpolation towards `target` over `remaining_ticks`.
/// Returns new value; caller decrements ticks externally.
pub fn linear_step(current: Q16_16, target: Q16_16, remaining_ticks: u32) -> Q16_16 {
    if remaining_ticks == 0 { return target; }
    let delta = target.wrapping_sub(current);
    let step = delta / remaining_ticks as i32;
    current.wrapping_add(step)
}

/// Q16.16 multiply. Shifts after 64-bit extension to avoid overflow.
pub fn qmul(a: Q16_16, b: Q16_16) -> Q16_16 {
    ((a as i64 * b as i64) >> 16) as Q16_16
}

// ── Tick scheduler (stub) ─────────────────────────────────────────────
//
// Real implementation: walks every scene's in-flight tween list,
// advances one step, rebuilds that scene's pixel buffer if any
// value changed, marks the window dirty.
//
// For now no tween list is populated (no diff → no transitions),
// so tick() is a no-op. Shade can already call it — when diff
// lands this lights up without further integration.

pub fn tick() {
    // No-op until tree diff feeds `Transition`-modifier deltas into
    // per-scene tween state.
}

pub fn any_active() -> bool {
    false
}
