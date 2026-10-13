//! display/intel_display.c (Linux 6.18.55): the pipe configuration readout
//! of a Tiger Lake display (`hsw_get_pipe_config` is the
//! `get_pipe_config` hook of every display version from 9 on), the derived
//! state and the plane visibility bookkeeping of the readout.

use crate::i915_drv::drm_i915_private;
use crate::intel_de::*;
use crate::intel_display_power::*;
use crate::intel_display_types::*;
use crate::linux::Line;
use crate::regs::*;

/// `REG_FIELD_GET`.
pub const fn REG_FIELD_GET(mask: u32, v: u32) -> u32 {
    (v & mask) >> mask.trailing_zeros()
}

/// `HAS_TRANSCODER`.
fn HAS_TRANSCODER(i915: &drm_i915_private, trans: u32) -> bool {
    i915.display.runtime.cpu_transcoder_mask & (1 << trans) != 0
}

/// `transcoder_is_dsi`.
pub fn transcoder_is_dsi(transcoder: u32) -> bool {
    transcoder == TRANSCODER_DSI_0 || transcoder == TRANSCODER_DSI_1
}

/// `joiner_pipes`: display version 12 can join all four pipes.
fn joiner_pipes(i915: &drm_i915_private) -> u8 {
    let pipes = (1 << PIPE_A) | (1 << PIPE_B) | (1 << PIPE_C) | (1 << PIPE_D);
    (pipes & i915.display.runtime.pipe_mask) as u8
}

/// `HAS_BIGJOINER`: display version 11+ with DSC.
fn HAS_BIGJOINER(i915: &drm_i915_private) -> bool {
    i915.display.runtime.has_dsc
}

/// `transcoder_ddi_func_is_enabled`.
fn transcoder_ddi_func_is_enabled(i915: &mut drm_i915_private, cpu_transcoder: u32) -> bool {
    let power_domain = POWER_DOMAIN_TRANSCODER(cpu_transcoder);
    let mut tmp = 0;
    if intel_display_power_get_if_enabled(i915, power_domain) {
        tmp = intel_de_read(i915, TRANS_DDI_FUNC_CTL(cpu_transcoder));
        intel_display_power_put_async(i915, power_domain);
    }
    tmp & TRANS_DDI_FUNC_ENABLE as u32 != 0
}

/// `enabled_bigjoiner_pipes`. Ultrajoiner (display version 20 discrete)
/// and the uncompressed joiner (version 13+) do not exist on TGL.
fn enabled_bigjoiner_pipes(i915: &mut drm_i915_private, primary_pipes: &mut u8, secondary_pipes: &mut u8) {
    *primary_pipes = 0;
    *secondary_pipes = 0;

    if !HAS_BIGJOINER(i915) {
        return;
    }

    let mask = joiner_pipes(i915);
    for c in 0..I915_MAX_PIPES as usize {
        if !i915.display.crtc_valid[c] {
            continue;
        }
        let pipe = i915.display.crtcs[c].pipe;
        if mask & (1 << pipe) == 0 {
            continue;
        }
        let power_domain = crate::intel_vdsc::intel_dsc_power_domain(pipe, pipe);
        if intel_display_power_get_if_enabled(i915, power_domain) {
            let tmp = intel_de_read(i915, crate::intel_vdsc::ICL_PIPE_DSS_CTL1_of(pipe));
            intel_display_power_put_async(i915, power_domain);
            if tmp & BIG_JOINER_ENABLE == 0 {
                continue;
            }
            if tmp & PRIMARY_BIG_JOINER_ENABLE != 0 {
                *primary_pipes |= 1 << pipe;
            } else {
                *secondary_pipes |= 1 << pipe;
            }
        }
    }
}

/// `expected_secondary_pipes`.
fn expected_secondary_pipes(primary_pipes: u8, num_pipes: i32) -> u8 {
    let mut secondary_pipes = 0u8;
    for i in 1..num_pipes {
        secondary_pipes |= primary_pipes << i;
    }
    secondary_pipes
}

