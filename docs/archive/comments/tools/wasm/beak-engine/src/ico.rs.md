# `tools/wasm/beak-engine/src/ico.rs` @ 5e0102684

## L1-14 · `use crate::image::Image;`

```
//! ico.rs — Windows ICO/CUR decode.
//!
//! Every search result carries one: `favicon.ico` is still what sites ship and
//! what aggregators (DuckDuckGo's `/ip3/<host>.ico`) hand back, so without this
//! a result list is a column of empty boxes.
//!
//! A `.ico` is a directory of images. Each entry is either a whole PNG (Vista+)
//! — handed straight back to the PNG decoder — or a "DIB": a BITMAPINFOHEADER
//! whose height counts DOUBLE, because a colour bitmap is followed by a 1-bit
//! AND mask. That mask is the transparency for every depth below 32, and the
//! rescue for the many 32-bit icons that ship an all-zero alpha channel.
//!
//! Rows are bottom-up and padded to a 4-byte boundary — both are properties of
//! the DIB format, not of the icon.
```

## L18 · `pub fn looks_like_ico(b: &[u8]) -> bool {`

```
/// An ICO (type 1) or CUR (type 2) directory header.
```

## L30 · `pub fn decode(b: &[u8]) -> Option<Image> {`

```
/// Decode the best entry of an ICO/CUR.
```

## L39-41 · `let mut best: Option<(u32, u32, usize, usize)> = None; // (area, bpp, off, len)`

```
// Pick the largest entry, tie-broken by colour depth: a favicon is scaled
// into a ~16 px box, and downscaling the richest source beats upscaling a
// 16×16 one. `0` in the size byte means 256 (it does not fit in a byte).
```

## L42 · `let mut best: Option<(u32, u32, usize, usize)> = None; // (area, bpp, off, len)`

```
// (area, bpp, off, len)
```

## L63-64 · `if payload.len() >= 8 && payload[0..8] == *b"\x89PNG\r\n\x1a\n" {`

```
// Vista+ entries are a whole PNG file. The directory's own width/height are
// advisory there; the PNG header is authoritative.
```

## L71 · `fn decode_dib(d: &[u8]) -> Option<Image> {`

```
/// A BITMAPINFOHEADER bitmap plus its trailing AND mask.
```

## L81 · `let h2 = u32le(d, 8);`

```
// The stored height covers colour bitmap AND mask, so the image is half it.
```

## L85-87 · `if compression != 0 && compression != 3 {`

```
// 0 = BI_RGB. 3 = BI_BITFIELDS, which for an icon is always the plain
// 8-8-8-8 layout we already read; anything else (RLE, JPEG, PNG) is not
// something an icon uses.
```

## L96-97 · `let pal_entries = match bpp {`

```
// Palette: present for every indexed depth. `biClrUsed` may be 0, meaning
// the full 2^bpp table.
```

## L110 · `let row_bytes = ((w as usize * bpp as usize + 31) / 32) * 4;`

```
// Rows are padded to 4 bytes, bottom-up.
```

## L114 · `let mask_row = ((w as usize + 31) / 32) * 4;`

```
// The AND mask is 1 bpp with its own padding. Truncated/absent → opaque.
```

## L121 · `let src = &xor[(h as usize - 1 - y) * row_bytes..];`

```
// Bottom-up: the first stored row is the last visual row.
```

## L136 · `let p = x * 2;`

```
// 5-5-5 with the top bit unused, expanded so 0x1F → 0xFF.
```

## L165-167 · `if bpp != 32 || !any_alpha {`

```
// The AND mask decides transparency for every depth below 32 — and for a
// 32-bit icon whose alpha channel is entirely zero, which would otherwise
// decode to a fully invisible image.
```

## L174 · `bgra[(y * w as usize + x) * 4 + 3] = if bit == 1 { 0 } else { 255 };`

```
// 1 = "leave the background" = transparent.
```

## L179 · `for p in bgra.chunks_exact_mut(4) {`

```
// No mask and no alpha at all: the icon is opaque, not invisible.
```

## L188-189 · `const MAX_ICON_PIXELS: usize = 512 * 512;`

```
/// Icons are small by definition; this only stops a malformed header from
/// asking for a gigabyte.
```

## L197 · `fn wrap(dib: Vec<u8>, w: u8, h: u8, bpp: u16) -> Vec<u8> {`

```
/// Build a one-entry ICO around a raw DIB payload.
```

## L214 · `d.extend_from_slice(&1u16.to_le_bytes()); // planes`

```
// planes
```

## L216 · `d.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB`

```
// BI_RGB
```

## L217 · `d.extend_from_slice(&0u32.to_le_bytes()); // size`

```
// size
```

## L227 · `let mut d = header(2, 2, 32, 0);`

```
// 2×1, BGRA: opaque blue, then a fully transparent pixel.
```

## L229 · `d.extend_from_slice(&[255, 0, 0, 255, 0, 0, 0, 0]); // one row, padded already`

```
// one row, padded already
```

## L230 · `d.extend_from_slice(&[0, 0, 0, 0]); // AND mask row (4-byte padded)`

```
// AND mask row (4-byte padded)
```

## L239 · `let mut d = header(2, 2, 4, 2);`

```
// Palette entry 1 = red; the AND mask hides the second pixel.
```

## L241 · `d.extend_from_slice(&[0, 0, 0, 0]); // index 0 — black`

```
// index 0 — black
```

## L242 · `d.extend_from_slice(&[0, 0, 255, 0]); // index 1 — red (BGRx)`

```
// index 1 — red (BGRx)
```

## L243 · `d.extend_from_slice(&[0x11, 0, 0, 0]); // both pixels index 1, row padded`

```
// both pixels index 1, row padded
```

## L244 · `d.extend_from_slice(&[0b0100_0000, 0, 0, 0]); // mask: 2nd pixel transparent`

```
// mask: 2nd pixel transparent
```

## L252-253 · `let mut d = header(1, 2, 32, 0);`

```
// Plenty of real favicons ship 32bpp with an unset alpha channel and
// rely on the AND mask. Trusting the channel renders nothing at all.
```

## L256 · `d.extend_from_slice(&[0, 0, 0, 0]); // mask: visible`

```
// mask: visible
```

## L263 · `let small = {`

```
// Two entries; the 2×1 one must be chosen over the 1×1.
```

