//! display/intel_dp.c `intel_dp_init_connector` with the link rate setup,
//! intel_dp_aux.c `intel_dp_aux_ch` / `intel_dp_aux_init` and
//! intel_dp_mst.c `intel_dp_mst_encoder_init` (Linux 6.18.55), for a Tiger
//! Lake display without eDP.
//!
//! What the init only registers with DRM (connector funcs, properties,
//! the MST topology manager, HDCP, PSR works, the CPU latency request) has
//! no hardware side and is left out; nothing here talks AUX yet.

use crate::i915_drv::drm_i915_private;
use crate::intel_bios;
use crate::intel_de::*;
use crate::intel_encoder::*;
use crate::linux::{self, Line};
use crate::regs::*;
use crate::tables;

/// `drm_dp_bw_channel_coding_efficiency` (drm_dp_helper.c), in ppm.
fn drm_dp_bw_channel_coding_efficiency(is_uhbr: bool) -> u64 {
    if is_uhbr { 967100 } else { 800000 }
}

/// `drm_dp_is_uhbr_rate`.
fn drm_dp_is_uhbr_rate(link_rate: i32) -> bool {
    link_rate >= 1000000
}

/// `drm_dp_max_dprx_data_rate`.
pub fn drm_dp_max_dprx_data_rate(max_link_rate: i32, max_lanes: i32) -> i32 {
    let eff = drm_dp_bw_channel_coding_efficiency(drm_dp_is_uhbr_rate(max_link_rate));
    ((max_link_rate as u64 * 10 * max_lanes as u64 * eff) / (1000000 * 8)) as i32
}

/// `intel_dp_is_edp`.
pub fn intel_dp_is_edp(o: &intel_outputs, d: usize) -> bool {
    o.encoders[o.dig_ports[d].encoder].type_ == INTEL_OUTPUT_EDP
}

/// `aux_ch_name`.
pub fn aux_ch_name(l: &mut Line, aux_ch: i32) -> &mut Line {
    if aux_ch >= AUX_CH_USBC1 as i32 {
        let n = [b'1' + (aux_ch - AUX_CH_USBC1 as i32) as u8];
        l.s("USBC").s(core::str::from_utf8(&n).unwrap_or("?"))
    } else {
        let n = [b'A' + aux_ch as u8];
        l.s(core::str::from_utf8(&n).unwrap_or("?"))
    }
}

/// `default_aux_ch` (not SKL).
fn default_aux_ch(e: &intel_encoder) -> i32 {
    e.port as i32
}

/// `get_encoder_by_aux_ch`.
fn get_encoder_by_aux_ch(o: &intel_outputs, encoder: usize, aux_ch: i32) -> Option<usize> {
    (0..o.n_encoders).find(|&other| {
        other != encoder && o.dig_port_of(other).is_some_and(|d| o.dig_ports[d].aux_ch == aux_ch)
    })
}

/// `intel_dp_aux_ch`.
pub fn intel_dp_aux_ch(o: &intel_outputs, vbt: &intel_bios::intel_display, encoder: usize) -> i32 {
    let e = &o.encoders[encoder];
    let devdata = &vbt.devices()[e.devdata];
    let mut aux_ch = intel_bios::intel_bios_dp_aux_ch(vbt, devdata);
    let mut source = "VBT";

    if aux_ch == AUX_CH_NONE {
        aux_ch = default_aux_ch(e);
        source = "platform default";
    }

    if aux_ch == AUX_CH_NONE {
        return AUX_CH_NONE;
    }

    if let Some(other) = get_encoder_by_aux_ch(o, encoder, aux_ch) {
        let mut l = Line::new();
        l.s("[ENCODER:").s(e.name()).s("] AUX CH ");
        aux_ch_name(&mut l, aux_ch).s(" already claimed by [ENCODER:").s(o.encoders[other].name()).s("]").say();
        return AUX_CH_NONE;
    }

    let mut l = Line::new();
    l.s("[ENCODER:").s(e.name()).s("] Using AUX CH ");
    aux_ch_name(&mut l, aux_ch).s(" (").s(source).s(")").say();
    aux_ch
}

