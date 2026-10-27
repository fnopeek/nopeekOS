//! The generic codec parser, after Linux 6.18.55: the pin configuration
//! (`common/auto_parser.c` `snd_hda_parse_pin_defcfg`), the widget graph
//! and output paths (`codecs/generic.c` `snd_hda_add_new_path`,
//! `map_singles`, `try_assign_dacs`, `parse_digital`), their activation
//! (`snd_hda_activate_path`, `set_output_and_unmute`), headphone automute
//! by polled pin sense (`check_auto_mute_availability`, `do_automute`, as
//! with `jackpoll_ms`) and the PCM capabilities of the converters
//! (`snd_hdac_query_supported_pcm`).
//!
//! Not implemented: multi-io pins, the analog loopback mixer (`mixer_nid`
//! is 0, as for a codec without a vendor driver), the badness comparison
//! between alternative DAC assignments (the first, hard-wired-first one is
//! taken), inputs.

use crate::regs::*;
use crate::{codec_cmd, log, loghex};

/// `HDA_MAX_OUTS`: outputs of one kind `snd_hda_parse_pin_defcfg` keeps.
pub const HDA_MAX_OUTS: usize = 5;
/// `MAX_NID_PATH_DEPTH`: recursion levels of the path search.
const MAX_NID_PATH_DEPTH: usize = 10;
/// Nodes a path of that many levels holds: one per level plus the pin,
/// and `idx` is written one past the node.
const PATH_NODES: usize = MAX_NID_PATH_DEPTH + 2;
/// Line-out, headphone, speaker and digital outputs, one path each.
const MAX_PATHS: usize = 4 * HDA_MAX_OUTS;
/// A node id is 8 bits in the command dword.
const NIDS: usize = 256;

const AC_VERB_GET_CONNECT_LIST: u32 = 0xf02;
const AC_VERB_GET_PIN_SENSE: u32 = 0xf09;
const AC_VERB_SET_PIN_SENSE: u32 = 0x709;
const AC_PAR_PCM: u32 = 0x0a;
const AC_PAR_STREAM: u32 = 0x0b;
const AC_WCAP_STEREO: u32 = 1 << 0;
const AC_WCAP_AMP_OVRD: u32 = 1 << 3;
const AC_WCAP_FORMAT_OVRD: u32 = 1 << 4;
const AC_WCAP_CONN_LIST: u32 = 1 << 8;
const AC_WCAP_DIGITAL: u32 = 1 << 9;
const AC_WCAP_POWER: u32 = 1 << 10;
const AC_WCAP_CHAN_CNT_EXT: u32 = 7 << 13;
const AC_WID_AUD_OUT: u32 = 0x0;
const AC_WID_AUD_IN: u32 = 0x1;
const AC_WID_AUD_MIX: u32 = 0x2;
const AC_WID_PIN: u32 = 0x4;
const AC_WID_VOL_KNB: u32 = 0x6;
const AC_CLIST_LONG: u32 = 1 << 7;
const AC_CLIST_LENGTH: u32 = 0x7f;
const AC_PINCAP_TRIG_REQ: u32 = 1 << 1;
const AC_PINCAP_PRES_DETECT: u32 = 1 << 2;
const AC_PINCAP_HP_DRV: u32 = 1 << 3;
const AC_PINCAP_OUT: u32 = 1 << 4;
const AC_PINCAP_IN: u32 = 1 << 5;
const AC_PINCAP_EAPD: u32 = 1 << 16;
const AC_PINSENSE_PRESENCE: u32 = 1 << 31;
const AC_PINCTL_OUT_EN: u32 = 1 << 6;
const AC_PINCTL_HP_EN: u32 = 1 << 7;
const PIN_OUT: u32 = AC_PINCTL_OUT_EN;
const PIN_HP: u32 = AC_PINCTL_OUT_EN | AC_PINCTL_HP_EN;
const AC_AMPCAP_OFFSET: u32 = 0x7f;
const AC_AMPCAP_NUM_STEPS: u32 = 0x7f << 8;
const AC_AMPCAP_MIN_MUTE: u32 = 1 << 30;
const HDA_AMP_MUTE: u32 = 0x80;
const AC_DEFCFG_MISC_NO_PRESENCE: u32 = 1 << 0;
const AC_SUPPCM_BITS_16: u32 = 1 << 17;
const AC_SUPPCM_BITS_20: u32 = 1 << 18;
const AC_SUPPCM_BITS_24: u32 = 1 << 19;
const AC_SUPPCM_BITS_32: u32 = 1 << 20;
const AC_SUPFMT_PCM: u32 = 1 << 0;

const AC_JACK_LINE_OUT: u32 = 0x0;
const AC_JACK_SPEAKER: u32 = 0x1;
const AC_JACK_HP_OUT: u32 = 0x2;
const AC_JACK_SPDIF_OUT: u32 = 0x4;
const AC_JACK_DIG_OTHER_OUT: u32 = 0x5;
const AC_JACK_PORT_NONE: u32 = 1;
const AC_JACK_PORT_FIXED: u32 = 2;
const AC_JACK_PORT_BOTH: u32 = 3;

/// `rate_bits`: the AC_PAR_PCM rate bits 0..10, their rate and SDxFMT
/// base/multiplier/divisor (`HDA_RATE`).
pub const RATE_BITS: [(u32, u16); 11] = [
    (8000, hda_rate(48, 1, 6)),
    (11025, hda_rate(44, 1, 4)),
    (16000, hda_rate(48, 1, 3)),
    (22050, hda_rate(44, 1, 2)),
    (32000, hda_rate(48, 2, 3)),
    (44100, hda_rate(44, 1, 1)),
    (48000, hda_rate(48, 1, 1)),
    (88200, hda_rate(44, 2, 1)),
    (96000, hda_rate(48, 2, 1)),
    (176400, hda_rate(44, 4, 1)),
    (192000, hda_rate(48, 4, 1)),
];

const fn hda_rate(base: u32, mult: u16, div: u16) -> u16 {
    (if base == 44 { 1 << 14 } else { 0 }) | ((mult - 1) << 11) | ((div - 1) << 8)
}

