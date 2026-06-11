//! audio_hda — generic Intel HD Audio (HDA) controller driver (WASM module).
//!
//! Hardware-independent by construction: binds the HDA controller by PCI *class*
//! (0x04/0x03), not vendor:device, so it drives any HDA-spec controller (Intel,
//! AMD, NVIDIA, QEMU's intel-hda). The codec is enumerated generically by walking
//! the widget graph (like `snd-hda-codec-generic`) to find a DAC -> output-pin path.
//!
//! Codec verbs use the spec's Immediate Command Interface (IC/IR/IRS) — what Linux
//! uses as `single_cmd` on Intel — so the only DMA is the audio ring + BDL.
//!
//! M1: bring the controller + codec + one output stream up and play a built-in
//! sine tone. Proves the whole path before the kernel audio mailbox (M2) exists.
//! Test target: bare metal (audible yes/no). Verbose stage banners over serial.

#![no_std]

mod host;
mod regs;
use host::*;
use regs::*;

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    log("[audio_hda] panic");
    loop {}
}

// Driver only needs to bind a PCI device -> EXECUTE right. The default caps
// grant READ|EXECUTE|RENDER; we declare EXECUTE explicitly (least privilege).
#[unsafe(link_section = ".npk.caps")]
#[used]
static NPK_CAPS: [u8; 1] = [0x04];

// ── audio buffer geometry ───────────────────────────────────────────────
// 480 Hz tone @ 48 kHz = exactly 100 samples/period -> a clean cyclic loop.
// HDA playback ring: two ping-pong halves fed from the kernel audio mailbox.
// The DMA cycles the ring; each tick we keep the half it is NOT currently
// playing filled with freshly mixed PCM (silence when no app is playing).
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

// Unmute the output amp of a widget at (near) max gain.
fn unmute_out(mmio: i32, cad: u32, nid: u32) {
    let steps = (get_param(mmio, cad, nid, PARAM_AMP_OUT_CAP) >> 8) & 0x7F;
    // ~3/4 of max gain — audible but not blasting (real volume control = M4).
    let gain = ((steps * 3 / 4) & 0x7F) as u16;
    codec_cmd(mmio, cad, nid, vset_amp(AMP_SET_OUT_BOTH | gain));
}

fn conn_entry0(mmio: i32, cad: u32, nid: u32) -> u32 {
    // Short-form connection list: 4 entries packed in one response.
    let r = codec_cmd(mmio, cad, nid, vget_conn_entry(0)).unwrap_or(0);
    r & 0xFF
}

// Walk from an output pin back to its feeding DAC (up to 2 hops through a
// mixer/selector), unmuting each node in the path.
fn trace_to_dac(mmio: i32, cad: u32, pin: u32) -> u32 {
    let len = get_param(mmio, cad, pin, PARAM_CONN_LIST_LEN) & 0x7F;
    if len == 0 { return 0; }
    let first = conn_entry0(mmio, cad, pin);
    if first == 0 { return 0; }
    if widget_type(mmio, cad, first) == WTYPE_DAC {
        return first;
    }
    // mixer or selector: select input 0, unmute, descend one level
    if widget_type(mmio, cad, first) == WTYPE_SELECTOR {
        codec_cmd(mmio, cad, first, vset_conn_select(0));
    }
    unmute_out(mmio, cad, first);
    let inner = conn_entry0(mmio, cad, first);
    if inner != 0 && widget_type(mmio, cad, inner) == WTYPE_DAC {
        return inner;
    }
    inner
}

// Generic codec setup: find an output pin + its DAC, configure format/stream,
// enable the pin, unmute the path. Returns the DAC NID or 0 on failure.
fn setup_codec(mmio: i32, cad: u32) -> u32 {
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
        return 0;
    }
    loghex("[audio_hda] AFG nid=0x", afg);
    log("\n");
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
        loghex("[audio_hda]  out-pin nid=0x", nid);
        loghex(" dev=0x", dev);
        loghex(" conn=0x", conn);
        log("\n");
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
        return 0;
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
        return 0;
    }
    loghex("[audio_hda] DAC nid=0x", dac);
    log("\n");

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

    dac
}

// ── controller bring-up ───────────────────────────────────────────────────
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
    log("[audio_hda] v0.2.0 — generic HDA driver (mailbox streaming) starting\n");

    // Bind the HDA controller by PCI class — hardware-independent, no
    // vendor:device hardcode. Intel cAVS controllers report subclass 0x01
    // ("Audio controller") instead of the canonical 0x03 ("HD Audio"); both
    // expose the same HDA register interface, so accept either.
    let mut bound = pci_bind_class(0x04, 0x03) == 0;
    if !bound {
        bound = pci_bind_class(0x04, 0x01) == 0;
    }
    if !bound {
        log("[audio_hda] no HDA controller (class 04:03 / 04:01) — exit\n");
        return;
    }
    pci_enable_bus_master();
    // Intel quirk: clear TCSEL (PCI 0x44) traffic-class bits so DMA uses TC0.
    let tcsel = pci_read_config(0x44);
    pci_write_config(0x44, tcsel & !0x7);

    let mmio = mmio_map_bar(0, 16);
    if mmio < 0 {
        log("[audio_hda] BAR0 map failed — exit\n");
        return;
    }

    if !reset_controller(mmio) {
        log("[audio_hda] controller reset timeout — exit\n");
        return;
    }
    let gcap = mmio_r16(mmio, GCAP) as u32;
    let iss = (gcap >> 8) & 0xF; // input streams (output streams follow them)
    let oss = (gcap >> 12) & 0xF;
    loghex("[audio_hda] controller up, gcap=0x", gcap);
    log("\n");
    if oss == 0 {
        log("[audio_hda] no output streams — exit\n");
        return;
    }

    let statests = mmio_r16(mmio, STATESTS) as u32;
    loghex("[audio_hda] STATESTS=0x", statests);
    log("\n");
    if statests == 0 {
        log("[audio_hda] no codec detected — exit\n");
        return;
    }
    let mut cad = 0u32;
    while cad < 15 && statests & (1 << cad) == 0 { cad += 1; }
    loghex("[audio_hda] codec addr=0x", cad);
    log("\n");

    let dac = setup_codec(mmio, cad);
    if dac == 0 {
        log("[audio_hda] codec setup failed — exit\n");
        return;
    }

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
        buf[i + 12..i + 16].copy_from_slice(&1u32.to_le_bytes()); // IOC (unused: we poll)
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
    mmio_w32(mmio, base + SD_CTL, (STREAM_TAG << SD_CTL_STRM_SHIFT) | SD_CTL_RUN);
    fence();

    log("[audio_hda] streaming from audio mailbox\n");

    // Streaming loop: keep the half the DMA is NOT playing filled from the
    // kernel mixer. poll_mix always returns a full buffer (silence when idle),
    // so the speaker stays fed and the driver is silent until an app plays.
    // (Resident service — belongs in autostart, not a `run` window.)
    let mut last_filled: usize = 2; // neither half yet
    loop {
        let pos = (mmio_r32(mmio, base + SD_LPIB) as usize) % RING_BYTES;
        let playing = if pos < HALF_BYTES { 0 } else { 1 };
        let fill = 1 - playing;
        if fill != last_filled {
            let mix = unsafe { &mut *core::ptr::addr_of_mut!(MIXBUF) };
            audio_poll_mix(mix);
            dma_write(audio, (fill * HALF_BYTES) as u32, unsafe {
                &*core::ptr::addr_of!(MIXBUF)
            });
            last_filled = fill;
        }
        sleep_ms(8);
    }
}
