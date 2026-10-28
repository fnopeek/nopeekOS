//! audio_hda — generic Intel HD Audio (HDA) controller driver (WASM module).
//!
//! Hardware-independent by construction: binds the HDA controller by PCI class
//! (0x04/0x03), not vendor:device, so it drives any HDA-spec controller (Intel,
//! AMD, NVIDIA, QEMU's intel-hda). Every codec on the link is probed and its
//! widget graph parsed as `snd-hda-codec-generic` does (`codec.rs`); the
//! stream format is the best one the chosen converters take, and the kernel
//! mixer produces it.
//!
//! Codec verbs use the spec's Immediate Command Interface (IC/IR/IRS) — what Linux
//! uses as `single_cmd` on Intel — so the only DMA is the audio ring + BDL.
//! The ring is fed from the kernel audio mailbox.

#![no_std]

mod codec;
mod host;
mod intelhdmi;
mod regs;
use host::*;
use regs::*;

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    log("[audio_hda] panic\n");
    core::arch::wasm32::unreachable()
}

// EXECUTE to bind the PCI device, HARDWARE to drain the kernel mixer
// (`npk_audio_poll_mix`).
#[unsafe(link_section = ".npk.caps")]
#[used]
static NPK_CAPS: [u8; 1] = [0x04 | 0x40];

// ── audio buffer geometry ───────────────────────────────────────────────
// HDA playback ring: two halves fed from the kernel audio mailbox. The DMA
// cycles the ring; the loop refills what it has played with freshly mixed
// PCM (silence when no app is playing). Its bytes follow the device format.
const HALF_FRAMES: usize = 2048; // ~43 ms per half @ 48 kHz
/// The largest frame: 8 channels in 32-bit containers.
const MAX_FRAME_BYTES: usize = 8 * 4;

const STREAM_TAG: u32 = 1;

// ── small hex/dec logging helpers (no alloc) ──────────────────────────────
fn loghex(prefix: &str, v: u32) {
    let mut buf = [0u8; 8];
    for i in 0..8 {
        let nib = (v >> ((7 - i) * 4)) & 0xF;
        buf[i] = if nib < 10 { b'0' + nib as u8 } else { b'a' + (nib - 10) as u8 };
    }
    log(prefix);
    log(core::str::from_utf8(&buf).unwrap_or("?"));
}

/// One whole diagnostic line (prefix, hex value, newline), only with
/// `set log.drivers 1`. The newline is included so a gated line never
/// leaves an ungated empty line behind.
fn dbghexln(prefix: &str, v: u32) {
    if host::verbose() { loghex(prefix, v); log("\n"); }
}

// ── Immediate-command codec access ────────────────────────────────────────
fn codec_cmd(mmio: i32, cad: u32, nid: u32, verb20: u32) -> Option<u32> {
    let cmd = (cad << 28) | (nid << 20) | (verb20 & 0xFFFFF);
    // Wait until not busy.
    let mut spin = 0;
    while mmio_r16(mmio, IRS) & IRS_ICB != 0 {
        spin += 1;
        if spin > 100_000 { return None; }
    }
    mmio_w16(mmio, IRS, IRS_IRV); // clear stale result (W1C)
    mmio_w32(mmio, IC, cmd);
    fence();
    mmio_w16(mmio, IRS, IRS_ICB); // send
    spin = 0;
    loop {
        let s = mmio_r16(mmio, IRS);
        if s & IRS_IRV != 0 {
            let r = mmio_r32(mmio, IR);
            mmio_w16(mmio, IRS, IRS_IRV); // clear
            return Some(r);
        }
        spin += 1;
        if spin > 200_000 { return None; }
    }
}

fn get_param(mmio: i32, cad: u32, nid: u32, param: u32) -> u32 {
    codec_cmd(mmio, cad, nid, vget_param(param)).unwrap_or(0)
}

fn widget_type(mmio: i32, cad: u32, nid: u32) -> u32 {
    (get_param(mmio, cad, nid, PARAM_AUDIO_WIDGET_CAP) >> 20) & 0xF
}

/// What the chosen codec drives: the generic parser's outputs, or an Intel
/// display codec, which routes by the display driver's report.
enum Output {
    Generic(codec::Spec),
    IntelHdmi(codec::Codec),
}