/// `snd_hdac_stream_format`: the SDxFMT / converter format word, 0 for a
/// rate or width HDA has no code for.
pub fn stream_format(channels: u32, bits: u32, rate: u32) -> u16 {
    let Some(&(_, r)) = RATE_BITS.iter().find(|(hz, _)| *hz == rate) else { return 0 };
    if channels == 0 || channels > 16 {
        return 0;
    }
    let b = match bits {
        8 => 0,
        16 => 1,
        20 => 2,
        24 => 3,
        32 => 4,
        _ => return 0,
    };
    r | (b << 4) | (channels as u16 - 1)
}

/// `get_wcaps_channels`.
fn wcaps_channels(wcaps: u32) -> u32 {
    let chans = (wcaps & AC_WCAP_CHAN_CNT_EXT) >> 13;
    ((chans << 1) | 1) + 1
}

/// Where the primary outputs go (`line_out_type`).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum OutType {
    Line,
    Speaker,
    Hp,
}

/// `struct auto_pin_cfg`, the output half.
#[derive(Clone, Copy)]
pub struct AutoCfg {
    pub line_outs: usize,
    pub line_out_pins: [u8; HDA_MAX_OUTS],
    pub speaker_outs: usize,
    pub speaker_pins: [u8; HDA_MAX_OUTS],
    pub hp_outs: usize,
    pub hp_pins: [u8; HDA_MAX_OUTS],
    pub line_out_type: OutType,
    pub dig_outs: usize,
    pub dig_out_pins: [u8; HDA_MAX_OUTS],
}

/// `struct nid_path`: `path[0]` is the DAC, `path[depth - 1]` the pin;
/// `idx[i]` is the connection of `path[i]` that leads to `path[i - 1]`.
#[derive(Clone, Copy)]
struct Path {
    depth: usize,
    path: [u8; PATH_NODES],
    idx: [u8; PATH_NODES],
    multi: [bool; PATH_NODES],
    active: bool,
}

const NO_PATH: Path = Path {
    depth: 0, path: [0; PATH_NODES], idx: [0; PATH_NODES], multi: [false; PATH_NODES], active: false,
};

/// One codec on the link, its widget capabilities read once.
pub struct Codec {
    pub mmio: i32,
    pub cad: u32,
    pub afg: u32,
    start: u32,
    end: u32,
    wcaps: [u32; NIDS],
}

impl Codec {
    pub fn new(mmio: i32, cad: u32, afg: u32) -> Self {
        let mut c = Codec { mmio, cad, afg, start: 0, end: 0, wcaps: [0; NIDS] };
        c.read_nodes();
        c
    }

    /// `snd_hda_codec_update_widgets`: the widget range under the AFG and
    /// each widget's capabilities.
    pub fn read_nodes(&mut self) {
        let w = self.param(self.afg, PARAM_SUB_NODE_COUNT);
        self.start = (w >> 16) & 0xff;
        self.end = (self.start + (w & 0xff)).min(NIDS as u32);
        self.wcaps = [0; NIDS];
        for nid in self.start..self.end {
            self.wcaps[nid as usize] = self.param(nid, PARAM_AUDIO_WIDGET_CAP);
        }
    }

    pub fn nodes(&self) -> core::ops::Range<u32> {
        self.start..self.end
    }

    pub fn read(&self, nid: u32, verb: u32) -> u32 {
        codec_cmd(self.mmio, self.cad, nid, verb).unwrap_or(0)
    }

    pub fn write(&self, nid: u32, verb: u32) {
        codec_cmd(self.mmio, self.cad, nid, verb);
    }

    pub fn param(&self, nid: u32, p: u32) -> u32 {
        self.read(nid, vget_param(p))
    }

    pub fn wcaps(&self, nid: u32) -> u32 {
        if (self.start..self.end).contains(&nid) { self.wcaps[nid as usize] } else { 0 }
    }

    pub fn wtype(&self, nid: u32) -> u32 {
        (self.wcaps(nid) >> 20) & 0xf
    }

    pub fn pin_caps(&self, nid: u32) -> u32 {
        self.param(nid, PARAM_PIN_CAP)
    }

    pub fn pincfg(&self, nid: u32) -> u32 {
        self.read(nid, vget_config_default())
    }

    /// `snd_hdac_get_connections`, into `out`; the number of entries.
    pub fn connections(&self, nid: u32, out: &mut [u8; NIDS]) -> usize {
        let wcaps = self.wcaps(nid);
        if wcaps & AC_WCAP_CONN_LIST == 0 && (wcaps >> 20) & 0xf != AC_WID_VOL_KNB {
            return 0;
        }
        let parm = self.param(nid, PARAM_CONN_LIST_LEN);
        let (shift, num_elems) = if parm & AC_CLIST_LONG != 0 { (16, 2) } else { (8, 4) };
        let conn_len = parm & AC_CLIST_LENGTH;
        let mask = (1u32 << (shift - 1)) - 1;
        if conn_len == 0 {
            return 0;
        }
        let mut push = |conns: &mut usize, n: u32| {
            if (n as usize) < NIDS && *conns < NIDS {
                out[*conns] = n as u8;
                *conns += 1;
            }
        };
        let mut conns = 0;
        if conn_len == 1 {
            let p = self.read(nid, (AC_VERB_GET_CONNECT_LIST << 8) | 0);
            push(&mut conns, p & mask);
            return conns;
        }
        let mut prev_nid = 0u32;
        let mut null_count = 0;
        let mut p = 0u32;
        for i in 0..conn_len {
            if i % num_elems == 0 {
                p = self.read(nid, (AC_VERB_GET_CONNECT_LIST << 8) | (i & 0xff));
            }
            let range_val = p & (1 << (shift - 1)) != 0;
            let val = p & mask;
            if val == 0 {
                null_count += 1;
                if null_count > 1 {
                    return 0;
                }
            }
            p >>= shift;
            if range_val {
                if prev_nid == 0 || prev_nid >= val {
                    continue;
                }
                for n in prev_nid + 1..=val {
                    push(&mut conns, n);
                }
            } else {
                push(&mut conns, val);
            }
            prev_nid = val;
        }
        conns
    }

