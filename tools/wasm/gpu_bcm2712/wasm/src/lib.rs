//! gpu_bcm2712: the Raspberry Pi 5 display module. It binds the display
//! set of the device tree (HVS, pixel valves, HDMI, DDC and their
//! interrupt controllers), takes the screen over from the firmware the
//! way Linux's vc4 does when it binds, and shows the framebuffer the
//! kernel draws into in the mode Linux's fbdev client picks from the
//! monitor's EDID, the kernel's framebuffer moved to that size. A
//! hotplug probes again and follows.
//!
//! The compositor keeps composing on the CPU into its two shadow buffers;
//! the module presents them (`CAP_COPY | CAP_PRESENT | CAP_FLIP`): a copy
//! of a whole shadow to the framebuffer becomes a page flip to that
//! shadow, and the kernel draws every update into the other shadow, never
//! into the one on screen. A copy of part of the shadow on screen is
//! already shown (the kernel wrote the rectangle back from its caches
//! before handing the copy over). The driver itself is `gpu_bcm2712_core`,
//! replayed against a Linux run on the host.
//!
//! The pointer is the HVS's cursor plane (`CAP_CURSOR`): its image in a
//! buffer of the module's own, a move an async update of the plane.

#![no_std]

extern crate alloc;

mod hw;
mod scene;

use drm_core::drm_client_modeset::drm_client_dev;
use drm_core::drm_fourcc::drm_format_info;
use drm_core::drm_plane::drm_framebuffer;
use drm_core::linux::Line;
use gpu_bcm2712_core::vc4_drv::Vc4;
use hw::PiHw;

// The default rights plus HARDWARE, which binding the display set
// requires. A section replaces the default, so READ, EXECUTE and RENDER
// are listed; RENDER also makes `run` start the module as a process of its
// own (the set is never bound to the shell's pid 0).
#[unsafe(link_section = ".npk.caps")]
#[used]
static NPK_CAPS: [u8; 1] = [0x01 | 0x04 | 0x08 | 0x40]; // READ|EXEC|RENDER|HARDWARE

#[global_allocator]
static ALLOCATOR: nopeek_widgets::heap::Allocator = nopeek_widgets::heap::new();

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    let mut l = Line::new();
    l.s("PANIC, driver stopped");
    if let Some(loc) = info.location() {
        l.s(" at ").s(loc.file()).s(":").dec(loc.line() as u64);
    }
    l.say();
    core::arch::wasm32::unreachable()
}

/// `kernel/src/gpu/gpudev.rs`: the protocol version, a buffer's
/// description and one job.
const GPUDEV_VERSION: i32 = 2;
const JOB_BYTES: usize = 32;

/// `npk_gpudev_ready` capabilities (`kernel/src/gpu/gpudev.rs`).
const CAP_COPY: i32 = 1;
const CAP_PRESENT: i32 = 16;
const CAP_FLIP: i32 = 128;
const CAP_CURSOR: i32 = 8;
const CAP_SCENE: i32 = 2;
/// `OP_BO_ADD` / `OP_BO_REMOVE`: `a0` buffer; `OP_SCENE`: `a0` target,
/// `a1` payload bytes.
const OP_BO_ADD: u32 = 2;
const OP_BO_REMOVE: u32 = 3;
const OP_SCENE: u32 = 4;
/// `OP_SHUTDOWN`: the system goes down.
const OP_SHUTDOWN: u32 = 15;
/// `OP_CURSOR_IMAGE`: `a0` width, `a1` height, `a2`/`a3` hotspot, the
/// payload premultiplied ARGB8888; `OP_CURSOR_MOVE`: `a0` x, `a1` y,
/// `a2` shown.
const OP_CURSOR_IMAGE: u32 = 11;
const OP_CURSOR_MOVE: u32 = 12;
/// `gpudev::MAX_CURSOR`: the largest image side.
const MAX_CURSOR: u32 = 256;
const CURSOR_SLOT: u32 = MAX_CURSOR * MAX_CURSOR * 4;
/// `OP_COPY_RECT`: `a0` source, `a1` target buffer, `a2..a5` x, y, w, h.
const OP_COPY_RECT: u32 = 1;

/// The mask bits of `npk_sys::wait`.
pub(crate) const WAIT_IRQ: i32 = 2;
const WAIT_GPU_CMD: i32 = 64;

