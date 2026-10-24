//! display/dc/link/protocols/link_dp_training.c + link_dp_training_8b_10b.c
//! + link_dp_phy.c + link_edp_panel_control.c (the panel mode) (Linux
//! 6.18.55): DisplayPort link training of the commit, 8b/10b, without an
//! LTTPR in non-transparent mode. 128b/132b, DPIA training and the fixed-VS
//! retimer sequence are not DCN 2.1's.
#![allow(dead_code)]

use crate::amdgpu_device::amdgpu_device;
use crate::dc::dc;
use crate::dc_state::link_resource;
use crate::regs::*;
use crate::dc_dp_types::{LINK_SPREAD_05_DOWNSPREAD_30KHZ, LINK_SPREAD_DISABLED};

/// `DP_BRANCH_DEVICE_ID_0022B9` / `_00001A` (ddc_service_types.h).
const DP_BRANCH_DEVICE_ID_0022B9: u32 = 0x0022B9;
const DP_BRANCH_DEVICE_ID_00001A: u32 = 0x00001A;

/// `DP_VGA_LVDS_CONVERTER_ID_2` (Travis) and `_3` (Nutmeg)
/// (link_edp_panel_control.c), compared over `sizeof(branch_dev_name)`.
const DP_VGA_LVDS_CONVERTER_ID_2: &[u8; 6] = b"sivarT";
const DP_VGA_LVDS_CONVERTER_ID_3: &[u8; 6] = b"dnomlA";

/// `dp_get_panel_mode` (link_edp_panel_control.c). A connector the VBIOS
/// reports as DP may be a Travis VGA converter, which needs the alternate
/// scrambler reset.
pub fn dp_get_panel_mode(dc: &dc, li: usize) -> u32 {
    let link = &dc.links[li];

    if link.connector_signal != LF_SIGNAL_TYPE_DISPLAY_PORT {
        match link.dpcd_caps.branch_dev_id {
            DP_BRANCH_DEVICE_ID_0022B9 => {
                if &link.dpcd_caps.branch_dev_name == DP_VGA_LVDS_CONVERTER_ID_2 {
                    return DP_PANEL_MODE_SPECIAL;
                }
            }
            DP_BRANCH_DEVICE_ID_00001A => {
                if &link.dpcd_caps.branch_dev_name == DP_VGA_LVDS_CONVERTER_ID_3 {
                    return DP_PANEL_MODE_SPECIAL;
                }
            }
            _ => {}
        }
    }

    if link.dpcd_caps.panel_mode_edp
        && (link.connector_signal == LF_SIGNAL_TYPE_EDP
            || (link.connector_signal == LF_SIGNAL_TYPE_DISPLAY_PORT && link.is_internal_display))
    {
        return DP_PANEL_MODE_EDP;
    }

    DP_PANEL_MODE_DEFAULT
}

/// `dp_set_fec_enable` (link_dp_phy.c). The FEC enable sequence may start
/// once 1000 LL codes went out after training; at 1 lane RBR that takes
/// at most 6.173 us, so 7 us are waited.
pub fn dp_set_fec_enable(adev: &amdgpu_device, dc: &mut dc, li: usize, _link_res: &link_resource, enable: bool) {
    let should_enable = crate::link_dp_capability::dp_should_enable_fec(dc, li);
    let link = &mut dc.links[li];
    let Some(enc) = link.link_enc.as_ref() else { return };
    let Some(fec_set_enable) = enc.funcs().fec_set_enable else { return };

    if enable && should_enable {
        if link.fec_state == dc_link_fec_ready {
            crate::linux::udelay(7);
            fec_set_enable(adev, enc, true);
            link.fec_state = dc_link_fec_enabled;
        }
    } else if link.fec_state == dc_link_fec_enabled {
        fec_set_enable(adev, enc, false);
        link.fec_state = dc_link_fec_ready;
    }
}

/// `enum link_training_result` (link_service_types.h).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum link_training_result {
    LINK_TRAINING_SUCCESS,
    LINK_TRAINING_CR_FAIL_LANE0,
    LINK_TRAINING_CR_FAIL_LANE1,
    LINK_TRAINING_CR_FAIL_LANE23,
    LINK_TRAINING_EQ_FAIL_CR,
    LINK_TRAINING_EQ_FAIL_CR_PARTIAL,
    LINK_TRAINING_EQ_FAIL_EQ,
    LINK_TRAINING_LQA_FAIL,
    LINK_TRAINING_LINK_LOSS,
    DP_128b_132b_LT_FAILED,
    DP_128b_132b_MAX_LOOP_COUNT_REACHED,
    DP_128b_132b_CHANNEL_EQ_DONE_TIMEOUT,
    DP_128b_132b_CDS_DONE_TIMEOUT,
    LINK_TRAINING_ABORT,
}
use link_training_result::*;

/// `struct dc_lane_settings` (dc_dp_types.h); `FFE_PRESET` is the raw
/// byte of its union.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct dc_lane_settings {
    pub VOLTAGE_SWING: u32,
    pub PRE_EMPHASIS: u32,
    pub POST_CURSOR2: u32,
    pub FFE_PRESET: u8,
}

/// `struct link_training_settings` (link_service_types.h). The lane
/// setting overrides point at the link's preferred values, so they are
/// copies here; `dpcd_lane_settings` are the raw bytes of `union
/// dpcd_training_lane`.
#[derive(Clone, Copy, Debug, Default)]
pub struct link_training_settings {
    pub link_settings: crate::dc_dp_types::dc_link_settings,
    pub voltage_swing: Option<u32>,
    pub pre_emphasis: Option<u32>,
    pub post_cursor2: Option<u32>,
    pub should_set_fec_ready: bool,
    pub ffe_preset: Option<u8>,
    pub cr_pattern_time: u16,
    pub eq_pattern_time: u16,
    pub cds_pattern_time: u16,
    pub pattern_for_cr: u32,
    pub pattern_for_eq: u32,
    pub pattern_for_cds: u32,
    pub eq_wait_time_limit: u32,
    pub eq_loop_count_limit: u8,
    pub cds_wait_time_limit: u32,
    pub enhanced_framing: bool,
    pub lttpr_mode: u32,
    pub lttpr_early_tps2: bool,
    pub disallow_per_lane_settings: bool,
    pub always_match_dpcd_with_hw_lane_settings: bool,
    pub hw_lane_settings: [dc_lane_settings; LANE_COUNT_DP_MAX as usize],
    pub dpcd_lane_settings: [u8; LANE_COUNT_DP_MAX as usize],
}

// `union dpcd_training_lane` bits.
fn tl_vs(raw: u8) -> u8 { raw & 0x3 }
fn tl_set(vs: u32, max_vs: bool, pe: u32, max_pe: bool) -> u8 {
    (vs as u8 & 0x3) | (max_vs as u8) << 2 | (pe as u8 & 0x3) << 3 | (max_pe as u8) << 5
}

// `union lane_status`, `union lane_adjust`, `union lane_align_status_updated` bits.
fn ls_cr_done(raw: u8) -> bool { raw & 0x1 != 0 }
fn ls_channel_eq_done(raw: u8) -> bool { raw & 0x2 != 0 }
fn ls_symbol_locked(raw: u8) -> bool { raw & 0x4 != 0 }
fn la_voltage_swing(raw: u8) -> u32 { (raw & 0x3) as u32 }
fn la_pre_emphasis(raw: u8) -> u32 { ((raw >> 2) & 0x3) as u32 }
fn interlane_align_done(raw: u8) -> bool { raw & 0x1 != 0 }

/// `dp_is_cr_done` (link_dp_training.c): CR_DONE on every lane.
fn dp_is_cr_done(ln_count: u32, dpcd_lane_status: &[u8]) -> bool {
    (0..ln_count as usize).all(|lane| ls_cr_done(dpcd_lane_status[lane]))
}

/// `dp_is_ch_eq_done` (link_dp_training.c).
fn dp_is_ch_eq_done(ln_count: u32, dpcd_lane_status: &[u8]) -> bool {
    (0..ln_count as usize).all(|lane| ls_channel_eq_done(dpcd_lane_status[lane]))
}

/// `dp_is_symbol_locked` (link_dp_training.c).
fn dp_is_symbol_locked(ln_count: u32, dpcd_lane_status: &[u8]) -> bool {
    (0..ln_count as usize).all(|lane| ls_symbol_locked(dpcd_lane_status[lane]))
}

/// `dp_check_interlane_aligned` (link_dp_training.c): the DP 1.4a LL
/// compliance corner case of USB4, which shares encoders, accepts a retry
/// on a link that skips fallback on link loss.
fn dp_check_interlane_aligned(align_status: u8, dc: &dc, li: usize, retries: u8) -> bool {
    interlane_align_done(align_status) || (dc.links[li].skip_fallback_on_link_loss && retries != 0)
}

