//! PNG decoder: 8-bit RGB and RGBA, not interlaced, to BGRA32.
//!
//! The format that iris, wallpaper and loft's thumbnails read. Behind the
//! `png` feature because it pulls in `miniz_oxide`.
//!
//! Width and height come from the file, so they are checked before any
//! buffer is sized: on wasm32 their product would wrap.

use alloc::vec;
use alloc::vec::Vec;

use miniz_oxide::inflate::TINFLStatus;
use miniz_oxide::inflate::core::{DecompressorOxide, decompress, inflate_flags};

/// Largest image decoded: per side, and in pixels (an 8K frame).
pub const MAX_SIDE: u32 = 16384;
pub const MAX_PIXELS: u64 = 32 * 1024 * 1024;

const SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";

/// A decoded picture, BGRA32, `width * height * 4` bytes.
pub struct Image {
    pub bgra:   Vec<u8>,
    pub width:  u32,
    pub height: u32,
}

/// Width and height from the IHDR, without decoding. `None` if `data` is
/// not a PNG this module can read.
pub fn dimensions(data: &[u8]) -> Option<(u32, u32)> {
    let h = header(data)?;
    Some((h.width, h.height))
}

/// Decode the whole picture.
pub fn decode(data: &[u8]) -> Option<Image> {
    decode_with_progress(data, |_, _, _| {})
}

/// Decode, handing `on_rows` the (still incomplete) BGRA buffer each time
/// another band of scanlines is finished.
///
/// Bands and not a low-resolution preview: a PNG holds no smaller version
/// of itself, and its pixels cannot be sampled, because the file is one
/// deflate stream and every scanline's filter refers to the one above.
/// What the format does give is order: the top of the picture is ready
/// while the bottom is still compressed.
pub fn decode_with_progress<F>(data: &[u8], mut on_rows: F) -> Option<Image>
where
    F: FnMut(&[u8], u32, u32),
{
    let h = header(data)?;
    let idat = idat(data);
    if idat.len() < 6 { return None; }
    let (width, height, channels) = (h.width, h.height, h.channels);
    let stride = width as usize * channels;
    let row_bytes = 1 + stride;
    let rows = height as usize;

    let mut decompressed = vec![0u8; rows * row_bytes];
    let mut unfiltered = vec![0u8; rows * stride];
    // No alpha pre-fill: the conversion writes all four bytes of a pixel.
    let mut bgra = vec![0u8; width as usize * rows * 4];

    let mut state = DecompressorOxide::new();
    let (mut in_pos, mut out_pos, mut rows_done) = (0usize, 0usize, 0usize);
    // Twelve bands: fine enough for early progress, coarse enough that the
    // per-band overhead stays negligible.
    let chunk = (idat.len() / 12).max(64 * 1024);
    loop {
        let end = (in_pos + chunk).min(idat.len());
        let last_slice = end >= idat.len();
        let mut flags = inflate_flags::TINFL_FLAG_PARSE_ZLIB_HEADER
            | inflate_flags::TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF;
        if !last_slice { flags |= inflate_flags::TINFL_FLAG_HAS_MORE_INPUT; }
        let (status, used_in, used_out) =
            decompress(&mut state, &idat[in_pos..end], &mut decompressed, out_pos, flags);
        in_pos += used_in;
        out_pos += used_out;

        let ready = (out_pos / row_bytes).min(rows);
        if ready > rows_done {
            unfilter_rows(&decompressed, &mut unfiltered, rows_done, ready, stride, channels);
            rows_to_bgra(&unfiltered, &mut bgra, rows_done, ready, width as usize, channels);
            rows_done = ready;
            on_rows(&bgra, width, height);
        }
        match status {
            TINFLStatus::Done => break,
            TINFLStatus::NeedsMoreInput | TINFLStatus::HasMoreOutput => {
                if last_slice && used_in == 0 && used_out == 0 { break; }
            }
            _ => return None,
        }
        if rows_done >= rows { break; }
    }
    if rows_done < rows { return None; }
    Some(Image { bgra, width, height })
}

