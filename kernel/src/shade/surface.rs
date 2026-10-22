//! Per-window raw-bitmap surfaces (display bridge).
//!
//! A `Surface`-kind window's content is an opaque pixel buffer fed by
//! an external source — today a microvm's virtio-gpu framebuffer
//! (`RESOURCE_FLUSH`). Shade
//! composites it as a tile like any other window (tiling invariant —
//! never fullscreen).
//!
//! Keyed by `WindowId`: a VM has one surface per app window.
//!
//! A guest that hands its frames over as blobs (`present_blob`) is not
//! copied at all: the frame stays in guest RAM, the GPU composes from it
//! there (`front_in_place`), and only a CPU composite copies what it needs.
//! Before the guest may write or free such a frame again, `detach_blob`
//! turns every picture still shown from it into a copy. A frame laid out
//! for the card (`ForeignPixels::gpu_only`) is never copied: the CPU shows
//! nothing of it rather than its tiles.
//!
//! Concurrency: the producer (virtio-gpu FLUSH on a vCPU core) and the
//! consumer (Shade render on Core 0) run on different cores. Three
//! buffers per surface (back → ready → front): each side copies or blits
//! with the map lock released and takes it only to swap buffers. Holding it
//! across a multi-MB copy or blit would stall the other side for
//! milliseconds — the vCPU on its next exit, and with it every interrupt it
//! would deliver.

extern crate alloc;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use spin::Mutex;

use crate::shade::buffers::{ForeignView, PixelBuffer};
use crate::shade::scene::InPlace;

/// One window's bitmap content. BGRX (0x00RRGGBB packed as the host
/// framebuffer expects), `width * height` pixels.
pub struct GuestSurface {
    /// Newest complete frame, not yet taken by the compositor.
    ready: Frame,
    /// The compositor's frame (moved out while it blits).
    front: Frame,
    /// The producer's scratch buffer (moved out while it copies).
    back: Frame,
    /// `ready` holds a frame newer than `front`.
    fresh: bool,
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
    /// `GET_DISPLAY_INFO` reports this for a new output, and a change goes
    /// to the guest compositor as `WIN_RESIZE`, so the guest renders at the
    /// tile size natively, with no host-side scaling.
    /// 0 until Shade has placed the window.
    tile_w: u32,
    tile_h: u32,
    /// Set by `set_tile_size` when the tile size changed, taken by the
    /// VM core (`take_display_dirty`), which sends the guest the new size.
    /// Many resizes before one take = one message.
    display_dirty: bool,
    /// Number of the newest frame written (`Frame::seq`).
    seq: u64,
    /// `front` is out with a reader (`with_front`).
    front_out: bool,
    /// What each of the last frames changed against the one before:
    /// `(seq, width, height, rect)`, indexed by `seq % HISTORY`.
    history: [(u64, u32, u32, (u32, u32, u32, u32)); HISTORY],
}

/// Frames of damage kept per surface. A buffer that missed more frames than
/// this is copied whole.
const HISTORY: usize = 8;

#[derive(Default)]
struct Frame {
    /// A `PixelBuffer`, so the GPU can compose straight from it.
    pixels: PixelBuffer,
    /// Number of the guest frame `pixels` holds; 0: undefined contents.
    seq: u64,
    width: u32,
    height: u32,
    /// A frame that lies in guest pages, and its number; `pixels` is then
    /// only a copy made for the CPU.
    blob: Option<(ForeignView, u64)>,
}

/// Keys of guest frames in guest pages (`Sources::with_source_keyed`).
const KEY_BLOB: u64 = 1 << 63;
/// The CPU met a frame only the card reads, and said so once.
static GPU_ONLY_SAID: AtomicBool = AtomicBool::new(false);

