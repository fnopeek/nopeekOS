//! Phosphor icon atlas — alpha-only, loaded from npkFS at boot.
//!
//! Packed binary format produced by `tools/regen-icons`:
//!
//! ```text
//! 0x00  magic      : [u8; 8] = b"NPKIATLS"
//! 0x08  version    : u16 LE = 1 or 2
//! 0x0A  num_icons  : u16 LE
//! 0x0C  num_sizes  : u16 LE
//! 0x0E  _pad       : u16
//! 0x10  sizes      : [u16; num_sizes]
//! ....  index      : num_icons × { id: u16, _pad: u16, offsets: [u32; num_sizes] }
//! ....  data       : alpha bytes; icon at size S contributes S*S bytes.
//! ....  outlines   : version 2 only, see `tools/regen-icons`.
//! ```
//!
//! With outlines an icon is filled at exactly the size asked for
//! (`icon_raster`) and kept in a cache; the bitmaps are the fallback.
//!
//! Parsing lives here; the atlas bytes themselves arrive via
//! `npkfs::fetch("sys/icons/phosphor")`, BLAKE3-verified at a higher
//! layer if a hash is frozen for a specific release.
//!
//! `alpha(IconId, size)` returns a slice view into the loaded atlas;
//! slice lifetime is static (atlas Vec<u8> is held forever once
//! loaded — same ownership model as the Inter Variable font bytes).

#![allow(dead_code)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

use crate::shade::widgets::abi::IconId;

const FS_PATH: &str = "sys/icons/phosphor";
const MAGIC: &[u8; 8] = b"NPKIATLS";
const PATH_MAGIC: &[u8; 8] = b"NPKIPATH";
/// Alpha bytes the rasterized-size cache may hold before it starts over.
const CACHE_MAX: usize = 4 * 1024 * 1024;

/// Parsed atlas header + raw bytes, stored once at load time.
pub struct Atlas {
    bytes:      Vec<u8>,
    sizes:      Vec<u16>,
    entries:    Vec<IndexEntry>,
    /// (icon id, px) → alpha, filled from the outlines on first use.
    cache:      BTreeMap<(u16, u16), Vec<u8>>,
    cache_bytes: usize,
}

struct IndexEntry {
    id:      u16,
    offsets: Vec<u32>,   // one per size
    outline: Option<crate::gui::icon_raster::Outline>,
}

static ATLAS: Mutex<Option<Atlas>> = Mutex::new(None);
static READY: AtomicBool = AtomicBool::new(false);

pub fn is_ready() -> bool {
    READY.load(Ordering::Acquire)
}

/// Load the atlas from npkFS. Logs + returns silently on any issue —
/// icons are a nice-to-have, missing atlas means CpuRasterizer falls
/// back to the stub square. Call after npkfs::mount, after login.
pub fn init() {
    if !crate::npkfs::is_mounted() {
        crate::kprintln!("[npk] icons: npkFS not mounted, skipping atlas load");
        return;
    }

    let bytes = match crate::npkfs::fetch(FS_PATH) {
        Ok((data, _)) => data,
        Err(e) => {
            crate::kprintln!("[npk] icons: atlas not found at {} ({:?})", FS_PATH, e);
            return;
        }
    };

    let atlas = match parse(bytes) {
        Ok(a) => a,
        Err(reason) => {
            crate::kprintln!("[npk] icons: atlas parse failed — {}", reason);
            return;
        }
    };

    crate::kdebug!(
        "[npk] icons: atlas loaded — {} icons × {} sizes ({} bytes)",
        atlas.entries.len(), atlas.sizes.len(), atlas.bytes.len(),
    );

    *ATLAS.lock() = Some(atlas);
    READY.store(true, Ordering::Release);
}

/// Run `f` on the alpha bitmap for `icon` at the requested `size_px`: the
/// size actually used (nearest >= requested in the atlas) and `S*S` alpha
/// bytes, borrowed from the atlas. The atlas lock is held for the call, so
/// `f` must not draw icons itself.
///
/// None if the atlas is not loaded or the icon is not in it.
pub fn with_alpha<R>(icon: IconId, size_px: u16, f: impl FnOnce(u16, &[u8]) -> R) -> Option<R> {
    if icon == IconId::None { return None; }
    let mut guard = ATLAS.lock();
    let atlas = guard.as_mut()?;

    let i = atlas.entries.binary_search_by_key(&icon.0, |e| e.id).ok()?;
    if size_px > 0 && size_px <= MAX_FILL_PX {
        if let Some(o) = atlas.entries[i].outline.as_ref() {
            let key = (icon.0, size_px);
            if !atlas.cache.contains_key(&key) {
                let alpha = crate::gui::icon_raster::rasterize(o, size_px);
                if atlas.cache_bytes + alpha.len() > CACHE_MAX {
                    atlas.cache.clear();
                    atlas.cache_bytes = 0;
                }
                atlas.cache_bytes += alpha.len();
                atlas.cache.insert(key, alpha);
            }
            return atlas.cache.get(&key).map(|a| f(size_px, a));
        }
    }
    let entry = &atlas.entries[i];

    // Pick smallest atlas size >= requested, else largest available.
    let (idx, size) = best_size(&atlas.sizes, size_px)?;
    let offset = *entry.offsets.get(idx)? as usize;
    let len = (size as usize) * (size as usize);
    if offset.saturating_add(len) > atlas.bytes.len() { return None; }

    Some(f(size, &atlas.bytes[offset..offset + len]))
}

