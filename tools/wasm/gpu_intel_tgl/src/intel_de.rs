//! display/intel_de.h (Linux 6.18.55): display register access.
//!
//! The DMC wakelock (`intel_dmc_wl_get`/`put`) is only taken from display
//! version 20 on (`intel_dmc_wl_sanitize_param`), so on Tiger Lake these
//! are plain uncore accesses.

use crate::i915_drv::drm_i915_private;

pub fn intel_de_read(i915: &mut drm_i915_private, reg: u32) -> u32 {
    i915.uncore.intel_uncore_read(reg)
}

pub fn intel_de_write(i915: &mut drm_i915_private, reg: u32, val: u32) {
    i915.uncore.intel_uncore_write(reg, val)
}

pub fn intel_de_rmw(i915: &mut drm_i915_private, reg: u32, clear: u32, set: u32) -> u32 {
    i915.uncore.intel_uncore_rmw(reg, clear, set)
}

/// `intel_de_wait` → `intel_wait_for_register`: 2 us busy, then the rest
/// of `timeout_ms` sleeping.
pub fn intel_de_wait(i915: &mut drm_i915_private, reg: u32, mask: u32, value: u32,
                     timeout_ms: u32) -> Result<(), ()> {
    i915.uncore.__intel_wait_for_register(reg, mask, value, 2, timeout_ms, None)
}

/// `intel_de_wait_custom`.
pub fn intel_de_wait_custom(i915: &mut drm_i915_private, reg: u32, mask: u32, value: u32,
                            fast_timeout_us: u32, slow_timeout_ms: u32,
                            out_value: Option<&mut u32>) -> Result<(), ()> {
    i915.uncore.__intel_wait_for_register(reg, mask, value, fast_timeout_us, slow_timeout_ms, out_value)
}

pub fn intel_de_wait_for_set(i915: &mut drm_i915_private, reg: u32, mask: u32,
                             timeout_ms: u32) -> Result<(), ()> {
    intel_de_wait(i915, reg, mask, mask, timeout_ms)
}

pub fn intel_de_wait_for_clear(i915: &mut drm_i915_private, reg: u32, mask: u32,
                               timeout_ms: u32) -> Result<(), ()> {
    intel_de_wait(i915, reg, mask, 0, timeout_ms)
}
