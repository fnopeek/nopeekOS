//! Intel High Definition Audio register map + codec verbs.
//! Offsets are the HDA 1.0a spec values (identical to Linux `hda_register.h`).

// ── Global controller registers (BAR0 offsets) ──────────────────────────
pub const GCAP: u32 = 0x00; // u16  capabilities (stream counts)
pub const GCTL: u32 = 0x08; // u32  global control; bit0 = CRST
pub const WAKEEN: u32 = 0x0C; // u16
pub const STATESTS: u32 = 0x0E; // u16  bit n = codec present at SDIN addr n
pub const INTCTL: u32 = 0x20; // u32
pub const INTSTS: u32 = 0x24; // u32

pub const GCTL_CRST: u32 = 1 << 0; // 0 = reset asserted, 1 = run

// Immediate Command Interface (single-verb codec access; no CORB/RIRB DMA).
pub const IC: u32 = 0x60; // u32  immediate command out
pub const IR: u32 = 0x64; // u32  immediate response in
pub const IRS: u32 = 0x68; // u16  status
pub const IRS_ICB: u16 = 1 << 0; // immediate command busy (write 1 = send)
pub const IRS_IRV: u16 = 1 << 1; // immediate result valid (W1C)

// ── Stream descriptor registers (relative to stream base) ───────────────
// Stream base = 0x80 + sd_index * 0x20. Output streams follow input streams.
pub const SD_BASE: u32 = 0x80;
pub const SD_STRIDE: u32 = 0x20;
pub const SD_CTL: u32 = 0x00; // 3 bytes CTL + 1 byte STS at 0x03 (accessed as u32)
pub const SD_LPIB: u32 = 0x04; // u32  link position in buffer (DMA read ptr)
pub const SD_CBL: u32 = 0x08; // u32  cyclic buffer length (bytes)
pub const SD_LVI: u32 = 0x0C; // u16  last valid BDL index
pub const SD_FIFOW: u32 = 0x0E; // u16
pub const SD_FIFOS: u32 = 0x10; // u16
pub const SD_FORMAT: u32 = 0x12; // u16
pub const SD_BDLPL: u32 = 0x18; // u32  BDL base lower
pub const SD_BDLPU: u32 = 0x1C; // u32  BDL base upper

pub const SD_CTL_SRST: u32 = 1 << 0; // stream reset
pub const SD_CTL_RUN: u32 = 1 << 1; // stream run
pub const SD_CTL_STRM_SHIFT: u32 = 20; // stream tag in bits [23:20]

// Stream format: base 48k, 16-bit, 2ch = 0x0011.
//   bit14 base(0=48k), bits[6:4] bits-per-sample(001=16), bits[3:0] chan-1(0001=2)
pub const FMT_48K_S16_STEREO: u16 = 0x0011;

// ── Codec verbs ─────────────────────────────────────────────────────────
// Command dword: [31:28] codec addr, [27:20] node id, [19:0] verb.
// "Long" verbs: (12-bit id << 8) | 8-bit payload.
// "Short" verbs: (4-bit id << 16) | 16-bit payload.

// GET_PARAMETER (0xF00) parameter ids:
pub const PARAM_SUB_NODE_COUNT: u32 = 0x04; // [23:16] start, [7:0] count
pub const PARAM_FUNCTION_TYPE: u32 = 0x05; // [7:0]: 0x01 = audio function group
pub const PARAM_AUDIO_WIDGET_CAP: u32 = 0x09; // [23:20] = widget type
pub const PARAM_PIN_CAP: u32 = 0x0C; // bit4 = output capable
pub const PARAM_CONN_LIST_LEN: u32 = 0x0E; // [6:0] len, bit7 long-form
pub const PARAM_AMP_OUT_CAP: u32 = 0x12; // [14:8] num steps

// Widget types (from AUDIO_WIDGET_CAP >> 20 & 0xF):
pub const WTYPE_DAC: u32 = 0x0; // audio output
pub const WTYPE_MIXER: u32 = 0x2;
pub const WTYPE_SELECTOR: u32 = 0x3;
pub const WTYPE_PIN: u32 = 0x4;

pub const FUNC_TYPE_AFG: u32 = 0x01;

// Build a 20-bit verb field.
pub fn vget_param(param: u32) -> u32 { (0xF00 << 8) | (param & 0xFF) }
pub fn vget_config_default() -> u32 { 0xF1C << 8 }
pub fn vget_conn_entry(idx: u32) -> u32 { (0xF02 << 8) | (idx & 0xFF) }
pub fn vset_power(state: u32) -> u32 { (0x705 << 8) | (state & 0xFF) }
pub fn vset_pin_ctl(val: u32) -> u32 { (0x707 << 8) | (val & 0xFF) }
pub fn vset_eapd(val: u32) -> u32 { (0x70C << 8) | (val & 0xFF) }
pub fn vset_stream_chan(val: u32) -> u32 { (0x706 << 8) | (val & 0xFF) }
pub fn vset_conn_select(val: u32) -> u32 { (0x701 << 8) | (val & 0xFF) }
pub fn vset_format(fmt: u16) -> u32 { (0x2 << 16) | (fmt as u32) }
pub fn vset_amp(payload: u16) -> u32 { (0x3 << 16) | (payload as u32) }

// Pin widget control: bit6 = output enable.
pub const PIN_CTL_OUT_EN: u32 = 1 << 6;
// EAPD/BTL enable: bit1 = EAPD.
pub const EAPD_ENABLE: u32 = 1 << 1;
// Amp set: output amp, both channels, unmuted, gain in [6:0].
pub const AMP_SET_OUT_BOTH: u16 = 0x8000 | 0x2000 | 0x1000; // = 0xB000
