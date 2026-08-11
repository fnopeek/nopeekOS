//! image.rs — raster image decode for `<img>`.
//!
//! PNG (grayscale/RGB/palette/gray+alpha, bit depths 1/2/4/8/16, non-interlaced,
//! with PLTE + tRNS transparency), JPEG (baseline + progressive, via the
//! no_std zune-jpeg decoder), ICO (`ico`) and SVG (`svg`). Other formats (GIF,
//! WebP) fall back to a labelled placeholder box in layout. The
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
pub(crate) fn zeroed(n: usize) -> Option<Vec<u8>> {
    let mut v: Vec<u8> = Vec::new();
    v.try_reserve_exact(n).ok()?;
    v.resize(n, 0);
    Some(v)
}

/// The payload bytes of a `data:` URI (RFC 2397), base64 or percent-encoded.
///
/// CSS icon systems inline their SVGs this way, so these need no fetch at all —
/// the bytes are already in the stylesheet.
pub fn decode_data_uri(uri: &str) -> Option<Vec<u8>> {
    let rest = uri.strip_prefix("data:").or_else(|| uri.strip_prefix("DATA:"))?;
    let comma = rest.find(',')?;
    let (meta, payload) = (&rest[..comma], &rest[comma + 1..]);
    if meta.trim_end().to_ascii_lowercase().ends_with("base64") {
        return base64_decode(payload);
    }
    // Percent-decoding. `+` is NOT a space here (that is form encoding, not
    // RFC 2397) — treating it as one corrupts SVG path data.
    let b = payload.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let hex = |c: u8| -> Option<u8> {
        match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            b'A'..=b'F' => Some(c - b'A' + 10),
            _ => None,
        }
    };
    let mut i = 0;
    while i < b.len() {
        match (b[i], b.get(i + 1).copied().and_then(hex), b.get(i + 2).copied().and_then(hex)) {
            (b'%', Some(h), Some(l)) => {
                out.push(h << 4 | l);
                i += 3;
            }
            (c, _, _) => {
                out.push(c);
                i += 1;
            }
        }
    }
    Some(out)
}

fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let val = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => (c - b'A') as u32,
            b'a'..=b'z' => (c - b'a') as u32 + 26,
            b'0'..=b'9' => (c - b'0') as u32 + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        })
    };
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let (mut acc, mut bits) = (0u32, 0u32);
    for &c in s.as_bytes() {
        if c == b'=' {
            break;
        }
        let Some(v) = val(c) else {
            // Whitespace inside base64 is legal and common (wrapped lines).
            if c.is_ascii_whitespace() {
                continue;
            }
            return None;
        };
        acc = acc << 6 | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

/// Decode image bytes. Returns `None` for unsupported formats / malformed data
/// / oversize images (→ placeholder).
pub fn decode(bytes: &[u8]) -> Option<Image> {
    if bytes.len() >= 8 && bytes[0..8] == *b"\x89PNG\r\n\x1a\n" {
        decode_png(bytes)
    } else if bytes.len() >= 3 && bytes[0..3] == [0xFF, 0xD8, 0xFF] {
        decode_jpeg(bytes)
    } else if crate::ico::looks_like_ico(bytes) {
        crate::ico::decode(bytes)
    } else if crate::svg::looks_like_svg(bytes) {
        crate::svg::render(bytes)
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
    let out = dec.decode().ok()?;
    let count = w.checked_mul(h)?;
    // How many channels came back, measured rather than assumed. We ask for
    // RGB, and a YCbCr source obliges — but a SINGLE-COMPONENT (grayscale)
    // JPEG returns one byte per pixel whatever was requested, and
    // `get_output_colorspace` just echoes the request rather than reporting
    // that. The buffer length is the only truthful signal. Wikipedia serves
    // its scanned aerial photographs exactly this way; assuming 3 channels
    // threw the whole image away and left a blank figure on the page.
    let channels = out.len().checked_div(count)?;
    if !(1..=4).contains(&channels) {
        return None;
    }
    let mut bgra = zeroed(count.checked_mul(4)?)?;
    for i in 0..count {
        let s = i * channels;
        let d = i * 4;
        let (r, g, b) = match channels {
            1 => (out[s], out[s], out[s]),
            _ => (out[s], out[s + 1], out[s + 2]),
        };
        bgra[d] = b;
        bgra[d + 1] = g;
        bgra[d + 2] = r;
        bgra[d + 3] = 255;
    }
    Some(Image { bgra, w: w as u32, h: h as u32 })
}

// ── PNG (grayscale/RGB/palette/gray+alpha, bit depths 1/2/4/8/16, non-interlaced) ──

fn decode_png(data: &[u8]) -> Option<Image> {
    let mut pos = 8;
    let (mut width, mut height): (u32, u32) = (0, 0);
    let mut bit_depth: u8 = 0;
    let mut color_type: u8 = 0;
    let mut palette: Vec<u8> = Vec::new(); // PLTE — RGB triples
    let mut trns: Vec<u8> = Vec::new(); // tRNS — type3: per-index α; type0/2: colour key
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
                bit_depth = d[8];
                color_type = d[9];
                if d[10] != 0 || d[11] != 0 || d[12] != 0 {
                    return None; // compression / filter / interlace (Adam7 unsupported)
                }
                // Valid (colour-type, bit-depth) combinations per the PNG spec.
                let ok = match color_type {
                    0 => matches!(bit_depth, 1 | 2 | 4 | 8 | 16), // grayscale
                    3 => matches!(bit_depth, 1 | 2 | 4 | 8),      // palette
                    2 | 4 | 6 => matches!(bit_depth, 8 | 16),     // RGB / gray+α / RGBA
                    _ => false,
                };
                if !ok {
                    return None;
                }
                // Early dimension guard: reject an oversize image at IHDR (the
                // FIRST chunk) so we never accumulate its IDAT / allocate raw +
                // bgra — the OOM spike is bounded before it starts.
                if (width as usize).saturating_mul(height as usize) > MAX_PIXELS {
                    return None;
                }
            }
            b"PLTE" => {
                palette.clear();
                palette.extend_from_slice(&data[start..end]);
            }
            b"tRNS" => {
                trns.clear();
                trns.extend_from_slice(&data[start..end]);
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

    let channels: usize = match color_type {
        0 | 3 => 1,
        4 => 2,
        2 => 3,
        6 => 4,
        _ => return None,
    };
    let bits_per_pixel = channels * bit_depth as usize;
    let stride = (width as usize * bits_per_pixel + 7) / 8; // bytes per row (may be sub-byte packed)
    let bpp = ((bits_per_pixel + 7) / 8).max(1); // filter byte-distance (spec: rounded up, ≥1)

    let decomp = miniz_oxide::inflate::decompress_to_vec_zlib(&idat)
        .ok()
        .or_else(|| miniz_oxide::inflate::decompress_to_vec(&idat[2..]).ok())?;
    if decomp.len() < height as usize * (1 + stride) {
        return None;
    }

    // Reverse the per-row PNG filters into raw (still-packed) sample bytes.
    let mut raw = zeroed(height as usize * stride)?;
    for y in 0..height as usize {
        let src = y * (1 + stride);
        let filter = decomp[src];
        let row = src + 1;
        let dst = y * stride;
        for x in 0..stride {
            let cur = decomp[row + x];
            let a = if x >= bpp { raw[dst + x - bpp] } else { 0 };
            let b = if y > 0 { raw[dst - stride + x] } else { 0 };
            let c = if x >= bpp && y > 0 { raw[dst - stride + x - bpp] } else { 0 };
            raw[dst + x] = match filter {
                1 => cur.wrapping_add(a),
                2 => cur.wrapping_add(b),
                3 => cur.wrapping_add(((a as u16 + b as u16) / 2) as u8),
                4 => cur.wrapping_add(paeth(a, b, c)),
                _ => cur,
            };
        }
    }

    // Expand packed samples → straight BGRA, applying palette/grayscale + tRNS.
    let count = (width * height) as usize;
    let mut bgra = zeroed(count * 4)?;
    for y in 0..height as usize {
        let rs = y * stride;
        for x in 0..width as usize {
            let d = (y * width as usize + x) * 4;
            let (r, g, b, al): (u8, u8, u8, u8) = match color_type {
                0 => {
                    let v = sample(&raw, rs, x, 0, channels, bit_depth);
                    let g8 = scale_to_8(v, bit_depth);
                    (g8, g8, g8, gray_key_alpha(&trns, v))
                }
                4 => {
                    let v = sample(&raw, rs, x, 0, channels, bit_depth);
                    let a = sample(&raw, rs, x, 1, channels, bit_depth);
                    let g8 = scale_to_8(v, bit_depth);
                    (g8, g8, g8, scale_to_8(a, bit_depth))
                }
                2 => {
                    let rv = sample(&raw, rs, x, 0, channels, bit_depth);
                    let gv = sample(&raw, rs, x, 1, channels, bit_depth);
                    let bv = sample(&raw, rs, x, 2, channels, bit_depth);
                    let a = rgb_key_alpha(&trns, rv, gv, bv);
                    (scale_to_8(rv, bit_depth), scale_to_8(gv, bit_depth), scale_to_8(bv, bit_depth), a)
                }
                6 => {
                    let rv = sample(&raw, rs, x, 0, channels, bit_depth);
                    let gv = sample(&raw, rs, x, 1, channels, bit_depth);
                    let bv = sample(&raw, rs, x, 2, channels, bit_depth);
                    let av = sample(&raw, rs, x, 3, channels, bit_depth);
                    (scale_to_8(rv, bit_depth), scale_to_8(gv, bit_depth), scale_to_8(bv, bit_depth), scale_to_8(av, bit_depth))
                }
                3 => {
                    let idx = sample(&raw, rs, x, 0, channels, bit_depth) as usize;
                    let pi = idx * 3;
                    if pi + 2 >= palette.len() {
                        (0, 0, 0, 0)
                    } else {
                        let a = trns.get(idx).copied().unwrap_or(255);
                        (palette[pi], palette[pi + 1], palette[pi + 2], a)
                    }
                }
                _ => (0, 0, 0, 255),
            };
            bgra[d] = b;
            bgra[d + 1] = g;
            bgra[d + 2] = r;
            bgra[d + 3] = al;
        }
    }
    Some(Image { bgra, w: width, h: height })
}

/// Read channel `c` of pixel `x` from a filtered row as a `bit_depth`-wide
/// sample. Handles the packed sub-byte depths (1/2/4, MSB-first) and 8/16-bit.
fn sample(raw: &[u8], row_start: usize, x: usize, c: usize, channels: usize, bit_depth: u8) -> u16 {
    let si = x * channels + c;
    match bit_depth {
        16 => {
            let i = row_start + si * 2;
            ((raw[i] as u16) << 8) | raw[i + 1] as u16
        }
        8 => raw[row_start + si] as u16,
        _ => {
            let bits = bit_depth as usize;
            let bitoff = si * bits;
            let byte = raw[row_start + bitoff / 8];
            let shift = 8 - bits - (bitoff % 8);
            (byte >> shift) as u16 & ((1u16 << bits) - 1)
        }
    }
}

/// Scale a `bit_depth`-wide sample up to 8 bits (16-bit → high byte).
fn scale_to_8(v: u16, bit_depth: u8) -> u8 {
    match bit_depth {
        16 => (v >> 8) as u8,
        8 => v as u8,
        4 => (v * 17) as u8,
        2 => (v * 85) as u8,
        1 => {
            if v != 0 {
                255
            } else {
                0
            }
        }
        _ => v as u8,
    }
}

/// tRNS for grayscale (type 0): a single transparent sample value (16-bit BE).
fn gray_key_alpha(trns: &[u8], v: u16) -> u8 {
    if trns.len() >= 2 {
        let key = ((trns[0] as u16) << 8) | trns[1] as u16;
        if key == v {
            return 0;
        }
    }
    255
}

/// tRNS for truecolour (type 2): a single transparent R,G,B triple (16-bit BE each).
fn rgb_key_alpha(trns: &[u8], r: u16, g: u16, b: u16) -> u8 {
    if trns.len() >= 6 {
        let kr = ((trns[0] as u16) << 8) | trns[1] as u16;
        let kg = ((trns[2] as u16) << 8) | trns[3] as u16;
        let kb = ((trns[4] as u16) << 8) | trns[5] as u16;
        if kr == r && kg == g && kb == b {
            return 0;
        }
    }
    255
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

/// Decoded-BGRA budget for CSS images. Smaller than the `<img>` one on
/// purpose: these are icons and tiles, and a page's whole icon set is a few
/// hundred KB — a `background-image` big enough to blow this is a page doing
/// something we would rather drop than pay for.
pub(crate) const CSS_BUDGET: usize = 8 * 1024 * 1024;

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
    fn decodes_a_4bit_palette_png_with_trns() {
        // 2 packed 4-bit indices (0,1) → one byte 0x01, preceded by filter 0.
        let idat = miniz_oxide::deflate::compress_to_vec_zlib(&[0u8, 0x01], 6);
        let mut png: Vec<u8> = alloc::vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&2u32.to_be_bytes());
        ihdr.extend_from_slice(&1u32.to_be_bytes());
        ihdr.extend_from_slice(&[4, 3, 0, 0, 0]); // 4-bit, palette
        chunk(&mut png, b"IHDR", &ihdr);
        chunk(&mut png, b"PLTE", &[255, 0, 0, 0, 255, 0, 0, 0, 255]); // red, green, blue
        chunk(&mut png, b"tRNS", &[128]); // index 0 → α=128; others default 255
        chunk(&mut png, b"IDAT", &idat);
        chunk(&mut png, b"IEND", &[]);

        let img = decode(&png).expect("palette PNG decodes");
        assert_eq!((img.w, img.h), (2, 1));
        assert_eq!(&img.bgra[0..4], &[0, 0, 255, 128]); // index0 red, α from tRNS
        assert_eq!(&img.bgra[4..8], &[0, 255, 0, 255]); // index1 green, opaque
    }

    #[test]
    fn decodes_a_2bit_grayscale_png() {
        // 4 packed 2-bit samples 0,1,2,3 → 0b00_01_10_11 = 0x1B, filter 0.
        let idat = miniz_oxide::deflate::compress_to_vec_zlib(&[0u8, 0x1B], 6);
        let mut png: Vec<u8> = alloc::vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&4u32.to_be_bytes());
        ihdr.extend_from_slice(&1u32.to_be_bytes());
        ihdr.extend_from_slice(&[2, 0, 0, 0, 0]); // 2-bit, grayscale
        chunk(&mut png, b"IHDR", &ihdr);
        chunk(&mut png, b"IDAT", &idat);
        chunk(&mut png, b"IEND", &[]);

        let img = decode(&png).expect("grayscale PNG decodes");
        assert_eq!((img.w, img.h), (4, 1));
        // samples 0/1/2/3 scale ×85 → 0/85/170/255, painted as opaque gray.
        assert_eq!(&img.bgra[0..4], &[0, 0, 0, 255]);
        assert_eq!(&img.bgra[4..8], &[85, 85, 85, 255]);
        assert_eq!(&img.bgra[8..12], &[170, 170, 170, 255]);
        assert_eq!(&img.bgra[12..16], &[255, 255, 255, 255]);
    }

    #[test]
    fn non_png_bytes_decode_to_none() {
        assert!(decode(b"\xff\xd8\xff\xe0 not a png").is_none());
    }
}
