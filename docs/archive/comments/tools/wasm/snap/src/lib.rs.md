# `tools/wasm/snap/src/lib.rs` @ 5e0102684

## L1-12 · `#![no_std]`

```
//! snap — screenshot tool for nopeekOS.
//!
//! One-shot, no window (full-screen mode). Launched by the bar's camera
//! button via `npk_launch("snap", "full" | "region")`:
//!   - "full"   → capture the whole composited screen, save as PNG.
//!   - "region" → (slice ③) freeze + rubber-band select; for now falls
//!                back to a full capture.
//!
//! The kernel only hands over raw BGRA pixels (`npk_capture_screen`,
//! CAPTURE-gated); snap does the PNG encode + save itself. Files land in
//! `home/<user>/pictures/printscreens/screenshot-NNN.png` (the folder is
//! created on first write — npkFS upsert writes parents).
```

## L28-30 · `#[unsafe(link_section = ".npk.caps")]`

```
// Read (home dir + list) + write (save PNG) + capture (read the screen).
// No RENDER/CANVAS yet — full-screen mode shows no window. Region mode
// (slice ③) will add CANVAS|RENDER for the frozen overlay.
```

## L35-37 · `#[link(wasm_import_module = "env")]`

```
// Host functions are WASM imports from the `env` module, resolved by the
// kernel at instantiation. Naming the module explicitly is what makes them
// imports rather than ordinary undefined C symbols, which rust-lld rejects.
```

## L57-60 · `fn log_ms(label: &str, ms: i64) {`

```
/// Log "<label>: <ms> ms". Permanent, not scaffolding: this runs under a
/// WASM interpreter, so which phase a save spends its seconds in is the
/// difference between fixing the slow thing and rewriting the fast one.
/// Resolution is the 100 Hz timer, i.e. 10 ms steps.
```

## L84 · `const LIST_BUF_SIZE: usize = 64 * 1024;`

```
// ── Buffers ───────────────────────────────────────────────────────────
```

## L91-92 · `const HEAP_SIZE: usize = 192 * 1024 * 1024;`

```
// ── Bump allocator (192 MB — a 4K capture is 33 MB BGRA + ~25 MB RGB
//    scanlines + the deflate output, all live at encode time). ──
```

## L116 · `#[unsafe(no_mangle)]`

```
// ── Entry ─────────────────────────────────────────────────────────────
```

## L119 · `let mut argbuf = [0u8; 32];`

```
// Mode (unused for now — region falls back to full until slice ③).
```

## L124 · `let packed = unsafe { npk_screen_size() };`

```
// Screen size: packed (w << 16) | h.
```

## L133 · `let need = w * h * 4;`

```
// Capture the composited screen as BGRA.
```

## L141-143 · `unsafe { let _ = npk_screen_flash(); }`

```
// Shutter blink — AFTER the capture, so the white is never in the
// shot, and BEFORE the encode, so the acknowledgement is immediate
// rather than a second later when the file lands.
```

## L146 · `let png = encode_png_rgb(&bgra, w as u32, h as u32);`

```
// Encode PNG (RGB, screenshots have no meaningful alpha).
```

## L150 · `let home = read_home_dir();`

```
// Save under the printscreens folder (created on first write).
```

## L160 · `log_ms("=== capture -> saved", t_stored - t_start);`

```
// The number that matters: button press → file on disk.
```

## L165 · `fn read_home_dir() -> String {`

```
// ── npkFS helpers ─────────────────────────────────────────────────────
```

## L174-175 · `fn next_name(dir: &str) -> String {`

```
/// Next free `screenshot-NNN.png` in `dir` (clock-free naming — we have
/// no time host fn, so number sequentially from the existing files).
```

## L193 · `fn encode_png_rgb(bgra: &[u8], w: u32, h: u32) -> Vec<u8> {`

```
// ── PNG encoder (RGB8, single IDAT, filter 0) ─────────────────────────
```

## L198-208 · `let row_bytes = 1 + wc * 3;`

```
// Raw scanlines: 1 filter byte (0 = None) + W*3 RGB bytes per row.
//
// Filter 0 for every row is deliberate on both ends: it costs the
// encoder nothing, and it lets iris un-filter a whole row with one
// `copy_from_slice` (a wasm `memory.copy`) instead of a per-byte
// add. A cleverer filter would shrink the file and make BOTH sides
// walk 25 MB byte by byte under the interpreter.
//
// Pre-zeroed + indexed writes rather than `push`: three pushes per
// pixel is ~25 M capacity checks at 4K. `chunks_exact` drops the
// per-access bounds checks on top.
```

## L212 · `let dst = y * row_bytes + 1;             // filter byte stays 0`

```
// filter byte stays 0
```

## L217 · `d[0] = s[2];  // B G R A → R G B`

```
// B G R A → R G B
```

## L225-231 · `let idat = miniz_oxide::deflate::compress_to_vec_zlib(&raw, 1);`

```
// zlib-wrapped deflate (header + deflate + adler32).
//
// Level 1, not 6. Measured on a 4K screen under the interpreter:
// level 6 spent 4240 ms of a 4970 ms save inside deflate — 85 % of
// the wait for a file that is only somewhat smaller. A screenshot is
// a transient artefact; seconds of the user's time cost more than
// the megabyte.
```

## L239 · `let mut ihdr = Vec::with_capacity(13);`

```
// IHDR
```

## L243 · `ihdr.push(8);  // bit depth`

```
// bit depth
```

## L244 · `ihdr.push(2);  // color type 2 = truecolour RGB`

```
// color type 2 = truecolour RGB
```

## L245 · `ihdr.push(0);  // compression`

```
// compression
```

## L246 · `ihdr.push(0);  // filter`

```
// filter
```

## L247 · `ihdr.push(0);  // interlace`

```
// interlace
```

## L251-252 · `log_ms("chunks + crc32", now_ms() - t_deflate);`

```
// CRC runs over every IDAT byte, so it scales with the image, not
// with the chunk count — worth its own number.
```

## L267-270 · `const CRC_TABLE: [u32; 256] = {`

```
// CRC-32 (IEEE, PNG), table-driven. The chunk COUNT is small but the
// IDAT chunk is the whole image: bitwise cost 8 iterations per byte and
// measured 200 ms of a 4K save. The table is built at compile time, so
// it costs 1 KB of module data and no startup work.
```

## L299 · `#[allow(dead_code)]`

```
// Keep IconRef referenced (used via the build.rs-generated AppMeta blob).
```