/// `intel_dp_aux_init`: display version 12 uses `tgl_aux_ctl_reg` /
/// `tgl_aux_data_reg`, `skl_get_aux_clock_divider` and
/// `skl_get_aux_send_ctl`.
fn intel_dp_aux_init(o: &mut intel_outputs, d: usize) {
    let aux_ch = o.dig_ports[d].aux_ch;
    let ch = if (AUX_CH_A as i32..=AUX_CH_USBC6 as i32).contains(&aux_ch) {
        aux_ch as u32
    } else {
        Line::new().s("MISSING_CASE AUX channel ").dec(aux_ch as u64).say();
        AUX_CH_A
    };
    let dp = &mut o.dig_ports[d].dp;
    dp.aux_ch_ctl_reg = DP_AUX_CH_CTL(ch);
    for i in 0..5 {
        dp.aux_ch_data_reg[i] = DP_AUX_CH_DATA(ch, i as u32);
    }
}

/// `intel_dp_set_default_sink_rates`.
fn intel_dp_set_default_sink_rates(dp: &mut intel_dp) {
    dp.sink_rates[0] = 162000;
    dp.num_sink_rates = 1;
}

/// `intel_dp_set_default_max_sink_lane_count`.
fn intel_dp_set_default_max_sink_lane_count(dp: &mut intel_dp) {
    dp.max_sink_lane_count = 1;
}

/// `intel_dp_rate_limit_len`.
fn intel_dp_rate_limit_len(rates: &[i32], len: usize, max_rate: i32) -> usize {
    for i in 0..len {
        if rates[len - i - 1] <= max_rate {
            return len - i;
        }
    }
    0
}

/// `icl_max_source_rate`.
fn icl_max_source_rate(o: &intel_outputs, d: usize) -> i32 {
    let e = &o.encoders[o.dig_ports[d].encoder];
    if intel_encoder_is_combo(e) && !intel_dp_is_edp(o, d) {
        return 540000;
    }
    810000
}

/// `vbt_max_link_rate` (no eDP).
fn vbt_max_link_rate(o: &intel_outputs, vbt: &intel_bios::intel_display, d: usize) -> i32 {
    let e = &o.encoders[o.dig_ports[d].encoder];
    intel_bios::intel_bios_dp_max_link_rate(vbt, &vbt.devices()[e.devdata]) as i32
}

/// `intel_dp_set_source_rates`: display version 11 and 12 use `icl_rates`;
/// Tiger Lake takes the `icl_max_source_rate` limit.
fn intel_dp_set_source_rates(o: &mut intel_outputs, vbt: &intel_bios::intel_display, d: usize) {
    if !o.dig_ports[d].dp.source_rates.is_empty() || o.dig_ports[d].dp.num_source_rates != 0 {
        linux::say("warning: DP source rates set twice");
    }

    let source_rates: &'static [i32] = &tables::icl_rates;
    let mut size = source_rates.len();
    let mut max_rate = icl_max_source_rate(o, d);

    let vbt_max_rate = vbt_max_link_rate(o, vbt, d);
    if max_rate != 0 && vbt_max_rate != 0 {
        max_rate = max_rate.min(vbt_max_rate);
    } else if vbt_max_rate != 0 {
        max_rate = vbt_max_rate;
    }

    if max_rate != 0 {
        size = intel_dp_rate_limit_len(source_rates, size, max_rate);
    }

    o.dig_ports[d].dp.source_rates = source_rates;
    o.dig_ports[d].dp.num_source_rates = size;
}

/// `intersect_rates`.
fn intersect_rates(source_rates: &[i32], sink_rates: &[i32], common_rates: &mut [i32]) -> usize {
    let (mut i, mut j, mut k) = (0, 0, 0);
    while i < source_rates.len() && j < sink_rates.len() {
        if source_rates[i] == sink_rates[j] {
            if k >= DP_MAX_SUPPORTED_RATES as usize {
                linux::say("warning: more common DP rates than fit");
                return k;
            }
            common_rates[k] = source_rates[i];
            k += 1;
            i += 1;
            j += 1;
        } else if source_rates[i] < sink_rates[j] {
            i += 1;
        } else {
            j += 1;
        }
    }
    k
}

