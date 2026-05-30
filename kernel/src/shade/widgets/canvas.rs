//! Canvas bitmap store (P10.10 escape hatch).
//!
//! Apps with the CANVAS capability upload a raw BGRA32 bitmap via
//! `npk_canvas_commit`; it is stored here keyed by `(window_id,
//! canvas_id)`. The render walker's `Widget::Canvas` arm looks it up and
//! blits it (contain-fit) into the canvas rect. Decoupled from the
//! widget tree so a re-render (resize / theme) keeps showing the last
//! committed pixels without the app re-uploading.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use spin::Mutex;

/// Per-app pixel caps (mirror the P10.10 spec): 4096×4096, 64 MB total.
pub const MAX_DIM: u32 = 4096;
pub const MAX_BYTES: usize = 64 * 1024 * 1024;

struct Bitmap {
    w: u32,
    h: u32,
    /// BGRA32, `w * h * 4` bytes.
    px: Vec<u8>,
}

static CANVASES: Mutex<BTreeMap<(u32, u32), Bitmap>> = Mutex::new(BTreeMap::new());

/// Store (or replace) the bitmap for `(window_id, canvas_id)`. `px` must
/// be `w * h * 4` BGRA bytes; rejected (returns false) if oversized or
/// the length doesn't match.
pub fn commit(window_id: u32, canvas_id: u32, w: u32, h: u32, px: Vec<u8>) -> bool {
    if w == 0 || h == 0 || w > MAX_DIM || h > MAX_DIM { return false; }
    let need = (w as usize) * (h as usize) * 4;
    if need > MAX_BYTES || px.len() != need { return false; }
    CANVASES.lock().insert((window_id, canvas_id), Bitmap { w, h, px });
    true
}

/// Run `f` with the stored bitmap (pixels, w, h) if present. The lock is
/// held for the duration, so `f` must not re-enter this module.
pub fn with_bitmap<R>(window_id: u32, canvas_id: u32, f: impl FnOnce(&[u8], u32, u32) -> R) -> Option<R> {
    let guard = CANVASES.lock();
    guard.get(&(window_id, canvas_id)).map(|b| f(&b.px, b.w, b.h))
}

/// Drop every bitmap owned by a window — called from `remove_scene`
/// when a widget window closes.
pub fn remove_window(window_id: u32) {
    CANVASES.lock().retain(|&(wid, _), _| wid != window_id);
}
