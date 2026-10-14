//! Scene lists on the GPU (docs/plan/GPU_STACK.md §3.3 v1, §5.4 A5).
//!
//! When the GPU module offers scenes, a frame's scene list is encoded with
//! its sources named by gpudev buffer handles and composed by the module's
//! programs. Before that happens for real, one whole frame is composed both
//! ways — the CPU into the target, the GPU into a copy of it — and compared
//! bit for bit; only an identical result switches the compositor over. A
//! scene the module rejects or does not finish in time is composed on the
//! CPU, and a run of them switches back to the CPU for good (until
//! `gpu verify`).

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering};

use crate::framebuffer::FbInfo;
use crate::gpu::gpudev;
use crate::shade::scene::{self, Fit, Item, Rect, Scene, Source, Sources};

const MAGIC: u32 = 0x314E_4353;
const HEADER: usize = 64;
const ITEM: usize = 64;
const NONE: u32 = u32::MAX;
const FLAG_WALLPAPER: u32 = 1;
const FLAG_BLURRED: u32 = 2;
/// Scenes in a row the module failed before the CPU takes over again.
const FAILURE_LIMIT: u32 = 8;
/// Differing pixels the verification lists.
const DIFFS_SHOWN: usize = 8;

const MODE_UNTESTED: u8 = 0;
const MODE_ON: u8 = 1;
const MODE_OFF: u8 = 2;
static MODE: AtomicU8 = AtomicU8::new(MODE_UNTESTED);
static FAILURES_IN_ROW: AtomicU32 = AtomicU32::new(0);

static SCENES_DONE: AtomicU64 = AtomicU64::new(0);
static SCENES_FAILED: AtomicU64 = AtomicU64::new(0);
static CPU_SCENES: AtomicU64 = AtomicU64::new(0);
/// Whole frames composed, and the TSC cycles they took: `[GPU, CPU]`.
static FULL_FRAMES: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];
static FULL_CYCLES: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];

/// Compose `frame` into `target` within `damage`, on the GPU when it is
/// verified and takes the job, else on the CPU.
pub fn compose(frame: &Scene, sources: &dyn Sources, target: *mut u8, info: &FbInfo, damage: &[Rect]) {
    let t0 = crate::interrupts::rdtsc();
    let full = covers_screen(damage, info);
    let note = |i: usize| if full {
        FULL_FRAMES[i].fetch_add(1, Ordering::Relaxed);
        FULL_CYCLES[i].fetch_add(crate::interrupts::rdtsc() - t0, Ordering::Relaxed);
    };
    if gpudev::scenes_offered() {
        match MODE.load(Ordering::Acquire) {
            MODE_ON => {
                if on_gpu(frame, sources, target, info, damage) {
                    note(0);
                    return;
                }
            }
            MODE_UNTESTED if full => {
                verify(frame, sources, target, info, damage);
                return;
            }
            _ => {}
        }
    }
    scene::execute(frame, sources, target, info, damage);
    CPU_SCENES.fetch_add(1, Ordering::Relaxed);
    note(1);
}

fn covers_screen(damage: &[Rect], info: &FbInfo) -> bool {
    damage.iter().any(|d| d.x == 0 && d.y == 0 && d.w >= info.width && d.h >= info.height)
}

/// One scene on the GPU into `target`, waited for. False when it did not
/// happen; the caller composes on the CPU.
fn on_gpu(frame: &Scene, sources: &dyn Sources, target: *mut u8, info: &FbInfo, damage: &[Rect]) -> bool {
    let Some(tid) = gpudev::shadow_bo(target as u64) else { return false };
    let Some((payload, srcs)) = encode(frame, sources, info, damage) else { return false };
    let ok = gpudev::submit_scene(tid, payload, &srcs).is_some_and(gpudev::wait_scene);
    if ok {
        SCENES_DONE.fetch_add(1, Ordering::Relaxed);
        FAILURES_IN_ROW.store(0, Ordering::Relaxed);
        return true;
    }
    SCENES_FAILED.fetch_add(1, Ordering::Relaxed);
    if FAILURES_IN_ROW.fetch_add(1, Ordering::Relaxed) + 1 >= FAILURE_LIMIT {
        MODE.store(MODE_OFF, Ordering::Release);
        crate::kprintln!("[npk] gpu: {} scenes in a row failed on the GPU, composing on the CPU (`gpu verify` retries)",
            FAILURE_LIMIT);
    }
    false
}

/// Passes sources through and notes which pixels each one showed, so a
/// verification can tell a real difference from a source that changed
/// between the two runs (a guest frame or a widget raster arriving).
struct Recorder<'a> {
    inner: &'a dyn Sources,
    seen: core::cell::RefCell<Vec<(Source, usize, u32, u32)>>,
}

