//! display/intel_modeset_setup.c (Linux 6.18.55): `intel_modeset_setup_hw_state`
//! for a Tiger Lake display: read the state the firmware left, then
//! sanitize it.
//!
//! What Linux does to switch something off that is inconsistent (a plane
//! on the wrong DBUF slices or pipe, a pipe without encoder, an encoder
//! without pipe) is the noatomic disable path of a modeset, which waits
//! for vblanks and is not ported; each such case is reported and left as
//! it is.

use crate::i915_drv::drm_i915_private;
use crate::intel_crtc;
use crate::intel_ddi;
use crate::intel_de::*;
use crate::intel_display;
use crate::intel_display_power::*;
use crate::intel_display_types::*;
use crate::intel_dpll_mgr;
use crate::intel_encoder::*;
use crate::intel_opregion::intel_opregion;
use crate::linux::Line;
use crate::regs::*;

const MAX_PIPES: usize = I915_MAX_PIPES as usize;

fn crtcs(i915: &drm_i915_private) -> impl Iterator<Item = usize> + use<> {
    let valid = i915.display.crtc_valid;
    (0..MAX_PIPES).filter(move |&c| valid[c])
}

/// `intel_early_display_was`: Display WA #1185 WaDisableDARBFClkGating
/// (Wa_14010480278) for display versions 10-12.
fn intel_early_display_was(i915: &mut drm_i915_private) {
    intel_de_rmw(i915, GEN9_CLKGATE_DIS_0, 0, DARBF_GATING_DIS);
}

/// `intel_crtc_update_active_timings`. The vblank timestamping constants
/// belong to DRM's vblank code, which comes with interrupts.
fn intel_crtc_update_active_timings(i915: &mut drm_i915_private, c: usize, vrr_enable: bool) {
    let cs = i915.display.crtcs[c].state;
    let mut mode_flags = cs.mode_flags;

    // intel_crtc_active_timings
    let vmax_vblank_start = 0;
    if vrr_enable {
        if mode_flags & I915_MODE_FLAG_VRR == 0 {
            crate::linux::say("warning: VRR on without the VRR mode flag");
        }
        crate::linux::say("warning: VRR active timings are not ported");
    } else {
        mode_flags &= !I915_MODE_FLAG_VRR;
    }

    let crtc = &mut i915.display.crtcs[c];
    crtc.vmax_vblank_start = vmax_vblank_start;
    crtc.mode_flags = mode_flags;
    // intel_crtc_scanline_offset (display version 9+)
    crtc.scanline_offset = if cs.has_type(INTEL_OUTPUT_HDMI) { 2 } else { 1 };
}

/// `intel_crtc_joiner_secondary_pipes`.
fn intel_crtc_joiner_secondary_pipes(cs: &intel_crtc_state) -> u8 {
    if cs.joiner_pipes == 0 {
        return 0;
    }
    let primary = cs.joiner_pipes.trailing_zeros();
    cs.joiner_pipes & !(1 << primary)
}