/// `expected_bigjoiner_secondary_pipes`.
fn expected_bigjoiner_secondary_pipes(bigjoiner_primary_pipes: u8) -> u8 {
    expected_secondary_pipes(bigjoiner_primary_pipes, 2)
}

/// `get_joiner_primary_pipe`.
fn get_joiner_primary_pipe(pipe: u32, mut primary_pipes: u8) -> u8 {
    // GENMASK(pipe, 0)
    primary_pipes &= ((1u32 << (pipe + 1)) - 1) as u8;
    if primary_pipes != 0 {
        1 << (7 - primary_pipes.leading_zeros())
    } else {
        0
    }
}

/// `enabled_joiner_pipes`, with only the bigjoiner present.
fn enabled_joiner_pipes(i915: &mut drm_i915_private, pipe: u32, primary_pipe: &mut u8, secondary_pipes: &mut u8) {
    let (mut primary_bigjoiner_pipes, mut secondary_bigjoiner_pipes) = (0u8, 0u8);
    enabled_bigjoiner_pipes(i915, &mut primary_bigjoiner_pipes, &mut secondary_bigjoiner_pipes);

    if primary_bigjoiner_pipes & secondary_bigjoiner_pipes != 0 {
        crate::linux::say("warning: a pipe is both bigjoiner primary and secondary");
    }
    let bigjoiner_pipes = primary_bigjoiner_pipes | secondary_bigjoiner_pipes;

    if secondary_bigjoiner_pipes != expected_bigjoiner_secondary_pipes(primary_bigjoiner_pipes) {
        Line::new().s("Wrong secondary bigjoiner pipes(expected ")
            .hex(expected_bigjoiner_secondary_pipes(primary_bigjoiner_pipes) as u32)
            .s(", current ").hex(secondary_bigjoiner_pipes as u32).s(")").say();
    }

    *primary_pipe = 0;
    *secondary_pipes = 0;

    if bigjoiner_pipes & (1 << pipe) != 0 {
        *primary_pipe = get_joiner_primary_pipe(pipe, primary_bigjoiner_pipes);
        *secondary_pipes = secondary_bigjoiner_pipes & expected_bigjoiner_secondary_pipes(*primary_pipe);
        if expected_bigjoiner_secondary_pipes(*primary_pipe) != *secondary_pipes {
            Line::new().s("Wrong bigjoiner secondary pipes for primary_pipe ").hex(*primary_pipe as u32)
                .s(" (expected ").hex(expected_bigjoiner_secondary_pipes(*primary_pipe) as u32)
                .s(", current ").hex(*secondary_pipes as u32).s(")").say();
        }
    }
}

/// `hsw_panel_transcoders` (display version 11+).
fn hsw_panel_transcoders() -> u8 {
    ((1 << TRANSCODER_EDP) | (1 << TRANSCODER_DSI_0) | (1 << TRANSCODER_DSI_1)) as u8
}