    /// `snd_hda_get_conn_index`: the connection of `mux` that leads to
    /// `nid`, through mixers and selectors when `recursive` is non-zero.
    fn conn_index(&self, mux: u32, nid: u32, recursive: u32) -> Option<usize> {
        let mut conn = [0u8; NIDS];
        let nums = self.connections(mux, &mut conn);
        if let Some(i) = conn[..nums].iter().position(|&c| c as u32 == nid) {
            return Some(i);
        }
        if recursive == 0 || recursive > 10 {
            return None;
        }
        (0..nums).find(|&i| {
            let t = self.wtype(conn[i] as u32);
            t != AC_WID_PIN && t != AC_WID_AUD_OUT && self.conn_index(conn[i] as u32, nid, recursive + 1).is_some()
        })
    }

    /// `is_reachable_path`.
    fn reachable(&self, from: u32, to: u32) -> bool {
        from != 0 && to != 0 && self.conn_index(to, from, 1).is_some()
    }

    /// `query_amp_caps`: a widget without the override bit uses the AFG's.
    fn amp_caps(&self, nid: u32, out: bool) -> u32 {
        let nid = if self.wcaps(nid) & AC_WCAP_AMP_OVRD != 0 { nid } else { self.afg };
        self.param(nid, if out { PARAM_AMP_OUT_CAP } else { PARAM_AMP_IN_CAP })
    }

    /// `snd_hdac_query_supported_pcm`: the AC_PAR_PCM rate bits and the
    /// sample widths (16, 20, 24, 32 bits as AC_SUPPCM bits) a converter
    /// takes; `(0, 0)` when it takes no PCM.
    pub fn supported_pcm(&self, nid: u32) -> (u32, u32) {
        let mut val = 0;
        if nid != self.afg && self.wcaps(nid) & AC_WCAP_FORMAT_OVRD != 0 {
            val = self.param(nid, AC_PAR_PCM);
        }
        if val == 0 || val == u32::MAX {
            val = self.param(self.afg, AC_PAR_PCM);
        }
        let mut streams = self.param(nid, AC_PAR_STREAM);
        if streams == 0 || streams == u32::MAX {
            streams = self.param(self.afg, AC_PAR_STREAM);
        }
        if streams & AC_SUPFMT_PCM == 0 {
            return (0, 0);
        }
        let mut widths = val & (AC_SUPPCM_BITS_16 | AC_SUPPCM_BITS_20 | AC_SUPPCM_BITS_24);
        // 32 bits on a digital converter is an IEC958 subframe, not PCM.
        if val & AC_SUPPCM_BITS_32 != 0 && self.wcaps(nid) & AC_WCAP_DIGITAL == 0 {
            widths |= AC_SUPPCM_BITS_32;
        }
        (val & ((1 << RATE_BITS.len()) - 1), widths)
    }

    /// `read_pin_sense`: is something plugged into `pin`?
    fn jack_present(&self, pin: u32) -> bool {
        if self.pin_caps(pin) & AC_PINCAP_TRIG_REQ != 0 {
            self.read(pin, AC_VERB_SET_PIN_SENSE << 8);
        }
        self.read(pin, AC_VERB_GET_PIN_SENSE << 8) & AC_PINSENSE_PRESENCE != 0
    }

    /// `is_jack_detectable`, with polling standing in for unsolicited
    /// events.
    fn jack_detectable(&self, pin: u32) -> bool {
        self.pin_caps(pin) & AC_PINCAP_PRES_DETECT != 0
            && (self.pincfg(pin) >> 8) & 0xf & AC_DEFCFG_MISC_NO_PRESENCE == 0
    }

    /// `snd_hda_correct_pin_ctl`, the output half.
    fn correct_pin_ctl(&self, pin: u32, mut val: u32) -> u32 {
        let cap = self.pin_caps(pin);
        if val == 0 || cap == 0 {
            return val;
        }
        if val & AC_PINCTL_OUT_EN != 0 {
            if cap & AC_PINCAP_OUT == 0 {
                val &= !(AC_PINCTL_OUT_EN | AC_PINCTL_HP_EN);
            } else if val & AC_PINCTL_HP_EN != 0 && cap & AC_PINCAP_HP_DRV == 0 {
                val &= !AC_PINCTL_HP_EN;
            }
        }
        val
    }
}

/// `check_pincap_validity`.
fn pincap_valid(c: &Codec, pin: u32, dev: u32) -> bool {
    let cap = c.pin_caps(pin);
    if cap == 0 {
        return true;
    }
    match dev {
        AC_JACK_LINE_OUT | AC_JACK_SPEAKER | AC_JACK_HP_OUT | AC_JACK_SPDIF_OUT | AC_JACK_DIG_OTHER_OUT => {
            cap & AC_PINCAP_OUT != 0
        }
        _ => cap & AC_PINCAP_IN != 0,
    }
}