/// Probe the codec at `cad`: power its audio function group and parse its
/// outputs. `None` without an audio function group.
fn probe_codec(mmio: i32, cad: u32) -> Option<Output> {
    // Function groups under the root node.
    let root = get_param(mmio, cad, 0, PARAM_SUB_NODE_COUNT);
    let fg_start = (root >> 16) & 0xFF;
    let fg_count = root & 0xFF;
    let afg = (fg_start..fg_start + fg_count)
        .find(|&fg| get_param(mmio, cad, fg, PARAM_FUNCTION_TYPE) & 0xFF == FUNC_TYPE_AFG)?;
    loghex("[audio_hda] codec 0x", cad);
    loghex(" vendor 0x", get_param(mmio, cad, 0, PARAM_VENDOR_ID));
    loghex(" AFG 0x", afg);
    log("\n");
    codec_cmd(mmio, cad, afg, vset_power(0)); // D0

    let c = codec::Codec::new(mmio, cad, afg);
    // An Intel display codec takes its own path: its pins are display
    // ports, and which one carries a sink is the display driver's to say.
    if intelhdmi::is_tgl_hdmi(mmio, cad) {
        return Some(Output::IntelHdmi(c));
    }
    let spec = codec::Spec::parse(c);
    spec.log_outputs();
    Some(Output::Generic(spec))
}

/// The device format: rate, sample width (16, 20, 24 or 32 bits, the
/// latter three in a 32-bit container), channels, and the SDxFMT word.
#[derive(Clone, Copy)]
struct DevFormat {
    rate: u32,
    bits: u32,
    channels: u32,
    word: u16,
}

impl DevFormat {
    fn sample_bytes(&self) -> usize {
        if self.bits == 16 { 2 } else { 4 }
    }
    fn frame_bytes(&self) -> usize {
        self.sample_bytes() * self.channels as usize
    }
}

/// Rates in order of preference: 48 kHz, which sources use as a rule, then
/// by closeness to it.
const RATE_PREFERENCE: [u32; 11] = [48000, 44100, 96000, 88200, 192000, 176400, 32000, 22050, 16000, 11025, 8000];

/// The best format the converters take (`rates`: AC_PAR_PCM rate bits,
/// `widths`: AC_SUPPCM_BITS_* bits): the most preferred rate, 16 bits when
/// supported, else the widest container.
fn pick_format(rates: u32, widths: u32, channels: u32) -> Option<DevFormat> {
    let rate = RATE_PREFERENCE.iter().copied().find(|r| {
        codec::RATE_BITS.iter().position(|(hz, _)| hz == r).is_some_and(|i| rates & (1 << i) != 0)
    })?;
    let bits = [(16, 1u32 << 17), (24, 1 << 19), (32, 1 << 20), (20, 1 << 18)]
        .iter()
        .find(|(_, bit)| widths & bit != 0)
        .map(|(b, _)| *b)?;
    let word = codec::stream_format(channels, bits, rate);
    (word != 0).then_some(DevFormat { rate, bits, channels, word })
}

/// Set up the chosen output for a stream in its best format.
fn setup_output(out: &mut Output) -> Option<DevFormat> {
    match out {
        Output::Generic(spec) => {
            let mut dacs = [0u8; 4 * codec::HDA_MAX_OUTS];
            let n = spec.used_dacs(&mut dacs);
            if n == 0 {
                return None;
            }
            // What every DAC of the stream takes.
            let (mut rates, mut widths) = (u32::MAX, u32::MAX);
            for &d in &dacs[..n] {
                let (r, w) = spec.codec.supported_pcm(d as u32);
                rates &= r;
                widths &= w;
            }
            let fmt = pick_format(rates, widths, spec.channels(2))?;
            spec.init();
            spec.setup_stream(STREAM_TAG, fmt.word, fmt.channels);
            Some(fmt)
        }
        Output::IntelHdmi(c) => {
            // One stereo stream (`intelhdmi::setup`).
            let (rates, widths) = c.supported_pcm(c.afg);
            let fmt = pick_format(rates, widths, 2)?;
            (intelhdmi::setup(c.mmio, c.cad, c.afg, STREAM_TAG, fmt.word) != 0).then_some(fmt)
        }
    }
}

// ── controller bring-up ───────────────────────────────────────────────────

/// Upper bound on HD Audio controllers probed.
const MAX_HDA: u32 = 4;

/// Codec addresses on the link: STATESTS bits 0..14 (`HDA_MAX_CODEC_ADDRESS`).
const HDA_MAX_CODECS: u32 = 15;

/// A controller brought up with its codec output set up.
struct Hda {
    mmio: i32,
    iss: u32,
    analog: bool,
    fmt: DevFormat,
    out: Output,
}

