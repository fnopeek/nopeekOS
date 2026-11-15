//! The desktop as layers on the display's planes (gpudev `CAP_LAYERS`).
//!
//! With a module that shows layers, the screen is no longer composed as a
//! whole: the wallpaper is the bottom layer as it is, and each group of
//! the scene (a window with its glow, chrome, content and controls; the
//! overlays above all windows) is painted into a buffer of its own, over
//! a transparent ground, premultiplied ARGB. The display mixes them, so a
//! window that does not change costs nothing and one that does is painted
//! again only within its damage.
//!
//! Each layer has two buffers, the one on screen and the one painted next;
//! the next one also takes what the last frame painted into the other
//! (buffer age 2). A layer that moves or changes size is painted whole,
//! as glass shows the wallpaper at its place.
//!
//! The GPU paints a layer as a scene of its own (`gpu_scene::submit_layer`).
//! Where it does not compose, the CPU paints the layer into the
//! screen-sized buffer the frame was handed (a shadow, not on screen while
//! layers are; `scene::execute_layer`) and copies its damage out. The
//! first layer scenes are painted both ways and compared, as whole frames
//! of the screen are.
//!
//! A frame happens in two steps. `compose` hands the scenes over; `settle`,
//! with the compositor released as for a scene (`gpu_scene::finish`),
//! waits for them, paints on the CPU what the GPU did not, and only then
//! commits the layers and waits for the flip, so the display never shows
//! a buffer half painted. The buffers of a layer that went stay until a
//! commit without them is done. More groups than the display has
//! overlays, no wallpaper buffer, a buffer that cannot be had, a commit
//! the module refused: the screen is composed as before, whole.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
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
/// Layer scenes painted both ways and compared before the GPU's alone count.
const CHECKS: u32 = 24;
/// Compared layer scenes in a row that differ before the GPU stops: a
/// source the CPU read can change before the GPU reads it, a wrong
/// program differs every time.
const DIFFS_IN_ROW: u32 = 2;
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
        let p = Plane { key, rect, bufs: [a, b], front: 0, lacks: [alloc::vec![rect], alloc::vec![rect]] };
        // The zeroes the CPU wrote go to memory before the GPU writes there.
        for i in 0..2 {
            if let Some(bo) = p.bo(i) {
                gpudev::sync(bo, None);
            }
        }
        Some(p)
    }

    fn bo(&self, i: usize) -> Option<u32> {
        let b = &self.bufs[i];
        gpudev::bo_for_format(b.phys(), b.width() as u64 * b.height() as u64 * 4, b.width(), b.height(),
            b.width() * 4, gpudev::FORMAT_ARGB8888)
    }
}

/// One layer painted for the commit: by the GPU (`seq`), or already by
/// the CPU. Its buffer stays with its plane until `settle`.
struct Job {
    gpu: Option<(u32, bool)>,
    /// What the CPU painted when the scene was handed over, to compare.
    reference: Option<PixelBuffer>,
    bo: u32,
    /// The buffer's first pixel, its width in pixels.
    pixels: u64,
    width: u32,
    layer: Rect,
    paint: Vec<Rect>,
    scene: Scene,
}

/// A frame handed over and not yet committed.
struct Pending {
    jobs: Vec<Job>,
    /// The layers to commit, None when they are on screen already.
    commit: Option<Vec<gpudev::Layer>>,
    /// The planes the commit no longer shows.
    gone: Vec<Plane>,
    /// The screen-sized buffer the CPU paints in.
    target: usize,
    info: FbInfo,
}