/// `snd_hda_parse_pin_defcfg`, outputs only.
pub fn parse_pin_defcfg(c: &Codec) -> AutoCfg {
    let mut cfg = AutoCfg {
        line_outs: 0, line_out_pins: [0; HDA_MAX_OUTS], speaker_outs: 0, speaker_pins: [0; HDA_MAX_OUTS],
        hp_outs: 0, hp_pins: [0; HDA_MAX_OUTS], line_out_type: OutType::Line, dig_outs: 0,
        dig_out_pins: [0; HDA_MAX_OUTS],
    };
    // (pin, sequence) per kind, sorted by sequence below.
    let mut line_out = [(0u8, 0u32); HDA_MAX_OUTS];
    let mut speaker_out = [(0u8, 0u32); HDA_MAX_OUTS];
    let mut hp_out = [(0u8, 0u32); HDA_MAX_OUTS];
    let mut assoc_line_out = 0;

    for nid in c.nodes() {
        if c.wtype(nid) != AC_WID_PIN {
            continue;
        }
        let def_conf = c.pincfg(nid);
        let conn = (def_conf >> 30) & 0x3;
        if conn == AC_JACK_PORT_NONE {
            continue;
        }
        let mut dev = (def_conf >> 20) & 0xf;
        // Workaround for buggy BIOS setups.
        if dev == AC_JACK_LINE_OUT && (conn == AC_JACK_PORT_FIXED || conn == AC_JACK_PORT_BOTH) {
            dev = AC_JACK_SPEAKER;
        }
        if !pincap_valid(c, nid, dev) {
            continue;
        }
        let seq = def_conf & 0xf;
        let assoc = (def_conf >> 4) & 0xf;
        let ignore = |nid: u32| {
            loghex("[audio_hda] ignore pin 0x", nid);
            log(", too many assigned pins\n");
        };
        match dev {
            AC_JACK_LINE_OUT => {
                if assoc == 0 {
                    continue;
                }
                if assoc_line_out == 0 {
                    assoc_line_out = assoc;
                } else if assoc_line_out != assoc {
                    loghex("[audio_hda] ignore pin 0x", nid);
                    loghex(" with mismatching assoc# 0x", assoc);
                    loghex(" vs 0x", assoc_line_out);
                    log("\n");
                    continue;
                }
                if cfg.line_outs >= HDA_MAX_OUTS {
                    ignore(nid);
                    continue;
                }
                line_out[cfg.line_outs] = (nid as u8, seq);
                cfg.line_outs += 1;
            }
            AC_JACK_SPEAKER => {
                if cfg.speaker_outs >= HDA_MAX_OUTS {
                    ignore(nid);
                    continue;
                }
                speaker_out[cfg.speaker_outs] = (nid as u8, (assoc << 4) | seq);
                cfg.speaker_outs += 1;
            }
            AC_JACK_HP_OUT => {
                if cfg.hp_outs >= HDA_MAX_OUTS {
                    ignore(nid);
                    continue;
                }
                hp_out[cfg.hp_outs] = (nid as u8, (assoc << 4) | seq);
                cfg.hp_outs += 1;
            }
            AC_JACK_SPDIF_OUT | AC_JACK_DIG_OTHER_OUT => {
                if cfg.dig_outs >= HDA_MAX_OUTS {
                    ignore(nid);
                    continue;
                }
                cfg.dig_out_pins[cfg.dig_outs] = nid as u8;
                cfg.dig_outs += 1;
            }
            _ => {}
        }
    }

    // If no line-out is defined but multiple HPs are found, some of them
    // might be the real line-outs: the real HPs have sequence 0xf.
    if cfg.line_outs == 0 && cfg.hp_outs > 1 {
        let mut i = 0;
        while i < cfg.hp_outs {
            if hp_out[i].1 & 0xf == 0xf {
                i += 1;
                continue;
            }
            line_out[cfg.line_outs] = hp_out[i];
            cfg.line_outs += 1;
            cfg.hp_outs -= 1;
            hp_out.copy_within(i + 1..cfg.hp_outs + 1, i);
        }
        for e in hp_out[cfg.hp_outs..].iter_mut() {
            *e = (0, 0);
        }
        if cfg.hp_outs == 0 {
            cfg.line_out_type = OutType::Hp;
        }
    }

    sort_pins_by_sequence(&mut cfg.line_out_pins, &mut line_out[..cfg.line_outs]);
    sort_pins_by_sequence(&mut cfg.speaker_pins, &mut speaker_out[..cfg.speaker_outs]);
    sort_pins_by_sequence(&mut cfg.hp_pins, &mut hp_out[..cfg.hp_outs]);

    // No line-out: the speaker or HP pins are the primary output.
    if cfg.line_outs == 0 {
        if cfg.speaker_outs != 0 {
            cfg.line_outs = cfg.speaker_outs;
            cfg.line_out_pins = cfg.speaker_pins;
            cfg.speaker_outs = 0;
            cfg.speaker_pins = [0; HDA_MAX_OUTS];
            cfg.line_out_type = OutType::Speaker;
        } else if cfg.hp_outs != 0 {
            cfg.line_outs = cfg.hp_outs;
            cfg.line_out_pins = cfg.hp_pins;
            cfg.hp_outs = 0;
            cfg.hp_pins = [0; HDA_MAX_OUTS];
            cfg.line_out_type = OutType::Hp;
        }
    }

    reorder_outputs(cfg.line_outs, &mut cfg.line_out_pins);
    reorder_outputs(cfg.hp_outs, &mut cfg.hp_pins);
    reorder_outputs(cfg.speaker_outs, &mut cfg.speaker_pins);
    cfg
}

/// `sort_pins_by_sequence` (a stable sort, as the list is short).
fn sort_pins_by_sequence(pins: &mut [u8; HDA_MAX_OUTS], list: &mut [(u8, u32)]) {
    for i in 1..list.len() {
        let mut j = i;
        while j > 0 && list[j - 1].1 > list[j].1 {
            list.swap(j - 1, j);
            j -= 1;
        }
    }
    for (i, e) in list.iter().enumerate() {
        pins[i] = e.0;
    }
}

/// `reorder_outputs`: HDA orders 6 channels front/clfe/surr, ALSA
/// front/surr/clfe.
fn reorder_outputs(nums: usize, pins: &mut [u8; HDA_MAX_OUTS]) {
    if nums == 3 || nums == 4 {
        pins.swap(1, 2);
    }
}

/// The parsed output side of one codec (`struct hda_gen_spec`, the parts
/// one playback stream needs).
pub struct Spec {
    pub codec: Codec,
    pub cfg: AutoCfg,
    paths: [Path; MAX_PATHS],
    num_paths: usize,
    all_dacs: [u8; NIDS],
    num_all_dacs: usize,
    /// `private_dac_nids`, `hp_out_nid`, `extra_out_nid`, digital DACs.
    pub dacs: [u8; HDA_MAX_OUTS],
    pub hp_dacs: [u8; HDA_MAX_OUTS],
    pub speaker_dacs: [u8; HDA_MAX_OUTS],
    pub dig_dacs: [u8; HDA_MAX_OUTS],
    /// Path indices plus one; 0 = none (`out_paths`, `hp_paths`, ...).
    out_paths: [usize; HDA_MAX_OUTS],
    hp_paths: [usize; HDA_MAX_OUTS],
    speaker_paths: [usize; HDA_MAX_OUTS],
    dig_paths: [usize; HDA_MAX_OUTS],
    /// Pin control targets (`snd_hda_codec_set_pin_target`).
    pin_target: [u8; NIDS],
    detect_hp: bool,
    detect_lo: bool,
    automute_speaker: bool,
    automute_lo: bool,
    hp_present: bool,
    line_present: bool,
}

