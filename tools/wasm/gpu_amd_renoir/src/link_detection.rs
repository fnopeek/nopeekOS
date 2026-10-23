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

/// `enum dc_detect_reason`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum dc_detect_reason {
    DETECT_REASON_BOOT,
    DETECT_REASON_RESUMEFROMS3S4,
    DETECT_REASON_HPD,
    DETECT_REASON_HPDRX,
    DETECT_REASON_FALLBACK,
    DETECT_REASON_RETRAIN,
    DETECT_REASON_UNKNOWN,
}

/// `dc_is_*_signal` (signal_types.h).
pub fn dc_is_hdmi_signal(signal: u32) -> bool {
    signal == LF_SIGNAL_TYPE_HDMI_TYPE_A
}

pub fn dc_is_dp_sst_signal(signal: u32) -> bool {
    signal == LF_SIGNAL_TYPE_DISPLAY_PORT || signal == LF_SIGNAL_TYPE_EDP
}

pub fn dc_is_dp_signal(signal: u32) -> bool {
    signal == LF_SIGNAL_TYPE_DISPLAY_PORT || signal == LF_SIGNAL_TYPE_EDP || signal == LF_SIGNAL_TYPE_DISPLAY_PORT_MST
}

pub fn dc_is_embedded_signal(signal: u32) -> bool {
    signal == LF_SIGNAL_TYPE_EDP || signal == LF_SIGNAL_TYPE_LVDS
}

pub fn dc_is_dvi_signal(signal: u32) -> bool {
    signal == LF_SIGNAL_TYPE_DVI_SINGLE_LINK || signal == LF_SIGNAL_TYPE_DVI_DUAL_LINK
}

pub fn dc_is_virtual_signal(signal: u32) -> bool {
    signal == LF_SIGNAL_TYPE_VIRTUAL
}

/// `link_disconnect_sink`.
fn link_disconnect_sink(link: &mut crate::link_factory::dc_link) {
    // dc_sink_release
    link.local_sink = None;

    link.dpcd_sink_count = 0;
}

/// `link_disconnect_remap`.
fn link_disconnect_remap(prev_sink: Option<alloc::boxed::Box<crate::dc_sink::dc_sink>>,
                         link: &mut crate::link_factory::dc_link) {
    link.local_sink = prev_sink;
}

/// `is_same_edid`.
fn is_same_edid(old_edid: &crate::dc_sink::dc_edid, new_edid: &crate::dc_sink::dc_edid) -> bool {
    if new_edid.length == 0 {
        return false;
    }

    if new_edid.length != old_edid.length {
        return false;
    }

    old_edid.raw_edid[..old_edid.length as usize] == new_edid.raw_edid[..new_edid.length as usize]
}

/// `read_current_link_settings_on_detect`.
fn read_current_link_settings_on_detect(adev: &amdgpu_device, dc: &mut dc, li: usize) {
    use crate::dc_dp_types::*;
    use crate::dc_types::dc_status;
    use crate::link_dpcd::core_link_read_dpcd;
    let mut lane_count_set = [0u8; 1];
    let mut link_bw_set = [0u8; 1];
    let mut link_rate_set = [0u8; 1];
    let read_dpcd_retry_cnt = 10;
    let mut max_down_spread = [0u8; 1];

    // Read DPCD 00101h to find out the number of lanes currently set
    for _ in 0..read_dpcd_retry_cnt {
        let status = core_link_read_dpcd(adev, dc, li, DP_LANE_COUNT_SET, &mut lane_count_set);
        // First DPCD read after VDD ON can fail if the particular board
        // does not have HPD pin wired correctly. So if DPCD read fails,
        // which it should never happen, retry a few times. Target worst
        // case scenario of 80 ms.
        if status == dc_status::DC_OK {
            dc.links[li].cur_link_settings.lane_count = lane_count_set_LANE_COUNT_SET(lane_count_set[0]) as u32;
            break;
        }

        crate::linux::msleep(8);
    }

    // Read DPCD 00100h to find if standard link rates are set
    core_link_read_dpcd(adev, dc, li, DP_LINK_BW_SET, &mut link_bw_set);

    if link_bw_set[0] == 0 {
        if dc.links[li].connector_signal == LF_SIGNAL_TYPE_EDP {
            // If standard link rates are not being used,
            // Read DPCD 00115h to find the edp link rate set used
            core_link_read_dpcd(adev, dc, li, DP_LINK_RATE_SET, &mut link_rate_set);

            // edp_supported_link_rates_count = 0 for DP
            let link = &mut dc.links[li];
            if link_rate_set[0] < link.dpcd_caps.edp_supported_link_rates_count {
                link.cur_link_settings.link_rate = link.dpcd_caps.edp_supported_link_rates[link_rate_set[0] as usize];
                link.cur_link_settings.link_rate_set = link_rate_set[0];
                link.cur_link_settings.use_link_rate_set = true;
            }
        }
        // else: Link Rate not found. Seamless boot may not work.
    } else {
        let link = &mut dc.links[li];
        link.cur_link_settings.link_rate = link_bw_set[0] as u32;
        link.cur_link_settings.use_link_rate_set = false;
    }
    // Read DPCD 00003h to find the max down spread.
    core_link_read_dpcd(adev, dc, li, DP_MAX_DOWNSPREAD, &mut max_down_spread);
    dc.links[li].cur_link_settings.link_spread = if max_down_spread_MAX_DOWN_SPREAD(max_down_spread[0]) != 0 {
        LINK_SPREAD_05_DOWNSPREAD_30KHZ
    } else {
        LINK_SPREAD_DISABLED
    };
}

