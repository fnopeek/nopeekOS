# `tools/wasm/beak-engine/src/image.rs` @ 5e0102684

## L1-8 · `use alloc::rc::Rc;`

```
//! image.rs — raster image decode for `<img>`.
//!
//! PNG (grayscale/RGB/palette/gray+alpha, bit depths 1/2/4/8/16, non-interlaced,
//! with PLTE + tRNS transparency), JPEG (baseline + progressive, via the
//! no_std zune-jpeg decoder), ICO (`ico`) and SVG (`svg`). Other formats (GIF,
//! WebP) fall back to a labelled placeholder box in layout. The
//! shell fetches the bytes (the engine is host-free); the engine decodes them
//! into `Image`s keyed by the original `src`, ready for layout + paint.
```

## L15 · `pub struct Image {`

```
/// A decoded image: premultiplied? no — straight BGRA, top-down, `w`×`h`.
```

## L22-23 · `pub type ImageMap = HashMap<String, Rc<Image>>;`

```
/// Decoded images available to layout/paint, keyed by the `<img>`'s `src`
/// attribute exactly as written in the HTML (the shell stores them so).
```

## L26-33 · `pub(crate) const MAX_PIXELS: usize = 32_000_000;`

```
/// Cap on decoded pixels per ONE image — a decompression bomb declares
/// 40000×40000 in a header of 30 bytes, and this is where that is refused
/// before anything is allocated for it.
///
/// It is NOT a memory policy: `zeroed` fails gracefully and the heap grows.
/// So it belongs far above any picture a camera or a screen produces — a phone
/// photograph is 12 MP and a 4K screenshot 8.3 MP, and the old 4 MP refused
/// both. 32 MP is 8000×4000.
```

## L36-43 · `#[derive(Debug, Clone, Copy, PartialEq, Eq)]`

```
/// Why an image did not make it into the store.
///
/// The two cases look identical in code (`decode` gave `None`, or the budget
/// said no) and are opposite at the device: one is a picture we cannot read,
/// the other a limit WE set on a picture that is perfectly fine. Reporting
/// them as one sentence — "undecodable or over budget" — sent a whole session
/// after a JPEG decoder that was never at fault
/// ([[feedback_a_denial_and_a_timeout_are_two_failures]]).
```

## L46 · `Undecodable,`

```
/// No decoder took the bytes: unsupported format, or malformed data.
```

## L48 · `OverBudget { need: usize, left: usize },`

```
/// Decoded fine; the page's remaining pixel budget could not hold it.
```

## L64-66 · `pub(crate) fn zeroed(n: usize) -> Option<Vec<u8>> {`

```
/// Allocate `n` zeroed bytes WITHOUT aborting on OOM — `try_reserve` returns
/// `Err` instead of calling `handle_alloc_error`, so an oversize image degrades
/// to a placeholder (decode → `None`) rather than killing the whole app/tab.
```

## L74-77 · `pub fn decode_data_uri(uri: &str) -> Option<Vec<u8>> {`

```
/// The payload bytes of a `data:` URI (RFC 2397), base64 or percent-encoded.
///
/// CSS icon systems inline their SVGs this way, so these need no fetch at all —
/// the bytes are already in the stylesheet.
```

## L85-86 · `let b = payload.as_bytes();`

```
// Percent-decoding. `+` is NOT a space here (that is form encoding, not
// RFC 2397) — treating it as one corrupts SVG path data.
```

## L131 · `if c.is_ascii_whitespace() {`

```
// Whitespace inside base64 is legal and common (wrapped lines).
```

## L147-148 · `pub fn decode(bytes: &[u8]) -> Option<Image> {`

```
/// Decode image bytes. Returns `None` for unsupported formats / malformed data
/// / oversize images (→ placeholder).
```

## L165 · `fn decode_jpeg(bytes: &[u8]) -> Option<Image> {`

```
// ── JPEG (baseline + progressive, via zune-jpeg) ────────────────────────────
```

## L173-174 · `let opts = DecoderOptions::default()`

```
// Force interleaved RGB output (YCbCr and grayscale both convert to RGB),
// and cap decoded dimensions so a huge asset can't exhaust the heap.
```

## L179-180 · `let mut dec = JpegDecoder::new_with_options(ZCursor::new(bytes), opts);`

```
// zune 0.5 reads through `ZByteReaderTrait`, which a bare slice no longer
// implements — `ZCursor` is the zero-copy wrapper for an in-memory buffer.
```

## L183 · `let (w, h) = dec.dimensions()?; // (width, height) in px`

```
// (width, height) in px
```

## L189-195 · `let channels = out.len().checked_div(count)?;`

