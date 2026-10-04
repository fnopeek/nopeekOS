# `kernel/src/shade/surface.rs` @ 5e0102684

## L1-19 · `extern crate alloc;`

```
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
//! Concurrency: the producer (virtio-gpu FLUSH on a vCPU core) and the
//! consumer (Shade render on Core 0) run on different cores. Three
//! buffers per surface (back → ready → front): each side copies or blits
//! with the map lock RELEASED and takes it only to swap buffers. Holding it
//! across a multi-MB copy or blit stalled the other side for milliseconds —
//! the vCPU on its next exit, and with it every interrupt it would have
//! delivered.
```

## L27-28 · `pub struct GuestSurface {`

```
/// One window's bitmap content. BGRX (0x00RRGGBB packed as the host
/// framebuffer expects), `width * height` pixels.
```

## L30 · `ready: Frame,`

```
/// Newest complete frame, not yet taken by the compositor.
```

## L32 · `front: Frame,`

```
/// The compositor's frame (moved out while it blits).
```

## L34 · `back: Frame,`

```
/// The producer's scratch buffer (moved out while it copies).
```

## L36 · `fresh: bool,`

```
/// `ready` holds a frame newer than `front`.
```

## L40-41 · `dirty: bool,`

```
/// Set on `write_frame`, cleared by `take_dirty`. Lets the
/// compositor skip recompositing an unchanged surface tile.
```

## L43-49 · `damage: Option<(u32, u32, u32, u32)>,`

```
/// Union of the guest's damage rects (surface-local coords) since the
/// last `take_damage`. The MMIO blit is clipped to this instead of the
/// whole tile — at 4K that turns a tens-of-MB full-tile blit into just
/// the changed region (e.g. a video/graph). Unions across frames that
/// coalesced while the cursor took priority (see poll_render), so no
/// damage is lost. `None` = nothing changed; a full-buffer (re)alloc
/// sets it to the whole surface.
```

## L51-55 · `tile_w: u32,`

```
/// Desired output size = the window's content rect, written by
/// Shade on create + every retile (`set_tile_size`). virtio-gpu
/// `GET_DISPLAY_INFO` reports this so the guest (wlroots/cage)
/// reflows to the tile natively — D4, no host-side scaling.
/// 0 until Shade has placed the window.
```

## L58-61 · `display_dirty: bool,`

```
/// Set by `set_tile_size` when the tile size changed, taken by the
/// VM core (`take_display_dirty`) to raise a virtio-gpu
/// config-change IRQ so the guest re-queries GET_DISPLAY_INFO.
/// Coalescing is free: many resizes before one take = one IRQ.
```

## L92-93 · `static ANY_DISPLAY_DIRTY: AtomicBool = AtomicBool::new(false);`

```
/// Any surface has `display_dirty` set — lets the vCPU ask on every exit
/// without taking the map lock.
```

## L96-99 · `pub fn write_frame(window_id: u32, src: &[u8], width: u32, height: u32, dmg: (u32, u32, u32, u32)) {`

```
/// Copy a freshly-flushed `w*h` BGRX frame (raw bytes, 4 per pixel)
/// into the window's surface and mark it dirty. Creates/resizes the
/// surface on first frame or geometry change. Cheap no-op if `src` is
/// too small for the claimed geometry (defensive — never panics).
```

## L115-118 · `let dst = unsafe {`

```
// The guest sends little-endian BGRX bytes, which on x86 (LE) ARE the
// exact in-memory layout of the packed u32 the compositor reads.
// SAFETY: back.pixels holds px_count u32s = px_count*4 contiguous bytes;
// src has at least px_count*4 bytes (checked above).
```

## L134-137 · `let dmg = if realloc {`

```
// Union the guest's damage rect (clamped to the surface) into the
// pending region. A fresh buffer must blit in full — the new pixels
// around the reported rect are otherwise undefined. Coalesced frames
// (cursor took priority) accumulate here until the next render.
```

## L160-166 · `if crate::shade::SURFACE_CLIP_BLIT {`

```
// A new guest frame must trigger a recomposite — otherwise the
// tile only updates when some *other* event (a click, a key)
// happens to call render_frame (observed: cyan appeared only
// after clicking the window). Use the clipped surface path (blit
// only the tile rect, not the whole screen) — a 60 Hz guest doing a
// full-screen MMIO blit every frame starved the cursor on bare metal.
// Both just set an atomic; poll_render picks it up.
```

## L174-175 · `pub fn with_front<F, R>(window_id: u32, f: F) -> Option<R>`

```
/// Read the current surface pixels for compositing. `f` gets
/// `(pixels, width, height)`. `None` if no surface for this window.
```

## L189 · `let r = f(&front.pixels, front.width, front.height);`

```
// Blit with the lock released — the producer may publish meanwhile.
```

## L197-199 · `pub fn take_damage(window_id: u32) -> Option<(u32, u32, u32, u32)> {`

```
/// Take (and clear) the union of damage rects (surface-local coords)
/// accumulated since the last call. The compositor blits only this region
/// to MMIO instead of the whole tile. `None` if nothing changed.
```

## L204-209 · `pub fn set_tile_size(window_id: u32, w: u32, h: u32) {`

```
/// Shade tells the guest how big to render: the window's content
/// rect. Called on window create + every retile. Creates the surface
/// entry if absent so the size is known before the guest's first
/// `GET_DISPLAY_INFO` (wlroots queries it before rendering a pixel).
/// On a real change, flags `display_dirty` so the VM core raises a
/// virtio-gpu config-change IRQ and asks the guest to reflow.
```

## L224-226 · `pub fn tile_size(window_id: u32) -> Option<(u32, u32)> {`

```
/// The size the guest should render at (window content rect), for
/// virtio-gpu `GET_DISPLAY_INFO`. `None` if Shade hasn't placed the
/// window yet → caller falls back to a default.
```

## L238-242 · `pub fn display_dirty_peek(window_id: u32) -> bool {`

```
/// Non-consuming peek of the display-dirty flag. The VM core checks
/// this first so it can rate-limit the config-change IRQ (R2 debounce)
/// WITHOUT clearing the flag when it decides to skip — the dirty state
/// (and the latest tile size) then survives to the next allowed
/// window, so the final resize size is always delivered.
```

## L248-251 · `pub fn take_display_dirty(window_id: u32) -> bool {`

```
/// True (and clears the flag) if the tile size changed since the last
/// call → the VM core must raise a virtio-gpu config-change IRQ so the
/// guest re-queries `GET_DISPLAY_INFO`. Only consumed on a tick where
/// the IRQ can actually be injected, so a missed slot keeps the flag.
```

## L267 · `pub fn remove_surface(window_id: u32) {`

```
/// Drop a window's surface (window closed / VM exited).
```