/// `verify_link_capability_non_destructive`.
fn verify_link_capability_non_destructive(adev: &amdgpu_device, dc: &mut dc, li: usize) {
    let signal = dc.links[li].local_sink.as_deref().unwrap().sink_signal;
    if dc_is_dp_signal(signal) {
        if dc_is_embedded_signal(signal) || dc.links[li].ep_type == LF_DISPLAY_ENDPOINT_USB4_DPIA {
            // TODO - should we check link encoder's max link caps here?
            // How do we know which link encoder to check from?
            dc.links[li].verified_link_cap = dc.links[li].reported_link_cap;
        } else {
            dc.links[li].verified_link_cap = crate::link_dp_capability::dp_get_max_link_cap(adev, dc, li);
        }
    }
}

/// `should_verify_link_capability_destructively`. `link_encs_assign` is
/// not in dcn21_res_pool_funcs, so the encoder is always available.
fn should_verify_link_capability_destructively(adev: &amdgpu_device, dc: &dc, li: usize,
                                               _reason: dc_detect_reason) -> bool {
    use crate::link_dp_capability::*;
    let mut destrictive = false;
    let is_link_enc_unavailable = false;
    let link = &dc.links[li];
    let signal = link.local_sink.as_deref().unwrap().sink_signal;

    if dc_is_dp_signal(signal) {
        let max_link_cap = dp_get_max_link_cap(adev, dc, li);
        destrictive = true;

        if dc.debug.skip_detection_link_training
            || dc_is_embedded_signal(signal)
            || (link.ep_type == LF_DISPLAY_ENDPOINT_USB4_DPIA && !dc.config.enable_dpia_pre_training)
        {
            destrictive = false;
        } else if link_dp_get_encoding_format(&max_link_cap) == crate::dc_dp_types::dp_link_encoding::DP_8b_10b_ENCODING
            && (link.dpcd_caps.is_mst_capable || is_link_enc_unavailable)
        {
            destrictive = false;
        }
    }

    destrictive
}

/// `verify_link_capability`.
fn verify_link_capability(adev: &amdgpu_device, dc: &mut dc, li: usize, reason: dc_detect_reason) {
    if should_verify_link_capability_destructively(adev, dc, li, reason) {
        crate::linux::Line::new().s("verify_link_capability: verify_link_capability_destructive not ported").say();
    } else {
        verify_link_capability_non_destructive(adev, dc, li);
    }
}

/// `link_clear_dprx_states`.
pub fn link_clear_dprx_states(link: &mut crate::link_factory::dc_link) {
    link.dprx_states = crate::dc_dp_types::dprx_states::default();
}

/// `dp_trace_init` / `dp_trace_reset` (link_dp_trace.c): the training
/// counters; only the eDP power stamps are kept and they survive both.
fn dp_trace_init(_link: &mut crate::link_factory::dc_link) {}

fn dp_trace_reset(_link: &mut crate::link_factory::dc_link) {}

