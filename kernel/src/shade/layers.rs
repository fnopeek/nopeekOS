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
//! The wallpaper is shown only where it can be seen: the display reads
//! every plane whole each frame, and under a window that paints its
//! layer opaquely (its content, its glass over the blurred wallpaper) the
//! wallpaper would be read for nothing. What no layer covers opaquely
//! goes as a few strips (`uncovered`), each a copy of its part of the
//! wallpaper in a buffer of its own: the HVS's line memory goes by a
//! buffer's pitch, so a narrow gap costs a narrow row. A strip is copied
//! once per layout and wallpaper. With too many strips, or a commit the
//! display refuses with them, the wallpaper is one plane again.
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
/// Layer scenes painted both ways and compared after `gpu verify`.
const CHECKS: u32 = 24;
/// Compared layer scenes in a row that differ before the GPU stops: a
/// source the CPU read can change before the GPU reads it, a wrong
/// program differs every time.
const DIFFS_IN_ROW: u32 = 2;
/// Wallpaper strips at most.
const MAX_STRIPS: usize = 10;
/// The HVS's line memory the planes may take (vc4_hvs.c: 1024 words of
/// HVS_UBM_WORD_SIZE), less what the cursor plane takes; half of it, as a
/// commit checks the new planes while the ones on screen still hold
/// theirs (vc4_plane_duplicate_state).
const UPM_BYTES: u32 = (1024 * 256 - 4096) / 2;
/// Planes the display has for the desktop: the primary and its overlays.
const MAX_PLANES: usize = MAX_GROUPS + 1;
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
    /// The commit shows the wallpaper in strips.
    strips: bool,
    /// The planes and strips the commit no longer shows.
    gone: Vec<Plane>,
    gone_strips: Vec<Strip>,
    /// The screen-sized buffer the CPU paints in.
    target: usize,
    info: FbInfo,
}

/// A strip of the wallpaper: where, of which wallpaper, its copy.
struct Strip {
    rect: Rect,
    generation: u32,
    buf: PixelBuffer,
}

impl Strip {
    fn bo(&self) -> Option<u32> {
        let b = &self.buf;
        gpudev::bo_for(b.phys(), b.width() as u64 * b.height() as u64 * 4, b.width(), b.height(), b.width() * 4)
    }
}

struct State {
    planes: Vec<Plane>,
    strips: Vec<Strip>,
    pending: Option<Pending>,
    /// The layers last committed.
    shown: Vec<gpudev::Layer>,
}

static STATE: Mutex<State> = Mutex::new(State { planes: Vec::new(), strips: Vec::new(), pending: None,
    shown: Vec::new() });