/// `hsw_enabled_transcoders`.
fn hsw_enabled_transcoders(i915: &mut drm_i915_private, crtc_pipe: u32) -> u8 {
    let panel_transcoder_mask = hsw_panel_transcoders();
    let mut enabled_transcoders = 0u8;

    for cpu_transcoder in 0..I915_MAX_TRANSCODERS {
        if !HAS_TRANSCODER(i915, cpu_transcoder) || panel_transcoder_mask & (1 << cpu_transcoder) == 0 {
            continue;
        }
        let power_domain = POWER_DOMAIN_TRANSCODER(cpu_transcoder);
        let mut tmp = 0;
        if intel_display_power_get_if_enabled(i915, power_domain) {
            tmp = intel_de_read(i915, TRANS_DDI_FUNC_CTL(cpu_transcoder));
            intel_display_power_put_async(i915, power_domain);
        }

        if tmp & TRANS_DDI_FUNC_ENABLE as u32 == 0 {
            continue;
        }

        let input = tmp & TRANS_DDI_EDP_INPUT_MASK;
        let trans_pipe = if input == TRANS_DDI_EDP_INPUT_A_ONOFF || input == TRANS_DDI_EDP_INPUT_A_ON {
            PIPE_A
        } else if input == TRANS_DDI_EDP_INPUT_B_ONOFF {
            PIPE_B
        } else if input == TRANS_DDI_EDP_INPUT_C_ONOFF {
            PIPE_C
        } else if input == TRANS_DDI_EDP_INPUT_D_ONOFF {
            PIPE_D
        } else {
            Line::new().s("warning: unknown pipe linked to transcoder ").dec(cpu_transcoder as u64).say();
            PIPE_A
        };

        if trans_pipe == crtc_pipe {
            enabled_transcoders |= 1 << cpu_transcoder;
        }
    }

    // single pipe or joiner primary
    let cpu_transcoder = crtc_pipe;
    if transcoder_ddi_func_is_enabled(i915, cpu_transcoder) {
        enabled_transcoders |= 1 << cpu_transcoder;
    }

    // joiner secondary -> consider the primary pipe's transcoder as well
    let (mut primary_pipe, mut secondary_pipes) = (0u8, 0u8);
    enabled_joiner_pipes(i915, crtc_pipe, &mut primary_pipe, &mut secondary_pipes);
    if secondary_pipes & (1 << crtc_pipe) != 0 {
        let cpu_transcoder = primary_pipe.trailing_zeros();
        if transcoder_ddi_func_is_enabled(i915, cpu_transcoder) {
            enabled_transcoders |= 1 << cpu_transcoder;
        }
    }

    enabled_transcoders
}

fn has_edp_transcoders(enabled_transcoders: u8) -> bool {
    enabled_transcoders & (1 << TRANSCODER_EDP) != 0
}

fn has_dsi_transcoders(enabled_transcoders: u8) -> bool {
    enabled_transcoders & ((1 << TRANSCODER_DSI_0) | (1 << TRANSCODER_DSI_1)) != 0
}

fn has_pipe_transcoders(enabled_transcoders: u8) -> bool {
    enabled_transcoders & !((1 << TRANSCODER_EDP) | (1 << TRANSCODER_DSI_0) | (1 << TRANSCODER_DSI_1)) as u8 != 0
}

/// `assert_enabled_transcoders`.
fn assert_enabled_transcoders(enabled_transcoders: u8) {
    let kinds = has_edp_transcoders(enabled_transcoders) as u32
        + has_dsi_transcoders(enabled_transcoders) as u32
        + has_pipe_transcoders(enabled_transcoders) as u32;
    // Only one type of transcoder please
    if kinds > 1 {
        crate::linux::say("warning: more than one kind of transcoder enabled on a pipe");
    }
    // Only DSI transcoders can be ganged
    if !has_dsi_transcoders(enabled_transcoders) && !enabled_transcoders.is_power_of_two() {
        crate::linux::say("warning: more than one transcoder enabled on a pipe");
    }
}

/// `hsw_get_transcoder_state`.
fn hsw_get_transcoder_state(i915: &mut drm_i915_private, c: usize,
                            power_domain_set: &mut intel_display_power_domain_set) -> bool {
    let pipe = i915.display.crtcs[c].pipe;
    let enabled_transcoders = hsw_enabled_transcoders(i915, pipe);
    if enabled_transcoders == 0 {
        return false;
    }

    assert_enabled_transcoders(enabled_transcoders);

    // Except for DSI there is a single enabled transcoder; DSI takes the
    // first one.
    let cpu_transcoder = enabled_transcoders.trailing_zeros();
    i915.display.crtcs[c].state.cpu_transcoder = cpu_transcoder;

    if !intel_display_power_get_in_set_if_enabled(i915, power_domain_set, POWER_DOMAIN_TRANSCODER(cpu_transcoder)) {
        return false;
    }

    if hsw_panel_transcoders() & (1 << cpu_transcoder) != 0 {
        let tmp = intel_de_read(i915, TRANS_DDI_FUNC_CTL(cpu_transcoder));
        if tmp & TRANS_DDI_EDP_INPUT_MASK == TRANS_DDI_EDP_INPUT_A_ONOFF {
            i915.display.crtcs[c].state.pch_pfit.force_thru = true;
        }
    }

    let tmp = intel_de_read(i915, TRANSCONF(cpu_transcoder));
    tmp & TRANSCONF_ENABLE != 0
}

