//! Canvas bitmap store (escape hatch for app-drawn pixels).
//!
//! Apps with the CANVAS capability upload pixels via `npk_canvas_commit`
//! (BGRA32) or `npk_canvas_commit_yuv` (planar 4:2:0); it is stored here
//! keyed by `(window_id, canvas_id)`. The render walker's `Widget::Canvas`
//! arm looks it up and blits it (contain-fit) into the canvas rect.
//! Decoupled from the widget tree so a re-render (resize / theme) keeps
//! showing the last committed pixels without the app re-uploading.
//!
//! Why 4:2:0 is stored as 4:2:0 and not converted here: a video decoder
//! holds planar YUV, and turning it into BGRA is the most expensive step in
//! the whole path, more than decoding the frame when done inside a module.
//! Kept planar, the conversion happens in the blit instead: natively, and
//! only for the pixels that actually land in the canvas rect. It also
//! carries 1.5 bytes per pixel across the module boundary instead of 4.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use spin::Mutex;

/// Largest canvas side: what the display hardware scans out, Linux's
/// `mode_config.max_width` on Gen12 and DCN.
pub const MAX_DIM: u32 = 16384;

/// Host bytes one entry costs besides its pixels.
const ENTRY_BYTES: usize = 64;

/// What all canvases of one window may hold: a quarter of RAM. Canvas ids
/// are the app's to choose, so without a bound one window could store a
/// full-size bitmap under every id it likes; a byte budget per owner, not
/// a count, is that bound.
pub fn window_budget() -> usize {
    (crate::memory::usable_bytes() / 4) as usize
}

/// Which matrix and range the Y′CbCr planes were coded with. A video
/// stream says so in its VUI; guessing here would tint every frame, so the
/// module passes what it read and we do not invent a default beyond the
/// one the flags spell out.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct YuvCoding {
    pub bt709: bool,
    pub full_range: bool,
}

/// Integer conversion coefficients, 8-bit fraction. Same shape for all four
/// combinations, so the blit has one code path and no branch per pixel.
#[derive(Clone, Copy)]
pub struct YuvCoeffs {
    pub y_mul: i32,
    pub y_off: i32,
    pub r_v: i32,
    pub g_u: i32,
    pub g_v: i32,
    pub b_u: i32,
}

impl YuvCoding {
    pub fn coeffs(self) -> YuvCoeffs {
        match (self.bt709, self.full_range) {
            // Rec. 601 limited — the classic 298/409/100/208/516.
            (false, false) => YuvCoeffs { y_mul: 298, y_off: 16, r_v: 409, g_u: 100, g_v: 208, b_u: 516 },
            // Rec. 601 full (what JPEG and most camera MP4s carry).
            (false, true)  => YuvCoeffs { y_mul: 256, y_off: 0,  r_v: 359, g_u: 88,  g_v: 183, b_u: 454 },
            // Rec. 709 limited — the default for HD video.
            (true, false)  => YuvCoeffs { y_mul: 298, y_off: 16, r_v: 459, g_u: 55,  g_v: 136, b_u: 541 },
            (true, true)   => YuvCoeffs { y_mul: 256, y_off: 0,  r_v: 403, g_u: 48,  g_v: 120, b_u: 475 },
        }
    }
}

/// What a canvas holds. Both arms are already validated when they get here:
/// every plane is long enough for `w`×`h` at its stride, so the blit may
/// index without asking again.
pub enum Pixels {
    /// BGRA32, `w * h * 4` bytes.
    Bgra(Vec<u8>),
    /// Planar 4:2:0. `ys`/`cs` are row strides in bytes (a decoder pads
    /// rows), `y.len() >= ys * h` and each chroma plane `>= cs * ceil(h/2)`.
    I420 {
        y: Vec<u8>,
        u: Vec<u8>,
        v: Vec<u8>,
        ys: usize,
        cs: usize,
        coding: YuvCoding,
    },
    /// A frame the compositor shows as a scene layer over the window
    /// (`npk_canvas_frame`): BGRA32 in a buffer the GPU reads in place,
    /// rewritten in place by the next frame of the same size. The window's
    /// raster leaves the canvas rect alone.
    /// In YUYV the buffer is `w / 2` words wide, two pixels a word.
    Frame(crate::shade::buffers::PixelBuffer, crate::shade::scene::Yuv),
}

struct Bitmap {
    w: u32,
    h: u32,
    px: Pixels,
}

static CANVASES: Mutex<BTreeMap<(u32, u32), Bitmap>> = Mutex::new(BTreeMap::new());

