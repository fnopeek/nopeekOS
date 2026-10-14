//! intel_clock_gating.c (Linux 6.18.55): `intel_clock_gating_init`.
//! `intel_clock_gating_hooks_init` gives Tiger Lake no hook of its own,
//! so it gets `nop_init_clock_gating`.

use crate::i915_drv::drm_i915_private;

/// `intel_clock_gating_init` → `nop_init_clock_gating`.
pub fn intel_clock_gating_init(_i915: &mut drm_i915_private) {}
