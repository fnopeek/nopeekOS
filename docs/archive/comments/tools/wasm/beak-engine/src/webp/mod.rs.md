# `tools/wasm/beak-engine/src/webp/mod.rs` @ 5e0102684

## L1-21 · `mod loop_filter;`

```
//! WebP: the RIFF container plus a `no_std` port of the lossy VP8 decoder.
//!
//! **Why this and not a crate:** `image-webp` is pure Rust and would be the
//! obvious dependency, but it is `std`-only (its decoder is built on
//! `std::io::Read`). Its VP8 core, however, touches `std` in exactly four
//! lines, and `loop_filter.rs`/`transform.rs` have no imports at all — so the
//! honest move is to port it rather than to reimplement it.
//!
//! `vp8.rs`, `loop_filter.rs` and `transform.rs` are taken from
//! **image-webp 0.1.3** (<https://github.com/image-rs/image-webp>, MIT OR
//! Apache-2.0, © the image-rs developers) and changed only where `std` had to
//! go: the two readers below replace `std::io::Read`/`Cursor` + `byteorder`,
//! `DecodingError` replaces the crate's, and `fill_bgra` was added because
//! beak paints BGRA. The decoding logic itself is untouched, so a fix upstream
//! stays diffable against ours.
//!
//! **Why only lossy:** measured over the page corpus — 12 of 12 sampled images
//! on srf.ch and tagesschau.de are `VP8 ` in the plain container, with no
//! `VP8X`, `ALPH` or `ANIM` chunk. Lossless (`VP8L`) is a second decoder and
//! waits until a real page asks for it; until then it is REJECTED, not
//! half-decoded (see `docs/plan/HTML_GAP_2026_08.md`).
```

## L32-34 · `#[derive(Debug)]`

```
/// What a decode can fail with. Deliberately flat: `image::decode` turns any
/// of these into `None` and paints the placeholder, so the variants exist to
/// keep the ported code readable, not to be matched on.
```

## L37 · `UnexpectedEof,`

```
/// The bitstream ended while the decoder still wanted bytes.
```

## L39 · `NotEnoughInitData,`

```
/// Fewer than two bytes to prime the arithmetic decoder.
```

## L41 · `IoError(Eof),`

```
/// `read_u8` past the end inside the bool decoder.
```

## L43 · `Vp8MagicInvalid([u8; 3]),`

```
/// The 3-byte start code after the frame tag was not `9d 01 2a`.
```

## L45 · `UnsupportedFeature(&'static str),`

```
/// A frame feature we do not implement (interframes, unusual scaling).
```

## L53-55 · `#[derive(Debug, Clone, Copy)]`

```
/// Stands in for `std::io::Error` at the one place the ported code inspects an
/// error kind. It has exactly one kind, which is the only one a slice can
/// produce.
```

## L59-61 · `pub struct SliceReader<'a> {`

```
/// `std::io::Read` over a borrowed slice, with just the six calls the ported
/// decoder makes. Reading past the end is an error, never a short read — the
/// VP8 code relies on `read_exact` semantics.
```

## L100-103 · `#[derive(Default)]`

```
/// The arithmetic decoder's own cursor. It owns its bytes (a partition is
/// handed over as a `Vec`) and reports EOF as a value rather than a panic,
/// because libwebp allows a bitstream to read one byte past the end and the
/// ported code depends on seeing that.
```

## L126 · `pub fn looks_like_webp(b: &[u8]) -> bool {`

```
/// A RIFF/WEBP container, by its two magic words.
```

## L131-132 · `fn chunk<'a>(b: &'a [u8], id: &[u8; 4]) -> Option<&'a [u8]> {`

```
/// The payload of the first chunk with this id, bounds-checked against the
/// buffer rather than trusting the size field.
```

## L142 · `i = end + (size & 1);`

```
// Chunks are padded to an even length.
```

## L148-152 · `pub fn decode(bytes: &[u8]) -> Option<Image> {`

```
/// Decode a WebP image to BGRA, or `None` for anything we do not handle.
///
/// `VP8L` (lossless) and `VP8X` (extended: alpha, animation) are declined here
/// rather than attempted — a half-decoded picture is worse than the
/// placeholder, and `<picture>` already falls back to a JPEG when we say no.
```

## L174-176 · `#[test]`

```
/// Four solid quadrants, encoded lossily by libwebp (Pillow). A correct
/// decode puts each colour back within a few steps at the quadrant centre;
/// a decoder that loses prediction or the inverse transform does not.
```

## L186 · `(img.bgra[i + 2] as i32, img.bgra[i + 1] as i32, img.bgra[i] as i32)`

```
// BGRA -> (r, g, b)
```

## L200-201 · `assert!(img.bgra.chunks_exact(4).all(|p| p[3] == 255));`

```
// Opaque: this container carries no alpha chunk, so nothing may be
// see-through — a zero here paints the whole picture as nothing.
```

## L205-208 · `#[test]`

```
/// Lossless is a SECOND decoder we have not ported. Declining is the whole
/// point: `<picture>` then falls back to the JPEG the page also offers,
/// while a half-decode would replace a picture that renders with one that
/// does not (`picture::decodable_type` makes the same call).
```

## L216-217 · `#[test]`

```
/// Truncation and garbage must come back as `None`, never as a panic — a
/// panic in the engine is a kernel panic (CLAUDE.md).
```

## L223 · `let _ = decode(&full[..n]); // must not panic`

```
// must not panic
```

## L226 · `lying[16..20].copy_from_slice(&u32::MAX.to_le_bytes());`

```
// A chunk size far past the end of the buffer.
```

## L233-235 · `#[test]`

```
/// The dispatch in `image::decode` has to route these bytes here — a
/// decoder nothing calls is the failure mode from
/// `memory/feedback_verify_the_call_path.md`.
```