// ── display audio component ──────────────────────────────────────────────

/// Bits of a request to the display driver (docs/spec/AUDIO.md).
const DISPLAY_POWER: i32 = 1;
const DISPLAY_WAKE: i32 = 2;

/// `snd_hdac_i915_init`'s wait for the display driver to bind, in ms.
const COMPONENT_BIND_MS: i64 = 60_000;
/// How long the display driver may take to carry out a request, in ms.
const COMPONENT_REQUEST_MS: i64 = 1000;

/// Whether the audio component bound; None until first asked.
static COMPONENT: npk_sys::cell::Single<Option<bool>> = npk_sys::cell::Single::new(None);

/// Ask the display driver for `bits` instead of what was asked before and
/// wait until it is carried out (`snd_hdac_display_power`,
/// `snd_hdac_set_codec_wakeup`). The first request waits for the component
/// to bind as `snd_hdac_i915_init` does. False when it does not bind, no
/// Intel display is there, or the request is not carried out.
fn display_request(bits: i32) -> bool {
    if COMPONENT.with(|c| *c) == Some(false) {
        return false;
    }
    let t0 = npk_sys::ticks();
    let n = loop {
        match npk_sys::display_audio_request(bits) {
            n if n > 0 => break n,
            -2 if COMPONENT.with(|c| c.is_none()) && npk_sys::ticks() - t0 < COMPONENT_BIND_MS => sleep_ms(10),
            rc => {
                if rc != -3 {
                    log("[audio_hda] couldn't bind with audio component\n");
                }
                COMPONENT.with(|c| *c = Some(false));
                return false;
            }
        }
    };
    if COMPONENT.with(|c| c.replace(true)).is_none() {
        log("[audio_hda] display audio component bound after ");
        logdec((npk_sys::ticks() - t0) as u32);
        log(" ms\n");
    }
    let t1 = npk_sys::ticks();
    while npk_sys::display_audio_done() != n {
        if npk_sys::ticks() - t1 > COMPONENT_REQUEST_MS {
            log("[audio_hda] the display driver did not carry out a request\n");
            return false;
        }
        sleep_ms(1);
    }
    true
}

/// Bring the bound controller up as [`bring_up_controller`]; a controller
/// that is not used gives back the display power it took.
fn bring_up() -> Option<Hda> {
    let hda = bring_up_controller();
    if hda.is_none() && COMPONENT.with(|c| *c) == Some(true) {
        display_request(0);
    }
    hda
}

/// The binding is in place: bring the controller up, probe every codec on
/// the link and set up the best output: the first codec with an analog
/// output, else the first with any. `None` if this controller is unusable
/// and the caller should try the next one.
fn bring_up_controller() -> Option<Hda> {
    pci_enable_bus_master();
    // Intel quirk: clear TCSEL (PCI 0x44) traffic-class bits so DMA uses TC0.
    let tcsel = pci_read_config(0x44);
    pci_write_config(0x44, tcsel & !0x7);

    let mmio = mmio_map_bar(0, 16);
    if mmio < 0 { log("[audio_hda] BAR0 map failed\n"); return None; }

    // An Intel controller's display codec is powered and woken by the
    // display driver (`azx_probe_continue`: the i915 component and display
    // power), with the wake signal forced on around the reset
    // (`hda_intel_init_chip`), so it reports itself in STATESTS.
    let display = pci_read_config(0x00) & 0xFFFF == 0x8086 && display_request(DISPLAY_POWER | DISPLAY_WAKE);
    let reset = reset_controller(mmio);
    let statests = mmio_r16(mmio, STATESTS) as u32;
    if display {
        display_request(DISPLAY_POWER);
    }
    if !reset { log("[audio_hda] controller reset timeout\n"); return None; }

    let gcap = mmio_r16(mmio, GCAP) as u32;
    let iss = (gcap >> 8) & 0xF; // input streams (output streams follow them)
    let oss = (gcap >> 12) & 0xF;
    dbghexln("[audio_hda] controller up, gcap=0x", gcap);
    if oss == 0 { log("[audio_hda] no output streams\n"); return None; }

    loghex("[audio_hda] codecs present (STATESTS) 0x", statests);
    log("\n");
    if statests == 0 { log("[audio_hda] no codec detected\n"); return None; }

    let mut chosen: Option<(Output, bool)> = None;
    for cad in 0..HDA_MAX_CODECS {
        if statests & (1 << cad) == 0 { continue; }
        let Some(out) = probe_codec(mmio, cad) else {
            loghex("[audio_hda] codec 0x", cad);
            log(" has no audio function group\n");
            continue;
        };
        let (usable, analog) = match &out {
            Output::Generic(spec) => (spec.has_output(), spec.has_analog()),
            Output::IntelHdmi(_) => (true, false),
        };
        if !usable { continue; }
        let better = match &chosen {
            None => true,
            Some((_, a)) => analog && !*a,
        };
        if better { chosen = Some((out, analog)); }
    }
    let Some((mut out, analog)) = chosen else {
        log("[audio_hda] no codec with an output\n");
        return None;
    };
    let Some(fmt) = setup_output(&mut out) else {
        log("[audio_hda] codec setup failed\n");
        return None;
    };
    let cad = match &out { Output::Generic(s) => s.codec.cad, Output::IntelHdmi(c) => c.cad };
    loghex("[audio_hda] output on codec 0x", cad);
    log(if analog { " (analog), " } else { " (digital), " });
    logdec(fmt.rate);
    log(" Hz ");
    logdec(fmt.bits);
    log(" bit ");
    logdec(fmt.channels);
    log(" ch\n");
    Some(Hda { mmio, iss, analog, fmt, out })
}

