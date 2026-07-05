//! image.rs — raster image decode for `<img>`.
//!
//! PNG (8-bit RGB/RGBA, non-interlaced, ported from iris/wallpaper) and JPEG
//! (baseline + progressive, via the no_std zune-jpeg decoder). Other formats
//! (SVG, GIF, WebP) fall back to a labelled placeholder box in layout. The
//! shell fetches the bytes (the engine is host-free); the engine decodes them
//! into `Image`s keyed by the original `src`, ready for layout + paint.

use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use hashbrown::HashMap;

/// A decoded image: premultiplied? no — straight BGRA, top-down, `w`×`h`.
pub struct Image {
    pub bgra: Vec<u8>,
    pub w: u32,
    pub h: u32,
}

/// Decoded images available to layout/paint, keyed by the `<img>`'s `src`
/// attribute exactly as written in the HTML (the shell stores them so).
pub type ImageMap = HashMap<String, Rc<Image>>;

/// Cap on decoded pixels per image — skip decoding anything larger so a single
/// huge asset can't exhaust the shell heap (it degrades to a placeholder).
const MAX_PIXELS: usize = 4_000_000; // ~16 MB BGRA

/// Allocate `n` zeroed bytes WITHOUT aborting on OOM — `try_reserve` returns
/// `Err` instead of calling `handle_alloc_error`, so an oversize image degrades
/// to a placeholder (decode → `None`) rather than killing the whole app/tab.
fn zeroed(n: usize) -> Option<Vec<u8>> {
    let mut v: Vec<u8> = Vec::new();
    v.try_reserve_exact(n).ok()?;
    v.resize(n, 0);
    Some(v)
}

/// Decode image bytes. Returns `None` for unsupported formats / malformed data
/// / oversize images (→ placeholder).
pub fn decode(bytes: &[u8]) -> Option<Image> {
    if bytes.len() >= 8 && bytes[0..8] == *b"\x89PNG\r\n\x1a\n" {
        decode_png(bytes)
    } else if bytes.len() >= 3 && bytes[0..3] == [0xFF, 0xD8, 0xFF] {
        decode_jpeg(bytes)
    } else {
        None
    }
}

// ── JPEG (baseline + progressive, via zune-jpeg) ────────────────────────────

fn decode_jpeg(bytes: &[u8]) -> Option<Image> {
    use zune_core::colorspace::ColorSpace;
    use zune_core::options::DecoderOptions;
    use zune_jpeg::JpegDecoder;

    // Force interleaved RGB output (YCbCr and grayscale both convert to RGB),
    // and cap decoded dimensions so a huge asset can't exhaust the heap.
    let opts = DecoderOptions::default()
        .jpeg_set_out_colorspace(ColorSpace::RGB)
        .set_max_width(8192)
        .set_max_height(8192);
    let mut dec = JpegDecoder::new_with_options(bytes, opts);
    dec.decode_headers().ok()?;
    let (w, h) = dec.dimensions()?; // (width, height) in px
    if w == 0 || h == 0 || w.saturating_mul(h) > MAX_PIXELS {
        return None;
    }
    let rgb = dec.decode().ok()?;
    let count = w.checked_mul(h)?;
    // Output is 3-channel RGB (we requested it); bail if the buffer is short.
    if rgb.len() < count.checked_mul(3)? {
        return None;
    }
    let mut bgra = zeroed(count.checked_mul(4)?)?;
    for i in 0..count {
        let s = i * 3;
        let d = i * 4;
        bgra[d] = rgb[s + 2]; // B
        bgra[d + 1] = rgb[s + 1]; // G
        bgra[d + 2] = rgb[s]; // R
        bgra[d + 3] = 255;
    }
    Some(Image { bgra, w: w as u32, h: h as u32 })
}

// ── PNG (8-bit RGB/RGBA, non-interlaced) ────────────────────────────────────

