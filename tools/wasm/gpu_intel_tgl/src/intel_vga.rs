//! display/intel_vga.c (Linux 6.18.55): `intel_vga_disable` for display
//! version 5+ (`CPU_VGACNTRL`, pipe A).

use crate::i915_drv::drm_i915_private;
use crate::intel_de::*;
use crate::linux::Line;
use crate::regs::*;

/// `intel_vga_disable`. Before it disables the VGA plane, Linux blanks the
/// legacy VGA screen through the sequencer (SR01 at ports 0x3c4/0x3c5);
/// no kernel call gives a module that port, so a VGA plane that is still
/// on is reported and left alone. UEFI firmware with GOP leaves it off.
pub fn intel_vga_disable(i915: &mut drm_i915_private) {
    let tmp = intel_de_read(i915, CPU_VGACNTRL);
    if tmp & VGA_DISP_DISABLE != 0 {
        return;
    }
    Line::new().s("VGA plane on pipe A is enabled; disabling it needs the VGA sequencer, not done").say();
}
