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
    /// Painted both ways and compared.
    check: bool,
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
        merge(&mut paint, rect);
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
            let check = gpu.is_some() && CHECKS_LEFT.load(Ordering::Relaxed) > 0;
            if gpu.is_none() {
                scene::execute_layer(&sub, sources, target, info, &paint);
                copy_out(target, info, p.bufs[back].ptr(), rect.w, rect, &paint, bo);
            }
            let b = &p.bufs[back];
            jobs.push(Job { gpu, check, bo, pixels: b.phys(), width: b.width(), layer: rect, paint: paint.clone(),
                scene: sub });
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
        if status != 0 || j.check {
            cpu.push((i, status == 0));
        }
    }
    if !cpu.is_empty() {
        let run = |sources: &dyn Sources| {
            for &(i, painted) in &cpu {
                repaint(&p, &p.jobs[i], painted, sources);
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

/// Job `j` on the CPU: in place of a scene the GPU failed, or beside one
/// it `painted`, to compare. A difference turns the GPU off and leaves
/// the CPU's pixels in the buffer.
fn repaint(p: &Pending, j: &Job, painted: bool, sources: &dyn Sources) {
    let target = p.target as *mut u8;
    scene::execute_layer(&j.scene, sources, target, &p.info, &j.paint);
    let buf = j.pixels as *mut u8;
    if painted {
        let (diffs, first) = compare(target, &p.info, buf, j.width, j.layer, &j.paint);
        if diffs == 0 {
            if CHECKS_LEFT.try_update(Ordering::Relaxed, Ordering::Relaxed, |v| v.checked_sub(1)) == Ok(1) {
                crate::kprintln!("[npk] gpu: {} layer scenes painted as the CPU does, the GPU paints them alone",
                    CHECKS);
            }
            return;
        }
        crate::shade::gpu_scene::layer_differs(&j.scene, j.layer, diffs, first);
    }
    copy_out(target, &p.info, buf, j.width, j.layer, &j.paint, j.bo);
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
    if y1 > y0 {
        gpudev::sync(bo, Some((y0, y1)));
    }
}

/// Pixels of `rs` that differ between the screen-sized `target` and the
/// layer's buffer, and the first: where, the CPU's word, the GPU's.
fn compare(target: *mut u8, info: &FbInfo, buf: *mut u8, width: u32, layer: Rect, rs: &[Rect])
           -> (usize, Option<(u32, u32, u32, u32)>) {
    let mut diffs = 0;
    let mut first = None;
    for r in rs {
        let r = r.intersect(&layer);
        for y in r.y..r.y1() {
            for x in r.x..r.x1() {
                // SAFETY: (x, y) lies within the screen and the layer, as in
                // `copy_out`.
                let (c, g) = unsafe {
                    (*(target.add(y as usize * info.pitch as usize + x as usize * 4) as *const u32),
                     *(buf.add((y - layer.y) as usize * width as usize * 4 + (x - layer.x) as usize * 4) as *const u32))
                };
                if c != g {
                    diffs += 1;
                    first.get_or_insert((x, y, c, g));
                }
            }
        }
    }
    (diffs, first)
}

/// `rs` as one rectangle once it holds more than `MAX_LACKS`.
fn merge(rs: &mut Vec<Rect>, within: Rect) {
    if rs.len() > MAX_LACKS {
        let all = rs.iter().fold(Rect::new(0, 0, 0, 0), |a, r| a.union(r)).intersect(&within);
        rs.clear();
        rs.push(all);
    }
}
