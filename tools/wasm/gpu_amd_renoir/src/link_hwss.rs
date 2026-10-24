//! display/dc/link/hwss/link_hwss_dio.c + link_hwss_dpia.c + link_hwss_hpo_dp.c
//! + core/dc_resource.c (`get_link_hwss`) (Linux 6.18.55): the link hardware
//! sequencer a pipe's link uses. Renoir's links are DIO links: no HPO DP
//! encoder, no USB4 DPIA, no fixed-VS retimer.
#![allow(dead_code)]

use crate::dc::dc;
use crate::dc_state::link_resource;

/// Which `struct link_hwss` a link uses.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum link_hwss_id {
    dio_link_hwss,
    dio_fixed_vs_pe_retimer_link_hwss,
    virtual_link_hwss,
}

/// `can_use_dio_link_hwss` (link_hwss_dio.c). `unify_link_enc_assignment`
/// is off on DCN 2.1.
fn can_use_dio_link_hwss(dc: &dc, li: usize, _link_res: &link_resource) -> bool {
    dc.links[li].link_enc.is_some()
}

/// `requires_fixed_vs_pe_retimer_dio_link_hwss` (link_hwss_dio_fixed_vs_pe_retimer.c).
fn requires_fixed_vs_pe_retimer_dio_link_hwss(dc: &dc, li: usize) -> bool {
    (dc.links[li].chip_caps as u32 & crate::regs::LF_AMD_EXT_DISPLAY_PATH_CAPS__EXT_CHIP_MASK)
        == crate::regs::LF_AMD_EXT_DISPLAY_PATH_CAPS__DP_FIXED_VS_EN
}

/// `get_link_hwss` (dc_resource.c). A pipe has no HPO DP link encoder on
/// DCN 2.1, and a DPIA link needs the pool's `link_encs_assign`, which DCN
/// 2.1 does not have.
pub fn get_link_hwss(dc: &dc, li: usize, link_res: &link_resource) -> link_hwss_id {
    if can_use_dio_link_hwss(dc, li, link_res) {
        if requires_fixed_vs_pe_retimer_dio_link_hwss(dc, li) {
            link_hwss_id::dio_fixed_vs_pe_retimer_link_hwss
        } else {
            link_hwss_id::dio_link_hwss
        }
    } else {
        link_hwss_id::virtual_link_hwss
    }
}

/// `setup_dio_stream_attribute` (link_hwss_dio.c): stereo sync and the
/// stream attributes of the signal. HDMI, DVI and LVDS attributes join
/// with those links.
fn setup_dio_stream_attribute(adev: &crate::amdgpu_device::amdgpu_device, dc: &mut dc,
                              state: &crate::dc_state::dc_state, p: usize) {
    use crate::link_detection::*;
    let pipe_ctx = &state.res_ctx.pipe_ctx[p];
    let stream = dc.streams[pipe_ctx.stream.unwrap()].as_deref().unwrap();
    let li = stream.link;
    let pool = dc.res_pool.as_deref().unwrap();
    let se = pool.stream_enc[pipe_ctx.stream_res.stream_enc.unwrap()].as_ref().unwrap();
    let tg_inst = pool.timing_generators[pipe_ctx.stream_res.tg.unwrap()].as_deref().unwrap().base.inst;

    if !dc_is_virtual_signal(stream.signal) {
        (se.funcs().setup_stereo_sync.unwrap())(adev, se, tg_inst as i32,
            stream.timing.timing_3d_format != crate::regs::TIMING_3D_FORMAT_NONE);
    }

    if dc_is_dp_signal(stream.signal) {
        let sst_split_sdp_cap = crate::dc_dp_types::bits(dc.links[li].dpcd_caps.dprx_feature, 1, 1);
        (se.funcs().dp_set_stream_attribute.unwrap())(adev, se, &stream.timing, stream.output_color_space,
            stream.use_vsc_sdp_for_colorimetry, sst_split_sdp_cap as u32);
    } else if dc_is_hdmi_tmds_signal(stream.signal) || dc_is_dvi_signal(stream.signal) || dc_is_lvds_signal(stream.signal) {
        crate::linux::Line::new().s("setup_dio_stream_attribute: hdmi/dvi/lvds_set_stream_attribute not ported").say();
    }

    if dc_is_dp_signal(stream.signal) {
        crate::link_dpms::dp_trace_source_sequence(adev, dc, li, crate::regs::DPCD_SOURCE_SEQ_AFTER_DP_STREAM_ATTR);
    }
}