/// `dp_get_cr_failure` (link_dp_training.c).
fn dp_get_cr_failure(ln_count: u32, dpcd_lane_status: &[u8]) -> link_training_result {
    if ln_count >= crate::dc_dp_types::LANE_COUNT_ONE && !ls_cr_done(dpcd_lane_status[0]) {
        LINK_TRAINING_CR_FAIL_LANE0
    } else if ln_count >= crate::dc_dp_types::LANE_COUNT_TWO && !ls_cr_done(dpcd_lane_status[1]) {
        LINK_TRAINING_CR_FAIL_LANE1
    } else if ln_count >= crate::dc_dp_types::LANE_COUNT_FOUR && !ls_cr_done(dpcd_lane_status[2]) {
        LINK_TRAINING_CR_FAIL_LANE23
    } else if ln_count >= crate::dc_dp_types::LANE_COUNT_FOUR && !ls_cr_done(dpcd_lane_status[3]) {
        LINK_TRAINING_CR_FAIL_LANE23
    } else {
        LINK_TRAINING_SUCCESS
    }
}

/// `is_repeater` (link_dp_training.c).
fn is_repeater(lt_settings: &link_training_settings, offset: u32) -> bool {
    lt_settings.lttpr_mode == LTTPR_MODE_NON_TRANSPARENT && offset != 0
}

/// `is_immediate_downstream` (link_dp_training.c).
fn is_immediate_downstream(dc: &dc, li: usize, offset: u32) -> bool {
    offset as u8 == crate::link_dp_capability::dp_parse_lttpr_repeater_count(dc.links[li].dpcd_caps.lttpr_caps.phy_repeater_cnt)
}

/// `dp_check_dpcd_reqeust_status` (link_dp_training.c).
fn dp_check_dpcd_reqeust_status(dc: &dc, li: usize, status: crate::dc_types::dc_status) -> bool {
    status != crate::dc_types::dc_status::DC_OK && dc.links[li].ep_type == LF_DISPLAY_ENDPOINT_USB4_DPIA
}

/// `dp_get_nibble_at_index` (link_dp_training.c).
fn dp_get_nibble_at_index(buf: &[u8], index: usize) -> u8 {
    let nibble = buf[index / 2];

    if index % 2 != 0 { nibble >> 4 } else { nibble & 0x0F }
}

/// `get_dpcd_link_rate` (link_dp_training.c). 128b/132b rates are not DCN
/// 2.1's.
fn get_dpcd_link_rate(link_settings: &crate::dc_dp_types::dc_link_settings) -> u8 {
    match crate::link_dp_capability::link_dp_get_encoding_format(link_settings) {
        crate::dc_dp_types::dp_link_encoding::DP_8b_10b_ENCODING => link_settings.link_rate as u8,
        _ => 0,
    }
}

fn is_8b_10b(link_settings: &crate::dc_dp_types::dc_link_settings) -> bool {
    crate::link_dp_capability::link_dp_get_encoding_format(link_settings)
        == crate::dc_dp_types::dp_link_encoding::DP_8b_10b_ENCODING
}

/// `dp_hw_to_dpcd_lane_settings` (link_dp_training.c).
fn dp_hw_to_dpcd_lane_settings(lt_settings: &link_training_settings, hw_lane_settings: &[dc_lane_settings],
                               dpcd_lane_settings: &mut [u8]) {
    for lane in 0..LANE_COUNT_DP_MAX as usize {
        if is_8b_10b(&lt_settings.link_settings) {
            let hw = &hw_lane_settings[lane];
            dpcd_lane_settings[lane] = tl_set(hw.VOLTAGE_SWING, hw.VOLTAGE_SWING == VOLTAGE_SWING_MAX_LEVEL,
                hw.PRE_EMPHASIS, hw.PRE_EMPHASIS == PRE_EMPHASIS_MAX_LEVEL);
        }
    }
}

/// `voltage_swing_to_pre_emphasis` (link_dp_training.c).
const VOLTAGE_SWING_TO_PRE_EMPHASIS: [u32; 4] =
    [PRE_EMPHASIS_LEVEL3, PRE_EMPHASIS_LEVEL2, PRE_EMPHASIS_LEVEL1, PRE_EMPHASIS_DISABLED];

/// `get_max_pre_emphasis_for_voltage_swing` (link_dp_training.c).
fn get_max_pre_emphasis_for_voltage_swing(voltage: u32) -> u32 {
    if voltage <= VOLTAGE_SWING_MAX_LEVEL {
        VOLTAGE_SWING_TO_PRE_EMPHASIS[voltage as usize]
    } else {
        PRE_EMPHASIS_MAX_LEVEL
    }
}

/// `maximize_lane_settings` (link_dp_training.c): the largest request of
/// any lane on every lane, within the limits and with a pre-emphasis the
/// swing allows.
fn maximize_lane_settings(lt_settings: &link_training_settings, lane_settings: &mut [dc_lane_settings]) {
    let mut max_requested = dc_lane_settings {
        VOLTAGE_SWING: lane_settings[0].VOLTAGE_SWING,
        PRE_EMPHASIS: lane_settings[0].PRE_EMPHASIS,
        FFE_PRESET: lane_settings[0].FFE_PRESET,
        ..Default::default()
    };

    // Determine what the maximum of the requested settings are
    for lane in 1..lt_settings.link_settings.lane_count as usize {
        if lane_settings[lane].VOLTAGE_SWING > max_requested.VOLTAGE_SWING {
            max_requested.VOLTAGE_SWING = lane_settings[lane].VOLTAGE_SWING;
        }

        if lane_settings[lane].PRE_EMPHASIS > max_requested.PRE_EMPHASIS {
            max_requested.PRE_EMPHASIS = lane_settings[lane].PRE_EMPHASIS;
        }
        if lane_settings[lane].FFE_PRESET & 0xf > max_requested.FFE_PRESET & 0xf {
            max_requested.FFE_PRESET = (max_requested.FFE_PRESET & !0xf) | (lane_settings[lane].FFE_PRESET & 0xf);
        }
    }

    // make sure the requested settings are not higher than maximum settings
    if max_requested.VOLTAGE_SWING > VOLTAGE_SWING_MAX_LEVEL {
        max_requested.VOLTAGE_SWING = VOLTAGE_SWING_MAX_LEVEL;
    }

    if max_requested.PRE_EMPHASIS > PRE_EMPHASIS_MAX_LEVEL {
        max_requested.PRE_EMPHASIS = PRE_EMPHASIS_MAX_LEVEL;
    }

    // make sure the pre-emphasis matches the voltage swing
    if max_requested.PRE_EMPHASIS > get_max_pre_emphasis_for_voltage_swing(max_requested.VOLTAGE_SWING) {
        max_requested.PRE_EMPHASIS = get_max_pre_emphasis_for_voltage_swing(max_requested.VOLTAGE_SWING);
    }

    for lane in lane_settings.iter_mut().take(LANE_COUNT_DP_MAX as usize) {
        lane.VOLTAGE_SWING = max_requested.VOLTAGE_SWING;
        lane.PRE_EMPHASIS = max_requested.PRE_EMPHASIS;
        lane.FFE_PRESET = max_requested.FFE_PRESET;
    }
}

/// `override_lane_settings` (link_dp_training.c).
fn override_lane_settings(lt_settings: &link_training_settings, lane_settings: &mut [dc_lane_settings]) {
    if lt_settings.voltage_swing.is_none() && lt_settings.pre_emphasis.is_none() && lt_settings.ffe_preset.is_none()
        && lt_settings.post_cursor2.is_none()
    {
        return;
    }

    for lane in lane_settings.iter_mut().take(LANE_COUNT_DP_MAX as usize) {
        if let Some(v) = lt_settings.voltage_swing {
            lane.VOLTAGE_SWING = v;
        }
        if let Some(v) = lt_settings.pre_emphasis {
            lane.PRE_EMPHASIS = v;
        }
        if let Some(v) = lt_settings.post_cursor2 {
            lane.POST_CURSOR2 = v;
        }
        if let Some(v) = lt_settings.ffe_preset {
            lane.FFE_PRESET = v;
        }
    }
}

/// `dp_decide_lane_settings` (link_dp_training.c): the sink's requested
/// drive settings, the same on every lane.
fn dp_decide_lane_settings(lt_settings: &mut link_training_settings, ln_adjust: &[u8]) {
    let mut hw = lt_settings.hw_lane_settings;
    let mut dpcd = lt_settings.dpcd_lane_settings;

    for lane in 0..LANE_COUNT_DP_MAX as usize {
        if is_8b_10b(&lt_settings.link_settings) {
            hw[lane].VOLTAGE_SWING = la_voltage_swing(ln_adjust[lane]);
            hw[lane].PRE_EMPHASIS = la_pre_emphasis(ln_adjust[lane]);
        }
    }
    dp_hw_to_dpcd_lane_settings(lt_settings, &hw, &mut dpcd);

    if lt_settings.disallow_per_lane_settings {
        // we find the maximum of the requested settings across all lanes
        // and set this maximum for all lanes
        maximize_lane_settings(lt_settings, &mut hw);
        override_lane_settings(lt_settings, &mut hw);

        if lt_settings.always_match_dpcd_with_hw_lane_settings {
            dp_hw_to_dpcd_lane_settings(lt_settings, &hw, &mut dpcd);
        }
    }
    lt_settings.hw_lane_settings = hw;
    lt_settings.dpcd_lane_settings = dpcd;
}