impl Spec {
    /// `snd_hda_gen_parse_auto_config`, outputs only.
    pub fn parse(codec: Codec) -> Spec {
        let cfg = parse_pin_defcfg(&codec);
        let mut s = Spec {
            codec, cfg, paths: [NO_PATH; MAX_PATHS], num_paths: 0, all_dacs: [0; NIDS], num_all_dacs: 0,
            dacs: [0; HDA_MAX_OUTS], hp_dacs: [0; HDA_MAX_OUTS], speaker_dacs: [0; HDA_MAX_OUTS],
            dig_dacs: [0; HDA_MAX_OUTS], out_paths: [0; HDA_MAX_OUTS], hp_paths: [0; HDA_MAX_OUTS],
            speaker_paths: [0; HDA_MAX_OUTS], dig_paths: [0; HDA_MAX_OUTS], pin_target: [0; NIDS],
            detect_hp: false, detect_lo: false, automute_speaker: false, automute_lo: false,
            hp_present: false, line_present: false,
        };
        s.fill_all_dac_nids();
        s.parse_output_paths();
        s.parse_digital();
        s.check_auto_mute_availability();
        s
    }

    /// Is there an analog output with a path from a DAC?
    pub fn has_analog(&self) -> bool {
        self.out_paths.iter().chain(&self.hp_paths).chain(&self.speaker_paths).any(|&p| p != 0)
    }

    /// Is there any output with a path?
    pub fn has_output(&self) -> bool {
        self.has_analog() || self.dig_paths.iter().any(|&p| p != 0)
    }

    /// Every DAC with an active path, front DACs first, each once.
    pub fn used_dacs(&self, out: &mut [u8; 4 * HDA_MAX_OUTS]) -> usize {
        let mut n = 0;
        for &d in self.dacs.iter().chain(&self.hp_dacs).chain(&self.speaker_dacs).chain(&self.dig_dacs) {
            if d != 0 && !out[..n].contains(&d) {
                out[n] = d;
                n += 1;
            }
        }
        n
    }

    fn fill_all_dac_nids(&mut self) {
        for nid in self.codec.nodes() {
            if self.codec.wtype(nid) == AC_WID_AUD_OUT {
                self.all_dacs[self.num_all_dacs] = nid as u8;
                self.num_all_dacs += 1;
            }
        }
    }

    fn path(&self, idx: usize) -> Option<&Path> {
        if idx == 0 || idx > self.num_paths { None } else { Some(&self.paths[idx - 1]) }
    }

    /// `is_dac_already_used`.
    fn dac_used(&self, nid: u32) -> bool {
        self.paths[..self.num_paths].iter().any(|p| p.depth > 0 && p.path[0] as u32 == nid)
    }

    /// `__parse_nid_path`, called recursively from the pin towards the DAC.
    fn parse_nid_path_inner(&self, from: u32, to: u32, mut anchor: i32, path: &mut Path, depth: usize) -> bool {
        let c = &self.codec;
        if to as i32 == anchor {
            anchor = 0;
        } else if to as i32 == -anchor {
            return false;
        }
        let mut conn = [0u8; NIDS];
        let nums = c.connections(to, &mut conn);
        let mut found = None;
        for (i, &n) in conn[..nums].iter().enumerate() {
            let n = n as u32;
            if n != from {
                // From 0: any DAC not in use yet.
                if from != 0 || c.wtype(n) != AC_WID_AUD_OUT || self.dac_used(n) {
                    continue;
                }
            }
            if anchor <= 0 {
                found = Some(i);
                break;
            }
        }
        if found.is_none() {
            if depth >= MAX_NID_PATH_DEPTH {
                return false;
            }
            for (i, &n) in conn[..nums].iter().enumerate() {
                let t = c.wtype(n as u32);
                if t == AC_WID_AUD_OUT || t == AC_WID_AUD_IN || t == AC_WID_PIN {
                    continue;
                }
                if self.parse_nid_path_inner(from, n as u32, anchor, path, depth + 1) {
                    found = Some(i);
                    break;
                }
            }
        }
        let Some(i) = found else { return false };
        path.path[path.depth] = conn[i];
        path.idx[path.depth + 1] = i as u8;
        if nums > 1 && c.wtype(to) != AC_WID_AUD_MIX {
            path.multi[path.depth + 1] = true;
        }
        path.depth += 1;
        true
    }

    /// `snd_hda_add_new_path`: the path index plus one, 0 for none.
    fn add_new_path(&mut self, from: u32, to: u32, anchor: i32) -> usize {
        if from != 0 && to != 0 && !self.codec.reachable(from, to) {
            return 0;
        }
        // An equal path already added.
        for (i, p) in self.paths[..self.num_paths].iter().enumerate() {
            if p.depth == 0 {
                continue;
            }
            let ends = (from == 0 || p.path[0] as u32 == from) && (to == 0 || p.path[p.depth - 1] as u32 == to);
            let contains = |n: i32| p.path[..p.depth].iter().any(|&x| x as i32 == n);
            if ends && (anchor == 0 || (anchor > 0 && contains(anchor)) || (anchor < 0 && !contains(anchor))) {
                return i + 1;
            }
        }
        if self.num_paths >= MAX_PATHS {
            loghex("[audio_hda] no room for a path to pin 0x", to);
            log("\n");
            return 0;
        }
        let mut path = NO_PATH;
        // `snd_hda_parse_nid_path`.
        if !self.parse_nid_path_inner(from, to, anchor, &mut path, 1) {
            return 0;
        }
        path.path[path.depth] = to as u8;
        path.depth += 1;
        self.paths[self.num_paths] = path;
        self.num_paths += 1;
        self.num_paths
    }

    fn invalidate_path(&mut self, idx: usize) {
        if idx != 0 && idx <= self.num_paths {
            self.paths[idx - 1] = NO_PATH;
        }
    }