impl Sources for Recorder<'_> {
    fn with_source(&self, src: Source, f: &mut dyn FnMut(&[u32], u32, u32)) {
        self.inner.with_source(src, &mut |px, w, h| {
            self.seen.borrow_mut().push((src, px.as_ptr() as usize, w, h));
            f(px, w, h);
        });
    }
}

impl Recorder<'_> {
    fn seen(self) -> Vec<(Source, usize, u32, u32)> {
        let mut v = self.seen.into_inner();
        v.sort_unstable_by_key(|&(s, p, w, h)| (source_key(s), p, w, h));
        v.dedup();
        v
    }
}

fn source_key(s: Source) -> (u8, u32) {
    match s {
        Source::Widget(i) => (0, i),
        Source::Surface(i) => (1, i),
        Source::TermText(i) => (2, i),
    }
}

/// The first whole frame both ways: the GPU into a copy of `target`, then
/// the CPU into `target` itself, compared bit for bit. A source that
/// changed in between leaves the question open for the next whole frame.
fn verify(frame: &Scene, sources: &dyn Sources, target: *mut u8, info: &FbInfo, damage: &[Rect]) {
    let planes_before = plane_ptrs();
    let gpu_rec = Recorder { inner: sources, seen: Default::default() };
    let cpu_rec = Recorder { inner: sources, seen: Default::default() };
    let bytes = info.pitch as usize * info.height as usize;
    let pages = bytes.div_ceil(4096);
    let gpu_result = crate::memory::allocate_contiguous(pages).and_then(|copy| {
        // SAFETY: `copy` is `pages` fresh identity-mapped frames, `target`
        // a screen-sized buffer of `bytes`; they do not overlap.
        unsafe { core::ptr::copy_nonoverlapping(target, copy as *mut u8, bytes) };
        let t0 = crate::interrupts::rdtsc();
        let r = gpudev::bo_for(copy, bytes as u64, info.width, info.height, info.pitch)
            .zip(encode(frame, &gpu_rec, info, damage))
            .and_then(|(tid, (payload, srcs))| gpudev::submit_scene(tid, payload, &srcs))
            .map(gpudev::wait_scene);
        Some((copy, r, crate::interrupts::rdtsc() - t0))
    });

    let t0 = crate::interrupts::rdtsc();
    scene::execute(frame, &cpu_rec, target, info, damage);
    let cpu = crate::interrupts::rdtsc() - t0;

    let Some((copy, result, gpu)) = gpu_result else {
        crate::kprintln!("[npk] gpu: no memory to verify scenes, composing on the CPU");
        MODE.store(MODE_OFF, Ordering::Release);
        return;
    };
    // The GPU run asks every source, the CPU only those its damage reaches.
    let gpu_seen = gpu_rec.seen();
    if cpu_rec.seen().iter().any(|s| !gpu_seen.contains(s)) || plane_ptrs() != planes_before {
        crate::kdebug!("[npk] gpu: a source changed during the verification, comparing the next whole frame");
        if gpudev::release(copy, copy, pages) {
            crate::memory::deallocate_contiguous(copy, pages);
        }
        return;
    }
    let us = |c: u64| c * 1000 / (crate::interrupts::tsc_freq() / 1000).max(1);
    match result {
        Some(true) => {
            let mut diffs = 0u64;
            let mut shown = Vec::new();
            for y in 0..info.height {
                let row = y as usize * info.pitch as usize;
                for x in 0..info.width {
                    let off = row + x as usize * 4;
                    // SAFETY: both buffers hold `bytes` = pitch * height bytes;
                    // x < width keeps `off + 4` within the row.
                    let (c, g) = unsafe {
                        (*(target.add(off) as *const u32), *((copy as usize + off) as *const u32))
                    };
                    if c != g {
                        diffs += 1;
                        if shown.len() < DIFFS_SHOWN {
                            shown.push((x, y, c, g));
                        }
                    }
                }
            }
            if diffs == 0 {
                MODE.store(MODE_ON, Ordering::Release);
                FAILURES_IN_ROW.store(0, Ordering::Relaxed);
                crate::kprintln!("[npk] gpu: scene verified ({} items), GPU {} us vs CPU {} us — composing on the GPU",
                    frame.items.len(), us(gpu), us(cpu));
            } else {
                MODE.store(MODE_OFF, Ordering::Release);
                crate::kprintln!("[npk] gpu: scene differs in {} of {} pixels, composing on the CPU",
                    diffs, info.width as u64 * info.height as u64);
                for (x, y, c, g) in shown {
                    crate::kprintln!("[npk] gpu:   ({}, {}) CPU {:08x} GPU {:08x}, under {}", x, y, c, g,
                        kinds_at(frame, x, y));
                }
            }
        }
        Some(false) => {
            MODE.store(MODE_OFF, Ordering::Release);
            crate::kprintln!("[npk] gpu: the module rejected or did not finish the verification scene, composing on the CPU");
        }
        None => {
            MODE.store(MODE_OFF, Ordering::Release);
            crate::kprintln!("[npk] gpu: the verification scene could not be handed over, composing on the CPU");
        }
    }
    if gpudev::release(copy, copy, pages) {
        crate::memory::deallocate_contiguous(copy, pages);
    }
}

