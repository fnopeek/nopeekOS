# `tools/wasm/beak-engine/src/webp/vp8.rs` @ 5e0102684

## L1-9 · `#![allow(dead_code)]`

```
//! VP8 lossy decoding — ported from `image-webp` 0.1.3 (image-rs, MIT OR
//! Apache-2.0). Only the `std` surface was replaced; see `super` for the four
//! places that were and why the port exists at all. Keep this file diffable
//! against upstream: fix decoding bugs by re-porting the upstream hunk, not by
//! rewriting around them.
//!
// Upstream's `fill_rgb`/`fill_rgba` are unreachable here (we paint BGRA), and
// a few upstream helpers are only used by its lossless path. They stay so the
// file keeps diffing cleanly against 0.1.3.
```

## L11-23 · `use alloc::boxed::Box;`

```
//! An implementation of the VP8 Video Codec
//!
//! This module contains a partial implementation of the
//! VP8 video format as defined in RFC-6386.
//!
//! It decodes Keyframes only.
//! VP8 is the underpinning of the WebP image format
//!
//! # Related Links
//! * [rfc-6386](http://tools.ietf.org/html/rfc6386) - The VP8 Data Format and Decoding Guide
//! * [VP8.pdf](http://static.googleusercontent.com/media/research.google.com/en//pubs/archive/37073.pdf) - An overview of
//!   of the VP8 format
//!
```

## L39 · `const DC_PRED: i8 = 0;`

```
// Prediction modes
```

## L57 · `#[repr(i8)]`

```
// Prediction mode enum
```

## L61 · `#[default]`

```
/// Predict DC using row above and column to the left.
```

## L65 · `V = V_PRED,`

```
/// Predict rows using row above.
```

## L68 · `H = H_PRED,`

```
/// Predict columns using column to the left.
```

## L71 · `TM = TM_PRED,`

```
/// Propagate second differences.
```

## L74 · `B = B_PRED,`

```
/// Each Y subblock is independently predicted.
```

## L81 · `#[default]`

```
/// Predict DC using row above and column to the left.
```

## L85 · `V = V_PRED,`

```
/// Predict rows using row above.
```

## L88 · `H = H_PRED,`

```
/// Predict columns using column to the left.
```

## L91 · `TM = TM_PRED,`

```
/// Propagate second differences.
```

## L115-116 · `static KEYFRAME_YMODE_TREE: [i8; 8] = [-B_PRED, 2, 4, 6, -DC_PRED, -V_PRED, -H_PRED, -TM_PRED];`

```
// Section 11.2
// Tree for determining the keyframe luma intra prediction modes:
```

## L119 · `static KEYFRAME_YMODE_PROBS: [Prob; 4] = [145, 156, 163, 128];`

```
// Default probabilities for decoding the keyframe luma modes
```

## L122 · `static KEYFRAME_BPRED_MODE_TREE: [i8; 18] = [`

```
// Tree for determining the keyframe B_PRED mode:
```

## L128 · `static KEYFRAME_BPRED_MODE_PROBS: [[[u8; 9]; 10]; 10] = [`

```
// Probabilities for the BPRED_MODE_TREE
```

## L252 · `static KEYFRAME_UV_MODE_TREE: [i8; 6] = [-DC_PRED, 2, -V_PRED, 4, -H_PRED, -TM_PRED];`

```
// Section 11.4 Tree for determining macroblock the chroma mode
```

## L255 · `static KEYFRAME_UV_MODE_PROBS: [Prob; 3] = [142, 114, 183];`

```
// Probabilities for determining macroblock mode
```

## L258 · `type TokenProbTables = [[[[Prob; NUM_DCT_TOKENS - 1]; 3]; 8]; 4];`

```
// Section 13.4
```

## L261 · `static COEFF_UPDATE_PROBS: TokenProbTables = [`

```
// Probabilities that a token's probability will be updated
```

## L433-434 · `static COEFF_PROBS: TokenProbTables = [`

```
// Section 13.5
// Default Probabilities for tokens
```

## L606 · `const DCT_0: i8 = 0;`

```
// DCT Tokens
```

## L725-729 · `let shift = self.range.leading_zeros() - 24;`

```
// Compute shift required to satisfy `self.range >= 128`.
// Apply that shift to `self.range`, `self.value`, and `self.bitcount`.
//
// Subtract 24 because we only care about leading zeros in the
// lowest byte of `self.range` which is a `u32`.
```

## L738-739 · `match self.reader.read_u8() {`

```
// libwebp seems to (sometimes?) allow bitstreams that read one byte past the end.
// This match statement replicates that logic.
```

## L810 · `#[derive(Default, Debug, Clone)]`

```
/// A Representation of the last decoded video frame
```

## L813 · `pub width: u16,`

```
/// The width of the luma plane
```

## L816 · `pub height: u16,`

```
/// The height of the luma plane
```

## L819 · `pub ybuf: Vec<u8>,`

```
/// The luma plane of the frame
```

## L822 · `pub ubuf: Vec<u8>,`

```
/// The blue plane of the frame
```

