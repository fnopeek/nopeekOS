# `tools/wasm/audio_hda/src/regs.rs` @ 5e0102684

## L1-2 · `pub const GCAP: u32 = 0x00; // u16  capabilities (stream counts)`

```
//! Intel High Definition Audio register map + codec verbs.
//! Offsets are the HDA 1.0a spec values (identical to Linux `hda_register.h`).
```

## L4 · `pub const GCAP: u32 = 0x00; // u16  capabilities (stream counts)`

```
// ── Global controller registers (BAR0 offsets) ──────────────────────────
```

## L5 · `pub const GCAP: u32 = 0x00; // u16  capabilities (stream counts)`

```
// u16  capabilities (stream counts)
```

## L6 · `pub const GCTL: u32 = 0x08; // u32  global control; bit0 = CRST`

```
// u32  global control; bit0 = CRST
```

## L7 · `pub const WAKEEN: u32 = 0x0C; // u16`

```
// u16
```

## L8 · `pub const STATESTS: u32 = 0x0E; // u16  bit n = codec present at SDIN addr n`

```
// u16  bit n = codec present at SDIN addr n
```

## L9 · `pub const INTCTL: u32 = 0x20; // u32`

```
// u32
```

## L10 · `pub const INTSTS: u32 = 0x24; // u32`

```
// u32
```

## L12 · `pub const GCTL_CRST: u32 = 1 << 0; // 0 = reset asserted, 1 = run`

```
// 0 = reset asserted, 1 = run
```

## L14 · `pub const IC: u32 = 0x60; // u32  immediate command out`

```
// Immediate Command Interface (single-verb codec access; no CORB/RIRB DMA).
```

## L15 · `pub const IC: u32 = 0x60; // u32  immediate command out`

```
// u32  immediate command out
```

## L16 · `pub const IR: u32 = 0x64; // u32  immediate response in`

```
// u32  immediate response in
```

## L17 · `pub const IRS: u32 = 0x68; // u16  status`

```
// u16  status
```

## L18 · `pub const IRS_ICB: u16 = 1 << 0; // immediate command busy (write 1 = send)`

```
// immediate command busy (write 1 = send)
```

## L19 · `pub const IRS_IRV: u16 = 1 << 1; // immediate result valid (W1C)`

```
// immediate result valid (W1C)
```

## L21-22 · `pub const SD_BASE: u32 = 0x80;`

```
// ── Stream descriptor registers (relative to stream base) ───────────────
// Stream base = 0x80 + sd_index * 0x20. Output streams follow input streams.
```

## L25 · `pub const SD_CTL: u32 = 0x00; // 3 bytes CTL + 1 byte STS at 0x03 (accessed as u32)`

```
// 3 bytes CTL + 1 byte STS at 0x03 (accessed as u32)
```

## L26 · `pub const SD_STS: u32 = 0x03; // u8, write-1-to-clear status (hda_register.h)`

```
// u8, write-1-to-clear status (hda_register.h)
```

## L27 · `pub const SD_LPIB: u32 = 0x04; // u32  link position in buffer (DMA read ptr)`

```
// u32  link position in buffer (DMA read ptr)
```

## L28 · `pub const SD_CBL: u32 = 0x08; // u32  cyclic buffer length (bytes)`

```
// u32  cyclic buffer length (bytes)
```

## L29 · `pub const SD_LVI: u32 = 0x0C; // u16  last valid BDL index`

```
// u16  last valid BDL index
```

## L30 · `pub const SD_FIFOW: u32 = 0x0E; // u16`

```
// u16
```

## L31 · `pub const SD_FIFOS: u32 = 0x10; // u16`

```
// u16
```

## L32 · `pub const SD_FORMAT: u32 = 0x12; // u16`

```
// u16
```

## L33 · `pub const SD_BDLPL: u32 = 0x18; // u32  BDL base lower`

```
// u32  BDL base lower
```

## L34 · `pub const SD_BDLPU: u32 = 0x1C; // u32  BDL base upper`

```
// u32  BDL base upper
```

## L36 · `pub const SD_CTL_SRST: u32 = 1 << 0; // stream reset`

```
// stream reset
```

## L37 · `pub const SD_CTL_RUN: u32 = 1 << 1; // stream run`

```
// stream run
```

## L38 · `pub const SD_CTL_STRM_SHIFT: u32 = 20; // stream tag in bits [23:20]`

```
// stream tag in bits [23:20]
```

## L39-40 · `pub const SD_INT_MASK: u32 = 0x1c; // DESC_ERR 0x10 | FIFO_ERR 0x08 | COMPLETE 0x04`

```
// hda_register.h: interrupt enables in SD_CTL byte 0 (`SD_INT_MASK`) and the
// matching write-1-to-clear status bits in SD_STS (byte 3 of the same dword).
```

## L41 · `pub const SD_INT_MASK: u32 = 0x1c; // DESC_ERR 0x10 | FIFO_ERR 0x08 | COMPLETE 0x04`