struct State {
    planes: Vec<Plane>,
    pending: Option<Pending>,
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
/// Layer scenes still to compare (`CHECKS`).
static CHECKS_LEFT: AtomicU32 = AtomicU32::new(CHECKS);
/// Compared layer scenes in a row that differed.
static DIFFS: AtomicU32 = AtomicU32::new(0);

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
    settle_with(Some(sources));
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
    let mut jobs = Vec::new();
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
        normalize(&mut paint, rect);
        if !paint.is_empty() {
            let Some(bo) = p.bo(back) else {
                planes.push(p);
                failed = true;
                break;
            };
            let mut sub = Vec::with_capacity(items.len() + 1);
            sub.push(Item::Background);
            sub.extend_from_slice(items);
            let sub = Scene { items: sub };
            let gpu = crate::shade::gpu_scene::submit_layer(&sub, sources, info, bo, rect, &paint);
            let mut reference = None;
            if gpu.is_none() {
                scene::execute_layer(&sub, sources, target, info, &paint);
                copy_out(target, info, p.bufs[back].ptr(), rect.w, rect, &paint, bo);
            } else if CHECKS_LEFT.load(Ordering::Relaxed) > 0 {
                // The CPU's pixels now, from the sources the scene was
                // handed with, as the screen's verification paints them.
                let r = PixelBuffer::new(rect.w, rect.h);
                if !r.is_empty() {
                    scene::execute_layer(&sub, sources, target, info, &paint);
                    copy_rows(target, info, r.ptr(), rect.w, rect, &paint);
                    reference = Some(r);
                }
            }
            let b = &p.bufs[back];
            jobs.push(Job { gpu, reference, bo, pixels: b.phys(), width: b.width(), layer: rect,
                paint: paint.clone(), scene: sub });
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
        // What is on screen stays until the screen replaces it; scenes
        // handed over are waited for before their buffers can go.
        for j in &jobs {
            if let Some((seq, bound)) = j.gpu {
                let _ = if bound { gpudev::wait_scene_bound(seq) } else { gpudev::wait_scene(seq) };
            }
        }
        planes.append(&mut old);
        st.planes = planes;
        drop(st);
        return leave();
    }
    st.planes = planes;
    let commit = (!on_screen() || layers != st.shown).then_some(layers);
    if commit.is_none() && jobs.is_empty() {
        return Composed::Layers(painted);
    }
    st.pending = Some(Pending { jobs, commit, gone: old, target: target as usize, info: *info });
    ON_SCREEN.store(true, Ordering::Release);
    Composed::Layers(painted)
}

/// The frame handed over: its scenes waited for, painted on the CPU where
/// the GPU did not (with the compositor's sources, which it takes), then
/// the layers committed and the flip waited for. A refused commit turns
/// layers off: the next frame composes the screen.
pub fn settle() {
    settle_with(None);
}

fn settle_with(sources: Option<&dyn Sources>) {
    let Some(p) = STATE.lock().pending.take() else { return };
    let mut cpu = Vec::new();
    for (i, j) in p.jobs.iter().enumerate() {
        let Some((seq, bound)) = j.gpu else { continue };
        let status = crate::shade::gpu_scene::layer_done(seq, bound, j.bo, j.layer, &j.paint);
        match &j.reference {
            Some(r) if status == 0 => judge(j, r),
            _ if status != 0 => cpu.push(i),
            _ => {}
        }
    }
    if !cpu.is_empty() {
        let run = |sources: &dyn Sources| {
            for &i in &cpu {
                let j = &p.jobs[i];
                let target = p.target as *mut u8;
                scene::execute_layer(&j.scene, sources, target, &p.info, &j.paint);
                copy_out(target, &p.info, j.pixels as *mut u8, j.width, j.layer, &j.paint, j.bo);
            }
        };
        match sources {
            Some(s) => run(s),
            None => {
                crate::shade::with_compositor(|comp| run(&*comp));
            }
        }
    }
    if let Some(layers) = p.commit {
        gpudev::set_layers_shown(true);
        let status = match gpudev::submit_layers(&layers) {
            Some(seq) => gpudev::wait_layers(seq),
            None => -1,
        };
        if status == 0 {
            STATE.lock().shown = layers;
        } else {
            crate::kprintln!("[npk] layers: the display refused the commit ({}), composing the screen", status);
            OFF.store(true, Ordering::Release);
            crate::shade::request_render();
        }
    }
    drop(p.gone);
}

