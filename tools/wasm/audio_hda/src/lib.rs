//! audio_hda — generic Intel HD Audio (HDA) controller driver (WASM module).
//!
//! Hardware-independent by construction: binds the HDA controller by PCI class
//! (0x04/0x03), not vendor:device, so it drives any HDA-spec controller (Intel,
//! AMD, NVIDIA, QEMU's intel-hda). The codec is enumerated generically by walking
//! the widget graph (like `snd-hda-codec-generic`) to find a DAC -> output-pin path.
//!
//! Codec verbs use the spec's Immediate Command Interface (IC/IR/IRS) — what Linux
//! uses as `single_cmd` on Intel — so the only DMA is the audio ring + BDL.
//! The ring is fed from the kernel audio mailbox.

#![no_std]

mod host;
mod regs;
use host::*;
use regs::*;

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    log("[audio_hda] panic");
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
// PCM (silence when no app is playing).
const HALF_FRAMES: usize = 2048; // ~43 ms per half @ 48 kHz
const HALF_BYTES: usize = HALF_FRAMES * 4; // S16 stereo
const RING_BYTES: usize = HALF_BYTES * 2;

const STREAM_TAG: u32 = 1;

// Scratch buffer for one mailbox poll -> one ring half.
static mut MIXBUF: [u8; HALF_BYTES] = [0; HALF_BYTES];