/// `intel_dp_common_rate`.
fn intel_dp_common_rate(dp: &intel_dp, index: i32) -> i32 {
    if index < 0 || index as usize >= dp.num_common_rates {
        linux::say("warning: DP common rate index out of range");
        return 162000;
    }
    dp.common_rates[index as usize]
}

/// `intel_dp_max_common_rate`.
pub fn intel_dp_max_common_rate(dp: &intel_dp) -> i32 {
    intel_dp_common_rate(dp, dp.num_common_rates as i32 - 1)
}

/// `intel_dp_max_source_lane_count`.
pub fn intel_dp_max_source_lane_count(o: &intel_outputs, vbt: &intel_bios::intel_display, d: usize) -> i32 {
    let e = &o.encoders[o.dig_ports[d].encoder];
    let vbt_max_lanes = intel_bios::intel_bios_dp_max_lane_count(vbt, &vbt.devices()[e.devdata]) as i32;
    let mut max_lanes = o.dig_ports[d].max_lanes;
    if vbt_max_lanes != 0 {
        max_lanes = max_lanes.min(vbt_max_lanes);
    }
    max_lanes
}

/// `intel_dp_max_common_lane_count`. Without a DPCD read there are no
/// LTTPR caps, so `drm_dp_lttpr_max_lane_count` is 0.
pub fn intel_dp_max_common_lane_count(o: &intel_outputs, vbt: &intel_bios::intel_display, d: usize) -> i32 {
    let source_max = intel_dp_max_source_lane_count(o, vbt, d);
    let sink_max = o.dig_ports[d].dp.max_sink_lane_count;
    let lane_max = crate::intel_tc::intel_tc_port_max_lane_count(o, d) as i32;
    source_max.min(sink_max).min(lane_max)
}

/// `intel_dp_link_config_bw`.
fn intel_dp_link_config_bw(dp: &intel_dp, lc: &intel_dp_link_config) -> i32 {
    drm_dp_max_dprx_data_rate(intel_dp_common_rate(dp, lc.link_rate_idx as i32), 1 << lc.lane_count_exp)
}

/// `intel_dp_link_config_init`: every common rate × lane count, sorted by
/// bandwidth, then rate.
fn intel_dp_link_config_init(o: &mut intel_outputs, vbt: &intel_bios::intel_display, d: usize) {
    let max_lanes = intel_dp_max_common_lane_count(o, vbt, d);
    if max_lanes <= 0 || (max_lanes & (max_lanes - 1)) != 0 {
        linux::say("warning: DP common lane count is not a power of two");
        return;
    }
    let num_common_lane_configs = max_lanes.trailing_zeros() as usize + 1;

    let dp = &mut o.dig_ports[d].dp;
    if dp.num_common_rates * num_common_lane_configs > dp.link.configs.len() {
        linux::say("warning: too many DP link configs");
        return;
    }

    dp.link.num_configs = dp.num_common_rates * num_common_lane_configs;

    let mut n = 0;
    for i in 0..dp.num_common_rates {
        for j in 0..num_common_lane_configs {
            dp.link.configs[n] = intel_dp_link_config { lane_count_exp: j as u8, link_rate_idx: i as u8 };
            n += 1;
        }
    }

    // sort_r(..., link_config_cmp_by_bw): insertion sort, stable.
    let snapshot = *dp;
    let cmp = |a: &intel_dp_link_config, b: &intel_dp_link_config| -> i32 {
        let (bw_a, bw_b) = (intel_dp_link_config_bw(&snapshot, a), intel_dp_link_config_bw(&snapshot, b));
        if bw_a != bw_b {
            return bw_a - bw_b;
        }
        intel_dp_common_rate(&snapshot, a.link_rate_idx as i32) - intel_dp_common_rate(&snapshot, b.link_rate_idx as i32)
    };
    let configs = &mut dp.link.configs[..n];
    for i in 1..configs.len() {
        let mut j = i;
        while j > 0 && cmp(&configs[j - 1], &configs[j]) > 0 {
            configs.swap(j - 1, j);
            j -= 1;
        }
    }
}