/// `intel_joiner_get_config`.
fn intel_joiner_get_config(i915: &mut drm_i915_private, c: usize) {
    let pipe = i915.display.crtcs[c].pipe;
    let (mut primary_pipe, mut secondary_pipes) = (0u8, 0u8);
    enabled_joiner_pipes(i915, pipe, &mut primary_pipe, &mut secondary_pipes);

    if (primary_pipe | secondary_pipes) & (1 << pipe) == 0 {
        return;
    }
    i915.display.crtcs[c].state.joiner_pipes = primary_pipe | secondary_pipes;
}

/// `intel_crtc_joined_pipe_mask`.
fn intel_crtc_joined_pipe_mask(state: &intel_crtc_state, pipe: u32) -> u8 {
    (1u8 << pipe) | state.joiner_pipes
}

/// `intel_crtc_num_joined_pipes`.
pub fn intel_crtc_num_joined_pipes(state: &intel_crtc_state, pipe: u32) -> i32 {
    intel_crtc_joined_pipe_mask(state, pipe).count_ones() as i32
}

/// `joiner_primary_pipe`.
fn joiner_primary_pipe(state: &intel_crtc_state, pipe: u32) -> u32 {
    intel_crtc_joined_pipe_mask(state, pipe).trailing_zeros()
}

/// `intel_pipe_is_interlaced` (display version 9+).
fn intel_pipe_is_interlaced(i915: &mut drm_i915_private, cpu_transcoder: u32) -> bool {
    intel_de_read(i915, TRANSCONF(cpu_transcoder)) & TRANSCONF_INTERLACE_MASK_HSW != 0
}

/// `intel_get_transcoder_timings` (display version 12).
fn intel_get_transcoder_timings(i915: &mut drm_i915_private, c: usize) {
    let cpu_transcoder = i915.display.crtcs[c].state.cpu_transcoder;
    let mut m = i915.display.crtcs[c].state.hw.adjusted_mode;

    let tmp = intel_de_read(i915, TRANS_HTOTAL(cpu_transcoder));
    m.crtc_hdisplay = REG_FIELD_GET(HACTIVE_MASK, tmp) as i32 + 1;
    m.crtc_htotal = REG_FIELD_GET(HTOTAL_MASK, tmp) as i32 + 1;

    if !transcoder_is_dsi(cpu_transcoder) {
        let tmp = intel_de_read(i915, TRANS_HBLANK(cpu_transcoder));
        m.crtc_hblank_start = REG_FIELD_GET(HBLANK_START_MASK, tmp) as i32 + 1;
        m.crtc_hblank_end = REG_FIELD_GET(HBLANK_END_MASK, tmp) as i32 + 1;
    }

    let tmp = intel_de_read(i915, TRANS_HSYNC(cpu_transcoder));
    m.crtc_hsync_start = REG_FIELD_GET(HSYNC_START_MASK, tmp) as i32 + 1;
    m.crtc_hsync_end = REG_FIELD_GET(HSYNC_END_MASK, tmp) as i32 + 1;

    let tmp = intel_de_read(i915, TRANS_VTOTAL(cpu_transcoder));
    m.crtc_vdisplay = REG_FIELD_GET(VACTIVE_MASK, tmp) as i32 + 1;
    m.crtc_vtotal = REG_FIELD_GET(VTOTAL_MASK, tmp) as i32 + 1;

    // TGL+ DSI transcoders have TRANS_VBLANK too; Linux does not read it.
    if !transcoder_is_dsi(cpu_transcoder) {
        let tmp = intel_de_read(i915, TRANS_VBLANK(cpu_transcoder));
        m.crtc_vblank_start = REG_FIELD_GET(VBLANK_START_MASK, tmp) as i32 + 1;
        m.crtc_vblank_end = REG_FIELD_GET(VBLANK_END_MASK, tmp) as i32 + 1;
    }
    let tmp = intel_de_read(i915, TRANS_VSYNC(cpu_transcoder));
    m.crtc_vsync_start = REG_FIELD_GET(VSYNC_START_MASK, tmp) as i32 + 1;
    m.crtc_vsync_end = REG_FIELD_GET(VSYNC_END_MASK, tmp) as i32 + 1;

    if intel_pipe_is_interlaced(i915, cpu_transcoder) {
        m.flags |= DRM_MODE_FLAG_INTERLACE;
        m.crtc_vtotal += 1;
        m.crtc_vblank_end += 1;
    }

    i915.display.crtcs[c].state.hw.adjusted_mode = m;
}