/// `detect_link_and_local_sink`: create or destroy `local_sink`. DP and
/// HDMI/DVI sinks (detect_dp, the DP alt mode wait, SCDC) are not yet
/// ported; an eDP panel and a disconnected connector are.
fn detect_link_and_local_sink(adev: &amdgpu_device, dc: &mut dc, li: usize, reason: dc_detect_reason) -> bool {
    use crate::dc_ddc_types::ddc_transaction_type;
    use crate::dc_sink::*;
    use crate::dc_types::dc_edid_status;
    let sink_caps_transaction_type;
    let sink_caps_signal;
    let sink_caps_max_hdmi_pixel_clock = 0;
    let converter_disable_audio = false;
    let mut same_edid = false;
    let mut new_connection_type = dc_connection_none;
    let post_oui_delay = 30; // 30ms
    let _ = reason;

    if dc_is_virtual_signal(dc.links[li].connector_signal) {
        return false;
    }

    {
        let link = &dc.links[li];
        if (link.connector_signal == LF_SIGNAL_TYPE_LVDS || link.connector_signal == LF_SIGNAL_TYPE_EDP)
            && dc.config.allow_edp_hotplug_detection == 0
            && link.local_sink.is_some()
        {
            // need to re-write OUI and brightness in resume case
            if link.connector_signal == LF_SIGNAL_TYPE_EDP
                && crate::dc_dp_types::sink_ext_caps_oled(link.dpcd_sink_ext_caps) == 1
            {
                crate::link_dp_capability::dpcd_set_source_specific_data(adev, dc, li);
                crate::linux::msleep(post_oui_delay);
                crate::link_dp_capability::set_default_brightness_aux(dc, li);
            }

            return true;
        }
    }

    if !link_detect_connection_type(adev, dc, li, &mut new_connection_type) {
        return false;
    }

    // dc_sink_retain: the previous sink moves out of the link.
    let mut prev_sink = dc.links[li].local_sink.take();
    let prev_dpcd_caps = prev_sink.as_ref().map(|_| dc.links[li].dpcd_caps);
    let _ = prev_dpcd_caps;

    link_disconnect_sink(&mut dc.links[li]);
    if new_connection_type != dc_connection_none {
        dc.links[li].r#type = new_connection_type;
        dc.links[li].link_state_valid = false;

        // From Disconnected-to-Connected.
        match dc.links[li].connector_signal {
            LF_SIGNAL_TYPE_HDMI_TYPE_A => {
                sink_caps_transaction_type = ddc_transaction_type::DDC_TRANSACTION_TYPE_I2C;
                sink_caps_signal = if dc.res_pool.as_deref().unwrap().audio_support.hdmi_audio_native {
                    LF_SIGNAL_TYPE_HDMI_TYPE_A
                } else {
                    LF_SIGNAL_TYPE_DVI_SINGLE_LINK
                };
            }
            LF_SIGNAL_TYPE_DVI_SINGLE_LINK => {
                sink_caps_transaction_type = ddc_transaction_type::DDC_TRANSACTION_TYPE_I2C;
                sink_caps_signal = LF_SIGNAL_TYPE_DVI_SINGLE_LINK;
            }
            LF_SIGNAL_TYPE_DVI_DUAL_LINK => {
                sink_caps_transaction_type = ddc_transaction_type::DDC_TRANSACTION_TYPE_I2C;
                sink_caps_signal = LF_SIGNAL_TYPE_DVI_DUAL_LINK;
            }
            LF_SIGNAL_TYPE_LVDS => {
                sink_caps_transaction_type = ddc_transaction_type::DDC_TRANSACTION_TYPE_I2C;
                sink_caps_signal = LF_SIGNAL_TYPE_LVDS;
            }
            LF_SIGNAL_TYPE_EDP => {
                crate::link_dp_capability::detect_edp_sink_caps(adev, dc, li);
                read_current_link_settings_on_detect(adev, dc, li);

                // Disable power sequence on MIPI panel + converter
                if dc.config.enable_mipi_converter_optimization
                    && dc.ctx.dce_version == crate::dc_types::dce_version::DCN_VERSION_3_01
                {
                    crate::linux::Line::new().s("detect_link_and_local_sink: MIPI converter check not ported").say();
                }

                sink_caps_transaction_type = ddc_transaction_type::DDC_TRANSACTION_TYPE_I2C_OVER_AUX;
                sink_caps_signal = LF_SIGNAL_TYPE_EDP;
            }
            LF_SIGNAL_TYPE_DISPLAY_PORT => {
                crate::linux::Line::new().s("detect_link_and_local_sink: detect_dp not ported").say();
                return false;
            }
            s => {
                crate::linux::Line::new().s("Invalid connector type! signal:").dec(s as u64).say();
                return false;
            }
        }

        {
            let link = &mut dc.links[li];
            let sc = crate::dc_dp_types::sink_count_SINK_COUNT(link.dpcd_caps.sink_count);
            link.dpcd_sink_count = if sc != 0 { sc as u32 } else { 1 };

            let ddc = link.ddc.as_deref_mut().unwrap();
            crate::link_ddc::set_ddc_transaction_type(ddc, sink_caps_transaction_type);

            link.aux_mode = crate::link_ddc::link_is_in_aux_transaction_mode(ddc);
        }

        let sink_init_data = dc_sink_init_data {
            sink_signal: sink_caps_signal,
            dongle_max_pix_clk: 0,
            converter_disable_audio: false,
        };

        let mut sink = dc_sink_create(&sink_init_data, &mut dc.ctx.dc_sink_id_count);

        dc.links[li].dongle_max_pix_clk = sink_caps_max_hdmi_pixel_clock;
        sink.converter_disable_audio = converter_disable_audio;

        // dc_sink_create returns a new reference
        let edid_status = crate::amdgpu_dm_helpers::dm_helpers_read_local_edid(adev, dc, li, &mut sink);

        match edid_status {
            dc_edid_status::EDID_BAD_CHECKSUM => crate::linux::Line::new().s("EDID checksum invalid.").say(),
            dc_edid_status::EDID_PARTIAL_VALID => {
                crate::linux::Line::new().s("Partial EDID valid, abandon invalid blocks.").say()
            }
            dc_edid_status::EDID_NO_RESPONSE => {
                crate::linux::Line::new().s("No EDID read.").say();
                // Abort detection for non-DP connectors if we have
                // no EDID
                //
                // DP needs to report as connected if HDP is high
                // even if we have no EDID in order to go to
                // fail-safe mode
                let cs = dc.links[li].connector_signal;
                if dc_is_hdmi_signal(cs) || dc_is_dvi_signal(cs) {
                    dc.links[li].local_sink = Some(sink);
                    return false;
                }

                if dc.links[li].r#type == dc_connection_sst_branch
                    && dc.links[li].dpcd_caps.dongle_type == crate::dc_ddc_types::DISPLAY_DONGLE_DP_VGA_CONVERTER
                    && reason == dc_detect_reason::DETECT_REASON_HPDRX
                {
                    // Abort detection for DP-VGA adapters when EDID
                    // can't be read and detection reason is VGA-side
                    // hotplug
                    link_disconnect_sink(&mut dc.links[li]);

                    return true;
                }
            }
            _ => {}
        }

        // Check if edid is the same
        if let Some(p) = prev_sink.as_deref() {
            if edid_status == dc_edid_status::EDID_THE_SAME || edid_status == dc_edid_status::EDID_OK {
                same_edid = is_same_edid(&p.dc_edid, &sink.dc_edid);
            }
        }

        if sink.edid_caps.panel_patch.skip_scdc_overwrite != 0 {
            dc.debug.hdmi20_disable = true;
        }

        if sink.edid_caps.panel_patch.remove_sink_ext_caps != 0 {
            dc.links[li].dpcd_sink_ext_caps = 0;
        }

        if dc_is_hdmi_signal(dc.links[li].connector_signal) {
            crate::linux::Line::new().s("detect_link_and_local_sink: read_scdc_caps not ported").say();
        }

        let sink_signal = sink.sink_signal;
        dc.links[li].local_sink = Some(sink);

        if dc.links[li].connector_signal == LF_SIGNAL_TYPE_DISPLAY_PORT
            && sink_caps_transaction_type == ddc_transaction_type::DDC_TRANSACTION_TYPE_I2C_OVER_AUX
        {
            // TODO debug why certain monitors don't like
            //  two link trainings
            crate::hdcp_msg::query_hdcp_capability(adev, dc, sink_signal, li);
        } else {
            // If edid is the same, then discard new sink and revert back to original sink
            if same_edid {
                link_disconnect_remap(prev_sink.take(), &mut dc.links[li]);
            }
            let s = dc.links[li].local_sink.as_deref().unwrap().sink_signal;
            crate::hdcp_msg::query_hdcp_capability(adev, dc, s, li);
        }

        {
            let hdmi_audio_native = dc.res_pool.as_deref().unwrap().audio_support.hdmi_audio_native;
            let connector_signal = dc.links[li].connector_signal;
            let sink = dc.links[li].local_sink.as_deref_mut().unwrap();
            // HDMI-DVI Dongle
            if sink.sink_signal == LF_SIGNAL_TYPE_HDMI_TYPE_A && !sink.edid_caps.edid_hdmi {
                sink.sink_signal = LF_SIGNAL_TYPE_DVI_SINGLE_LINK;
            } else if dc_is_dvi_signal(sink.sink_signal)
                && dc_is_dvi_signal(connector_signal)
                && hdmi_audio_native
                && sink.edid_caps.edid_hdmi
            {
                sink.sink_signal = LF_SIGNAL_TYPE_HDMI_TYPE_A;
            }
        }

        if dc.links[li].local_sink.is_some() && dc_is_dp_signal(sink_caps_signal) {
            dp_trace_init(&mut dc.links[li]);
        }

        if dc.links[li].connector_signal == LF_SIGNAL_TYPE_EDP {
            // Init dc_panel_config by HW config
            crate::dcn21_resource::dcn21_get_panel_config_defaults(&mut dc.links[li].panel_config);
            // Pickup base DM settings
            {
                let link = &mut dc.links[li];
                let sink = link.local_sink.as_deref().unwrap();
                crate::amdgpu_dm_helpers::dm_helpers_init_panel_settings(&mut link.panel_config, sink);
            }
            // Override dc_panel_config if system has specific settings
            crate::amdgpu_dm_helpers::dm_helpers_override_panel_settings(&mut dc.links[li].panel_config);

            //sink only can use supported link rate table, we are foreced to enable it
            if dc.links[li].reported_link_cap.link_rate == crate::dc_dp_types::LINK_RATE_UNKNOWN {
                dc.links[li].panel_config.ilr_optimize_edp_link_rate = true;
            }
            dc.links[li].reported_link_cap.link_rate = crate::link_dp_capability::get_max_edp_link_rate(adev, dc, li);
        }
    } else {
        // From Connected-to-Disconnected.
        let link = &mut dc.links[li];
        link.r#type = dc_connection_none;
        sink_caps_signal = LF_SIGNAL_TYPE_NONE;
        link.hdcp_caps = crate::dc_dp_types::hdcp_caps::default();
        // When we unplug a passive DP-HDMI dongle connection, dongle_max_pix_clk
        //  is not cleared. If we emulate a DP signal on this connection, it thinks
        //  the dongle is still there and limits the number of modes we can emulate.
        //  Clear dongle_max_pix_clk on disconnect to fix this
        link.dongle_max_pix_clk = 0;

        link_clear_dprx_states(link);
        dp_trace_reset(link);
    }
    let _ = sink_caps_signal;

    // dc_sink_release(prev_sink)
    drop(prev_sink);

    true
}