## L825 · `pub vbuf: Vec<u8>,`

```
/// The red plane of the frame
```

## L828 · `pub keyframe: bool,`

```
/// Indicates whether this frame is a keyframe
```

## L833 · `pub for_display: bool,`

```
/// Indicates whether this frame is intended for display
```

## L836-838 · `pub pixel_type: u8,`

```
// Section 9.2
/// The pixel type of the frame as defined by Section 9.2
/// of the VP8 Specification
```

## L841 · `filter_type: bool, //if true uses simple filter // if false uses normal filter`

```
// Section 9.4 and 15
```

## L842 · `filter_type: bool, //if true uses simple filter // if false uses normal filter`

```
//if true uses simple filter // if false uses normal filter
```

## L848 · `fn chroma_width(&self) -> u16 {`

```
/// Chroma plane is half the size of the Luma plane
```

## L857 · `pub(crate) fn fill_rgb(&self, buf: &mut [u8]) {`

```
/// Fills an rgb buffer with the image
```

## L882 · `let mut rgb_chunks = rgb.chunks_exact_mut(6);`

```
// Fill 2 pixels per iteration: these pixels share `u` and `v` components
```

## L930 · `pub(crate) fn fill_rgba(&self, buf: &mut [u8]) {`

```
/// Fills an rgba buffer by skipping the alpha values
```

## L955-960 · `pub(crate) fn fill_bgra(&self, buf: &mut [u8]) {`

```
/// Fill a BGRA buffer — beak's paint order — with an opaque alpha.
///
/// Added for this port. Upstream has `fill_rgb`/`fill_rgba`; going through
/// either and swizzling afterwards would walk every pixel a second time,
/// and under wasmi a per-pixel pass is the expensive kind
/// (`memory/feedback_wasmi_hot_loops.md`).
```

## L979-991 · `rgb[0] = clip(mulhi(y, 19077) + mulhi(v, 26149) - 14234);`

```
// // Conversion values from https://docs.microsoft.com/en-us/windows/win32/medfound/recommended-8-bit-yuv-formats-for-video-rendering#converting-8-bit-yuv-to-rgb888
// let c: i32 = i32::from(y) - 16;
// let d: i32 = i32::from(u) - 128;
// let e: i32 = i32::from(v) - 128;
// let r: u8 = clamp((298 * c + 409 * e + 128) >> 8, 0, 255)
//     .try_into()
//     .unwrap();
// let g: u8 = clamp((298 * c - 100 * d - 208 * e + 128) >> 8, 0, 255)
//     .try_into()
//     .unwrap();
// let b: u8 = clamp((298 * c + 516 * d + 128) >> 8, 0, 255)
//     .try_into()
//     .unwrap();
```

## L997 · `pub fn get_buf_size(&self) -> usize {`

```
/// Gets the buffer size
```

## L1003 · `fn mulhi(v: u8, coeff: u16) -> i32 {`

```
/// `_mm_mulhi_epu16` emulation used in `Frame::fill_rgb` and `Frame::fill_rgba`.
```

## L1008-1025 · `fn clip(v: i32) -> u8 {`

```
/// Used in `Frame::fill_rgb` and `Frame::fill_rgba`.
/// This function has been rewritten to encourage auto-vectorization.
///
/// Based on [src/dsp/yuv.h](https://github.com/webmproject/libwebp/blob/8534f53960befac04c9631e6e50d21dcb42dfeaf/src/dsp/yuv.h#L79)
/// from the libwebp source.
/// `​``text
/// const YUV_FIX2: i32 = 6;
/// const YUV_MASK2: i32 = (256 << YUV_FIX2) - 1;
/// fn clip(v: i32) -> u8 {
///     if (v & !YUV_MASK2) == 0 {
///         (v >> YUV_FIX2) as u8
///     } else if v < 0 {
///         0
///     } else {
///         255
///     }
/// }
/// `​``
```

## L1048-1050 · `pub struct Vp8Decoder<'a> {`

```
/// VP8 Decoder
///
/// Only decodes keyframes
```

## L1074 · `prob_intra: Prob,`

```
// Section 9.10
```

## L1077 · `prob_skip_false: Option<Prob>,`

```
// Section 9.11
```

## L1088-1089 · `pub fn new(r: SliceReader<'a>) -> Vp8Decoder<'a> {`

```
/// Create a new decoder.
/// The reader must present a raw vp8 bitstream to the decoder
```

## L1127 · `prob_intra: 0u8,`

```
// Section 9.10
```

## L1130 · `prob_skip_false: None,`

```
// Section 9.11
```

## L1164-1165 · `let size = u32::from_le_bytes([s[0], s[1], s[2], 0]);`

```
// Upstream reads this through `byteorder` on a `&[u8]`; the
// chunk is exactly three bytes by construction above.
```

## L1242 · `self.segment[i].y2ac = (i32::from(ac_quant(base + y2ac_delta)) * 155 / 100) as i16;`

```
// The intermediate result (max`284*155`) can be larger than the `i16` range.
```

## L1287 · `self.segments_update_map = self.b.read_flag()?;`

```
// Section 9.3
```