/// `intel_joiner_adjust_pipe_src`.
fn intel_joiner_adjust_pipe_src(state: &mut intel_crtc_state, pipe: u32) {
    let num_pipes = intel_crtc_num_joined_pipes(state, pipe);
    if num_pipes == 1 {
        return;
    }
    let primary_pipe = joiner_primary_pipe(state, pipe);
    let width = state.pipe_src.width();
    state.pipe_src.translate_to((pipe as i32 - primary_pipe as i32) * width, 0);
}

/// `intel_get_pipe_src_size`.
fn intel_get_pipe_src_size(i915: &mut drm_i915_private, c: usize) {
    let pipe = i915.display.crtcs[c].pipe;
    let tmp = intel_de_read(i915, PIPESRC(pipe));
    let state = &mut i915.display.crtcs[c].state;
    state.pipe_src = drm_rect::init(0, 0, REG_FIELD_GET(PIPESRC_WIDTH_MASK, tmp) as i32 + 1,
                                    REG_FIELD_GET(PIPESRC_HEIGHT_MASK, tmp) as i32 + 1);
    intel_joiner_adjust_pipe_src(state, pipe);
}

/// `bdw_get_pipe_misc_output_format` (display version below 30).
fn bdw_get_pipe_misc_output_format(i915: &mut drm_i915_private, pipe: u32) -> u32 {
    let tmp = intel_de_read(i915, PIPE_MISC(pipe));
    if tmp & PIPE_MISC_YUV420_ENABLE != 0 {
        // 4:2:0 is supported in full blend mode only.
        if tmp & PIPE_MISC_YUV420_MODE_FULL_BLEND == 0 {
            crate::linux::say("warning: YUV 4:2:0 without full blend");
        }
        INTEL_OUTPUT_FORMAT_YCBCR420
    } else if tmp & PIPE_MISC_OUTPUT_COLORSPACE_YUV != 0 {
        INTEL_OUTPUT_FORMAT_YCBCR444
    } else {
        INTEL_OUTPUT_FORMAT_RGB
    }
}

/// `transcoder_has_vrr` (HAS_VRR: display version 11+).
fn transcoder_has_vrr(cpu_transcoder: u32) -> bool {
    !transcoder_is_dsi(cpu_transcoder)
}