/// The display shows layers, not a shadow.
static ON_SCREEN: AtomicBool = AtomicBool::new(false);
/// Layers stay off: a commit was refused.
static OFF: AtomicBool = AtomicBool::new(false);
/// The wallpaper stays one plane: a commit with strips was refused.
static WHOLE_WALL: AtomicBool = AtomicBool::new(false);
/// The first commit with strips was reported.
static STRIPS_SAID: AtomicBool = AtomicBool::new(false);
/// The planes go with the next frame: the screen's flip that replaces
/// them is queued by then.
static FORGET: AtomicBool = AtomicBool::new(false);
/// Layer scenes still to compare (`CHECKS`).
static CHECKS_LEFT: AtomicU32 = AtomicU32::new(0);
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
        st.strips.clear();
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
    let mut layers = Vec::with_capacity(groups.len());
    let mut opaque = Vec::with_capacity(groups.len());
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
        if let Some(o) = opaque_part(items, sources) {
            opaque.push(o.intersect(&rect));
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
    // The wallpaper beneath: in strips where no layer covers it, or whole.
    let mut rects = uncovered(screen, &opaque);
    let windows_upm: u32 = layers.iter().map(|l| upm(l.w * 4)).sum();
    let strips_upm = |rs: &[Rect]| rs.iter().map(|r| upm(r.w * 4)).sum::<u32>();
    while rects.len() > 1 && (rects.len() > MAX_STRIPS || rects.len() + layers.len() > MAX_PLANES
        || windows_upm + strips_upm(&rects) > UPM_BYTES)
    {
        join_cheapest(&mut rects);
    }
    let generation = crate::gui::background::wallpaper_generation();
    let mut kept = core::mem::take(&mut st.strips);
    let mut strips = Vec::new();
    let mut in_strips = !WHOLE_WALL.load(Ordering::Acquire) && rects.len() + layers.len() <= MAX_PLANES
        && windows_upm + strips_upm(&rects) <= UPM_BYTES && !(rects.is_empty() && layers.is_empty());
    let mut all = Vec::with_capacity(layers.len() + rects.len().max(1));
    if in_strips {
        for r in &rects {
            let found = kept.iter().position(|s| s.rect == *r && s.generation == generation).map(|i| kept.swap_remove(i));
            let Some(s) = found.or_else(|| strip(wall, info, *r, generation)) else {
                in_strips = false;
                break;
            };
            let Some(bo) = s.bo() else {
                strips.push(s);
                in_strips = false;
                break;
            };
            all.push(gpudev::Layer { bo, sx: 0, sy: 0, w: r.w, h: r.h, x: r.x as i32, y: r.y as i32,
                premultiplied: false });
            strips.push(s);
        }
    }
    if !in_strips {
        all.clear();
        all.push(gpudev::Layer { bo: wall_bo, sx: 0, sy: 0, w: info.width, h: info.height, x: 0, y: 0,
            premultiplied: false });
        kept.append(&mut strips);
    }
    st.strips = strips;
    all.append(&mut layers);
    let commit = (!on_screen() || all != st.shown).then_some(all);
    if commit.is_none() && jobs.is_empty() {
        // The strips not shown were not shown before either.
        drop(kept);
        return Composed::Layers(painted);
    }
    st.pending = Some(Pending { jobs, commit, strips: in_strips, gone: old, gone_strips: kept, target: target as usize,
        info: *info });
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
        let status = crate::shade::gpu_scene::layer_done(&j.scene, seq, bound, j.bo, j.layer, &j.paint);
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
            if p.strips && !STRIPS_SAID.swap(true, Ordering::Relaxed) {
                let n = STATE.lock().strips.len();
                crate::kprintln!("[npk] layers: the wallpaper in {} strips where it shows", n);
            }
            STATE.lock().shown = layers;
        } else if p.strips {
            crate::kprintln!("[npk] layers: the display refused the wallpaper in strips ({}), showing it whole",
                status);
            WHOLE_WALL.store(true, Ordering::Release);
            crate::shade::request_render();
        } else {
            crate::kprintln!("[npk] layers: the display refused the commit ({}), composing the screen", status);
            OFF.store(true, Ordering::Release);
            crate::shade::request_render();
        }
    }
    drop(p.gone);
    drop(p.gone_strips);
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
    st.strips.clear();
    st.shown.clear();
}