/// `link_hwss->setup_stream_attribute`.
pub fn setup_stream_attribute(adev: &crate::amdgpu_device::amdgpu_device, dc: &mut dc, state: &crate::dc_state::dc_state,
                              p: usize, hwss: link_hwss_id) {
    match hwss {
        link_hwss_id::dio_link_hwss | link_hwss_id::dio_fixed_vs_pe_retimer_link_hwss => {
            setup_dio_stream_attribute(adev, dc, state, p)
        }
        // virtual_setup_stream_attribute
        link_hwss_id::virtual_link_hwss => {}
    }
}

/// `setup_dio_stream_encoder` (link_hwss_dio.c): the DIG front end onto
/// its back end. dcn20_str_enc_funcs has neither `enable_stream`,
/// `map_stream_to_link`, `set_input_mode` nor `enable_fifo`.
fn setup_dio_stream_encoder(adev: &crate::amdgpu_device::amdgpu_device, dc: &mut dc, state: &crate::dc_state::dc_state,
                            p: usize) {
    let pipe_ctx = &state.res_ctx.pipe_ctx[p];
    let stream = dc.streams[pipe_ctx.stream.unwrap()].as_deref().unwrap();
    let li = stream.link;
    let signal = stream.signal;

    // link_enc_cfg_get_link_enc: the DIG encoder of a Renoir link is fixed.
    let Some(enc) = dc.links[li].link_enc.as_ref() else { return };

    let se_id = dc.res_pool.as_deref().unwrap().stream_enc[pipe_ctx.stream_res.stream_enc.unwrap()].as_ref().unwrap()
        .base().id as u32;
    (enc.funcs().connect_dig_be_to_fe.unwrap())(adev, enc, se_id, true);
    if crate::link_detection::dc_is_dp_signal(signal) {
        crate::link_dpms::dp_trace_source_sequence(adev, dc, li, crate::regs::DPCD_SOURCE_SEQ_AFTER_CONNECT_DIG_FE_BE);
    }
}

/// `link_hwss->setup_stream_encoder`.
pub fn setup_stream_encoder(adev: &crate::amdgpu_device::amdgpu_device, dc: &mut dc, state: &crate::dc_state::dc_state,
                            p: usize, hwss: link_hwss_id) {
    match hwss {
        link_hwss_id::dio_link_hwss | link_hwss_id::dio_fixed_vs_pe_retimer_link_hwss => {
            setup_dio_stream_encoder(adev, dc, state, p)
        }
        // virtual_setup_stream_encoder
        link_hwss_id::virtual_link_hwss => {}
    }
}

/// `set_dio_dp_link_test_pattern` (link_hwss_dio.c).
fn set_dio_dp_link_test_pattern(adev: &crate::amdgpu_device::amdgpu_device, dc: &mut dc, li: usize,
                                tp_params: &crate::link_dp_training::encoder_set_dp_phy_pattern_param) {
    // link_enc_cfg_get_link_enc: the DIG encoder of a Renoir link is fixed.
    let Some(enc) = dc.links[li].link_enc.as_ref() else { return };

    (enc.funcs().dp_set_phy_pattern.unwrap())(adev, enc, tp_params);
    crate::link_dpms::dp_trace_source_sequence(adev, dc, li, crate::regs::DPCD_SOURCE_SEQ_AFTER_SET_SOURCE_PATTERN);
}

/// `link_hwss->ext.set_dp_link_test_pattern`.
pub fn set_dp_link_test_pattern(adev: &crate::amdgpu_device::amdgpu_device, dc: &mut dc, li: usize, hwss: link_hwss_id,
                                tp_params: &crate::link_dp_training::encoder_set_dp_phy_pattern_param) {
    match hwss {
        link_hwss_id::dio_link_hwss => set_dio_dp_link_test_pattern(adev, dc, li, tp_params),
        link_hwss_id::dio_fixed_vs_pe_retimer_link_hwss => {
            crate::linux::Line::new().s("set_dp_link_test_pattern: the fixed-VS retimer link hwss is not ported").say()
        }
        link_hwss_id::virtual_link_hwss => {}
    }
}