fn printk(b: &[u8]) {
    let mut v = alloc::vec::Vec::with_capacity(b.len() + 16);
    v.extend_from_slice(b"[gpu_bcm2712] ");
    v.extend_from_slice(b);
    v.push(b'\n');
    npk_sys::log(&v);
}

/// A kernel buffer: gpudev buffer 0 is the framebuffer, 1 and 2 the
/// compositor's shadows.
#[derive(Clone, Copy)]
struct KernelFb {
    phys: u64,
    pitch: u32,
    width: u32,
    height: u32,
    format: u32,
}

fn kernel_fb() -> Option<KernelFb> {
    buffer(0)
}

fn buffer(id: i32) -> Option<KernelFb> {
    let mut o = [0u8; 40];
    if npk_sys::gpudev_bo_info(id, &mut o) < 0 {
        return None;
    }
    let u32_at = |i: usize| u32::from_le_bytes([o[i], o[i + 1], o[i + 2], o[i + 3]]);
    Some(KernelFb {
        phys: u64::from_le_bytes([o[0], o[1], o[2], o[3], o[4], o[5], o[6], o[7]]),
        pitch: u32_at(16),
        width: u32_at(20),
        height: u32_at(24),
        format: u32_at(28),
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    drm_core::linux::set_printk(printk);
    Line::new().s("v").s(env!("CARGO_PKG_VERSION")).s(" starting").say();

    if npk_sys::dt_bind(b"brcm,bcm2712-hvs") != 0 {
        Line::new().s("no display set to bind (needs the Pi 5, HARDWARE and a process of its own)").say();
        return;
    }
    let Some(mut hw) = PiHw::map() else {
        Line::new().s("a register window or interrupt of the display set is missing").say();
        return;
    };

    // gpudev::register: -1 no framebuffer or shadows, -2 version, -3 taken.
    let r = npk_sys::gpudev_register(GPUDEV_VERSION);
    if r < 0 {
        let why = match r {
            -2 => "the kernel speaks another gpudev version",
            -3 => "the GPU service is taken",
            _ => "the kernel has no 32 bpp framebuffer to hand over",
        };
        Line::new().s(why).s(", not binding").say();
        return;
    }
    let Some(mut kfb) = kernel_fb() else {
        Line::new().s("no framebuffer from the kernel").say();
        return;
    };

    // From here on the firmware no longer drives the display.
    let mut vc4 = match Vc4::bind(&mut hw) {
        Ok(v) => v,
        Err(e) => {
            Line::new().s("bind failed: ").dec(e.unsigned_abs() as u64).say();
            return;
        }
    };

    let client = vc4.fbdev_probe(&mut hw);
    show(&mut vc4, &mut hw, &client, &mut kfb);

    // The shadows, if the HVS can show them, are presented by flips;
    // otherwise the compositor copies into the framebuffer on the CPU.
    let shadows = bind_shadows(&kfb);
    let cursor = Cursor::new();
    let mut caps = if shadows.is_some() { CAP_COPY | CAP_PRESENT | CAP_FLIP } else { 0 };
    if caps != 0 && cursor.is_some() {
        caps |= CAP_CURSOR;
    }
    // V3D composes the scenes once its dispatch path proved itself.
    let scenes = if caps != 0 { Scenes::new(&mut hw) } else { None };
    if scenes.is_some() {
        caps |= CAP_SCENE;
    }
    if npk_sys::gpudev_ready(caps) < 0 {
        Line::new().s("the kernel did not take the presenting, the compositor copies on the CPU").say();
    }

    let mut p = Present { shown: 0, shadows: shadows.unwrap_or([kfb, kfb]), cursor, scenes };
    serve(&mut vc4, &mut hw, &mut kfb, &mut p);
}

/// Buffers 0..=2 bound at their bus addresses (identity through the
/// display's IOMMU, which the kernel granted them), when both shadows
/// are one contiguous run of pages each, of the framebuffer's size and
/// format: the HVS scans a buffer from one base address.
fn bind_shadows(kfb: &KernelFb) -> Option<[KernelFb; 2]> {
    let a = buffer(1)?;
    let b = buffer(2)?;
    for (id, s) in [(1, &a), (2, &b)] {
        if (s.width, s.height, s.pitch, s.format) != (kfb.width, kfb.height, kfb.pitch, kfb.format)
            || !contiguous(id, s)
        {
            Line::new().s("a shadow buffer the HVS cannot scan, the compositor copies on the CPU").say();
            return None;
        }
    }
    for (id, f) in [(0, kfb), (1, &a), (2, &b)] {
        if npk_sys::gpudev_bo_bound(id, f.phys as i64) < 0 {
            return None;
        }
    }
    Some([a, b])
}

/// Whether buffer `id`'s pages follow each other from `f.phys` on.
fn contiguous(id: i32, f: &KernelFb) -> bool {
    let pages = (f.pitch as u64 * f.height as u64).div_ceil(4096) as i32;
    let mut list = [0u64; 512];
    let mut first = 0;
    while first < pages {
        let n = npk_sys::gpudev_bo_pages(id, first, &mut list);
        if n <= 0 {
            return false;
        }
        for (i, &p) in list[..n as usize].iter().enumerate() {
            if p != f.phys + (first as u64 + i as u64) * 4096 {
                return false;
            }
        }
        first += n;
    }
    true
}

/// Which buffer the HVS shows: 0 the framebuffer, 1 or 2 a shadow; and
/// the pointer.
struct Present {
    shown: u32,
    shadows: [KernelFb; 2],
    cursor: Option<Cursor>,
    scenes: Option<Scenes>,
}

/// A buffer a scene reads or writes, mapped for V3D.
#[derive(Clone, Copy)]
struct SceneBo {
    id: u32,
    surface: gpu_scene::scene::surface,
    /// The mapping, to take away again.
    va: u32,
}

/// The scene service: V3D's composer and the buffers the kernel
/// registered with it, the shadows (1 and 2) first.
struct Scenes {
    c: scene::Composer,
    bos: alloc::vec::Vec<SceneBo>,
    /// Scenes whose cost was logged.
    logged: u32,
    /// Queued scenes.
    pending: alloc::vec::Vec<Pending>,
    /// Slow scenes logged, and those with their counters.
    slow_logged: u32,
    perf_logged: u32,
}

/// A scene on V3D, not yet completed.
#[derive(Clone, Copy)]
struct Pending {
    seq: i32,
    /// Its last fence.
    fence: u64,
    /// When it was taken (us).
    t0: u64,
    dispatches: u32,
    traced: bool,
    /// What powering V3D up for it cost (us).
    power_us: u64,
}

/// What woke the loop that saw a scene finished: `npk_wait`'s bits, or
/// `WOKE_DRAIN` for a wait on the fence itself.
const WOKE_DRAIN: i32 = -1;
/// A scene this slow (us) says where its time went.
const SCENE_SLOW_US: u64 = 50_000;
/// Slow scenes that say so.
const SLOW_LOGGED: u32 = 64;
/// Scenes logged with their performance counters.
const PERF_LOGGED: u32 = 16;

/// Scenes after the start whose cost goes to the log.
const SCENES_LOGGED: u32 = 8;
/// Of those, the scenes logged dispatch by dispatch.
const SCENES_TRACED: u32 = 2;

/// Buffer `id`'s pages, in order (`npk_gpudev_bo_pages`).
fn bo_page_list(id: u32, pages: u32) -> Option<alloc::vec::Vec<u64>> {
    let mut list = alloc::vec![0u64; pages as usize];
    let mut at = 0;
    while at < list.len() {
        let n = npk_sys::gpudev_bo_pages(id as i32, at as i32, &mut list[at..]);
        if n <= 0 {
            return None;
        }
        at += n as usize;
    }
    Some(list)
}

/// Buffer `id` mapped for V3D: its description and V3D address.
fn map_bo(id: u32) -> Option<SceneBo> {
    let mut o = [0u8; 40];
    if npk_sys::gpudev_bo_info(id as i32, &mut o) < 0 {
        return None;
    }
    let u32_at = |i: usize| u32::from_le_bytes([o[i], o[i + 1], o[i + 2], o[i + 3]]);
    let size = u64::from_le_bytes([o[8], o[9], o[10], o[11], o[12], o[13], o[14], o[15]]);
    let (pitch, width, height, offset, pages) = (u32_at(16), u32_at(20), u32_at(24), u32_at(32), u32_at(36));
    let list = bo_page_list(id, pages)?;
    let va = npk_sys::v3d_map(&list);
    if va <= 0 {
        return None;
    }
    let va = va as u32;
    let surface = gpu_scene::scene::surface { va: (va + offset) as u64, pitch, width, height, size };
    Some(SceneBo { id, surface, va })
}

impl Scenes {
    fn new(hw: &mut PiHw) -> Option<Scenes> {
        let mut c = scene::composer_init(hw)?;
        if !c.self_test(hw) || !c.draw_self_test(hw) {
            c.free(hw);
            return None;
        }
        let bos = alloc::vec![map_bo(1)?, map_bo(2)?];
        Some(Scenes { c, bos, logged: 0, pending: alloc::vec::Vec::new(), slow_logged: 0, perf_logged: 0 })
    }

    /// `OP_BO_ADD`: mapped for V3D, its address reported.
    fn add(&mut self, id: u32) -> i32 {
        if self.bos.iter().any(|b| b.id == id) {
            return -22;
        }
        let Some(b) = map_bo(id) else { return -22 };
        if npk_sys::gpudev_bo_bound(id as i32, b.surface.va as i64) < 0 {
            let _ = npk_sys::v3d_unmap(b.va as i64);
            return -22;
        }
        self.bos.push(b);
        0
    }

    /// `OP_BO_REMOVE`: V3D no longer reaches it.
    fn remove(&mut self, hw: &mut PiHw, id: u32) -> i32 {
        let Some(i) = self.bos.iter().position(|b| b.id == id && id > 2) else { return -22 };
        self.c.drain(hw);
        self.reap(hw, WOKE_DRAIN);
        let b = self.bos.remove(i);
        let _ = npk_sys::v3d_unmap(b.va as i64);
        0
    }

    /// `OP_SCENE`: queued on V3D. None while it runs (`reap` completes
    /// it), or its status when it was refused.
    fn run(&mut self, hw: &mut PiHw, seq: i32, a: &[u32; 6]) -> Option<i32> {
        // One scene on V3D at a time (the next waits for the last fence):
        // the one before is completed first, so the trace and the counters
        // are this scene's alone.
        self.c.drain(hw);
        self.reap(hw, WOKE_DRAIN);
        let bufs = SceneBufs(&self.bos);
        let t0 = npk_sys::now_us() as u64;
        let traced = self.logged < SCENES_TRACED;
        self.c.trace = Some(alloc::vec::Vec::new());
        self.c.perf_armed = true;
        match self.c.run_scene(hw, &bufs, seq as u32, a[0], a[1]) {
            Ok((n, f)) => {
                let power_us = self.c.take_power_us();
                self.pending.push(Pending { seq, fence: f, t0, dispatches: n, traced, power_us });
                if traced {
                    self.logged += 1;
                }
                None
            }
            Err(e) => {
                Line::new().s("scene into buffer ").dec(a[0] as u64).s(" refused (").dec(e.unsigned_abs() as u64)
                    .s(")").say();
                Some(e)
            }
        }
    }

    /// V3D's interrupts and timeouts served; every queued scene whose last
    /// fence signalled completed. The first scenes say what they cost: the
    /// measure of the V3D path, dispatch by dispatch for the first ones; a
    /// slow one says what woke the loop that saw it done (`woke`).
    fn reap(&mut self, hw: &mut PiHw, woke: i32) {
        self.c.service(hw);
        let mut i = 0;
        while i < self.pending.len() {
            let p = self.pending[i];
            let (seq, traced) = (p.seq, p.traced);
            let (done, failed) = self.c.fence_state(p.fence);
            if !done {
                i += 1;
                continue;
            }
            self.pending.remove(i);
            let total = (npk_sys::now_us() as u64).saturating_sub(p.t0);
            let gpu = self.c.take_job_us(p.fence);
            let slow = total >= SCENE_SLOW_US && self.slow_logged < SLOW_LOGGED;
            if slow {
                self.slow_logged += 1;
            }
            let trace = self.c.trace.take().unwrap_or_default();
            let perf = self.c.perf_take(hw);
            if traced || self.logged < SCENES_LOGGED || slow {
                let by = match woke {
                    WOKE_DRAIN => "a wait on it",
                    0 => "the loop's timeout",
                    w if w & WAIT_IRQ != 0 => "an interrupt",
                    _ => "a command",
                };
                Line::new().s("scene: ").dec(p.dispatches as u64).s(" submissions in ").dec(total).s(" us")
                    .s(if failed { ", failed" } else { "" })
                    .s(" (power-up ").dec(p.power_us).s(" us, jobs ").dec(gpu).s(" us, seen after ").s(by).s(")")
                    .say();
            }
            if traced || slow {
                for (name, g, us) in trace {
                    let mut l = Line::new();
                    l.s("scene:   ").s(name).s(" ");
                    if name == "bin" || name == "render" {
                        // Draws, and the render area.
                        l.dec(g[0] as u64).s(" draws over ").dec(g[1] as u64).s("x").dec(g[2] as u64);
                    } else {
                        l.dec(g[0] as u64).s("x").dec(g[1] as u64).s(" groups");
                    }
                    l.s(", ").dec(us).s(" us").say();
                }
                if let Some(v) = perf.filter(|_| self.perf_logged < PERF_LOGGED) {
                    self.perf_logged += 1;
                    for (names, vals) in scene::PERF_COUNTERS.chunks(6).zip(v.chunks(6)) {
                        let mut l = Line::new();
                        l.s("scene:   ");
                        for (n, v) in names.iter().zip(vals) {
                            l.s(n).s("=").dec(*v).s(" ");
                        }
                        l.say();
                    }
                }
            }
            if !traced && self.logged < SCENES_LOGGED {
                self.logged += 1;
            }
            npk_sys::gpudev_complete(seq, if failed { -5 } else { 0 });
        }
    }
}

struct SceneBufs<'a>(&'a [SceneBo]);

impl gpu_scene::scene::buffers for SceneBufs<'_> {
    fn surface(&self, id: u32) -> Option<gpu_scene::scene::surface> {
        self.0.iter().find(|b| b.id == id).map(|b| b.surface)
    }
}

