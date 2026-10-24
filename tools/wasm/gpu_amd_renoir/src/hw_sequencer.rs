//! display/dc/inc/hw_sequencer.h + hwss/hw_sequencer_private.h +
//! hwss/dcn21/dcn21_init.c + hwss/dce110/dce110_hwseq.c +
//! core/dc_hw_sequencer.c (Linux 6.18.55): the hw sequencer vtables DCN 2.1
//! binds, grow-as-reached, and the eDP panel power and HPD waits of dce110
//! it uses. A link is named by its index in `dc.links`, a pipe by its index
//! in the current state's `pipe_ctx`, a block by its index in the pool.
#![allow(dead_code)]

use crate::amdgpu_device::amdgpu_device;
use crate::bios_parser2::bp_result;
use crate::command_table2::{bios_parser_enable_lvtma_control, bios_parser_transmitter_control, bp_transmitter_control};
use crate::dc::dc;
use crate::dc_state::dc_state;
use crate::dc_types::MAX_PIPES;
use crate::dc_helper::{dm_get_elapse_time_in_ns, dm_get_timestamp};
use crate::dce_panel_cntl::dce_is_panel_powered_on;
use crate::gpio_base::{dal_gpio_close, dal_gpio_get_value, dal_gpio_open, gpio_mode};
use crate::link_factory::{dp_trace_get_edp_poweroff_timestamp, dp_trace_set_edp_power_timestamp};
use crate::link_hpd::link_get_hpd_gpio;
use crate::linux::{msleep, Line};
use crate::regs::*;

/// `struct hw_sequencer_funcs` (hw_sequencer.h), the members whose bodies
/// are ported. `None` for a member is not a choice of DCN 2.1: it is one
/// the port has not reached. The `state` Linux passes beside a pipe is the
/// current state here.
#[derive(Default, Clone, Copy)]
pub struct hw_sequencer_funcs {
    pub init_hw: Option<fn(&amdgpu_device, &mut dc)>,
    pub apply_ctx_to_hw: Option<fn(&amdgpu_device, &mut dc, &mut dc_state) -> crate::dc_types::dc_status>,
    pub program_front_end_for_ctx: Option<fn(&amdgpu_device, &mut dc, &mut dc_state)>,
    pub post_unlock_program_front_end: Option<fn(&amdgpu_device, &mut dc, &mut dc_state)>,
    pub update_pending_status: Option<fn(&amdgpu_device, &mut dc, &crate::dc_state::pipe_ctx)>,
    pub enable_accelerated_mode: Option<fn(&amdgpu_device, &mut dc, &mut dc_state)>,
    pub update_info_frame: Option<fn(&amdgpu_device, &mut dc, &mut dc_state, usize)>,
    pub enable_stream: Option<fn(&amdgpu_device, &mut dc, &mut dc_state, usize)>,
    pub unblank_stream: Option<fn(&amdgpu_device, &mut dc, &mut dc_state, usize, &crate::dc_dp_types::dc_link_settings)>,
    pub enable_audio_stream: Option<fn(&amdgpu_device, &mut dc, &mut dc_state, usize)>,
    pub disable_plane: Option<fn(&amdgpu_device, &mut dc, usize)>,
    pub interdependent_update_lock: Option<fn(&amdgpu_device, &mut dc, Option<&dc_state>, bool)>,
    pub prepare_bandwidth: Option<fn(&amdgpu_device, &mut dc, &mut dc_state)>,
    pub optimize_bandwidth: Option<fn(&amdgpu_device, &mut dc, &mut dc_state)>,
    pub set_drr: Option<fn(&amdgpu_device, &mut dc, &mut dc_state, &[usize], &crate::dc_stream::dc_crtc_timing_adjust)>,
    pub set_static_screen_control: Option<fn(&amdgpu_device, &mut dc, &mut dc_state, &[usize], &crate::dc_types::dc_static_screen_params)>,
    pub setup_stereo: Option<fn(&amdgpu_device, &mut dc, &mut dc_state, usize)>,
    pub wait_for_mpcc_disconnect: Option<fn(&amdgpu_device, &mut dc, &crate::dc_state::pipe_ctx)>,
    pub edp_backlight_control: Option<fn(&amdgpu_device, &mut dc, usize, bool)>,
    pub edp_power_control: Option<fn(&amdgpu_device, &mut dc, usize, bool)>,
    pub edp_wait_for_hpd_ready: Option<fn(&amdgpu_device, &mut dc, usize, bool)>,
    pub init_sys_ctx: Option<fn(&amdgpu_device, &mut dc, &crate::dc::dc_phy_addr_space_config) -> i32>,
    pub set_flip_control_gsl: Option<fn(&amdgpu_device, &dc, usize, bool)>,
    pub optimize_pwr_state: Option<fn(&amdgpu_device, &mut dc, &mut dc_state)>,
    pub exit_optimized_pwr_state: Option<fn(&amdgpu_device, &mut dc, &mut dc_state)>,
    pub get_vupdate_offset_from_vsync: Option<fn(&dc, &dc_state, usize) -> i32>,
    pub set_abm_immediate_disable: Option<fn(&amdgpu_device, &mut dc, &dc_state, usize)>,
    pub enable_dp_link_output: Option<fn(&amdgpu_device, &mut dc, usize, &crate::dc_state::link_resource, u32, u32, &crate::dc_dp_types::dc_link_settings)>,
    pub disable_link_output: Option<fn(&amdgpu_device, &mut dc, usize, &crate::dc_state::link_resource, u32)>,
    pub is_abm_supported: Option<fn(&dc, &dc_state, usize) -> bool>,
    pub set_disp_pattern_generator: Option<fn(&amdgpu_device, &mut dc, &dc_state, usize, u32, u32, u32, Option<&crate::dcn20_opp::tg_color>, i32, i32, i32)>,
}

