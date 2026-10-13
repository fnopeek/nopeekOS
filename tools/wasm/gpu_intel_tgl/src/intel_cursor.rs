//! display/intel_cursor.c (Linux 6.18.55): the cursor plane of a display
//! version 12 pipe (i9xx cursor, not i845/i865), its identity and enable
//! state.

use crate::i915_drv::drm_i915_private;
use crate::intel_de::*;
use crate::intel_display_power::*;
use crate::intel_display_types::*;
use crate::regs::*;
use crate::skl_universal_plane::INTEL_FRONTBUFFER;

/// `intel_cursor_plane_create`: the hardware identity of the cursor.
pub fn intel_cursor_plane_create(pipe: u32) -> intel_plane {
    intel_plane {
        pipe,
        id: PLANE_CURSOR,
        i9xx_plane: pipe,
        frontbuffer_bit: INTEL_FRONTBUFFER(pipe, PLANE_CURSOR),
        fbc_id: None,
        kind: plane_kind::i9xx_cursor,
        state: intel_plane_state::default(),
    }
}

/// `i9xx_cursor_get_hw_state` (display version 5+: the cursor stays on
/// its pipe).
pub fn i9xx_cursor_get_hw_state(i915: &mut drm_i915_private, plane: &intel_plane, pipe: &mut u32) -> bool {
    let power_domain = POWER_DOMAIN_PIPE(plane.pipe);
    if !intel_display_power_get_if_enabled(i915, power_domain) {
        return false;
    }

    let val = intel_de_read(i915, CURCNTR(plane.pipe));
    let ret = val & MCURSOR_MODE_MASK != 0;
    *pipe = plane.pipe;

    intel_display_power_put(i915, power_domain);
    ret
}
