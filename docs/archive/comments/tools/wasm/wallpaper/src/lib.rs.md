# `tools/wasm/wallpaper/src/lib.rs` @ 5e0102684

## L1-9 · `#![no_std]`

```
//! Wallpaper WASM module for nopeekOS.
//!
//! Two modes, both via `_start`:
//!   1. **Decode**: target file is a filename → fetch PNG → decode → set.
//!   2. **Generate**: target starts with `@demos:<W>x<H>:<wp_dir>` →
//!      write 4 gradient wallpapers into `<wp_dir>/<theme>`.
//!
//! The kernel picks the mode by writing `.npk-wallpaper-target`
//! accordingly. Runs inside the nopeekOS WASM sandbox (wasmi).
```

## L24 · `#[link(wasm_import_module = "env")]`

```
// --- Host function bindings (provided by nopeekOS kernel) ---
```

## L26-28 · `#[link(wasm_import_module = "env")]`

```
// Host functions are WASM imports from the `env` module, resolved by the
// kernel at instantiation. Naming the module explicitly is what makes them
// imports rather than ordinary undefined C symbols, which rust-lld rejects.
```

## L65-80 · `const HEAP_SIZE: usize = 256 * 1024 * 1024; // 256 MB`

```
// --- Simple bump allocator for WASM ---
//
// 256 MB heap covers worst-case 4K-PNG decode. The bump allocator
// never frees, so every intermediate buffer the decoder holds onto
// stacks up:
//   - input PNG buffer (decode mode):    up to 32 MB
//   - IDAT concatenation Vec:            ~9 MB (pre-sized)
//   - miniz_oxide decompressed output:   ~38 MB (4K, 1 byte filter
//                                                + 4 channels per
//                                                pixel @ 3840×2400)
//   - per-row unfilter buffer:           ~37 MB
//   - final BGRA buffer:                 ~37 MB
//   - miniz_oxide internal state:        ~5 MB
// Total: ~158 MB worst-case at 4K. 128 MB blew up at v0.4.1.
// 256 MB linear memory is fine — wasmi grows on demand and the
// module's lifetime is one intent invocation.
```

## L81 · `const HEAP_SIZE: usize = 256 * 1024 * 1024; // 256 MB`

```
// 256 MB
```

## L103 · `}`

```
// Bump allocator: no deallocation (module runs once and exits)
```

## L116 · `#[unsafe(no_mangle)]`

```
// --- Entry point ---
```

## L120-121 · `let mut name_buf = [0u8; 512];`

```
// Read target from .npk-wallpaper-target — either a PNG filename
// (decode mode) or a `@demos:<W>x<H>:<wp_dir>` string (generate mode).
```

## L147 · `fn parse_wh(dims: &str) -> Option<(u32, u32)> {`

```
// --- Shared helpers -----------------------------------------------------
```

## L149 · `fn parse_wh(dims: &str) -> Option<(u32, u32)> {`

```
/// Parse "<W>x<H>" from a slice.
```

## L167-168 · `fn alloc_bgra(w: u32, h: u32) -> Vec<u8> {`

```
/// Allocate a BGRA buffer with the 8-byte (W LE + H LE) header the
/// kernel's npk_set_wallpaper consumes.
```

## L189-190 · `fn run_solid(spec: &str) {`

```
// --- Solid color --------------------------------------------------------
// @solid:<hex>:<W>x<H>:<dir>
```

## L221-222 · `fn run_gradient2(spec: &str) {`

```
// --- 2-color vertical gradient -----------------------------------------
// @gradient2:<top>:<bot>:<W>x<H>:<dir>
```

## L242-243 · `fn run_gradient4(spec: &str) {`

```
// --- 4-corner bilinear gradient ----------------------------------------
// @gradient4:<tl>:<tr>:<bl>:<br>:<W>x<H>:<dir>
```

## L284-286 · `fn run_pattern(spec: &str) {`

```
// --- Patterns ----------------------------------------------------------
// @pattern:<name>:<fg>:<bg>:<W>x<H>:<dir>
// names: dots, stripes, checker, grid, noise
```