/// `dcn21_funcs` (dcn21_init.c), as far as ported.
const DCN21_FUNCS: hw_sequencer_funcs = hw_sequencer_funcs {
    init_hw: Some(crate::dcn10_hwseq::dcn10_init_hw),
    apply_ctx_to_hw: Some(dce110_apply_ctx_to_hw),
    program_front_end_for_ctx: Some(crate::dcn20_hwseq::dcn20_program_front_end_for_ctx),
    post_unlock_program_front_end: Some(crate::dcn20_hwseq::dcn20_post_unlock_program_front_end),
    update_pending_status: None,
    enable_accelerated_mode: Some(dce110_enable_accelerated_mode),
    update_info_frame: Some(dce110_update_info_frame),
    enable_stream: Some(crate::dcn20_hwseq::dcn20_enable_stream),
    unblank_stream: Some(crate::dcn20_hwseq::dcn20_unblank_stream),
    enable_audio_stream: Some(dce110_enable_audio_stream),
    disable_plane: Some(crate::dcn20_hwseq::dcn20_disable_plane),
    interdependent_update_lock: Some(crate::dcn10_hwseq::dcn10_lock_all_pipes),
    prepare_bandwidth: Some(crate::dcn20_hwseq::dcn20_prepare_bandwidth),
    optimize_bandwidth: Some(crate::dcn20_hwseq::dcn20_optimize_bandwidth),
    set_drr: None, // dcn10_set_drr
    set_static_screen_control: None, // dcn10_set_static_screen_control
    setup_stereo: Some(crate::dcn10_hwseq::dcn10_setup_stereo),
    wait_for_mpcc_disconnect: Some(crate::dcn10_hwseq::dcn10_wait_for_mpcc_disconnect),
    edp_backlight_control: Some(dce110_edp_backlight_control),
    edp_power_control: Some(dce110_edp_power_control),
    edp_wait_for_hpd_ready: Some(dce110_edp_wait_for_hpd_ready),
    init_sys_ctx: Some(crate::dcn21_hwseq::dcn21_init_sys_ctx),
    set_flip_control_gsl: Some(crate::dcn20_hwseq::dcn20_set_flip_control_gsl),
    optimize_pwr_state: Some(crate::dcn21_hwseq::dcn21_optimize_pwr_state),
    exit_optimized_pwr_state: Some(crate::dcn21_hwseq::dcn21_exit_optimized_pwr_state),
    get_vupdate_offset_from_vsync: Some(crate::dcn10_hwseq::dcn10_get_vupdate_offset_from_vsync),
    set_abm_immediate_disable: Some(crate::dcn21_hwseq::dcn21_set_abm_immediate_disable),
    enable_dp_link_output: Some(dce110_enable_dp_link_output),
    disable_link_output: Some(dce110_disable_link_output),
    is_abm_supported: Some(crate::dcn21_hwseq::dcn21_is_abm_supported),
    set_disp_pattern_generator: Some(crate::dcn20_hwseq::dcn20_set_disp_pattern_generator),
};

/// `struct hwseq_private_funcs` (hw_sequencer_private.h), the members whose
/// bodies are ported. The `None` members of `DCN21_PRIVATE_FUNCS` are
/// members dcn21 leaves out.
#[derive(Default, Clone, Copy)]
pub struct hwseq_private_funcs {
    pub init_pipes: Option<fn(&amdgpu_device, &mut dc, Option<&dc_state>)>,
    pub plane_atomic_disconnect: Option<fn(&amdgpu_device, &mut dc, usize)>,
    pub enable_display_power_gating: Option<fn(&amdgpu_device, &mut dc, u8, u32) -> bool>,
    pub blank_pixel_data: Option<fn(&amdgpu_device, &mut dc, &dc_state, usize, bool)>,
    pub reset_hw_ctx_wrap: Option<fn(&amdgpu_device, &mut dc, &mut dc_state)>,
    pub enable_stream_timing: Option<fn(&amdgpu_device, &mut dc, &mut dc_state, usize) -> crate::dc_types::dc_status>,
    pub edp_backlight_control: Option<fn(&amdgpu_device, &mut dc, usize, bool)>,
    pub disable_stream_gating: Option<fn(&amdgpu_device, &mut dc, &dc_state, usize)>,
    pub enable_stream_gating: Option<fn(&amdgpu_device, &mut dc, &dc_state, usize)>,
    pub setup_vupdate_interrupt: Option<fn(&amdgpu_device, &mut dc, &dc_state, usize)>,
    pub init_blank: Option<fn(&amdgpu_device, &dc, usize)>,
    pub disable_vga: Option<fn(&amdgpu_device, &crate::dce_hwseq::dce_hwseq)>,
    pub bios_golden_init: Option<fn(&amdgpu_device, &mut dc)>,
    pub plane_atomic_disable: Option<fn(&amdgpu_device, &mut dc, usize)>,
    pub plane_atomic_power_down: Option<fn(&amdgpu_device, &mut dc, usize, usize)>,
    pub enable_power_gating_plane: Option<fn(&amdgpu_device, &crate::dce_hwseq::dce_hwseq, bool)>,
    pub dpp_pg_control: Option<fn(&amdgpu_device, &dc, u32, bool)>,
    pub hubp_pg_control: Option<fn(&amdgpu_device, &dc, u32, bool)>,
    pub update_odm: Option<fn(&amdgpu_device, &mut dc, &mut dc_state, usize)>,
    pub dsc_pg_control: Option<fn(&amdgpu_device, &dc, u32, bool)>,
    pub dpp_root_clock_control: Option<fn(&amdgpu_device, &dc, u32, bool)>,
    pub verify_allow_pstate_change_high: Option<fn(&amdgpu_device, &dc)>,
    pub s0i3_golden_init_wa: Option<fn(&amdgpu_device, &dc) -> bool>,
    pub wait_for_blank_complete: Option<fn(&amdgpu_device, &crate::dcn20_opp::dcn20_opp) -> bool>,
}

/// `dcn21_private_funcs` (dcn21_init.c), as far as ported.
const DCN21_PRIVATE_FUNCS: hwseq_private_funcs = hwseq_private_funcs {
    init_pipes: Some(crate::dcn10_hwseq::dcn10_init_pipes),
    plane_atomic_disconnect: Some(crate::dcn10_hwseq::dcn10_plane_atomic_disconnect),
    enable_display_power_gating: Some(dcn10_dummy_display_power_gating),
    blank_pixel_data: Some(crate::dcn20_hwseq::dcn20_blank_pixel_data),
    reset_hw_ctx_wrap: Some(crate::dcn20_hwseq::dcn20_reset_hw_ctx_wrap),
    enable_stream_timing: Some(crate::dcn20_hwseq::dcn20_enable_stream_timing),
    edp_backlight_control: Some(dce110_edp_backlight_control),
    disable_stream_gating: Some(crate::dcn20_hwseq::dcn20_disable_stream_gating),
    enable_stream_gating: None, // dcn20_enable_stream_gating
    setup_vupdate_interrupt: Some(crate::dcn20_hwseq::dcn20_setup_vupdate_interrupt),
    init_blank: Some(crate::dcn20_hwseq::dcn20_init_blank),
    disable_vga: Some(crate::dcn20_hwseq::dcn20_disable_vga),
    bios_golden_init: Some(crate::dcn10_hwseq::dcn10_bios_golden_init),
    plane_atomic_disable: Some(crate::dcn20_hwseq::dcn20_plane_atomic_disable),
    plane_atomic_power_down: Some(crate::dcn10_hwseq::dcn10_plane_atomic_power_down),
    enable_power_gating_plane: Some(crate::dcn20_hwseq::dcn20_enable_power_gating_plane),
    dpp_pg_control: Some(crate::dcn20_hwseq::dcn20_dpp_pg_control),
    hubp_pg_control: Some(crate::dcn20_hwseq::dcn20_hubp_pg_control),
    update_odm: Some(crate::dcn20_hwseq::dcn20_update_odm),
    dsc_pg_control: Some(crate::dcn20_hwseq::dcn20_dsc_pg_control),
    dpp_root_clock_control: None,
    verify_allow_pstate_change_high: Some(crate::dcn10_hwseq::dcn10_verify_allow_pstate_change_high),
    s0i3_golden_init_wa: Some(crate::dcn21_hwseq::dcn21_s0i3_golden_init_wa),
    wait_for_blank_complete: Some(crate::dcn20_hwseq::dcn20_wait_for_blank_complete),
};