    /// `look_for_dac`.
    fn look_for_dac(&self, pin: u32, digital: bool) -> u32 {
        for &d in &self.all_dacs[..self.num_all_dacs] {
            let d = d as u32;
            if self.dac_used(d) {
                continue;
            }
            if (self.codec.wcaps(d) & AC_WCAP_DIGITAL != 0) != digital {
                continue;
            }
            if self.codec.reachable(d, pin) {
                return d;
            }
        }
        0
    }

    /// `get_dac_if_single`.
    fn dac_if_single(&self, pin: u32) -> u32 {
        let mut found = 0;
        for &d in &self.all_dacs[..self.num_all_dacs] {
            let d = d as u32;
            if self.dac_used(d) {
                continue;
            }
            if self.codec.reachable(d, pin) {
                if found != 0 {
                    return 0;
                }
                found = d;
            }
        }
        found
    }

    /// `map_singles`: pins wired to exactly one free DAC get it first.
    fn map_singles(&mut self, outs: usize, pins: [u8; HDA_MAX_OUTS], dacs: &mut [u8; HDA_MAX_OUTS],
                   path_idx: &mut [usize; HDA_MAX_OUTS]) -> bool {
        let mut found = false;
        for i in 0..outs {
            if dacs[i] != 0 {
                continue;
            }
            let dac = self.dac_if_single(pins[i] as u32);
            if dac == 0 {
                continue;
            }
            let p = self.add_new_path(dac, pins[i] as u32, 0);
            if p != 0 {
                dacs[i] = dac as u8;
                found = true;
                self.paths[p - 1].active = true;
                path_idx[i] = p;
            }
        }
        found
    }

    /// `get_primary_out`.
    fn primary_out(&self, idx: usize) -> u32 {
        if idx < self.cfg.line_outs { self.dacs[idx] as u32 } else { 0 }
    }

    /// `try_assign_dacs`, without the badness score.
    fn try_assign_dacs(&mut self, outs: usize, pins: [u8; HDA_MAX_OUTS], dacs: &mut [u8; HDA_MAX_OUTS],
                       path_idx: &mut [usize; HDA_MAX_OUTS]) {
        for i in 0..outs {
            let pin = pins[i] as u32;
            if self.path(path_idx[i]).is_some_and(|p| p.depth > 0) {
                continue;
            }
            if dacs[i] == 0 {
                dacs[i] = self.look_for_dac(pin, false) as u8;
            }
            if dacs[i] == 0 && i == 0 {
                // Steal the DAC of a surround for the front.
                for j in 1..outs {
                    if self.codec.reachable(dacs[j] as u32, pin) {
                        dacs[0] = dacs[j];
                        dacs[j] = 0;
                        self.invalidate_path(path_idx[j]);
                        path_idx[j] = 0;
                        break;
                    }
                }
            }
            let mut dac = dacs[i] as u32;
            if dac == 0 {
                let try_dac = |d: u32| if self.codec.reachable(d, pin) { d } else { 0 };
                if outs > 2 {
                    dac = try_dac(self.primary_out(i));
                }
                if dac == 0 {
                    dac = try_dac(dacs[0] as u32);
                }
                if dac == 0 {
                    dac = try_dac(self.primary_out(i));
                }
                if dac == 0 && self.codec.reachable(self.dacs[0] as u32, pin) {
                    dac = self.dacs[0] as u32;
                }
            }
            if dac == 0 {
                continue;
            }
            let p = self.add_new_path(dac, pin, 0);
            if p == 0 {
                dacs[i] = 0;
            } else {
                self.paths[p - 1].active = true;
                path_idx[i] = p;
            }
        }
    }

    /// `refill_shared_dacs`.
    fn refill_shared_dacs(&self, outs: usize, dacs: &mut [u8; HDA_MAX_OUTS], path_idx: &[usize; HDA_MAX_OUTS]) {
        for i in 0..outs {
            if dacs[i] == 0 {
                if let Some(p) = self.path(path_idx[i]) {
                    dacs[i] = p.path[0];
                }
            }
        }
    }

    /// `parse_output_paths` with `fill_and_eval_dacs(codec, true, false)`.
    fn parse_output_paths(&mut self) {
        let cfg = self.cfg;
        let (mut dacs, mut hp, mut spk) = ([0u8; HDA_MAX_OUTS], [0u8; HDA_MAX_OUTS], [0u8; HDA_MAX_OUTS]);
        let (mut outp, mut hpp, mut spkp) = ([0usize; HDA_MAX_OUTS], [0usize; HDA_MAX_OUTS], [0usize; HDA_MAX_OUTS]);
        loop {
            let mut mapped = self.map_singles(cfg.line_outs, cfg.line_out_pins, &mut dacs, &mut outp);
            mapped |= self.map_singles(cfg.hp_outs, cfg.hp_pins, &mut hp, &mut hpp);
            mapped |= self.map_singles(cfg.speaker_outs, cfg.speaker_pins, &mut spk, &mut spkp);
            if !mapped {
                break;
            }
        }
        self.dacs = dacs;
        self.try_assign_dacs(cfg.line_outs, cfg.line_out_pins, &mut dacs, &mut outp);
        self.dacs = dacs;
        if cfg.line_out_type != OutType::Hp {
            self.try_assign_dacs(cfg.hp_outs, cfg.hp_pins, &mut hp, &mut hpp);
        }
        if cfg.line_out_type != OutType::Speaker {
            self.try_assign_dacs(cfg.speaker_outs, cfg.speaker_pins, &mut spk, &mut spkp);
        }
        self.refill_shared_dacs(cfg.hp_outs, &mut hp, &hpp);
        self.refill_shared_dacs(cfg.speaker_outs, &mut spk, &spkp);
        self.dacs = dacs;
        self.hp_dacs = hp;
        self.speaker_dacs = spk;
        self.out_paths = outp;
        self.hp_paths = hpp;
        self.speaker_paths = spkp;

        let line_val = if cfg.line_out_type == OutType::Hp { PIN_HP } else { PIN_OUT };
        for &p in &cfg.line_out_pins[..cfg.line_outs] {
            self.set_pin_target(p as u32, line_val);
        }
        for &p in &cfg.hp_pins[..cfg.hp_outs] {
            self.set_pin_target(p as u32, PIN_HP);
        }
        for &p in &cfg.speaker_pins[..cfg.speaker_outs] {
            self.set_pin_target(p as u32, PIN_OUT);
        }
    }