/// The pointer on the cursor plane. Its image lives in one of two slots
/// of a DMA buffer, a new image in the slot the HVS does not read (as
/// Linux's cursor gets a new buffer per image).
struct Cursor {
    dma: i32,
    phys: u64,
    slot: u32,
    fb: Option<drm_framebuffer>,
    hot: (i32, i32),
    pos: (i32, i32),
    shown: bool,
}

impl Cursor {
    fn new() -> Option<Cursor> {
        let dma = npk_sys::dma_alloc((2 * CURSOR_SLOT / 4096) as i32);
        if dma < 0 {
            return None;
        }
        let phys = npk_sys::dma_phys_addr(dma) as u64;
        Some(Cursor { dma, phys, slot: 0, fb: None, hot: (0, 0), pos: (0, 0), shown: false })
    }

    /// `OP_CURSOR_IMAGE` of job `seq`.
    fn image(&mut self, vc4: &mut Vc4, hw: &mut PiHw, seq: i32, a: &[u32; 6]) -> i32 {
        let (w, h) = (a[0], a[1]);
        if w == 0 || h == 0 || w > MAX_CURSOR || h > MAX_CURSOR {
            return -22;
        }
        let len = (w * h * 4) as usize;
        let mut px = alloc::vec![0u8; len];
        let mut at = 0;
        while at < len {
            let n = npk_sys::gpudev_job_data(seq, at as i32, &mut px[at..]);
            if n <= 0 {
                return -22;
            }
            at += n as usize;
        }
        self.slot ^= 1;
        let off = self.slot * CURSOR_SLOT;
        if npk_sys::dma_write(self.dma, off as i32, &px) < 0 {
            return -22;
        }
        self.fb = Some(drm_framebuffer {
            format: drm_format_info(drm_core::regs::DRM_FORMAT_ARGB8888).unwrap(),
            width: w,
            height: h,
            pitches: [w * 4, 0, 0, 0],
            offsets: [0; 4],
            modifier: drm_core::regs::DRM_FORMAT_MOD_LINEAR as u64,
            dma_addr: [self.phys + off as u64, 0, 0, 0],
        });
        self.hot = (a[2] as i32, a[3] as i32);
        self.apply(vc4, hw)
    }