/// `dcn21_hw_sequencer_construct` (dcn21_init.c).
pub fn dcn21_hw_sequencer_construct(dc: &mut dc) {
    dc.hwss = DCN21_FUNCS;
    if let Some(hws) = dc.hwseq.as_deref_mut() {
        hws.funcs = DCN21_PRIVATE_FUNCS;
    }
}

/// `color_space_to_black_color` (dc_hw_sequencer.c): the black of a colour
/// space, full-range sRGB black for any not named here.
pub fn color_space_to_black_color(colorspace: u32, black_color: &mut crate::dcn20_opp::tg_color) {
    use crate::regs::*;
    let format = match colorspace {
        COLOR_SPACE_YCBCR601 | COLOR_SPACE_YCBCR709 | COLOR_SPACE_YCBCR709_BLACK | COLOR_SPACE_YCBCR601_LIMITED
        | COLOR_SPACE_YCBCR709_LIMITED | COLOR_SPACE_2020_YCBCR_LIMITED | COLOR_SPACE_2020_YCBCR_FULL => {
            BLACK_COLOR_FORMAT_YUV_CV
        }
        COLOR_SPACE_SRGB_LIMITED => BLACK_COLOR_FORMAT_RGB_LIMITED,
        _ => BLACK_COLOR_FORMAT_RGB_FULLRANGE,
    } as usize;
    let t = &crate::tables::black_color_format[format * 3..format * 3 + 3];
    *black_color = crate::dcn20_opp::tg_color { color_r_cr: t[0], color_g_y: t[1], color_b_cb: t[2] };
}

/// `dce110_edp_power_control` (dce110_hwseq.c): power the eDP panel up or
/// down through the VBIOS, keeping the minimum 500 ms off time.
pub fn dce110_edp_power_control(adev: &amdgpu_device, dc: &mut dc, li: usize, power_up: bool) {
    let link = &dc.links[li];
    let Some(enc) = link.link_enc.as_ref().map(|e| *e.base()) else { return };

    if enc.connector.get_connector_id() != CONNECTOR_ID_EDP {
        return;
    }

    let Some(panel_cntl) = link.panel_cntl.as_deref() else { return };
    if power_up == dce_is_panel_powered_on(adev, panel_cntl) {
        Line::new().s("dce110_edp_power_control: Skipping Panel Power action: ")
            .s(if power_up { "On" } else { "Off" }).dbg();
        return;
    }

    let current_ts = dm_get_timestamp();
    let poweroff_ts = dp_trace_get_edp_poweroff_timestamp(link);
    let time_since_edp_poweroff_ms = dm_get_elapse_time_in_ns(current_ts, poweroff_ts) / 1_000_000;

    // Send VBIOS command to prompt eDP panel power
    if power_up {
        // edp requires a min of 500ms from LCDVDD off to on
        let mut remaining_min_edp_poweroff_time_ms: u64 = 500;

        // add time defined by a patch, if any (usually patch extra_t12_ms is 0)
        if link.local_sink.is_some() {
            remaining_min_edp_poweroff_time_ms += link.panel_config.pps_extra_t12_ms as u64;
        }

        // Adjust remaining_min_edp_poweroff_time_ms if this is not the first time.
        if poweroff_ts != 0 {
            if time_since_edp_poweroff_ms < remaining_min_edp_poweroff_time_ms {
                remaining_min_edp_poweroff_time_ms -= time_since_edp_poweroff_ms;
            } else {
                remaining_min_edp_poweroff_time_ms = 0;
            }
        }

        if remaining_min_edp_poweroff_time_ms != 0 {
            msleep(remaining_min_edp_poweroff_time_ms);
        }
    }

    let cntl = bp_transmitter_control {
        action: if power_up { TRANSMITTER_CONTROL_POWER_ON } else { TRANSMITTER_CONTROL_POWER_OFF },
        transmitter: enc.transmitter,
        connector_obj_id: enc.connector,
        coherent: false,
        lanes_number: LANE_COUNT_FOUR,
        hpd_sel: enc.hpd_source,
        ..bp_transmitter_control::default()
    };
    let pwrseq_instance = panel_cntl.pwrseq_inst as u8;
    let link_powered_externally = link.link_powered_externally as u8;

    if dc.ctx.dmub_srv.is_some() && dc.debug.dmub_command_table {
        let action = if power_up { LVTMA_CONTROL_POWER_ON } else { LVTMA_CONTROL_POWER_OFF };
        bios_parser_enable_lvtma_control(adev, dc, action as u8, pwrseq_instance, link_powered_externally);
    }

    let bp_result = bios_parser_transmitter_control(adev, dc, &cntl);

    dp_trace_set_edp_power_timestamp(&mut dc.links[li], power_up);

    if bp_result != bp_result::BP_RESULT_OK {
        Line::new().s("dce110_edp_power_control: Panel Power bp_result: ").dec(bp_result as u64).say();
    }
}

/// `dce110_edp_wait_for_hpd_ready` (dce110_hwseq.c): after powering the
/// panel up, wait for its HPD sense (from KV on HPD does not drop when VCC
/// goes off, so a power-down does not wait).
pub fn dce110_edp_wait_for_hpd_ready(adev: &amdgpu_device, dc: &mut dc, li: usize, power_up: bool) {
    let link = &dc.links[li];
    let Some(connector) = link.link_enc.as_ref().map(|e| e.base().connector) else { return };
    let timeout = if power_up { PANEL_POWER_UP_TIMEOUT } else { PANEL_POWER_DOWN_TIMEOUT };

    if connector.get_connector_id() != CONNECTOR_ID_EDP {
        return;
    }

    if !power_up {
        return;
    }

    // When we power on/off the eDP panel, we need to wait until SENSE bit is high/low.
    let extra_t3_ms = link.panel_config.pps_extra_t3_ms;
    let Some(gs) = dc.ctx.gpio_service.as_deref_mut() else { return };
    let Some(mut hpd) = link_get_hpd_gpio(&dc.ctx.dc_bios, &adev.bios, connector, gs) else { return };

    if extra_t3_ms > 0 {
        msleep(extra_t3_ms as u64);
    }

    dal_gpio_open(adev, gs, &mut hpd, gpio_mode::GPIO_MODE_INTERRUPT);

    // wait until timeout or panel detected
    let mut edp_hpd_high = false;
    let mut time_elapsed = 0;
    loop {
        let mut detected = 0;
        dal_gpio_get_value(adev, &hpd, &mut detected);

        if detected ^ power_up as u32 == 0 {
            edp_hpd_high = true;
            break;
        }

        msleep(HPD_CHECK_INTERVAL as u64);
        time_elapsed += HPD_CHECK_INTERVAL;
        if time_elapsed >= timeout {
            break;
        }
    }

    dal_gpio_close(adev, gs, &mut hpd);

    // ensure that the panel is detected
    if !edp_hpd_high {
        Line::new().s("dce110_edp_wait_for_hpd_ready: wait timed out!").dbg();
    }
}

