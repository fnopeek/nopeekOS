# `kernel/src/shade/widgets/canvas.rs` @ 5e0102684

## L1-16 · `use alloc::collections::BTreeMap;`

```
//! Canvas bitmap store (P10.10 escape hatch).
//!
//! Apps with the CANVAS capability upload pixels via `npk_canvas_commit`
//! (BGRA32) or `npk_canvas_commit_yuv` (planar 4:2:0); it is stored here
//! keyed by `(window_id, canvas_id)`. The render walker's `Widget::Canvas`
//! arm looks it up and blits it (contain-fit) into the canvas rect.
//! Decoupled from the widget tree so a re-render (resize / theme) keeps
//! showing the last committed pixels without the app re-uploading.
//!
//! **Why 4:2:0 is stored as 4:2:0 and not converted here:** a video
//! decoder holds planar YUV, and turning it into BGRA is the single most
//! expensive thing in the whole path. Measured at 1080p30 it costs 145 %
//! of a core INSIDE a module — more than decoding the frame. Kept planar,
//! the conversion happens in the blit instead: natively, and only for the
//! pixels that actually land in the canvas rect. It also carries 1.5 bytes
//! per pixel across the module boundary instead of 4.
```

## L22 · `pub const MAX_DIM: u32 = 4096;`

```
/// Per-app pixel caps (mirror the P10.10 spec): 4096×4096, 64 MB total.
```

## L26-29 · `#[derive(Clone, Copy, PartialEq, Eq, Debug)]`

```
/// Which matrix and range the Y′CbCr planes were coded with. A video
/// stream says so in its VUI; guessing here would tint every frame, so the
/// module passes what it read and we do not invent a default beyond the
/// one the flags spell out.
```

## L36-37 · `#[derive(Clone, Copy)]`

```
/// Integer conversion coefficients, 8-bit fraction. Same shape for all four
/// combinations, so the blit has one code path and no branch per pixel.
```

## L51 · `(false, false) => YuvCoeffs { y_mul: 298, y_off: 16, r_v: 409, g_u: 100, g_v: 208, b_u: 516 },`

```
// Rec. 601 limited — the classic 298/409/100/208/516.
```

## L53 · `(false, true)  => YuvCoeffs { y_mul: 256, y_off: 0,  r_v: 359, g_u: 88,  g_v: 183, b_u: 454 },`

```
// Rec. 601 full (what JPEG and most camera MP4s carry).
```

## L55 · `(true, false)  => YuvCoeffs { y_mul: 298, y_off: 16, r_v: 459, g_u: 55,  g_v: 136, b_u: 541 },`

```
// Rec. 709 limited — the default for HD video.
```

## L62-64 · `pub enum Pixels {`

```
/// What a canvas holds. Both arms are already validated when they get here:
/// every plane is long enough for `w`×`h` at its stride, so the blit may
/// index without asking again.
```

## L66 · `Bgra(Vec<u8>),`

```
/// BGRA32, `w * h * 4` bytes.
```

## L68-69 · `I420 {`

```
/// Planar 4:2:0. `ys`/`cs` are row strides in bytes (a decoder pads
/// rows), `y.len() >= ys * h` and each chroma plane `>= cs * ceil(h/2)`.
```

## L88-90 · `static RECTS: Mutex<BTreeMap<(u32, u32), (i32, i32, u32, u32)>> = Mutex::new(BTreeMap::new());`

```
/// Last laid-out rect (x, y, w, h) per (window_id, canvas_id), recorded by
/// the render walker so an app (e.g. the browser) can query its real
/// on-screen size and paint 1:1 instead of relying on the contain-fit blit.
```

## L93-95 · `pub fn commit(window_id: u32, canvas_id: u32, w: u32, h: u32, px: Vec<u8>) -> bool {`

```
/// Store (or replace) the bitmap for `(window_id, canvas_id)`. `px` must
/// be `w * h * 4` BGRA bytes; rejected (returns false) if oversized or
/// the length doesn't match.
```

## L104-107 · `pub fn commit_i420(`

```
/// Store (or replace) a planar 4:2:0 frame. Every bound the blit relies on
/// is checked HERE and nowhere else — the blit walks these planes without
/// re-deriving a single length, so this is the one place that decides
/// whether an index can go out of range.
```

## L114 · `let cw = ((w + 1) / 2) as usize;`

```
// Chroma is half size, rounded UP: an odd width still has a column.
```

## L118-120 · `let need_y = ys.checked_mul(h as usize).unwrap_or(usize::MAX);`

```
// `ys * h` rather than `ys * (h-1) + w`: a caller that pads rows also
// owns the padding, and the looser bound would let the last row run
// into memory it never promised.
```

## L132-134 · `pub fn with_bitmap<R>(window_id: u32, canvas_id: u32, f: impl FnOnce(&Pixels, u32, u32) -> R) -> Option<R> {`

```
/// Run `f` with the stored pixels (whatever form they are in, w, h) if
/// present. The lock is held for the duration, so `f` must not re-enter
/// this module.
```

## L140 · `pub fn record_rect(window_id: u32, canvas_id: u32, x: i32, y: i32, w: u32, h: u32) {`

```
/// Record the canvas widget's laid-out rect (called by the render walker).
```

## L145 · `pub fn rect_of(window_id: u32, canvas_id: u32) -> Option<(i32, i32, u32, u32)> {`

```
/// The last recorded rect for a canvas, if it has been laid out at least once.
```

## L150-151 · `pub fn remove_window(window_id: u32) {`

```
/// Drop every bitmap + rect owned by a window — called from `remove_scene`
/// when a widget window closes.
```

