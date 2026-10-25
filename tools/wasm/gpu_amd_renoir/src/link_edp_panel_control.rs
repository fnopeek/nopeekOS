//! display/dc/link/protocols/link_edp_panel_control.c (Linux 6.18.55), the
//! backlight of an eDP panel driven by PWM, and its exports in
//! core/dc_link_exports.c. The AUX (nits) backlight is OLED's and joins
//! with it.

use crate::amdgpu_device::amdgpu_device;
use crate::dc::dc;
use crate::dc_types::{set_backlight_level_params, MAX_PIPES};
use crate::linux::Line;

/// `get_pipe_from_link`: the pipe of the current state whose stream is on
/// link `li`.
fn get_pipe_from_link(dc: &dc, li: usize) -> Option<usize> {
    let cur = dc.current_state.as_deref().unwrap();

    (0..MAX_PIPES).find(|&i| cur.res_ctx.pipe_ctx[i].stream
        .is_some_and(|s| dc.streams[s].as_deref().unwrap().link == li))
}

/// `edp_set_backlight_level`: the PWM level of the panel on link `li`,
/// kept as the panel's user level, without ramping while it shows no
/// plane.
pub fn edp_set_backlight_level(adev: &amdgpu_device, dc: &mut dc, li: usize,
                               backlight_level_params: &mut set_backlight_level_params) -> bool {
    let backlight_pwm_u16_16 = backlight_level_params.backlight_pwm_u16_16;
    let mut frame_ramp = backlight_level_params.frame_ramp;

    Line::new().s("New Backlight level: ").dec(backlight_pwm_u16_16 as u64).s(" (0x").hex(backlight_pwm_u16_16)
        .s(")").dbg();

    if crate::link_detection::dc_is_embedded_signal(dc.links[li].connector_signal) {
        let pipe_ctx = get_pipe_from_link(dc, li);

        if let Some(p) = dc.links[li].panel_cntl.as_deref_mut() {
            p.stored_backlight_registers.USER_LEVEL = backlight_pwm_u16_16;
        }

        let Some(p) = pipe_ctx else { return false };
        let cur = dc.current_state.take().unwrap();
        // Disable brightness ramping when the display is blanked as it can
        // hang the DMCU
        if cur.res_ctx.pipe_ctx[p].plane_state.is_none() {
            frame_ramp = 0;
        }

        backlight_level_params.frame_ramp = frame_ramp;

        (dc.hwss.set_backlight_level.unwrap())(adev, dc, &cur.res_ctx.pipe_ctx[p], backlight_level_params);
        dc.current_state = Some(cur);
    }
    true
}

/// `get_abm_from_stream_res`: whether the pipe of the stream on link `li`
/// has the ABM.
fn get_abm_from_stream_res(dc: &dc, li: usize) -> bool {
    let cur = dc.current_state.as_deref().unwrap();

    for i in 0..MAX_PIPES {
        let pipe_ctx = &cur.res_ctx.pipe_ctx[i];

        if pipe_ctx.stream.is_some_and(|s| dc.streams[s].as_deref().unwrap().link == li) {
            return pipe_ctx.stream_res.abm;
        }
    }
    false
}

/// `edp_get_backlight_level`: the ABM firmware's current level.
/// `res_pool->dmcu` is NULL with a DMCUB (`config.disable_dmcu`), so the
/// firmware sets the brightness and the PWM read of `panel_cntl` is not
/// reached.
pub fn edp_get_backlight_level(adev: &amdgpu_device, dc: &dc, li: usize) -> i32 {
    let abm = get_abm_from_stream_res(dc, li).then(|| dc.res_pool.as_deref().unwrap().abm.as_deref()).flatten();

    match abm.and_then(|a| a.funcs().get_current_backlight) {
        Some(get) => get(adev, dc) as i32,
        None => crate::regs::DC_ERROR_UNEXPECTED as i32,
    }
}

/// `dc_link_set_backlight_level` (dc_link_exports.c).
pub fn dc_link_set_backlight_level(adev: &amdgpu_device, dc: &mut dc, li: usize,
                                   backlight_level_params: &mut set_backlight_level_params) -> bool {
    edp_set_backlight_level(adev, dc, li, backlight_level_params)
}

/// `dc_link_get_backlight_level` (dc_link_exports.c).
pub fn dc_link_get_backlight_level(adev: &amdgpu_device, dc: &dc, li: usize) -> i32 {
    edp_get_backlight_level(adev, dc, li)
}
