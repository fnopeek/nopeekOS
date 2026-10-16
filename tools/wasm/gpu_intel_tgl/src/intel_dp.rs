//! display/intel_dp.c `intel_dp_init_connector` with the link rate setup,
//! intel_dp_aux.c `intel_dp_aux_ch` / `intel_dp_aux_init` and
//! intel_dp_mst.c `intel_dp_mst_encoder_init` (Linux 6.18.55), for a Tiger
//! Lake display without eDP.
//!
//! What the init only registers with DRM (connector funcs, properties,
//! the MST topology manager, HDCP, PSR works, the CPU latency request) has
//! no hardware side and is left out. `intel_dp_detect` reads the DPCD and
//! the EDID over AUX.

use crate::i915_drv::drm_i915_private;
use crate::intel_bios;
use crate::intel_de::*;
use crate::intel_display_types::intel_crtc_state;
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

/// `intel_dp_link_symbol_size`: 32 bits per symbol for 128b/132b, 10 for
/// 8b/10b.
pub fn intel_dp_link_symbol_size(rate: i32) -> i32 {
    if drm_dp_is_uhbr_rate(rate) { 32 } else { 10 }
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
pub fn intel_dp_aux_ch(o: &intel_outputs, vbt: &intel_bios::intel_bios_display, encoder: usize) -> i32 {
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
fn vbt_max_link_rate(o: &intel_outputs, vbt: &intel_bios::intel_bios_display, d: usize) -> i32 {
    let e = &o.encoders[o.dig_ports[d].encoder];
    intel_bios::intel_bios_dp_max_link_rate(vbt, &vbt.devices()[e.devdata]) as i32
}

/// `intel_dp_set_source_rates`: display version 11 and 12 use `icl_rates`;
/// Tiger Lake takes the `icl_max_source_rate` limit.
fn intel_dp_set_source_rates(o: &mut intel_outputs, vbt: &intel_bios::intel_bios_display, d: usize) {
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
pub fn intel_dp_max_source_lane_count(o: &intel_outputs, vbt: &intel_bios::intel_bios_display, d: usize) -> i32 {
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
pub fn intel_dp_max_common_lane_count(o: &intel_outputs, vbt: &intel_bios::intel_bios_display, d: usize) -> i32 {
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
fn intel_dp_link_config_init(o: &mut intel_outputs, vbt: &intel_bios::intel_bios_display, d: usize) {
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
fn intel_dp_set_common_rates(o: &mut intel_outputs, vbt: &intel_bios::intel_bios_display, d: usize) {
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
pub fn intel_dp_reset_link_params(o: &mut intel_outputs, vbt: &intel_bios::intel_bios_display, d: usize) {
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
                               vbt: &intel_bios::intel_bios_display, d: usize, pipe_mask: u32) -> bool {
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
    let Some(c) = o.add_connector(intel_connector { connector_type: type_, encoder: enc, polled, ddc_pin: 0, ..Default::default() })
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

/// `intel_dp_sync_state`. `intel_dp_tunnel_resume` is empty without
/// CONFIG_DRM_I915_DP_TUNNEL.
pub fn intel_dp_sync_state(i915: &mut drm_i915_private, o: &mut intel_outputs, vbt: &intel_bios::intel_bios_display,
                           encoder: usize, crtc_state: Option<&intel_crtc_state>) {
    let Some(d) = o.dig_port_of(encoder) else {
        return;
    };
    // Don't clobber DPCD if it's been already read out during output setup
    // (eDP) or detect.
    if crtc_state.is_some() && o.dig_ports[d].dp.dpcd[DP_DPCD_REV as usize] == 0 {
        let connector_type = o.dig_ports[d].dp.attached_connector
            .map(|c| o.connectors[c].connector_type).unwrap_or(DRM_MODE_CONNECTOR_DisplayPort);
        let mut dp = o.dig_ports[d].dp;
        let mut aux = crate::intel_dp_aux::drm_dp_aux {
            i915, o: &mut *o, d, dpcd_probe_disabled: false, i2c_nack_count: 0, i2c_defer_count: 0,
        };
        intel_dp_get_dpcd(&mut aux, &mut dp, connector_type);
        o.dig_ports[d].dp = dp;
    }

    if let Some(cs) = crtc_state {
        intel_dp_reset_link_params(o, vbt, d);
        let dp = &mut o.dig_ports[d].dp;
        intel_dp_set_link_params(dp, cs.port_clock, cs.lane_count);
        dp.link.active = true;
    }
}

/// `drm_dp_read_desc`: the sink's or branch's OUI and identity.
fn drm_dp_read_desc(aux: &mut crate::intel_dp_aux::drm_dp_aux, is_branch: bool, ident: &mut [u8; 12]) -> i32 {
    let offset = if is_branch { DP_BRANCH_OUI } else { DP_SINK_OUI };
    crate::intel_dp_aux::drm_dp_dpcd_read_data(aux, offset, ident)
}

/// `intel_dp_has_sink_count`.
fn intel_dp_has_sink_count(dp: &intel_dp, connector_type: u32) -> bool {
    crate::intel_dp_aux::drm_dp_read_sink_count_cap(connector_type, &dp.dpcd)
}

/// `intel_dp_get_dpcd`. LTTPRs are not detected (the DPCD is read through
/// them in transparent mode); colorimetry and the sink's link rates are
/// left to the link training, which is not ported.
fn intel_dp_get_dpcd(aux: &mut crate::intel_dp_aux::drm_dp_aux, dp: &mut intel_dp, connector_type: u32) -> bool {
    let mut dpcd = [0u8; DP_RECEIVER_CAP_SIZE as usize];
    if crate::intel_dp_aux::drm_dp_read_dpcd_caps(aux, &mut dpcd) < 0 {
        return false;
    }
    dp.dpcd = dpcd;
    dp.dpcd_rev = dpcd[DP_DPCD_REV as usize];

    let mut ident = [0u8; 12];
    if drm_dp_read_desc(aux, crate::intel_dp_aux::drm_dp_is_branch(&dpcd), &mut ident) == 0 {
        dp.desc_ident = ident;
    }

    if intel_dp_has_sink_count(dp, connector_type) {
        let ret = crate::intel_dp_aux::drm_dp_read_sink_count(aux);
        if ret < 0 {
            return false;
        }
        dp.sink_count = ret;
        // SINK_COUNT == 0 with a downstream port: a dongle without a display.
        if dp.sink_count == 0 {
            return false;
        }
    }

    let mut ports = [0u8; DP_MAX_DOWNSTREAM_PORTS as usize];
    let ok = crate::intel_dp_aux::drm_dp_read_downstream_info(aux, &dpcd, &mut ports) == 0;
    dp.downstream_ports = ports;
    ok
}

/// `drm_probe_ddc`: a one-byte read at the EDID address.
fn drm_probe_ddc(aux: &mut crate::intel_dp_aux::drm_dp_aux) -> bool {
    let mut offset = [0u8; 1];
    let mut byte = [0u8; 1];
    let mut msgs = [
        crate::intel_dp_aux::i2c_msg { addr: DDC_ADDR, flags: 0, buf: &mut offset },
        crate::intel_dp_aux::i2c_msg { addr: DDC_ADDR, flags: I2C_M_RD, buf: &mut byte },
    ];
    crate::intel_dp_aux::drm_dp_i2c_xfer(aux, &mut msgs) == 2
}

/// `enum drm_connector_status`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum connector_status {
    connected,
    disconnected,
    unknown,
}

/// `intel_dp_detect_dpcd`. No LSPCON on this board, and MST is not
/// detected (the topology manager is not ported).
fn intel_dp_detect_dpcd(aux: &mut crate::intel_dp_aux::drm_dp_aux, dp: &mut intel_dp, connector_type: u32)
                        -> connector_status {
    if !intel_dp_get_dpcd(aux, dp, connector_type) {
        return connector_status::disconnected;
    }

    // if there's no downstream port, we're done
    if !crate::intel_dp_aux::drm_dp_is_branch(&dp.dpcd) {
        return connector_status::connected;
    }

    // If we're HPD-aware, SINK_COUNT changes dynamically.
    if intel_dp_has_sink_count(dp, connector_type) && dp.downstream_ports[0] & DP_DS_PORT_HPD as u8 != 0 {
        return if dp.sink_count != 0 { connector_status::connected } else { connector_status::disconnected };
    }

    // If no HPD, poke DDC gently.
    if drm_probe_ddc(aux) {
        return connector_status::connected;
    }

    // Well we tried, say unknown for unreliable port types.
    if dp.dpcd[DP_DPCD_REV as usize] >= 0x11 {
        let type_ = dp.downstream_ports[0] as u32 & DP_DS_PORT_TYPE_MASK;
        if type_ == DP_DS_PORT_TYPE_VGA || type_ == DP_DS_PORT_TYPE_NON_EDID {
            return connector_status::unknown;
        }
    } else {
        let type_ = dp.dpcd[DP_DOWNSTREAMPORT_PRESENT as usize] as u32 & DP_DWN_STRM_PORT_TYPE_MASK;
        if type_ == DP_DWN_STRM_PORT_TYPE_ANALOG || type_ == DP_DWN_STRM_PORT_TYPE_OTHER {
            return connector_status::unknown;
        }
    }

    // Anything else is out of spec.
    connector_status::disconnected
}

/// Short audio descriptor rates (`cea_sad.freq` bits), in Hz.
const SAD_RATES: [u32; 7] = [32000, 44100, 48000, 88200, 96000, 176400, 192000];

/// One line naming what was found: DPCD, branch, EDID and audio.
fn log_detect(name: &str, dp: &intel_dp, monitor: &[u8], blocks: usize, a: &crate::i915_drv::connector_audio,
              sads: usize) {
    let mut l = Line::new();
    l.s("[CONNECTOR:").s(name).s("] DPCD ").dec((dp.dpcd_rev >> 4) as u64).s(".").dec((dp.dpcd_rev & 0xf) as u64);
    if crate::intel_dp_aux::drm_dp_is_branch(&dp.dpcd) {
        let id = &dp.desc_ident;
        l.s(", branch OUI ").hex(((id[0] as u32) << 16) | ((id[1] as u32) << 8) | id[2] as u32).s(" \"");
        for &b in &id[3..9] {
            if (0x20..0x7f).contains(&b) {
                l.s(core::str::from_utf8(&[b]).unwrap_or("?"));
            }
        }
        l.s("\", port type ").dec(((dp.downstream_ports[0] as u32 & DP_DS_PORT_TYPE_MASK) >> 0) as u64)
            .s(", sink count ").dec(dp.sink_count as u64);
    }
    l.s("; EDID \"").s(core::str::from_utf8(monitor).unwrap_or("?")).s("\", ").dec(blocks as u64).s(" blocks, audio ")
        .s(if a.has_audio { "yes" } else { "no" }).s(", ").dec(sads as u64).s(" SADs");
    l.say();
    let mnl = (a.eld[DRM_ELD_CEA_EDID_VER_MNL as usize] as usize) & DRM_ELD_MNL_MASK as usize;
    for i in 0..sads {
        let sad = &a.eld[20 + mnl + 3 * i..20 + mnl + 3 * i + 3];
        let mut l = Line::new();
        l.s("  SAD ").dec(i as u64).s(": format ").dec(((sad[0] >> 3) & 0xf) as u64).s(", ")
            .dec(((sad[0] & 7) + 1) as u64).s(" ch,");
        for (b, r) in SAD_RATES.iter().enumerate() {
            if sad[1] & (1 << b) != 0 {
                l.s(" ").dec(*r as u64);
            }
        }
        l.s(" Hz").say();
    }
}

/// `intel_dp_detect` for one DisplayPort connector, the parts with a
/// hardware side: the port's connection, the DPCD, and the EDID with the
/// display info's audio and the ELD (`intel_dp_set_edid` →
/// `drm_edid_connector_update`). PSR, DSC and SDP caps, MST and the link
/// state check are not ported.
fn intel_dp_detect(i915: &mut drm_i915_private, c: usize) {
    let mut o = i915.display.outputs;
    let enc = o.connectors[c].encoder;
    let connector_type = o.connectors[c].connector_type;
    let Some(d) = o.dig_port_of(enc) else { return };
    let name: alloc::string::String = o.encoders[enc].name().into();

    let mut aux = crate::intel_dp_aux::drm_dp_aux {
        i915: &mut *i915, o: &mut o, d, dpcd_probe_disabled: false, i2c_nack_count: 0, i2c_defer_count: 0,
    };

    // The DPCD probe is on for this detect (intel_dp_dpcd_set_probe at
    // init), as for any first detect.
    let status = if crate::intel_dp_aux::intel_digital_port_connected(aux.i915, aux.o, d) {
        let mut dp = aux.o.dig_ports[d].dp;
        let st = intel_dp_detect_dpcd(&mut aux, &mut dp, connector_type);
        aux.o.dig_ports[d].dp = dp;
        st
    } else {
        connector_status::disconnected
    };

    let mut a = crate::i915_drv::connector_audio::default();
    if status == connector_status::disconnected {
        i915.display.outputs = o;
        i915.display.connector_audio[c] = a;
        return;
    }

    // intel_dp_set_edid
    let edid = crate::drm_edid::drm_edid_read_ddc(&mut aux);
    let dp = aux.o.dig_ports[d].dp;
    let mut monitor = [0u8; 13];
    let mut mnl = 0;
    let mut blocks = 0;
    let mut sads = 0;
    if let Some(e) = edid.as_ref() {
        blocks = e.edid.len() / EDID_LENGTH as usize;
        mnl = crate::drm_edid::monitor_name(e, &mut monitor);
        a.has_audio = crate::drm_edid::drm_detect_monitor_audio(e);
        let is_dp = connector_type == DRM_MODE_CONNECTOR_DisplayPort || connector_type == DRM_MODE_CONNECTOR_eDP;
        sads = crate::drm_edid::drm_edid_to_eld(e, is_dp, &mut a.eld, &mut a.latency);
    } else {
        Line::new().s("[CONNECTOR:").s(&name).s("] connected, but no EDID").say();
    }
    i915.display.outputs = o;
    i915.display.connector_audio[c] = a;
    if edid.is_some() {
        log_detect(&name, &dp, &monitor[..mnl], blocks, &a, sads);
    }
}

/// Detect every DisplayPort connector once, as the first client probe does.
pub fn intel_dp_detect_all(i915: &mut drm_i915_private) {
    for c in 0..i915.display.outputs.n_connectors {
        let t = i915.display.outputs.connectors[c].connector_type;
        if t == DRM_MODE_CONNECTOR_DisplayPort {
            intel_dp_detect(i915, c);
        }
    }
}
