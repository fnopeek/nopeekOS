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
const SAMPLE_RATE: usize = 48000;
const TONE_HZ: usize = 480;
const PERIOD: usize = SAMPLE_RATE / TONE_HZ; // 100 frames
const FRAMES: usize = 4800; // 0.1 s, 48 whole periods
const BYTES_PER_FRAME: usize = 4; // S16 stereo
const AUDIO_BYTES: usize = FRAMES * BYTES_PER_FRAME; // 19200
const AMPLITUDE: f64 = 12000.0; // of 32767 (~ -8.7 dBFS)

const STREAM_TAG: u32 = 1;

static mut AUDIO: [u8; AUDIO_BYTES] = [0; AUDIO_BYTES];
static mut SINE: [i16; PERIOD] = [0; PERIOD];

// ── 7th-order Taylor sine (no libm in no_std; wasm has hardware f64) ──────
fn sinf(x: f64) -> f64 {
    let x2 = x * x;
    x * (1.0 - x2 * (1.0 / 6.0 - x2 * (1.0 / 120.0 - x2 * (1.0 / 5040.0))))
}

fn build_tone() {
    const PI: f64 = 3.14159265358979;
    let sine = core::ptr::addr_of_mut!(SINE);
    for j in 0..PERIOD {
        let arg = 2.0 * PI * (j as f64) / (PERIOD as f64); // [0, 2pi)
        let a = if arg > PI { arg - 2.0 * PI } else { arg }; // reduce to [-pi, pi]
        let v = (sinf(a) * AMPLITUDE) as i16;
        unsafe { (*sine)[j] = v };
    }
    let audio = core::ptr::addr_of_mut!(AUDIO);
    for i in 0..FRAMES {
        let v = unsafe { (*sine)[i % PERIOD] }.to_le_bytes();
        let o = i * BYTES_PER_FRAME;
        unsafe {
            (*audio)[o] = v[0];
            (*audio)[o + 1] = v[1]; // left
            (*audio)[o + 2] = v[0];
            (*audio)[o + 3] = v[1]; // right
        }
    }
}

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
    let gain = (steps & 0x7F) as u16;
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
    log("[audio_hda] v0.1.2 — generic HDA driver starting\n");

    // Bind the HDA controller. Prefer by class (HW-independent); fall back to
    // the known Intel vendor:device as a diagnostic — log both return codes so
    // we can see WHY class-bind failed on hardware (-1 not found, -2 denied).
    let rc = pci_bind_class(0x04, 0x03);
    loghex("[audio_hda] bind_class(04:03) rc=0x", rc as u32);
    log("\n");
    if rc != 0 {
        let rc2 = pci_bind(0x8086, 0x9dc8);
        loghex("[audio_hda] bind(8086:9dc8) rc=0x", rc2 as u32);
        log("\n");
        if rc2 != 0 {
            log("[audio_hda] both binds failed — exit\n");
            return;
        }
        log("[audio_hda] bound by vendor:device (class-bind failed — investigate)\n");
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

    // ── build the tone + DMA buffers ──────────────────────────────────────
    build_tone();
    let audio = dma_alloc(((AUDIO_BYTES + 4095) / 4096) as u16);
    let bdl = dma_alloc(1);
    if audio < 0 || bdl < 0 {
        log("[audio_hda] DMA alloc failed — exit\n");
        return;
    }
    let audio_phys = dma_phys(audio);
    dma_write(audio, 0, unsafe { &*core::ptr::addr_of!(AUDIO) });

    // BDL: two equal cyclic entries (HDA wants >= 2; LVI = entries-1).
    let half = (AUDIO_BYTES / 2) as u32;
    let mut bdl_buf = [0u8; 32];
    let mk = |buf: &mut [u8], i: usize, addr: u64, len: u32| {
        buf[i..i + 8].copy_from_slice(&addr.to_le_bytes());
        buf[i + 8..i + 12].copy_from_slice(&len.to_le_bytes());
        buf[i + 12..i + 16].copy_from_slice(&1u32.to_le_bytes()); // IOC
    };
    mk(&mut bdl_buf, 0, audio_phys, half);
    mk(&mut bdl_buf, 16, audio_phys + half as u64, half);
    dma_write(bdl, 0, &bdl_buf);
    let bdl_phys = dma_phys(bdl);

    // ── program the output stream descriptor ──────────────────────────────
    let base = SD_BASE + iss * SD_STRIDE; // first output stream
    reset_stream(mmio, base);
    mmio_w32(mmio, base + SD_BDLPL, (bdl_phys & 0xFFFF_FFFF) as u32);
    mmio_w32(mmio, base + SD_BDLPU, (bdl_phys >> 32) as u32);
    mmio_w32(mmio, base + SD_CBL, AUDIO_BYTES as u32);
    mmio_w16(mmio, base + SD_LVI, 1);
    mmio_w16(mmio, base + SD_FORMAT, FMT_48K_S16_STEREO);
    fence();
    // Stream tag + RUN.
    mmio_w32(mmio, base + SD_CTL, (STREAM_TAG << SD_CTL_STRM_SHIFT) | SD_CTL_RUN);
    fence();

    log("[audio_hda] stream running — tone should be audible\n");

    // Resident loop: keep the module alive (DMA cycles the buffer on its own).
    loop {
        sleep_ms(1000);
    }
}
