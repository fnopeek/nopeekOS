//! The desktop as layers on the display's planes (gpudev `CAP_LAYERS`).
//!
//! With a module that shows layers, the screen is no longer composed as a
//! whole: the wallpaper is the bottom layer as it is, and each group of
//! the scene (a window with its glow, chrome, content and controls; the
//! overlays above all windows) is painted into a buffer of its own, over
//! a transparent ground, premultiplied ARGB (`scene::execute_layer`). The
//! display mixes them, so a window that does not change costs nothing and
//! one that does is painted again only within its damage.
//!
//! Each layer has two buffers, the one on screen and the one painted next;
//! the next one also takes what the last frame painted into the other
//! (buffer age 2). A layer that moves or changes size is painted whole,
//! as glass shows the wallpaper at its place. The CPU paints a layer into
//! the screen-sized buffer the frame was handed (a shadow, not on screen
//! while layers are) and copies its damage out.
//!
//! A commit is waited for in `settle`, with the compositor released, as a
//! scene is (`gpu_scene::finish`); the buffers of a layer that went stay
//! until a commit without them is done. More groups than the display has
//! overlays, no wallpaper buffer, a buffer that cannot be had, a commit
//! the module refused: the screen is composed as before, whole.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

use crate::framebuffer::FbInfo;
use crate::gpu::gpudev;
use crate::shade::buffers::PixelBuffer;
use crate::shade::scene::{self, Item, Rect, Scene, Sources};

/// Overlays the display has for the windows (`VC4_NUM_OVERLAY_PLANES`);
/// the wallpaper takes the primary plane.
const MAX_GROUPS: usize = 16;
/// Rectangles a buffer keeps as what it lacks before they merge into one.
const MAX_LACKS: usize = 16;
/// The key of the group above all windows.
pub const TOP: u64 = u64::MAX;

/// Items `start..end` of a frame's scene, painted into one layer.
#[derive(Clone, Copy, Debug)]
pub struct Group {
    pub key: u64,
    pub start: usize,
    pub end: usize,
}

struct Plane {
    key: u64,
    /// Where the layer lies on the screen.
    rect: Rect,
    bufs: [PixelBuffer; 2],
    /// The buffer on screen, or painted for the next commit.
    front: usize,
    /// What each buffer lacks of the layer, in screen coordinates.
    lacks: [Vec<Rect>; 2],
}

impl Plane {
    fn new(key: u64, rect: Rect) -> Option<Plane> {
        let a = PixelBuffer::new(rect.w, rect.h);
        let b = PixelBuffer::new(rect.w, rect.h);
        if a.is_empty() || b.is_empty() {
            return None;
        }
        Some(Plane { key, rect, bufs: [a, b], front: 0, lacks: [alloc::vec![rect], alloc::vec![rect]] })
    }

    fn bo(&self, i: usize) -> Option<u32> {
        let b = &self.bufs[i];
        gpudev::bo_for_format(b.phys(), b.width() as u64 * b.height() as u64 * 4, b.width(), b.height(),
            b.width() * 4, gpudev::FORMAT_ARGB8888)
    }
}

struct State {
    planes: Vec<Plane>,
    /// The commit in flight, and the planes it no longer shows.
    pending: Option<(u32, Vec<Plane>)>,
    /// The layers last committed.
    shown: Vec<gpudev::Layer>,
}

static STATE: Mutex<State> = Mutex::new(State { planes: Vec::new(), pending: None, shown: Vec::new() });
/// The display shows layers, not a shadow.
static ON_SCREEN: AtomicBool = AtomicBool::new(false);
/// Layers stay off: a commit was refused.
static OFF: AtomicBool = AtomicBool::new(false);
/// The planes go with the next frame: the screen's flip that replaces
/// them is queued by then.
static FORGET: AtomicBool = AtomicBool::new(false);

/// Whether the display shows layers.
pub fn on_screen() -> bool {
    ON_SCREEN.load(Ordering::Acquire)
}