/// A copy of `img` that fits in `max` × `max`, aspect kept, each target
/// pixel the average of the source pixels it covers. A picture that fits
/// already is returned as is.
pub fn fit(img: Image, max: u32) -> Image {
    let max = max.max(1);
    if img.width <= max && img.height <= max { return img; }
    let (w, h) = if img.width >= img.height {
        (max, ((img.height as u64 * max as u64) / img.width as u64).max(1) as u32)
    } else {
        (((img.width as u64 * max as u64) / img.height as u64).max(1) as u32, max)
    };
    let mut out = vec![0u8; w as usize * h as usize * 4];
    let (sw, sh) = (img.width as usize, img.height as usize);
    for ty in 0..h as usize {
        let y0 = ty * sh / h as usize;
        let y1 = ((ty + 1) * sh / h as usize).max(y0 + 1);
        for tx in 0..w as usize {
            let x0 = tx * sw / w as usize;
            let x1 = ((tx + 1) * sw / w as usize).max(x0 + 1);
            let mut sum = [0u32; 4];
            for y in y0..y1 {
                let row = &img.bgra[(y * sw + x0) * 4..(y * sw + x1) * 4];
                for px in row.chunks_exact(4) {
                    for k in 0..4 { sum[k] += px[k] as u32; }
                }
            }
            let n = ((y1 - y0) * (x1 - x0)) as u32;
            let d = (ty * w as usize + tx) * 4;
            for k in 0..4 { out[d + k] = (sum[k] / n) as u8; }
        }
    }
    Image { bgra: out, width: w, height: h }
}

struct Header {
    width:    u32,
    height:   u32,
    channels: usize,
}

/// Chunks as (type, payload), stopping at IEND or the first chunk that
/// runs past the end of `data`.
fn chunks(data: &[u8]) -> impl Iterator<Item = (&[u8], &[u8])> {
    let mut pos = SIGNATURE.len();
    core::iter::from_fn(move || {
        if pos + 12 > data.len() { return None; }
        let len = u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]) as usize;
        let kind = &data[pos + 4..pos + 8];
        let start = pos + 8;
        // Checked: a wrapped end would send `pos` backwards, forever.
        let end = start.checked_add(len).filter(|&e| e <= data.len())?;
        if kind == b"IEND" { return None; }
        pos = end + 4;
        Some((kind, &data[start..end]))
    })
}

fn header(data: &[u8]) -> Option<Header> {
    if data.len() < SIGNATURE.len() || &data[..SIGNATURE.len()] != SIGNATURE { return None; }
    let (kind, d) = chunks(data).next()?;
    if kind != b"IHDR" || d.len() < 13 { return None; }
    let width = u32::from_be_bytes([d[0], d[1], d[2], d[3]]);
    let height = u32::from_be_bytes([d[4], d[5], d[6], d[7]]);
    let (bit_depth, color_type) = (d[8], d[9]);
    // Compression, filter method, interlace.
    if d[10] != 0 || d[11] != 0 || d[12] != 0 || bit_depth != 8 { return None; }
    let channels = match color_type { 2 => 3, 6 => 4, _ => return None };
    let size_ok = width > 0 && height > 0 && width <= MAX_SIDE && height <= MAX_SIDE
        && (width as u64) * (height as u64) <= MAX_PIXELS;
    size_ok.then_some(Header { width, height, channels })
}

fn idat(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    for (kind, payload) in chunks(data) {
        if kind == b"IDAT" { out.extend_from_slice(payload); }
    }
    out
}

/// Un-filter scanlines `[from, to)` in place. Resumable: everything it
/// needs about earlier rows is already in `unfiltered`.
fn unfilter_rows(decompressed: &[u8], unfiltered: &mut [u8],
                 from: usize, to: usize, stride: usize, channels: usize) {
    for y in from..to {
        let src_offset = y * (1 + stride);
        let filter_type = decompressed[src_offset];
        let src = &decompressed[src_offset + 1..src_offset + 1 + stride];
        let dst = y * stride;
        // The row above as an immutable slice, the current one mutable, so
        // the predictors read a plain slice.
        let (done, rest) = unfiltered.split_at_mut(dst);
        let above: Option<&[u8]> = if y == 0 { None } else { Some(&done[dst - stride..]) };
        if channels == 3 {
            unfilter_row::<3>(filter_type, &mut rest[..stride], src, above)
        } else {
            unfilter_row::<4>(filter_type, &mut rest[..stride], src, above)
        }
    }
}