/// `get_edp_links_with_sink` (dce110_hwseq.c): the eDP links with a sink.
fn get_edp_links_with_sink(dc: &dc) -> alloc::vec::Vec<usize> {
    let mut edp_links_with_sink = alloc::vec::Vec::new();

    // check if there is an eDP panel not in use
    for i in 0..dc.link_count as usize {
        if dc.links[i].local_sink.as_deref().is_some_and(|s| s.sink_signal == LF_SIGNAL_TYPE_EDP) {
            edp_links_with_sink.push(i);
            if edp_links_with_sink.len() == MAX_NUM_EDP as usize {
                break;
            }
        }
    }
    edp_links_with_sink
}

/// `get_edp_streams` (dce110_hwseq.c).
fn get_edp_streams(dc: &dc, context: &crate::dc_state::dc_state) -> alloc::vec::Vec<usize> {
    let mut edp_streams = alloc::vec::Vec::new();

    for i in 0..context.stream_count as usize {
        let s = context.streams[i].unwrap();
        if dc.streams[s].as_deref().unwrap().signal == LF_SIGNAL_TYPE_EDP {
            edp_streams.push(s);
            if edp_streams.len() == MAX_NUM_EDP as usize {
                break;
            }
        }
    }
    edp_streams
}

/// `dce110_edp_backlight_control` (dce110_hwseq.c): the eDP backlight on
/// or off through the VBIOS. An OLED or AUX-controlled backlight goes
/// through the DPCD instead, which joins with those panels.
pub fn dce110_edp_backlight_control(adev: &amdgpu_device, dc: &mut dc, li: usize, enable: bool) {
    let link = &dc.links[li];
    let ext = link.dpcd_sink_ext_caps;
    let oled = crate::dc_dp_types::sink_ext_caps_oled(ext) != 0;
    let hdr_aux = crate::dc_dp_types::bits(ext, 1, 1) != 0;
    let sdr_aux = crate::dc_dp_types::bits(ext, 2, 1) != 0;
    let mut pre_t11_delay = if oled { OLED_PRE_T11_DELAY } else { 0 };
    let post_t7_delay = if oled { OLED_POST_T7_DELAY } else { 0 };
    let enc = *link.link_enc.as_ref().unwrap().base();

    if enc.connector.get_connector_id() != CONNECTOR_ID_EDP {
        return;
    }

    if let Some(panel_cntl) = link.panel_cntl.as_deref()
        && !(oled || hdr_aux || sdr_aux)
    {
        let is_backlight_on = (panel_cntl.funcs().is_panel_backlight_on.unwrap())(adev, panel_cntl);

        if (enable && is_backlight_on) || (!enable && !is_backlight_on) {
            Line::new().s("dce110_edp_backlight_control: panel already powered up/off. Do nothing.").dbg();
            return;
        }
    }

    // Send VBIOS command to control eDP panel backlight
    let cntl = bp_transmitter_control {
        action: if enable { TRANSMITTER_CONTROL_BACKLIGHT_ON } else { TRANSMITTER_CONTROL_BACKLIGHT_OFF },
        transmitter: enc.transmitter,
        connector_obj_id: enc.connector,
        // todo: unhardcode
        lanes_number: LANE_COUNT_FOUR,
        hpd_sel: enc.hpd_source,
        signal: LF_SIGNAL_TYPE_EDP,
        ..bp_transmitter_control::default()
    };

    let pwrseq_instance = link.panel_cntl.as_deref().map_or(0, |p| p.pwrseq_inst as u8);
    let link_powered_externally = link.link_powered_externally as u8;

    if cntl.action == TRANSMITTER_CONTROL_BACKLIGHT_ON {
        if !dc.config.edp_no_power_sequencing {
            Line::new().s("dce110_edp_backlight_control: edp_receiver_ready_T7 not ported").say();
        }
    }

    // Setting link_powered_externally will bypass delays in the backlight
    // as they are not required if the link is being powered by a different
    // source.
    if dc.ctx.dmub_srv.is_some() && dc.debug.dmub_command_table {
        let action = if cntl.action == TRANSMITTER_CONTROL_BACKLIGHT_ON {
            LVTMA_CONTROL_LCD_BLON
        } else {
            LVTMA_CONTROL_LCD_BLOFF
        };
        bios_parser_enable_lvtma_control(adev, dc, action as u8, pwrseq_instance, link_powered_externally);
    }

    bios_parser_transmitter_control(adev, dc, &cntl);

    let link = &dc.links[li];
    if enable && oled && !dc.config.edp_no_power_sequencing
        && !link.local_sink.as_deref().unwrap().edid_caps.panel_patch.oled_optimize_display_on
    {
        msleep((post_t7_delay + link.panel_config.pps_extra_post_t7_ms) as u64);
    }

    if oled || hdr_aux || sdr_aux {
        Line::new().s("dce110_edp_backlight_control: edp_backlight_enable_aux not ported").say();
    }

    // edp 1.2
    if cntl.action == TRANSMITTER_CONTROL_BACKLIGHT_OFF && !dc.config.edp_no_power_sequencing {
        crate::link_dpms::edp_add_delay_for_T9(dc, li);
    }

    if !enable {
        // follow oem panel config's requirement
        pre_t11_delay += dc.links[li].panel_config.pps_extra_pre_t11_ms;
        if pre_t11_delay != 0 {
            msleep(pre_t11_delay as u64);
        }
    }
}

/// `power_down_encoders` (dce110_hwseq.c).
fn power_down_encoders(adev: &amdgpu_device, dc: &mut dc) {
    for i in 0..dc.link_count as usize {
        let mut signal = dc.links[i].connector_signal;

        crate::link_dpms::link_blank_dp_stream(adev, dc, i, false);
        if signal != LF_SIGNAL_TYPE_EDP {
            signal = LF_SIGNAL_TYPE_NONE;
        }

        // link_enc_cfg_get_link_enc: DIG encoders are mapped fixed on Renoir.
        let link = &mut dc.links[i];
        if link.ep_type == LF_DISPLAY_ENDPOINT_PHY && let Some(enc) = link.link_enc.as_ref() {
            (enc.funcs().disable_output.unwrap())(adev, dc, i, signal);
        }

        let link = &mut dc.links[i];
        if link.fec_state == dc_link_fec_enabled {
            if let Some(enc) = link.link_enc.as_ref()
                && let (Some(set_enable), Some(set_ready)) = (enc.funcs().fec_set_enable, enc.funcs().fec_set_ready)
            {
                set_enable(adev, enc, false);
                set_ready(adev, enc, false);
                link.fec_state = dc_link_fec_not_ready;
            }
        }

        link.link_status.link_active = false;
        link.cur_link_settings = Default::default();
    }
}