/// `link_detect`. MST discovery (`discover_dp_mst_topology`) needs an MST
/// sink, which an eDP panel is not.
pub fn link_detect(adev: &amdgpu_device, dc: &mut dc, li: usize, reason: dc_detect_reason) -> bool {
    let mut is_delegated_to_mst_top_mgr = false;
    let pre_link_type = dc.links[li].r#type;

    let is_local_sink_detect_success = detect_link_and_local_sink(adev, dc, li, reason);

    if is_local_sink_detect_success && dc.links[li].local_sink.is_some() {
        verify_link_capability(adev, dc, li, reason);
    }

    if is_local_sink_detect_success
        && dc.links[li].local_sink.as_deref().is_some_and(|s| dc_is_dp_signal(s.sink_signal))
        && dc.links[li].dpcd_caps.is_mst_capable
    {
        crate::linux::Line::new().s("link_detect: discover_dp_mst_topology not ported").say();
        is_delegated_to_mst_top_mgr = false;
    }

    if pre_link_type == dc_connection_mst_branch && dc.links[li].r#type != dc_connection_mst_branch {
        crate::linux::Line::new().s("link_detect: link_reset_cur_dp_mst_topology not ported").say();
    }

    is_local_sink_detect_success && !is_delegated_to_mst_top_mgr
}

/// `dc_link_detect` / `dc_link_detect_connection_type` (dc_link_exports.c),
/// through `link_srv`.
pub fn dc_link_detect(adev: &amdgpu_device, dc: &mut dc, li: usize, reason: dc_detect_reason) -> bool {
    link_detect(adev, dc, li, reason)
}

pub fn dc_link_detect_connection_type(adev: &amdgpu_device, dc: &mut dc, li: usize, r#type: &mut u32) -> bool {
    link_detect_connection_type(adev, dc, li, r#type)
}