/// Whether a frame should be shown as layers.
fn usable() -> bool {
    gpudev::layers_supported() && !OFF.load(Ordering::Acquire)
}

/// What became of a frame handed to `compose`.
pub enum Composed {
    /// Shown as layers; the damage painted.
    Layers(Vec<Rect>),
    /// Not: the caller composes the screen, whole when `whole`.
    Screen { whole: bool },
}

/// The screen as layers, or why the caller composes it instead. `target`
/// is the screen-sized buffer the frame was handed, `groups` the scene's
/// groups bottom to top.
pub fn compose(frame: &Scene, groups: &[Group], sources: &dyn Sources, target: *mut u8, info: &FbInfo,
               damage: &[Rect]) -> Composed {
    settle();
    if !on_screen() && FORGET.swap(false, Ordering::AcqRel) {
        let mut st = STATE.lock();
        st.planes.clear();
        st.shown.clear();
    }
    if !usable() || groups.len() > MAX_GROUPS || info.bpp != 32 {
        return leave();
    }
    let planes = crate::gui::background::planes();
    let Some(wall) = planes.wall else { return leave() };
    let Some(wall_bo) = gpudev::bo_for(wall as u64, info.pitch as u64 * info.height as u64, info.width,
        info.height, info.pitch) else { return leave() };
    let screen = Rect::new(0, 0, info.width, info.height);

    let mut st = STATE.lock();
    let mut old = core::mem::take(&mut st.planes);
    let mut layers = alloc::vec![gpudev::Layer {
        bo: wall_bo, sx: 0, sy: 0, w: info.width, h: info.height, x: 0, y: 0, premultiplied: false,
    }];
    let mut painted = Vec::new();
    let mut planes = Vec::with_capacity(groups.len());
    let mut failed = false;
    for g in groups {
        let items = &frame.items[g.start..g.end];
        let rect = items.iter().fold(Rect::new(0, 0, 0, 0), |r, it| r.union(&reach(it))).intersect(&screen);
        if rect.is_empty() {
            continue;
        }
        let same_size = |p: &Plane| p.key == g.key && (p.rect.w, p.rect.h) == (rect.w, rect.h);
        let Some(mut p) = old.iter().position(same_size).map(|i| old.swap_remove(i)).or_else(|| Plane::new(g.key, rect))
        else {
            failed = true;
            break;
        };
        if p.rect != rect {
            p.rect = rect;
            p.lacks = [alloc::vec![rect], alloc::vec![rect]];
        }
        let back = 1 - p.front;
        let new: Vec<Rect> = damage.iter().map(|d| d.intersect(&rect)).filter(|d| !d.is_empty()).collect();
        let mut paint = core::mem::take(&mut p.lacks[back]);
        paint.extend_from_slice(&new);
        if !paint.is_empty() {
            let mut sub = Vec::with_capacity(items.len() + 1);
            sub.push(Item::Background);
            sub.extend_from_slice(items);
            scene::execute_layer(&Scene { items: sub }, sources, target, info, &paint);
            copy_out(target, info, &p, back, &paint);
            painted.extend_from_slice(&paint);
            p.lacks[p.front].extend_from_slice(&new);
            merge(&mut p.lacks[p.front], rect);
            p.front = back;
        }
        let bo = p.bo(p.front);
        planes.push(p);
        let Some(bo) = bo else {
            failed = true;
            break;
        };
        layers.push(gpudev::Layer { bo, sx: 0, sy: 0, w: rect.w, h: rect.h, x: rect.x as i32, y: rect.y as i32,
            premultiplied: true });
    }
    if failed {
        // What is on screen stays until the screen replaces it.
        planes.append(&mut old);
        st.planes = planes;
        drop(st);
        return leave();
    }
    st.planes = planes;
    if on_screen() && layers == st.shown {
        return Composed::Layers(painted);
    }

    gpudev::set_layers_shown(true);
    let Some(seq) = gpudev::submit_layers(&layers) else {
        gpudev::set_layers_shown(on_screen());
        st.planes.append(&mut old);
        drop(st);
        return leave();
    };
    st.pending = Some((seq, old));
    st.shown = layers;
    ON_SCREEN.store(true, Ordering::Release);
    Composed::Layers(painted)
}