/// Largest size filled from the outline; above it the bitmaps are scaled.
const MAX_FILL_PX: u16 = 512;

fn best_size(sizes: &[u16], requested: u16) -> Option<(usize, u16)> {
    if sizes.is_empty() { return None; }
    let mut best_ge = None;
    let mut largest = (0usize, sizes[0]);
    for (i, &s) in sizes.iter().enumerate() {
        if s >= requested {
            match best_ge {
                None => best_ge = Some((i, s)),
                Some((_, cur)) if s < cur => best_ge = Some((i, s)),
                _ => {}
            }
        }
        if s > largest.1 { largest = (i, s); }
    }
    best_ge.or(Some(largest))
}

fn parse(bytes: Vec<u8>) -> Result<Atlas, &'static str> {
    if bytes.len() < 16 { return Err("atlas too small"); }
    if &bytes[0..8] != MAGIC { return Err("bad magic"); }
    let version = u16::from_le_bytes([bytes[8], bytes[9]]);
    if version != 1 && version != 2 { return Err("unsupported version"); }
    let num_icons = u16::from_le_bytes([bytes[10], bytes[11]]) as usize;
    let num_sizes = u16::from_le_bytes([bytes[12], bytes[13]]) as usize;

    let sizes_start = 16;
    let sizes_end = sizes_start + num_sizes * 2;
    if bytes.len() < sizes_end { return Err("truncated sizes"); }
    let mut sizes = Vec::with_capacity(num_sizes);
    for i in 0..num_sizes {
        let off = sizes_start + i * 2;
        sizes.push(u16::from_le_bytes([bytes[off], bytes[off + 1]]));
    }

    let entry_len = 4 + num_sizes * 4;
    let index_end = sizes_end + num_icons * entry_len;
    if bytes.len() < index_end { return Err("truncated index"); }

    let mut entries = Vec::with_capacity(num_icons);
    for i in 0..num_icons {
        let base = sizes_end + i * entry_len;
        let id = u16::from_le_bytes([bytes[base], bytes[base + 1]]);
        let mut offsets = Vec::with_capacity(num_sizes);
        for j in 0..num_sizes {
            let off = base + 4 + j * 4;
            offsets.push(u32::from_le_bytes([
                bytes[off], bytes[off + 1], bytes[off + 2], bytes[off + 3],
            ]));
        }
        entries.push(IndexEntry { id, offsets, outline: None });
    }

    if version == 2 {
        let data_len: usize = sizes.iter().map(|&s| s as usize * s as usize).sum::<usize>() * num_icons;
        parse_outlines(&bytes, index_end + data_len, &mut entries)?;
    }

    // Sorted by id for `with_alpha`'s binary search.
    entries.sort_unstable_by_key(|e| e.id);
    Ok(Atlas { bytes, sizes, entries, cache: BTreeMap::new(), cache_bytes: 0 })
}

/// The version-2 outline section at `at`, attached to `entries` by id.
fn parse_outlines(b: &[u8], at: usize, entries: &mut [IndexEntry]) -> Result<(), &'static str> {
    let u16_at = |o: usize| -> Result<u16, &'static str> {
        b.get(o..o + 2).map(|s| u16::from_le_bytes([s[0], s[1]])).ok_or("truncated outlines")
    };
    if b.get(at..at + 8) != Some(&PATH_MAGIC[..]) { return Err("bad outline magic"); }
    let count = u16_at(at + 8)? as usize;
    let mut p = at + 12;
    for _ in 0..count {
        let id = u16_at(p)?;
        let contours = u16_at(p + 2)? as usize;
        p += 4;
        let mut o = crate::gui::icon_raster::Outline { points: Vec::new(), ends: Vec::new() };
        for _ in 0..contours {
            let n = u16_at(p)? as usize;
            p += 2;
            for _ in 0..n {
                o.points.push((u16_at(p)? as i16, u16_at(p + 2)? as i16));
                p += 4;
            }
            o.ends.push(o.points.len() as u32);
        }
        if let Some(e) = entries.iter_mut().find(|e| e.id == id) {
            e.outline = Some(o);
        }
    }
    Ok(())
}
