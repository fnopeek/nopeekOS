//! HDMI audio on the Pi 5: the kernel's audio driver for the display's
//! ports, in the place of ALSA's dmaengine PCM, the `iec958` plugin and
//! hdmi-codec. The display module (`gpu_bcm2712`) reports each port's ELD
//! and carries out the codec's `audio_startup`/`prepare`/`audio_shutdown`
//! when asked through the display-audio component (`docs/spec/AUDIO.md`);
//! this side owns the DMA ring (`pi5_hdmi_dma`) and fills it from the
//! mixer as IEC958 subframes (`pi5_core::iec958`).
//!
//! A port with a sink enters the device register as an audio output. The
//! stream runs while that output plays the mix and a stream is open, as an
//! ALSA PCM runs while an application holds it open.

use crate::audio::{self, Encoding, Format};
use crate::devices::{self, Kind, Owner};
use crate::kprintln;
use crate::smp::fiber;
use core::sync::atomic::{AtomicU32, Ordering};
use pi5_core::iec958::{self, Encoder};

const RATE: u32 = 48000;
const CHANNELS: usize = 2;
/// 10 ms periods of 32-bit subframes, four of them in the ring.
const PERIOD_FRAMES: usize = 480;
const PERIOD_BYTES: usize = PERIOD_FRAMES * CHANNELS * 4;
const PERIODS: usize = 4;
/// How long the display driver may take to carry out a request.
const REQUEST_MS: u64 = 1000;
/// A stream stops this long after the last one closed.
const IDLE_STOP_MS: u64 = 2000;
/// How often the ports are looked at while nothing plays.
const POLL_MS: u64 = 200;

static WAKER: AtomicU32 = AtomicU32::new(u32::MAX);
/// The port entered in the register, `NONE` for none.
static ENTERED: AtomicU32 = AtomicU32::new(NONE);
/// The port the stream runs on, `NONE` for none.
static STREAMING: AtomicU32 = AtomicU32::new(NONE);
/// Streams started and refused since boot.
static STARTS: AtomicU32 = AtomicU32::new(0);
static REFUSED: AtomicU32 = AtomicU32::new(0);
const NONE: u32 = u32::MAX;

fn ms(n: u64) -> u64 {
    n * (crate::time::counter_hz() / 1000)
}

fn on_period() {
    let w = WAKER.load(Ordering::Acquire);
    if w != u32::MAX {
        fiber::signal(w, fiber::SIG_EVENT);
    }
}

/// The first port whose sink takes audio (an ELD reported).
fn port_with_sink() -> Option<usize> {
    let mut eld = [0u8; audio::ELD_MAX];
    (0..2).find(|&p| audio::display_audio_get(p, &mut eld).is_some_and(|n| n > 0))
}

/// The display driver carried out request `seq` within `REQUEST_MS`.
fn carried_out(seq: u32) -> bool {
    let until = crate::time::counter() + ms(REQUEST_MS);
    loop {
        if audio::display_audio_done() == Some(seq) {
            return true;
        }
        if crate::time::counter() > until {
            return false;
        }
        let _ = fiber::wait(0, crate::time::counter() + ms(5));
    }
}

struct Stream {
    ring: u64,
    written: u64,
    enc: Encoder,
    mix: alloc::vec::Vec<u8>,
}

impl Stream {
    /// Mix one period into ring slot `written % PERIODS`.
    fn fill_one(&mut self) {
        audio::poll_mix_as(Owner::Kernel, &mut self.mix);
        let mut frames = [0u32; PERIOD_FRAMES * CHANNELS];
        for (i, s) in frames.iter_mut().enumerate() {
            let b = &self.mix[i * 4..i * 4 + 4];
            *s = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        }
        self.enc.encode(&mut frames, CHANNELS);
        let slot = (self.written % PERIODS as u64) as usize;
        let dst = self.ring + (slot * PERIOD_BYTES) as u64;
        for (i, w) in frames.iter().enumerate() {
            // SAFETY: inside the ring `pi5_hdmi_dma::open` returned, which
            // stays allocated until `close`; uncached, so the controller
            // reads what is written.
            unsafe { core::ptr::write_volatile((dst + i as u64 * 4) as *mut u32, *w) };
        }
        self.written += 1;
    }

    /// Keep `PERIODS - 1` periods ahead of the controller.
    fn refill(&mut self) {
        let done = super::pi5_hdmi_dma::periods();
        if self.written < done + 1 {
            // The controller caught up: it plays whatever the slots hold.
            self.written = done + 1;
        }
        while self.written < done + PERIODS as u64 {
            self.fill_one();
        }
    }
}