/// `power_down_controllers` (dce110_hwseq.c).
fn power_down_controllers(adev: &amdgpu_device, dc: &mut dc) {
    let pool = dc.res_pool.as_deref().unwrap();
    for i in 0..pool.timing_generator_count as usize {
        let tg = pool.timing_generators[i].as_deref().unwrap();
        (tg.funcs().disable_crtc.unwrap())(adev, tg);
    }
}

/// `power_down_clock_sources` (dce110_hwseq.c).
fn power_down_clock_sources(adev: &amdgpu_device, dc: &mut dc) {
    use crate::dce_clock_source::dce110_clock_source_power_down;

    if !dce110_clock_source_power_down(adev, dc, crate::dc_state::clock_source_ref::Dp) {
        Line::new().s("Failed to power down pll! (dp clk src)").say();
    }

    let clk_src_count = dc.res_pool.as_deref().unwrap().clk_src_count as usize;
    for i in 0..clk_src_count {
        if !dce110_clock_source_power_down(adev, dc, crate::dc_state::clock_source_ref::Pll(i)) {
            Line::new().s("Failed to power down pll! (clk src index=").dec(i as u64).s(")").say();
        }
    }
}

/// `power_down_all_hw_blocks` (dce110_hwseq.c). DCN 2.1 has no FBC
/// compressor.
fn power_down_all_hw_blocks(adev: &amdgpu_device, dc: &mut dc) {
    power_down_encoders(adev, dc);

    power_down_controllers(adev, dc);

    power_down_clock_sources(adev, dc);
}

/// `disable_vga_and_power_gate_all_controllers` (dce110_hwseq.c).
/// dcn20_tg_funcs has no `disable_vga`, and
/// `enable_display_pipe_clock_gating` is empty.
fn disable_vga_and_power_gate_all_controllers(adev: &amdgpu_device, dc: &mut dc) {
    if dc.caps.ips_support {
        return;
    }

    let pipe_count = dc.res_pool.as_deref().unwrap().pipe_count as usize;
    for i in 0..pipe_count {
        // Enable CLOCK gating for each pipe BEFORE controller powergating.
        dc.current_state.as_deref_mut().unwrap().res_ctx.pipe_ctx[i].pipe_idx = i as u8;
        (dc.hwss.disable_plane.unwrap())(adev, dc, i);
    }
}

/// `dce110_enable_accelerated_mode` (dce110_hwseq.c): take the display
/// over from the VBIOS. eDP fast boot keeps a lit panel whose timing the
/// new state repeats; otherwise the backlight goes off and every block is
/// powered down.
pub fn dce110_enable_accelerated_mode(adev: &amdgpu_device, dc: &mut dc, context: &mut crate::dc_state::dc_state) {
    let mut can_apply_edp_fast_boot = false;
    let mut can_apply_seamless_boot = false;
    let mut keep_edp_vdd_on = false;

    let edp_links_with_sink = get_edp_links_with_sink(dc);
    let edp_links = crate::dc::dc_get_edp_links(dc);

    if let Some(init_pipes) = dc.hwseq.as_deref().unwrap().funcs.init_pipes {
        init_pipes(adev, dc, Some(context));
    }

    let edp_streams = get_edp_streams(dc, context);

    // Check fastboot support, disable on DCE 6-8-10 because of blank screens
    if !edp_links.is_empty() && !edp_streams.is_empty() {
        let edp_stream = edp_streams[0];
        for &edp_link in &edp_links {
            let s = dc.streams[edp_stream].as_deref().unwrap();
            if edp_link != s.link {
                continue;
            }
            // enable fastboot if backend is enabled on eDP
            let enc = dc.links[edp_link].link_enc.as_ref().unwrap();
            if enc.funcs().is_dig_enabled.is_some_and(|f| f(adev, enc)) && dc.links[edp_link].link_status.link_active {
                let sink_signal = s.sink.as_deref().unwrap().sink_signal;
                let (sl, timing) = (s.link, s.timing);
                can_apply_edp_fast_boot = crate::dc::dc_validate_boot_timing(adev, dc, sl, sink_signal, &timing);

                // For Mux-platform, the default value is false. Disable
                // fast boot during mux switching; is_switch_in_progress_dest
                // is not set on this platform.

                dc.streams[edp_stream].as_deref_mut().unwrap().apply_edp_fast_boot_optimization =
                    can_apply_edp_fast_boot;
                if can_apply_edp_fast_boot {
                    // is_dp_dig_pixel_rate_div_policy is not a member of
                    // dcn21_private_funcs.
                }
                break;
            }
        }
        // We are trying to enable eDP, don't power down VDD
        if can_apply_edp_fast_boot {
            keep_edp_vdd_on = true;
        }
    }

    // Check seamless boot support
    for i in 0..context.stream_count as usize {
        if dc.streams[context.streams[i].unwrap()].as_deref().unwrap().apply_seamless_boot_optimization {
            can_apply_seamless_boot = true;
            break;
        }
    }

    // eDP should not have stream in resume from S4 and so even with VBios
    // post it should get turned off
    let edp_link_with_sink = edp_links_with_sink.first().copied();

    if !can_apply_edp_fast_boot && !can_apply_seamless_boot {
        if let Some(l) = edp_link_with_sink
            && !keep_edp_vdd_on
        {
            // turn off backlight before DP_blank and encoder powered down
            (dc.hwseq.as_deref().unwrap().funcs.edp_backlight_control.unwrap())(adev, dc, l, false);
        }
        // resume from S3, no vbios posting, no need to power down again
        if !crate::bios_parser2::bios_parser_is_accelerated_mode(adev, &dc.ctx.dc_bios.base) {
            crate::clk_mgr::clk_mgr_exit_optimized_pwr_state(adev, dc);
        }

        power_down_all_hw_blocks(adev, dc);

        // DSC could be enabled on eDP during VBIOS post. To clean up dsc
        // blocks if eDP is in link but not active.
        if edp_link_with_sink.is_some() && edp_streams.is_empty() {
            Line::new().s("dce110_enable_accelerated_mode: clean_up_dsc_blocks not ported").say();
        }

        disable_vga_and_power_gate_all_controllers(adev, dc);
        if let Some(l) = edp_link_with_sink
            && !keep_edp_vdd_on
        {
            (dc.hwss.edp_power_control.unwrap())(adev, dc, l, false);
        }
        if !crate::bios_parser2::bios_parser_is_accelerated_mode(adev, &dc.ctx.dc_bios.base) {
            crate::clk_mgr::clk_mgr_optimize_pwr_state(adev, dc);
        }
    }
    crate::bios_parser2::bios_set_scratch_acc_mode_change(adev, &dc.ctx.dc_bios.base, 1);
}