```
// How many channels came back, measured rather than assumed. We ask for
// RGB, and a YCbCr source obliges — but a SINGLE-COMPONENT (grayscale)
// JPEG returns one byte per pixel whatever was requested, and
// `get_output_colorspace` just echoes the request rather than reporting
// that. The buffer length is the only truthful signal. Wikipedia serves
// its scanned aerial photographs exactly this way; assuming 3 channels
// threw the whole image away and left a blank figure on the page.
```

## L216 · `fn decode_png(data: &[u8]) -> Option<Image> {`

```
// ── PNG (grayscale/RGB/palette/gray+alpha, bit depths 1/2/4/8/16, non-interlaced) ──
```

## L223 · `let mut palette: Vec<u8> = Vec::new(); // PLTE — RGB triples`

```
// PLTE — RGB triples
```

## L224 · `let mut trns: Vec<u8> = Vec::new(); // tRNS — type3: per-index α; type0/2: colour key`

```
// tRNS — type3: per-index α; type0/2: colour key
```

## L247 · `return None; // compression / filter / interlace (Adam7 unsupported)`

```
// compression / filter / interlace (Adam7 unsupported)
```

## L249 · `let ok = match color_type {`

```
// Valid (colour-type, bit-depth) combinations per the PNG spec.
```

## L251 · `0 => matches!(bit_depth, 1 | 2 | 4 | 8 | 16), // grayscale`

```
// grayscale
```

## L252 · `3 => matches!(bit_depth, 1 | 2 | 4 | 8),      // palette`

```
// palette
```

## L253 · `2 | 4 | 6 => matches!(bit_depth, 8 | 16),     // RGB / gray+α / RGBA`

```
// RGB / gray+α / RGBA
```

## L259-261 · `if (width as usize).saturating_mul(height as usize) > MAX_PIXELS {`

```
// Early dimension guard: reject an oversize image at IHDR (the
// FIRST chunk) so we never accumulate its IDAT / allocate raw +
// bgra — the OOM spike is bounded before it starts.
```

## L278 · `pos = end + 4; // + CRC`

```
// + CRC
```

## L296 · `let stride = (width as usize * bits_per_pixel + 7) / 8; // bytes per row (may be sub-byte packed)`

```
// bytes per row (may be sub-byte packed)
```

## L297 · `let bpp = ((bits_per_pixel + 7) / 8).max(1); // filter byte-distance (spec: rounded up, ≥1)`

```
// filter byte-distance (spec: rounded up, ≥1)
```

## L306 · `let mut raw = zeroed(height as usize * stride)?;`

```
// Reverse the per-row PNG filters into raw (still-packed) sample bytes.
```

## L328 · `let count = (width * height) as usize;`

```
// Expand packed samples → straight BGRA, applying palette/grayscale + tRNS.
```

## L382-383 · `fn sample(raw: &[u8], row_start: usize, x: usize, c: usize, channels: usize, bit_depth: u8) -> u16 {`

```
/// Read channel `c` of pixel `x` from a filtered row as a `bit_depth`-wide
/// sample. Handles the packed sub-byte depths (1/2/4, MSB-first) and 8/16-bit.
```

## L402 · `fn scale_to_8(v: u16, bit_depth: u8) -> u8 {`

```
/// Scale a `bit_depth`-wide sample up to 8 bits (16-bit → high byte).
```

## L420 · `fn gray_key_alpha(trns: &[u8], v: u16) -> u8 {`

```
/// tRNS for grayscale (type 0): a single transparent sample value (16-bit BE).
```

## L431 · `fn rgb_key_alpha(trns: &[u8], r: u16, g: u16, b: u16) -> u8 {`

```
/// tRNS for truecolour (type 2): a single transparent R,G,B triple (16-bit BE each).
```

## L456-480 · `pub(crate) const TOTAL_BUDGET: usize = 1024 * 1024 * 1024;`

```
// ── Pixel budgets ───────────────────────────────────────────────────────────
//
// These used to be four slices of a `static mut HEAP: [u8; 128 MB]` — shares of
// a pot nobody measured. The heap grows now (see `GrowingHeap` in the shell),
// so they are not shares any more and they are not what stops beak from
// running out of memory. What they still do is name the point where we stop
// believing a page is legitimate: a document may not make us decode an
// unbounded number of pixels just by asking.
//
// **They were set from a guess, and the guess was under a normal page.**
// arcade.ch — a Swiss IT company's front page, 5.3 MB of pictures on the wire —
// decodes to 131 MB of BGRA across 50 images. The budget ran out after #49 and
// dropped the last two logos. Nothing on that page is unreasonable: eleven
// 1152×1152 PNGs and three 1920×1080 headers, each painted into a box a few
// hundred pixels wide. That is what today's CMS ships.
//
// So the numbers are now what they claim to be — an anti-abuse ceiling, an
// order of magnitude above the worst REAL page measured, not a memory policy.
// The memory policy is one layer down and can be measured there: `zeroed`
// uses `try_reserve` and degrades to a placeholder, and the heap grows.
// **Every clip still says so in the log, and now it says WHICH clip** (see
// `Reject`) — the next real page that hits one will tell us.
//
// They stay four rather than one because a page full of icons must not be able
// to starve its `<img>`s, or the reverse.
```