/// `intel_modeset_readout_hw_state`. `intel_pmdemand_*` is display
/// version 14+.
fn intel_modeset_readout_hw_state(i915: &mut drm_i915_private) {
    let pipes = i915.display.runtime.pipe_mask;

    // The crtcs (state reset, get_pipe_config, hw.enable), then the planes.
    intel_crtc::intel_modeset_readout_crtcs(i915);
    for c in crtcs(i915) {
        let crtc = &mut i915.display.crtcs[c];
        crtc.enabled = crtc.state.hw.enable;
        crtc.active = crtc.state.hw.active;
    }

    let n_encoders = i915.display.outputs.n_encoders;
    for e in 0..n_encoders {
        let o = i915.display.outputs;
        let mut crtc_state_of: Option<usize> = None;
        let mut pipe_seen = 0u32;

        match intel_ddi::intel_encoder_get_hw_state(i915, &o, e, pipes) {
            Some(pipe) => {
                pipe_seen = pipe;
                if let Some(c) = intel_display::intel_crtc_for_pipe(i915, pipe) {
                    i915.display.outputs.encoders[e].crtc = Some(c);
                    intel_ddi::intel_encoder_get_config(i915, e, c);

                    // read out to secondary crtc as well for joiner
                    let cs = i915.display.crtcs[c].state;
                    if cs.joiner_pipes != 0 {
                        if intel_crtc::intel_crtc_is_joiner_secondary(&cs, pipe) {
                            crate::linux::say("warning: encoder linked to a joiner secondary");
                        }
                        let secondaries = intel_crtc_joiner_secondary_pipes(&cs);
                        for s in crtcs(i915) {
                            if secondaries & (1 << i915.display.crtcs[s].pipe) != 0 {
                                intel_ddi::intel_encoder_get_config(i915, e, s);
                            }
                        }
                    }
                    crtc_state_of = Some(c);
                }
            }
            None => {
                i915.display.outputs.encoders[e].crtc = None;
            }
        }

        // encoder->sync_state: the DDI encoders have one.
        if matches!(i915.display.outputs.encoders[e].kind, encoder_kind::ddi(_)) {
            let mut o = i915.display.outputs;
            let vbt = i915.display.bios;
            let cs = crtc_state_of.map(|c| i915.display.crtcs[c].state);
            intel_ddi::intel_ddi_sync_state(i915, &mut o, &vbt, e, cs.as_ref());
            i915.display.outputs = o;
        }

        let enc = i915.display.outputs.encoders[e];
        Line::new().s("[ENCODER:").s(enc.name()).s("] hw state readout: ")
            .s(if enc.crtc.is_some() { "enabled" } else { "disabled" }).s(", pipe ")
            .dec(pipe_seen as u64).say();
    }

    // intel_dpll_readout_hw_state: a PLL belongs to every active crtc whose
    // state names it.
    let mut pll_pipes = [0u32; I915_NUM_PLLS as usize];
    for c in crtcs(i915) {
        let cs = &i915.display.crtcs[c].state;
        if let (true, Some(p)) = (cs.hw.active, cs.intel_dpll) {
            pll_pipes[p] |= 1 << i915.display.crtcs[c].pipe;
        }
    }
    let vbt = i915.display.bios.vbt;
    intel_dpll_mgr::intel_dpll_readout_hw_state(i915, &vbt, &|idx| pll_pipes[idx]);

    let n_connectors = i915.display.outputs.n_connectors;
    for k in 0..n_connectors {
        let o = i915.display.outputs;
        let mut crtc_state_of: Option<usize> = None;

        if intel_ddi::intel_ddi_connector_get_hw_state(i915, &o, k, pipes) {
            let encoder = o.connectors[k].encoder;
            let con = &mut i915.display.outputs.connectors[k];
            con.dpms_on = true;
            con.attached = Some(encoder);

            if let Some(c) = o.encoders[encoder].crtc {
                crtc_state_of = Some(c);
                let cs = &mut i915.display.crtcs[c].state;
                if cs.hw.active {
                    // .crtc_disable relies on an accurate connector mask.
                    cs.uapi_connector_mask |= 1 << k;
                    cs.uapi_encoder_mask |= 1 << encoder;
                }
            }
        } else {
            let con = &mut i915.display.outputs.connectors[k];
            con.dpms_on = false;
            con.attached = None;
        }

        // connector->sync_state: intel_dp_connector_sync_state on DP.
        let con = &mut i915.display.outputs.connectors[k];
        if con.connector_type == DRM_MODE_CONNECTOR_DisplayPort || con.connector_type == DRM_MODE_CONNECTOR_eDP {
            con.dsc_decompression_enabled =
                crtc_state_of.is_some_and(|c| i915.display.crtcs[c].state.dsc.compression_enable);
        }

        Line::new().s("[CONNECTOR:").dec(k as u64).s("] hw state readout: ")
            .s(if i915.display.outputs.connectors[k].attached.is_some() { "enabled" } else { "disabled" }).say();
    }

    for c in crtcs(i915) {
        // The initial mode keeps the atomic core happy; the derived state is
        // recomputed on the next commit.
        i915.display.crtcs[c].state.inherited = true;

        let cs = i915.display.crtcs[c].state;
        if cs.hw.active {
            intel_crtc_update_active_timings(i915, c, cs.vrr.enable);
            intel_crtc::intel_crtc_copy_hw_to_uapi_state(i915, c);
        }

        let pipe = i915.display.crtcs[c].pipe;
        for p in 0..i915.display.num_planes {
            let plane = i915.display.planes[p];
            if plane.pipe != pipe || !plane.state.visible {
                continue;
            }
            let cs = &mut i915.display.crtcs[c].state;
            // No framebuffer yet, so 4 bytes per pixel and half the pixel
            // rate (display version 10+) stand in for the plane's own.
            cs.data_rate[plane.id as usize] = 4 * cs.pixel_rate;
            if plane.kind == plane_kind::skl_universal {
                cs.min_cdclk[plane.id as usize] = cs.pixel_rate.div_ceil(2) as i32;
            }
            Line::new().s("plane ").dec(plane.id as u64 + 1).s(" pipe ").dec(pipe as u64).s(": min_cdclk ")
                .dec(cs.min_cdclk[plane.id as usize] as u64).s(" kHz").say();
        }
    }

    crate::skl_watermark::skl_wm_get_hw_state(i915);
    crate::intel_bw::intel_bw_update_hw_state(i915);
    crate::intel_cdclk::intel_cdclk_update_hw_state(i915);
}

