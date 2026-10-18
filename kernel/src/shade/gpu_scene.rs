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
use crate::shade::scene::{self, Fit, InPlace, Item, Rect, Scene, Source, Sources};

const MAGIC: u32 = 0x314E_4353;
const HEADER: usize = 64;
const ITEM: usize = 64;
const NONE: u32 = u32::MAX;
const FLAG_WALLPAPER: u32 = 1;
const FLAG_BLURRED: u32 = 2;
/// Scenes in a row the module failed before the CPU takes over again. A
/// scene that was late is not one: the desktop composes on the CPU while
/// the module is stalled, and goes back to it when the late job is done.
const FAILURE_LIMIT: u32 = 8;
/// Failed scenes that say why in the log.
const FAILURES_LOGGED: u64 = 16;
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

/// A scene handed to the module and not yet waited for. Waiting happens in
/// `finish`, after the caller let go of the compositor: a widget app that
/// asks the compositor meanwhile spins on its lock, and on the GPU module's
/// core that would keep the module from ever completing the scene.
enum Pending {
    Scene {
        seq: u32,
        scene: Scene,
        target: usize,
        info: FbInfo,
        damage: Vec<Rect>,
        /// New buffers were queued for binding ahead of it.
        binding: bool,
        full: bool,
        t0: u64,
    },
    Verify {
        seq: Option<u32>,
        scene: Scene,
        target: usize,
        info: FbInfo,
        copy: u64,
        pages: usize,
        cpu: u64,
        t0: u64,
    },
}

// SAFETY: `target` and `copy` are addresses of identity-mapped buffers the
// compositor owns; the pending scene is finished on the core that made it.
unsafe impl Send for Pending {}

static PENDING: spin::Mutex<Option<Pending>> = spin::Mutex::new(None);

