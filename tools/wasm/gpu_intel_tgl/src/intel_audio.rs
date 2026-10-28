//! display/intel_audio.c (Linux 6.18.55): the display side of HD audio on
//! Tiger Lake, the hsw functions with the DisplayPort config update and
//! the DSC workaround, and the audio component's power and codec-wake
//! operations.
//!
//! The ELD reaches the audio driver through the component, never through
//! the hardware ELD buffer. HDMI's N/CTS table, the codec disable and the
//! LPE audio bridge are not ported (no HDMI output is driven here).

use crate::i915_drv::drm_i915_private;
use crate::intel_de::*;
use crate::linux::{self, Line};
use crate::regs::*;

const ELD_BYTES: usize = MAX_ELD_BYTES as usize;

/// `display->audio.state[cpu_transcoder]`.
#[derive(Clone, Copy)]
pub struct intel_audio_state {
    pub encoder: Option<usize>,
    pub eld: [u8; ELD_BYTES],
}

impl Default for intel_audio_state {
    fn default() -> Self {
        intel_audio_state { encoder: None, eld: [0; ELD_BYTES] }
    }
}

/// `display->audio`.
#[derive(Clone, Copy, Default)]
pub struct intel_audio {
    pub freq_cntrl: u32,
    pub power_refcount: i32,
    /// The audio driver's last request carried out, and what it holds.
    pub component_done: u32,
    pub component_bits: u32,
    pub state: [intel_audio_state; I915_MAX_TRANSCODERS as usize],
}

/// `intel_audio_component_init`; `intel_audio_cdclk_change_post` has
/// nothing to do below display version 13.
pub fn intel_audio_init(i915: &mut drm_i915_private) {
    let aud_freq_init = intel_de_read(i915, AUD_FREQ_CNTRL);
    // Use the BIOS provided value for TGL unless it is a known bad value.
    let aud_freq = if aud_freq_init as u64 != AUD_FREQ_TGL_BROKEN { aud_freq_init } else { AUD_FREQ_GEN12 as u32 };
    i915.display.audio.freq_cntrl = aud_freq;
}

/// `intel_audio_component_get_power`. No GLK CDCLK forcing on TGL.
pub fn intel_audio_component_get_power(i915: &mut drm_i915_private) {
    crate::intel_display_power::intel_display_power_get(i915, POWER_DOMAIN_AUDIO_PLAYBACK);

    let first = i915.display.audio.power_refcount == 0;
    i915.display.audio.power_refcount += 1;
    if first {
        let freq = i915.display.audio.freq_cntrl;
        intel_de_write(i915, AUD_FREQ_CNTRL, freq);
        intel_de_rmw(i915, AUD_PIN_BUF_CTL, 0, AUD_PIN_BUF_ENABLE);
    }
}

/// `intel_audio_component_put_power`.
pub fn intel_audio_component_put_power(i915: &mut drm_i915_private) {
    i915.display.audio.power_refcount -= 1;
    crate::intel_display_power::intel_display_power_put(i915, POWER_DOMAIN_AUDIO_PLAYBACK);
}

/// `intel_audio_component_codec_wake_override`: enable or disable the codec
/// wake signal, overriding the internal logic that generates it to the
/// controller.
pub fn intel_audio_component_codec_wake_override(i915: &mut drm_i915_private, enable: bool) {
    intel_audio_component_get_power(i915);

    intel_de_rmw(i915, HSW_AUD_CHICKENBIT, SKL_AUD_CODEC_WAKE_SIGNAL, 0);
    linux::udelay(1000);

    if enable {
        intel_de_rmw(i915, HSW_AUD_CHICKENBIT, 0, SKL_AUD_CODEC_WAKE_SIGNAL);
        linux::udelay(1000);
    }

    intel_audio_component_put_power(i915);
}

/// Bits of an audio driver's request (docs/spec/AUDIO.md).
const COMPONENT_POWER: u32 = 1;
const COMPONENT_WAKE: u32 = 2;