    /// `parse_digital`, outputs.
    fn parse_digital(&mut self) {
        for i in 0..self.cfg.dig_outs {
            let pin = self.cfg.dig_out_pins[i] as u32;
            let dig_nid = self.look_for_dac(pin, true);
            if dig_nid == 0 {
                continue;
            }
            let p = self.add_new_path(dig_nid, pin, 0);
            if p == 0 {
                continue;
            }
            self.paths[p - 1].active = true;
            self.dig_paths[i] = p;
            self.dig_dacs[i] = dig_nid as u8;
            self.set_pin_target(pin, PIN_OUT);
        }
    }

    fn set_pin_target(&mut self, pin: u32, val: u32) {
        if pin != 0 {
            self.pin_target[pin as usize] = self.codec.correct_pin_ctl(pin, val) as u8;
        }
    }

    /// `check_auto_mute_availability`.
    fn check_auto_mute_availability(&mut self) {
        let cfg = &mut self.cfg;
        let present = [cfg.hp_pins[0], cfg.line_out_pins[0], cfg.speaker_pins[0]].iter().filter(|&&p| p != 0).count();
        if present < 2 {
            return;
        }
        if cfg.speaker_pins[0] == 0 && cfg.line_out_type == OutType::Speaker {
            cfg.speaker_pins = cfg.line_out_pins;
            cfg.speaker_outs = cfg.line_outs;
            self.speaker_paths = self.out_paths;
        }
        if cfg.hp_pins[0] == 0 && cfg.line_out_type == OutType::Hp {
            cfg.hp_pins = cfg.line_out_pins;
            cfg.hp_outs = cfg.line_outs;
            self.hp_paths = self.out_paths;
        }
        let cfg = self.cfg;
        self.detect_hp = cfg.hp_pins[..cfg.hp_outs].iter().any(|&p| self.codec.jack_detectable(p as u32));
        if cfg.line_out_type == OutType::Line && cfg.line_outs != 0 {
            if cfg.speaker_outs != 0 {
                self.detect_lo = cfg.line_out_pins[..cfg.line_outs].iter().any(|&p| self.codec.jack_detectable(p as u32));
            }
            self.automute_lo = self.detect_hp;
        }
        self.automute_speaker = cfg.speaker_outs != 0 && (self.detect_hp || self.detect_lo);
    }

    /// `snd_hda_activate_path` for an output path being enabled.
    fn activate_path(&self, p: &Path) {
        let c = &self.codec;
        // `path_power_update`: every widget of the path in D0.
        for &nid in &p.path[..p.depth] {
            let nid = nid as u32;
            if nid != c.afg && c.wcaps(nid) & AC_WCAP_POWER != 0 {
                c.write(nid, vset_power(0));
            }
        }
        for i in (0..p.depth).rev() {
            let nid = p.path[i] as u32;
            let wcaps = c.wcaps(nid);
            let t = (wcaps >> 20) & 0xf;
            if p.multi[i] {
                c.write(nid, vset_conn_select(p.idx[i] as u32));
            }
            // `has_amp_in`: input amps of every widget but the output pin.
            if wcaps & WCAP_IN_AMP != 0 && !(t == AC_WID_PIN && i > 0) {
                self.activate_amp_in(p, i);
            }
            // `has_amp_out`: output amps of every widget but an input pin.
            if wcaps & WCAP_OUT_AMP != 0 && !(t == AC_WID_PIN && i == 0) {
                self.set_amp(nid, true, 0, true);
            }
        }
    }

    /// `activate_amp_in`: the input that carries the path is opened, the
    /// others start muted (`init_amp`).
    fn activate_amp_in(&self, p: &Path, i: usize) {
        let c = &self.codec;
        let nid = p.path[i] as u32;
        let mut conn = [0u8; NIDS];
        let mut nums = c.connections(nid, &mut conn);
        let idx = if c.wtype(nid) == AC_WID_PIN {
            nums = 1;
            0
        } else {
            p.idx[i] as usize
        };
        for n in 0..nums {
            self.set_amp(nid, false, n as u32, n == idx);
        }
    }

    /// `get_amp_val_to_activate` written with `update_amp`: 0 dB and
    /// unmuted, or muted.
    fn set_amp(&self, nid: u32, out: bool, idx: u32, enable: bool) {
        let c = &self.codec;
        let caps = c.amp_caps(nid, out);
        let mut val = 0;
        if caps & AC_AMPCAP_NUM_STEPS != 0 && enable {
            val = caps & AC_AMPCAP_OFFSET;
        }
        if caps & (AMP_CAP_MUTE | AC_AMPCAP_MIN_MUTE) != 0 && !enable {
            val |= HDA_AMP_MUTE;
        }
        // An amp without steps or a mute switch has nothing to set.
        if caps & (AC_AMPCAP_NUM_STEPS | AMP_CAP_MUTE | AC_AMPCAP_MIN_MUTE) == 0 {
            return;
        }
        // Bit 15 output, 14 input, 13 left, 12 right; right only on a
        // stereo amp.
        let chans: u32 = if c.wcaps(nid) & AC_WCAP_STEREO != 0 { 0x3000 } else { 0x2000 };
        let dir: u32 = if out { 0x8000 } else { 0x4000 };
        c.write(nid, vset_amp((dir | chans | ((idx & 0xf) << 8) | val) as u16));
    }

    /// `set_pin_eapd`.
    fn set_pin_eapd(&self, pin: u32, enable: bool) {
        if self.codec.pin_caps(pin) & AC_PINCAP_EAPD == 0 {
            return;
        }
        self.codec.write(pin, vset_eapd(if enable { EAPD_ENABLE } else { 0 }));
    }

