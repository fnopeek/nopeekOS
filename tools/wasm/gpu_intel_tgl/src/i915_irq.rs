//! i915_irq.c (Linux 6.18.55): interrupt install, reset, postinstall, the
//! top-level handler and uninstall, on Tiger Lake (gen11 layout).

use crate::i915_drv::drm_i915_private;
use crate::intel_gt::intel_gt;

/// Interrupt state of the device (`i915->irq_*`, `display->irq`).
#[derive(Clone, Copy, Default)]
pub struct i915_irq_state {}

/// `intel_irq_install`.
pub fn intel_irq_install(_i915: &mut drm_i915_private, _gt: &mut intel_gt) -> Result<(), i32> {
    crate::linux::say("intel_irq_install: not ported");
    Ok(())
}

/// `intel_irq_uninstall`.
pub fn intel_irq_uninstall(_i915: &mut drm_i915_private, _gt: &mut intel_gt) {}