/// `get_encoder_power_domains`: inactive encoders hold none.
fn get_encoder_power_domains(i915: &mut drm_i915_private) {
    for e in 0..i915.display.outputs.n_encoders {
        let enc = i915.display.outputs.encoders[e];
        if !matches!(enc.kind, encoder_kind::ddi(_)) {
            continue;
        }
        let Some(c) = enc.crtc else { continue };
        let cs = i915.display.crtcs[c].state;
        let mut o = i915.display.outputs;
        intel_ddi::intel_ddi_get_power_domains(i915, &mut o, e, &cs);
        i915.display.outputs = o;
    }
}

/// `intel_encoder_find_connector`.
fn intel_encoder_find_connector(o: &intel_outputs, encoder: usize) -> Option<usize> {
    (0..o.n_connectors).find(|&k| o.connectors[k].attached == Some(encoder))
}

/// `intel_sanitize_encoder`. `has_bogus_dpll_config` is Sandy Bridge only.
fn intel_sanitize_encoder(i915: &mut drm_i915_private, opregion: Option<&intel_opregion>, e: usize) {
    let enc = i915.display.outputs.encoders[e];
    let crtc = enc.crtc;
    // Both the crtc link and the pipe itself must be active.
    let has_active_crtc = crtc.is_some_and(|c| i915.display.crtcs[c].state.hw.active);

    let connector = intel_encoder_find_connector(&i915.display.outputs, e);
    if let (Some(k), false) = (connector, has_active_crtc) {
        Line::new().s("[ENCODER:").s(enc.name()).s("] has active connectors but no active pipe!").say();
        if crtc.is_some() {
            Line::new().s("[ENCODER:").s(enc.name())
                .s("] would be disabled by hand (encoder disable/post_disable), not ported").say();
        }
        i915.display.outputs.encoders[e].crtc = None;
        // Clamp the inconsistent state to off.
        let con = &mut i915.display.outputs.connectors[k];
        con.dpms_on = false;
        con.attached = None;
    }

    // notify opregion of the sanitized encoder state
    if let Some(op) = opregion {
        let _ = op.intel_opregion_notify_encoder(&enc, connector.is_some() && has_active_crtc);
    }

    let o = i915.display.outputs;
    let has_crtc = o.encoders[e].crtc.is_some();
    intel_ddi::intel_ddi_sanitize_encoder_pll_mapping(i915, &o, e, has_crtc, i915.display.runtime.pipe_mask);
}