/// `hwss_wait_for_all_blank_complete` (dc_hw_sequencer.c).
pub fn hwss_wait_for_all_blank_complete(adev: &amdgpu_device, dc: &dc, context: &crate::dc_state::dc_state) {
    use crate::dc_resource::{pipe_type, resource_is_pipe_type};
    let Some(wait_for_blank_complete) = dc.hwseq.as_deref().unwrap().funcs.wait_for_blank_complete else { return };

    for i in 0..MAX_PIPES {
        let opp_head = &context.res_ctx.pipe_ctx[i];

        if !resource_is_pipe_type(opp_head, pipe_type::OPP_HEAD) {
            continue;
        }

        let opp = dc.res_pool.as_deref().unwrap().opps[opp_head.stream_res.opp.unwrap()].as_deref().unwrap();
        wait_for_blank_complete(adev, opp);
    }
}

/// `hwss_wait_for_odm_update_pending_complete` (dc_hw_sequencer.c).
/// dcn20_tg_funcs has neither `wait_odm_doublebuffer_pending_clear` nor
/// `wait_otg_disable`.
pub fn hwss_wait_for_odm_update_pending_complete(adev: &amdgpu_device, dc: &dc, context: &crate::dc_state::dc_state) {
    // ODM update may require to reprogram blank pattern for each OPP
    hwss_wait_for_all_blank_complete(adev, dc, context);
}

/// `hwss_wait_for_no_pipes_pending` (dc_hw_sequencer.c): wait up to 100 ms
/// for each plane's flip to land.
pub fn hwss_wait_for_no_pipes_pending(adev: &amdgpu_device, dc: &mut dc, context: &crate::dc_state::dc_state) {
    for i in 0..MAX_PIPES {
        let pipe = context.res_ctx.pipe_ctx[i];
        let Some(plane) = pipe.plane_state else { continue };

        // Timeout 100 ms
        let mut count = 0;
        while count < 100000 {
            // Must set to false to start with, due to OR in update function
            dc.planes[plane].as_deref_mut().unwrap().status.is_flip_pending = false;
            (dc.hwss.update_pending_status.unwrap())(adev, dc, &pipe);
            if !dc.planes[plane].as_deref().unwrap().status.is_flip_pending {
                break;
            }
            crate::linux::udelay(1);
            count += 1;
        }
    }
}

/// `resource_is_hpo_acquired` (dc_resource.c): DCN 2.1 has no HPO DP
/// encoders.
fn resource_is_hpo_acquired(_context: &dc_state) -> bool {
    false
}

/// `bios_set_scratch_critical_state` (bios_parser_helper.c).
fn bios_set_scratch_critical_state(adev: &amdgpu_device, dc: &dc, state: bool) {
    let regs = dc.ctx.dc_bios.base.regs.expect("bios regs");
    crate::dc_helper::generic_reg_update_ex(adev, regs.BIOS_SCRATCH_6,
        &[(ATOM_S6_CRITICAL_STATE_SHIFT as u8, ATOM_S6_CRITICAL_STATE, state as u32)]);
}

/// `dce110_setup_audio_dto` (dce110_hwseq.c): the audio wall clock follows
/// the first pipe with HDMI audio, else the first with DP audio. Building
/// the audio output joins with audio.
fn dce110_setup_audio_dto(dc: &dc, context: &dc_state) {
    let pipe_count = dc.res_pool.as_deref().unwrap().pipe_count as usize;
    let mut i = 0;

    while i < pipe_count {
        let pipe_ctx = &context.res_ctx.pipe_ctx[i];
        i += 1;
        let Some(s) = pipe_ctx.stream else { continue };

        if pipe_ctx.top_pipe.is_some() {
            continue;
        }
        if dc.streams[s].as_deref().unwrap().signal != LF_SIGNAL_TYPE_HDMI_TYPE_A {
            continue;
        }
        if pipe_ctx.stream_res.audio.is_some() {
            Line::new().s("dce110_setup_audio_dto: build_audio_output, wall_dto_setup not ported").say();
            return;
        }
    }

    // no HDMI audio is found, try DP audio
    for i in 0..pipe_count {
        let pipe_ctx = &context.res_ctx.pipe_ctx[i];
        let Some(s) = pipe_ctx.stream else { continue };

        if pipe_ctx.top_pipe.is_some() {
            continue;
        }

        if !crate::link_detection::dc_is_dp_signal(dc.streams[s].as_deref().unwrap().signal) {
            continue;
        }

        if pipe_ctx.stream_res.audio.is_some() {
            Line::new().s("dce110_setup_audio_dto: build_audio_output, wall_dto_setup not ported").say();
            return;
        }
    }
}

