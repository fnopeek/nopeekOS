# `tools/wasm/tune/src/mp3.rs` @ 5e0102684

## L1-5 · `use alloc::string::{String, ToString};`

```
//! MP3 — MPEG-1/2/2.5 Layer III, decoded by `nanomp3` (minimp3).
//!
//! This file is everything around the decoder that the decoder refuses to
//! care about: ID3 tags at both ends, the Xing/Info header that turns a VBR
//! file into a duration and a seek table, and the byte-level seek itself.
```

## L13-14 · `data:  &'static [u8],`

```
/// Audio bytes only — ID3v2 at the front and ID3v1 at the back removed,
/// so byte fractions map to time fractions.
```

## L20-24 · `index:  alloc::vec::Vec<u32>,`

```
/// Byte offset of every `stride`-th frame, built by walking the frame
/// headers once at open. Costs one pass over ~9000 headers and gives
/// what no tag does: an exact duration and a seek that lands on the
/// frame asked for. The Xing table it replaces is quantised to 1/256
/// of the file — measured 832 ms off on a four-minute VBR track.
```

## L27 · `spf:    u32,`

```
/// Samples per frame, constant within a stream.
```

## L33 · `b.len() >= 4 && b[0] == 0xFF && (b[1] & 0xE6) == 0xE2`

```
// A bare frame sync: 11 set bits, layer III, a defined bitrate.
```

## L37-39 · `struct Header {`

```
/// Fields of an MPEG audio frame header, or `None` if these four bytes are
/// not one. Reserved values (bitrate 0/15, rate index 3) count as not-one:
/// treating them as a frame is how a resync lands in the middle of audio.
```

## L44 · `spf:           u32,`

```
/// 1152 for MPEG-1, 576 for MPEG-2/2.5.
```

## L46 · `side_info:     usize,`

```
/// Bytes between the header and a Xing tag, if there is one.
```

## L55 · `[44100, 48000, 32000], // MPEG-1`

```
// MPEG-1
```

## L56 · `[22050, 24000, 16000], // MPEG-2`

```
// MPEG-2
```

## L57 · `[11025, 12000, 8000],  // MPEG-2.5`

```
// MPEG-2.5
```

## L62 · `let version = (b[1] >> 3) & 3;      // 0=2.5, 1=reserved, 2=MPEG-2, 3=MPEG-1`

```
// 0=2.5, 1=reserved, 2=MPEG-2, 3=MPEG-1
```

## L63 · `let layer = (b[1] >> 1) & 3;        // 1 = Layer III`

```
// 1 = Layer III
```

## L85 · `fn find_header(data: &[u8], from: usize, limit: usize) -> Option<usize> {`

```
/// First frame header at or after `from`, scanning at most `limit` bytes.
```

## L100-102 · `if end >= start + 128 && &bytes[end - 128..end - 125] == b"TAG" {`

```
// ID3v1 is 128 trailing bytes that are not audio; leaving them in
// makes every byte-fraction seek land slightly late and adds a
// burst of noise at the end of the file.
```

## L116-121 · `let mut audio_start = first;`

```
// Xing/Info sits inside the first frame, right after the side info.
// It is the only place a VBR file states its length.
//
// The two names are not synonyms: "Xing" means the stream is VBR,
// "Info" means it is CBR. That distinction decides how to seek —
// see below.
```

## L127-130 · `audio_start = first + frame_len(&h, &data[first..]);`

```
// Neither the frame count nor the seek table is read: the
// scan below knows both exactly. What the tag is needed for
// is that its own frame is silence — leaving it in prepends
// a frame of nothing and shifts every position by one frame.
```

## L134-137 · `let data = &data[audio_start.min(data.len())..];`

```
// From here on `data` is audio and nothing else, so a byte fraction
// IS a time fraction. Anything left in front of it — tags, the Xing
// frame, junk before the first sync — would otherwise shift every
// seek by its own size.
```

## L141-143 · `let bitrate_kbps = match total_frames {`

```
// Average, not the first frame's nominal rate: on a VBR file the
// first frame is often the quietest and reads as "64 kbps" for a
// track that averages 130.
```

## L170 · `fn frame_len(h: &Header, b: &[u8]) -> usize {`

```
/// Encoded length of the frame whose header starts at `b`.
```

## L173 · `let spf_bytes = h.spf as usize / 8; // 144 for MPEG-1, 72 for MPEG-2`

```
// 144 for MPEG-1, 72 for MPEG-2
```

## L177-180 · `fn scan(data: &[u8]) -> (u32, alloc::vec::Vec<u32>, u32) {`

```
/// Walk every frame header once. Returns (frame count, sparse byte index,
/// frames per index entry). Roughly nine thousand iterations of pointer
/// arithmetic for a four-minute track — three orders of magnitude below
/// what decoding one second costs.
```

## L190-191 · `None => match find_header(data, at + 1, 64 * 1024) {`

```
// Garbage between frames happens (embedded tags, splice points).
// Resync rather than declaring the file over.
```

## L199-201 · `let mut i = 0;`

```
// Halve the resolution instead of growing without bound:
// a two-hour podcast keeps the same 16 KB of index a
// four-minute song has.
```

## L230-231 · `if let Some(i) = info {`

```
// A frame the decoder skipped as garbage returns bytes but no
// samples — keep going rather than reporting end of stream.
```

## L242 · `let want = frame.min(total) / self.spf as u64;          // frame number`

```
// frame number
```

## L247-248 · `while n < want && at < self.data.len() {`

```
// Walk the remaining `stride` headers by hand: the index is coarse
// on purpose, and this makes the landing exact.
```

## L261-263 · `self.dec = Decoder::new();`

```
// The bit reservoir means the first frame after a seek may refer to
// data we skipped. Starting from a clean decoder keeps that to one
// frame of quiet instead of a burst from the previous position.
```

## L270 · `fn read_id3v2(b: &[u8]) -> (Option<String>, Option<String>, usize) {`

```
// ── ID3 ───────────────────────────────────────────────────────────────
```

## L272 · `fn read_id3v2(b: &[u8]) -> (Option<String>, Option<String>, usize) {`

```
/// Returns (title, artist, offset of the first audio byte).
```

## L279 · `if flags & 0x10 != 0 { end += 10; } // footer`

```
// footer
```

## L282-283 · `return (None, None, end);`

```
// v2.2 uses 3-byte frame ids. Rare enough that skipping the tag and
// showing the file name beats a second parser.
```

## L290 · `if flags & 0x40 != 0 && p + 4 <= end {`

```
// An extended header, if present, is announced by its own length.
```

## L297 · `if id == [0, 0, 0, 0] { break; } // padding`

```
// padding
```

## L320-321 · `fn syncsafe(b: &[u8]) -> u32 {`

```
/// Syncsafe integer — seven bits per byte, so a tag length can never
/// contain a byte that looks like a frame sync.
```

## L327-328 · `fn decode_text(f: &[u8]) -> Option<String> {`

```
/// ID3 text frame: one encoding byte, then the text. All four encodings
/// appear in the wild; a title is worth 30 lines.
```

## L349-350 · `out.push(char::from_u32(u as u32).unwrap_or('?'));`

```
// Surrogate pairs would need the next unit; music metadata
// outside the BMP is rare enough to render as a placeholder.
```