## L482-485 · `pub(crate) const TOTAL_BUDGET: usize = 1024 * 1024 * 1024;`

```
/// Decoded BGRA one page's `<img>`s may hold.
///
/// 131 MB measured on arcade.ch, so 128 MB was BELOW the normal case. A page
/// that wants more than a gigabyte of pixels is no longer a page.
```

## L488-493 · `pub(crate) const CSS_CACHE_BUDGET: usize = 64 * 1024 * 1024;`

```
/// Decoded BGRA the cross-navigation cache may hold for
/// `background-image`/`mask-image` layers.
///
/// Half of what the `<img>` cache gets, because backgrounds are sprites and
/// icons rather than photographs — and counted SEPARATELY from it, because two
/// stores sharing one constant would hold twice the memory the number says.
```

## L496-502 · `pub(crate) const IMG_CACHE_BUDGET: usize = 256 * 1024 * 1024;`

```
/// Decoded BGRA the cross-navigation `<img>` cache may hold.
///
/// The pictures a live page is still using cost nothing here (`Rc`), so this
/// bounds only what NO page holds any more — the price of going back being
/// free. Measured on the device: four navigations across two Wikipedia pages
/// filled 3 MB of it — but ONE arcade.ch is 131 MB, and a cache that cannot
/// hold a single page it just left buys nothing on the way back.
```

## L505-507 · `pub(crate) const CSS_BUDGET: usize = 256 * 1024 * 1024;`

```
/// Decoded BGRA one page's `background-image`/`mask-image` layers may hold.
/// Smaller than the `<img>` one on purpose: these are icons and tiles, and a
/// page's whole icon set is a few hundred KB.
```

## L510-511 · `pub fn decode_all(pairs: &[(String, Vec<u8>)]) -> ImageMap {`

```
/// Decode a batch of (src, bytes) into an `ImageMap` (failures / over-budget →
/// skipped, they render as placeholders).
```

## L535 · `out.extend_from_slice(&[0, 0, 0, 0]); // CRC — our decoder skips it`

```
// CRC — our decoder skips it
```

## L540 · `let idat = miniz_oxide::deflate::compress_to_vec_zlib(&[0u8, 255, 0, 0, 0, 255, 0], 6);`

```
// one scanline: filter byte 0, then two RGB pixels red, green
```

## L546 · `ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit, RGB, no compression/filter/interlace`

```
// 8-bit, RGB, no compression/filter/interlace
```

## L553 · `assert_eq!(&img.bgra[0..4], &[0, 0, 255, 255]); // red → BGRA`

```
// red → BGRA
```

## L554 · `assert_eq!(&img.bgra[4..8], &[0, 255, 0, 255]); // green → BGRA`

```
// green → BGRA
```

## L559 · `let idat = miniz_oxide::deflate::compress_to_vec_zlib(&[0u8, 0x01], 6);`

```
// 2 packed 4-bit indices (0,1) → one byte 0x01, preceded by filter 0.
```

## L565 · `ihdr.extend_from_slice(&[4, 3, 0, 0, 0]); // 4-bit, palette`

```
// 4-bit, palette
```

## L567 · `chunk(&mut png, b"PLTE", &[255, 0, 0, 0, 255, 0, 0, 0, 255]); // red, green, blue`

```
// red, green, blue
```

## L568 · `chunk(&mut png, b"tRNS", &[128]); // index 0 → α=128; others default 255`

```
// index 0 → α=128; others default 255
```

## L574 · `assert_eq!(&img.bgra[0..4], &[0, 0, 255, 128]); // index0 red, α from tRNS`

```
// index0 red, α from tRNS
```

## L575 · `assert_eq!(&img.bgra[4..8], &[0, 255, 0, 255]); // index1 green, opaque`

```
// index1 green, opaque
```

## L580 · `let idat = miniz_oxide::deflate::compress_to_vec_zlib(&[0u8, 0x1B], 6);`

```
// 4 packed 2-bit samples 0,1,2,3 → 0b00_01_10_11 = 0x1B, filter 0.
```

## L586 · `ihdr.extend_from_slice(&[2, 0, 0, 0, 0]); // 2-bit, grayscale`

```
// 2-bit, grayscale
```

## L593 · `assert_eq!(&img.bgra[0..4], &[0, 0, 0, 255]);`

```
// samples 0/1/2/3 scale ×85 → 0/85/170/255, painted as opaque gray.
```