```
// DESC_ERR 0x10 | FIFO_ERR 0x08 | COMPLETE 0x04
```

## L42-44 · `pub const AZX_INT_GLOBAL_EN: u32 = 1 << 31;`

```
// INTCTL: global enable (bit 31), controller enable (bit 30), then one bit
// per stream by its index — input streams first, so the first output stream
// is index `iss`.
```

## L47-48 · `pub const FMT_48K_S16_STEREO: u16 = 0x0011;`

```
// Stream format: base 48k, 16-bit, 2ch = 0x0011.
//   bit14 base(0=48k), bits[6:4] bits-per-sample(001=16), bits[3:0] chan-1(0001=2)
```

## L51-54 · `pub const PARAM_SUB_NODE_COUNT: u32 = 0x04; // [23:16] start, [7:0] count`

```
// ── Codec verbs ─────────────────────────────────────────────────────────
// Command dword: [31:28] codec addr, [27:20] node id, [19:0] verb.
// "Long" verbs: (12-bit id << 8) | 8-bit payload.
// "Short" verbs: (4-bit id << 16) | 16-bit payload.
```

## L56 · `pub const PARAM_SUB_NODE_COUNT: u32 = 0x04; // [23:16] start, [7:0] count`

```
// GET_PARAMETER (0xF00) parameter ids:
```

## L57 · `pub const PARAM_SUB_NODE_COUNT: u32 = 0x04; // [23:16] start, [7:0] count`

```
// [23:16] start, [7:0] count
```

## L58 · `pub const PARAM_FUNCTION_TYPE: u32 = 0x05; // [7:0]: 0x01 = audio function group`

```
// [7:0]: 0x01 = audio function group
```

## L59 · `pub const PARAM_AUDIO_WIDGET_CAP: u32 = 0x09; // [23:20] = widget type`

```
// [23:20] = widget type
```

## L60 · `pub const PARAM_PIN_CAP: u32 = 0x0C; // bit4 = output capable`

```
// bit4 = output capable
```

## L61 · `pub const PARAM_CONN_LIST_LEN: u32 = 0x0E; // [6:0] len, bit7 long-form`

```
// [6:0] len, bit7 long-form
```

## L62 · `pub const PARAM_AMP_OUT_CAP: u32 = 0x12; // [14:8] num steps`

```
// [14:8] num steps
```

## L63 · `pub const PARAM_AMP_IN_CAP: u32 = 0x0D;  // [14:8] num steps`

```
// [14:8] num steps
```

## L64-65 · `pub const AMP_CAP_MUTE: u32 = 1 << 31;`

```
/// Bit31 von AMP_*_CAP: „kann stummschalten". Null Stufen UND dieses Bit
/// heisst reiner Stummschalter — ein Verstaerker, der nur auf/zu kann.
```

## L67-69 · `pub const WCAP_OUT_AMP: u32 = 1 << 2;`

```
/// Widget-Faehigkeit „hat einen Ausgangsverstaerker" (AC_WCAP_OUT_AMP).
/// Ohne dieses Bit ist ein Amp-Verb an das Widget undefiniert — siehe
/// `unmute_out`, wo genau das einmal den Ton gekostet hat.
```

## L71 · `pub const WCAP_IN_AMP: u32 = 1 << 1;`

```
/// Widget-Faehigkeit „hat einen EINGANGSverstaerker" (AC_WCAP_IN_AMP).
```

## L74 · `pub const WTYPE_DAC: u32 = 0x0; // audio output`

```
// Widget types (from AUDIO_WIDGET_CAP >> 20 & 0xF):
```

## L75 · `pub const WTYPE_DAC: u32 = 0x0; // audio output`

```
// audio output
```

## L82 · `pub fn vget_param(param: u32) -> u32 { (0xF00 << 8) | (param & 0xFF) }`

```
// Build a 20-bit verb field.
```

## L94 · `pub const PIN_CTL_OUT_EN: u32 = 1 << 6;`

```
// Pin widget control: bit6 = output enable.
```

## L96 · `pub const EAPD_ENABLE: u32 = 1 << 1;`

```
// EAPD/BTL enable: bit1 = EAPD.
```

## L98 · `pub const AMP_SET_OUT_BOTH: u16 = 0x8000 | 0x2000 | 0x1000; // = 0xB000`

```
// Amp set: output amp, both channels, unmuted, gain in [6:0].
```

## L99 · `pub const AMP_SET_OUT_BOTH: u16 = 0x8000 | 0x2000 | 0x1000; // = 0xB000`

```
// = 0xB000
```

## L100 · `pub const AMP_SET_IN_BOTH: u16 = 0x4000 | 0x2000 | 0x1000; // = 0x7000`

```
/// Bit14 statt Bit15 = EINGANGSverstaerker; der Index steht in [11:8].
```

## L101 · `pub const AMP_SET_IN_BOTH: u16 = 0x4000 | 0x2000 | 0x1000; // = 0x7000`

```
// = 0x7000
```