/// The audio component's ops as the audio driver calls them: what it wants
/// now becomes `get_power` / `put_power` and `codec_wake_override`, then the
/// request is reported done. Called on every pass of the service loop; the
/// first call binds the component.
pub fn intel_audio_component_serve(i915: &mut drm_i915_private) {
    let want = npk_sys::display_audio_serve(i915.display.audio.component_done as i32);
    if want < 0 {
        return;
    }
    let (n, bits) = (want as u32 >> 2, want as u32 & (COMPONENT_POWER | COMPONENT_WAKE));
    if n == i915.display.audio.component_done {
        return;
    }
    let had = i915.display.audio.component_bits;
    if bits & COMPONENT_POWER != 0 && had & COMPONENT_POWER == 0 {
        intel_audio_component_get_power(i915);
    }
    if (bits ^ had) & COMPONENT_WAKE != 0 {
        intel_audio_component_codec_wake_override(i915, bits & COMPONENT_WAKE != 0);
    }
    if bits & COMPONENT_POWER == 0 && had & COMPONENT_POWER != 0 {
        intel_audio_component_put_power(i915);
    }
    i915.display.audio.component_bits = bits;
    i915.display.audio.component_done = n;
    npk_sys::display_audio_serve(n as i32);
    Line::new().s("audio: component request ").dec(n as u64).s(", power ").dec((bits & COMPONENT_POWER) as u64)
        .s(", codec wake ").dec((bits & COMPONENT_WAKE != 0) as u64).dbg();
}

/// `intel_audio_component_get_cdclk_freq`, in kHz.
pub fn intel_audio_component_get_cdclk_freq(i915: &drm_i915_private) -> u32 {
    i915.display.cdclk.hw.cdclk
}

/// `drm_av_sync_delay` (drm_edid.c), in ms; progressive modes only.
fn drm_av_sync_delay(lat: &crate::drm_edid::av_latency, interlaced: bool) -> i32 {
    let mut i = interlaced as usize;
    if !lat.latency_present[0] {
        return 0;
    }
    if !lat.latency_present[1] {
        i = 0;
    }
    let mut a = lat.audio_latency[i];
    let mut v = lat.video_latency[i];

    // The sink doesn't support audio or video.
    if a == 255 || v == 255 {
        return 0;
    }

    // Convert raw EDID values to millisecond; unknown latency is 0 ms.
    if a != 0 {
        a = (2 * (a - 1)).min(500);
    }
    if v != 0 {
        v = (2 * (v - 1)).min(500);
    }
    (v - a).max(0)
}

/// `intel_audio_compute_config`: the connector's ELD with the A/V sync
/// delay, or None for a bogus ELD.
pub fn intel_audio_compute_config(i915: &drm_i915_private, connector: usize, interlaced: bool)
                                  -> Option<[u8; ELD_BYTES]> {
    let a = &i915.display.connector_audio[connector];
    if a.eld[0] == 0 {
        return None;
    }
    let mut eld = a.eld;
    eld[6] = (drm_av_sync_delay(&a.latency, interlaced) / 2) as u8;
    Some(eld)
}

/// `calc_hblank_early_prog`.
fn calc_hblank_early_prog(i915: &drm_i915_private, cs: &crate::intel_display_types::intel_crtc_state) -> u32 {
    let m = &cs.hw.adjusted_mode;
    let h_active = m.crtc_hdisplay as u64;
    let h_total = m.crtc_htotal as u64;
    let pixel_clk = m.crtc_clock as u64;
    let vdsc_bppx16 = cs.dsc.compressed_bpp_x16 as u64;
    let cdclk = i915.display.cdclk.hw.cdclk as u64;
    // fec = 0.972261, using rounding multiplier of 1000000
    let fec_coeff = 972261u64;
    let link_clk = cs.port_clock as u64;
    let lanes = cs.lane_count as u64;

    if link_clk == 0 || pixel_clk == 0 || lanes == 0 || vdsc_bppx16 == 0 || cdclk == 0 {
        linux::say("warning: audio DSC workaround without link, pixel clock, lanes, DSC bpp or CDCLK");
        return 0;
    }

    let link_clks_available = ((h_total - h_active) * link_clk / pixel_clk) as u32 as u64 - 28;
    let link_clks_required = (192000 * h_total).div_ceil(1000 * pixel_clk) * (48 / lanes + 2);

    let hblank_delta = if link_clks_available > link_clks_required {
        32
    } else {
        (5 * (link_clk + cdclk) * pixel_clk).div_ceil(link_clk * cdclk)
    };

    let tu_data = (pixel_clk * vdsc_bppx16 * 8 * 1000000) / (link_clk * lanes * 16 * fec_coeff);
    let tu_line = (h_active * link_clk * fec_coeff) / (64 * pixel_clk * 1000000);
    let link_clks_active = (tu_line - 1) * 64 + tu_data;

    let hblank_rise = (link_clks_active + 6 * link_clks_active.div_ceil(250) + 4) * pixel_clk / link_clk;

    (h_active - hblank_rise + hblank_delta) as u32
}