impl Frame {
    /// Bring `pixels` up to the blob frame: only what changed since the
    /// frame they hold, if `history` says that. False when there is no
    /// memory for them, or the frame is one only the card reads; `held` is
    /// what all surfaces hold.
    fn materialize(&mut self, history: &[(u64, u32, u32, (u32, u32, u32, u32)); HISTORY], held: usize) -> bool {
        let Some((view, n)) = self.blob.as_ref() else { return true };
        if view.pixels.gpu_only() {
            if !GPU_ONLY_SAID.swap(true, Ordering::Relaxed) {
                crate::kprintln!("[gpu] a guest frame laid out for the card is not composed on the CPU");
            }
            return false;
        }
        let (w, h) = (view.width, view.height);
        if self.pixels.width() != w || self.pixels.height() != h {
            if !surface_fits(held.saturating_sub(self.pixels.len()), w as usize * h as usize) {
                return false;
            }
            self.pixels = PixelBuffer::empty();
            self.pixels = PixelBuffer::new(w, h);
            self.seq = 0;
            if self.pixels.is_empty() {
                return false;
            }
        }
        if self.seq == *n {
            return true;
        }
        let region = missed(history, self.seq, *n, w, h).unwrap_or((0, 0, w, h));
        view.copy_into(&mut self.pixels, region);
        COPIED.fetch_add(region.2 as u64 * region.3 as u64 * 4, Ordering::Relaxed);
        self.seq = *n;
        true
    }
}

impl GuestSurface {
    fn new() -> Self {
        GuestSurface {
            ready: Frame::default(),
            front: Frame::default(),
            back: Frame::default(),
            fresh: false,
            width: 0,
            height: 0,
            dirty: false,
            damage: None,
            tile_w: 0,
            tile_h: 0,
            display_dirty: false,
            seq: 0,
            front_out: false,
            history: [(0, 0, 0, (0, 0, 0, 0)); HISTORY],
        }
    }
}

impl GuestSurface {
    fn held_pixels(&self) -> usize {
        self.ready.pixels.len() + self.front.pixels.len() + self.back.pixels.len()
    }
}

/// Slack over the screen size, in percent: frames during a resize may be
/// larger than the tile they are headed for.
const SURFACE_SLACK_PERCENT: usize = 25;

/// Most pixels of one guest frame: a tile is never larger than the screen.
fn frame_max() -> usize {
    let fb = crate::framebuffer::get_info();
    let screen = match (fb.width as usize).saturating_mul(fb.height as usize) {
        0 => 3840 * 2160,
        n => n,
    };
    screen.saturating_mul(100 + SURFACE_SLACK_PERCENT) / 100
}

/// Whether a new `px_count`-pixel buffer may join the `held` pixels of all
/// surfaces. A tile is never larger than the screen, so neither is a frame;
/// all windows together need at most three screen-sized buffers each. The
/// host keeps its memory reserve on top.
fn surface_fits(held: usize, px_count: usize) -> bool {
    let frame_max = frame_max();
    let total_max = frame_max
        .saturating_mul(3)
        .saturating_mul(crate::microvm::devices::virtio_gpu_pci::MAX_OUTPUTS as usize);
    if px_count > frame_max || held.saturating_add(px_count) > total_max {
        return false;
    }
    let (free, _) = crate::memory::stats();
    let reserve = crate::forge_rt::KERNEL_RESERVE_MB * (1024 * 1024 / crate::memory::PAGE_SIZE);
    free.saturating_sub(reserve) > (px_count * 4).div_ceil(crate::memory::PAGE_SIZE)
}

static SURFACES: Mutex<BTreeMap<u32, GuestSurface>> = Mutex::new(BTreeMap::new());


/// Any surface has `display_dirty` set — lets the vCPU ask on every exit
/// without taking the map lock.
static ANY_DISPLAY_DIRTY: AtomicBool = AtomicBool::new(false);

/// The pixels as their raw bytes, in memory order.
fn as_bytes_mut(px: &mut [u32]) -> &mut [u8] {
    // SAFETY: `px` is `px.len() * 4` contiguous initialised bytes, u8 has
    // alignment 1 and every bit pattern is a valid u8 and u32; the returned
    // slice borrows `px` mutably, so nothing else can alias it.
    unsafe { core::slice::from_raw_parts_mut(px.as_mut_ptr() as *mut u8, px.len() * 4) }
}