fn decode_png(data: &[u8]) -> Option<Image> {
    let mut pos = 8;
    let (mut width, mut height): (u32, u32) = (0, 0);
    let mut color_type: u8 = 0;
    let mut idat: Vec<u8> = Vec::new();
    idat.try_reserve(data.len()).ok()?;

    while pos + 12 <= data.len() {
        let clen = u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]) as usize;
        let ctype = &data[pos + 4..pos + 8];
        let start = pos + 8;
        let end = start.checked_add(clen)?;
        if end > data.len() {
            break;
        }
        match ctype {
            b"IHDR" => {
                if clen < 13 {
                    return None;
                }
                let d = &data[start..];
                width = u32::from_be_bytes([d[0], d[1], d[2], d[3]]);
                height = u32::from_be_bytes([d[4], d[5], d[6], d[7]]);
                if d[8] != 8 {
                    return None; // bit depth
                }
                color_type = d[9];
                if d[10] != 0 || d[11] != 0 || d[12] != 0 {
                    return None; // compression / filter / interlace
                }
                if color_type != 2 && color_type != 6 {
                    return None; // only RGB / RGBA
                }
                // Early dimension guard: reject an oversize image at IHDR (the
                // FIRST chunk) so we never accumulate its IDAT / allocate raw +
                // bgra — the OOM spike is bounded before it starts.
                if (width as usize).saturating_mul(height as usize) > MAX_PIXELS {
                    return None;
                }
            }
            b"IDAT" => idat.extend_from_slice(&data[start..end]),
            b"IEND" => break,
            _ => {}
        }
        pos = end + 4; // + CRC
    }

    if width == 0 || height == 0 || idat.len() < 6 {
        return None;
    }
    if (width as usize).checked_mul(height as usize)? > MAX_PIXELS {
        return None;
    }

    let channels: usize = if color_type == 6 { 4 } else { 3 };
    let stride = width as usize * channels;

    let decomp = miniz_oxide::inflate::decompress_to_vec_zlib(&idat)
        .ok()
        .or_else(|| miniz_oxide::inflate::decompress_to_vec(&idat[2..]).ok())?;
    if decomp.len() < height as usize * (1 + stride) {
        return None;
    }

    // Reverse the per-row PNG filters into raw samples.
    let mut raw = zeroed(height as usize * stride)?;
    for y in 0..height as usize {
        let src = y * (1 + stride);
        let filter = decomp[src];
        let row = src + 1;
        let dst = y * stride;
        for x in 0..stride {
            let cur = decomp[row + x];
            let a = if x >= channels { raw[dst + x - channels] } else { 0 };
            let b = if y > 0 { raw[dst - stride + x] } else { 0 };
            let c = if x >= channels && y > 0 { raw[dst - stride + x - channels] } else { 0 };
            raw[dst + x] = match filter {
                1 => cur.wrapping_add(a),
                2 => cur.wrapping_add(b),
                3 => cur.wrapping_add(((a as u16 + b as u16) / 2) as u8),
                4 => cur.wrapping_add(paeth(a, b, c)),
                _ => cur,
            };
        }
    }

    let count = (width * height) as usize;
    let mut bgra = zeroed(count * 4)?;
    for i in 0..count {
        let s = i * channels;
        let d = i * 4;
        bgra[d] = raw[s + 2];
        bgra[d + 1] = raw[s + 1];
        bgra[d + 2] = raw[s];
        bgra[d + 3] = if channels == 4 { raw[s + 3] } else { 255 };
    }
    Some(Image { bgra, w: width, h: height })
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = a as i16 + b as i16 - c as i16;
    let (pa, pb, pc) = ((p - a as i16).unsigned_abs(), (p - b as i16).unsigned_abs(), (p - c as i16).unsigned_abs());
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

/// Total decoded-pixel budget across a page — once exceeded, further images
/// stay placeholders so an image-heavy page can't exhaust the shell heap.
pub(crate) const TOTAL_BUDGET: usize = 24 * 1024 * 1024; // bytes of BGRA

/// Decode a batch of (src, bytes) into an `ImageMap` (failures / over-budget →
/// skipped, they render as placeholders).
pub fn decode_all(pairs: &[(String, Vec<u8>)]) -> ImageMap {
    let mut map = ImageMap::new();
    let mut budget = TOTAL_BUDGET;
    for (src, bytes) in pairs {
        if let Some(img) = decode(bytes) {
            if img.bgra.len() > budget {
                continue;
            }
            budget -= img.bgra.len();
            map.insert(src.clone(), Rc::new(img));
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(out: &mut Vec<u8>, typ: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(typ);
        out.extend_from_slice(data);
        out.extend_from_slice(&[0, 0, 0, 0]); // CRC — our decoder skips it
    }

    #[test]
    fn decodes_a_2x1_rgb_png() {
        // one scanline: filter byte 0, then two RGB pixels red, green
        let idat = miniz_oxide::deflate::compress_to_vec_zlib(&[0u8, 255, 0, 0, 0, 255, 0], 6);
        let mut png: Vec<u8> = alloc::vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&2u32.to_be_bytes());
        ihdr.extend_from_slice(&1u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit, RGB, no compression/filter/interlace
        chunk(&mut png, b"IHDR", &ihdr);
        chunk(&mut png, b"IDAT", &idat);
        chunk(&mut png, b"IEND", &[]);

        let img = decode(&png).expect("valid PNG decodes");
        assert_eq!((img.w, img.h), (2, 1));
        assert_eq!(&img.bgra[0..4], &[0, 0, 255, 255]); // red → BGRA
        assert_eq!(&img.bgra[4..8], &[0, 255, 0, 255]); // green → BGRA
    }

    #[test]
    fn non_png_bytes_decode_to_none() {
        assert!(decode(b"\xff\xd8\xff\xe0 not a png").is_none());
    }
}
