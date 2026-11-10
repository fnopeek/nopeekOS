//! The board's activity LED as a sign of life before any screen works.
//!
//! The device tree names it as the `ACT` child of a `gpio-leds` node; on
//! the Raspberry Pi 5 it sits on a Broadcom STB GPIO block (`gpio`).

use super::dt::Fdt;
use super::gpio::Line;

static ACT: spin::Mutex<Option<Line>> = spin::Mutex::new(None);

/// Find the ACT LED and drive it as an output, off. False without one.
pub fn init(fdt: &Fdt) -> bool {
    let Some(leds) = fdt.find_compatible(b"gpio-leds") else { return false };
    let mut act = None;
    fdt.walk(|n| {
        if n.ancestors().last() == Some(&leds.offset) && fdt.prop_has_str(n, b"label", b"ACT") {
            act = Some(*n);
            return false;
        }
        true
    });
    let Some(line) = act.and_then(|a| Line::from_dt(fdt, &a, b"gpios", 0)) else { return false };
    line.output(false);
    *ACT.lock() = Some(line);
    true
}

/// Turn the LED on or off; nothing without one.
pub fn set(on: bool) {
    if let Some(l) = *ACT.lock() {
        l.set(on);
    }
}