/// Copy a freshly-flushed `w*h` BGRX frame (raw bytes, 4 per pixel)
/// into the window's surface and mark it dirty. `dmg` is what the frame
/// changed against the one flushed before it. Only what changed since the
/// frame the reused buffer holds is copied — the buffers rotate, so that is
/// the union of the damage of the frames it missed. Creates/resizes the
/// surface on first frame or geometry change. Cheap no-op if `src` is too
/// small for the claimed geometry (defensive — never panics).
pub fn write_frame(window_id: u32, src: &[u8], width: u32, height: u32, dmg: (u32, u32, u32, u32)) {
    let px_count = (width as usize).saturating_mul(height as usize);
    if px_count == 0 || src.len() < px_count * 4 {
        return;
    }
    let x = dmg.0.min(width);
    let y = dmg.1.min(height);
    let own = (x, y, dmg.2.min(width - x), dmg.3.min(height - y));
    let (mut back, held, seq, history) = {
        let mut map = SURFACES.lock();
        let held: usize = map.values().map(GuestSurface::held_pixels).sum();
        let surf = map.entry(window_id).or_insert_with(GuestSurface::new);
        (core::mem::take(&mut surf.back), held, surf.seq, surf.history)
    };
    if back.pixels.width() != width || back.pixels.height() != height {
        // The guest picks the frame size: past the budget, or when the host
        // cannot reserve the memory, the frame is dropped.
        if !surface_fits(held, px_count) {
            return;
        }
        back.pixels = PixelBuffer::empty();
        back.pixels = PixelBuffer::new(width, height);
        back.seq = 0;
        if back.pixels.is_empty() {
            return;
        }
    }
    back.width = width;
    back.height = height;
    let new_seq = seq + 1;
    let region = missed(&history, back.seq, seq, width, height)
        .map(|r| union(r, own))
        .unwrap_or((0, 0, width, height));
    // The guest sends little-endian BGRX bytes, which on x86 (LE) are the
    // exact in-memory layout of the packed u32 the compositor reads.
    let stride = width as usize * 4;
    let (rx, ry, rw, rh) = region;
    let dst = as_bytes_mut(&mut back.pixels);
    for row in ry as usize..(ry + rh) as usize {
        let a = row * stride + rx as usize * 4;
        let b = a + rw as usize * 4;
        dst[a..b].copy_from_slice(&src[a..b]);
    }
    back.seq = new_seq;
    COPIED.fetch_add(rw as u64 * rh as u64 * 4, Ordering::Relaxed);
    WHOLE.fetch_add(px_count as u64 * 4, Ordering::Relaxed);

    let mut map = SURFACES.lock();
    let Some(surf) = map.get_mut(&window_id) else { return };
    core::mem::swap(&mut surf.ready, &mut back);
    surf.back = back;
    publish(map, window_id, new_seq, width, height, own);
}

/// A guest frame that lies in guest pages becomes the window's newest
/// picture without a copy. `dmg` is what it changed against the frame
/// presented before it.
pub fn present_blob(window_id: u32, view: ForeignView, dmg: (u32, u32, u32, u32)) {
    if !view.fits() || view.width as usize * view.height as usize > frame_max() {
        return;
    }
    let (width, height) = (view.width, view.height);
    let x = dmg.0.min(width);
    let y = dmg.1.min(height);
    let own = (x, y, dmg.2.min(width - x), dmg.3.min(height - y));
    let mut map = SURFACES.lock();
    let surf = map.entry(window_id).or_insert_with(GuestSurface::new);
    let new_seq = surf.seq + 1;
    // The frame it replaces, if never shown, keeps only its copy's buffer.
    surf.ready.blob = Some((view, new_seq));
    surf.ready.width = width;
    surf.ready.height = height;
    WHOLE.fetch_add(width as u64 * height as u64 * 4, Ordering::Relaxed);
    publish(map, window_id, new_seq, width, height, own);
}

