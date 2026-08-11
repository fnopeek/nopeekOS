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

use crate::image::Image;

/// An ICO (type 1) or CUR (type 2) directory header.
pub fn looks_like_ico(b: &[u8]) -> bool {
    b.len() >= 6 && b[0] == 0 && b[1] == 0 && (b[2] == 1 || b[2] == 2) && b[3] == 0
}

fn u16le(b: &[u8], i: usize) -> u32 {
    u16::from_le_bytes([b[i], b[i + 1]]) as u32
}
fn u32le(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

/// Decode the best entry of an ICO/CUR.
pub fn decode(b: &[u8]) -> Option<Image> {
    if !looks_like_ico(b) {
        return None;
    }
    let count = u16le(b, 4) as usize;
    if count == 0 {
        return None;
    }
    // Pick the largest entry, tie-broken by colour depth: a favicon is scaled
    // into a ~16 px box, and downscaling the richest source beats upscaling a
    // 16×16 one. `0` in the size byte means 256 (it does not fit in a byte).
    let mut best: Option<(u32, u32, usize, usize)> = None; // (area, bpp, off, len)
    for i in 0..count {
        let e = 6 + i * 16;
        if e + 16 > b.len() {
            break;
        }
        let w = if b[e] == 0 { 256 } else { b[e] as u32 };
        let h = if b[e + 1] == 0 { 256 } else { b[e + 1] as u32 };
        let bpp = u16le(b, e + 6);
        let len = u32le(b, e + 8) as usize;
        let off = u32le(b, e + 12) as usize;
        if off.checked_add(len)? > b.len() {
            continue;
        }
        let area = w * h;
        if best.is_none_or(|(ba, bb, _, _)| area > ba || (area == ba && bpp > bb)) {
            best = Some((area, bpp, off, len));
        }
    }
    let (_, _, off, len) = best?;
    let payload = b.get(off..off + len)?;
    // Vista+ entries are a whole PNG file. The directory's own width/height are
    // advisory there; the PNG header is authoritative.
    if payload.len() >= 8 && payload[0..8] == *b"\x89PNG\r\n\x1a\n" {
        return crate::image::decode(payload);
    }
    decode_dib(payload)
}

/// A BITMAPINFOHEADER bitmap plus its trailing AND mask.
fn decode_dib(d: &[u8]) -> Option<Image> {
    if d.len() < 40 {
        return None;
    }
    let header = u32le(d, 0) as usize;
    if header < 40 || header > d.len() {
        return None;
    }
    let w = u32le(d, 4);
    // The stored height covers colour bitmap AND mask, so the image is half it.
    let h2 = u32le(d, 8);
    let bpp = u16le(d, 14);
    let compression = u32le(d, 16);
    // 0 = BI_RGB. 3 = BI_BITFIELDS, which for an icon is always the plain
    // 8-8-8-8 layout we already read; anything else (RLE, JPEG, PNG) is not
    // something an icon uses.
    if compression != 0 && compression != 3 {
        return None;
    }
    let h = h2 / 2;
    if w == 0 || h == 0 || (w as usize).checked_mul(h as usize)? > MAX_ICON_PIXELS {
        return None;
    }

    // Palette: present for every indexed depth. `biClrUsed` may be 0, meaning
    // the full 2^bpp table.
    let pal_entries = match bpp {
        1 | 2 | 4 | 8 => {
            let used = u32le(d, 32) as usize;
            if used == 0 { 1usize << bpp } else { used }
        }
        _ => 0,
    };
    let pal_off = header;
    let pal_len = pal_entries * 4;
    let bits_off = pal_off.checked_add(pal_len)?;
    let palette = d.get(pal_off..bits_off)?;

    // Rows are padded to 4 bytes, bottom-up.
    let row_bytes = ((w as usize * bpp as usize + 31) / 32) * 4;
    let xor_len = row_bytes.checked_mul(h as usize)?;
    let xor = d.get(bits_off..bits_off + xor_len)?;
    // The AND mask is 1 bpp with its own padding. Truncated/absent → opaque.
    let mask_row = ((w as usize + 31) / 32) * 4;
    let mask = d.get(bits_off + xor_len..bits_off + xor_len + mask_row * h as usize);

    let mut bgra = crate::image::zeroed(w as usize * h as usize * 4)?;
    let mut any_alpha = false;
    for y in 0..h as usize {
        // Bottom-up: the first stored row is the last visual row.
        let src = &xor[(h as usize - 1 - y) * row_bytes..];
        for x in 0..w as usize {
            let (b_, g, r, a) = match bpp {
                32 => {
                    let p = x * 4;
                    let a = *src.get(p + 3)?;
                    any_alpha |= a != 0;
                    (*src.get(p)?, *src.get(p + 1)?, *src.get(p + 2)?, a)
                }
                24 => {
                    let p = x * 3;
                    (*src.get(p)?, *src.get(p + 1)?, *src.get(p + 2)?, 255)
                }
                16 => {
                    // 5-5-5 with the top bit unused, expanded so 0x1F → 0xFF.
                    let p = x * 2;
                    let v = u16::from_le_bytes([*src.get(p)?, *src.get(p + 1)?]);
                    let x5 = |c: u16| ((c * 255 + 15) / 31) as u8;
                    (x5(v & 0x1F), x5((v >> 5) & 0x1F), x5((v >> 10) & 0x1F), 255)
                }
                1 | 2 | 4 | 8 => {
                    let bit = x * bpp as usize;
                    let byte = *src.get(bit / 8)?;
                    let shift = 8 - bpp as usize - (bit % 8);
                    let idx = ((byte >> shift) & ((1u16 << bpp) - 1) as u8) as usize;
                    let e = idx * 4;
                    (
                        *palette.get(e)?,
                        *palette.get(e + 1)?,
                        *palette.get(e + 2)?,
                        255,
                    )
                }
                _ => return None,
            };
            let o = (y * w as usize + x) * 4;
            bgra[o] = b_;
            bgra[o + 1] = g;
            bgra[o + 2] = r;
            bgra[o + 3] = a;
        }
    }

    // The AND mask decides transparency for every depth below 32 — and for a
    // 32-bit icon whose alpha channel is entirely zero, which would otherwise
    // decode to a fully invisible image.
    if bpp != 32 || !any_alpha {
        if let Some(mask) = mask {
            for y in 0..h as usize {
                let row = &mask[(h as usize - 1 - y) * mask_row..];
                for x in 0..w as usize {
                    let bit = (row[x / 8] >> (7 - (x % 8))) & 1;
                    // 1 = "leave the background" = transparent.
                    bgra[(y * w as usize + x) * 4 + 3] = if bit == 1 { 0 } else { 255 };
                }
            }
        } else if bpp == 32 {
            // No mask and no alpha at all: the icon is opaque, not invisible.
            for p in bgra.chunks_exact_mut(4) {
                p[3] = 255;
            }
        }
    }
    Some(Image { bgra, w, h })
}

/// Icons are small by definition; this only stops a malformed header from
/// asking for a gigabyte.
const MAX_ICON_PIXELS: usize = 512 * 512;

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    /// Build a one-entry ICO around a raw DIB payload.
    fn wrap(dib: Vec<u8>, w: u8, h: u8, bpp: u16) -> Vec<u8> {
        let mut v = vec![0, 0, 1, 0, 1, 0];
        v.extend_from_slice(&[w, h, 0, 0]);
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&bpp.to_le_bytes());
        v.extend_from_slice(&(dib.len() as u32).to_le_bytes());
        v.extend_from_slice(&22u32.to_le_bytes());
        v.extend_from_slice(&dib);
        v
    }

    fn header(w: i32, h_doubled: i32, bpp: u16, clr_used: u32) -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(&40u32.to_le_bytes());
        d.extend_from_slice(&w.to_le_bytes());
        d.extend_from_slice(&h_doubled.to_le_bytes());
        d.extend_from_slice(&1u16.to_le_bytes()); // planes
        d.extend_from_slice(&bpp.to_le_bytes());
        d.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
        d.extend_from_slice(&0u32.to_le_bytes()); // size
        d.extend_from_slice(&0i32.to_le_bytes());
        d.extend_from_slice(&0i32.to_le_bytes());
        d.extend_from_slice(&clr_used.to_le_bytes());
        d.extend_from_slice(&0u32.to_le_bytes());
        d
    }

    #[test]
    fn a_32bpp_icon_keeps_its_alpha() {
        // 2×1, BGRA: opaque blue, then a fully transparent pixel.
        let mut d = header(2, 2, 32, 0);
        d.extend_from_slice(&[255, 0, 0, 255, 0, 0, 0, 0]); // one row, padded already
        d.extend_from_slice(&[0, 0, 0, 0]); // AND mask row (4-byte padded)
        let img = decode(&wrap(d, 2, 1, 32)).expect("decode");
        assert_eq!((img.w, img.h), (2, 1));
        assert_eq!(&img.bgra[0..4], &[255, 0, 0, 255]);
        assert_eq!(img.bgra[7], 0, "the transparent pixel stays transparent");
    }

    #[test]
    fn a_4bpp_palette_icon_reads_its_table_and_mask() {
        // Palette entry 1 = red; the AND mask hides the second pixel.
        let mut d = header(2, 2, 4, 2);
        d.extend_from_slice(&[0, 0, 0, 0]); // index 0 — black
        d.extend_from_slice(&[0, 0, 255, 0]); // index 1 — red (BGRx)
        d.extend_from_slice(&[0x11, 0, 0, 0]); // both pixels index 1, row padded
        d.extend_from_slice(&[0b0100_0000, 0, 0, 0]); // mask: 2nd pixel transparent
        let img = decode(&wrap(d, 2, 1, 4)).expect("decode");
        assert_eq!(&img.bgra[0..4], &[0, 0, 255, 255], "opaque red");
        assert_eq!(img.bgra[7], 0, "masked pixel is transparent");
    }

    #[test]
    fn an_all_zero_alpha_32bpp_icon_is_opaque_not_invisible() {
        // Plenty of real favicons ship 32bpp with an unset alpha channel and
        // rely on the AND mask. Trusting the channel renders nothing at all.
        let mut d = header(1, 2, 32, 0);
        d.extend_from_slice(&[10, 20, 30, 0]);
        d.extend_from_slice(&[0, 0, 0, 0]); // mask: visible
        let img = decode(&wrap(d, 1, 1, 32)).expect("decode");
        assert_eq!(img.bgra[3], 255);
    }

    #[test]
    fn the_largest_entry_wins() {
        // Two entries; the 2×1 one must be chosen over the 1×1.
        let small = {
            let mut d = header(1, 2, 32, 0);
            d.extend_from_slice(&[1, 1, 1, 255]);
            d.extend_from_slice(&[0, 0, 0, 0]);
            d
        };
        let big = {
            let mut d = header(2, 2, 32, 0);
            d.extend_from_slice(&[2, 2, 2, 255, 2, 2, 2, 255]);
            d.extend_from_slice(&[0, 0, 0, 0]);
            d
        };
        let mut v = vec![0, 0, 1, 0, 2, 0];
        let dir = 6 + 32;
        for (i, (w, dib)) in [(1u8, &small), (2u8, &big)].iter().enumerate() {
            v.extend_from_slice(&[*w, 1, 0, 0]);
            v.extend_from_slice(&1u16.to_le_bytes());
            v.extend_from_slice(&32u16.to_le_bytes());
            v.extend_from_slice(&(dib.len() as u32).to_le_bytes());
            let off = dir + if i == 0 { 0 } else { small.len() };
            v.extend_from_slice(&(off as u32).to_le_bytes());
        }
        v.extend_from_slice(&small);
        v.extend_from_slice(&big);
        let img = decode(&v).expect("decode");
        assert_eq!((img.w, img.h), (2, 1));
        assert_eq!(img.bgra[0], 2, "the big entry's pixels");
    }

    #[test]
    fn a_truncated_directory_is_rejected_not_panicked() {
        assert!(decode(&[0, 0, 1, 0, 5, 0]).is_none());
        assert!(decode(&[0, 0, 1, 0]).is_none());
        assert!(decode(b"not an icon").is_none());
    }
}