/// `hsw_get_pipe_config` for display version 12 (not GLK/BXT, not HSW/BDW).
fn hsw_get_pipe_config(i915: &mut drm_i915_private, c: usize) -> bool {
    let pipe = i915.display.crtcs[c].pipe;
    let mut domains = i915.display.crtcs[c].hw_readout_power_domains;

    if !intel_display_power_get_in_set_if_enabled(i915, &mut domains, POWER_DOMAIN_PIPE(pipe)) {
        i915.display.crtcs[c].hw_readout_power_domains = domains;
        return false;
    }

    let active = hsw_get_transcoder_state(i915, c, &mut domains);

    if active {
        intel_joiner_get_config(i915, c);
        crate::intel_vdsc::intel_dsc_get_config(i915, c);

        // DSI transcoders have the timing registers from display version 11.
        intel_get_transcoder_timings(i915, c);

        if transcoder_has_vrr(i915.display.crtcs[c].state.cpu_transcoder) {
            crate::intel_vrr::intel_vrr_get_config(i915, c);
        }

        intel_get_pipe_src_size(i915, c);

        let output_format = bdw_get_pipe_misc_output_format(i915, pipe);
        let state = &mut i915.display.crtcs[c].state;
        state.output_format = output_format;
        state.sink_format = output_format;

        crate::intel_color::intel_color_get_config(i915, c);

        let tmp = intel_de_read(i915, WM_LINETIME(pipe));
        i915.display.crtcs[c].state.linetime = REG_FIELD_GET(HSW_LINETIME_MASK, tmp) as u16;

        if intel_display_power_get_in_set_if_enabled(i915, &mut domains, POWER_DOMAIN_PIPE_PANEL_FITTER(pipe)) {
            crate::skl_scaler::skl_scaler_get_config(i915, c);
        }

        // hsw_ips_get_config: IPS is HSW ULT and BDW only.

        let cpu_transcoder = i915.display.crtcs[c].state.cpu_transcoder;
        let pixel_multiplier = if cpu_transcoder != TRANSCODER_EDP && !transcoder_is_dsi(cpu_transcoder) {
            intel_de_read(i915, TRANS_MULT(cpu_transcoder)) + 1
        } else {
            1
        };
        i915.display.crtcs[c].state.pixel_multiplier = pixel_multiplier;

        let framestart_delay = if !transcoder_is_dsi(cpu_transcoder) {
            let tmp = intel_de_read(i915, CHICKEN_TRANS(cpu_transcoder));
            REG_FIELD_GET(HSW_FRAME_START_DELAY_MASK, tmp) + 1
        } else {
            // no idea if this is correct
            1
        };
        i915.display.crtcs[c].state.framestart_delay = framestart_delay;
    }

    intel_display_power_put_all_in_set(i915, &mut domains);
    i915.display.crtcs[c].hw_readout_power_domains = domains;

    active
}

/// `intel_mode_from_crtc_timings`. `drm_mode_set_name` only names the mode.
fn intel_mode_from_crtc_timings(mode: &mut drm_display_mode, timings: &drm_display_mode) {
    mode.hdisplay = timings.crtc_hdisplay;
    mode.htotal = timings.crtc_htotal;
    mode.hsync_start = timings.crtc_hsync_start;
    mode.hsync_end = timings.crtc_hsync_end;

    mode.vdisplay = timings.crtc_vdisplay;
    mode.vtotal = timings.crtc_vtotal;
    mode.vsync_start = timings.crtc_vsync_start;
    mode.vsync_end = timings.crtc_vsync_end;

    mode.flags = timings.flags;
    mode.type_ = DRM_MODE_TYPE_DRIVER;

    mode.clock = timings.crtc_clock;
}

/// `intel_splitter_adjust_timings`.
fn intel_splitter_adjust_timings(state: &intel_crtc_state, mode: &mut drm_display_mode) {
    let overlap = state.splitter.pixel_overlap;
    let n = state.splitter.link_count;
    if !state.splitter.enable {
        return;
    }
    // h_full = (h_segment - pixel_overlap) * link_count
    mode.crtc_hdisplay = (mode.crtc_hdisplay - overlap) * n;
    mode.crtc_hblank_start = (mode.crtc_hblank_start - overlap) * n;
    mode.crtc_hblank_end = (mode.crtc_hblank_end - overlap) * n;
    mode.crtc_hsync_start = (mode.crtc_hsync_start - overlap) * n;
    mode.crtc_hsync_end = (mode.crtc_hsync_end - overlap) * n;
    mode.crtc_htotal = (mode.crtc_htotal - overlap) * n;
    mode.crtc_clock *= n;
}