/// Compose `frame` into `target` within `damage`: on the CPU now, or handed
/// to the GPU, in which case the caller must call `finish` once it has let
/// go of the compositor and before it reads `target`.
pub fn compose(frame: &Scene, sources: &dyn Sources, target: *mut u8, info: &FbInfo, damage: &[Rect]) {
    let t0 = crate::interrupts::rdtsc();
    let full = covers_screen(damage, info);
    // A scene still in flight is finished first; the caller should have.
    finish_with(sources);
    if gpudev::scenes_offered() {
        match MODE.load(Ordering::Acquire) {
            MODE_ON => {
                if submit(frame, sources, target, info, damage, full, t0) {
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
    if full {
        FULL_FRAMES[1].fetch_add(1, Ordering::Relaxed);
        FULL_CYCLES[1].fetch_add(crate::interrupts::rdtsc() - t0, Ordering::Relaxed);
    }
}

fn covers_screen(damage: &[Rect], info: &FbInfo) -> bool {
    damage.iter().any(|d| d.x == 0 && d.y == 0 && d.w >= info.width && d.h >= info.height)
}

/// Hand one scene to the module. False when it was not taken; the caller
/// composes on the CPU.
fn submit(frame: &Scene, sources: &dyn Sources, target: *mut u8, info: &FbInfo, damage: &[Rect], full: bool,
          t0: u64) -> bool {
    let Some(tid) = gpudev::shadow_bo(target as u64) else { return false };
    if gpudev::stalled_now() {
        return false;
    }
    let pin = gpudev::ScenePin::new();
    let r = match encode(frame, sources, info, damage) {
        None => Err("a source cannot be handed to the module"),
        Some((payload, srcs)) => gpudev::submit_scene(tid, payload, &srcs).ok_or("the scene was refused"),
    };
    let binding = pin.added();
    drop(pin);
    match r {
        Ok(seq) => {
            *PENDING.lock() = Some(Pending::Scene {
                seq, scene: Scene { items: frame.items.clone() }, target: target as usize, info: *info,
                damage: damage.to_vec(), binding, full, t0,
            });
            true
        }
        Err(why) => {
            failed(Err(why));
            false
        }
    }
}

/// Count and report a scene the module did not compose.
fn failed(status: Result<i32, &str>) {
    if SCENES_FAILED.fetch_add(1, Ordering::Relaxed) < FAILURES_LOGGED {
        match status {
            Err(why) => crate::kprintln!("[npk] gpu: scene on the CPU, {}", why),
            Ok(-62) => crate::kprintln!("[npk] gpu: scene on the CPU, the module did not finish in time"),
            Ok(e) => crate::kprintln!("[npk] gpu: scene on the CPU, the module failed it ({})", e),
        }
    }
    if status == Ok(-62) {
        return;
    }
    if FAILURES_IN_ROW.fetch_add(1, Ordering::Relaxed) + 1 >= FAILURE_LIMIT {
        MODE.store(MODE_OFF, Ordering::Release);
        crate::kprintln!("[npk] gpu: {} scenes in a row failed on the GPU, composing on the CPU (`gpu verify` retries)",
            FAILURE_LIMIT);
    }
}

/// Wait for the scene handed to the module, if any. Call it without holding
/// the compositor, before reading the target. A failed scene is composed on
/// the CPU here, which takes the compositor for its sources.
pub fn finish() {
    let Some(p) = PENDING.lock().take() else { return };
    if let Some(redo) = settle(p) {
        crate::shade::with_compositor(|comp| redo.run(&*comp));
    }
}

/// `finish` for a caller that holds the compositor.
fn finish_with(sources: &dyn Sources) {
    let Some(p) = PENDING.lock().take() else { return };
    if let Some(redo) = settle(p) {
        redo.run(sources);
    }
}

/// A scene the CPU has to compose after all.
struct Redo {
    scene: Scene,
    target: usize,
    info: FbInfo,
    damage: Vec<Rect>,
}

impl Redo {
    fn run(&self, sources: &dyn Sources) {
        scene::execute(&self.scene, sources, self.target as *mut u8, &self.info, &self.damage);
        CPU_SCENES.fetch_add(1, Ordering::Relaxed);
    }
}

/// Wait for `p`; what the CPU still has to compose.
fn settle(p: Pending) -> Option<Redo> {
    match p {
        Pending::Scene { seq, scene, target, info, damage, binding, full, t0 } => {
            let status = if binding { gpudev::wait_scene_bound(seq) } else { gpudev::wait_scene(seq) };
            if status == 0 {
                SCENES_DONE.fetch_add(1, Ordering::Relaxed);
                FAILURES_IN_ROW.store(0, Ordering::Relaxed);
                if full {
                    FULL_FRAMES[0].fetch_add(1, Ordering::Relaxed);
                    FULL_CYCLES[0].fetch_add(crate::interrupts::rdtsc() - t0, Ordering::Relaxed);
                }
                return None;
            }
            failed(Ok(status));
            Some(Redo { scene, target, info, damage })
        }
        Pending::Verify { seq, scene, target, info, copy, pages, cpu, t0 } => {
            let ok = seq.map(|s| gpudev::wait_scene_bound(s) == 0);
            let gpu = crate::interrupts::rdtsc() - t0;
            judge(&scene, target as *mut u8, &info, copy, ok, gpu, cpu);
            if gpudev::release(copy, copy, pages) {
                crate::memory::deallocate_contiguous(copy, pages);
            }
            None
        }
    }
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
        self.inner.with_source_keyed(src, &mut |px, w, h, key| {
            self.seen.borrow_mut().push((src, key as usize, w, h));
            f(px, w, h);
        });
    }

    fn in_place(&self, src: Source) -> Option<InPlace> {
        let p = self.inner.in_place(src)?;
        self.seen.borrow_mut().push((src, p.key as usize, p.width, p.height));
        Some(p)
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

/// The first whole frame both ways: the GPU into a copy of `target`, the
/// CPU into `target` itself; `finish` compares them bit for bit. A source
/// that changed in between leaves the question open for the next whole
/// frame.
fn verify(frame: &Scene, sources: &dyn Sources, target: *mut u8, info: &FbInfo, damage: &[Rect]) {
    let planes_before = plane_ptrs();
    let gpu_rec = Recorder { inner: sources, seen: Default::default() };
    let cpu_rec = Recorder { inner: sources, seen: Default::default() };
    let bytes = info.pitch as usize * info.height as usize;
    let pages = bytes.div_ceil(4096);
    let Some(copy) = crate::memory::allocate_contiguous(pages) else {
        scene::execute(frame, sources, target, info, damage);
        crate::kprintln!("[npk] gpu: no memory to verify scenes, composing on the CPU");
        MODE.store(MODE_OFF, Ordering::Release);
        return;
    };
    // SAFETY: `copy` is `pages` fresh identity-mapped frames, `target` a
    // screen-sized buffer of `bytes`; they do not overlap.
    unsafe { core::ptr::copy_nonoverlapping(target, copy as *mut u8, bytes) };
    let t0 = crate::interrupts::rdtsc();
    let seq = {
        let _pin = gpudev::ScenePin::new();
        gpudev::bo_for(copy, bytes as u64, info.width, info.height, info.pitch)
            .zip(encode(frame, &gpu_rec, info, damage))
            .and_then(|(tid, (payload, srcs))| gpudev::submit_scene(tid, payload, &srcs))
    };

    let t1 = crate::interrupts::rdtsc();
    scene::execute(frame, &cpu_rec, target, info, damage);
    let cpu = crate::interrupts::rdtsc() - t1;

    // The GPU run asks every source, the CPU only those its damage reaches.
    let gpu_seen = gpu_rec.seen();
    let changed = cpu_rec.seen().iter().any(|s| !gpu_seen.contains(s)) || plane_ptrs() != planes_before;
    if changed {
        crate::kdebug!("[npk] gpu: a source changed during the verification, comparing the next whole frame");
        // The copy stays allocated until the scene that writes it is done.
        if let Some(s) = seq {
            gpudev::wait_scene_bound(s);
        }
        if gpudev::release(copy, copy, pages) {
            crate::memory::deallocate_contiguous(copy, pages);
        }
        return;
    }
    *PENDING.lock() = Some(Pending::Verify {
        seq, scene: Scene { items: frame.items.clone() }, target: target as usize, info: *info, copy, pages, cpu, t0,
    });
}

/// Compare the verification copy with the CPU's frame and switch.
fn judge(frame: &Scene, target: *mut u8, info: &FbInfo, copy: u64, ok: Option<bool>, gpu: u64, cpu: u64) {
    let us = |c: u64| c * 1000 / (crate::interrupts::tsc_freq() / 1000).max(1);
    match ok {
        Some(true) => {
            let mut diffs = 0u64;
            let mut shown = Vec::new();
            for y in 0..info.height {
                let row = y as usize * info.pitch as usize;
                for x in 0..info.width {
                    let off = row + x as usize * 4;
                    // SAFETY: both buffers hold pitch * height bytes; x < width
                    // keeps `off + 4` within the row.
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
                crate::kprintln!("[npk] gpu: scene verified ({} items), GPU {} us incl. binding vs CPU {} us — composing on the GPU",
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
    if let Some(p) = sources.in_place(src) {
        return p.bo.map(Some);
    }
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

/// Scenes may go to the GPU: verified, or not yet tried.
pub fn may_compose_on_gpu() -> bool {
    MODE.load(Ordering::Acquire) != MODE_OFF
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

/// `gpu iommu test`: the GPU composes a small buffer into a screen-sized one
/// twice — while the IOMMU lets the card reach the small buffer, and after
/// that grant was taken back. The first run must deliver the pattern; the
/// second must not, and must leave a fault at the buffer's address. The
/// faulting run may hang the engine, and the module then gives the card
/// back (Linux wedges the GT on a DMAR fault as well).
pub fn iommu_probe() {
    use crate::shade::buffers::PixelBuffer;
    const W: u32 = 64;
    const H: u32 = 64;
    let Some(dev) = gpudev::status().owner else {
        crate::kprintln!("[npk] iommu test: no GPU module registered");
        return;
    };
    if !crate::drivers::iommu::protected(dev) {
        crate::kprintln!("[npk] iommu test: the card is not behind its IOMMU (`set gpu_iommu on`, then reboot)");
        return;
    }
    if !gpudev::scenes_offered() {
        crate::kprintln!("[npk] iommu test: the module runs no scenes");
        return;
    }
    finish();
    let fb = crate::framebuffer::get_info();
    let (sw, sh) = (fb.width.max(W), fb.height.max(H));
    let mut src = PixelBuffer::new(W, H);
    let mut dst = PixelBuffer::new(sw, sh);
    if src.is_empty() || dst.is_empty() {
        crate::kprintln!("[npk] iommu test: no memory for the buffers");
        return;
    }
    for (i, px) in src.iter_mut().enumerate() {
        *px = 0xFF00_0000 | (i as u32).wrapping_mul(0x9E37_79B9) >> 8 | 1;
    }
    struct One<'a>(&'a PixelBuffer);
    impl Sources for One<'_> {
        fn with_source(&self, _src: Source, f: &mut dyn FnMut(&[u32], u32, u32)) {
            f(self.0, self.0.width(), self.0.height());
        }
    }
    let frame = Scene { items: alloc::vec![Item::Content {
        src: Source::Surface(u32::MAX), dst: Rect::new(0, 0, W, H), radius: 0, fit: Fit::Extend,
    }] };
    let damage = [Rect::new(0, 0, W, H)];
    let info = dst.info();
    let run = |src: &PixelBuffer, dst: &PixelBuffer| -> Option<i32> {
        let seq = {
            let _pin = gpudev::ScenePin::new();
            let tid = gpudev::bo_for(dst.phys(), sw as u64 * sh as u64 * 4, sw, sh, sw * 4)?;
            let (payload, srcs) = encode(&frame, &One(src), &info, &damage)?;
            gpudev::submit_scene(tid, payload, &srcs)?
        };
        Some(gpudev::wait_scene_bound(seq))
    };
    let arrived = |src: &PixelBuffer, dst: &PixelBuffer| {
        (0..H as usize).all(|y| dst[y * sw as usize..y * sw as usize + W as usize] == src[y * W as usize..(y + 1) * W as usize])
    };
    let pages = (W as usize * H as usize * 4).div_ceil(4096);
    let (lo, hi) = (src.phys(), src.phys() + pages as u64 * 4096);
    let dst_bytes = sw as u64 * sh as u64 * 4;
    crate::kprintln!("[npk] iommu test: src {:#x}..{:#x}, dst {:#x}..{:#x}", lo, hi, dst.phys(), dst.phys() + dst_bytes);
    let _ = crate::drivers::iommu::take_faults(dev);

    let s1 = run(&src, &dst);
    let ok1 = arrived(&src, &dst);
    let f1 = crate::drivers::iommu::take_faults(dev).len();
    crate::kprintln!("[npk] iommu test: granted — scene {:?}, pattern {}, {} fault(s)",
        s1, if ok1 { "arrived" } else { "MISSING" }, f1);

    for px in dst.iter_mut() {
        *px = 0;
    }
    crate::drivers::iommu::revoke(dev, src.phys(), pages);
    let s2 = run(&src, &dst);
    let leaked = arrived(&src, &dst);
    let faults = crate::drivers::iommu::take_faults(dev);
    crate::drivers::iommu::grant(dev, src.phys(), pages);
    let hit = faults.iter().any(|f| f.addr >= lo && f.addr < hi);
    crate::kprintln!("[npk] iommu test: revoked — scene {:?}, pattern {}, fault at the buffer {}",
        s2, if leaked { "ARRIVED" } else { "did not arrive" }, if hit { "recorded" } else { "MISSING" });
    for f in faults.iter().take(4) {
        let at = if f.addr >= lo && f.addr < hi {
            "src"
        } else if f.addr >= dst.phys() && f.addr < dst.phys() + dst_bytes {
            "dst"
        } else {
            "elsewhere"
        };
        crate::kprintln!("[npk] iommu test:   {} fault at {:#x} ({}), reason {:#x}", if f.read { "read" } else { "write" },
            f.addr, at, f.reason);
    }
    crate::kprintln!("[npk] iommu test: {}", if ok1 && !leaked && hit { "PASSED" } else { "FAILED" });
}