    /// `OP_CURSOR_MOVE`.
    fn moved(&mut self, vc4: &mut Vc4, hw: &mut PiHw, a: &[u32; 6]) -> i32 {
        self.pos = (a[0] as i32, a[1] as i32);
        self.shown = a[2] != 0;
        self.apply(vc4, hw)
    }

    /// The cursor plane of the output that shows the desktop: the image
    /// with its hotspot on the pointer (`drm_mode_cursor_universal`), or
    /// off.
    fn apply(&self, vc4: &mut Vc4, hw: &mut PiHw) -> i32 {
        let Some(crtc) = (0..vc4.crtc.len()).find(|&c| vc4.kms.crtcs[c].active) else { return 0 };
        let fb = self.fb.as_ref().filter(|_| self.shown);
        vc4.cursor_update(hw, crtc, fb, self.pos.0 - self.hot.0, self.pos.1 - self.hot.1)
    }
}

impl Present {
    /// One `OP_COPY_RECT`. A copy of a whole shadow to the framebuffer
    /// flips to that shadow; a copy out of the shadow on screen is there
    /// already. Anything else the kernel does on the CPU (-1).
    fn copy(&mut self, vc4: &mut Vc4, hw: &mut PiHw, a: &[u32; 6]) -> i32 {
        let (src, dst) = (a[0], a[1]);
        if dst != 0 || !(1..=2).contains(&src) {
            return -1;
        }
        if src == self.shown {
            return 0;
        }
        let s = self.shadows[src as usize - 1];
        if (a[2], a[3], a[4], a[5]) != (0, 0, s.width, s.height) {
            return -1;
        }
        let ret = vc4.page_flip(hw, &framebuffer(&s));
        if ret != 0 {
            Line::new().s("page flip failed ").dec(ret.unsigned_abs() as u64).say();
            return -1;
        }
        self.shown = src;
        0
    }
}

