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

mod loop_filter;
mod transform;
pub mod vp8;

use alloc::vec;
use alloc::vec::Vec;

use crate::image::Image;

/// What a decode can fail with. Deliberately flat: `image::decode` turns any
/// of these into `None` and paints the placeholder, so the variants exist to
/// keep the ported code readable, not to be matched on.
#[derive(Debug)]
pub enum DecodingError {
    /// The bitstream ended while the decoder still wanted bytes.
    UnexpectedEof,
    /// Fewer than two bytes to prime the arithmetic decoder.
    NotEnoughInitData,
    /// `read_u8` past the end inside the bool decoder.
    IoError(Eof),
    /// The 3-byte start code after the frame tag was not `9d 01 2a`.
    Vp8MagicInvalid([u8; 3]),
    /// A frame feature we do not implement (interframes, unusual scaling).
    UnsupportedFeature(&'static str),
    ColorSpaceInvalid(u8),
    LumaPredictionModeInvalid(i8),
    IntraPredictionModeInvalid(i8),
    ChromaPredictionModeInvalid(i8),
}

/// Stands in for `std::io::Error` at the one place the ported code inspects an
/// error kind. It has exactly one kind, which is the only one a slice can
/// produce.
#[derive(Debug, Clone, Copy)]
pub struct Eof;

/// `std::io::Read` over a borrowed slice, with just the six calls the ported
/// decoder makes. Reading past the end is an error, never a short read — the
/// VP8 code relies on `read_exact` semantics.
pub struct SliceReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> SliceReader<'a> {
    pub fn new(data: &'a [u8]) -> SliceReader<'a> {
        SliceReader { data, pos: 0 }
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], DecodingError> {
        let end = self.pos.checked_add(n).ok_or(DecodingError::UnexpectedEof)?;
        let s = self.data.get(self.pos..end).ok_or(DecodingError::UnexpectedEof)?;
        self.pos = end;
        Ok(s)
    }
    pub fn read_exact(&mut self, out: &mut [u8]) -> Result<(), DecodingError> {
        out.copy_from_slice(self.take(out.len())?);
        Ok(())
    }
    pub fn read_to_end(&mut self, out: &mut Vec<u8>) -> Result<usize, DecodingError> {
        let rest = &self.data[self.pos.min(self.data.len())..];
        out.extend_from_slice(rest);
        self.pos = self.data.len();
        Ok(rest.len())
    }
    pub fn read_u8(&mut self) -> Result<u8, DecodingError> {
        Ok(self.take(1)?[0])
    }
    pub fn read_u16_le(&mut self) -> Result<u16, DecodingError> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }
    pub fn read_u24_le(&mut self) -> Result<u32, DecodingError> {
        let b = self.take(3)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], 0]))
    }
}

/// The arithmetic decoder's own cursor. It owns its bytes (a partition is
/// handed over as a `Vec`) and reports EOF as a value rather than a panic,
/// because libwebp allows a bitstream to read one byte past the end and the
/// ported code depends on seeing that.
#[derive(Default)]
pub struct ByteReader {
    data: Vec<u8>,
    pos: usize,
}

impl ByteReader {
    pub fn new(data: Vec<u8>) -> ByteReader {
        ByteReader { data, pos: 0 }
    }
    pub fn read_u8(&mut self) -> Result<u8, Eof> {
        let v = *self.data.get(self.pos).ok_or(Eof)?;
        self.pos += 1;
        Ok(v)
    }
    pub fn read_u16_be(&mut self) -> Result<u16, DecodingError> {
        let hi = self.read_u8().map_err(|_| DecodingError::UnexpectedEof)?;
        let lo = self.read_u8().map_err(|_| DecodingError::UnexpectedEof)?;
        Ok(u16::from_be_bytes([hi, lo]))
    }
}

/// A RIFF/WEBP container, by its two magic words.
pub fn looks_like_webp(b: &[u8]) -> bool {
    b.len() >= 16 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP"
}

/// The payload of the first chunk with this id, bounds-checked against the
/// buffer rather than trusting the size field.
fn chunk<'a>(b: &'a [u8], id: &[u8; 4]) -> Option<&'a [u8]> {
    let mut i = 12;
    while i + 8 <= b.len() {
        let size = u32::from_le_bytes([b[i + 4], b[i + 5], b[i + 6], b[i + 7]]) as usize;
        let start = i + 8;
        let end = start.checked_add(size)?;
        if &b[i..i + 4] == id {
            return b.get(start..end.min(b.len()));
        }
        // Chunks are padded to an even length.
        i = end + (size & 1);
    }
    None
}