/// `set_dio_dp_lane_settings` (link_hwss_dio.c).
fn set_dio_dp_lane_settings(adev: &crate::amdgpu_device::amdgpu_device, dc: &mut dc, li: usize,
                            link_settings: &crate::dc_dp_types::dc_link_settings,
                            lane_settings: &[crate::link_dp_training::dc_lane_settings]) {
    let Some(enc) = dc.links[li].link_enc.as_ref() else { return };

    let f = enc.funcs().dp_set_lane_settings.unwrap();
    f(adev, dc, li, link_settings, lane_settings);
}

/// `link_hwss->ext.set_dp_lane_settings`.
pub fn set_dp_lane_settings(adev: &crate::amdgpu_device::amdgpu_device, dc: &mut dc, li: usize, hwss: link_hwss_id,
                            link_settings: &crate::dc_dp_types::dc_link_settings,
                            lane_settings: &[crate::link_dp_training::dc_lane_settings]) {
    match hwss {
        link_hwss_id::dio_link_hwss => set_dio_dp_lane_settings(adev, dc, li, link_settings, lane_settings),
        link_hwss_id::dio_fixed_vs_pe_retimer_link_hwss => {
            crate::linux::Line::new().s("set_dp_lane_settings: the fixed-VS retimer link hwss is not ported").say()
        }
        link_hwss_id::virtual_link_hwss => {}
    }
}

/// `enable_dio_dp_link_output` (link_hwss_dio.c).
fn enable_dio_dp_link_output(adev: &crate::amdgpu_device::amdgpu_device, dc: &mut dc, li: usize, signal: u32,
                             clock_source: u32, link_settings: &crate::dc_dp_types::dc_link_settings) {
    let Some(enc) = dc.links[li].link_enc.as_ref() else { return };

    if crate::link_detection::dc_is_dp_sst_signal(signal) {
        let f = enc.funcs().enable_dp_output.unwrap();
        f(adev, dc, li, link_settings, clock_source);
    } else {
        crate::linux::Line::new().s("enable_dio_dp_link_output: enable_dp_mst_output not ported").say();
    }
    crate::link_dpms::dp_trace_source_sequence(adev, dc, li, crate::regs::DPCD_SOURCE_SEQ_AFTER_ENABLE_LINK_PHY);
}

/// `link_hwss->ext.enable_dp_link_output`.
pub fn enable_dp_link_output(adev: &crate::amdgpu_device::amdgpu_device, dc: &mut dc, li: usize, hwss: link_hwss_id,
                             signal: u32, clock_source: u32, link_settings: &crate::dc_dp_types::dc_link_settings) {
    match hwss {
        link_hwss_id::dio_link_hwss => enable_dio_dp_link_output(adev, dc, li, signal, clock_source, link_settings),
        link_hwss_id::dio_fixed_vs_pe_retimer_link_hwss => {
            crate::linux::Line::new().s("enable_dp_link_output: the fixed-VS retimer link hwss is not ported").say()
        }
        link_hwss_id::virtual_link_hwss => {}
    }
}

/// `disable_dio_link_output` (link_hwss_dio.c).
fn disable_dio_link_output(adev: &crate::amdgpu_device::amdgpu_device, dc: &mut dc, li: usize, signal: u32) {
    let Some(enc) = dc.links[li].link_enc.as_ref() else { return };

    let f = enc.funcs().disable_output.unwrap();
    f(adev, dc, li, signal);
    crate::link_dpms::dp_trace_source_sequence(adev, dc, li, crate::regs::DPCD_SOURCE_SEQ_AFTER_DISABLE_LINK_PHY);
}

/// `link_hwss->disable_link_output`.
pub fn disable_link_output(adev: &crate::amdgpu_device::amdgpu_device, dc: &mut dc, li: usize, hwss: link_hwss_id,
                           signal: u32) {
    match hwss {
        link_hwss_id::dio_link_hwss => disable_dio_link_output(adev, dc, li, signal),
        link_hwss_id::dio_fixed_vs_pe_retimer_link_hwss => {
            crate::linux::Line::new().s("disable_link_output: the fixed-VS retimer link hwss is not ported").say()
        }
        link_hwss_id::virtual_link_hwss => {}
    }
}
