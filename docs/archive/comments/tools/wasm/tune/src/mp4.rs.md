# `tools/wasm/tune/src/mp4.rs` @ 5e0102684

## L1-14 · `use alloc::vec::Vec;`

```
//! MP4 / ISO base media file format — the container, not a codec.
//!
//! Written rather than pulled in: this is the first code to touch bytes a
//! stranger wrote, so it is the piece we most want to own. Every read goes
//! through `Cur`, which returns `None` past the end instead of panicking —
//! a panic here is a kernel halt.
//!
//! What it does: walk the box tree, find each track's sample table, and turn
//! it into a flat `Vec<Sample>` of (offset, size, dts, pts, sync). That is
//! everything a player needs; the rest of the format is metadata we skip.
//!
//! What it does NOT do, and says so instead of guessing: fragmented MP4
//! (`moof`), where the sample table lives in the fragments rather than in
//! `moov`.
```

## L18-19 · `struct Cur<'a> {`

```
/// A bounds-checked cursor. Every getter answers `None` past the end, so a
/// truncated or hostile file ends as a refusal and never as a panic.
```

## L58-60 · `fn next_box(&mut self) -> Option<([u8; 4], &'a [u8])> {`

```
/// Header of the next box: its four-character type and its payload.
/// `size == 1` means the real size follows as 64 bits; `size == 0` means
/// "to the end of the enclosing box", which is legal for the last one.
```

## L75 · `fn full(&mut self) -> Option<(u8, u32)> {`

```
/// Version + flags of a full box, as one word with the version on top.
```

## L84 · `#[derive(Clone, Copy)]`

```
/// One decodable unit, already located in the file.
```

## L89 · `pub dts: u64,`

```
/// Decode and presentation time, in the track's timescale.
```

## L92 · `pub sync: bool,`

```
/// A sample the decoder may start at. Seeking lands on one of these.
```

## L100-103 · `#[allow(dead_code)]`

```
// `Aac`, the four-character code in `Other`, and the audio track's rate and
// channel count are parsed and not yet read. That is deliberate: they are
// what the AUDIO half will need, and a container that parses only what today
// happens to consume is a container that gets re-read later.
```

## L106-108 · `Avc { sps: Vec<Vec<u8>>, pps: Vec<Vec<u8>>, nal_len: usize },`

```
/// H.264. `sps`/`pps` come from `avcC` and have to be prepended to the
/// stream, because in MP4 they live in the header and not in the data.
/// `nal_len` is how many bytes each NAL unit's length prefix uses.
```

## L110 · `Aac { config: Vec<u8> },`

```
/// AAC, with its AudioSpecificConfig from `esds`.
```

## L112-113 · `Other([u8; 4]),`

```
/// Present, understood well enough to skip. The four-character code is
/// kept so a log line can name what we declined.
```

## L117-121 · `#[derive(Clone, Copy, PartialEq, Eq, Debug)]`

```
/// Which matrix and range the planes are coded with.
///
/// Three sources say this, and we read the second: the SPS's VUI (the
/// decoder throws it away), the container's `colr` box, and — when neither
/// is there — the same rule every player falls back on.
```

## L132 · `pub timescale: u32,`

```
/// Ticks per second for this track's `dts`/`pts`.
```

## L138-141 · `pub rotation: u16,`

```
/// Clockwise display rotation in degrees (0, 90, 180, 270) from the
/// `tkhd` matrix. A phone films landscape and writes the matrix; the
/// coded picture is NOT the picture the viewer expects. Dropping this
/// is how a player shows every holiday video on its side.
```

## L145-151 · `pub edit_start: u64,`

```
/// First media time the edit list asks for, in this track's timescale.
///
/// Not cosmetic: the two tracks of one file carry DIFFERENT values. A
/// phone recording here has 0 on the video and 2112 on the audio — the
/// AAC encoder's priming samples, 48 ms. Playing the media timeline
/// straight puts sound and picture that far apart, and it is the single
/// easiest way to ship a player that is subtly out of sync.
```

## L161-165 · `pub fn pts_ms(&self, s: &Sample) -> i64 {`

```
/// Presentation time of a sample on the shared clock, in ms.
///
/// Signed on purpose: samples before `edit_start` are decoded but not
/// shown, and a player has to be able to tell that from "show now".
/// Every consumer asks through here so the edit list is applied once.
```