/// A layer scene the GPU painted against the CPU's `reference`. Where they
/// differ the CPU's pixels go into the buffer; `DIFFS_IN_ROW` such scenes
/// in a row turn the GPU off.
fn judge(j: &Job, reference: &PixelBuffer) {
    let buf = j.pixels as *mut u8;
    let (diffs, first) = compare(reference.ptr(), buf, j.width, j.layer, &j.paint);
    if diffs == 0 {
        DIFFS.store(0, Ordering::Relaxed);
        if CHECKS_LEFT.try_update(Ordering::Relaxed, Ordering::Relaxed, |v| v.checked_sub(1)) == Ok(1) {
            crate::kprintln!("[npk] gpu: {} layer scenes painted as the CPU does, the GPU paints them alone", CHECKS);
        }
        return;
    }
    let row = j.width as usize * 4;
    for r in &j.paint {
        let r = r.intersect(&j.layer);
        for y in r.y..r.y1() {
            let at = (y - j.layer.y) as usize * row + (r.x - j.layer.x) as usize * 4;
            // SAFETY: both buffers hold the layer's rows at `row` bytes and
            // `r` lies within the layer; they are distinct allocations.
            unsafe { core::ptr::copy_nonoverlapping(reference.ptr().add(at), buf.add(at), r.w as usize * 4) };
        }
    }
    for r in &j.paint {
        let r = r.intersect(&j.layer);
        if !r.is_empty() {
            gpudev::sync(j.bo, Some((r.y - j.layer.y, r.y1() - j.layer.y)));
        }
    }
    if DIFFS.fetch_add(1, Ordering::Relaxed) + 1 >= DIFFS_IN_ROW {
        crate::shade::gpu_scene::layer_differs(&j.scene, j.layer, diffs, first);
    } else {
        crate::kprintln!("[npk] gpu: layer at {:?} differs from the CPU's in {} pixels, a source may have \
            changed meanwhile; comparing the next", j.layer, diffs);
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

/// The desktop goes (`shade::stop`): the frame in flight is settled, and
/// the console's next copy shows a shadow instead of the layers.
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

/// `gpu verify`: compare the next layer scenes again.
pub fn verify_again() {
    CHECKS_LEFT.store(CHECKS, Ordering::Relaxed);
    DIFFS.store(0, Ordering::Relaxed);
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

/// Rectangles `rs` of the layer at `layer` from the screen-sized `target`
/// into its buffer `buf` (`width` pixels a row, buffer `bo`), then out of
/// the CPU caches for the display.
fn copy_out(target: *mut u8, info: &FbInfo, buf: *mut u8, width: u32, layer: Rect, rs: &[Rect], bo: u32) {
    if let Some((y0, y1)) = copy_rows(target, info, buf, width, layer, rs) {
        gpudev::sync(bo, Some((y0, y1)));
    }
}

/// `copy_out` without the caches: the layer's rows it wrote, if any.
fn copy_rows(target: *mut u8, info: &FbInfo, buf: *mut u8, width: u32, layer: Rect, rs: &[Rect])
             -> Option<(u32, u32)> {
    let dpitch = width as usize * 4;
    let (mut y0, mut y1) = (u32::MAX, 0);
    for r in rs {
        let r = r.intersect(&layer);
        if r.is_empty() {
            continue;
        }
        (y0, y1) = (y0.min(r.y - layer.y), y1.max(r.y1() - layer.y));
        for y in r.y..r.y1() {
            let from = y as usize * info.pitch as usize + r.x as usize * 4;
            let to = (y - layer.y) as usize * dpitch + (r.x - layer.x) as usize * 4;
            // SAFETY: `r` lies within the screen, which `target` holds with
            // `info.pitch`, and within the layer, whose buffer holds its
            // `layer.w` x `layer.h` pixels at `dpitch`; the two do not overlap.
            unsafe { core::ptr::copy_nonoverlapping(target.add(from), buf.add(to), r.w as usize * 4) };
        }
    }
    (y1 > y0).then_some((y0, y1))
}

/// Pixels of `rs` that differ between two buffers of the layer at
/// `layer` (`width` pixels a row), and the first: where on the screen,
/// `a`'s word, `b`'s.
fn compare(a: *mut u8, b: *mut u8, width: u32, layer: Rect, rs: &[Rect]) -> (usize, Option<(u32, u32, u32, u32)>) {
    let mut diffs = 0;
    let mut first = None;
    for r in rs {
        let r = r.intersect(&layer);
        for y in r.y..r.y1() {
            for x in r.x..r.x1() {
                let at = (y - layer.y) as usize * width as usize * 4 + (x - layer.x) as usize * 4;
                // SAFETY: (x, y) lies within the layer, whose pixels both
                // buffers hold at `width` pixels a row.
                let (c, g) = unsafe { (*(a.add(at) as *const u32), *(b.add(at) as *const u32)) };
                if c != g {
                    diffs += 1;
                    first.get_or_insert((x, y, c, g));
                }
            }
        }
    }
    (diffs, first)
}

/// The rectangles to paint: each once, none inside another; the whole
/// layer when one covers it or there are many.
fn normalize(rs: &mut Vec<Rect>, within: Rect) {
    let inside = |o: &Rect, r: &Rect| o.x <= r.x && o.y <= r.y && o.x1() >= r.x1() && o.y1() >= r.y1();
    let mut out: Vec<Rect> = Vec::with_capacity(rs.len());
    for (i, r) in rs.iter().enumerate() {
        let r = r.intersect(&within);
        if r.is_empty() || out.contains(&r) {
            continue;
        }
        if rs.iter().enumerate().any(|(k, o)| k != i && *o != r && inside(&o.intersect(&within), &r)) {
            continue;
        }
        out.push(r);
    }
    let area: u64 = out.iter().map(|r| r.w as u64 * r.h as u64).sum();
    if out.iter().any(|r| *r == within) || out.len() > MAX_LACKS || area >= within.w as u64 * within.h as u64 {
        out.clear();
        out.push(within);
    }
    *rs = out;
}

/// `rs` as one rectangle once it holds more than `MAX_LACKS`.
fn merge(rs: &mut Vec<Rect>, within: Rect) {
    if rs.len() > MAX_LACKS {
        let all = rs.iter().fold(Rect::new(0, 0, 0, 0), |a, r| a.union(r)).intersect(&within);
        rs.clear();
        rs.push(all);
    }
}