// ── small hex/dec logging helpers (no alloc) ──────────────────────────────
fn loghex(prefix: &str, v: u32) {
    let mut buf = [0u8; 8];
    for i in 0..8 {
        let nib = (v >> ((7 - i) * 4)) & 0xF;
        buf[i] = if nib < 10 { b'0' + nib as u8 } else { b'a' + (nib - 10) as u8 };
    }
    log(prefix);
    log(unsafe { core::str::from_utf8_unchecked(&buf) });
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

/// Unmute the output amp of a widget at (near) max gain — if it has one.
///
/// An amp verb sent to a widget without an amp is undefined by the spec, and
/// QEMU's line-out pin (`AMP_OUT_CAP = 0`, no `stindex`) shares the DAC's
/// stream, so a gain-0 verb there silences the DAC that was just set up.
///
/// Zero steps does not mean "no amp" either: it can be a pure mute switch
/// (e.g. on Realtek speaker pins), whose mute bit must be cleared. The spec
/// separates the two by bit 31 of `AMP_*_CAP` ("can mute"):
///
/// * 0 steps and bit 31 set   → mute switch, must be opened
/// * 0 steps and bit 31 clear → nothing there, the verb only does harm (QEMU)
fn unmute_out(mmio: i32, cad: u32, nid: u32) {
    // Ask both, because each can say no independently: the widget's
    // capability bit and the step count of its amp.
    if get_param(mmio, cad, nid, PARAM_AUDIO_WIDGET_CAP) & WCAP_OUT_AMP == 0 {
        return;
    }
    let cap = get_param(mmio, cad, nid, PARAM_AMP_OUT_CAP);
    let steps = (cap >> 8) & 0x7F;
    if steps == 0 {
        if cap & AMP_CAP_MUTE == 0 { return; }
        // Gain 0, mute bit (bit 7) clear: open.
        codec_cmd(mmio, cad, nid, vset_amp(AMP_SET_OUT_BOTH));
        return;
    }
    // ~3/4 of max gain — audible but not blasting.
    let gain = ((steps * 3 / 4) & 0x7F) as u16;
    codec_cmd(mmio, cad, nid, vset_amp(AMP_SET_OUT_BOTH | gain));
}

/// Unmute input amp `index` of a node.
///
/// A mixer has one amp per input, and on many codecs they default to muted.
/// `unmute_out` opens only the output, leaving mixer → pin open and DAC →
/// mixer closed: LPIB runs, the pin is right, and nothing is heard.
fn unmute_in(mmio: i32, cad: u32, nid: u32, index: u32) {
    if get_param(mmio, cad, nid, PARAM_AUDIO_WIDGET_CAP) & WCAP_IN_AMP == 0 {
        return;
    }
    let cap = get_param(mmio, cad, nid, PARAM_AMP_IN_CAP);
    let steps = (cap >> 8) & 0x7F;
    let idx = (index as u16 & 0xF) << 8;
    if steps == 0 {
        // Same split as for the output: open a mute switch, leave an amp
        // without any capability alone.
        if cap & AMP_CAP_MUTE == 0 { return; }
        codec_cmd(mmio, cad, nid, vset_amp(AMP_SET_IN_BOTH | idx));
        return;
    }
    let gain = ((steps * 3 / 4) & 0x7F) as u16;
    codec_cmd(mmio, cad, nid, vset_amp(AMP_SET_IN_BOTH | idx | gain));
}

/// The raw amp capability of a node, for the log.
///
/// The step count alone is ambiguous: `steps = 0` can mean "no amp", "pure
/// mute switch" or "no output amp at all", and each needs something
/// different. Bit 31 = can mute, [14:8] = steps, [6:0] = offset. `0` means
/// the widget has no output amp.
fn amp_cap(mmio: i32, cad: u32, nid: u32) -> u32 {
    if get_param(mmio, cad, nid, PARAM_AUDIO_WIDGET_CAP) & WCAP_OUT_AMP == 0 { return 0; }
    get_param(mmio, cad, nid, PARAM_AMP_OUT_CAP)
}

fn conn_entry0(mmio: i32, cad: u32, nid: u32) -> u32 {
    // Short-form connection list: 4 entries packed in one response.
    let r = codec_cmd(mmio, cad, nid, vget_conn_entry(0)).unwrap_or(0);
    r & 0xFF
}

// Walk from an output pin back to its feeding DAC (up to 2 hops through a
// mixer/selector), unmuting each node in the path.
fn trace_to_dac(mmio: i32, cad: u32, pin: u32) -> u32 {
    // Log the path, so it shows whether a mixer — and thus an input amp —
    // sits between pin and DAC.
    if host::verbose() {
        loghex("[audio_hda] path: pin 0x", pin);
        loghex(" (out-amp cap=0x", amp_cap(mmio, cad, pin));
        log(")");
    }
    let len = get_param(mmio, cad, pin, PARAM_CONN_LIST_LEN) & 0x7F;
    if len == 0 { log(" <- (no connection list)\n"); return 0; }
    let first = conn_entry0(mmio, cad, pin);
    if first == 0 { log(" <- (empty)\n"); return 0; }
    if widget_type(mmio, cad, first) == WTYPE_DAC {
        loghex(" <- dac 0x", first);
        loghex(" (out-amp cap=0x", amp_cap(mmio, cad, first));
        log(")\n");
        return first;
    }
    // mixer or selector: select input 0, unmute both directions, descend
    if widget_type(mmio, cad, first) == WTYPE_SELECTOR {
        codec_cmd(mmio, cad, first, vset_conn_select(0));
    }
    unmute_out(mmio, cad, first);
    unmute_in(mmio, cad, first, 0);
    loghex(" <- mid 0x", first);
    loghex(" (out-amp cap=0x", amp_cap(mmio, cad, first));
    log(")");
    let inner = conn_entry0(mmio, cad, first);
    if inner != 0 && widget_type(mmio, cad, inner) == WTYPE_DAC {
        loghex(" <- dac 0x", inner);
        loghex(" (out-amp cap=0x", amp_cap(mmio, cad, inner));
        log(")\n");
        return inner;
    }
    loghex(" <- 0x", inner);
    log(" (not a DAC)\n");
    inner
}

// Generic codec setup: find an output pin + its DAC, configure format/stream,
// enable the pin, unmute the path. Returns the DAC NID or 0 on failure, and
// whether the chosen output is analog (speaker, headphone or line-out).
// `_start` uses that to pick the controller: an HDMI audio unit has only
// `dev=0x05` pins, and a tone sent there is not heard on the speakers.
fn setup_codec(mmio: i32, cad: u32) -> (u32, bool) {
    // Function groups under the root node.
    let root = get_param(mmio, cad, 0, PARAM_SUB_NODE_COUNT);
    let fg_start = (root >> 16) & 0xFF;
    let fg_count = root & 0xFF;
    let mut afg = 0u32;
    for fg in fg_start..fg_start + fg_count {
        if get_param(mmio, cad, fg, PARAM_FUNCTION_TYPE) & 0xFF == FUNC_TYPE_AFG {
            afg = fg;
            break;
        }
    }
    if afg == 0 {
        log("[audio_hda] no audio function group\n");
        return (0, false);
    }
    dbghexln("[audio_hda] AFG nid=0x", afg);
    codec_cmd(mmio, cad, afg, vset_power(0)); // D0

    // Widgets under the AFG.
    let w = get_param(mmio, cad, afg, PARAM_SUB_NODE_COUNT);
    let w_start = (w >> 16) & 0xFF;
    let w_count = w & 0xFF;

    // Find the best output pin. Prefer the built-in speaker (so a tone is
    // audible with no cable), then headphone, then line-out. Log every
    // output-capable pin so the real codec topology is visible on hardware.
    let mut pin = 0u32;
    let mut pin_pri = 99u32;
    let mut pin_dev = 0xFFu32;
    let mut pin_fallback = 0u32;
    for nid in w_start..w_start + w_count {
        if widget_type(mmio, cad, nid) != WTYPE_PIN { continue; }
        let pcap = get_param(mmio, cad, nid, PARAM_PIN_CAP);
        if pcap & (1 << 4) == 0 { continue; } // not output-capable
        if pin_fallback == 0 { pin_fallback = nid; }
        let cfg = codec_cmd(mmio, cad, nid, vget_config_default()).unwrap_or(0);
        let conn = (cfg >> 30) & 0x3; // 1 = no physical connection
        let dev = (cfg >> 20) & 0xF; // 0=LineOut 1=Speaker 2=HPOut
        if host::verbose() {
            loghex("[audio_hda]  out-pin nid=0x", nid);
            loghex(" dev=0x", dev);
            loghex(" conn=0x", conn);
            log("\n");
        }
        if conn == 1 { continue; } // no physical jack/connection
        let pri = match dev { 1 => 0, 2 => 1, 0 => 2, _ => continue };
        if pri < pin_pri {
            pin_pri = pri;
            pin = nid;
            pin_dev = dev;
        }
    }
    if pin == 0 { pin = pin_fallback; }
    if pin == 0 {
        log("[audio_hda] no output pin\n");
        return (0, false);
    }
    let devname = match pin_dev {
        1 => "speaker",
        2 => "headphone",
        0 => "line-out",
        _ => "(fallback)",
    };
    log("[audio_hda] selected output: ");
    log(devname);
    loghex(" pin nid=0x", pin);
    log("\n");

    let dac = trace_to_dac(mmio, cad, pin);
    if dac == 0 {
        log("[audio_hda] no DAC behind pin\n");
        return (0, false);
    }
    dbghexln("[audio_hda] DAC nid=0x", dac);

    // Configure the DAC: power, format, bind to our stream tag, unmute.
    codec_cmd(mmio, cad, dac, vset_power(0));
    codec_cmd(mmio, cad, dac, vset_format(FMT_48K_S16_STEREO));
    codec_cmd(mmio, cad, dac, vset_stream_chan((STREAM_TAG << 4) | 0));
    unmute_out(mmio, cad, dac);

    // Enable the pin: power, output enable, EAPD, unmute.
    codec_cmd(mmio, cad, pin, vset_power(0));
    codec_cmd(mmio, cad, pin, vset_pin_ctl(PIN_CTL_OUT_EN));
    codec_cmd(mmio, cad, pin, vset_eapd(EAPD_ENABLE));
    unmute_out(mmio, cad, pin);

    (dac, pin_pri != 99)
}

// ── controller bring-up ───────────────────────────────────────────────────

/// Upper bound on HD Audio controllers probed.
const MAX_HDA: u32 = 4;

/// The binding is in place: bring the controller up and set up the codec.
/// Returns `(mmio, iss, analog)`, or `None` if this controller is unusable
/// and the caller should try the next one.
fn bring_up() -> Option<(i32, u32, bool)> {
    pci_enable_bus_master();
    // Intel quirk: clear TCSEL (PCI 0x44) traffic-class bits so DMA uses TC0.
    let tcsel = pci_read_config(0x44);
    pci_write_config(0x44, tcsel & !0x7);

    let mmio = mmio_map_bar(0, 16);
    if mmio < 0 { log("[audio_hda] BAR0 map failed\n"); return None; }
    if !reset_controller(mmio) { log("[audio_hda] controller reset timeout\n"); return None; }

    let gcap = mmio_r16(mmio, GCAP) as u32;
    let iss = (gcap >> 8) & 0xF; // input streams (output streams follow them)
    let oss = (gcap >> 12) & 0xF;
    dbghexln("[audio_hda] controller up, gcap=0x", gcap);
    if oss == 0 { log("[audio_hda] no output streams\n"); return None; }

    let statests = mmio_r16(mmio, STATESTS) as u32;
    dbghexln("[audio_hda] STATESTS=0x", statests);
    if statests == 0 { log("[audio_hda] no codec detected\n"); return None; }
    let mut cad = 0u32;
    while cad < 15 && statests & (1 << cad) == 0 { cad += 1; }
    dbghexln("[audio_hda] codec addr=0x", cad);

    let (dac, analog) = setup_codec(mmio, cad);
    if dac == 0 { log("[audio_hda] codec setup failed\n"); return None; }
    Some((mmio, iss, analog))
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
    let mut chosen: Option<(i32, u32)> = None;   // (mmio, iss)
    let mut fb: Option<(u8, u32)> = None;        // first usable one, but digital
    'outer: for &sub in [0x03u8, 0x01u8].iter() {
        for i in 0..MAX_HDA {
            if pci_bind_class_n(0x04, sub, i) != 0 { break; }
            match bring_up() {
                Some((mmio, iss, true)) => { chosen = Some((mmio, iss)); break 'outer; }
                Some((_, _, false)) => { if fb.is_none() { fb = Some((sub, i)); } }
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
                if let Some((mmio, iss, _)) = bring_up() { chosen = Some((mmio, iss)); }
            }
        }
    }
    let (mmio, iss) = match chosen {
        Some(c) => c,
        None => { log("[audio_hda] no usable HDA controller — exit\n"); return; }
    };

    // ── DMA: the playback ring (zeroed by the kernel = silence) + its BDL ──
    let audio = dma_alloc(((RING_BYTES + 4095) / 4096) as u16);
    let bdl = dma_alloc(1);
    if audio < 0 || bdl < 0 {
        log("[audio_hda] DMA alloc failed — exit\n");
        return;
    }
    let audio_phys = dma_phys(audio);

    // BDL: two equal cyclic halves (HDA wants >= 2; LVI = entries-1).
    let half = HALF_BYTES as u32;
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
    mmio_w32(mmio, base + SD_CBL, RING_BYTES as u32);
    mmio_w16(mmio, base + SD_LVI, 1);
    mmio_w16(mmio, base + SD_FORMAT, FMT_48K_S16_STEREO);
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
    // DMA's real 48 kHz regardless of how often this loop is scheduled; a
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
    loop {
        let lpib = (mmio_r32(mmio, base + SD_LPIB) as usize) % RING_BYTES;
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
            // Bytes the DMA has played since we last filled (frame-aligned to 4).
            let avail = ((lpib + RING_BYTES - write_pos) % RING_BYTES) & !3;
            if avail == 0 { break; }
            // Don't cross the ring end or overflow MIXBUF in one copy.
            let n = avail.min(RING_BYTES - write_pos).min(HALF_BYTES);
            let mix = unsafe { &mut *core::ptr::addr_of_mut!(MIXBUF) };
            audio_poll_mix(&mut mix[..n]);
            // Peak of the mixed block: shows whether the mailbox delivers
            // anything. Every eighth sample is enough for a peak.
            let mut i = 0usize;
            while i + 1 < n {
                let v = i16::from_le_bytes([mix[i], mix[i + 1]]) as i32;
                let a = if v < 0 { -v } else { v };
                if a > peak { peak = a; }
                i += 16;
            }
            dma_write(audio, write_pos as u32, &mix[..n]);
            // Read back what was just written. If it does not match,
            // `dma_write` does not land where the device reads.
            if n >= 4 {
                wrote = u32::from_le_bytes([mix[0], mix[1], mix[2], mix[3]]);
                readback = dma_read32(audio, write_pos as u32);
            }
            pulled += n as u64;
            write_pos = (write_pos + n) % RING_BYTES;
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
            // 48 kHz; if the interrupt never came we would still refill a
            // half-period late, with the other half queued.
            wait_irq(HALF_MS + 10);
        } else {
            sleep_ms(4);
        }
    }
}

/// Play time of one ring half.
const HALF_MS: u32 = (HALF_FRAMES as u32 * 1000) / 48_000;