/// The fbdev client's choice on screen: the monitor's own mode (Linux's
/// pick from its EDID), with the kernel's framebuffer moved to that size
/// first. Outputs are turned off before the kernel lets go of the
/// framebuffer they read. Without a framebuffer of the mode the HVS
/// scales the one there is; without a monitor every output goes off.
fn show(vc4: &mut Vc4, hw: &mut PiHw, client: &drm_client_dev, kfb: &mut KernelFb) {
    let off = drm_client_dev::new(vc4.crtc.len() as u32);
    let Some(ms) = client.modesets.iter().find(|m| m.mode.is_some()) else {
        let ret = vc4.takeover_commit(hw, &off, &framebuffer(kfb));
        Line::new().s("no monitor connected").s(if ret != 0 { ", turning off failed" } else { "" }).say();
        return;
    };
    let m = ms.mode.clone().unwrap();
    let (w, h) = (m.hdisplay as u32, m.vdisplay as u32);

    if (w, h) != (kfb.width, kfb.height) {
        if vc4.kms.crtcs.iter().any(|c| c.active) {
            vc4.takeover_commit(hw, &off, &framebuffer(kfb));
        }
        if npk_sys::gpudev_framebuffer(w as i32, h as i32) == 0
            && let Some(new) = kernel_fb()
        {
            *kfb = new;
        } else {
            Line::new().s("no ").dec(w as u64).s("x").dec(h as u64)
                .s(" framebuffer from the kernel, the HVS scales the one there is").say();
        }
    }

    let conn = vc4.connectors[ms.connectors[0]].name.clone();
    let ret = vc4.takeover_commit(hw, client, &framebuffer(kfb));
    let mut l = Line::new();
    l.s(&conn).s(" ").dec(w as u64).s("x").dec(h as u64).s("@")
        .dec(vc4.vrefresh(ms.crtc as usize) as u64).s(", framebuffer ")
        .dec(kfb.width as u64).s("x").dec(kfb.height as u64).s(" at ").hex(kfb.phys as u32);
    if ret != 0 {
        l.s(": commit failed ").dec(ret.unsigned_abs() as u64);
    }
    l.say();
}