impl Pixels {
    fn bytes(&self) -> usize {
        match self {
            Pixels::Bgra(px) => px.len(),
            Pixels::I420 { y, u, v, .. } => y.len() + u.len() + v.len(),
            Pixels::Frame(b, _) => b.len() * 4,
        }
    }
}

/// Store `bmp` under `(window_id, canvas_id)` if the window stays within its
/// caps; the bitmap it replaces does not count.
fn insert_within_caps(window_id: u32, canvas_id: u32, bmp: Bitmap) -> bool {
    let mut map = CANVASES.lock();
    let others = map.range((window_id, 0)..=(window_id, u32::MAX))
        .filter(|(k, _)| k.1 != canvas_id);
    let bytes: usize = others.map(|(_, v)| v.px.bytes() + ENTRY_BYTES).sum();
    if bytes + bmp.px.bytes() + ENTRY_BYTES > window_budget() {
        crate::kprintln!("[npk] canvas: window {} would hold {} MB of canvases, more than its {} MB",
            window_id, (bytes + bmp.px.bytes()) >> 20, window_budget() >> 20);
        return false;
    }
    map.insert((window_id, canvas_id), bmp);
    true
}

/// Last laid-out rect (x, y, w, h) per (window_id, canvas_id), recorded by
/// the render walker so an app (e.g. the browser) can query its real
/// on-screen size and paint 1:1 instead of relying on the contain-fit blit.
static RECTS: Mutex<BTreeMap<(u32, u32), (i32, i32, u32, u32)>> = Mutex::new(BTreeMap::new());

/// Store (or replace) the bitmap for `(window_id, canvas_id)`. `px` must
/// be `w * h * 4` BGRA bytes; rejected (returns false) if oversized or
/// the length doesn't match.
pub fn commit(window_id: u32, canvas_id: u32, w: u32, h: u32, px: Vec<u8>) -> bool {
    if w == 0 || h == 0 || w > MAX_DIM || h > MAX_DIM { return false; }
    let need = (w as usize) * (h as usize) * 4;
    if px.len() != need { return false; }
    insert_within_caps(window_id, canvas_id, Bitmap { w, h, px: Pixels::Bgra(px) })
}

/// Store (or replace) a planar 4:2:0 frame. Every bound the blit relies on
/// is checked here and nowhere else — the blit walks these planes without
/// re-deriving a single length, so this is the one place that decides
/// whether an index can go out of range.
pub fn commit_i420(
    window_id: u32, canvas_id: u32, w: u32, h: u32,
    y: Vec<u8>, u: Vec<u8>, v: Vec<u8>, ys: usize, cs: usize,
    coding: YuvCoding,
) -> bool {
    if w == 0 || h == 0 || w > MAX_DIM || h > MAX_DIM { return false; }
    // Chroma is half size, rounded up: an odd width still has a column.
    let cw = ((w + 1) / 2) as usize;
    let ch = ((h + 1) / 2) as usize;
    if ys < w as usize || cs < cw { return false; }
    // `ys * h` rather than `ys * (h-1) + w`: a caller that pads rows also
    // owns the padding, and the looser bound would let the last row run
    // into memory it never promised.
    let need_y = ys.checked_mul(h as usize).unwrap_or(usize::MAX);
    let need_c = cs.checked_mul(ch).unwrap_or(usize::MAX);
    if y.len() < need_y || u.len() < need_c || v.len() < need_c { return false; }
    insert_within_caps(
        window_id, canvas_id,
        Bitmap { w, h, px: Pixels::I420 { y, u, v, ys, cs, coding } },
    )
}

/// Run `f` with the stored pixels (whatever form they are in, w, h) if
/// present. The lock is held for the duration, so `f` must not re-enter
/// this module.
pub fn with_bitmap<R>(window_id: u32, canvas_id: u32, f: impl FnOnce(&Pixels, u32, u32) -> R) -> Option<R> {
    let guard = CANVASES.lock();
    guard.get(&(window_id, canvas_id)).map(|b| f(&b.px, b.w, b.h))
}

/// Frames committed since the last compose, by (window, canvas): their
/// rects are the compositor's damage (`take_frame_damage`).
static FRAME_DAMAGE: Mutex<Vec<(u32, u32)>> = Mutex::new(Vec::new());

