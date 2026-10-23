//! display/dc/link/link_dpms.c (Linux 6.18.55): the link power steps the
//! hardware init reaches.
#![allow(dead_code)]

use crate::amdgpu_device::amdgpu_device;
use crate::dc::dc;
use crate::regs::*;

/// `link_blank_all_dp_displays` (link_dpms.c): blank every lit DisplayPort
/// sink before detection. Only a link with a DM connector and a sink can be
/// lit, and both come after detection; the DPCD power-state read and the
/// stream blank join with sink detection.
pub fn link_blank_all_dp_displays(adev: &amdgpu_device, dc: &mut dc) {
    let _ = adev;
    for i in 0..dc.link_count as usize {
        let link = &dc.links[i];
        if link.connector_signal != LF_SIGNAL_TYPE_DISPLAY_PORT || link.priv_.is_none() || link.local_sink.is_none() {
            continue;
        }

        crate::linux::Line::new().s("link_blank_all_dp_displays: dp_retrieve_lttpr_cap not ported").say();
    }
}