fn plane_ptrs() -> [Option<*const u8>; 3] {
    let p = crate::gui::background::planes();
    [p.wall, p.blur, p.glass]
}

/// The kinds of the items that cover `(x, y)`, bottom to top.
fn kinds_at(frame: &Scene, x: u32, y: u32) -> alloc::string::String {
    use core::fmt::Write;
    let mut s = alloc::string::String::new();
    let p = Rect::new(x, y, 1, 1);
    for item in &frame.items {
        let (name, r) = match *item {
            Item::Background => ("Background", None),
            Item::Glow { rect, band, .. } => ("Glow", Some(rect.grow(band))),
            Item::GlassBackdrop { rect, .. } => ("GlassBackdrop", Some(rect)),
            Item::Chrome { rect, .. } => ("Chrome", Some(rect)),
            Item::Content { dst, .. } => ("Content", Some(dst)),
            Item::Panel { dst, .. } => ("Panel", Some(dst)),
            Item::Text { dst, .. } => ("Text", Some(dst)),
            Item::Fill { rect, .. } => ("Fill", Some(rect)),
            Item::CloseGlyph { rect, .. } => ("CloseGlyph", Some(rect)),
        };
        if r.is_none_or(|r| !r.intersect(&p).is_empty()) {
            let _ = write!(s, "{}{}", if s.is_empty() { "" } else { " " }, name);
        }
    }
    s
}

fn put(out: &mut Vec<u8>, words: &[u32]) {
    for w in words {
        out.extend_from_slice(&w.to_le_bytes());
    }
}

/// A screen-sized plane of `background`.
fn plane_bo(p: Option<*const u8>, info: &FbInfo) -> Option<u32> {
    let p = p?;
    gpudev::bo_for(p as u64, info.pitch as u64 * info.height as u64, info.width, info.height, info.pitch)
}

/// The handle of a source's pixels, or `Some(None)` when the source has
/// none right now (the CPU paints nothing for it). None when it has pixels
/// the module cannot have.
fn source_bo(sources: &dyn Sources, src: Source) -> Option<Option<u32>> {
    let mut r = Some(None);
    sources.with_source(src, &mut |px, w, h| {
        if px.is_empty() || w == 0 || h == 0 || px.len() < (w * h) as usize {
            return;
        }
        r = gpudev::bo_for(px.as_ptr() as u64, w as u64 * h as u64 * 4, w, h, w * 4).map(Some);
    });
    r
}