/// Store a frame `w` x `h` as the canvas's scene layer: BGRA (`src` holds
/// `w * h * 4` bytes) or packed YUYV (`w` even, `w * h * 2` bytes). Same
/// size and format as the last frame: written into its buffer, after the
/// GPU finished reading it. Only the canvas rect is damaged; the window is
/// not rasterized again.
pub fn commit_frame(window_id: u32, canvas_id: u32, w: u32, h: u32, yuv: crate::shade::scene::Yuv,
                    src: &[u8]) -> bool {
    use crate::shade::scene::Yuv;
    if w == 0 || h == 0 || w > MAX_DIM || h > MAX_DIM { return false; }
    let bpp = if yuv == Yuv::None { 4 } else { 2 };
    if yuv != Yuv::None && w % 2 != 0 { return false; }
    if src.len() != w as usize * h as usize * bpp { return false; }
    let bw = if yuv == Yuv::None { w } else { w / 2 };
    let fill = |buf: &mut crate::shade::buffers::PixelBuffer| {
        for (d, s) in buf.iter_mut().zip(src.chunks_exact(4)) {
            *d = u32::from_le_bytes([s[0], s[1], s[2], s[3]]);
        }
    };
    'reuse: {
        let mut map = CANVASES.lock();
        if let Some(b) = map.get_mut(&(window_id, canvas_id)) {
            if let (Pixels::Frame(buf, f), true) = (&mut b.px, b.w == w && b.h == h) {
                if *f != yuv { break 'reuse; }
                fill(buf);
                drop(map);
                note_frame(window_id, canvas_id);
                return true;
            }
        }
    }
    let mut buf = crate::shade::buffers::PixelBuffer::new(bw, h);
    if buf.is_empty() { return false; }
    fill(&mut buf);
    if !insert_within_caps(window_id, canvas_id, Bitmap { w, h, px: Pixels::Frame(buf, yuv) }) { return false; }
    note_frame(window_id, canvas_id);
    true
}

fn note_frame(window_id: u32, canvas_id: u32) {
    let mut d = FRAME_DAMAGE.lock();
    if !d.contains(&(window_id, canvas_id)) { d.push((window_id, canvas_id)); }
}

/// The rects of the frames committed since the last call, on the screen
/// (canvas rects are relative to their window's content origin).
pub fn take_frame_damage() -> Vec<(i32, i32, u32, u32)> {
    let list = core::mem::take(&mut *FRAME_DAMAGE.lock());
    list.iter().filter_map(|&(w, c)| {
        let (x, y, rw, rh) = RECTS.lock().get(&(w, c)).copied()?;
        let (ox, oy) = super::origin_of(w)?;
        Some((x + ox, y + oy, rw, rh))
    }).collect()
}

/// The canvases of `window_id` that hold a frame layer, with its size in
/// pixels and its format.
pub fn frames_of(window_id: u32) -> Vec<(u32, u32, u32, crate::shade::scene::Yuv)> {
    CANVASES.lock().range((window_id, 0)..=(window_id, u32::MAX))
        .filter_map(|(k, b)| match b.px { Pixels::Frame(_, yuv) => Some((k.1, b.w, b.h, yuv)), _ => None })
        .collect()
}

/// `f(words, words per row, rows)` with a frame layer's buffer.
pub fn with_frame(window_id: u32, canvas_id: u32, f: &mut dyn FnMut(&[u32], u32, u32)) {
    let map = CANVASES.lock();
    if let Some(Bitmap { px: Pixels::Frame(buf, _), .. }) = map.get(&(window_id, canvas_id)) {
        f(buf, buf.width(), buf.height());
    }
}

/// Record the canvas widget's laid-out rect (called by the render walker).
pub fn record_rect(window_id: u32, canvas_id: u32, x: i32, y: i32, w: u32, h: u32) {
    RECTS.lock().insert((window_id, canvas_id), (x, y, w, h));
}

/// The last recorded rect for a canvas, if it has been laid out at least once.
pub fn rect_of(window_id: u32, canvas_id: u32) -> Option<(i32, i32, u32, u32)> {
    RECTS.lock().get(&(window_id, canvas_id)).copied()
}

/// Drop one bitmap and its rect. True if there was a bitmap.
pub fn remove(window_id: u32, canvas_id: u32) -> bool {
    RECTS.lock().remove(&(window_id, canvas_id));
    CANVASES.lock().remove(&(window_id, canvas_id)).is_some()
}

/// Drop every bitmap + rect owned by a window — called from `remove_scene`
/// when a widget window closes.
pub fn remove_window(window_id: u32) {
    CANVASES.lock().retain(|&(wid, _), _| wid != window_id);
    RECTS.lock().retain(|&(wid, _), _| wid != window_id);
}