/// `dp_is_max_vs_reached` (link_dp_training.c).
fn dp_is_max_vs_reached(lt_settings: &link_training_settings) -> bool {
    (0..lt_settings.link_settings.lane_count as usize)
        .any(|lane| tl_vs(lt_settings.dpcd_lane_settings[lane]) as u32 == VOLTAGE_SWING_MAX_LEVEL)
}

/// `get_cr_training_aux_rd_interval` (link_dp_training_8b_10b.c).
fn get_cr_training_aux_rd_interval(adev: &amdgpu_device, dc: &mut dc, li: usize,
                                   link_settings: &crate::dc_dp_types::dc_link_settings, lttpr_mode: u32) -> u32 {
    let mut training_rd_interval = [0u8];
    let mut wait_in_micro_secs = 100;

    if is_8b_10b(link_settings) {
        if dc.links[li].dpcd_caps.dpcd_rev >= DPCD_REV_12 as u8 {
            crate::link_dpcd::core_link_read_dpcd(adev, dc, li, DP_TRAINING_AUX_RD_INTERVAL, &mut training_rd_interval);
        } else if crate::link_dp_capability::dp_is_lttpr_present(dc, li) {
            // get_default_8b_10b_lttpr_aux_rd_interval: an LTTPR programs
            // DPCD 0000Eh to 0x4 (16ms) upon the AUX read reply.
            training_rd_interval[0] = 0x4;
        }

        if training_rd_interval[0] != 0 {
            if lttpr_mode != LTTPR_MODE_NON_TRANSPARENT {
                wait_in_micro_secs = 400;
            }
            let interval = (training_rd_interval[0] & 0x7f) as u32;
            if interval != 0 {
                wait_in_micro_secs = interval * 4000;
            }
        }
    }
    wait_in_micro_secs
}

/// `get_eq_training_aux_rd_interval` (link_dp_training_8b_10b.c).
fn get_eq_training_aux_rd_interval(adev: &amdgpu_device, dc: &mut dc, li: usize,
                                   link_settings: &crate::dc_dp_types::dc_link_settings) -> u32 {
    let mut training_rd_interval = [0u8];

    if is_8b_10b(link_settings) {
        if dc.links[li].dpcd_caps.dpcd_rev >= DPCD_REV_12 as u8 {
            crate::link_dpcd::core_link_read_dpcd(adev, dc, li, DP_TRAINING_AUX_RD_INTERVAL, &mut training_rd_interval);
        } else if crate::link_dp_capability::dp_is_lttpr_present(dc, li) {
            training_rd_interval[0] = 0x4;
        }
    }

    match training_rd_interval[0] & 0x7f {
        0 => 400,
        1 => 4000,
        2 => 8000,
        3 => 12000,
        4 => 16000,
        5 => 32000,
        6 => 64000,
        _ => 400,
    }
}

/// `decide_cr_training_pattern` (link_dp_training.c).
fn decide_cr_training_pattern(_link_settings: &crate::dc_dp_types::dc_link_settings) -> u32 {
    DP_TRAINING_PATTERN_SEQUENCE_1
}

/// `decide_eq_training_pattern` (link_dp_training.c): the highest pattern
/// both the encoder and the receiver support.
fn decide_eq_training_pattern(dc: &dc, li: usize, link_settings: &crate::dc_dp_types::dc_link_settings) -> u32 {
    use crate::dcn21_link_encoder::{IS_TPS3_CAPABLE, IS_TPS4_CAPABLE};
    let rx_caps = &dc.links[li].dpcd_caps;

    if !is_8b_10b(link_settings) {
        return DP_TRAINING_PATTERN_SEQUENCE_2;
    }
    // link_enc_cfg_get_link_enc: the DIG encoder of a Renoir link is fixed.
    let Some(enc) = dc.links[li].link_enc.as_ref() else { return DP_TRAINING_PATTERN_SEQUENCE_2 };

    let enc_caps = &enc.base().features;
    if enc_caps.flags & IS_TPS4_CAPABLE != 0 && rx_caps.max_down_spread & 0x80 != 0 {
        DP_TRAINING_PATTERN_SEQUENCE_4
    } else if enc_caps.flags & IS_TPS3_CAPABLE != 0 && rx_caps.max_ln_count & 0x40 != 0 {
        DP_TRAINING_PATTERN_SEQUENCE_3
    } else {
        DP_TRAINING_PATTERN_SEQUENCE_2
    }
}

/// `dp_decide_8b_10b_lttpr_mode` (link_dp_training_8b_10b.c).
fn dp_decide_8b_10b_lttpr_mode(dc: &dc, li: usize) -> u32 {
    let is_lttpr_present = crate::link_dp_capability::dp_is_lttpr_present(dc, li);
    let vbios_lttpr_force_non_transparent = dc.caps.vbios_lttpr_enable;
    let vbios_lttpr_aware = dc.caps.vbios_lttpr_aware;

    if !is_lttpr_present {
        return LTTPR_MODE_NON_LTTPR;
    }

    if vbios_lttpr_aware {
        return if vbios_lttpr_force_non_transparent { LTTPR_MODE_NON_TRANSPARENT } else { LTTPR_MODE_TRANSPARENT };
    }

    if dc.config.allow_lttpr_non_transparent_mode_dp1_4a && dc.caps.extended_aux_timeout_support {
        return LTTPR_MODE_NON_TRANSPARENT;
    }

    LTTPR_MODE_NON_LTTPR
}

/// `decide_8b_10b_training_settings` (link_dp_training_8b_10b.c). The
/// spread is hard coded to on unless the link turns it off.
fn decide_8b_10b_training_settings(adev: &amdgpu_device, dc: &mut dc, li: usize,
                                   link_setting: &crate::dc_dp_types::dc_link_settings,
                                   lt_settings: &mut link_training_settings) {
    *lt_settings = link_training_settings::default();

    // Initialize link settings
    lt_settings.link_settings.use_link_rate_set = link_setting.use_link_rate_set;
    lt_settings.link_settings.link_rate_set = link_setting.link_rate_set;
    lt_settings.link_settings.link_rate = link_setting.link_rate;
    lt_settings.link_settings.lane_count = link_setting.lane_count;
    lt_settings.link_settings.link_spread =
        if dc.links[li].dp_ss_off { LINK_SPREAD_DISABLED } else { LINK_SPREAD_05_DOWNSPREAD_30KHZ };
    lt_settings.eq_pattern_time = get_eq_training_aux_rd_interval(adev, dc, li, link_setting) as u16;
    lt_settings.pattern_for_cr = decide_cr_training_pattern(link_setting);
    lt_settings.pattern_for_eq = decide_eq_training_pattern(dc, li, link_setting);
    lt_settings.enhanced_framing = true;
    lt_settings.should_set_fec_ready = true;
    lt_settings.disallow_per_lane_settings = true;
    lt_settings.always_match_dpcd_with_hw_lane_settings = true;
    lt_settings.lttpr_mode = dp_decide_8b_10b_lttpr_mode(dc, li);
    lt_settings.cr_pattern_time =
        get_cr_training_aux_rd_interval(adev, dc, li, link_setting, lt_settings.lttpr_mode) as u16;
    let hw = lt_settings.hw_lane_settings;
    let mut dpcd = lt_settings.dpcd_lane_settings;
    dp_hw_to_dpcd_lane_settings(lt_settings, &hw, &mut dpcd);
    lt_settings.dpcd_lane_settings = dpcd;

    // Some embedded LTTPRs rely on receiving TPS2 before LT to interop
    // reliably with sensitive VGA dongles. This allows these LTTPRs to
    // minimize freq/phase and skew variation during lock and deskew
    // sequences
    if (dc.links[li].chip_caps as u32 & LF_AMD_EXT_DISPLAY_PATH_CAPS__EXT_CHIP_MASK)
        == AMD_EXT_DISPLAY_PATH_CAPS__DP_EARLY_8B10B_TPS2
    {
        lt_settings.lttpr_early_tps2 = true;
    }
}

/// `dp_decide_training_settings` (link_dp_training.c).
fn dp_decide_training_settings(adev: &amdgpu_device, dc: &mut dc, li: usize,
                               link_settings: &crate::dc_dp_types::dc_link_settings,
                               lt_settings: &mut link_training_settings) {
    if is_8b_10b(link_settings) {
        decide_8b_10b_training_settings(adev, dc, li, link_settings, lt_settings);
    }
}