    /// `set_output_and_unmute`.
    fn set_output_and_unmute(&self, idx: usize) {
        let Some(p) = self.path(idx) else { return };
        if p.depth == 0 {
            return;
        }
        let pin = p.path[p.depth - 1] as u32;
        self.codec.write(pin, vset_pin_ctl(self.pin_target[pin as usize] as u32));
        self.activate_path(p);
        self.set_pin_eapd(pin, p.active);
    }

    /// `snd_hda_gen_init`, outputs: the primary, extra and digital outputs
    /// set up, then the automute state applied.
    pub fn init(&mut self) {
        let cfg = self.cfg;
        for i in 0..cfg.line_outs {
            self.set_output_and_unmute(self.out_paths[i]);
        }
        if cfg.line_out_type != OutType::Hp {
            for i in 0..cfg.hp_outs {
                self.set_output_and_unmute(self.hp_paths[i]);
            }
        }
        if cfg.line_out_type != OutType::Speaker {
            for i in 0..cfg.speaker_outs {
                self.set_output_and_unmute(self.speaker_paths[i]);
            }
        }
        for i in 0..cfg.dig_outs {
            self.set_output_and_unmute(self.dig_paths[i]);
        }
        self.update_automute(true);
    }

    /// `snd_hda_codec_setup_stream` for every DAC in use. Front DACs take
    /// channel pairs in order while the stream has them and mirror the
    /// front otherwise, as `snd_hda_multi_out_analog_prepare`.
    pub fn setup_stream(&self, tag: u32, format: u16, channels: u32) {
        let c = &self.codec;
        let mut done = [0u8; 4 * HDA_MAX_OUTS];
        let mut n = 0;
        let mut setup = |dac: u8, ch: u32| {
            if dac == 0 || done[..n].contains(&dac) {
                return;
            }
            done[n] = dac;
            n += 1;
            c.write(dac as u32, vset_stream_chan((tag << 4) | ch));
            c.write(dac as u32, vset_format(format));
        };
        for (i, &d) in self.dacs.iter().enumerate() {
            setup(d, if channels >= (i as u32 + 1) * 2 { i as u32 * 2 } else { 0 });
        }
        for &d in self.hp_dacs.iter().chain(&self.speaker_dacs).chain(&self.dig_dacs) {
            setup(d, 0);
        }
    }

    /// The channels the front DAC takes (`get_wcaps_channels`), at most
    /// `want`.
    pub fn channels(&self, want: u32) -> u32 {
        let front = self.dacs.iter().chain(&self.hp_dacs).chain(&self.speaker_dacs).chain(&self.dig_dacs)
            .find(|&&d| d != 0);
        match front {
            Some(&d) => wcaps_channels(self.codec.wcaps(d as u32)).min(want),
            None => want,
        }
    }

    /// The jacks are polled (`jackpoll_ms`): mute speakers while
    /// headphones or line-outs are plugged in, line-outs while headphones
    /// are (`call_update_outputs`).
    pub fn poll_jacks(&mut self) {
        self.update_automute(false);
    }

    fn update_automute(&mut self, force: bool) {
        let cfg = self.cfg;
        let c = &self.codec;
        let hp = self.detect_hp && cfg.hp_pins[..cfg.hp_outs].iter().any(|&p| c.jack_present(p as u32));
        let lo = self.detect_lo && cfg.line_out_pins[..cfg.line_outs].iter().any(|&p| c.jack_present(p as u32));
        if !force && hp == self.hp_present && lo == self.line_present {
            return;
        }
        self.hp_present = hp;
        self.line_present = lo;
        if !self.automute_speaker && !self.automute_lo {
            return;
        }
        if self.automute_speaker {
            let mute = hp || lo;
            let shared = cfg.speaker_pins[0] == cfg.hp_pins[0];
            if !shared {
                self.do_automute(&cfg.speaker_pins[..cfg.speaker_outs], mute);
            }
        }
        if cfg.line_out_pins[0] != cfg.hp_pins[0] && cfg.line_out_pins[0] != cfg.speaker_pins[0] {
            self.do_automute(&cfg.line_out_pins[..cfg.line_outs], self.automute_lo && hp);
        }
        if hp {
            log("[audio_hda] headphones plugged in\n");
        } else if lo {
            log("[audio_hda] line-out plugged in\n");
        } else if !force {
            log("[audio_hda] headphones and line-out unplugged\n");
        }
    }

    /// `do_automute` by pin control: a muted pin's output is switched off,
    /// an unmuted one gets its target back.
    fn do_automute(&self, pins: &[u8], mute: bool) {
        for &p in pins {
            if p == 0 {
                break;
            }
            let val = if mute { 0 } else { self.pin_target[p as usize] as u32 };
            self.codec.write(p as u32, vset_pin_ctl(val));
        }
    }

    /// The parsed outputs, one line each.
    pub fn log_outputs(&self) {
        let kinds: [(&str, usize, &[u8; HDA_MAX_OUTS], &[usize; HDA_MAX_OUTS]); 4] = [
            (match self.cfg.line_out_type { OutType::Line => "line-out", OutType::Speaker => "speaker", OutType::Hp => "headphone" },
             self.cfg.line_outs, &self.cfg.line_out_pins, &self.out_paths),
            ("headphone", self.cfg.hp_outs, &self.cfg.hp_pins, &self.hp_paths),
            ("speaker", self.cfg.speaker_outs, &self.cfg.speaker_pins, &self.speaker_paths),
            ("digital", self.cfg.dig_outs, &self.cfg.dig_out_pins, &self.dig_paths),
        ];
        for (k, (name, n, pins, paths)) in kinds.into_iter().enumerate() {
            // The automute check lists the primary pins as speakers or
            // headphones too.
            if k > 0 && n > 0 && pins[0] == self.cfg.line_out_pins[0] {
                continue;
            }
            for i in 0..n {
                log("[audio_hda]  ");
                log(name);
                loghex(" pin 0x", pins[i] as u32);
                match self.path(paths[i]) {
                    Some(p) if p.depth > 0 => {
                        log(" path");
                        for &nid in &p.path[..p.depth] {
                            loghex(" 0x", nid as u32);
                        }
                    }
                    _ => log(" (no DAC)"),
                }
                log("\n");
            }
        }
    }
}
