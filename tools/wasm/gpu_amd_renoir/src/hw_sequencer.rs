//! display/dc/inc/hw_sequencer.h + hwss/dcn21/dcn21_init.c +
//! hwss/dce110/dce110_hwseq.c (Linux 6.18.55): the hw sequencer vtable DCN
//! 2.1 binds, grow-as-reached, and the eDP panel power and HPD waits of
//! dce110 it uses. A link is named by its index in `dc.links`.
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
/// the port has not reached.
#[derive(Default, Clone, Copy)]
pub struct hw_sequencer_funcs {
    pub edp_power_control: Option<fn(&amdgpu_device, &mut dc, usize, bool)>,
    pub edp_wait_for_hpd_ready: Option<fn(&amdgpu_device, &mut dc, usize, bool)>,
}

/// `dcn21_funcs` (dcn21_init.c), as far as ported.
const DCN21_FUNCS: hw_sequencer_funcs = hw_sequencer_funcs {
    edp_power_control: Some(dce110_edp_power_control),
    edp_wait_for_hpd_ready: Some(dce110_edp_wait_for_hpd_ready),
};

/// `dcn21_hw_sequencer_construct` (dcn21_init.c). `dcn21_private_funcs`
/// join `dc.hwseq` with their first caller.
pub fn dcn21_hw_sequencer_construct(dc: &mut dc) {
    dc.hwss = DCN21_FUNCS;
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