/// `intel_modeset_update_connector_atomic_state` with
/// `set_encoder_for_connector`.
fn intel_modeset_update_connector_atomic_state(i915: &mut drm_i915_private) {
    for k in 0..i915.display.outputs.n_connectors {
        let con = i915.display.outputs.connectors[k];
        match con.attached {
            Some(e) => {
                let crtc = i915.display.outputs.encoders[e].crtc;
                let con = &mut i915.display.outputs.connectors[k];
                con.state_crtc = crtc;
                if let Some(c) = crtc {
                    let pipe_bpp = i915.display.crtcs[c].state.pipe_bpp;
                    let con = &mut i915.display.outputs.connectors[k];
                    con.max_bpc = ((if pipe_bpp != 0 { pipe_bpp } else { 24 }) / 3) as u8;
                }
            }
            None => i915.display.outputs.connectors[k].state_crtc = None,
        }
    }
}

/// `intel_crtc_needs_link_reset`.
fn intel_crtc_needs_link_reset(i915: &mut drm_i915_private, c: usize) -> bool {
    for e in 0..i915.display.outputs.n_encoders {
        if i915.display.outputs.encoders[e].crtc != Some(c) {
            continue;
        }
        let mut o = i915.display.outputs;
        let Some(d) = o.dig_port_of(e) else { continue };
        let needs = crate::intel_tc::intel_tc_port_link_needs_reset(i915, &mut o, d);
        i915.display.outputs = o;
        if needs {
            return true;
        }
    }
    false
}

/// `intel_crtc_has_encoders`.
fn intel_crtc_has_encoders(i915: &drm_i915_private, c: usize) -> bool {
    let o = &i915.display.outputs;
    (0..o.n_encoders).any(|e| o.encoders[e].crtc == Some(c))
}

/// `intel_sanitize_crtc`; returns whether the crtc was forced off, which
/// never happens here (see the module comment).
fn intel_sanitize_crtc(i915: &mut drm_i915_private, c: usize) -> bool {
    let cs = i915.display.crtcs[c].state;
    let pipe = i915.display.crtcs[c].pipe;

    if cs.hw.active {
        // Disable everything but the primary plane
        for p in 0..i915.display.num_planes {
            let plane = i915.display.planes[p];
            if plane.pipe == pipe && plane.state.visible && plane.id != PLANE_1 {
                Line::new().s("plane ").dec(plane.id as u64 + 1).s(" pipe ").dec(pipe as u64)
                    .s(": Linux disables it here (intel_plane_disable_noatomic), not ported").say();
            }
        }

        // Disable any background color/etc. set by the BIOS
        crate::intel_color::intel_color_commit_noarm(i915, c);
        crate::intel_color::intel_color_commit_arm(i915, c);
    }

    if !cs.hw.active || intel_crtc::intel_crtc_is_joiner_secondary(&cs, pipe) {
        return false;
    }

    let needs_link_reset = intel_crtc_needs_link_reset(i915, c);

    // The pipe stays as it is while it has encoders and needs no link reset.
    if !needs_link_reset && intel_crtc_has_encoders(i915, c) {
        return false;
    }

    Line::new().s("pipe ").dec(pipe as u64).s(": Linux disables it here (")
        .s(if needs_link_reset { "TypeC link needs a reset" } else { "no encoder" })
        .s(", intel_crtc_disable_noatomic), not ported").say();
    false
}