/// `drm_atomic_helper_shutdown` as the system goes down: V3D's interrupts
/// off, then every output off in one commit, which disables the HDMI
/// encoders and with them a sink's scrambling, so the firmware starts the
/// next boot on a display as it was at power-on.
fn shutdown(vc4: &mut Vc4, hw: &mut PiHw, kfb: &KernelFb, p: &mut Present) -> i32 {
    if let Some(s) = p.scenes.as_mut() {
        s.c.drain(hw);
        s.reap(hw, WOKE_DRAIN);
        gpu_bcm2712_core::v3d::irq_disable(hw);
    }
    let off = drm_client_dev::new(vc4.crtc.len() as u32);
    let ret = vc4.takeover_commit(hw, &off, &framebuffer(kfb));
    p.shown = 0;
    Line::new().s("outputs off for the shutdown").s(if ret != 0 { ", failed" } else { "" }).say();
    ret
}

/// The kernel's framebuffer as the plane's.
fn framebuffer(kfb: &KernelFb) -> drm_framebuffer {
    drm_framebuffer {
        format: drm_format_info(kfb.format).unwrap_or(drm_format_info(drm_core::regs::DRM_FORMAT_XRGB8888).unwrap()),
        width: kfb.width,
        height: kfb.height,
        pitches: [kfb.pitch, 0, 0, 0],
        offsets: [0; 4],
        modifier: drm_core::regs::DRM_FORMAT_MOD_LINEAR as u64,
        dma_addr: [kfb.phys, 0, 0, 0],
    }
}

