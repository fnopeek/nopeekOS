//! Per-window raw-bitmap surfaces (Phase 12.4 — display bridge).
//!
//! A `Surface`-kind window's content is an opaque pixel buffer fed by
//! an external source — today a microvm's virtio-gpu framebuffer
//! (`RESOURCE_FLUSH`), later any Canvas-escape-hatch app. Shade
//! composites it as a tile like any other window (tiling invariant —
//! never fullscreen).
//!
//! Keyed by `WindowId` so the design is N-surface-shaped from day one
//! even though one VM exists now (forward-compat contract #2 —
//! consumer side never assumes count).
//!
//! Concurrency: the microvm currently runs cooperatively on Core 0
//! (`vm_poll_slice`), so the producer (virtio-gpu FLUSH) and the
//! consumer (Shade render) are both Core-0 and serialized — a single
//! buffer behind the `Mutex<BTreeMap>` is correct. The API
//! (`write_frame` / `with_front`) is already double-buffer-shaped, so
//! moving the VM to its own core later (perf track) only changes the
//! internals here, not any caller.

extern crate alloc;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use spin::Mutex;

/// One window's bitmap content. BGRX (0x00RRGGBB packed as the host
/// framebuffer expects), `width * height` pixels.
pub struct GuestSurface {
    pixels: Vec<u32>,
    pub width: u32,
    pub height: u32,
    /// Set on `write_frame`, cleared by `take_dirty`. Lets the
    /// compositor skip recompositing an unchanged surface tile.
    dirty: bool,
}

static SURFACES: Mutex<BTreeMap<u32, GuestSurface>> = Mutex::new(BTreeMap::new());

/// Copy a freshly-flushed `w*h` BGRX frame (raw bytes, 4 per pixel)
/// into the window's surface and mark it dirty. Creates/resizes the
/// surface on first frame or geometry change. Cheap no-op if `src` is
/// too small for the claimed geometry (defensive — never panics).
pub fn write_frame(window_id: u32, src: &[u8], width: u32, height: u32) {
    let px_count = (width as usize).saturating_mul(height as usize);
    if px_count == 0 || src.len() < px_count * 4 {
        return;
    }
    let mut map = SURFACES.lock();
    let surf = map.entry(window_id).or_insert_with(|| GuestSurface {
        pixels: Vec::new(),
        width,
        height,
        dirty: false,
    });
    if surf.width != width || surf.height != height || surf.pixels.len() != px_count {
        surf.width = width;
        surf.height = height;
        surf.pixels = alloc::vec![0u32; px_count];
    }
    for (dst, chunk) in surf.pixels.iter_mut().zip(src.chunks_exact(4)) {
        *dst = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
    }
    surf.dirty = true;
    drop(map);
    // A new guest frame must trigger a recomposite — otherwise the
    // tile only updates when some *other* event (a click, a key)
    // happens to call render_frame (observed: cyan appeared only
    // after clicking the window). request_render just sets an atomic;
    // run_loop's poll_render/take_deferred_render picks it up.
    crate::shade::request_render();
}

/// Read the current surface pixels for compositing. `f` gets
/// `(pixels, width, height)`. `None` if no surface for this window.
pub fn with_front<F, R>(window_id: u32, f: F) -> Option<R>
where
    F: FnOnce(&[u32], u32, u32) -> R,
{
    let map = SURFACES.lock();
    map.get(&window_id).map(|s| f(&s.pixels, s.width, s.height))
}

/// True (and clears the flag) if the surface changed since last call.
pub fn take_dirty(window_id: u32) -> bool {
    let mut map = SURFACES.lock();
    match map.get_mut(&window_id) {
        Some(s) => {
            let d = s.dirty;
            s.dirty = false;
            d
        }
        None => false,
    }
}

/// Whether a surface exists for this window.
pub fn surface_exists(window_id: u32) -> bool {
    SURFACES.lock().contains_key(&window_id)
}

/// Drop a window's surface (window closed / VM exited).
pub fn remove_surface(window_id: u32) {
    SURFACES.lock().remove(&window_id);
}