/// Convert scanlines `[from, to)` to BGRA, one 32-bit store per pixel
/// (B | G<<8 | R<<16 | A<<24 little-endian).
fn rows_to_bgra(unfiltered: &[u8], bgra: &mut [u8],
                from: usize, to: usize, width: usize, channels: usize) {
    let src_stride = width * channels;
    let dst_stride = width * 4;
    for y in from..to {
        let s = &unfiltered[y * src_stride..(y + 1) * src_stride];
        let d = &mut bgra[y * dst_stride..(y + 1) * dst_stride];
        if channels == 4 {
            for (d, s) in d.chunks_exact_mut(4).zip(s.chunks_exact(4)) {
                let v = (s[2] as u32) | ((s[1] as u32) << 8) | ((s[0] as u32) << 16) | ((s[3] as u32) << 24);
                d.copy_from_slice(&v.to_le_bytes());
            }
        } else {
            for (d, s) in d.chunks_exact_mut(4).zip(s.chunks_exact(3)) {
                let v = (s[2] as u32) | ((s[1] as u32) << 8) | ((s[0] as u32) << 16) | 0xFF00_0000;
                d.copy_from_slice(&v.to_le_bytes());
            }
        }
    }
}

/// One scanline. The filter type is constant per row, so it is resolved
/// once; the left-hand predictors (`a` left, `c` above-left) stay in
/// locals and only `b` is a load. `C` is the channel count, a constant so
/// the per-channel loop unrolls. Real PNGs use Paeth and Average for
/// nearly every row: this is the hot loop of a decode.
fn unfilter_row<const C: usize>(filter: u8, cur: &mut [u8], src: &[u8], above: Option<&[u8]>) {
    match (filter, above) {
        // None, and Up on the first row, where "above" reads as zero.
        (0, _) | (2, None) => cur.copy_from_slice(src),
        // Sub. Paeth on the first row is the same: paeth(a, 0, 0) == a.
        (1, _) | (4, None) => {
            let mut a = [0u8; C];
            for (out, inp) in cur.chunks_exact_mut(C).zip(src.chunks_exact(C)) {
                for k in 0..C {
                    let v = inp[k].wrapping_add(a[k]);
                    out[k] = v;
                    a[k] = v;
                }
            }
        }
        (2, Some(up)) => {
            for ((o, s), b) in cur.iter_mut().zip(src.iter()).zip(up.iter()) {
                *o = s.wrapping_add(*b);
            }
        }
        // Average of left and above, floored.
        (3, up) => {
            let mut a = [0u8; C];
            match up {
                Some(up) => {
                    for ((out, upx), inp) in cur.chunks_exact_mut(C).zip(up.chunks_exact(C)).zip(src.chunks_exact(C)) {
                        for k in 0..C {
                            let v = inp[k].wrapping_add(((a[k] as u16 + upx[k] as u16) / 2) as u8);
                            out[k] = v;
                            a[k] = v;
                        }
                    }
                }
                None => {
                    for (out, inp) in cur.chunks_exact_mut(C).zip(src.chunks_exact(C)) {
                        for k in 0..C {
                            let v = inp[k].wrapping_add(a[k] / 2);
                            out[k] = v;
                            a[k] = v;
                        }
                    }
                }
            }
        }
        (4, Some(up)) => {
            let mut a = [0u8; C];
            let mut c = [0u8; C];
            for ((out, upx), inp) in cur.chunks_exact_mut(C).zip(up.chunks_exact(C)).zip(src.chunks_exact(C)) {
                for k in 0..C {
                    let b = upx[k];
                    let v = inp[k].wrapping_add(paeth(a[k], b, c[k]));
                    out[k] = v;
                    a[k] = v;
                    c[k] = b;
                }
            }
        }
        // An unknown filter byte: the row as is, rather than failing the image.
        _ => cur.copy_from_slice(src),
    }
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = a as i16 + b as i16 - c as i16;
    let pa = (p - a as i16).unsigned_abs();
    let pb = (p - b as i16).unsigned_abs();
    let pc = (p - c as i16).unsigned_abs();
    if pa <= pb && pa <= pc { a } else if pb <= pc { b } else { c }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], payload: &[u8]) {
        out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(payload);
        // The decoder does not check CRCs.
        out.extend_from_slice(&[0; 4]);
    }

    /// A PNG of `w`×`h` RGBA pixels, row `y` filtered with `filters[y % len]`.
    fn encode(w: u32, h: u32, rgba: &[u8], filters: &[u8]) -> Vec<u8> {
        let stride = w as usize * 4;
        let mut raw = Vec::new();
        for y in 0..h as usize {
            let f = filters[y % filters.len()];
            raw.push(f);
            let row = &rgba[y * stride..(y + 1) * stride];
            let up = if y == 0 { None } else { Some(&rgba[(y - 1) * stride..y * stride]) };
            for x in 0..stride {
                let a = if x >= 4 { row[x - 4] } else { 0 };
                let b = up.map_or(0, |u| u[x]);
                let c = if x >= 4 { up.map_or(0, |u| u[x - 4]) } else { 0 };
                let pred = match f {
                    0 => 0,
                    1 => a,
                    2 => b,
                    3 => ((a as u16 + b as u16) / 2) as u8,
                    _ => paeth(a, b, c),
                };
                raw.push(row[x].wrapping_sub(pred));
            }
        }
        let mut out = SIGNATURE.to_vec();
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&w.to_be_bytes());
        ihdr.extend_from_slice(&h.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
        chunk(&mut out, b"IHDR", &ihdr);
        chunk(&mut out, b"IDAT", &miniz_oxide::deflate::compress_to_vec_zlib(&raw, 6));
        chunk(&mut out, b"IEND", &[]);
        out
    }

    fn pattern(w: u32, h: u32) -> Vec<u8> {
        (0..w * h).flat_map(|i| {
            let (x, y) = (i % w, i / w);
            [(x * 7) as u8, (y * 13) as u8, ((x ^ y) * 3) as u8, (200 + x) as u8]
        }).collect()
    }

    #[test]
    fn every_filter_round_trips() {
        let (w, h) = (37, 29);
        let rgba = pattern(w, h);
        let img = decode(&encode(w, h, &rgba, &[0, 1, 2, 3, 4])).expect("decodes");
        assert_eq!((img.width, img.height), (w, h));
        for (i, (d, s)) in img.bgra.chunks_exact(4).zip(rgba.chunks_exact(4)).enumerate() {
            assert_eq!(d, &[s[2], s[1], s[0], s[3]], "pixel {i}");
        }
    }

    #[test]
    fn rejects_bad_input() {
        assert!(decode(b"not a png").is_none());
        let mut huge = encode(2, 2, &pattern(2, 2), &[0]);
        // Width in the IHDR: 16 + 0..4.
        huge[16..20].copy_from_slice(&(MAX_SIDE + 1).to_be_bytes());
        assert!(decode(&huge).is_none());
        let full = encode(8, 8, &pattern(8, 8), &[4]);
        assert!(decode(&full[..full.len() - 30]).is_none());
    }

    #[test]
    fn fit_keeps_aspect_and_averages() {
        let img = Image { bgra: [10, 20, 30, 255, 30, 40, 50, 255].repeat(50 * 100), width: 100, height: 100 };
        let small = fit(img, 25);
        assert_eq!((small.width, small.height), (25, 25));
        assert_eq!(&small.bgra[..4], &[20, 30, 40, 255]);
        let wide = Image { bgra: vec![0; 400 * 100 * 4], width: 400, height: 100 };
        let t = fit(wide, 128);
        assert_eq!((t.width, t.height), (128, 32));
    }
}