/// Wait for the commit in flight; the planes it no longer shows go. A
/// refused commit turns layers off: the next frame composes the screen.
pub fn settle() {
    let Some((seq, gone)) = STATE.lock().pending.take() else { return };
    let status = gpudev::wait_layers(seq);
    drop(gone);
    if status != 0 {
        crate::kprintln!("[npk] layers: the display refused the commit ({}), composing the screen", status);
        OFF.store(true, Ordering::Release);
        crate::shade::request_render();
    }
}

/// No layers for this frame. Leaving them, the screen is composed whole;
/// their buffers go with the next frame (`FORGET`).
fn leave() -> Composed {
    if !on_screen() {
        return Composed::Screen { whole: false };
    }
    ON_SCREEN.store(false, Ordering::Release);
    gpudev::set_layers_shown(false);
    FORGET.store(true, Ordering::Release);
    Composed::Screen { whole: true }
}

/// The desktop goes (`shade::stop`): the commit in flight is waited for,
/// and the console's next copy shows a shadow instead of the layers.
pub fn stop() {
    settle();
    leave();
}

/// The layers' buffers go, once the screen is presented again in their
/// place (after `stop`).
pub fn release() {
    if on_screen() {
        return;
    }
    FORGET.store(false, Ordering::Release);
    let mut st = STATE.lock();
    st.planes.clear();
    st.shown.clear();
}

/// Layers again (`gpu layers on`).
pub fn allow() {
    OFF.store(false, Ordering::Release);
}

/// No layers until `allow` (`gpu layers off`).
pub fn forbid() {
    OFF.store(true, Ordering::Release);
}

/// Where item `it` may paint.
fn reach(it: &Item) -> Rect {
    match *it {
        Item::Background => Rect::new(0, 0, 0, 0),
        Item::Glow { rect, band, .. } => rect.grow(band),
        Item::GlassBackdrop { rect, .. } | Item::Chrome { rect, .. } | Item::Fill { rect, .. }
        | Item::Glyph { rect, .. } => rect,
        Item::Content { dst, .. } | Item::Panel { dst, .. } | Item::Text { dst, .. } => dst,
    }
}

/// Rectangles `rs` of plane `p`'s buffer `i` from the screen-sized `target`.
fn copy_out(target: *mut u8, info: &FbInfo, p: &Plane, i: usize, rs: &[Rect]) {
    let buf = &p.bufs[i];
    let dst = buf.ptr();
    let dpitch = buf.width() as usize * 4;
    let (mut y0, mut y1) = (u32::MAX, 0);
    for r in rs {
        let r = r.intersect(&p.rect);
        if r.is_empty() {
            continue;
        }
        (y0, y1) = (y0.min(r.y - p.rect.y), y1.max(r.y1() - p.rect.y));
        for y in r.y..r.y1() {
            let from = y as usize * info.pitch as usize + r.x as usize * 4;
            let to = (y - p.rect.y) as usize * dpitch + (r.x - p.rect.x) as usize * 4;
            // SAFETY: `r` lies within the screen, which `target` holds with
            // `info.pitch`, and within the plane, whose buffer holds its
            // `rect.w` x `rect.h` pixels at `dpitch`; the two do not overlap.
            unsafe { core::ptr::copy_nonoverlapping(target.add(from), dst.add(to), r.w as usize * 4) };
        }
    }
    if y1 > y0 && let Some(bo) = p.bo(i) {
        gpudev::sync(bo, Some((y0, y1)));
    }
}

/// `rs` as one rectangle once it holds more than `MAX_LACKS`.
fn merge(rs: &mut Vec<Rect>, within: Rect) {
    if rs.len() > MAX_LACKS {
        let all = rs.iter().fold(Rect::new(0, 0, 0, 0), |a, r| a.union(r)).intersect(&within);
        rs.clear();
        rs.push(all);
    }
}