/// `dp_get_lttpr_mode_override` (link_dp_training.c).
fn dp_get_lttpr_mode_override(dc: &dc, li: usize, mode: &mut u32) {
    if !crate::link_dp_capability::dp_is_lttpr_present(dc, li) {
        return;
    }

    match dc.debug.lttpr_mode_override {
        LTTPR_MODE_TRANSPARENT | LTTPR_MODE_NON_TRANSPARENT | LTTPR_MODE_NON_LTTPR => {
            *mode = dc.debug.lttpr_mode_override
        }
        _ => {}
    }
}

/// `override_training_settings` (link_dp_training.c).
fn override_training_settings(dc: &dc, li: usize, lt_settings: &mut link_training_settings) {
    let link = &dc.links[li];
    let overrides = &link.preferred_training_settings;

    // Override link spread
    if !link.dp_ss_off && let Some(d) = overrides.downspread {
        lt_settings.link_settings.link_spread =
            if d { LINK_SPREAD_05_DOWNSPREAD_30KHZ } else { LINK_SPREAD_DISABLED };
    }

    // Override lane settings
    if overrides.voltage_swing.is_some() {
        lt_settings.voltage_swing = overrides.voltage_swing;
    }
    if overrides.pre_emphasis.is_some() {
        lt_settings.pre_emphasis = overrides.pre_emphasis;
    }
    if overrides.post_cursor2.is_some() {
        lt_settings.post_cursor2 = overrides.post_cursor2;
    }
    if link.wa_flags.force_dp_ffe_preset && !crate::link_dp_capability::dp_is_lttpr_present(dc, li) {
        lt_settings.ffe_preset = Some(link.forced_dp_ffe_preset);
    }
    if overrides.ffe_preset.is_some() {
        lt_settings.ffe_preset = overrides.ffe_preset;
    }
    // Override HW lane settings with BIOS forced values if present
    if link.chip_caps as u32 & LF_AMD_EXT_DISPLAY_PATH_CAPS__DP_FIXED_VS_EN != 0
        && lt_settings.lttpr_mode == LTTPR_MODE_TRANSPARENT
    {
        lt_settings.voltage_swing = Some(link.bios_forced_drive_settings.VOLTAGE_SWING);
        lt_settings.pre_emphasis = Some(link.bios_forced_drive_settings.PRE_EMPHASIS);
        lt_settings.always_match_dpcd_with_hw_lane_settings = false;
    }
    for lane in lt_settings.hw_lane_settings.iter_mut() {
        lane.VOLTAGE_SWING = lt_settings.voltage_swing.unwrap_or(VOLTAGE_SWING_LEVEL0);
        lane.PRE_EMPHASIS = lt_settings.pre_emphasis.unwrap_or(PRE_EMPHASIS_DISABLED);
        lane.POST_CURSOR2 = lt_settings.post_cursor2.unwrap_or(POST_CURSOR2_DISABLED);
    }

    if lt_settings.always_match_dpcd_with_hw_lane_settings {
        let hw = lt_settings.hw_lane_settings;
        let mut dpcd = lt_settings.dpcd_lane_settings;
        dp_hw_to_dpcd_lane_settings(lt_settings, &hw, &mut dpcd);
        lt_settings.dpcd_lane_settings = dpcd;
    }

    // Override training timings
    if let Some(v) = overrides.cr_pattern_time {
        lt_settings.cr_pattern_time = v;
    }
    if let Some(v) = overrides.eq_pattern_time {
        lt_settings.eq_pattern_time = v;
    }
    if let Some(v) = overrides.pattern_for_cr {
        lt_settings.pattern_for_cr = v;
    }
    if let Some(v) = overrides.pattern_for_eq {
        lt_settings.pattern_for_eq = v;
    }
    if let Some(v) = overrides.enhanced_framing {
        lt_settings.enhanced_framing = v;
    }
    if let Some(v) = link.preferred_training_settings.fec_enable {
        lt_settings.should_set_fec_ready = v;
    }

    // The DP tunnel LTTPR debug option is a DPIA's.

    let mut mode = lt_settings.lttpr_mode;
    dp_get_lttpr_mode_override(dc, li, &mut mode);
    lt_settings.lttpr_mode = mode;
}

/// `dp_training_pattern_to_dpcd_training_pattern` (link_dp_training.c).
fn dp_training_pattern_to_dpcd_training_pattern(pattern: u32) -> u8 {
    (match pattern {
        DP_TRAINING_PATTERN_SEQUENCE_1 => DPCD_TRAINING_PATTERN_1,
        DP_TRAINING_PATTERN_SEQUENCE_2 => DPCD_TRAINING_PATTERN_2,
        DP_TRAINING_PATTERN_SEQUENCE_3 => DPCD_TRAINING_PATTERN_3,
        DP_TRAINING_PATTERN_SEQUENCE_4 => DPCD_TRAINING_PATTERN_4,
        DP_128b_132b_TPS1 => DPCD_128b_132b_TPS1,
        DP_128b_132b_TPS2 => DPCD_128b_132b_TPS2,
        DP_128b_132b_TPS2_CDS => DPCD_128b_132b_TPS2_CDS,
        DP_TRAINING_PATTERN_VIDEOIDLE => DPCD_TRAINING_PATTERN_VIDEOIDLE,
        _ => DPCD_TRAINING_PATTERN_VIDEOIDLE,
    }) as u8
}

/// `dpcd_set_training_pattern` (link_dp_training.c).
fn dpcd_set_training_pattern(adev: &amdgpu_device, dc: &mut dc, li: usize, training_pattern: u32)
                             -> crate::dc_types::dc_status {
    let dpcd_pattern = dp_training_pattern_to_dpcd_training_pattern(training_pattern) & 0xf;

    crate::link_dpcd::core_link_write_dpcd(adev, dc, li, DP_TRAINING_PATTERN_SET, &[dpcd_pattern])
}

/// `dpcd_exit_training_mode` (link_dp_training.c): the training pattern
/// cleared. Polling for intra-hop disable is 128b/132b's.
fn dpcd_exit_training_mode(adev: &amdgpu_device, dc: &mut dc, li: usize) -> link_training_result {
    // clear training pattern set
    let status = dpcd_set_training_pattern(adev, dc, li, DP_TRAINING_PATTERN_VIDEOIDLE);

    if dp_check_dpcd_reqeust_status(dc, li, status) {
        return LINK_TRAINING_ABORT;
    }

    LINK_TRAINING_SUCCESS
}

/// `dpcd_configure_lttpr_mode` (link_dp_training.c). Configuring an LTTPR
/// joins with LTTPRs.
fn dpcd_configure_lttpr_mode(lt_settings: &link_training_settings) -> crate::dc_types::dc_status {
    if lt_settings.lttpr_mode == LTTPR_MODE_TRANSPARENT || lt_settings.lttpr_mode == LTTPR_MODE_NON_TRANSPARENT {
        crate::linux::Line::new().s("dpcd_configure_lttpr_mode: configure_lttpr_mode not ported").say();
    }
    crate::dc_types::dc_status::DC_OK
}

/// `dp_set_fec_ready` (link_dp_phy.c): FEC is set ready before training;
/// the policy is to always train with FEC if the sink supports it and
/// leave it on, and to switch it off otherwise.
fn dp_set_fec_ready(adev: &amdgpu_device, dc: &mut dc, li: usize, ready: bool) -> crate::dc_types::dc_status {
    use crate::dc_types::dc_status;
    let mut status = dc_status::DC_OK;

    let enc = dc.links[li].link_enc.as_ref().unwrap();
    let Some(fec_set_ready) = enc.funcs().fec_set_ready else { return dc_status::DC_NOT_SUPPORTED };

    if ready && crate::link_dp_capability::dp_should_enable_fec(dc, li) {
        status = crate::link_dpcd::core_link_write_dpcd(adev, dc, li, DP_FEC_CONFIGURATION, &[1]);

        if status == dc_status::DC_OK {
            fec_set_ready(adev, dc.links[li].link_enc.as_ref().unwrap(), true);
            dc.links[li].fec_state = dc_link_fec_ready;
        }
    } else if dc.links[li].fec_state == dc_link_fec_ready {
        if dc.links[li].r#type != dc_connection_none {
            crate::link_dpcd::core_link_write_dpcd(adev, dc, li, DP_FEC_CONFIGURATION, &[0]);
        }

        fec_set_ready(adev, dc.links[li].link_enc.as_ref().unwrap(), false);
        dc.links[li].fec_state = dc_link_fec_not_ready;
    }

    status
}

/// `dpcd_configure_channel_coding` (link_dp_training.c).
fn dpcd_configure_channel_coding(adev: &amdgpu_device, dc: &mut dc, li: usize, lt_settings: &link_training_settings)
                                 -> crate::dc_types::dc_status {
    let encoding = crate::link_dp_capability::link_dp_get_encoding_format(&lt_settings.link_settings) as u8;

    crate::link_dpcd::core_link_write_dpcd(adev, dc, li, DP_MAIN_LINK_CHANNEL_CODING_SET, &[encoding])
}

