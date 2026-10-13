//! display/intel_gmbus.c (Linux 6.18.55): `intel_gmbus_setup` for PCH TGP.
//! The I2C adapters are the DRM side; what reaches the hardware is the
//! register base, each pin's GPIO register and the reset.

use crate::i915_drv::drm_i915_private;
use crate::intel_de::*;
use crate::regs::*;
use crate::tables;

pub const NUM_PINS: usize = GMBUS_NUM_PINS as usize;

/// One row of `gmbus_pins_icp`: whether the pin exists and its GPIO.
#[derive(Clone, Copy)]
pub struct gmbus_pin_gpio {
    pub valid: u32,
    pub gpio: u32,
}

/// `struct intel_gmbus`, the hardware part.
#[derive(Clone, Copy, Default)]
pub struct intel_gmbus {
    pub present: bool,
    pub reg0: u32,
    pub gpio_reg: u32,
}

/// `get_gmbus_pin` for PCH TGP (at least ICP, below DG1).
fn get_gmbus_pin(pin: usize) -> Option<&'static gmbus_pin_gpio> {
    tables::gmbus_pins_icp_gpio.get(pin).filter(|p| p.valid != 0)
}

/// `intel_gmbus_reset`.
pub fn intel_gmbus_reset(i915: &mut drm_i915_private) {
    intel_de_write(i915, GMBUS0_PCH, 0);
    intel_de_write(i915, GMBUS4_PCH, 0);
}

/// `intel_gmbus_setup`: no GMCH, so the base is PCH_DISPLAY_BASE.
pub fn intel_gmbus_setup(i915: &mut drm_i915_private) {
    i915.display.gmbus_mmio_base = PCH_DISPLAY_BASE;
    for pin in 0..NUM_PINS {
        let Some(p) = get_gmbus_pin(pin) else { continue };
        i915.display.gmbus[pin] = intel_gmbus {
            present: true,
            // A conservative clock rate by default.
            reg0: pin as u32 | GMBUS_RATE_100KHZ,
            gpio_reg: GPIO_PCH_BASE + GPIO_PCH_STRIDE * p.gpio,
        };
    }
    intel_gmbus_reset(i915);
}
