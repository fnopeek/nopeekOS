//! WOFF 1.0 → sfnt.
//!
//! Still in use: sites ship their house fonts as `.woff` only, and without
//! them every width and height on the page is computed with the fallback
//! font.
//!
//! Short because WOFF1, unlike WOFF2, needs no `glyf`/`loca` reconstruction
//! and no Brotli: it is the sfnt itself, each table zlib-compressed
//! (RFC 1950, via `miniz_oxide`). Read header, read directory, inflate,
//! reassemble the sfnt.
//!
//! W3C WOFF File Format 1.0, §3 (header) and §4 (table directory).

use alloc::vec;
use alloc::vec::Vec;

/// Same cap as in `woff2`: a font larger than this when unpacked is
/// rejected rather than filling memory.
const MAX_SFNT: usize = 32 * 1024 * 1024;

/// Header (44 B) + 20 B per directory entry.
const HDR: usize = 44;
const DIR_ENTRY: usize = 20;

fn u16at(d: &[u8], p: usize) -> Option<u16> {
    Some(u16::from_be_bytes(d.get(p..p + 2)?.try_into().ok()?))
}
fn u32at(d: &[u8], p: usize) -> Option<u32> {
    Some(u32::from_be_bytes(d.get(p..p + 4)?.try_into().ok()?))
}

pub fn looks_like_woff(b: &[u8]) -> bool {
    b.starts_with(b"wOFF")
}

/// `wOFF` → sfnt, or `None` if the container does not hold what its header
/// claims.
pub fn to_sfnt(d: &[u8]) -> Option<Vec<u8>> {
    if !looks_like_woff(d) {
        return None;
    }
    let flavor = u32at(d, 4)?;
    let num = u16at(d, 12)? as usize;
    let total = u32at(d, 16)? as usize;
    if num == 0 || total > MAX_SFNT {
        return None;
    }
    // `totalSfntSize` is a claim of the file, not a measurement: used once to
    // reserve, then not trusted. The size that counts is the sum of the tables.
    let mut tables: Vec<(u32, Vec<u8>)> = Vec::new();
    let mut sum = 0usize;
    for i in 0..num {
        let p = HDR + i * DIR_ENTRY;
        let tag = u32at(d, p)?;
        let off = u32at(d, p + 4)? as usize;
        let comp = u32at(d, p + 8)? as usize;
        let orig = u32at(d, p + 12)? as usize;
        let end = off.checked_add(comp)?;
        if end > d.len() || orig > MAX_SFNT {
            return None;
        }
        sum = sum.checked_add((orig + 3) & !3)?;
        if sum > MAX_SFNT {
            return None;
        }
        let src = &d[off..end];
        // §4: `compLength == origLength` means stored uncompressed; anything else
        // is zlib. `head` is almost always stored raw.
        let raw = if comp >= orig {
            src[..orig.min(src.len())].to_vec()
        } else {
            let out = miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(src, orig).ok()?;
            if out.len() != orig {
                return None;
            }
            out
        };
        if raw.len() != orig {
            return None;
        }
        tables.push((tag, raw));
    }
    // The sfnt directory must be sorted by tag. The spec requires that of the
    // WOFF file too, but a file is no promise.
    tables.sort_by_key(|(t, _)| *t);

    let dir_len = 12 + num * 16;
    let mut out: Vec<u8> = Vec::new();
    out.try_reserve(dir_len + sum).ok()?;
    out.extend_from_slice(&flavor.to_be_bytes());
    out.extend_from_slice(&(num as u16).to_be_bytes());
    // searchRange / entrySelector / rangeShift, the sfnt header's search
    // hints. fontdue does not read them, other readers do.
    let mut sel = 0u16;
    while (1usize << (sel + 1)) <= num {
        sel += 1;
    }
    let range = (1u16 << sel) * 16;
    out.extend_from_slice(&range.to_be_bytes());
    out.extend_from_slice(&sel.to_be_bytes());
    out.extend_from_slice(&((num as u16) * 16 - range).to_be_bytes());

    let mut off = dir_len;
    for (tag, raw) in &tables {
        out.extend_from_slice(&tag.to_be_bytes());
        out.extend_from_slice(&checksum(raw).to_be_bytes());
        out.extend_from_slice(&(off as u32).to_be_bytes());
        out.extend_from_slice(&(raw.len() as u32).to_be_bytes());
        off += (raw.len() + 3) & !3;
    }
    for (_, raw) in &tables {
        out.extend_from_slice(raw);
        out.extend_from_slice(&vec![0u8; ((raw.len() + 3) & !3) - raw.len()]);
    }
    Some(out)
}

/// sfnt checksum: the table's u32s summed, zero-padded.
///
/// Recomputed rather than copied from the WOFF file, which stores the
/// original's checksum; a wrong value there would otherwise reach a reader
/// that verifies it.
fn checksum(d: &[u8]) -> u32 {
    let mut sum = 0u32;
    let mut i = 0;
    while i < d.len() {
        let mut w = [0u8; 4];
        let n = (d.len() - i).min(4);
        w[..n].copy_from_slice(&d[i..i + n]);
        sum = sum.wrapping_add(u32::from_be_bytes(w));
        i += 4;
    }
    sum
}