/// `intel_dp_set_common_rates`.
fn intel_dp_set_common_rates(o: &mut intel_outputs, vbt: &intel_bios::intel_display, d: usize) {
    {
        let dp = &mut o.dig_ports[d].dp;
        if dp.num_source_rates == 0 || dp.num_sink_rates == 0 {
            linux::say("warning: DP rates missing");
        }
        let source = &dp.source_rates[..dp.num_source_rates];
        let sink = dp.sink_rates;
        let n_sink = dp.num_sink_rates;
        let mut common = [0i32; DP_MAX_SUPPORTED_RATES as usize];
        dp.num_common_rates = intersect_rates(source, &sink[..n_sink], &mut common);
        dp.common_rates = common;

        // Paranoia, there should always be something in common.
        if dp.num_common_rates == 0 {
            linux::say("warning: no common DP rate");
            dp.common_rates[0] = 162000;
            dp.num_common_rates = 1;
        }
    }
    intel_dp_link_config_init(o, vbt, d);
}

/// `intel_dp_reset_link_params`.
pub fn intel_dp_reset_link_params(o: &mut intel_outputs, vbt: &intel_bios::intel_display, d: usize) {
    let lanes = intel_dp_max_common_lane_count(o, vbt, d);
    let dp = &mut o.dig_ports[d].dp;
    dp.link.max_lane_count = lanes;
    dp.link.max_rate = intel_dp_max_common_rate(dp);
    dp.link.mst_probed_lane_count = 0;
    dp.link.mst_probed_rate = 0;
    dp.link.retrain_disabled = false;
    dp.link.seq_train_failures = 0;
}

/// `intel_dp_set_link_params`.
pub fn intel_dp_set_link_params(dp: &mut intel_dp, link_rate: i32, lane_count: i32) {
    dp.link.active = false;
    dp.port_clock = link_rate;
    dp.lane_count = lane_count;
}

/// `mst_stream_encoders_create`: one fake encoder per pipe.
fn mst_stream_encoders_create(o: &mut intel_outputs, d: usize, pipe_mask: u32) {
    let primary = o.encoders[o.dig_ports[d].encoder];
    for pipe in 0..I915_MAX_PIPES {
        if pipe_mask & (1 << pipe) == 0 {
            continue;
        }
        let mut e = intel_encoder {
            kind: encoder_kind::dp_mst(d, pipe),
            type_: INTEL_OUTPUT_DP_MST,
            power_domain: primary.power_domain,
            port: primary.port,
            cloneable: 0,
            // Wrong, but userspace intersects possible_crtcs over all
            // encoders of a connector.
            pipe_mask: !0,
            ..Default::default()
        };
        let p = [b'A' + pipe as u8];
        e.set_name(&["DP-MST ", core::str::from_utf8(&p).unwrap_or("?")]);
        o.dig_ports[d].dp.mst_stream_encoders[pipe as usize] = o.add_encoder(e);
    }
}

/// `intel_dp_mst_encoder_init`.
fn intel_dp_mst_encoder_init(o: &mut intel_outputs, d: usize, pipe_mask: u32) {
    if TGL_DISPLAY_HAS_DP_MST == 0 || intel_dp_is_edp(o, d) {
        return;
    }
    mst_stream_encoders_create(o, d, pipe_mask);
    o.dig_ports[d].dp.mst_source_support = true;
}

/// `_intel_dp_is_port_edp` (display version 9 and up).
fn _intel_dp_is_port_edp(devdata: Option<&intel_bios::intel_bios_encoder_data>) -> bool {
    devdata.is_some_and(intel_bios::intel_bios_encoder_supports_edp)
}