/// Open the ring, have the display driver prepare the port, start.
fn start(port: usize) -> Option<Stream> {
    let status = iec958::consumer_status(RATE)?;
    let ring = super::pi5_hdmi_dma::open(port, PERIODS, PERIOD_BYTES, on_period)?;
    let mut s = Stream {
        ring,
        written: 0,
        enc: Encoder::new(status),
        mix: alloc::vec![0u8; PERIOD_BYTES],
    };
    for _ in 0..PERIODS - 1 {
        s.fill_one();
    }
    let params = audio::display_audio_pack(port as u32, CHANNELS as u32, RATE);
    let seq = audio::display_audio_stream(Some(params));
    let ok = seq.is_some_and(carried_out);
    if !ok {
        REFUSED.fetch_add(1, Ordering::Relaxed);
        kprintln!("[npk] hdmi audio: the display driver did not prepare HDMI{} ({})", port,
            if seq.is_none() { "no display driver bound" } else { "request not carried out" });
        let _ = audio::display_audio_stream(None);
        super::pi5_hdmi_dma::close();
        return None;
    }
    super::pi5_hdmi_dma::start();
    let _ = fiber::wait(0, crate::time::counter() + ms(100));
    if let Some((mai, sched)) = super::pi5_hdmi_dma::mai_state() {
        kprintln!("[npk] hdmi audio: 100 ms in, DMA periods {}, MAI_CTL {:#06x}, SCHEDULER_CONTROL {:#010x}",
            super::pi5_hdmi_dma::periods(), mai, sched);
    }
    STARTS.fetch_add(1, Ordering::Relaxed);
    STREAMING.store(port as u32, Ordering::Release);
    Some(s)
}

fn stop() {
    STREAMING.store(NONE, Ordering::Release);
    super::pi5_hdmi_dma::stop();
    if let Some(seq) = audio::display_audio_stream(None) {
        let _ = carried_out(seq);
    }
    super::pi5_hdmi_dma::close();
}

fn audio_fiber(_: u64) {
    if let Some(w) = fiber::current_waker() {
        WAKER.store(w, Ordering::Release);
    }
    let fmt = Format { rate: RATE, encoding: Encoding::S32, channels: CHANNELS as u8 };
    audio::set_device_format_as(Owner::Kernel, fmt);
    let mut entered: Option<(usize, u32)> = None;
    let mut stream: Option<(usize, Stream)> = None;
    let mut idle_since: Option<u64> = None;
    loop {
        let sink = port_with_sink();
        // The register follows the sink.
        if entered.map(|e| e.0) != sink {
            if let Some((_, h)) = entered.take() {
                if stream.take().is_some() {
                    stop();
                }
                devices::remove(h, Owner::Kernel);
            }
            if let Some(p) = sink {
                let id = if p == 0 { "pi5:hdmi0" } else { "pi5:hdmi1" };
                let name = if p == 0 { "HDMI 0" } else { "HDMI 1" };
                if let Some(h) = devices::register(Kind::AudioOut, 0, id, name, Owner::Kernel) {
                    entered = Some((p, h));
                }
            }
            ENTERED.store(entered.map_or(NONE, |e| e.0 as u32), Ordering::Release);
            if let Some((p, _)) = entered {
                kprintln!("[npk] hdmi audio: HDMI{} has an audio sink", p);
            }
        }
        let want = entered.is_some() && audio::kernel_serves() && audio::any_open();
        let now = crate::time::counter();
        idle_since = if want { None } else { Some(idle_since.unwrap_or(now)) };
        match (&mut stream, entered) {
            (None, Some((p, _))) if want => {
                stream = start(p).map(|s| (p, s));
            }
            (Some(_), _) if idle_since.is_some_and(|t| now - t > ms(IDLE_STOP_MS)) => {
                stream = None;
                stop();
            }
            (Some((_, s)), _) => s.refill(),
            _ => {}
        }
        let next = if stream.is_some() { ms(PERIODS as u64 * 10) } else { ms(POLL_MS) };
        if fiber::wait(fiber::SIG_EVENT, crate::time::counter() + next).is_none() {
            return;
        }
    }
}

/// What `beep` prints after its tone: how far the path got.
pub fn report() {
    let port = |v: u32| if v == NONE { alloc::string::String::from("none") } else { alloc::format!("HDMI{}", v) };
    let mut eld = [0u8; audio::ELD_MAX];
    let elds: [usize; 2] = [0, 1].map(|p| audio::display_audio_get(p, &mut eld).unwrap_or(0));
    kprintln!("[npk] hdmi audio: display driver bound {}, ELD bytes HDMI0 {} HDMI1 {}",
        audio::display_audio_done().is_some(), elds[0], elds[1]);
    kprintln!("[npk] hdmi audio: output {}, plays the mix {}, stream {}, started {} refused {}",
        port(ENTERED.load(Ordering::Acquire)), audio::kernel_serves(),
        port(STREAMING.load(Ordering::Acquire)), STARTS.load(Ordering::Relaxed), REFUSED.load(Ordering::Relaxed));
    if STREAMING.load(Ordering::Acquire) != NONE {
        kprintln!("[npk] hdmi audio: DMA periods {}, read position {:?}",
            super::pi5_hdmi_dma::periods(), super::pi5_hdmi_dma::position());
    }
}

/// Start the driver's fiber; it waits for the display driver's reports.
pub fn init() {
    crate::smp::scheduler::spawn_fiber(audio_fiber, 0);
}
