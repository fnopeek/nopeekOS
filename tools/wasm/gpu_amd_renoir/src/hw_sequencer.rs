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
    pub disable_plane: Option<fn(&amdgpu_device, &mut dc, usize)>,
    pub wait_for_mpcc_disconnect: Option<fn(&amdgpu_device, &mut dc, usize)>,
    pub set_flip_control_gsl: Option<fn(&amdgpu_device, &dc, usize, bool)>,
    pub edp_power_control: Option<fn(&amdgpu_device, &mut dc, usize, bool)>,
    pub edp_wait_for_hpd_ready: Option<fn(&amdgpu_device, &mut dc, usize, bool)>,
}

/// `dcn21_funcs` (dcn21_init.c), as far as ported.
const DCN21_FUNCS: hw_sequencer_funcs = hw_sequencer_funcs {
    init_hw: Some(crate::dcn10_hwseq::dcn10_init_hw),
    disable_plane: Some(crate::dcn20_hwseq::dcn20_disable_plane),
    wait_for_mpcc_disconnect: Some(crate::dcn10_hwseq::dcn10_wait_for_mpcc_disconnect),
    set_flip_control_gsl: Some(crate::dcn20_hwseq::dcn20_set_flip_control_gsl),
    edp_power_control: Some(dce110_edp_power_control),
    edp_wait_for_hpd_ready: Some(dce110_edp_wait_for_hpd_ready),
};

/// `struct hwseq_private_funcs` (hw_sequencer_private.h), the members whose
/// bodies are ported. The `None` members of `DCN21_PRIVATE_FUNCS` are
/// members dcn21 leaves out.
#[derive(Default, Clone, Copy)]
pub struct hwseq_private_funcs {
    pub init_pipes: Option<fn(&amdgpu_device, &mut dc)>,
    pub plane_atomic_disconnect: Option<fn(&amdgpu_device, &mut dc, usize)>,
    pub init_blank: Option<fn(&amdgpu_device, &dc, usize)>,
    pub disable_vga: Option<fn(&amdgpu_device, &crate::dce_hwseq::dce_hwseq)>,
    pub bios_golden_init: Option<fn(&amdgpu_device, &mut dc)>,
    pub plane_atomic_disable: Option<fn(&amdgpu_device, &mut dc, usize)>,
    pub plane_atomic_power_down: Option<fn(&amdgpu_device, &mut dc, usize, usize)>,
    pub enable_power_gating_plane: Option<fn(&amdgpu_device, &crate::dce_hwseq::dce_hwseq, bool)>,
    pub dpp_pg_control: Option<fn(&amdgpu_device, &dc, u32, bool)>,
    pub hubp_pg_control: Option<fn(&amdgpu_device, &dc, u32, bool)>,
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
    init_blank: Some(crate::dcn20_hwseq::dcn20_init_blank),
    disable_vga: Some(crate::dcn20_hwseq::dcn20_disable_vga),
    bios_golden_init: Some(crate::dcn10_hwseq::dcn10_bios_golden_init),
    plane_atomic_disable: Some(crate::dcn20_hwseq::dcn20_plane_atomic_disable),
    plane_atomic_power_down: Some(crate::dcn10_hwseq::dcn10_plane_atomic_power_down),
    enable_power_gating_plane: Some(crate::dcn20_hwseq::dcn20_enable_power_gating_plane),
    dpp_pg_control: Some(crate::dcn20_hwseq::dcn20_dpp_pg_control),
    hubp_pg_control: Some(crate::dcn20_hwseq::dcn20_hubp_pg_control),
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
        if link.has_local_sink {
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