/// `dpcd_set_link_settings` (link_dp_training.c): spread, lane count with
/// enhanced framing, and the rate (or, for eDP 1.4, the rate set index).
fn dpcd_set_link_settings(adev: &amdgpu_device, dc: &mut dc, li: usize, lt_settings: &link_training_settings)
                          -> crate::dc_types::dc_status {
    use crate::dc_types::dc_status;
    let rate;

    let downspread = lt_settings.link_settings.link_spread as u8;

    // union lane_count_set: LANE_COUNT_SET 4:0, POST_LT_ADJ_REQ_GRANTED 5,
    // ENHANCED_FRAMING 7.
    let mut lane_count_set = (lt_settings.link_settings.lane_count as u8 & 0x1f) | (lt_settings.enhanced_framing as u8) << 7;

    if dc.links[li].ep_type == LF_DISPLAY_ENDPOINT_PHY && lt_settings.pattern_for_eq < DP_TRAINING_PATTERN_SEQUENCE_4 {
        // max_ln_count.POST_LT_ADJ_REQ_SUPPORTED is bit 5.
        lane_count_set |= dc.links[li].dpcd_caps.max_ln_count & 0x20;
    }

    let mut status = crate::link_dpcd::core_link_write_dpcd(adev, dc, li, DP_DOWNSPREAD_CTRL, &[downspread]);
    if status != dc_status::DC_OK {
        crate::linux::Line::new().s("dpcd_set_link_settings: core_link_write_dpcd (DP_DOWNSPREAD_CTRL) failed").say();
    }

    status = crate::link_dpcd::core_link_write_dpcd(adev, dc, li, DP_LANE_COUNT_SET, &[lane_count_set]);
    if status != dc_status::DC_OK {
        crate::linux::Line::new().s("dpcd_set_link_settings: core_link_write_dpcd (DP_LANE_COUNT_SET) failed").say();
    }

    if dc.links[li].dpcd_caps.dpcd_rev >= DPCD_REV_13 as u8 && lt_settings.link_settings.use_link_rate_set {
        rate = 0u8;
        // WA for some MUX chips that will power down with eDP and lose
        // supported link rate set for eDP 1.4. Source reads DPCD 0x010 again
        // to ensure MUX chip gets link rate set back before link training.
        if dc.links[li].connector_signal == LF_SIGNAL_TYPE_EDP {
            let mut supported_link_rates = [0u8; 16];

            crate::link_dpcd::core_link_read_dpcd(adev, dc, li, DP_SUPPORTED_LINK_RATES, &mut supported_link_rates);
        }
        status = crate::link_dpcd::core_link_write_dpcd(adev, dc, li, DP_LINK_BW_SET, &[rate]);
        if status != dc_status::DC_OK {
            crate::linux::Line::new().s("dpcd_set_link_settings: core_link_write_dpcd (DP_LINK_BW_SET) failed").say();
        }

        status = crate::link_dpcd::core_link_write_dpcd(adev, dc, li, DP_LINK_RATE_SET,
            &[lt_settings.link_settings.link_rate_set]);
        if status != dc_status::DC_OK {
            crate::linux::Line::new().s("dpcd_set_link_settings: core_link_write_dpcd (DP_LINK_RATE_SET) failed").say();
        }
    } else {
        rate = get_dpcd_link_rate(&lt_settings.link_settings);

        status = crate::link_dpcd::core_link_write_dpcd(adev, dc, li, DP_LINK_BW_SET, &[rate]);
        if status != dc_status::DC_OK {
            crate::linux::Line::new().s("dpcd_set_link_settings: core_link_write_dpcd (DP_LINK_BW_SET) failed").say();
        }
    }

    status
}

/// `struct encoder_set_dp_phy_pattern_param` (link_encoder.h), the members
/// the DIO path reads.
#[derive(Clone, Copy, Debug, Default)]
pub struct encoder_set_dp_phy_pattern_param {
    pub dp_phy_pattern: u32,
    pub dp_panel_mode: u32,
}

/// `dp_set_hw_test_pattern` (link_dp_training.c). Custom patterns are
/// compliance tests'.
fn dp_set_hw_test_pattern(adev: &amdgpu_device, dc: &mut dc, li: usize, link_res: &link_resource, test_pattern: u32) {
    let link_hwss = crate::link_hwss::get_link_hwss(dc, li, link_res);
    let pattern_param = encoder_set_dp_phy_pattern_param {
        dp_phy_pattern: test_pattern,
        dp_panel_mode: dp_get_panel_mode(dc, li),
    };

    crate::link_hwss::set_dp_link_test_pattern(adev, dc, li, link_hwss, &pattern_param);
}

/// `dp_set_hw_training_pattern` (link_dp_training.c).
fn dp_set_hw_training_pattern(adev: &amdgpu_device, dc: &mut dc, li: usize, link_res: &link_resource, pattern: u32,
                              _offset: u32) -> bool {
    let test_pattern = match pattern {
        DP_TRAINING_PATTERN_SEQUENCE_1 => DP_TEST_PATTERN_TRAINING_PATTERN1,
        DP_TRAINING_PATTERN_SEQUENCE_2 => DP_TEST_PATTERN_TRAINING_PATTERN2,
        DP_TRAINING_PATTERN_SEQUENCE_3 => DP_TEST_PATTERN_TRAINING_PATTERN3,
        DP_TRAINING_PATTERN_SEQUENCE_4 => DP_TEST_PATTERN_TRAINING_PATTERN4,
        DP_128b_132b_TPS1 => DP_TEST_PATTERN_128b_132b_TPS1_TRAINING_MODE,
        DP_128b_132b_TPS2 => DP_TEST_PATTERN_128b_132b_TPS2_TRAINING_MODE,
        _ => DP_TEST_PATTERN_UNSUPPORTED,
    };

    dp_set_hw_test_pattern(adev, dc, li, link_res, test_pattern);

    true
}

/// `dp_set_hw_lane_settings` (link_dp_phy.c). An LTTPR in non-transparent
/// mode takes the lane settings of the hop it is trained for.
fn dp_set_hw_lane_settings(adev: &amdgpu_device, dc: &mut dc, li: usize, link_res: &link_resource,
                           link_settings: &link_training_settings, offset: u32) {
    let link_hwss = crate::link_hwss::get_link_hwss(dc, li, link_res);

    // Don't return here if using FIXED_VS link HWSS and encoding is
    // 128b/132b
    if link_settings.lttpr_mode == LTTPR_MODE_NON_TRANSPARENT
        && !is_immediate_downstream(dc, li, offset)
        && ((dc.links[li].chip_caps as u32 & LF_AMD_EXT_DISPLAY_PATH_CAPS__EXT_CHIP_MASK)
            != LF_AMD_EXT_DISPLAY_PATH_CAPS__DP_FIXED_VS_EN
            || is_8b_10b(&link_settings.link_settings))
    {
        return;
    }

    crate::link_hwss::set_dp_lane_settings(adev, dc, li, link_hwss, &link_settings.link_settings,
        &link_settings.hw_lane_settings);

    dc.links[li].cur_lane_setting = link_settings.hw_lane_settings;
}

/// `dp_initialize_scrambling_data_symbols` (link_dp_training.c).
fn dp_initialize_scrambling_data_symbols(pattern: u32) -> u8 {
    match pattern {
        DP_TRAINING_PATTERN_SEQUENCE_1 | DP_TRAINING_PATTERN_SEQUENCE_2 | DP_TRAINING_PATTERN_SEQUENCE_3 => 1,
        DP_TRAINING_PATTERN_SEQUENCE_4 | DP_128b_132b_TPS1 | DP_128b_132b_TPS2 => 0,
        _ => {
            crate::linux::Line::new().s("dp_initialize_scrambling_data_symbols: Invalid HW Training pattern").say();
            0
        }
    }
}

/// `dpcd_set_lt_pattern_and_lane_settings` (link_dp_training.c): the
/// pattern and the lane settings as one (1 + lanes)-byte burst. The eDP
/// two-part workaround is a TODO in Linux.
fn dpcd_set_lt_pattern_and_lane_settings(adev: &amdgpu_device, dc: &mut dc, li: usize,
                                         lt_settings: &link_training_settings, pattern: u32, offset: u32) {
    let mut dpcd_base_lt_offset = DP_TRAINING_PATTERN_SET;
    let mut dpcd_lt_buffer = [0u8; 5];

    if is_repeater(lt_settings, offset) {
        dpcd_base_lt_offset = DP_TRAINING_PATTERN_SET_PHY_REPEATER1
            + DP_REPEATER_CONFIGURATION_AND_STATUS_SIZE * (offset - 1);
    }

    // union dpcd_training_pattern v1_4: TRAINING_PATTERN_SET 3:0,
    // SCRAMBLING_DISABLE 5.
    let dpcd_pattern = (dp_training_pattern_to_dpcd_training_pattern(pattern) & 0xf)
        | (dp_initialize_scrambling_data_symbols(pattern) & 1) << 5;

    dpcd_lt_buffer[0] = dpcd_pattern;

    if dc.links[li].ep_type == LF_DISPLAY_ENDPOINT_USB4_DPIA {
        crate::linux::Line::new().s("dpcd_set_lt_pattern_and_lane_settings: dpia_set_tps_notification not ported").say();
    }

    // concatenate everything into one buffer
    let size_in_bytes = lt_settings.link_settings.lane_count as usize;

    // 0x00103 - 0x00102
    let lane0 = (DP_TRAINING_LANE0_SET - DP_TRAINING_PATTERN_SET) as usize;
    dpcd_lt_buffer[lane0..lane0 + size_in_bytes].copy_from_slice(&lt_settings.dpcd_lane_settings[..size_in_bytes]);

    // write it all in (1 + number-of-lanes)-byte burst
    crate::link_dpcd::core_link_write_dpcd(adev, dc, li, dpcd_base_lt_offset, &dpcd_lt_buffer[..size_in_bytes + 1]);
}