/// Layers again (`gpu layers on`), the wallpaper in strips again.
pub fn allow() {
    OFF.store(false, Ordering::Release);
    WHOLE_WALL.store(false, Ordering::Release);
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

/// The largest rectangle of a group's layer whose every pixel its items
/// set (items.glsl `SET`, `render::opaque`): a content item with pixels
/// to show, the glass backdrop over the blurred wallpaper, an opaque
/// fill, without their corner rows and a pixel in from each edge, where
/// coverage takes off; and the inside of a chrome that paints its content
/// opaquely or as glass over the blurred wallpaper (items.glsl `chrome`'s
/// straight rows: the rows off its inner corners, two pixels in from its
/// border).
fn opaque_part(items: &[Item], sources: &dyn Sources) -> Option<Rect> {
    let blurred = crate::gui::background::blurred().is_some();
    let inset = |r: Rect, radius: u32| {
        let rad = radius.min(r.w / 2).min(r.h / 2) + 1;
        (r.w > 2 && r.h > 2 * rad).then(|| Rect::new(r.x + 1, r.y + rad, r.w - 2, r.h - 2 * rad))
    };
    let mut best: Option<Rect> = None;
    for it in items {
        let o = match *it {
            Item::Content { src, dst, radius, .. } => {
                let mut shows = false;
                sources.with_source(src, &mut |px, w, h| shows = !px.is_empty() && w > 0 && h > 0);
                if shows { inset(dst, radius) } else { None }
            }
            Item::GlassBackdrop { rect, radius } if blurred => inset(rect, radius),
            Item::Fill { rect, alpha, radius, .. } if alpha >= 255 => inset(rect, radius),
            Item::Chrome { rect, rounding, border, bg_opacity, paint_content: true, .. }
                if blurred || bg_opacity >= 255 => chrome_inside(rect, rounding, border),
            _ => None,
        };
        if let Some(o) = o
            && best.is_none_or(|b| o.w as u64 * o.h as u64 > b.w as u64 * b.h as u64)
        {
            best = Some(o);
        }
    }
    best
}

/// Part `r` of the wallpaper at `wall` (the screen's size, `info`) copied
/// into a strip of its own, out of the CPU caches.
fn strip(wall: *const u8, info: &FbInfo, r: Rect, generation: u32) -> Option<Strip> {
    let buf = PixelBuffer::new(r.w, r.h);
    if buf.is_empty() {
        return None;
    }
    let dst = buf.ptr();
    for y in r.y..r.y1() {
        // SAFETY: `r` lies within the screen, whose pixels the wallpaper
        // holds at `info.pitch`; the strip holds `r.w` x `r.h` at `r.w * 4`.
        unsafe {
            core::ptr::copy_nonoverlapping(wall.add(y as usize * info.pitch as usize + r.x as usize * 4),
                dst.add((y - r.y) as usize * r.w as usize * 4), r.w as usize * 4);
        }
    }
    let s = Strip { rect: r, generation, buf };
    let bo = s.bo()?;
    gpudev::sync(bo, None);
    Some(s)
}

/// The line memory the HVS gives a plane whose buffer has `pitch` bytes a
/// row (vc6_upm_size, two lines, SCALER6_PTR0_UPM_BUFF_SIZE_2_LINES).
fn upm(pitch: u32) -> u32 {
    (((pitch + 62) / 32) * 32 * 2).next_multiple_of(256)
}

/// The two strips whose bounding box adds the least area, as that box:
/// the wallpaper beneath shows the same either way, read a little more.
fn join_cheapest(rs: &mut Vec<Rect>) {
    let area = |r: &Rect| r.w as u64 * r.h as u64;
    let mut best = (u64::MAX, 0, 1);
    for i in 0..rs.len() {
        for k in i + 1..rs.len() {
            let grown = area(&rs[i].union(&rs[k])).saturating_sub(area(&rs[i]) + area(&rs[k]));
            if grown < best.0 {
                best = (grown, i, k);
            }
        }
    }
    let (_, i, k) = best;
    let joined = rs[i].union(&rs[k]);
    rs.swap_remove(k);
    rs[i] = joined;
}

/// The straight rows of a chrome item's inside (render::fill_rounded_chrome_aa
/// `straight_lo..straight_hi`, `skip_lo..skip_hi`).
fn chrome_inside(r: Rect, rounding: u32, border: u32) -> Option<Rect> {
    if r.w < 2 || r.h < 2 {
        return None;
    }
    let r_out = rounding.min(r.w / 2).min(r.h / 2);
    let b = border.min(r_out).min(r.w / 2).min(r.h / 2);
    let (iw, ih) = (r.w - 2 * b, r.h - 2 * b);
    let r_in = r_out.saturating_sub(b).min(iw / 2).min(ih / 2);
    (iw > 4 && ih > 2 * r_in).then(|| Rect::new(r.x + b + 2, r.y + b + r_in, iw - 4, ih - 2 * r_in))
}

/// What of `screen` none of `opaque` covers, as disjoint rectangles: the
/// bands between their top and bottom edges, each band's uncovered spans,
/// a span joined with the same span of the band above.
fn uncovered(screen: Rect, opaque: &[Rect]) -> Vec<Rect> {
    let opaque: Vec<Rect> = opaque.iter().map(|o| o.intersect(&screen)).filter(|o| !o.is_empty()).collect();
    let mut ys: Vec<u32> = alloc::vec![screen.y, screen.y1()];
    for o in &opaque {
        ys.push(o.y);
        ys.push(o.y1());
    }
    ys.sort_unstable();
    ys.dedup();
    let mut out = Vec::new();
    let mut open: Vec<Rect> = Vec::new();
    for b in ys.windows(2) {
        let (y0, y1) = (b[0], b[1]);
        let mut cover: Vec<(u32, u32)> = opaque.iter().filter(|o| o.y <= y0 && o.y1() >= y1).map(|o| (o.x, o.x1())).collect();
        cover.sort_unstable();
        let mut spans = Vec::new();
        let mut x = screen.x;
        for (a, e) in cover {
            if a > x {
                spans.push((x, a));
            }
            x = x.max(e);
        }
        if x < screen.x1() {
            spans.push((x, screen.x1()));
        }
        let mut next = Vec::with_capacity(spans.len());
        for (a, e) in spans {
            match open.iter().position(|r| r.x == a && r.w == e - a) {
                Some(i) => {
                    let mut r = open.swap_remove(i);
                    r.h += y1 - y0;
                    next.push(r);
                }
                None => next.push(Rect::new(a, y0, e - a, y1 - y0)),
            }
        }
        out.append(&mut open);
        open = next;
    }
    out.append(&mut open);
    out
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