/// `intel_crtc_state_dump`, the lines that say what the pipe runs.
fn intel_crtc_state_dump(i915: &drm_i915_private, c: usize) {
    let crtc = &i915.display.crtcs[c];
    let cs = &crtc.state;
    if !cs.hw.active {
        Line::new().s("pipe ").dec(crtc.pipe as u64).s(": setup_hw_state: disabled").say();
        return;
    }
    let m = &cs.hw.adjusted_mode;
    let mut l = Line::new();
    l.s("pipe ").dec(crtc.pipe as u64).s(": setup_hw_state: ").dec(m.hdisplay as u64).s("x")
        .dec(m.vdisplay as u64).s(", dotclock ").dec(m.crtc_clock as u64).s(" kHz, transcoder ")
        .dec(cs.cpu_transcoder as u64).s(", port_clock ").dec(cs.port_clock as u64).s(", lanes ")
        .dec(cs.lane_count as u64).s(", bpp ").dec(cs.pipe_bpp as u64).s(", output_types ")
        .hex(cs.output_types);
    match cs.intel_dpll {
        Some(p) => l.s(", PLL ").s(intel_dpll_mgr::dpll_name(&i915.display.dpll.dplls[p])),
        None => l.s(", no PLL"),
    };
    let o = &i915.display.outputs;
    for e in 0..o.n_encoders {
        if cs.uapi_encoder_mask & (1 << e) != 0 {
            l.s(", [ENCODER:").s(o.encoders[e].name()).s("]");
        }
    }
    l.say();
    if let Some(p) = cs.intel_dpll {
        let pll = &i915.display.dpll.dplls[p];
        if pll.info.map(|i| i.funcs) != Some(intel_dpll_mgr::dpll_info_funcs::tbt_pll_funcs) {
            Line::new().s("pipe ").dec(crtc.pipe as u64).s(": ").s(intel_dpll_mgr::dpll_name(pll)).s(" at ")
                .dec(intel_dpll_mgr::intel_dpll_get_freq(i915, pll, &cs.dpll_hw_state) as u64).s(" kHz").say();
        }
        intel_dpll_mgr::intel_dpll_dump_hw_state(&cs.dpll_hw_state);
    }
    Line::new().s("pipe ").dec(crtc.pipe as u64).s(": m_n tu ").dec(cs.dp_m_n.tu as u64).s(" data ")
        .dec(cs.dp_m_n.data_m as u64).s("/").dec(cs.dp_m_n.data_n as u64).s(" link ")
        .dec(cs.dp_m_n.link_m as u64).s("/").dec(cs.dp_m_n.link_n as u64).s(", enhanced framing ")
        .dec(cs.enhanced_framing as u64).s(", fec ").dec(cs.fec_enable as u64).s(", audio ")
        .dec(cs.has_audio as u64).s(", infoframes ").hex(cs.infoframes.enable).s(", min voltage ")
        .dec(cs.min_voltage_level as u64).say();
}

/// `intel_sanitize_all_crtcs`. A crtc forced off can unstick the HPD state
/// of other TypeC ports, so the pass repeats while it forces more off.
fn intel_sanitize_all_crtcs(i915: &mut drm_i915_private) {
    let mut crtcs_forced_off = 0u32;
    loop {
        let old_mask = crtcs_forced_off;
        for c in crtcs(i915) {
            let crtc_mask = 1 << c;
            if crtcs_forced_off & crtc_mask != 0 {
                continue;
            }
            if intel_sanitize_crtc(i915, c) {
                crtcs_forced_off |= crtc_mask;
            }
        }
        if crtcs_forced_off == old_mask {
            break;
        }
    }

    for c in crtcs(i915) {
        intel_crtc_state_dump(i915, c);
    }
}

/// `get_crtc_power_domains`.
fn get_crtc_power_domains(i915: &drm_i915_private, c: usize) -> [u64; DOMAIN_WORDS] {
    let mut mask = [0u64; DOMAIN_WORDS];
    let crtc = &i915.display.crtcs[c];
    let cs = &crtc.state;
    let mut set = |d: u32| mask[(d / 64) as usize] |= 1 << (d % 64);

    if !cs.hw.active {
        return mask;
    }

    set(POWER_DOMAIN_PIPE(crtc.pipe));
    set(POWER_DOMAIN_TRANSCODER(cs.cpu_transcoder));
    if cs.pch_pfit.enabled || cs.pch_pfit.force_thru {
        set(POWER_DOMAIN_PIPE_PANEL_FITTER(crtc.pipe));
    }

    let o = &i915.display.outputs;
    for e in 0..o.n_encoders {
        if cs.uapi_encoder_mask & (1 << e) != 0 {
            set(o.encoders[e].power_domain);
        }
    }

    if cs.has_audio {
        set(POWER_DOMAIN_AUDIO_MMIO);
    }
    if cs.intel_dpll.is_some() {
        set(POWER_DOMAIN_DISPLAY_CORE);
    }
    if cs.dsc.compression_enable {
        set(crate::intel_vdsc::intel_dsc_power_domain(crtc.pipe, cs.cpu_transcoder));
    }
    mask
}