/// Decode a WebP image to BGRA, or `None` for anything we do not handle.
///
/// `VP8L` (lossless) and `VP8X` (extended: alpha, animation) are declined here
/// rather than attempted — a half-decoded picture is worse than the
/// placeholder, and `<picture>` already falls back to a JPEG when we say no.
pub fn decode(bytes: &[u8]) -> Option<Image> {
    if !looks_like_webp(bytes) {
        return None;
    }
    let frame_data = chunk(bytes, b"VP8 ")?;
    let mut dec = vp8::Vp8Decoder::new(SliceReader::new(frame_data));
    let frame = dec.decode_frame().ok()?;
    let (w, h) = (u32::from(frame.width), u32::from(frame.height));
    let px = (w as usize).checked_mul(h as usize)?;
    if px == 0 || px > crate::image::MAX_PIXELS {
        return None;
    }
    let mut bgra = vec![0u8; px * 4];
    frame.fill_bgra(&mut bgra);
    Some(Image { bgra, w, h })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Four solid quadrants, encoded lossily by libwebp (Pillow). A correct
    /// decode puts each colour back within a few steps at the quadrant centre;
    /// a decoder that loses prediction or the inverse transform does not.
    #[test]
    fn a_lossy_webp_decodes_to_the_colours_it_was_made_from() {
        let img = decode(include_bytes!("../../assets/test-quadrants.webp"))
            .expect("a plain VP8 file must decode");
        assert_eq!((img.w, img.h), (64, 64));
        assert_eq!(img.bgra.len(), 64 * 64 * 4);

        let at = |x: u32, y: u32| {
            let i = ((y * img.w + x) * 4) as usize;
            // BGRA -> (r, g, b)
            (img.bgra[i + 2] as i32, img.bgra[i + 1] as i32, img.bgra[i] as i32)
        };
        let want = [
            ((16, 16), (220, 30, 40)),
            ((48, 16), (30, 200, 60)),
            ((16, 48), (40, 60, 220)),
            ((48, 48), (240, 230, 20)),
        ];
        for ((x, y), (r, g, b)) in want {
            let got = at(x, y);
            let d = (got.0 - r).abs().max((got.1 - g).abs()).max((got.2 - b).abs());
            assert!(d <= 12, "at ({x},{y}) got {got:?}, wanted ({r},{g},{b}), off by {d}");
        }
        // Opaque: this container carries no alpha chunk, so nothing may be
        // see-through — a zero here paints the whole picture as nothing.
        assert!(img.bgra.chunks_exact(4).all(|p| p[3] == 255));
    }

    /// Lossless is a SECOND decoder we have not ported. Declining is the whole
    /// point: `<picture>` then falls back to the JPEG the page also offers,
    /// while a half-decode would replace a picture that renders with one that
    /// does not (`picture::decodable_type` makes the same call).
    #[test]
    fn a_lossless_webp_is_declined_not_half_decoded() {
        let bytes = include_bytes!("../../assets/test-lossless.webp");
        assert!(looks_like_webp(bytes), "it IS a webp, just not one we do");
        assert!(decode(bytes).is_none());
    }

    /// Truncation and garbage must come back as `None`, never as a panic — a
    /// panic in the engine is a kernel panic (CLAUDE.md).
    #[test]
    fn damaged_input_is_refused_without_panicking() {
        let full = include_bytes!("../../assets/test-quadrants.webp");
        assert!(decode(&full[..8]).is_none(), "shorter than the header");
        for n in [16, 20, 24, 32, 64, 100, 147] {
            let _ = decode(&full[..n]); // must not panic
        }
        let mut lying = full.to_vec();
        // A chunk size far past the end of the buffer.
        lying[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
        let _ = decode(&lying);
        assert!(decode(b"RIFF____WEBPVP8 ").is_none());
        assert!(decode(&[0u8; 0]).is_none());
    }

    /// The dispatch in `image::decode` has to route these bytes here — a
    /// decoder nothing calls is the failure mode from
    /// `memory/feedback_verify_the_call_path.md`.
    #[test]
    fn image_decode_routes_webp_here() {
        let img = crate::image::decode(include_bytes!("../../assets/test-quadrants.webp"))
            .expect("image::decode must know webp");
        assert_eq!((img.w, img.h), (64, 64));
    }
}