fn logdec(mut v: u32) {
    let mut buf = [0u8; 10];
    let mut i = buf.len();
    loop {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 { break; }
    }
    log(core::str::from_utf8(&buf[i..]).unwrap_or("?"));
}

fn reset_controller(mmio: i32) -> bool {
    // Assert reset (CRST=0), wait, then deassert (CRST=1), wait for run.
    let g = mmio_r32(mmio, GCTL);
    mmio_w32(mmio, GCTL, g & !GCTL_CRST);
    let mut spin = 0;
    while mmio_r32(mmio, GCTL) & GCTL_CRST != 0 {
        spin += 1;
        if spin > 100_000 { return false; }
    }
    mmio_w32(mmio, GCTL, mmio_r32(mmio, GCTL) | GCTL_CRST);
    spin = 0;
    while mmio_r32(mmio, GCTL) & GCTL_CRST == 0 {
        spin += 1;
        if spin > 100_000 { return false; }
    }
    // Codecs need time to report presence after reset.
    sleep_ms(1);
    true
}

fn reset_stream(mmio: i32, base: u32) {
    mmio_w32(mmio, base + SD_CTL, SD_CTL_SRST);
    let mut spin = 0;
    while mmio_r32(mmio, base + SD_CTL) & SD_CTL_SRST == 0 {
        spin += 1;
        if spin > 100_000 { break; }
    }
    mmio_w32(mmio, base + SD_CTL, 0);
    spin = 0;
    while mmio_r32(mmio, base + SD_CTL) & SD_CTL_SRST != 0 {
        spin += 1;
        if spin > 100_000 { break; }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    host::log_init();
    log(concat!("[audio_hda] ", env!("CARGO_PKG_VERSION"), " — generic HDA driver\n"));

    // Bind the HDA controller by PCI class — no vendor:device hardcode.
    // Intel cAVS controllers report subclass 0x01 ("Audio controller")
    // instead of the canonical 0x03 ("HD Audio"); both expose the same
    // register interface, so both lists are walked.
    //
    // Search for the right controller rather than taking the first: most
    // machines have two (the GPU's for HDMI/DP and the chipset's for
    // speakers and headphones), and PCI order between them is arbitrary.
    // Only this driver knows what a usable output is, so the choice lives
    // here, not in the kernel.
    let mut chosen: Option<Hda> = None;
    let mut fb: Option<(u8, u32)> = None;        // first usable one, but digital
    'outer: for &sub in [0x03u8, 0x01u8].iter() {
        for i in 0..MAX_HDA {
            if pci_bind_class_n(0x04, sub, i) != 0 { break; }
            match bring_up() {
                Some(h) if h.analog => { chosen = Some(h); break 'outer; }
                Some(_) => { if fb.is_none() { fb = Some((sub, i)); } }
                None => {}
            }
        }
    }
    if chosen.is_none() {
        // None has an analog output — take the first that came up, so an
        // HDMI-only machine still gets sound.
        if let Some((sub, i)) = fb {
            log("[audio_hda] no analog output anywhere — using the first controller that worked\n");
            if pci_bind_class_n(0x04, sub, i) == 0 {
                chosen = bring_up();
            }
        }
    }
    let Some(Hda { mmio, iss, fmt, mut out, .. }) = chosen else {
        log("[audio_hda] no usable HDA controller — exit\n");
        return;
    };

    // The kernel mixer produces the device format.
    let encoding = if fmt.bits == 16 { 0 } else { 1 }; // S16LE, else S32LE (MSB-aligned)
    if npk_sys::audio_device_format(fmt.rate as i32, encoding, fmt.channels as i32) != 0 {
        log("[audio_hda] the mixer does not take the device format, exit\n");
        return;
    }
    let fb_bytes = fmt.frame_bytes();
    let half_bytes = HALF_FRAMES * fb_bytes;
    let ring_bytes = half_bytes * 2;

    // ── DMA: the playback ring (zeroed by the kernel = silence) + its BDL ──
    let audio = dma_alloc(((ring_bytes + 4095) / 4096) as u16);
    let bdl = dma_alloc(1);
    if audio < 0 || bdl < 0 {
        log("[audio_hda] DMA alloc failed — exit\n");
        return;
    }
    let audio_phys = dma_phys(audio);

    // BDL: two equal cyclic halves (HDA wants >= 2; LVI = entries-1).
    let half = half_bytes as u32;
    let mut bdl_buf = [0u8; 32];
    let mk = |buf: &mut [u8], i: usize, addr: u64, len: u32| {
        buf[i..i + 8].copy_from_slice(&addr.to_le_bytes());
        buf[i + 8..i + 12].copy_from_slice(&len.to_le_bytes());
        buf[i + 12..i + 16].copy_from_slice(&1u32.to_le_bytes()); // IOC: one interrupt per half
    };
    mk(&mut bdl_buf, 0, audio_phys, half);
    mk(&mut bdl_buf, 16, audio_phys + half as u64, half);
    dma_write(bdl, 0, &bdl_buf);
    let bdl_phys = dma_phys(bdl);

    // ── program + start the output stream descriptor ──────────────────────
    let base = SD_BASE + iss * SD_STRIDE; // first output stream
    reset_stream(mmio, base);
    mmio_w32(mmio, base + SD_BDLPL, (bdl_phys & 0xFFFF_FFFF) as u32);
    mmio_w32(mmio, base + SD_BDLPU, (bdl_phys >> 32) as u32);
    mmio_w32(mmio, base + SD_CBL, ring_bytes as u32);
    mmio_w16(mmio, base + SD_LVI, 1);
    mmio_w16(mmio, base + SD_FORMAT, fmt.word);
    fence();
    // The controller's MSI (`azx_acquire_irq`; the AMD SB preset allows MSI).
    // With it the stream raises an interrupt at the end of each half
    // (IOC in both BDL entries) and the loop below sleeps until then.
    let irq = host::irq_register() >= 0;
    let mut sd_ctl = (STREAM_TAG << SD_CTL_STRM_SHIFT) | SD_CTL_RUN;
    if irq {
        // `snd_hdac_stream_start`: the stream's bit in INTCTL plus the
        // global enable, and `SD_INT_MASK` in SD_CTL with the run bit. The
        // controller-interrupt enable (CIE) stays off: codec verbs are
        // polled, only during bring-up, so no RIRB interrupt is wanted.
        let ic = mmio_r32(mmio, INTCTL);
        mmio_w32(mmio, INTCTL, ic | AZX_INT_GLOBAL_EN | (1 << iss));
        sd_ctl |= SD_INT_MASK;
    }
    mmio_w32(mmio, base + SD_CTL, sd_ctl);
    fence();

    log(if irq {
        "[audio_hda] streaming from audio mailbox (MSI: one wake per half)\n"
    } else {
        "[audio_hda] streaming from audio mailbox (polling)\n"
    });

    // Streaming loop: a true ring-buffer copy. Each poll refills exactly the
    // region the DMA has played since last time — [write_pos, LPIB) — pulling
    // that many bytes from the kernel mixer. This locks the drain rate to the
    // DMA's real rate regardless of how often this loop is scheduled; a
    // refill-on-crossing scheme misses crossings under coarse wakeups and
    // drains the mailbox too slowly. write_pos chases LPIB; the ring stays
    // about one lap ahead of the play head (~85 ms latency, reported to the
    // guest as the HDA-ring latency). poll_mix yields silence when no app is
    // playing.
    let mut write_pos: usize = 0;
    // First five seconds: report whether the DMA runs at all.
    //
    // The loop below only fills what the DMA has already played
    // (`avail = LPIB - write_pos`). If LPIB stands still nothing is ever
    // written, which in the log looks like a clean start without sound.
    // Self-limiting: silent after five reports.
    let mut reports = 5u32;
    let mut pulled: u64 = 0;
    let mut ticks: u32 = 0;
    // Reports that fire only when sound actually passed. The five
    // reports above hit the silence at start; a beep comes later.
    let mut loud_reports = 6u32;
    let mut peak: i32 = 0;
    let mut wrote: u32 = 0;
    let mut readback: u32 = 0;
    // Scratch buffer for one mailbox poll -> one ring half.
    let mut mix = [0u8; HALF_FRAMES * MAX_FRAME_BYTES];
    // Peaks are read from the top 16 bits of each sample.
    let sample_bytes = fmt.sample_bytes();
    let half_ms = (HALF_FRAMES as u32 * 1000) / fmt.rate;
    let mut last_jack_poll = npk_sys::ticks();
    loop {
        intelhdmi::poll();
        if let Output::Generic(spec) = &mut out {
            let now = npk_sys::ticks();
            if now - last_jack_poll >= JACK_POLL_MS {
                last_jack_poll = now;
                spec.poll_jacks();
            }
        }
        let lpib = (mmio_r32(mmio, base + SD_LPIB) as usize) % ring_bytes;
        ticks += 1;
        if host::verbose() && reports > 0 && ticks % 250 == 0 {
            reports -= 1;
            loghex("[audio_hda] LPIB=0x", lpib as u32);
            loghex(" wpos=0x", write_pos as u32);
            // SD_CTL read as u32 carries STS in its top byte (offset 0x03) —
            // run bit, stream tag and status in one number.
            loghex(" SDCTL=0x", mmio_r32(mmio, base + SD_CTL));
            loghex(" gemischt=0x", pulled as u32);
            log("\n");
        }
        if host::verbose() && loud_reports > 0 && peak > 0 {
            loud_reports -= 1;
            loghex("[audio_hda] TON: Spitze=0x", peak as u32);
            loghex(" geschrieben=0x", wrote);
            loghex(" zurueckgelesen=0x", readback);
            loghex(" wpos=0x", write_pos as u32);
            log("\n");
            peak = 0;
        }
        loop {
            // Bytes the DMA has played since we last filled, whole frames.
            let avail = (lpib + ring_bytes - write_pos) % ring_bytes;
            let avail = avail - avail % fb_bytes;
            if avail == 0 { break; }
            // Don't cross the ring end or overflow `mix` in one copy.
            let n = avail.min(ring_bytes - write_pos).min(half_bytes);
            audio_poll_mix(&mut mix[..n]);
            // Peak of the mixed block: shows whether the mailbox delivers
            // anything. Every eighth sample is enough for a peak.
            let mut i = sample_bytes - 2;
            while i + 1 < n {
                let v = i16::from_le_bytes([mix[i], mix[i + 1]]) as i32;
                let a = if v < 0 { -v } else { v };
                if a > peak { peak = a; }
                i += 8 * sample_bytes;
            }
            dma_write(audio, write_pos as u32, &mix[..n]);
            // Read back what was just written. If it does not match,
            // `dma_write` does not land where the device reads.
            if n >= 4 {
                wrote = u32::from_le_bytes([mix[0], mix[1], mix[2], mix[3]]);
                readback = dma_read32(audio, write_pos as u32);
            }
            pulled += n as u64;
            write_pos = (write_pos + n) % ring_bytes;
        }
        if irq {
            // `azx_interrupt` → `snd_hdac_bus_handle_stream_irq`: the
            // stream's bit in INTSTS, then SD_STS cleared with SD_INT_MASK
            // (write-1-to-clear) — as a byte, like Linux' `writeb`. QEMU's
            // intel-hda models SD_CTL (3 bytes) and SD_STS (1 byte) as separate
            // registers, so a 32-bit write to SD_CTL never clears the status and
            // every register update raises another MSI.
            if mmio_r32(mmio, INTSTS) & (1 << iss) != 0 {
                mmio_w8(mmio, base + SD_STS, SD_INT_MASK as u8);
            }
            // Sleep until the DMA finishes a half. One half is HALF_FRAMES at
            // the device rate; if the interrupt never came we would still
            // refill a half-period late, with the other half queued.
            wait_irq(half_ms + 10);
        } else {
            sleep_ms(4);
        }
    }
}

/// How often the jacks are read for headphone automute (Linux
/// `jackpoll_ms`, whose usual setting is in this range).
const JACK_POLL_MS: i64 = 500;