/// `intel_modeset_get_crtc_power_domains`: takes the domains the crtc needs
/// and does not hold yet; returns the ones it holds and no longer needs.
fn intel_modeset_get_crtc_power_domains(i915: &mut drm_i915_private, c: usize) -> [u64; DOMAIN_WORDS] {
    let domains = get_crtc_power_domains(i915, c);
    let enabled = i915.display.crtcs[c].enabled_power_domains;
    let mut new_domains = [0u64; DOMAIN_WORDS];
    let mut old_domains = [0u64; DOMAIN_WORDS];
    for w in 0..DOMAIN_WORDS {
        new_domains[w] = domains[w] & !enabled.mask[w];
        old_domains[w] = enabled.mask[w] & !domains[w];
    }

    let mut set = enabled;
    for domain in 0..POWER_DOMAIN_NUM {
        if new_domains[(domain / 64) as usize] & (1 << (domain % 64)) != 0 {
            intel_display_power_get_in_set(i915, &mut set, domain);
        }
    }
    i915.display.crtcs[c].enabled_power_domains = set;
    old_domains
}

/// `intel_modeset_setup_hw_state`. `intel_pch_sanitize` acts on IBX only,
/// `intel_cmtg_sanitize` from display version 13, `intel_sanitize_plane_mapping`
/// below version 4, and `intel_dmc_enable_pipe` needs a DMC payload.
pub fn intel_modeset_setup_hw_state(i915: &mut drm_i915_private, opregion: Option<&intel_opregion>) {
    intel_display_power_get(i915, POWER_DOMAIN_INIT);

    intel_early_display_was(i915);
    crate::intel_vga::intel_vga_disable(i915);

    intel_modeset_readout_hw_state(i915);

    // HW state is read out, now sanitize it.
    get_encoder_power_domains(i915);

    // Vblank interrupts come back before anything below may wait for one.
    // drm_crtc_vblank_reset finds the vblank state fresh; FIFO underrun
    // reporting starts off and stays so until a commit.
    for c in 0..I915_MAX_PIPES as usize {
        if i915.display.crtc_valid[c] && i915.display.crtcs[c].state.hw.active {
            let pipe = i915.display.crtcs[c].pipe;
            crate::intel_display_irq::drm_crtc_vblank_on(i915, pipe);
        }
    }

    crate::intel_fbc::intel_fbc_sanitize(i915);

    for e in 0..i915.display.outputs.n_encoders {
        intel_sanitize_encoder(i915, opregion, e);
    }

    // Sanitizing the crtcs needs the connector state up to date.
    intel_modeset_update_connector_atomic_state(i915);

    intel_sanitize_all_crtcs(i915);

    intel_dpll_mgr::intel_dpll_sanitize_state(i915);

    crate::skl_watermark::intel_wm_sanitize(i915);

    for c in crtcs(i915) {
        let put_domains = intel_modeset_get_crtc_power_domains(i915, c);
        if put_domains.iter().any(|&w| w != 0) {
            crate::linux::say("warning: crtc held power domains it does not need");
            let mut set = i915.display.crtcs[c].enabled_power_domains;
            intel_display_power_put_mask_in_set(i915, &mut set, &put_domains);
            i915.display.crtcs[c].enabled_power_domains = set;
        }
    }

    intel_display_power_put(i915, POWER_DOMAIN_INIT);

    intel_power_domains_sanitize_state(i915);
}
