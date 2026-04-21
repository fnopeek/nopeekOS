//! Phosphor icon atlas — alpha-only, loaded from npkFS at boot.
//!
//! Packed binary format produced by `tools/regen-icons`:
//!
//! ```text
//! 0x00  magic      : [u8; 8] = b"NPKIATLS"
//! 0x08  version    : u16 LE = 1
//! 0x0A  num_icons  : u16 LE
//! 0x0C  num_sizes  : u16 LE
//! 0x0E  _pad       : u16
//! 0x10  sizes      : [u16; num_sizes]
//! ....  index      : num_icons × { id: u16, _pad: u16, offsets: [u32; num_sizes] }
//! ....  data       : alpha bytes; icon at size S contributes S*S bytes.
//! ```
//!
//! Parsing lives here; the ATLAS bytes themselves arrive via
//! `npkfs::fetch("sys/icons/phosphor")`, BLAKE3-verified at a higher
//! layer if a hash is frozen for a specific release.
//!
//! `alpha(IconId, size)` returns a slice view into the loaded atlas;
//! slice lifetime is static (atlas Vec<u8> is held forever once
//! loaded — same ownership model as the Inter Variable font bytes).

#![allow(dead_code)]

extern crate alloc;

use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

use crate::shade::widgets::abi::IconId;

const FS_PATH: &str = "sys/icons/phosphor";
const MAGIC: &[u8; 8] = b"NPKIATLS";
const SUPPORTED_VERSION: u16 = 1;

/// Parsed atlas header + raw bytes. Stored once at load time; we keep
/// the owned `Vec<u8>` to keep returned slices valid forever.
pub struct Atlas {
    bytes:      Vec<u8>,
    sizes:      Vec<u16>,
    entries:    Vec<IndexEntry>,
}

struct IndexEntry {
    id:      u16,
    offsets: Vec<u32>,   // one per size
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

    crate::kprintln!(
        "[npk] icons: atlas loaded — {} icons × {} sizes ({} bytes)",
        atlas.entries.len(), atlas.sizes.len(), atlas.bytes.len(),
    );

    *ATLAS.lock() = Some(atlas);
    READY.store(true, Ordering::Release);
}

/// Look up the alpha bitmap for `icon` at the requested `size_px`.
/// Returns the size that was actually used (nearest >= requested in
/// the atlas) plus the packed alpha byte slice (`S*S` bytes).
///
/// Returns None if atlas not loaded, icon not present, or requested
/// size unreasonable.
pub fn alpha_for(icon: IconId, size_px: u16) -> Option<(u16, Vec<u8>)> {
    if icon as u16 == 0 { return None; }
    let guard = ATLAS.lock();
    let atlas = guard.as_ref()?;

    // Find entry by id.
    let entry = atlas.entries.iter().find(|e| e.id == icon as u16)?;

    // Pick smallest atlas size >= requested, else largest available.
    let (idx, size) = best_size(&atlas.sizes, size_px)?;
    let offset = *entry.offsets.get(idx)? as usize;
    let len = (size as usize) * (size as usize);
    if offset.saturating_add(len) > atlas.bytes.len() { return None; }

    Some((size, atlas.bytes[offset..offset + len].to_vec()))
}

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
    if version != SUPPORTED_VERSION { return Err("unsupported version"); }
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
        entries.push(IndexEntry { id, offsets });
    }

    Ok(Atlas { bytes, sizes, entries })
}