/// `intel_dp_init_connector`.
pub fn intel_dp_init_connector(i915: &mut drm_i915_private, o: &mut intel_outputs,
                               vbt: &intel_bios::intel_display, d: usize, pipe_mask: u32) -> bool {
    let enc = o.dig_ports[d].encoder;
    if o.dig_ports[d].max_lanes < 1 {
        Line::new().s("Not enough lanes (").dec(o.dig_ports[d].max_lanes as u64).s(") for DP on [ENCODER:")
            .s(o.encoders[enc].name()).s("]").say();
        return false;
    }

    o.dig_ports[d].dp.reset_link_params = true;

    // Preserve the current hw state.
    let output_reg = o.dig_ports[d].dp.output_reg;
    o.dig_ports[d].dp.DP = intel_de_read(i915, output_reg);

    let devdata = vbt.devices()[o.encoders[enc].devdata];
    let type_ = if _intel_dp_is_port_edp(Some(&devdata)) {
        // eDP on a TypeC port is not supported below display version 30.
        if intel_encoder_is_tc(&o.encoders[enc]) {
            linux::say("warning: eDP on a TypeC port");
        }
        o.encoders[enc].type_ = INTEL_OUTPUT_EDP;
        DRM_MODE_CONNECTOR_eDP
    } else {
        DRM_MODE_CONNECTOR_DisplayPort
    };

    intel_dp_set_default_sink_rates(&mut o.dig_ports[d].dp);
    intel_dp_set_default_max_sink_lane_count(&mut o.dig_ports[d].dp);

    intel_dp_aux_init(o, d);

    Line::new().s("Adding ").s(if type_ == DRM_MODE_CONNECTOR_eDP { "eDP" } else { "DP" })
        .s(" connector on [ENCODER:").s(o.encoders[enc].name()).s("]").say();

    // intel_connector_attach_encoder; DisplayPort connectors poll on HPD.
    let polled = if type_ != DRM_MODE_CONNECTOR_eDP { DRM_CONNECTOR_POLL_HPD } else { 0 };
    let Some(c) = o.add_connector(intel_connector { connector_type: type_, encoder: enc, polled, ddc_pin: 0 })
    else {
        return false;
    };
    o.dig_ports[d].dp.attached_connector = Some(c);

    if intel_dp_is_edp(o, d) {
        linux::say("intel_edp_init_connector is not ported (no eDP on this board)");
        o.n_connectors -= 1;
        o.dig_ports[d].dp.attached_connector = None;
        return false;
    }

    intel_dp_set_source_rates(o, vbt, d);
    intel_dp_set_common_rates(o, vbt, d);
    intel_dp_reset_link_params(o, vbt, d);

    // init MST on ports that can support it
    intel_dp_mst_encoder_init(o, d, pipe_mask);
    true
}

/// `intel_dp_sync_state`. The DPCD read that Linux does for an active
/// output (`intel_dp_get_dpcd`) needs AUX transfers, which are not ported;
/// it is reported and skipped. `intel_dp_tunnel_resume` is empty without
/// CONFIG_DRM_I915_DP_TUNNEL.
pub fn intel_dp_sync_state(o: &mut intel_outputs, vbt: &intel_bios::intel_display, encoder: usize,
                           crtc_state: Option<&encoder_crtc_state>) {
    let Some(d) = o.dig_port_of(encoder) else {
        return;
    };
    if crtc_state.is_some() && o.dig_ports[d].dp.dpcd_rev == 0 {
        Line::new().s("[ENCODER:").s(o.encoders[encoder].name())
            .s("] intel_dp_get_dpcd skipped: AUX transfers are not ported").say();
    }

    if let Some(cs) = crtc_state {
        intel_dp_reset_link_params(o, vbt, d);
        let dp = &mut o.dig_ports[d].dp;
        intel_dp_set_link_params(dp, cs.port_clock, cs.lane_count);
        dp.link.active = true;
    }
}
