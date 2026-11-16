//! The display side of the display-audio component (docs/spec/AUDIO.md
//! "Display audio (component)"): each HDMI port's ELD reported as a probe
//! leaves it (`drm_connector_hdmi_audio_plugged_notify`), and the audio
//! side's stream requests carried out as ASoC calls the HDMI codec:
//! startup and prepare when a stream starts, prepare again for new
//! parameters, shutdown when it ends.
//!
//! As in Linux the audio paths take no runtime-PM reference of their own:
//! a stream starts only on a port whose encoder holds it resumed, and a
//! shutdown after the output went off leaves the registers alone (the
//! next resume resets the port).

use alloc::vec::Vec;
use drm_core::drm_connector::{drm_connector_status, MAX_ELD_BYTES};
use drm_core::drm_edid::drm_eld_size;
use drm_core::linux::Line;
use gpu_bcm2712_core::vc4_drv::Vc4;

use crate::hw::PiHw;

#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn npk_display_audio_params() -> i32;
}

/// `npk_display_audio_params`: `(port << 28) | (channels << 20) | rate`
/// of the current request.
fn display_audio_params() -> i32 {
    // SAFETY: FFI without pointers.
    unsafe { npk_display_audio_params() }
}

/// Bit 2 of a request: a stream runs. Bits 0 and 1 are Intel's.
const COMPONENT_STREAM: u32 = 4;

#[derive(Clone, Copy)]
struct Stream {
    port: usize,
    rate: u32,
    channels: u32,
}

pub struct DisplayAudio {
    /// The ELD last reported per port; None before the first report.
    reported: [Option<Vec<u8>>; 2],
    /// The last request carried out.
    done: u32,
    /// The stream started on a port, holding its runtime PM.
    stream: Option<Stream>,
}

impl DisplayAudio {
    pub fn new() -> Self {
        DisplayAudio { reported: [None, None], done: 0, stream: None }
    }

    /// The ELD of each port that changed since the last report: empty for
    /// no sink and for a DVI sink (no audio, `vc4_hdmi_audio_can_stream`).
    pub fn report(&mut self, vc4: &Vc4) {
        for port in 0..self.reported.len() {
            let c = &vc4.connectors[port];
            let eld: &[u8] = if c.status == drm_connector_status::connector_status_connected && c.display_info.is_hdmi {
                &c.eld[..drm_eld_size(&c.eld).min(MAX_ELD_BYTES)]
            } else {
                &[]
            };
            if self.reported[port].as_deref() == Some(eld) {
                continue;
            }
            if npk_sys::display_audio_set(port as i32, eld) != 0 {
                Line::new().s("audio: the kernel refused the ELD of port ").dec(port as u64).say();
                continue;
            }
            self.reported[port] = Some(eld.to_vec());
            Line::new().s("audio: port ").dec(port as u64).s(", ").dec(eld.len() as u64).s(" bytes ELD").say();
        }
    }

    /// One pass of the service loop: binds the component on the first
    /// call, then carries out a new request and reports it done.
    pub fn serve(&mut self, vc4: &mut Vc4, hw: &mut PiHw) {
        let want = npk_sys::display_audio_serve(self.done as i32);
        if want < 0 {
            return;
        }
        let (n, bits) = (want as u32 >> 2, want as u32 & COMPONENT_STREAM);
        if n == self.done {
            return;
        }

        let wanted = if bits & COMPONENT_STREAM != 0 { params() } else { None };
        if let Some(s) = self.stream
            && wanted.is_none_or(|w| w.port != s.port)
        {
            stop(vc4, hw, s.port);
            self.stream = None;
        }
        if let Some(w) = wanted
            && (self.stream.is_some() || start(vc4, hw, w.port))
        {
            let ret = vc4.audio_prepare(hw, w.port, w.rate, w.channels);
            self.stream = Some(w);
            Line::new().s("audio: port ").dec(w.port as u64).s(", ").dec(w.channels as u64).s(" channels at ")
                .dec(w.rate as u64).s(" Hz").s(if ret != 0 { ", prepare failed" } else { "" }).say();
        }

        self.done = n;
        npk_sys::display_audio_serve(n as i32);
    }
}

/// The parameters of the current request, when they name a port.
fn params() -> Option<Stream> {
    let p = display_audio_params();
    if p < 0 {
        return None;
    }
    let p = p as u32;
    let s = Stream { port: (p >> 28) as usize, channels: (p >> 20) & 0xff, rate: p & 0xf_ffff };
    if s.port >= 2 {
        Line::new().s("audio: no HDMI port ").dec(s.port as u64).say();
        return None;
    }
    Some(s)
}

/// `audio_startup` on `port`; false when the encoder is off or the codec
/// refused.
fn start(vc4: &mut Vc4, hw: &mut PiHw, port: usize) -> bool {
    if !vc4.hdmi[port].pm_runtime_active() {
        Line::new().s("audio: port ").dec(port as u64).s(" has no output, no stream").say();
        return false;
    }
    let ret = vc4.audio_startup(hw, port);
    if ret != 0 {
        Line::new().s("audio: startup on port ").dec(port as u64).s(" failed: -").dec(ret.unsigned_abs() as u64).say();
        return false;
    }
    true
}

/// `audio_shutdown` on `port` while its encoder keeps it resumed.
fn stop(vc4: &mut Vc4, hw: &mut PiHw, port: usize) {
    if vc4.hdmi[port].pm_runtime_active() {
        vc4.audio_shutdown(hw, port);
    }
    Line::new().s("audio: port ").dec(port as u64).s(" stopped").say();
}