/// The scene as `OP_SCENE` payload, with the handles of every buffer it
/// reads. None when a buffer cannot be handed to the module.
fn encode(frame: &Scene, sources: &dyn Sources, info: &FbInfo, damage: &[Rect])
    -> Option<(Vec<u8>, Vec<u32>)>
{
    let planes = crate::gui::background::planes();
    let mut srcs = Vec::new();
    let mut flags = 0;
    let mut plane = |p: Option<*const u8>, flag: u32, srcs: &mut Vec<u32>| -> Option<u32> {
        match p {
            None => Some(NONE),
            Some(_) => {
                let id = plane_bo(p, info)?;
                srcs.push(id);
                flags |= flag;
                Some(id)
            }
        }
    };
    let wall = plane(planes.wall, FLAG_WALLPAPER, &mut srcs)?;
    let blur = plane(planes.blur, FLAG_BLURRED, &mut srcs)?;
    let glass = plane(planes.glass, 0, &mut srcs)?;
    let ink = crate::shade::glass::ink();

    let mut items: Vec<[u32; 16]> = Vec::with_capacity(frame.items.len());
    let mut glyphs: Vec<u8> = Vec::new();
    for item in &frame.items {
        let mut w = [0u32; 16];
        let r = |w: &mut [u32; 16], r: Rect| { w[1] = r.x; w[2] = r.y; w[3] = r.w; w[4] = r.h; };
        match *item {
            Item::Background => w[0] = 1,
            Item::Glow { rect, radius, color, band, alpha } => {
                w[0] = 2;
                r(&mut w, rect);
                w[5..9].copy_from_slice(&[radius, color, band, alpha]);
            }
            Item::GlassBackdrop { rect, radius } => {
                w[0] = 3;
                r(&mut w, rect);
                w[5] = radius;
            }
            Item::Chrome { rect, border_a, border_b, bg, rounding, border, border_opacity, bg_opacity,
                           paint_content } => {
                w[0] = 4;
                r(&mut w, rect);
                w[5..13].copy_from_slice(&[border_a, border_b, bg, rounding, border, border_opacity, bg_opacity,
                    paint_content as u32]);
            }
            Item::Content { src, dst, radius, fit } => {
                let Some(id) = source_bo(sources, src)? else { continue };
                srcs.push(id);
                w[0] = 5;
                r(&mut w, dst);
                w[5..8].copy_from_slice(&[radius, (fit == Fit::Extend) as u32, id]);
            }
            Item::Panel { src, dst } | Item::Text { src, dst } => {
                let Some(id) = source_bo(sources, src)? else { continue };
                srcs.push(id);
                w[0] = if matches!(item, Item::Panel { .. }) { 6 } else { 7 };
                r(&mut w, dst);
                w[5] = id;
            }
            Item::Fill { rect, color, alpha, radius } => {
                w[0] = 8;
                r(&mut w, rect);
                w[5..8].copy_from_slice(&[color, alpha, radius]);
            }
            Item::CloseGlyph { rect, size, color } => {
                let glyph = crate::gui::icons::with_alpha(crate::shade::widgets::abi::IconId::X, size as u16,
                    |asz, a| (asz as u32, a.to_vec()));
                // The CPU paints nothing for a missing or short glyph either.
                let Some((asz, a)) = glyph else { continue };
                if asz == 0 || a.len() < (asz * asz) as usize { continue; }
                w[0] = 9;
                r(&mut w, rect);
                w[5..8].copy_from_slice(&[asz, color, glyphs.len() as u32]);
                glyphs.extend_from_slice(&a[..(asz * asz) as usize]);
                glyphs.resize(glyphs.len().next_multiple_of(4), 0);
            }
        }
        items.push(w);
    }

    let screen = Rect::new(0, 0, info.width, info.height);
    let damage: Vec<Rect> = damage.iter().map(|d| d.intersect(&screen)).filter(|d| !d.is_empty()).collect();
    let mut out = Vec::with_capacity(HEADER + damage.len() * 16 + items.len() * ITEM + glyphs.len());
    put(&mut out, &[MAGIC, items.len() as u32, damage.len() as u32, glyphs.len() as u32, flags, wall, blur, glass,
        planes.radius, ink.ceil, ink.tint, ink.tint_w, info.width, info.height, 0, 0]);
    for d in &damage {
        put(&mut out, &[d.x, d.y, d.w, d.h]);
    }
    for w in &items {
        put(&mut out, w);
    }
    out.extend_from_slice(&glyphs);
    srcs.sort_unstable();
    srcs.dedup();
    Some((out, srcs))
}

/// `gpu verify`: compare the next whole frame again.
pub fn request_verify() {
    MODE.store(MODE_UNTESTED, Ordering::Release);
    FAILURES_IN_ROW.store(0, Ordering::Relaxed);
}

/// `gpu cpu`: compose on the CPU until `gpu verify`.
pub fn force_cpu() {
    MODE.store(MODE_OFF, Ordering::Release);
}

/// What `gpu` reports.
pub struct Stats {
    pub mode: &'static str,
    pub gpu_scenes: u64,
    pub gpu_failed: u64,
    pub cpu_scenes: u64,
    /// Whole frames and their average time in microseconds, `[GPU, CPU]`.
    pub full: [(u64, u64); 2],
}

pub fn stats() -> Stats {
    let per_us = (crate::interrupts::tsc_freq() / 1_000_000).max(1);
    let avg = |cycles: u64, n: u64| if n == 0 { 0 } else { cycles / n / per_us };
    let full = core::array::from_fn(|i| {
        let n = FULL_FRAMES[i].load(Ordering::Relaxed);
        (n, avg(FULL_CYCLES[i].load(Ordering::Relaxed), n))
    });
    Stats {
        mode: match MODE.load(Ordering::Acquire) {
            _ if !gpudev::scenes_offered() => "CPU (no module offers scenes)",
            MODE_ON => "GPU (verified)",
            MODE_UNTESTED => "CPU until the next whole frame is verified",
            _ => "CPU",
        },
        gpu_scenes: SCENES_DONE.load(Ordering::Relaxed),
        gpu_failed: SCENES_FAILED.load(Ordering::Relaxed),
        cpu_scenes: CPU_SCENES.load(Ordering::Relaxed),
        full,
    }
}