/// The interrupts and the timed work of a running display, and the GPU
/// service's queue, which stays empty without the copy capability.
fn serve(vc4: &mut Vc4, hw: &mut PiHw, kfb: &mut KernelFb, p: &mut Present) {
    let mut jobs = [0u8; JOB_BYTES * 16];
    let mut seen = hw.arm_all();
    loop {
        let now = npk_sys::now_us() as u64;
        let mut timeout_ms = 1000u64;
        for h in vc4.hdmi.iter() {
            if let Some(due) = h.scrambling_work {
                timeout_ms = timeout_ms.min(due.saturating_sub(now) / 1000);
            }
        }

        if let Some(s) = p.scenes.as_ref() {
            if let Some(us) = s.c.next_due(now) {
                timeout_ms = timeout_ms.min(us.div_ceil(1000));
            }
        }

        let woke = npk_sys::wait(WAIT_IRQ | WAIT_GPU_CMD, timeout_ms.max(1) as i32);

        let fired = hw.fired(&mut seen);
        if let Some(s) = p.scenes.as_mut() {
            s.reap(hw, woke);
            s.c.autosuspend(hw, npk_sys::now_us() as u64);
        }
        for pv in 0..vc4.crtc.len() {
            if fired.pv[pv] {
                vc4.crtc_irq_handler(hw, pv);
            }
        }
        if fired.disp_intr {
            vc4.disp_intr_handler(hw);
        }
        if fired.aon_intr {
            // vc4_hdmi_hpd_irq_thread → the fbdev client's hotplug: probe
            // again, show its choice.
            if vc4.aon_intr_handler(hw) != 0 {
                let client = vc4.fbdev_probe(hw);
                show(vc4, hw, &client, kfb);
                // The commit showed the framebuffer again, without the
                // cursor plane.
                p.shown = 0;
                if let Some(c) = &p.cursor {
                    c.apply(vc4, hw);
                }
            }
        }
        seen = hw.arm_all();

        let now = npk_sys::now_us() as u64;
        for i in 0..vc4.hdmi.len() {
            if vc4.hdmi[i].scrambling_work.is_some_and(|due| now >= due) {
                let h = &mut vc4.hdmi[i];
                h.scrambling_wq(hw, &mut vc4.bsc);
            }
        }

        let n = npk_sys::gpudev_poll(&mut jobs);
        if n < 0 {
            Line::new().s("no longer the GPU service, stopping").say();
            return;
        }
        for j in 0..n as usize {
            let job = &jobs[j * JOB_BYTES..(j + 1) * JOB_BYTES];
            let word = |i: usize| u32::from_le_bytes([job[i * 4], job[i * 4 + 1], job[i * 4 + 2], job[i * 4 + 3]]);
            let mut a = [0u32; 6];
            for (k, v) in a.iter_mut().enumerate() {
                *v = word(2 + k);
            }
            let status = match (word(1), p.cursor.as_mut()) {
                (OP_COPY_RECT, _) => p.copy(vc4, hw, &a),
                (OP_CURSOR_IMAGE, Some(c)) => c.image(vc4, hw, word(0) as i32, &a),
                (OP_CURSOR_MOVE, Some(c)) => c.moved(vc4, hw, &a),
                (OP_BO_ADD, _) => p.scenes.as_mut().map_or(-1, |s| s.add(a[0])),
                (OP_BO_REMOVE, _) => p.scenes.as_mut().map_or(-1, |s| s.remove(hw, a[0])),
                (OP_SCENE, _) => match p.scenes.as_mut() {
                    Some(s) => match s.run(hw, word(0) as i32, &a) {
                        Some(st) => st,
                        None => continue,
                    },
                    None => -1,
                },
                (OP_SHUTDOWN, _) => shutdown(vc4, hw, kfb, p),
                _ => -1,
            };
            npk_sys::gpudev_complete(word(0) as i32, status);
        }
    }
}