## L323 · `for y in 0..h {`

```
// 32px lattice, 3px radius dots.
```

## L336 · `for y in 0..h {`

```
// Diagonal 24px bands.
```

## L347 · `for y in 0..h {`

```
// 48px tiles.
```

## L358 · `for y in 0..h {`

```
// 1px lines on a 32px grid.
```

## L369 · `for y in 0..h {`

```
// Hash each pixel coord to a byte, mix fg/bg by that.
```

## L388 · `fn run_decode(filename: &str) {`

```
// --- Decode mode (PNG → BGRA → set framebuffer) ---
```

## L391-396 · `let max_size = 32 * 1024 * 1024;`

```
// 32 MB cap — covers a high-quality 4K JPEG-equivalent PNG (the
// shipped npk01.png is 9 MB at native 1080p; high-detail 4K
// wallpapers can hit 20-25 MB). Anything larger would need
// streaming decode, which `npk_fetch` doesn't support today.
// Was 6 MB before v0.4.1: truncated 9 MB inputs → decode panic
// on missing IEND chunk.
```

## L419 · `struct Theme {`

```
// --- Legacy demo mode (4 gradient themes into wp_dir) ---
```

## L421-422 · `struct Theme {`

```
/// Corner colors for one theme: top-left, top-right, bottom-left,
/// bottom-right (R, G, B).
```

## L434 · `tl: (2, 3, 15),        // near black`

```
// near black
```

## L435 · `tr: (5, 30, 60),       // dark navy`

```
// dark navy
```

## L436 · `bl: (10, 60, 120),     // deep ocean`

```
// deep ocean
```

## L437 · `br: (60, 200, 240),    // bright cyan`

```
// bright cyan
```

## L441 · `tl: (10, 2, 15),       // near black`

```
// near black
```

## L442 · `tr: (80, 10, 30),      // dark crimson`

```
// dark crimson
```

## L443 · `bl: (160, 40, 20),     // deep orange`

```
// deep orange
```

## L444 · `br: (255, 160, 80),    // bright amber`

```
// bright amber
```

## L448 · `tl: (2, 8, 3),         // near black`

```
// near black
```

## L449 · `tr: (8, 40, 15),       // dark forest`

```
// dark forest
```

## L450 · `bl: (15, 80, 30),      // deep emerald`

```
// deep emerald
```

## L451 · `br: (80, 220, 100),    // bright green`

```
// bright green
```

## L455 · `tl: (5, 2, 18),        // near black`

```
// near black
```

## L456 · `tr: (30, 10, 80),      // dark indigo`

```
// dark indigo
```

## L457 · `bl: (80, 20, 160),     // deep purple`

```
// deep purple
```

## L458 · `br: (180, 100, 255),   // bright violet`

```
// bright violet
```

## L463 · `let (dims, wp_dir) = match spec.split_once(':') {`

```
// Parse "<W>x<H>:<wp_dir>"
```

## L481-482 · `let pixel_count = (width as usize) * (height as usize);`

```
// One reusable BGRA buffer with an 8-byte (W LE + H LE) header —
// kernel's background::set_wallpaper consumes this exact layout.
```

## L500-502 · `fn fill_gradient(pixels: &mut [u8], w: u32, h: u32, t: &Theme) {`

```
/// Fill a BGRA pixel slice with the bilinear-interpolated gradient of
/// `theme`, plus a sine streak overlay and a bottom-right radial glow.
/// The pixel math mirrors the original kernel implementation.
```

## L513 · `let diag = ((x as i32 * 600 + y as i32 * 800) / 1000) as u32;`

```
// Diagonal sine streaks (soft aurora-like bands)
```

## L520 · `let dx = (fx as i32 - 700).abs();`

```
// Radial glow toward bottom-right
```

## L537 · `fn bilinear(tl: u8, tr: u8, bl: u8, br: u8, fx: u32, fy: u32) -> u8 {`