## L172-173 · `pub fn sync_at_ms(&self, ms: u64) -> usize {`

```
/// Index of the last sync sample at or before `ms`. A decoder started
/// anywhere else produces garbage until the next one.
```

## L188-190 · `pub fragmented: bool,`

```
/// The file said it is fragmented. The sample tables in `moov` are then
/// empty by design and the real ones live in each `moof`, which we do
/// not read — so this is reported rather than played as an empty file.
```

## L195-197 · `let mut c = Cur::new(d);`

```
// `ftyp` at offset 4 is the normal case. Some muxers put `styp`, `moov`
// or `free` first, so accept any known top-level box rather than only
// the tidy one.
```

## L222-223 · `out.fragmented = true;`

```
// Movie-extends: the header promising fragments. Present even
// when the first `moof` sits past whatever we have read.
```

## L250 · `t.skip(if ver == 1 { 32 } else { 20 })?;`

```
// creation, modification, track_id, reserved, duration
```

## L252-255 · `t.skip(16)?;`

```
// reserved(8) layer(2) altgroup(2) volume(2) reserved(2)
// matrix(36), then width/height as 16.16 fixed point.
// reserved(8) layer(2) altgroup(2) volume(2) reserved(2),
// then the 3x3 display matrix, then width/height as 16.16.
```

## L291 · `t.skip(4)?; // pre_defined`

```
// pre_defined
```

## L306-308 · `let (w, h) = match tables.visual {`

```
// `stsd` carries the real picture size for video; `tkhd`'s is the
// DISPLAY size and may differ (anamorphic, or a track matrix). The
// decoder outputs coded pixels, so the coded size wins where we have it.
```

## L320-323 · `.unwrap_or(Colour { bt709: h > 576, full_range: false });`

```
// Drei Quellen, in dieser Reihenfolge: die `colr`-Box des Containers,
// das VUI des SPS, und erst dann die Konvention nach Bildhoehe (SD
// wurde unter Rec. 601 gedreht, HD unter Rec. 709). Die Hoehenregel
// ist ein Rateschritt und steht deshalb zuletzt.
```

## L332-336 · `colour,`

```
// Untagged is the normal case for anything not made for broadcast,
// and then the picture's HEIGHT is the convention: SD was shot under
// Rec. 601, HD under Rec. 709. Guessing 709 for a 544-line clip
// tilts every strong red — which looks like a decoder bug and is a
// missing tag.
```

## L346-351 · `fn matrix_rotation(c: &mut Cur) -> Option<u16> {`

```
/// Read the 3x3 display matrix and answer the rotation it expresses.
///
/// Only the four right-angle cases are recognised, because they are the only
/// ones a camera writes and the only ones a nearest-neighbour blit can carry
/// out exactly. A matrix that means anything else (a shear, a flip, a free
/// angle) answers 0 rather than a wrong quarter turn.
```

## L355 · `c.skip(4)?; // u`

```
// u
```

## L358 · `c.skip(4 + 4 + 4 + 4)?; // v, x, y, w`

```
// v, x, y, w
```

## L368-374 · `fn parse_elst(edts: &[u8]) -> Option<u64> {`

```
/// The media time an `edts`/`elst` asks playback to start at.
///
/// Only the shape that every muxer writes is honoured: ONE entry, rate 1.0,
/// a non-negative media time. Multiple segments, an empty edit (`-1`) or a
/// changed rate describe an edit we do not perform, and shifting the clock
/// by a number taken out of such a list would be worse than leaving it —
/// so those answer 0 and the media timeline stands.
```

## L405 · `stts: Vec<(u32, u32)>,`

```
/// (count, delta) pairs from `stts`.
```

## L407 · `ctts: Vec<(u32, i32)>,`

```
/// (count, offset) pairs from `ctts`; offsets are signed in version 1.
```

## L409 · `stss: Vec<u32>,`

```
/// 1-based sample numbers that are sync points. Empty = every sample is.
```

## L411 · `stsc: Vec<(u32, u32)>,`

```
/// (first_chunk, samples_per_chunk) from `stsc`.
```

## L414 · `uniform_size: u32,`

```
/// A single size for all samples, when `stsz` says so.
```