/// `dpcd_set_lane_settings` (link_dp_training.c).
fn dpcd_set_lane_settings(adev: &amdgpu_device, dc: &mut dc, li: usize, link_training_setting: &link_training_settings,
                          offset: u32) -> crate::dc_types::dc_status {
    let mut lane0_set_address = DP_TRAINING_LANE0_SET;

    if is_repeater(link_training_setting, offset) {
        lane0_set_address = DP_TRAINING_LANE0_SET_PHY_REPEATER1
            + DP_REPEATER_CONFIGURATION_AND_STATUS_SIZE * (offset - 1);
    }

    let n = link_training_setting.link_settings.lane_count as usize;
    crate::link_dpcd::core_link_write_dpcd(adev, dc, li, lane0_set_address, &link_training_setting.dpcd_lane_settings[..n])
}

/// `dp_wait_for_training_aux_rd_interval` (link_dp_training.c).
fn dp_wait_for_training_aux_rd_interval(wait_in_micro_secs: u32) {
    crate::linux::udelay(wait_in_micro_secs as u64);
}

/// `dp_get_lane_status_and_lane_adjust` (link_dp_training.c): the lane
/// status and drive requests of the receiver (or of an LTTPR hop).
fn dp_get_lane_status_and_lane_adjust(adev: &amdgpu_device, dc: &mut dc, li: usize,
                                      link_training_setting: &link_training_settings, ln_status: &mut [u8],
                                      ln_align: &mut u8, ln_adjust: &mut [u8], offset: u32)
                                      -> crate::dc_types::dc_status {
    let mut lane01_status_address = DP_LANE0_1_STATUS;
    let mut lane_adjust_offset = 4;
    let mut dpcd_buf = [0u8; 6];

    if is_repeater(link_training_setting, offset) {
        lane01_status_address = DP_LANE0_1_STATUS_PHY_REPEATER1
            + DP_REPEATER_CONFIGURATION_AND_STATUS_SIZE * (offset - 1);
        lane_adjust_offset = 3;
    }

    let status = crate::link_dpcd::core_link_read_dpcd(adev, dc, li, lane01_status_address, &mut dpcd_buf);

    if status != crate::dc_types::dc_status::DC_OK {
        return status;
    }

    for lane in 0..link_training_setting.link_settings.lane_count as usize {
        ln_status[lane] = dp_get_nibble_at_index(&dpcd_buf[0..], lane);
        ln_adjust[lane] = dp_get_nibble_at_index(&dpcd_buf[lane_adjust_offset..], lane);
    }

    *ln_align = dpcd_buf[2];

    status
}

/// `perform_8b_10b_clock_recovery_sequence` (link_dp_training_8b_10b.c):
/// TPS1 with the current drive settings until every lane locks its clock,
/// raising them as the sink asks. A Synaptics MST hub can loop between
/// swing levels 0 and 1; the retry count caps that.
fn perform_8b_10b_clock_recovery_sequence(adev: &amdgpu_device, dc: &mut dc, li: usize, link_res: &link_resource,
                                          lt_settings: &mut link_training_settings, offset: u32)
                                          -> link_training_result {
    let mut retries_cr = 0;
    let mut retry_count = 0;
    let lane_count = lt_settings.link_settings.lane_count;
    let mut dpcd_lane_status = [0u8; LANE_COUNT_DP_MAX as usize];
    let mut dpcd_lane_status_updated = 0u8;
    let mut dpcd_lane_adjust = [0u8; LANE_COUNT_DP_MAX as usize];

    // work_arounds.lt_early_cr_pattern is not set on DCN 2.1.
    dp_set_hw_training_pattern(adev, dc, li, link_res, lt_settings.pattern_for_cr, offset);

    while retries_cr < LINK_TRAINING_MAX_RETRY_COUNT && retry_count < LINK_TRAINING_MAX_CR_RETRY {
        // 1. call HWSS to set lane settings
        dp_set_hw_lane_settings(adev, dc, li, link_res, lt_settings, offset);

        // 2. update DPCD of the receiver
        if retry_count == 0 {
            // EPR #361076 - write as a 5-byte burst, but only for the 1-st
            // iteration.
            let pattern = lt_settings.pattern_for_cr;
            dpcd_set_lt_pattern_and_lane_settings(adev, dc, li, lt_settings, pattern, offset);
        } else {
            dpcd_set_lane_settings(adev, dc, li, lt_settings, offset);
        }

        // 3. wait receiver to lock-on
        dp_wait_for_training_aux_rd_interval(lt_settings.cr_pattern_time as u32);

        // 4. Read lane status and requested drive settings as set by the sink
        let status = dp_get_lane_status_and_lane_adjust(adev, dc, li, lt_settings, &mut dpcd_lane_status,
            &mut dpcd_lane_status_updated, &mut dpcd_lane_adjust, offset);

        if dp_check_dpcd_reqeust_status(dc, li, status) {
            return LINK_TRAINING_ABORT;
        }

        // 5. check CR done
        if dp_is_cr_done(lane_count, &dpcd_lane_status) {
            return LINK_TRAINING_SUCCESS;
        }

        // 6. max VS reached
        if is_8b_10b(&lt_settings.link_settings) && dp_is_max_vs_reached(lt_settings) {
            break;
        }

        // 7. same lane settings. Note: settings are the same for all lanes,
        // so comparing first lane is sufficient
        if is_8b_10b(&lt_settings.link_settings)
            && tl_vs(lt_settings.dpcd_lane_settings[0]) as u32 == la_voltage_swing(dpcd_lane_adjust[0])
        {
            retries_cr += 1;
        } else {
            retries_cr = 0;
        }

        // 8. update VS/PE/PC2 in lt_settings
        dp_decide_lane_settings(lt_settings, &dpcd_lane_adjust);
        retry_count += 1;
    }

    if retry_count >= LINK_TRAINING_MAX_CR_RETRY {
        crate::linux::Line::new().s("perform_8b_10b_clock_recovery_sequence: Link Training Error, could not get CR after ")
            .dec(LINK_TRAINING_MAX_CR_RETRY as u64).s(" tries. Possibly voltage swing issue").say();
    }

    dp_get_cr_failure(lane_count, &dpcd_lane_status)
}

/// `dp_get_eq_aux_rd_interval` (link_dp_training.c). DPIA and LTTPR hops
/// are later stages.
fn dp_get_eq_aux_rd_interval(dc: &dc, li: usize, lt_settings: &link_training_settings, offset: u32) -> u32 {
    if dc.links[li].ep_type == LF_DISPLAY_ENDPOINT_USB4_DPIA || is_repeater(lt_settings, offset) {
        crate::linux::Line::new().s("dp_get_eq_aux_rd_interval: DPIA/LTTPR interval not ported").say();
    }
    lt_settings.eq_pattern_time as u32
}