/// `intel_joiner_adjust_timings`.
fn intel_joiner_adjust_timings(state: &intel_crtc_state, pipe: u32, mode: &mut drm_display_mode) {
    let num_pipes = intel_crtc_num_joined_pipes(state, pipe);
    if num_pipes == 1 {
        return;
    }
    mode.crtc_clock /= num_pipes;
    mode.crtc_hdisplay /= num_pipes;
    mode.crtc_hblank_start /= num_pipes;
    mode.crtc_hblank_end /= num_pipes;
    mode.crtc_hsync_start /= num_pipes;
    mode.crtc_hsync_end /= num_pipes;
    mode.crtc_htotal /= num_pipes;
}

/// `intel_adjusted_rate` (intel_plane.c).
fn intel_adjusted_rate(src: &drm_rect, dst: &drm_rect, rate: u32) -> u32 {
    let src_w = (src.width() >> 16) as u32;
    let src_h = (src.height() >> 16) as u32;
    let mut dst_w = dst.width() as u32;
    let mut dst_h = dst.height() as u32;

    // Downscaling limits the maximum pixel rate
    dst_w = dst_w.min(src_w);
    dst_h = dst_h.min(src_h);

    (rate as u64 * (src_w * src_h) as u64).div_ceil((dst_w * dst_h) as u64) as u32
}

/// `ilk_pipe_pixel_rate`.
fn ilk_pipe_pixel_rate(state: &intel_crtc_state) -> u32 {
    let pixel_rate = state.hw.pipe_mode.crtc_clock as u32;

    // Only IF-ID interlacing is used; PF-ID would change the rate.
    if !state.pch_pfit.enabled {
        return pixel_rate;
    }

    let src = drm_rect::init(0, 0, state.pipe_src.width() << 16, state.pipe_src.height() << 16);
    intel_adjusted_rate(&src, &state.pch_pfit.dst, pixel_rate)
}

/// `intel_crtc_readout_derived_state`.
pub fn intel_crtc_readout_derived_state(state: &mut intel_crtc_state, pipe: u32) {
    // The adjusted_mode crtc timings hold the transcoder timings.
    let mut pipe_mode = state.hw.adjusted_mode;

    // Expand MSO per-segment transcoder timings to full
    intel_splitter_adjust_timings(state, &mut pipe_mode);

    // adjusted_mode normal timings get the full numbers; its crtc timings
    // keep the raw transcoder timings.
    intel_mode_from_crtc_timings(&mut state.hw.adjusted_mode, &pipe_mode);

    // Populate the "user" mode with full numbers
    let mut mode = pipe_mode;
    let timings = mode;
    intel_mode_from_crtc_timings(&mut mode, &timings);
    mode.hdisplay = state.pipe_src.width() * intel_crtc_num_joined_pipes(state, pipe);
    mode.vdisplay = state.pipe_src.height();
    state.hw.mode = mode;

    // Derive per-pipe timings in case joiner is used
    intel_joiner_adjust_timings(state, pipe, &mut pipe_mode);
    let timings = pipe_mode;
    intel_mode_from_crtc_timings(&mut pipe_mode, &timings);
    state.hw.pipe_mode = pipe_mode;

    // intel_crtc_compute_pixel_rate, not GMCH
    state.pixel_rate = ilk_pipe_pixel_rate(state);
}

/// `intel_crtc_get_pipe_config`.
pub fn intel_crtc_get_pipe_config(i915: &mut drm_i915_private, c: usize) -> bool {
    if !hsw_get_pipe_config(i915, c) {
        return false;
    }

    let pipe = i915.display.crtcs[c].pipe;
    let state = &mut i915.display.crtcs[c].state;
    state.hw.active = true;
    intel_crtc_readout_derived_state(state, pipe);
    true
}

