//! display/dc/link/link_detection.c (Linux 6.18.55): the connection test
//! of a link — its HPD level, with the eDP panel powered first. Sink
//! detection proper (EDID, DPCD) joins with the commit stage.
#![allow(dead_code)]

use crate::amdgpu_device::amdgpu_device;
use crate::dc::dc;
use crate::gpio_base::{dal_gpio_close, dal_gpio_get_value, dal_gpio_open, gpio_mode};
use crate::link_hpd::link_get_hpd_gpio;
use crate::regs::*;

/// `query_hpd_status` (link_hpd.c): the HPD pin's level.
pub fn query_hpd_status(adev: &amdgpu_device, dc: &mut dc, li: usize, is_hpd_high: &mut u32) -> bool {
    let link_id = dc.links[li].link_id;
    let Some(gs) = dc.ctx.gpio_service.as_deref_mut() else { return false };
    let Some(mut hpd_pin) = link_get_hpd_gpio(&dc.ctx.dc_bios, &adev.bios, link_id, gs) else { return false };

    dal_gpio_open(adev, gs, &mut hpd_pin, gpio_mode::GPIO_MODE_INTERRUPT);
    dal_gpio_get_value(adev, &hpd_pin, is_hpd_high);
    dal_gpio_close(adev, gs, &mut hpd_pin);
    true
}

/// `link_detect_connection_type`. Every link of DCN 2.1 is a PHY endpoint
/// (no USB4 DPIA), so the DPIA branch is not ported.
pub fn link_detect_connection_type(adev: &amdgpu_device, dc: &mut dc, li: usize, r#type: &mut u32) -> bool {
    let mut is_hpd_high = 0;
    let signal = dc.links[li].connector_signal;

    if signal == LF_SIGNAL_TYPE_LVDS {
        *r#type = dc_connection_single;
        return true;
    }

    if signal == LF_SIGNAL_TYPE_EDP {
        // in case it is not on
        if !dc.config.edp_no_power_sequencing {
            if let Some(f) = dc.hwss.edp_power_control {
                f(adev, dc, li, true);
            }
        }
        if let Some(f) = dc.hwss.edp_wait_for_hpd_ready {
            f(adev, dc, li, true);
        }
    }

    if dc.links[li].ep_type != LF_DISPLAY_ENDPOINT_PHY {
        crate::linux::Line::new().s("link_detect_connection_type: DPIA link not ported").say();
        *r#type = dc_connection_none;
        return true;
    }

    if !query_hpd_status(adev, dc, li, &mut is_hpd_high) {
        return false;
    }

    if is_hpd_high != 0 {
        *r#type = dc_connection_single;
    } else {
        *r#type = dc_connection_none;
        if signal == LF_SIGNAL_TYPE_EDP {
            // eDP is not connected, power down it
            if !dc.config.edp_no_power_sequencing {
                if let Some(f) = dc.hwss.edp_power_control {
                    f(adev, dc, li, false);
                }
            }
        }
    }
    true
}
