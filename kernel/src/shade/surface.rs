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
    /// Union of the guest's damage rects (surface-local coords) since the
    /// last `take_damage`. The MMIO blit is clipped to this instead of the
    /// whole tile — at 4K that turns a tens-of-MB full-tile blit into just
    /// the changed region (e.g. a video/graph). Unions across frames that
    /// coalesced while the cursor took priority (see poll_render), so no
    /// damage is lost. `None` = nothing changed; a full-buffer (re)alloc
    /// sets it to the whole surface.
    damage: Option<(u32, u32, u32, u32)>,
    /// Desired output size = the window's content rect, written by
    /// Shade on create + every retile (`set_tile_size`). virtio-gpu
    /// `GET_DISPLAY_INFO` reports this so the guest (wlroots/cage)
    /// reflows to the tile natively — D4, no host-side scaling.
    /// 0 until Shade has placed the window.
    tile_w: u32,
    tile_h: u32,
    /// Set by `set_tile_size` when the tile size changed, taken by the
    /// VM core (`take_display_dirty`) to raise a virtio-gpu
    /// config-change IRQ so the guest re-queries GET_DISPLAY_INFO.
    /// Coalescing is free: many resizes before one take = one IRQ.
    display_dirty: bool,
}

static SURFACES: Mutex<BTreeMap<u32, GuestSurface>> = Mutex::new(BTreeMap::new());

/// Copy a freshly-flushed `w*h` BGRX frame (raw bytes, 4 per pixel)
/// into the window's surface and mark it dirty. Creates/resizes the
/// surface on first frame or geometry change. Cheap no-op if `src` is
/// too small for the claimed geometry (defensive — never panics).
pub fn write_frame(window_id: u32, src: &[u8], width: u32, height: u32, dmg: (u32, u32, u32, u32)) {
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
        damage: None,
        tile_w: 0,
        tile_h: 0,
        display_dirty: false,
    });
    let realloc = surf.width != width || surf.height != height || surf.pixels.len() != px_count;
    if realloc {
        surf.width = width;
        surf.height = height;
        surf.pixels = alloc::vec![0u32; px_count];
    }
    // The guest sends little-endian BGRX bytes, which on x86 (LE) ARE the
    // exact in-memory layout of the packed u32 the compositor reads. Bulk-
    // copy the bytes in one memcpy instead of a per-pixel `from_le_bytes`
    // scalar pass — that loop was a full extra frame-sized pass on the VM-
    // pump core every guest FLUSH (TRANSFER already copied the same bytes).
    // SAFETY: surf.pixels holds px_count u32s = px_count*4 contiguous bytes;
    // src has at least px_count*4 bytes (checked above).
    let dst = unsafe {
        core::slice::from_raw_parts_mut(surf.pixels.as_mut_ptr() as *mut u8, px_count * 4)
    };
    dst.copy_from_slice(&src[..px_count * 4]);
    surf.dirty = true;

    // Union the guest's damage rect (clamped to the surface) into the
    // pending region. A fresh buffer must blit in full — the new pixels
    // around the reported rect are otherwise undefined. Coalesced frames
    // (cursor took priority) accumulate here until the next render.
    let dmg = if realloc {
        (0, 0, width, height)
    } else {
        let x = dmg.0.min(width);
        let y = dmg.1.min(height);
        let w = dmg.2.min(width - x);
        let h = dmg.3.min(height - y);
        (x, y, w, h)
    };
    if dmg.2 > 0 && dmg.3 > 0 {
        surf.damage = Some(match surf.damage {
            None => dmg,
            Some(o) => {
                let x0 = o.0.min(dmg.0);
                let y0 = o.1.min(dmg.1);
                let x1 = (o.0 + o.2).max(dmg.0 + dmg.2);
                let y1 = (o.1 + o.3).max(dmg.1 + dmg.3);
                (x0, y0, x1 - x0, y1 - y0)
            }
        });
    }
    drop(map);
    // A new guest frame must trigger a recomposite — otherwise the
    // tile only updates when some *other* event (a click, a key)
    // happens to call render_frame (observed: cyan appeared only
    // after clicking the window). Use the clipped surface path (blit
    // only the tile rect, not the whole screen) — a 60 Hz guest doing a
    // full-screen MMIO blit every frame starved the cursor on bare metal.
    // Both just set an atomic; poll_render picks it up.
    if crate::shade::SURFACE_CLIP_BLIT {
        crate::shade::request_surface_render();
    } else {
        crate::shade::request_render();
    }
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

/// Take (and clear) the union of damage rects (surface-local coords)
/// accumulated since the last call. The compositor blits only this region
/// to MMIO instead of the whole tile. `None` if nothing changed.
pub fn take_damage(window_id: u32) -> Option<(u32, u32, u32, u32)> {
    SURFACES.lock().get_mut(&window_id).and_then(|s| s.damage.take())
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

/// Shade tells the guest how big to render: the window's content
/// rect. Called on window create + every retile. Creates the surface
/// entry if absent so the size is known before the guest's first
/// `GET_DISPLAY_INFO` (wlroots queries it before rendering a pixel).
/// On a real change, flags `display_dirty` so the VM core raises a
/// virtio-gpu config-change IRQ and asks the guest to reflow.
pub fn set_tile_size(window_id: u32, w: u32, h: u32) {
    if w == 0 || h == 0 {
        return;
    }
    let mut map = SURFACES.lock();
    let surf = map.entry(window_id).or_insert_with(|| GuestSurface {
        pixels: Vec::new(),
        width: 0,
        height: 0,
        dirty: false,
        damage: None,
        tile_w: 0,
        tile_h: 0,
        display_dirty: false,
    });
    if surf.tile_w != w || surf.tile_h != h {
        surf.tile_w = w;
        surf.tile_h = h;
        surf.display_dirty = true;
    }
}

/// The size the guest should render at (window content rect), for
/// virtio-gpu `GET_DISPLAY_INFO`. `None` if Shade hasn't placed the
/// window yet → caller falls back to a default.
pub fn tile_size(window_id: u32) -> Option<(u32, u32)> {
    let map = SURFACES.lock();
    map.get(&window_id).and_then(|s| {
        if s.tile_w != 0 && s.tile_h != 0 {
            Some((s.tile_w, s.tile_h))
        } else {
            None
        }
    })
}

/// Non-consuming peek of the display-dirty flag. The VM core checks
/// this first so it can rate-limit the config-change IRQ (R2 debounce)
/// WITHOUT clearing the flag when it decides to skip — the dirty state
/// (and the latest tile size) then survives to the next allowed
/// window, so the final resize size is always delivered.
pub fn display_dirty_peek(window_id: u32) -> bool {
    SURFACES.lock().get(&window_id).is_some_and(|s| s.display_dirty)
}

/// True (and clears the flag) if the tile size changed since the last
/// call → the VM core must raise a virtio-gpu config-change IRQ so the
/// guest re-queries `GET_DISPLAY_INFO`. Only consumed on a tick where
/// the IRQ can actually be injected, so a missed slot keeps the flag.
pub fn take_display_dirty(window_id: u32) -> bool {
    let mut map = SURFACES.lock();
    match map.get_mut(&window_id) {
        Some(s) => {
            let d = s.display_dirty;
            s.display_dirty = false;
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