## L1354 · `self.left = self.top.first().cloned().unwrap_or_default();`

```
// Almost always the first macro block, except when non exists (i.e. `width == 0`)
```

## L1373 · `self.b.init(buf)?;`

```
// initialise binary decoder
```

## L1406-1407 · `return Err(DecodingError::UnsupportedFeature(`

```
// 9.7 refresh golden frame and altref frame
// FIXME: support this?
```

## L1412 · `let _ = self.b.read_literal(1);`

```
// Refresh entropy probs ?????
```

## L1426 · `self.prob_intra = 0;`

```
// 9.10 remaining frame data
```

## L1429 · `return Err(DecodingError::UnsupportedFeature(`

```
// FIXME: support this?
```

## L1434 · `}`

```
// Reset motion vectors
```

## L1469 · `let luma = self`

```
// intra prediction
```

## L1477 · `None => {`

```
// `LumaMode::B` - This is predicted individually
```

## L1537 · `let rb: &[i32; 16] = resdata[i * 16..][..16].try_into().unwrap();`

```
// Create a reference to a [i32; 16] array for add_residue (slices of size 16 do not work).
```

## L1560 · `let ylength = cmp::min(self.frame.height as usize - mby * 16, 16);`

```
// Length is the remainder to the border, but maximally the current chunk.
```

## L1579 · `let mut uws = [0u8; (8 + 1) * (8 + 1)];`

```
//8x8 with left top border of 1
```

## L1586 · `for y in 0usize..8 {`

```
//left border
```

## L1598 · `for x in 0usize..8 {`

```
//top border
```

## L1611 · `let (u1, v1) = if mby == 0 {`

```
//top left point
```

## L1839 · `fn loop_filter(&mut self, mbx: usize, mby: usize, mb: &MacroBlock) {`

```
/// Does loop filtering on the macroblock
```

## L1858 · `if mbx > 0 {`

```
//filter across left of macroblock
```

## L1860 · `if self.frame.filter_type {`

```
//simple loop filtering
```

## L1918 · `if mb.luma_mode == LumaMode::B || !mb.coeffs_skipped {`

```
//filter across vertical subblocks in macroblock
```

## L1980 · `if mby > 0 {`

```
//filter across top of macroblock
```

## L1997 · `if luma_ylength >= 4 {`

```
//if bottom macroblock, can only filter if there is 3 pixels below
```

## L2040 · `if mb.luma_mode == LumaMode::B || !mb.coeffs_skipped {`

```
//filter across horizontal subblock edges within the macroblock
```

## L2104 · `fn calculate_filter_parameters(&self, macroblock: &MacroBlock) -> (u8, u8, u8) {`

```
//return values are the filter level, interior limit and hev threshold
```

## L2125 · `let mut interior_limit = filter_level;`

```
//interior limit
```

## L2140 · `let mut hev_threshold = 0;`

```
//high edge variance threshold
```

## L2163 · `pub fn decode_frame(&mut self) -> Result<&Frame, DecodingError> {`

```
/// Decodes the current frame
```

## L2198 · `for mby in 0..self.mbheight as usize {`

```
//do loop filtering
```

## L2267 · `bpred: [IntraMode::DC; 16],`

```
// Section 11.3 #3
```

## L2280 · `{`

```
// A
```

## L2310 · `if mbx == 0 {`

```
// L
```

## L2321 · `ws[0] = if mby == 0 {`

```
// P
```

## L2343-2344 · `fn add_residue(pblock: &mut [u8], rblock: &[i32; 16], y0: usize, x0: usize, stride: usize) {`

```
// Only 16 elements from rblock are used to add residue, so it is restricted to 16 elements
// to enable SIMD and other optimizations.
```

## L2382 · `let (above, curr) = a.split_at_mut(stride * y0);`

```
// This pass copies the top row to the rows below it.
```

## L2394 · `for chunk in a.chunks_exact_mut(stride).skip(y0).take(size) {`

```
// This pass copies the first value of a row to the values right of it.
```

## L2433-2447 · `let (above, x_block) = a.split_at_mut(y0 * stride + (x0 - 1));`

```
// The formula for tmpred is:
// X_ij = L_i + A_j - P (i, j=0, 1, 2, 3)
//
// |-----|-----|-----|-----|-----|
// | P   | A0  | A1  | A2  | A3  |
// |-----|-----|-----|-----|-----|
// | L0  | X00 | X01 | X02 | X03 |
// |-----|-----|-----|-----|-----|
// | L1  | X10 | X11 | X12 | X13 |
// |-----|-----|-----|-----|-----|
// | L2  | X20 | X21 | X22 | X23 |
// |-----|-----|-----|-----|-----|
// | L3  | X30 | X31 | X32 | X33 |
// |-----|-----|-----|-----|-----|
// Diagram from p. 52 of RFC 6386
```

## L2449 · `let (above, x_block) = a.split_at_mut(y0 * stride + (x0 - 1));`

```
// Split at L0
```

## L2457 · `x_block[y * stride + 1..][..size]`

```
// Add 1 to skip over L0 byte
```