/// `ready` holds frame `new_seq` (`own` changed against the one before):
/// record it, mark it for the compositor and ask for a render.
fn publish(mut map: spin::MutexGuard<'_, BTreeMap<u32, GuestSurface>>, window_id: u32, new_seq: u64, width: u32,
           height: u32, own: (u32, u32, u32, u32)) {
    let Some(surf) = map.get_mut(&window_id) else { return };
    let dmg = own;
    surf.seq = new_seq;
    surf.history[new_seq as usize % HISTORY] = (new_seq, width, height, own);
    let first = surf.width == 0;
    let realloc = surf.width != width || surf.height != height;
    surf.width = width;
    surf.height = height;
    surf.fresh = true;
    surf.dirty = true;

    // Union the guest's damage rect (clamped to the surface) into the
    // pending region. A fresh buffer must blit in full — the new pixels
    // around the reported rect are otherwise undefined — and so must a
    // frame that does not match the tile, whose last row and column the
    // compositor extends to the tile's edge. Coalesced frames (cursor took
    // priority) accumulate here until the next render.
    let mismatch = surf.tile_w != 0 && (width != surf.tile_w || height != surf.tile_h);
    let dmg = if realloc || mismatch {
        (0, 0, width.max(surf.tile_w), height.max(surf.tile_h))
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
    // A new guest frame must trigger a recomposite, otherwise the tile only
    // updates when some other event (a click, a key) happens to call
    // render_frame. Use the clipped surface path (blit only the tile rect,
    // not the whole screen): a 60 Hz guest doing a full-screen MMIO blit
    // every frame starves the cursor. Both just set an atomic; poll_render
    // picks it up.
    // The first frame makes the window itself appear (`has_frame`), border
    // and all, which the clipped tile blit does not cover.
    if first {
        PENDING_REVEAL.lock().push(window_id);
        crate::shade::request_render();
    } else if crate::shade::SURFACE_CLIP_BLIT {
        crate::shade::request_surface_render();
    } else {
        crate::shade::request_render();
    }
}

/// The union of what frames `held + 1 ..= newest` changed, all of size
/// `width` x `height`; None when a buffer holding frame `held` cannot be
/// brought up to date that way (undefined, too old, or a size in between).
fn missed(history: &[(u64, u32, u32, (u32, u32, u32, u32)); HISTORY], held: u64, newest: u64, width: u32,
          height: u32) -> Option<(u32, u32, u32, u32)> {
    if held == 0 || held > newest || newest - held >= HISTORY as u64 {
        return None;
    }
    let mut r = (0, 0, 0, 0);
    for s in held + 1..=newest {
        let (hs, w, h, d) = history[s as usize % HISTORY];
        if hs != s || w != width || h != height {
            return None;
        }
        r = union(r, d);
    }
    Some(r)
}

fn union(a: (u32, u32, u32, u32), b: (u32, u32, u32, u32)) -> (u32, u32, u32, u32) {
    if a.2 == 0 || a.3 == 0 {
        return b;
    }
    if b.2 == 0 || b.3 == 0 {
        return a;
    }
    let x0 = a.0.min(b.0);
    let y0 = a.1.min(b.1);
    let x1 = (a.0 + a.2).max(b.0 + b.2);
    let y1 = (a.1 + a.3).max(b.1 + b.3);
    (x0, y0, x1 - x0, y1 - y0)
}

/// Bytes `write_frame` copied, and what whole frames would have been.
static COPIED: AtomicU64 = AtomicU64::new(0);
static WHOLE: AtomicU64 = AtomicU64::new(0);

/// `(copied, whole)` bytes since boot, for `cores`.
pub fn copy_stats() -> (u64, u64) {
    (COPIED.load(Ordering::Relaxed), WHOLE.load(Ordering::Relaxed))
}

/// Read the current surface pixels for compositing. `f` gets
/// `(pixels, width, height)`. `None` if no surface for this window.
pub fn with_front<F, R>(window_id: u32, f: F) -> Option<R>
where
    F: FnOnce(&[u32], u32, u32) -> R,
{
    with_front_keyed(window_id, |px, w, h, _| f(px, w, h))
}

/// `with_front`, also passing the picture's key: the one `front_in_place`
/// gives for a frame in guest pages, else the pixels' address. A frame in
/// guest pages is copied here, as far as it changed since the last copy.
pub fn with_front_keyed<F, R>(window_id: u32, f: F) -> Option<R>
where
    F: FnOnce(&[u32], u32, u32, u64) -> R,
{
    let (mut front, history, held) = {
        let mut map = SURFACES.lock();
        let held: usize = map.values().map(GuestSurface::held_pixels).sum();
        let s = map.get_mut(&window_id)?;
        if s.front_out {
            return None;
        }
        if s.fresh {
            core::mem::swap(&mut s.front, &mut s.ready);
            s.fresh = false;
        }
        s.front_out = true;
        if let Some((v, _)) = &s.front.blob {
            v.pixels.hold();
        }
        (core::mem::take(&mut s.front), s.history, held)
    };
    // Copy and blit with the lock released — the producer may publish
    // meanwhile.
    let ok = front.materialize(&history, held);
    let key = match &front.blob {
        Some((v, n)) => {
            v.pixels.unhold();
            KEY_BLOB | n
        }
        None => front.pixels.as_ptr() as u64,
    };
    let r = ok.then(|| f(&front.pixels, front.width, front.height, key));
    if let Some(s) = SURFACES.lock().get_mut(&window_id) {
        if s.front_out {
            s.front = front;
            s.front_out = false;
        }
    }
    r
}

/// The front picture when it lies in guest pages, registered with gpudev
/// for the scene being encoded (under the map lock, so `pages_idle` sees it
/// from the moment the picture is taken).
pub fn front_in_place(window_id: u32) -> Option<InPlace> {
    let mut map = SURFACES.lock();
    let s = map.get_mut(&window_id)?;
    if s.front_out {
        return None;
    }
    if s.fresh {
        core::mem::swap(&mut s.front, &mut s.ready);
        s.fresh = false;
    }
    let (view, n) = s.front.blob.as_ref()?;
    Some(InPlace {
        bo: crate::gpu::gpudev::bo_for_pages(view),
        key: KEY_BLOB | n,
        width: view.width,
        height: view.height,
    })
}

/// The pages with `serial` are about to be written or freed by the guest:
/// a picture still shown from them becomes a copy, the others forget them.
/// False when a reader kept the surfaces past the deadline.
pub fn detach_blob(serial: u64) -> bool {
    let deadline = crate::interrupts::rdtsc() + crate::interrupts::tsc_freq() / 10;
    loop {
        let mut map = SURFACES.lock();
        if map.values().any(|s| s.front_out) {
            drop(map);
            if crate::interrupts::rdtsc() >= deadline {
                return false;
            }
            crate::smp::fiber::spin_or_yield();
            continue;
        }
        let mut held: usize = map.values().map(GuestSurface::held_pixels).sum();
        for s in map.values_mut() {
            let history = s.history;
            let fresh = s.fresh;
            for (frame, shown) in [(&mut s.front, true), (&mut s.ready, fresh), (&mut s.back, false)] {
                if !frame.blob.as_ref().is_some_and(|(v, _)| v.pixels.serial == serial) {
                    continue;
                }
                let before = frame.pixels.len();
                let ok = !shown || frame.materialize(&history, held);
                held = (held + frame.pixels.len()).saturating_sub(before);
                if !ok {
                    // No copy (no memory, or a frame only the card reads): the
                    // window shows nothing.
                    frame.seq = 0;
                    frame.width = frame.pixels.width();
                    frame.height = frame.pixels.height();
                }
                frame.blob = None;
            }
        }
        return true;
    }
}

/// Take (and clear) the union of damage rects (surface-local coords)
/// accumulated since the last call. The compositor blits only this region
/// to MMIO instead of the whole tile. `None` if nothing changed.
pub fn take_damage(window_id: u32) -> Option<(u32, u32, u32, u32)> {
    SURFACES.lock().get_mut(&window_id).and_then(|s| s.damage.take())
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
    let surf = map.entry(window_id).or_insert_with(GuestSurface::new);
    if surf.tile_w != w || surf.tile_h != h {
        surf.tile_w = w;
        surf.tile_h = h;
        surf.display_dirty = true;
        ANY_DISPLAY_DIRTY.store(true, Ordering::Release);
        crate::microvm::cpu::note_device_work();
    }
}

/// Windows to reveal once Core 0 renders next: their guest presented the
/// first frame on a vCPU core, and only Core 0 touches the compositor.
static PENDING_REVEAL: Mutex<Vec<u32>> = Mutex::new(Vec::new());

/// A window whose first frame arrived, if any (taken off the list).
pub fn take_reveal() -> Option<u32> {
    PENDING_REVEAL.lock().pop()
}

/// True once the guest has presented a frame. Until then the window is not
/// drawn at all: a Linux app's window appears with its first picture.
pub fn has_frame(window_id: u32) -> bool {
    SURFACES.lock().get(&window_id).is_some_and(|s| s.width != 0)
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

/// Any window's tile size changed and is not yet taken — lets the vCPU ask
/// on every exit without taking the map lock.
pub fn any_display_dirty() -> bool {
    ANY_DISPLAY_DIRTY.load(Ordering::Acquire)
}

/// True (and clears the flag) if the tile size changed since the last
/// call → the VM core sends the guest the new size.
pub fn take_display_dirty(window_id: u32) -> bool {
    let mut map = SURFACES.lock();
    match map.get_mut(&window_id) {
        Some(s) => {
            let d = s.display_dirty;
            s.display_dirty = false;
            if !map.values().any(|s| s.display_dirty) {
                ANY_DISPLAY_DIRTY.store(false, Ordering::Release);
            }
            d
        }
        None => false,
    }
}

/// Drop a window's surface (window closed / VM exited).
pub fn remove_surface(window_id: u32) {
    SURFACES.lock().remove(&window_id);
}