/// `intel_set_plane_visible`; `plane_index` is `drm_plane_index`.
pub fn intel_set_plane_visible(i915: &mut drm_i915_private, c: usize, plane_index: usize, visible: bool) {
    i915.display.planes[plane_index].state.visible = visible;
    let crtc_state = &mut i915.display.crtcs[c].state;
    if visible {
        crtc_state.uapi_plane_mask |= 1 << plane_index;
    } else {
        crtc_state.uapi_plane_mask &= !(1 << plane_index);
    }
}

/// `intel_plane_fixup_bitmasks`.
pub fn intel_plane_fixup_bitmasks(i915: &mut drm_i915_private, c: usize) {
    let mask = i915.display.crtcs[c].state.uapi_plane_mask;
    let mut planes = 0u8;
    for p in 0..i915.display.num_planes {
        if mask & (1 << p) != 0 {
            planes |= 1 << i915.display.planes[p].id;
        }
    }
    let state = &mut i915.display.crtcs[c].state;
    state.enabled_planes = planes;
    state.active_planes = planes;
}

/// `intel_crtc_for_pipe`: the index of the crtc driving `pipe`.
pub fn intel_crtc_for_pipe(i915: &drm_i915_private, pipe: u32) -> Option<usize> {
    (0..I915_MAX_PIPES as usize).find(|&c| i915.display.crtc_valid[c] && i915.display.crtcs[c].pipe == pipe)
}

/// `intel_get_m_n`.
fn intel_get_m_n(i915: &mut drm_i915_private, data_m_reg: u32, data_n_reg: u32, link_m_reg: u32,
                 link_n_reg: u32) -> intel_link_m_n {
    intel_link_m_n {
        link_m: intel_de_read(i915, link_m_reg) & DATA_LINK_M_N_MASK,
        link_n: intel_de_read(i915, link_n_reg) & DATA_LINK_M_N_MASK,
        data_m: intel_de_read(i915, data_m_reg) & DATA_LINK_M_N_MASK,
        data_n: intel_de_read(i915, data_n_reg) & DATA_LINK_M_N_MASK,
        tu: REG_FIELD_GET(TU_SIZE_MASK, intel_de_read(i915, data_m_reg)) + 1,
    }
}

/// `intel_cpu_transcoder_get_m1_n1` (display version 5+).
pub fn intel_cpu_transcoder_get_m1_n1(i915: &mut drm_i915_private, transcoder: u32) -> intel_link_m_n {
    intel_get_m_n(i915, PIPE_DATA_M1(transcoder), PIPE_DATA_N1(transcoder), PIPE_LINK_M1(transcoder),
                  PIPE_LINK_N1(transcoder))
}

/// `intel_dotclock_calculate`.
fn intel_dotclock_calculate(link_freq: i32, m_n: &intel_link_m_n) -> i32 {
    // pixel_clock = (m * link_freq * 10) / (n * link_symbol_size)
    if m_n.link_n == 0 {
        return 0;
    }
    let num = m_n.link_m as u64 * (link_freq as u64 * 10);
    let den = m_n.link_n as u64 * crate::intel_dp::intel_dp_link_symbol_size(link_freq) as u64;
    num.div_ceil(den) as i32
}

/// `intel_crtc_dotclock`.
pub fn intel_crtc_dotclock(state: &intel_crtc_state) -> i32 {
    let mut dotclock = if state.has_dp_encoder() {
        intel_dotclock_calculate(state.port_clock, &state.dp_m_n)
    } else if state.has_hdmi_sink && state.pipe_bpp > 24 {
        (state.port_clock * 24 + state.pipe_bpp / 2) / state.pipe_bpp
    } else {
        state.port_clock
    };

    if state.output_format == INTEL_OUTPUT_FORMAT_YCBCR420 && !state.has_dp_encoder() {
        dotclock *= 2;
    }
    if state.pixel_multiplier != 0 {
        dotclock /= state.pixel_multiplier as i32;
    }
    dotclock
}