## L460-461 · `t.ctts.push((cnt, if ver == 0 { off as i32 } else { off as i32 }));`

```
// Version 0 says unsigned, but files in the wild carry
// negative offsets there anyway; version 1 says signed.
```

## L476 · `b.u32()?; // sample_description_index`

```
// sample_description_index
```

## L507 · `if e.skip(8).is_none() { return; }`

```
// SampleEntry: reserved(6) data_reference_index(2)
```

## L511 · `if e.skip(16).is_none() { return; }`

```
// VisualSampleEntry up to the sub-boxes.
```

## L545-548 · `fn parse_colr(d: &[u8]) -> Option<Colour> {`

```
/// The container's colour tag. `nclx` carries a range flag, its older twin
/// `nclc` does not and is always limited range. An ICC profile (`rICC`,
/// `prof`) says nothing about the matrix, so it answers None and the
/// fallback decides — better than picking one at random.
```

## L553 · `c.skip(2 + 2)?; // primaries, transfer`

```
// primaries, transfer
```

## L557-559 · `bt709: matrix == 1,`

```
// 1 = Rec. 709. 5 and 6 are the two spellings of Rec. 601; anything
// else (2 = unspecified, 9 = Rec. 2020) is not something this blit
// can carry out, so it reads as 601 for SD and 709 above.
```

## L565 · `struct Bits<'a> { d: &'a [u8], bit: usize }`

```
/// Bitwise reader for an SPS: Exp-Golomb, as H.264 writes its syntax.
```

## L581-582 · `fn ue(&mut self) -> Option<u32> {`

```
/// Unsigned Exp-Golomb. The leading-zero count is capped: a corrupt
/// stream of zero bytes would otherwise spin to the end of the buffer.
```

## L598-600 · `fn rbsp(nal: &[u8]) -> Vec<u8> {`

```
/// Strip the emulation-prevention bytes. Inside a NAL, the encoder inserts a
/// `0x03` after any `00 00` that would otherwise look like a start code; the
/// syntax underneath does not know about them.
```

## L612-620 · `fn sps_colour(sps: &[u8]) -> Option<Colour> {`

```
/// The colour tag the SPS carries in its VUI.
///
/// ffmpeg reads this, and so a file that looks untagged at the container
/// level is usually not: a phone video with no `colr` box still says Rec.709
/// here. Guessing from the picture height instead gets that one wrong, and a
/// wrong matrix tilts every strong colour — which looks like a decoder bug.
///
/// Everything before the VUI has to be walked because the syntax is not
/// byte-aligned; there is no shortcut to the field we want.
```

## L623 · `let mut b = Bits::new(d.get(1..)?);`

```
// Skip the one-byte NAL header, then profile / constraints / level.
```

## L626 · `b.un(8)?;  // constraint flags + reserved`

```
// constraint flags + reserved
```

## L627 · `b.un(8)?;  // level_idc`

```
// level_idc
```

## L628 · `b.ue()?;   // seq_parameter_set_id`

```
// seq_parameter_set_id
```

## L632 · `b.ue()?;  // bit_depth_luma_minus8`

```
// bit_depth_luma_minus8
```

## L633 · `b.ue()?;  // bit_depth_chroma_minus8`

```
// bit_depth_chroma_minus8
```

## L634 · `b.u1()?;  // qpprime_y_zero_transform_bypass_flag`

```
// qpprime_y_zero_transform_bypass_flag
```

## L636-638 · `let n = if chroma != 3 { 8 } else { 12 };`

```
// Scaling lists: each present one is a delta walk of 16 or 64
// values, and skipping them wrong puts every field after this
// one bit out of place.
```

## L655 · `b.ue()?;  // log2_max_frame_num_minus4`

```
// log2_max_frame_num_minus4
```

## L668 · `b.ue()?;  // max_num_ref_frames`

```
// max_num_ref_frames
```

## L669 · `b.u1()?;  // gaps_in_frame_num_value_allowed_flag`

```
// gaps_in_frame_num_value_allowed_flag
```

## L670 · `b.ue()?;  // pic_width_in_mbs_minus1`

```
// pic_width_in_mbs_minus1
```

## L671 · `b.ue()?;  // pic_height_in_map_units_minus1`