/// `perform_8b_10b_channel_equalization_sequence` (link_dp_training_8b_10b.c).
fn perform_8b_10b_channel_equalization_sequence(adev: &amdgpu_device, dc: &mut dc, li: usize,
                                                link_res: &link_resource, lt_settings: &mut link_training_settings,
                                                offset: u32) -> link_training_result {
    let lane_count = lt_settings.link_settings.lane_count;
    let mut dpcd_lane_status_updated = 0u8;
    let mut dpcd_lane_status = [0u8; LANE_COUNT_DP_MAX as usize];
    let mut dpcd_lane_adjust = [0u8; LANE_COUNT_DP_MAX as usize];

    // Note: also check that TPS4 is a supported feature
    let mut tr_pattern = lt_settings.pattern_for_eq;

    if is_repeater(lt_settings, offset) && is_8b_10b(&lt_settings.link_settings) {
        tr_pattern = DP_TRAINING_PATTERN_SEQUENCE_4;
    }

    dp_set_hw_training_pattern(adev, dc, li, link_res, tr_pattern, offset);

    for retries_ch_eq in 0..=LINK_TRAINING_MAX_RETRY_COUNT {
        dp_set_hw_lane_settings(adev, dc, li, link_res, lt_settings, offset);

        // 2. update DPCD
        if retries_ch_eq == 0 {
            // EPR #361076 - write as a 5-byte burst, but only for the 1-st
            // iteration
            dpcd_set_lt_pattern_and_lane_settings(adev, dc, li, lt_settings, tr_pattern, offset);
        } else {
            dpcd_set_lane_settings(adev, dc, li, lt_settings, offset);
        }

        // 3. wait for receiver to lock-on
        let wait_time_microsec = dp_get_eq_aux_rd_interval(dc, li, lt_settings, offset);

        dp_wait_for_training_aux_rd_interval(wait_time_microsec);

        // 4. Read lane status and requested drive settings as set by the sink
        let status = dp_get_lane_status_and_lane_adjust(adev, dc, li, lt_settings, &mut dpcd_lane_status,
            &mut dpcd_lane_status_updated, &mut dpcd_lane_adjust, offset);

        if dp_check_dpcd_reqeust_status(dc, li, status) {
            return LINK_TRAINING_ABORT;
        }

        // 5. check CR done
        if !dp_is_cr_done(lane_count, &dpcd_lane_status) {
            return if ls_cr_done(dpcd_lane_status[0]) { LINK_TRAINING_EQ_FAIL_CR_PARTIAL } else { LINK_TRAINING_EQ_FAIL_CR };
        }

        // 6. check CHEQ done
        if dp_is_ch_eq_done(lane_count, &dpcd_lane_status)
            && dp_is_symbol_locked(lane_count, &dpcd_lane_status)
            && dp_check_interlane_aligned(dpcd_lane_status_updated, dc, li, retries_ch_eq as u8)
        {
            return LINK_TRAINING_SUCCESS;
        }

        // 7. update VS/PE/PC2 in lt_settings
        dp_decide_lane_settings(lt_settings, &dpcd_lane_adjust);
    }

    LINK_TRAINING_EQ_FAIL_EQ
}

/// `dp_perform_8b_10b_link_training` (link_dp_training_8b_10b.c). Training
/// the hops of an LTTPR in non-transparent mode, the early CR pattern and
/// the early TPS2 of embedded LTTPRs join with LTTPRs.
fn dp_perform_8b_10b_link_training(adev: &amdgpu_device, dc: &mut dc, li: usize, link_res: &link_resource,
                                   lt_settings: &mut link_training_settings) -> link_training_result {
    let mut status = LINK_TRAINING_SUCCESS;

    // 1. set link rate, lane count and spread.
    if lt_settings.lttpr_early_tps2 {
        crate::linux::Line::new().s("dp_perform_8b_10b_link_training: early TPS2 retimer sequence not ported").say();
    } else {
        dpcd_set_link_settings(adev, dc, li, lt_settings);
    }

    if lt_settings.lttpr_mode == LTTPR_MODE_NON_TRANSPARENT {
        crate::linux::Line::new().s("dp_perform_8b_10b_link_training: LTTPR hop training not ported").say();
    }

    if status == LINK_TRAINING_SUCCESS {
        status = perform_8b_10b_clock_recovery_sequence(adev, dc, li, link_res, lt_settings, DPRX);
        if status == LINK_TRAINING_SUCCESS {
            status = perform_8b_10b_channel_equalization_sequence(adev, dc, li, link_res, lt_settings, DPRX);
        }
    }

    status
}

/// `dp_transition_to_video_idle` (link_dp_training.c): the main link sends
/// the idle pattern. When the receiver and transmitter both support TPS4,
/// TPS4 is used instead of POST_LT_ADJ_REQ; an external DP sink is then
/// checked for link loss 5 ms later.
fn dp_transition_to_video_idle(adev: &amdgpu_device, dc: &mut dc, li: usize, link_res: &link_resource,
                               lt_settings: &link_training_settings, mut status: link_training_result)
                               -> link_training_result {
    // 4. mainlink output idle pattern
    dp_set_hw_test_pattern(adev, dc, li, link_res, DP_TEST_PATTERN_VIDEO_MODE);

    // 5. post training adjust if required
    let post_lt_adj_req_supported = dc.links[li].dpcd_caps.max_ln_count & 0x20 != 0;
    if !post_lt_adj_req_supported || lt_settings.pattern_for_eq >= DP_TRAINING_PATTERN_SEQUENCE_4 {
        if dc.links[li].connector_signal != LF_SIGNAL_TYPE_EDP && status == LINK_TRAINING_SUCCESS {
            crate::linux::msleep(5);
            if !dc.links[li].skip_fallback_on_link_loss {
                crate::linux::Line::new().s("dp_transition_to_video_idle: dp_check_link_loss_status not ported").say();
            }
        }
        return status;
    }

    if status == LINK_TRAINING_SUCCESS {
        crate::linux::Line::new().s("dp_transition_to_video_idle: perform_post_lt_adj_req_sequence not ported").say();
        status = LINK_TRAINING_LQA_FAIL;
    }

    let lane_count_set = (lt_settings.link_settings.lane_count as u8 & 0x1f) | (lt_settings.enhanced_framing as u8) << 7;

    crate::link_dpcd::core_link_write_dpcd(adev, dc, li, DP_LANE_COUNT_SET, &[lane_count_set]);

    status
}

/// `dp_perform_link_training` (link_dp_training.c). The fixed-VS retimer
/// sequence and 128b/132b are not DCN 2.1's; dumping the result is a log
/// line.
fn dp_perform_link_training(adev: &amdgpu_device, dc: &mut dc, li: usize, link_res: &link_resource,
                            link_settings: &crate::dc_dp_types::dc_link_settings, skip_video_pattern: bool)
                            -> link_training_result {
    let mut status = LINK_TRAINING_SUCCESS;
    let mut lt_settings = link_training_settings::default();

    // decide training settings
    dp_decide_training_settings(adev, dc, li, link_settings, &mut lt_settings);

    override_training_settings(dc, li, &mut lt_settings);

    // reset previous training states
    dpcd_exit_training_mode(adev, dc, li);

    // configure link prior to entering training mode
    dpcd_configure_lttpr_mode(&lt_settings);
    if is_8b_10b(link_settings) {
        dp_set_fec_ready(adev, dc, li, lt_settings.should_set_fec_ready);
    }
    dpcd_configure_channel_coding(adev, dc, li, &lt_settings);

    // enter training mode: per DP specs starting from here, DPTX device
    // shall not issue Non-LT AUX transactions inside training mode.
    if (dc.links[li].chip_caps as u32 & LF_AMD_EXT_DISPLAY_PATH_CAPS__EXT_CHIP_MASK)
        == LF_AMD_EXT_DISPLAY_PATH_CAPS__DP_FIXED_VS_EN
        && is_8b_10b(link_settings)
    {
        crate::linux::Line::new().s("dp_perform_link_training: dp_perform_fixed_vs_pe_training_sequence not ported").say();
    } else if is_8b_10b(link_settings) {
        status = dp_perform_8b_10b_link_training(adev, dc, li, link_res, &mut lt_settings);
    }

    // exit training mode; a DPIA aborts its training on failure
    if (dpcd_exit_training_mode(adev, dc, li) != LINK_TRAINING_SUCCESS || status == LINK_TRAINING_ABORT)
        && dc.links[li].ep_type == LF_DISPLAY_ENDPOINT_USB4_DPIA
    {
        crate::linux::Line::new().s("dp_perform_link_training: dpia_training_abort not ported").say();
    }

    // switch to video idle
    if status == LINK_TRAINING_SUCCESS || !skip_video_pattern {
        status = dp_transition_to_video_idle(adev, dc, li, link_res, &lt_settings, status);
    }

    status
}

/// `edp_set_panel_assr` (link_edp_panel_control.c): ASSR through the
/// PSP's content protection hooks, which the port's DM does not set (HDCP
/// is a later stage); `use_assr_psp_message` is a DCN 3.5 config.
fn edp_set_panel_assr(_dc: &dc, _li: usize, panel_mode: &mut u32, _enable: bool) {
    if *panel_mode != DP_PANEL_MODE_EDP {
        return;
    }
}

/// `dp_set_panel_mode` (link_edp_panel_control.c): the panel mode the
/// receiver uses, in its eDP configuration.
fn dp_set_panel_mode(adev: &amdgpu_device, dc: &mut dc, li: usize, panel_mode: u32) {
    let panel_mode_edp = matches!(panel_mode, DP_PANEL_MODE_EDP | DP_PANEL_MODE_SPECIAL);
    let mut edp_config_set = [0u8];

    // set edp panel mode in receiver
    let result = crate::link_dpcd::core_link_read_dpcd(adev, dc, li, DP_EDP_CONFIGURATION_SET, &mut edp_config_set);

    // union dpcd_edp_config: PANEL_MODE_EDP is bit 0.
    if result == crate::dc_types::dc_status::DC_OK && (edp_config_set[0] & 1 != 0) != panel_mode_edp {
        edp_config_set[0] = (edp_config_set[0] & !1) | panel_mode_edp as u8;
        crate::link_dpcd::core_link_write_dpcd(adev, dc, li, DP_EDP_CONFIGURATION_SET, &edp_config_set);
    }

    dc.links[li].panel_mode = panel_mode;
}