/// `calc_samples_room`.
fn calc_samples_room(cs: &crate::intel_display_types::intel_crtc_state) -> u32 {
    let m = &cs.hw.adjusted_mode;
    let h_active = m.hdisplay as u32;
    let h_total = m.htotal as u32;
    let pixel_clk = m.clock as u32;
    let link_clk = cs.port_clock as u32;
    let lanes = cs.lane_count as u32;
    ((h_total - h_active) * link_clk - 12 * pixel_clk) / (pixel_clk * (48 / lanes + 2))
}

/// `enable_audio_dsc_wa` (display version 12).
fn enable_audio_dsc_wa(i915: &mut drm_i915_private, cs: &crate::intel_display_types::intel_crtc_state) {
    let cpu_transcoder = cs.cpu_transcoder;
    let mut val = intel_de_read(i915, AUD_CONFIG_BE);

    val |= HBLANK_EARLY_ENABLE_TGL(cpu_transcoder);

    let m = &cs.hw.adjusted_mode;
    if cs.dsc.compression_enable && m.hdisplay >= 3840 && m.vdisplay >= 2160 {
        // Get hblank early enable value required.
        val &= !HBLANK_START_COUNT_MASK(cpu_transcoder);
        let hblank_early_prog = calc_hblank_early_prog(i915, cs);
        val |= if hblank_early_prog < 32 {
            HBLANK_START_COUNT(cpu_transcoder, HBLANK_START_COUNT_32)
        } else if hblank_early_prog < 64 {
            HBLANK_START_COUNT(cpu_transcoder, HBLANK_START_COUNT_64)
        } else if hblank_early_prog < 96 {
            HBLANK_START_COUNT(cpu_transcoder, HBLANK_START_COUNT_96)
        } else {
            HBLANK_START_COUNT(cpu_transcoder, HBLANK_START_COUNT_128)
        };

        // Get samples room value required.
        val &= !NUMBER_SAMPLES_PER_LINE_MASK(cpu_transcoder);
        let samples_room = calc_samples_room(cs);
        val |= if samples_room < 3 {
            NUMBER_SAMPLES_PER_LINE(cpu_transcoder, samples_room)
        } else {
            // All samples available in buffer.
            NUMBER_SAMPLES_PER_LINE(cpu_transcoder, 0)
        };
    }

    intel_de_write(i915, AUD_CONFIG_BE, val);
}

/// `hsw_dp_audio_config_update`: time stamps on, Maud/Naud from hardware.
fn hsw_dp_audio_config_update(i915: &mut drm_i915_private, cpu_transcoder: u32) {
    intel_de_rmw(i915, HSW_AUD_CFG(cpu_transcoder),
                 AUD_CONFIG_N_VALUE_INDEX | AUD_CONFIG_PIXEL_CLOCK_HDMI_MASK | AUD_CONFIG_UPPER_N_MASK
                     | AUD_CONFIG_LOWER_N_MASK | AUD_CONFIG_N_PROG_ENABLE,
                 AUD_CONFIG_N_VALUE_INDEX);
}