```
// pic_height_in_map_units_minus1
```

## L672 · `if b.u1()? == 0 { b.u1()?; }   // frame_mbs_only_flag / mb_adaptive`

```
// frame_mbs_only_flag / mb_adaptive
```

## L673 · `b.u1()?;  // direct_8x8_inference_flag`

```
// direct_8x8_inference_flag
```

## L674 · `if b.u1()? == 1 { b.ue()?; b.ue()?; b.ue()?; b.ue()?; }  // cropping`

```
// cropping
```

## L675 · `if b.u1()? == 0 { return None; }  // vui_parameters_present_flag`

```
// vui_parameters_present_flag
```

## L677 · `if b.u1()? == 1 {                 // aspect_ratio_info_present_flag`

```
// aspect_ratio_info_present_flag
```

## L678 · `if b.un(8)? == 255 { b.un(16)?; b.un(16)?; }   // Extended_SAR`

```
// Extended_SAR
```

## L680 · `if b.u1()? == 1 { b.u1()?; }      // overscan`

```
// overscan
```

## L681 · `if b.u1()? == 0 { return None; }  // video_signal_type_present_flag`

```
// video_signal_type_present_flag
```

## L682 · `b.un(3)?;                         // video_format`

```
// video_format
```

## L685 · `return Some(Colour { bt709: false, full_range });`

```
// Range said, matrix not. That is still worth having.
```

## L688 · `b.un(8)?;                         // colour_primaries`

```
// colour_primaries
```

## L689 · `b.un(8)?;                         // transfer_characteristics`

```
// transfer_characteristics
```

## L696 · `c.skip(4)?; // version, profile, compat, level`

```
// version, profile, compat, level
```

## L713-714 · `fn parse_esds(d: &[u8]) -> Option<Vec<u8>> {`

```
/// `esds` is an MPEG-4 descriptor blob; we want exactly one field out of it,
/// the DecoderSpecificInfo (tag 0x05) that AAC needs to configure itself.
```

## L718-719 · `fn len_of(c: &mut Cur) -> Option<usize> {`

```
// Descriptors nest: ES(0x03) -> DecoderConfig(0x04) -> DecSpecific(0x05).
// Each is tag, then a length in 1..4 bytes with a continuation bit.
```

## L744-747 · `fn build_samples(t: &Tables, file: &[u8]) -> Vec<Sample> {`

```
/// Turn the five tables into one flat list. This is the whole point of the
/// container: `stsc` says how many samples sit in each chunk, `stco` says
/// where each chunk starts, `stsz` how long each sample is, and the offsets
/// simply accumulate inside a chunk.
```

## L760-761 · `let last = t.stsc.get(run + 1).map(|&(f, _)| f).unwrap_or(t.chunks.len() as u32 + 1);`

```
// A run of chunks holding `per` samples each, until the next entry's
// `first_chunk` (or the last chunk).
```

## L770-771 · `if off.checked_add(sz).map_or(true, |e| e > file.len()) {`

```
// A sample that does not lie inside the file is where a
// damaged or hostile table shows itself. Stop; do not clamp.
```

## L782-783 · `let mut dts = 0u64;`

```
// Times: `stts` is a run-length list of deltas, `ctts` shifts
// presentation against decode (that is what B-frames need).
```

## L804-805 · `if !t.stss.is_empty() {`

```
// `stss` present means only those samples are sync points. Absent means
// every one is — which is true for audio and for all-intra video.
```

## L817-821 · `pub fn to_annex_b(sample: &[u8], nal_len: usize, out: &mut Vec<u8>) {`

```
/// Rewrite one MP4 sample into Annex-B, which is what a decoder reads.
///
/// In MP4 each NAL unit carries a length prefix; Annex-B separates them with
/// a start code instead. Same payload, different framing — so this is a
/// reframing, not a conversion.
```

## L830-831 · `_ => sample.len(),`

```
// A length that runs past the sample is corruption; take what is
// there and stop rather than reading a neighbour's bytes.
```

## L840-841 · `pub fn parameter_sets(codec: &Codec, out: &mut Vec<u8>) {`

```
/// SPS and PPS as an Annex-B preamble. In MP4 they sit in `avcC`, so a
/// decoder fed only the samples never sees them and cannot start.
```