```
/// Bilinear interpolation of 4 corner values.
```

## L544 · `fn sine_lut(x: u32) -> i32 {`

```
/// Quarter-wave sine lookup (0..1023 → −128..127 via mirroring).
```

## L563 · `fn decode_png(data: &[u8]) -> Option<(Vec<u8>, u32, u32)> {`

```
// --- PNG Decoder ---
```

## L566 · `if data.len() < 8 || &data[0..8] != b"\x89PNG\r\n\x1a\n" {`

```
// Verify PNG signature
```

## L576-580 · `let mut idat_data: Vec<u8> = Vec::with_capacity(data.len());`

```
// IDAT concatenation: pre-size to the *whole* PNG payload so the
// chunk-by-chunk `extend_from_slice` doesn't re-allocate (and,
// because the bump allocator never frees, leak each previous
// buffer in the geometric doubling cascade — that's a multi-
// MB leak on a 4K wallpaper).
```

## L583 · `while pos + 12 <= data.len() {`

```
// Parse chunks
```

## L613 · `log("[wallpaper] only RGB/RGBA PNG supported");`

```
// 2 = RGB, 6 = RGBA
```

## L622 · `_ => {} // Skip unknown chunks`

```
// Skip unknown chunks
```

## L625 · `pos = chunk_data_end + 4; // +4 for CRC`

```
// +4 for CRC
```

## L632 · `let channels: usize = match color_type {`

```
// Channels per pixel
```

## L634 · `2 => 3, // RGB`

```
// RGB
```

## L635 · `6 => 4, // RGBA`

```
// RGBA
```

## L639 · `let stride = width as usize * channels; // bytes per row (without filter byte)`

```
// bytes per row (without filter byte)
```

## L641 · `if idat_data.len() < 6 { return None; }`

```
// Decompress IDAT (zlib = 2-byte header + deflate + 4-byte checksum)
```

## L643 · `let deflate_data = &idat_data[2..];`

```
// Skip zlib header (2 bytes), decompress deflate stream
```

## L648 · `match miniz_oxide::inflate::decompress_to_vec(deflate_data) {`

```
// Try raw deflate without zlib wrapper
```

## L656 · `let expected = height as usize * (1 + stride); // 1 filter byte per row`

```
// 1 filter byte per row
```

## L662 · `let mut unfiltered = vec![0u8; height as usize * stride];`

```
// Unfilter rows
```

## L672 · `let a = if x >= channels { unfiltered[dst_offset + x - channels] } else { 0 }; // left`

```
// left
```

## L673 · `let b = if y > 0 { unfiltered[dst_offset - stride + x] } else { 0 }; // up`

```
// up
```

## L674 · `let c = if x >= channels && y > 0 { unfiltered[dst_offset - stride + x - channels] } else { 0 }; // up-left`

```
// up-left
```

## L677 · `0 => raw,                                          // None`

```
// None
```

## L678 · `1 => raw.wrapping_add(a),                         // Sub`

```
// Sub
```

## L679 · `2 => raw.wrapping_add(b),                         // Up`

```
// Up
```

## L680 · `3 => raw.wrapping_add(((a as u16 + b as u16) / 2) as u8), // Average`

```
// Average
```

## L681 · `4 => raw.wrapping_add(paeth(a, b, c)),            // Paeth`

```
// Paeth
```

## L688 · `let pixel_count = (width * height) as usize;`

```
// Convert to BGRA (framebuffer format)
```

## L694 · `bgra[dst]     = unfiltered[src + 2]; // B`

```
// B
```

## L695 · `bgra[dst + 1] = unfiltered[src + 1]; // G`

```
// G
```

## L696 · `bgra[dst + 2] = unfiltered[src];     // R`

```
// R
```

## L697 · `bgra[dst + 3] = if channels == 4 { unfiltered[src + 3] } else { 255 }; // A`

```
// A
```

## L703 · `fn paeth(a: u8, b: u8, c: u8) -> u8 {`

```
/// Paeth predictor (PNG filter type 4).
```