/// `dce110_apply_single_controller_ctx_to_hw` (dce110_hwseq.c): the back
/// end of one OTG master: format, timing, DRR, the DIG front end on the
/// OTG, then the link. The audio output and the DSC-ahead workaround of
/// SmartMux are later stages.
fn dce110_apply_single_controller_ctx_to_hw(adev: &amdgpu_device, dc: &mut dc, context: &mut dc_state, p: usize)
                                            -> crate::dc_types::dc_status {
    let funcs = dc.hwseq.as_deref().unwrap().funcs;
    let pipe = context.res_ctx.pipe_ctx[p];
    let s = pipe.stream.unwrap();
    let li = dc.streams[s].as_deref().unwrap().link;
    let mut event_triggers = 0;
    let _link_hwss = crate::link_hwss::get_link_hwss(dc, li, &pipe.link_res);

    if let Some(f) = funcs.disable_stream_gating {
        f(adev, dc, context, p);
    }

    if pipe.stream_res.audio.is_some() {
        Line::new().s("dce110_apply_single_controller_ctx_to_hw: audio output not ported").say();
    }

    // make sure no pipes syncd to the pipe being enabled:
    // use_pipe_ctx_sync_logic is a DCN 3.x config.

    let stream = dc.streams[s].as_deref().unwrap();
    {
        let pool = dc.res_pool.as_deref().unwrap();
        let opp = pool.opps[pipe.stream_res.opp.unwrap()].as_deref().unwrap();
        (opp.funcs().opp_program_fmt.unwrap())(adev, &dc.debug, opp, &stream.bit_depth_params, &stream.clamping);

        (opp.funcs().opp_set_dyn_expansion.unwrap())(adev, opp, COLOR_SPACE_YCBCR601,
            stream.timing.display_color_depth, stream.signal);

        let mut odm_pipe = pipe.next_odm_pipe;
        while let Some(o) = odm_pipe {
            let opp = pool.opps[context.res_ctx.pipe_ctx[o].stream_res.opp.unwrap()].as_deref().unwrap();
            (opp.funcs().opp_set_dyn_expansion.unwrap())(adev, opp, COLOR_SPACE_YCBCR601,
                stream.timing.display_color_depth, stream.signal);

            (opp.funcs().opp_program_fmt.unwrap())(adev, &dc.debug, opp, &stream.bit_depth_params,
                &stream.clamping);
            odm_pipe = context.res_ctx.pipe_ctx[o].next_odm_pipe;
        }
    }

    // DCN3.1 FPGA Workaround: hws->wa.dp_hpo_and_otg_sequence is not set
    // on DCN 2.1. Do not touch stream timing on seamless boot optimization.
    if !dc.streams[s].as_deref().unwrap().apply_seamless_boot_optimization {
        (funcs.enable_stream_timing.unwrap())(adev, dc, context, p);
    }

    if let Some(f) = funcs.setup_vupdate_interrupt {
        f(adev, dc, context, p);
    }

    let stream = dc.streams[s].as_deref().unwrap();
    let params = crate::dcn20_optc::drr_params {
        vertical_total_min: stream.adjust.v_total_min,
        vertical_total_max: stream.adjust.v_total_max,
        ..Default::default()
    };
    set_drr_and_clear_adjust_pending(adev, dc, &context.res_ctx.pipe_ctx[p], Some(s), Some(&params));

    // DRR should set trigger event to monitor surface update event
    let stream = dc.streams[s].as_deref().unwrap();
    if stream.adjust.v_total_min != 0 && stream.adjust.v_total_max != 0 {
        event_triggers = 0x80;
    }
    // Event triggers and num frames initialized for DRR, but can be later
    // updated for PSR use. Note DRR trigger events are generated regardless
    // of whether num frames met.
    let pool = dc.res_pool.as_deref().unwrap();
    let tg = pool.timing_generators[pipe.stream_res.tg.unwrap()].as_deref().unwrap();
    if let Some(f) = tg.funcs().set_static_screen_control {
        f(adev, tg, event_triggers, 2);
    }

    let signal = stream.signal;
    if !crate::link_detection::dc_is_virtual_signal(signal) {
        let se = pool.stream_enc[pipe.stream_res.stream_enc.unwrap()].as_ref().unwrap();
        (se.funcs().dig_connect_to_otg.unwrap())(adev, se, tg.base.inst as i32);
    }

    if crate::link_detection::dc_is_dp_signal(signal) {
        crate::link_dpms::dp_trace_source_sequence(adev, dc, li, DPCD_SOURCE_SEQ_AFTER_CONNECT_DIG_FE_OTG);
    }

    // Temporary workaround to perform DSC programming ahead of stream
    // enablement for smartmux/SPRS
    if dc.streams[s].as_deref().unwrap().timing.flags.DSC {
        Line::new().s("dce110_apply_single_controller_ctx_to_hw: DSC not ported").say();
    }

    if !dc.streams[s].as_deref().unwrap().dpms_off {
        crate::link_dpms::link_set_dpms_on(adev, dc, context, p);
    }

    // hws->wa.dp_hpo_and_otg_sequence is not set on DCN 2.1.

    context.res_ctx.pipe_ctx[p].plane_res.scl_data.lb_params.alpha_en = context.res_ctx.pipe_ctx[p].bottom_pipe.is_some();

    // Phantom and main stream share the same link (because the stream is
    // constructed with the same sink). Make sure not to override and link
    // programming on the main.
    dc.links[li].psr_settings.psr_feature_enabled = false;
    dc.links[li].replay_settings.replay_feature_enabled = false;

    crate::dc_types::dc_status::DC_OK
}

/// `dce110_apply_ctx_to_hw` (dce110_hwseq.c): reset what the new state
/// drops, then bring up every OTG master it adds. FBC and the HPO DP
/// control are not DCN 2.1's.
pub fn dce110_apply_ctx_to_hw(adev: &amdgpu_device, dc: &mut dc, context: &mut dc_state) -> crate::dc_types::dc_status {
    let funcs = dc.hwseq.as_deref().unwrap().funcs;
    let _was_hpo_acquired = resource_is_hpo_acquired(dc.current_state.as_deref().unwrap());
    let _is_hpo_acquired = resource_is_hpo_acquired(context);
    let pipe_count = dc.res_pool.as_deref().unwrap().pipe_count as usize;

    // reset syncd pipes from disabled pipes: use_pipe_ctx_sync_logic is off.

    // Reset old context: look up the targets that have been removed since
    // last commit
    (funcs.reset_hw_ctx_wrap.unwrap())(adev, dc, context);

    // Skip applying if no targets
    if context.stream_count == 0 {
        return crate::dc_types::dc_status::DC_OK;
    }

    // Apply new context
    bios_set_scratch_critical_state(adev, dc, true);

    // below is for real asic only
    for i in 0..pipe_count {
        let pipe_ctx_old = dc.current_state.as_deref().unwrap().res_ctx.pipe_ctx[i];
        let pipe_ctx = context.res_ctx.pipe_ctx[i];

        if pipe_ctx.stream.is_none() || pipe_ctx.top_pipe.is_some() {
            continue;
        }

        if pipe_ctx.stream == pipe_ctx_old.stream {
            if pipe_ctx_old.clock_source != pipe_ctx.clock_source {
                Line::new().s("dce110_apply_ctx_to_hw: dce_crtc_switch_to_clk_src not ported").say();
            }
            continue;
        }

        (funcs.enable_display_power_gating.unwrap())(adev, dc, i as u8, PIPE_GATING_CONTROL_DISABLE);
    }

    dce110_setup_audio_dto(dc, context);

    for i in 0..pipe_count {
        let pipe_ctx_old = dc.current_state.as_deref().unwrap().res_ctx.pipe_ctx[i];
        let pipe_ctx = context.res_ctx.pipe_ctx[i];

        let Some(s) = pipe_ctx.stream else { continue };

        if pipe_ctx.stream == pipe_ctx_old.stream
            && dc.links[dc.streams[s].as_deref().unwrap().link].link_state_valid
        {
            continue;
        }

        if pipe_ctx_old.stream.is_some() {
            Line::new().s("dce110_apply_ctx_to_hw: pipe_need_reprogram not ported").say();
        }

        if pipe_ctx.top_pipe.is_some() || pipe_ctx.prev_odm_pipe.is_some() {
            continue;
        }

        let status = dce110_apply_single_controller_ctx_to_hw(adev, dc, context, i);

        if status != crate::dc_types::dc_status::DC_OK {
            return status;
        }

        // resync_fifo_dccg_dio is not a member of dcn21_private_funcs.
    }

    bios_set_scratch_critical_state(adev, dc, false);

    crate::dc_types::dc_status::DC_OK
}