/// `dp_disable_link_phy` (link_dp_phy.c): the receiver to D3 with the PHY
/// going off (powering one down without the other loses symbol lock).
pub fn dp_disable_link_phy(adev: &amdgpu_device, dc: &mut dc, li: usize, link_res: &link_resource, signal: u32) {
    let link = &dc.links[li];
    if !link.wa_flags.dp_keep_receiver_powered && !link.skip_implict_edp_power_control
        && link.r#type != dc_connection_none
    {
        crate::link_dpms::dpcd_write_rx_power_ctrl(adev, dc, li, false);
    }

    (dc.hwss.disable_link_output.unwrap())(adev, dc, li, link_res, signal);
    // Clear current link setting.
    dc.links[li].cur_link_settings = Default::default();

    let mut clk_mgr = dc.clk_mgr.take().unwrap();
    if let Some(f) = clk_mgr.funcs().notify_link_rate_change {
        f(adev, dc, &mut clk_mgr, li);
    }
    dc.clk_mgr = Some(clk_mgr);
}

/// `dp_set_fec_ready` for `disable_link_dp`.
pub fn dp_set_fec_ready_for(adev: &amdgpu_device, dc: &mut dc, li: usize, ready: bool) {
    dp_set_fec_ready(adev, dc, li, ready);
}

/// `dp_enable_link_phy` (link_dp_phy.c).
fn dp_enable_link_phy(adev: &amdgpu_device, dc: &mut dc, li: usize, link_res: &link_resource, signal: u32,
                      clock_source: u32, link_settings: &crate::dc_dp_types::dc_link_settings) {
    dc.links[li].cur_link_settings = *link_settings;
    (dc.hwss.enable_dp_link_output.unwrap())(adev, dc, li, link_res, signal, clock_source, link_settings);
    crate::link_dpms::dpcd_write_rx_power_ctrl(adev, dc, li, true);
}

/// `perform_link_training_with_retries` (link_dp_training.c): train at the
/// requested settings up to `attempts` times, with the PHY re-enabled each
/// time. A fallback to lower settings between attempts (DPIA) joins with
/// DPIA.
#[allow(clippy::too_many_arguments)]
pub fn perform_link_training_with_retries(adev: &amdgpu_device, dc: &mut dc, state: &mut crate::dc_state::dc_state,
                                          link_setting: &crate::dc_dp_types::dc_link_settings,
                                          skip_video_pattern: bool, attempts: i32, p: usize, signal: u32,
                                          do_fallback: bool) -> bool {
    let mut delay_between_attempts = LINK_TRAINING_RETRY_DELAY as u64;
    let pipe_ctx = state.res_ctx.pipe_ctx[p];
    let s = pipe_ctx.stream.unwrap();
    let li = dc.streams[s].as_deref().unwrap().link;
    let mut panel_mode = dp_get_panel_mode(dc, li);
    let mut status;
    let mut cur_link_settings = *link_setting;
    let link_hwss = crate::link_hwss::get_link_hwss(dc, li, &pipe_ctx.link_res);
    let mut fail_count = 0;
    let mut is_link_bw_low = false; // link bandwidth < stream bandwidth
    // RBR x 1
    let mut is_link_bw_min = cur_link_settings.link_rate <= crate::dc_dp_types::LINK_RATE_LOW
        && cur_link_settings.lane_count <= crate::dc_dp_types::LANE_COUNT_ONE;

    // dp_trace_commit_lt_init and the commit's training timestamps and
    // counts are the trace's; the port keeps no training trace.

    if is_8b_10b(&cur_link_settings) {
        // We need to do this before the link training to ensure the idle
        // pattern in SST mode will be sent right after the link training
        crate::link_hwss::setup_stream_encoder(adev, dc, state, p, link_hwss);
    }

    let mut j = 0;
    while j < attempts && fail_count < attempts * 10 {
        let cs = pipe_ctx.clock_source.unwrap();
        let clock_source_id = crate::dc_resource::clock_source_of(dc.res_pool.as_deref().unwrap(), cs).base.id as u32;
        dp_enable_link_phy(adev, dc, li, &pipe_ctx.link_res, signal, clock_source_id, &cur_link_settings);

        let dppowerup_delay = dc.streams[s].as_deref().unwrap().sink_patches.dppowerup_delay;
        if dppowerup_delay > 0 {
            crate::linux::msleep(dppowerup_delay as u64);
        }

        edp_set_panel_assr(dc, li, &mut panel_mode, true);

        dp_set_panel_mode(adev, dc, li, panel_mode);

        if dc.links[li].aux_access_disabled {
            crate::linux::Line::new().s("perform_link_training_with_retries: dp_perform_link_training_skip_aux not ported").say();
            return true;
        } else {
            // A USB4 DPIA trains through dpia_perform_link_training.
            status = dp_perform_link_training(adev, dc, li, &pipe_ctx.link_res, &cur_link_settings, skip_video_pattern);

            if status == LINK_TRAINING_SUCCESS && !is_link_bw_low {
                return true;
            }
        }

        fail_count += 1;
        if dc.links[li].ep_type == LF_DISPLAY_ENDPOINT_PHY {
            // latest link training still fail or link training is aborted
            // skip delay and keep PHY on
            if j == attempts - 1 || status == LINK_TRAINING_ABORT {
                break;
            }
        }

        // A DPIA MST link falls back on its last attempt only; DPIA is a
        // later stage.

        dp_disable_link_phy(adev, dc, li, &pipe_ctx.link_res, signal);

        // Abort link training if failure due to sink being unplugged.
        if status == LINK_TRAINING_ABORT {
            let mut r#type = dc_connection_none;

            if crate::link_detection::link_detect_connection_type(adev, dc, li, &mut r#type)
                && r#type == dc_connection_none
            {
                break;
            }
        }

        // Try to train again at original settings if not falling back
        // between training attempts, aborted for another reason than an
        // unplug, trained below the stream's link rate or at the minimum
        // link bandwidth.
        if !do_fallback || status == LINK_TRAINING_ABORT || (status == LINK_TRAINING_SUCCESS && is_link_bw_low)
            || is_link_bw_min
        {
            j += 1;
            cur_link_settings = *link_setting;
            delay_between_attempts += LINK_TRAINING_RETRY_DELAY as u64;
            is_link_bw_low = false;
            is_link_bw_min = cur_link_settings.link_rate <= crate::dc_dp_types::LINK_RATE_LOW
                && cur_link_settings.lane_count <= crate::dc_dp_types::LANE_COUNT_ONE;
        } else {
            // Try training at lower link bandwidth if doing fallback.
            crate::linux::Line::new().s("perform_link_training_with_retries: decide_fallback_link_setting not ported").say();
            break;
        }

        crate::linux::msleep(delay_between_attempts);
    }

    false
}

/// `edp_is_ilr_optimization_required` (link_edp_panel_control.c): whether
/// the link the VBIOS trained is not the one the driver would choose, so
/// fast boot cannot keep it.
pub fn edp_is_ilr_optimization_required(adev: &amdgpu_device, dc: &mut dc, li: usize,
                                        crtc_timing: &crate::dc_hw_types::dc_crtc_timing) -> bool {
    let mut link_setting = crate::dc_dp_types::dc_link_settings::default();
    let mut link_bw_set = [0u8];
    let mut link_rate_set = [0u8];
    let mut lane_count_set = [0u8];

    if !crate::link_dp_capability::edp_is_ilr_optimization_enabled(dc, li) {
        return false;
    }

    // Read DPCD 00100h to find if standard link rates are set
    crate::link_dpcd::core_link_read_dpcd(adev, dc, li, DP_LINK_BW_SET, &mut link_bw_set);

    if link_bw_set[0] != 0 {
        return true;
    }

    // Read DPCD 00115h to find the edp link rate set used
    crate::link_dpcd::core_link_read_dpcd(adev, dc, li, DP_LINK_RATE_SET, &mut link_rate_set);

    // Read DPCD 00101h to find out the number of lanes currently set
    crate::link_dpcd::core_link_read_dpcd(adev, dc, li, DP_LANE_COUNT_SET, &mut lane_count_set);

    let req_bw = crate::dc_dsc::dc_bandwidth_in_kbps_from_timing(crtc_timing,
        crate::link_dp_capability::dc_link_get_highest_encoding_format(dc, li));

    if !crtc_timing.flags.DSC {
        crate::link_dp_capability::edp_decide_link_settings(dc, li, &mut link_setting, req_bw);
    } else {
        crate::link_dp_capability::decide_edp_link_settings_with_dsc(dc, li, &mut link_setting, req_bw,
            crate::dc_dp_types::LINK_RATE_UNKNOWN);
    }

    // A rate set index past the table (Linux reads beyond it) counts as not
    // optimal.
    dc.links[li].dpcd_caps.edp_supported_link_rates.get(link_rate_set[0] as usize).copied() != Some(link_setting.link_rate)
        || (lane_count_set[0] & 0x1f) as u32 != link_setting.lane_count
}