/// `hsw_audio_codec_enable`. No SDP split before DP 2.0 and no
/// Wa_14020863754 on display version 12.
fn hsw_audio_codec_enable(i915: &mut drm_i915_private, c: usize) {
    let cs = i915.display.crtcs[c].state;
    let pipe = i915.display.crtcs[c].pipe;
    let cpu_transcoder = cs.cpu_transcoder;

    // Enable Audio WA for 4k DSC usecases.
    if cs.has_type(INTEL_OUTPUT_DP) {
        enable_audio_dsc_wa(i915, &cs);
    }

    // Enable audio presence detect.
    intel_de_rmw(i915, HSW_AUD_PIN_ELD_CP_VLD, 0, AUDIO_OUTPUT_ENABLE(cpu_transcoder));

    crate::intel_display_irq::intel_crtc_wait_for_next_vblank(i915, pipe);

    // Invalidate ELD.
    intel_de_rmw(i915, HSW_AUD_PIN_ELD_CP_VLD, AUDIO_ELD_VALID(cpu_transcoder), 0);

    // The audio component conveys the ELD instead of the hardware ELD
    // buffer. Enable timestamps.
    if cs.has_dp_encoder() {
        hsw_dp_audio_config_update(i915, cpu_transcoder);
    } else {
        linux::say("warning: HDMI audio config update is not ported");
    }
}

/// `intel_audio_codec_enable`, after the transcoder, the port and the link
/// are up.
pub fn intel_audio_codec_enable(i915: &mut drm_i915_private, c: usize, encoder: usize, eld: &[u8; ELD_BYTES]) {
    let cs = i915.display.crtcs[c].state;
    if !cs.has_audio {
        return;
    }

    hsw_audio_codec_enable(i915, c);

    let t = cs.cpu_transcoder as usize;
    i915.display.audio.state[t] = intel_audio_state { encoder: Some(encoder), eld: *eld };

    // pin_eld_notify: the audio driver learns the port and reads the ELD
    // (`get_eld`) through the kernel.
    let port = i915.display.outputs.encoders[encoder].port;
    let len = crate::drm_edid::drm_eld_size(eld).min(eld.len());
    if npk_sys::display_audio_set(port as i32, &eld[..len]) != 0 {
        Line::new().s("audio: the kernel refused the ELD of port ").dec(port as u64).say();
    }

    Line::new().s("audio: enabled on [ENCODER:").s(i915.display.outputs.encoders[encoder].name())
        .s("], transcoder ").dec(t as u64).s(", ").dec(crate::drm_edid::drm_eld_size(eld) as u64)
        .s(" bytes ELD, AUD_CONFIG_BE ").hex(intel_de_read(i915, AUD_CONFIG_BE))
        .s(", DSC bpp x16 ").dec(cs.dsc.compressed_bpp_x16 as u64).say();
}

/// The audio part of the first commit for every active pipe that drives a
/// DisplayPort sink with audio: `intel_dp_compute_config` sets
/// `has_audio` from the connector's display info and copies the ELD
/// (`intel_audio_compute_config`), the fastset enables the codec
/// (`intel_encoders_audio_enable`). The audio component's power is taken
/// as snd-hda's `snd_hdac_display_power` would while the codec is used.
pub fn intel_audio_initial_commit(i915: &mut drm_i915_private) {
    for c in 0..I915_MAX_PIPES as usize {
        if !i915.display.crtc_valid[c] || !i915.display.crtcs[c].state.hw.active {
            continue;
        }
        let cs = i915.display.crtcs[c].state;
        if !cs.has_dp_encoder() || cs.has_audio {
            continue;
        }
        let o = &i915.display.outputs;
        let Some(conn) = (0..o.n_connectors).find(|&k| {
            o.connectors[k].attached.is_some_and(|e| o.encoders[e].crtc == Some(c))
        }) else {
            continue;
        };
        let encoder = o.connectors[conn].attached.unwrap_or(0);
        if !i915.display.connector_audio[conn].has_audio {
            continue;
        }
        let interlaced = cs.hw.adjusted_mode.flags & DRM_MODE_FLAG_INTERLACE != 0;
        let Some(eld) = intel_audio_compute_config(i915, conn, interlaced) else {
            Line::new().s("audio: bogus ELD on connector ").dec(conn as u64).say();
            continue;
        };
        intel_audio_component_get_power(i915);
        i915.display.crtcs[c].state.has_audio = true;
        intel_audio_codec_enable(i915, c, encoder, &eld);
    }
}