/// `set_drr_and_clear_adjust_pending` (dc_hw_sequencer.c).
pub fn set_drr_and_clear_adjust_pending(adev: &amdgpu_device, dc: &mut dc, pipe_ctx: &crate::dc_state::pipe_ctx,
                                        stream: Option<usize>, params: Option<&crate::dcn20_optc::drr_params>) {
    // params can be null.
    if let Some(tg) = pipe_ctx.stream_res.tg {
        let tg = dc.res_pool.as_deref().unwrap().timing_generators[tg].as_deref().unwrap();
        if let Some(f) = tg.funcs().set_drr {
            f(adev, tg, params);
        }
    }

    if let Some(s) = stream {
        dc.streams[s].as_deref_mut().unwrap().adjust.timing_adjust_pending = false;
    }
}

/// `dcn10_dummy_display_power_gating` (dcn10_hwseq.c).
pub fn dcn10_dummy_display_power_gating(_adev: &amdgpu_device, _dc: &mut dc, _controller_id: u8, _power_gating: u32)
                                        -> bool {
    true
}

/// `dce110_update_info_frame` (dce110_hwseq.c): the pipe's info packets to
/// its stream encoder. A pipe without one is not a root pipe.
pub fn dce110_update_info_frame(adev: &amdgpu_device, dc: &mut dc, context: &mut dc_state, p: usize) {
    let pipe_ctx = &context.res_ctx.pipe_ctx[p];
    let Some(se) = pipe_ctx.stream_res.stream_enc else { return };
    let signal = dc.streams[pipe_ctx.stream.unwrap()].as_deref().unwrap().signal;

    let is_hdmi_tmds = crate::link_detection::dc_is_hdmi_tmds_signal(signal);
    let is_dp = crate::link_detection::dc_is_dp_signal(signal);

    if !is_hdmi_tmds && !is_dp {
        return;
    }

    let se = dc.res_pool.as_deref().unwrap().stream_enc[se].as_ref().unwrap();
    if is_hdmi_tmds {
        Line::new().s("dce110_update_info_frame: update_hdmi_info_packets not ported").say();
    } else {
        if let Some(f) = se.funcs().update_dp_info_packets_sdp_line_num {
            f(adev, se, &pipe_ctx.stream_res.encoder_info_frame);
        }

        (se.funcs().update_dp_info_packets.unwrap())(adev, se, &pipe_ctx.stream_res.encoder_info_frame);
    }
}

/// `dce110_enable_audio_stream` (dce110_hwseq.c): tell the audio endpoint
/// the monitor's modes. Enabling it joins with audio.
pub fn dce110_enable_audio_stream(_adev: &amdgpu_device, _dc: &mut dc, context: &mut dc_state, p: usize) {
    let pipe_ctx = &context.res_ctx.pipe_ctx[p];
    if pipe_ctx.stream.is_none() {
        return;
    }

    if pipe_ctx.stream_res.audio.is_some() {
        Line::new().s("dce110_enable_audio_stream: az_enable, enable_audio_packet not ported").say();
    }
}

/// `dce110_enable_dp_link_output` (dce110_hwseq.c): with the eDP's HPD up,
/// a pipe of the link whose pixel clock does not come from the DP DTO (a
/// HDMI passive dongle swapped for DP) moves to it, the PHY clock follows
/// the link rate, and the link's encoder takes the PHY. DCN 2.1 has no
/// DMCU to lock the PHY.
pub fn dce110_enable_dp_link_output(adev: &amdgpu_device, dc: &mut dc, li: usize,
                                    link_res: &crate::dc_state::link_resource, signal: u32, clock_source: u32,
                                    link_settings: &crate::dc_dp_types::dc_link_settings) {
    let link_hwss = crate::link_hwss::get_link_hwss(dc, li, link_res);

    // Extracting the power-up and power-down sequences from enable/disable
    // link output, so the eDP panel control runs once in enable_link_dp and
    // disable_link_dp, is a TODO in Linux.
    if dc.links[li].connector_signal == LF_SIGNAL_TYPE_EDP {
        (dc.hwss.edp_wait_for_hpd_ready.unwrap())(adev, dc, li, true);
    }

    // If the current pixel clock source is not DTO(happens after switching
    // from HDMI passive dongle to DP on the same connector), switch the
    // pixel clock source to DTO.
    for i in 0..MAX_PIPES {
        let pipe = dc.current_state.as_deref().unwrap().res_ctx.pipe_ctx[i];
        let Some(s) = pipe.stream else { continue };
        if dc.streams[s].as_deref().unwrap().link != li {
            continue;
        }
        if let Some(cs) = pipe.clock_source {
            let id = crate::dc_resource::clock_source_of(dc.res_pool.as_deref().unwrap(), cs).base.id;
            if id != crate::dce_clock_source::clock_source_id::CLOCK_SOURCE_ID_DP_DTO {
                Line::new().s("dce110_enable_dp_link_output: switch to the DP DTO not ported").say();
            }
        }
    }

    if crate::link_dp_capability::link_dp_get_encoding_format(link_settings)
        == crate::dc_dp_types::dp_link_encoding::DP_8b_10b_ENCODING
    {
        let mut clk_mgr = dc.clk_mgr.take().unwrap();
        if let Some(f) = clk_mgr.funcs().notify_link_rate_change {
            f(adev, dc, &mut clk_mgr, li);
        }
        dc.clk_mgr = Some(clk_mgr);
    }

    crate::link_hwss::enable_dp_link_output(adev, dc, li, link_hwss, signal, clock_source, link_settings);

    dc.links[li].phy_state.symclk_state = SYMCLK_ON_TX_ON;

    crate::link_dpms::dp_trace_source_sequence(adev, dc, li, DPCD_SOURCE_SEQ_AFTER_ENABLE_LINK_PHY);
}

/// `dce110_disable_link_output` (dce110_hwseq.c): an eDP's backlight goes
/// off first. DCN 2.1 has no DMCU to lock the PHY.
pub fn dce110_disable_link_output(adev: &amdgpu_device, dc: &mut dc, li: usize,
                                  link_res: &crate::dc_state::link_resource, signal: u32) {
    let link_hwss = crate::link_hwss::get_link_hwss(dc, li, link_res);

    if signal == LF_SIGNAL_TYPE_EDP && dc.hwss.edp_backlight_control.is_some()
        && !dc.links[li].skip_implict_edp_power_control
    {
        (dc.hwss.edp_backlight_control.unwrap())(adev, dc, li, false);
    }

    crate::link_hwss::disable_link_output(adev, dc, li, link_hwss, signal);
    dc.links[li].phy_state.symclk_state = SYMCLK_OFF_TX_OFF;
    // Extracting the power-up and power-down sequences from enable/disable
    // link output, so the eDP panel control runs once in enable_link_dp and
    // disable_link_dp, is a TODO in Linux.
    crate::link_dpms::dp_trace_source_sequence(adev, dc, li, DPCD_SOURCE_SEQ_AFTER_DISABLE_LINK_PHY);
}
