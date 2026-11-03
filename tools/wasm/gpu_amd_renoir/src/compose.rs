//! The gpudev scene executor on the compute ring:
//! the scene checked and planned by gpu_scene, run by compose.comp,
//! blur_down.comp and blur_box.comp in one IB of the compositor's own
//! client (amdgpu_cs.rs), the dispatches written as RADV writes them
//! (radv_stream.rs). Not a Linux file.
//!
//! The compositor is a client like a Vulkan application: its buffers are
//! GEM objects in its own VM, its programs, push constants and descriptor
//! sets live in RADV's 32-bit window (`ADDRESS32_HI`), and a scene is one
//! submission of two IBs, the queue preamble and the command buffer. A
//! scene that shows a tiled guest frame first runs import.comp over it
//! into a linear buffer, which compose.comp then reads.

use crate::amdgpu_cs::{self as cs, amdgpu_fpriv};
use crate::amdgpu_device::amdgpu_device;
use crate::amdgpu_object::{self as obj, amdgpu_kptr};
use crate::dma_fence::{self, Fence};
use crate::drm_mm::{drm_mm, drm_mm_node};
use crate::linux::Line;
use crate::radv_stream as rs;
use crate::regs::*;
use crate::shaders_gen as sg;
use alloc::vec::Vec;
use gpu_scene::plan::{self, EMPTY, bin_table, grid, item_shape, rect};

const EINVAL: i32 = 22;
const ENOMEM: i32 = 12;
const EIO: i32 = 5;
const E2BIG: i32 = 7;
const PAGE: u64 = 4096;

/// RADV's 32-bit window, where the programs and push constants live.
const HIGH32: u64 = (sg::ADDRESS32_HI as u64) << 32;
const CODE_VA: u64 = HIGH32 + 0x0010_0000;
const PUSH_VA: u64 = HIGH32 + 0x0020_0000;
const SET_VA: u64 = HIGH32 + 0x0030_0000;
const CODE_PAGES: u64 = 8;
/// The compositor's other buffers and the kernel's, below the window.
pub const DYN_START: u64 = 1 << 32;
pub const DYN_END: u64 = 1 << 47;

const SCENE_PAGES: u64 = 16;
/// Bin tables of one scene: per bin a header, then the item lists.
const BINS_PAGES: u64 = 128;
const BINS_WORDS: u32 = (BINS_PAGES * PAGE / 4) as u32;
/// Blocks one plane buffer holds: the blur grid of the largest panel.
const PLANE_PAGES: u64 = 128;
const PLANE_BLOCKS: u32 = (PLANE_PAGES * PAGE / 4) as u32;
/// Dispatches one scene can hold, and the push constants of each.
pub const MAX_DISPATCHES: u32 = 256;
const PUSH_SLOT: u64 = 128;
const PUSH_PAGES: u64 = MAX_DISPATCHES as u64 * PUSH_SLOT / PAGE;
/// Per dispatch one descriptor set, of the program's `set_bytes`.
const SET_SLOT: u64 = 64;
const SET_PAGES: u64 = MAX_DISPATCHES as u64 * SET_SLOT / PAGE;
const IB_PAGES: u64 = 16;
const PRE_PAGES: u64 = 1;

/// What every mapping of the compositor allows, as RADV maps its buffers.
const VM_FLAGS: u32 = AMDGPU_VM_PAGE_READABLE | AMDGPU_VM_PAGE_WRITEABLE | AMDGPU_VM_PAGE_EXECUTABLE;

/// A buffer of the compositor: GEM object, address in its VM, mapping.
#[derive(Clone, Copy)]
pub struct gem {
    pub bo: usize,
    pub va: u64,
    pub k: amdgpu_kptr,
    pub bytes: u64,
    /// The VA range, when it came from the allocator.
    node: Option<drm_mm_node>,
}

pub struct composer {
    pub fpriv: amdgpu_fpriv,
    pub ctx: u32,
    /// The VA allocator below RADV's window.
    pub mm: drm_mm,
    code: gem,
    push: gem,
    sets: gem,
    pre: gem,
    ib: gem,
    scene: gem,
    bins: gem,
    planes: gem,
    tmp: gem,
    code_off: [u64; 5],
    pre_dw: u32,
}

/// A buffer of `bytes` in `domain` mapped into the client at `fixed` or
/// from `mm`.
fn gem_new(adev: &mut amdgpu_device, fpriv: &amdgpu_fpriv, mm: &mut drm_mm, fixed: Option<u64>, bytes: u64,
           domain: u32) -> Result<gem, i32> {
    let bo = cs::amdgpu_gem_object_create(adev, bytes, PAGE, domain, 0)?;
    cs::amdgpu_gem_object_open(adev, fpriv, bo);
    let node = match fixed {
        Some(_) => None,
        None => match mm.insert_node_in_range_best(bytes, PAGE, 0, DYN_START, DYN_END) {
            Ok(n) => Some(n),
            Err(e) => {
                cs::amdgpu_gem_object_close(adev, fpriv, bo);
                obj::amdgpu_bo_unref(adev, bo);
                return Err(e);
            }
        },
    };
    let va = fixed.unwrap_or_else(|| node.unwrap().start);
    let undo = |adev: &mut amdgpu_device, mm: &mut drm_mm| {
        if let Some(n) = node {
            mm.remove_node(&n);
        }
        cs::amdgpu_gem_object_close(adev, fpriv, bo);
        obj::amdgpu_bo_unref(adev, bo);
    };
    if let Err(e) = cs::amdgpu_gem_va_ioctl(adev, fpriv, bo, true, va, 0, bytes, VM_FLAGS) {
        undo(adev, mm);
        return Err(e);
    }
    match obj::amdgpu_bo_kmap(adev, bo) {
        Ok(k) => Ok(gem { bo, va, k, bytes, node }),
        Err(e) => {
            undo(adev, mm);
            Err(e)
        }
    }
}

/// The buffer unmapped and released.
fn gem_free(adev: &mut amdgpu_device, fpriv: &amdgpu_fpriv, mm: &mut drm_mm, g: gem) {
    let _ = cs::amdgpu_gem_va_ioctl(adev, fpriv, g.bo, false, g.va, 0, g.bytes, 0);
    if let Some(n) = g.node {
        mm.remove_node(&n);
    }
    cs::amdgpu_gem_object_close(adev, fpriv, g.bo);
    obj::amdgpu_bo_unref(adev, g.bo);
}

fn write(adev: &amdgpu_device, g: &gem, off: u64, data: &[u8]) -> bool {
    if off + data.len() as u64 > g.bytes {
        return false;
    }
    crate::amdgpu_gart::kptr_write(adev, g.k, off, data);
    true
}

fn write_words(adev: &amdgpu_device, g: &gem, off: u64, words: &[u32]) -> bool {
    let mut buf = [0u8; PAGE as usize];
    for (i, chunk) in words.chunks(PAGE as usize / 4).enumerate() {
        for (k, v) in chunk.iter().enumerate() {
            buf[k * 4..k * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        if !write(adev, g, off + i as u64 * PAGE, &buf[..chunk.len() * 4]) {
            return false;
        }
    }
    true
}

/// The compositor's client, its context on the compute engine, its
/// buffers, the programs uploaded and the queue preamble written.
pub fn composer_init(adev: &mut amdgpu_device) -> Result<composer, i32> {
    let mut fpriv = cs::amdgpu_driver_open_kms(adev)?;
    let ctx = match crate::amdgpu_ctx::amdgpu_ctx_alloc(adev, &mut fpriv.ctx_mgr, fpriv.vm,
                                                        crate::amdgpu_ctx::PRIORITY_NORMAL) {
        Ok(c) => c,
        Err(e) => {
            cs::amdgpu_driver_postclose_kms(adev, fpriv);
            return Err(e);
        }
    };
    let mut mm = drm_mm::init(DYN_START, DYN_END - DYN_START);
    let mut made: Vec<gem> = Vec::new();
    let r = (|| -> Result<[gem; 9], i32> {
        let specs = [
            (Some(CODE_VA), CODE_PAGES),
            (Some(PUSH_VA), PUSH_PAGES),
            (Some(SET_VA), SET_PAGES),
            (None, PRE_PAGES),
            (None, IB_PAGES),
            (None, SCENE_PAGES),
            (None, BINS_PAGES),
            (None, PLANE_PAGES),
            (None, PLANE_PAGES),
        ];
        for (fixed, pages) in specs {
            let g = gem_new(adev, &fpriv, &mut mm, fixed, pages * PAGE, AMDGPU_GEM_DOMAIN_GTT)?;
            made.push(g);
        }
        Ok([made[0], made[1], made[2], made[3], made[4], made[5], made[6], made[7], made[8]])
    })();
    let g = match r {
        Ok(g) => g,
        Err(e) => {
            for g in made {
                gem_free(adev, &fpriv, &mut mm, g);
            }
            crate::amdgpu_cs::amdgpu_driver_postclose_kms(adev, fpriv);
            return Err(e);
        }
    };
    let mut c = composer {
        fpriv,
        ctx,
        mm,
        code: g[0],
        push: g[1],
        sets: g[2],
        pre: g[3],
        ib: g[4],
        scene: g[5],
        bins: g[6],
        planes: g[7],
        tmp: g[8],
        code_off: [0; 5],
        pre_dw: 0,
    };
    // The programs, each where a program address may point (256 bytes).
    let mut off = 0u64;
    for (i, k) in rs::KERNELS.iter().enumerate() {
        c.code_off[i] = off;
        if !write(adev, &c.code, off, k.code) {
            composer_fini(adev, c);
            return Err(-E2BIG);
        }
        off = (off + k.code.len() as u64).next_multiple_of(256);
    }
    let mut pre = Vec::new();
    rs::preamble(&mut pre);
    c.pre_dw = pre.len() as u32;
    if !write_words(adev, &c.pre, 0, &pre) {
        composer_fini(adev, c);
        return Err(-ENOMEM);
    }
    Ok(c)
}

/// The client closed, its buffers with it.
pub fn composer_fini(adev: &mut amdgpu_device, mut c: composer) {
    for g in [c.code, c.push, c.sets, c.pre, c.ib, c.scene, c.bins, c.planes, c.tmp] {
        gem_free(adev, &c.fpriv, &mut c.mm, g);
    }
    cs::amdgpu_driver_postclose_kms(adev, c.fpriv);
}

/// The dispatches of one submission.
pub struct batch {
    cmd: Vec<u32>,
    prev: Option<usize>,
    dispatches: u32,
}

impl batch {
    pub fn new() -> Self {
        batch { cmd: Vec::new(), prev: None, dispatches: 0 }
    }
}

/// One dispatch of program `k` over `grid` workgroups with `push` as its
/// push constants and, for a program that reads an image, `image` as the
/// image's descriptor; it starts after the dispatch before it has written
/// everything (the barrier of its template).
pub fn dispatch(adev: &mut amdgpu_device, c: &composer, b: &mut batch, k: usize, push: &[u8],
                image: Option<&[u32; 8]>, grid: [u32; 3]) -> Result<(), i32> {
    let kernel = rs::KERNELS[k];
    if push.len() as u32 > kernel.push_bytes || kernel.push_bytes as u64 > PUSH_SLOT || push.len() % 4 != 0
        || kernel.set_bytes as u64 > SET_SLOT || kernel.set_sgpr.is_some() != image.is_some() {
        return Err(-EINVAL);
    }
    if b.dispatches == MAX_DISPATCHES {
        return Err(-E2BIG);
    }
    if grid.iter().any(|&n| n == 0) {
        return Ok(());
    }
    let mut words = [0u32; (PUSH_SLOT / 4) as usize];
    for (i, w) in push.chunks(4).enumerate() {
        words[i] = u32::from_le_bytes(w.try_into().unwrap());
    }
    let words = &words[..(kernel.push_bytes / 4) as usize];
    let slot = b.dispatches as u64 * PUSH_SLOT;
    if !write_words(adev, &c.push, slot, words) {
        return Err(-ENOMEM);
    }
    let set = b.dispatches as u64 * SET_SLOT;
    if let Some(desc) = image {
        let mut words = [0u32; (SET_SLOT / 4) as usize];
        words[..8].copy_from_slice(desc);
        if !write_words(adev, &c.sets, set, &words[..(kernel.set_bytes / 4) as usize]) {
            return Err(-ENOMEM);
        }
    }
    let d = rs::dispatch {
        kernel,
        code_va: CODE_VA + c.code_off[k],
        push: words,
        push_ptr: (PUSH_VA + slot) as u32,
        set_ptr: (SET_VA + set) as u32,
        grid,
    };
    rs::emit(&mut b.cmd, rs::template_for(k, b.prev), &d);
    b.prev = Some(k);
    b.dispatches += 1;
    Ok(())
}

/// The batch submitted with the preamble, `bos` in its buffer list; its
/// fence.
pub fn submit(adev: &mut amdgpu_device, c: &mut composer, mut b: batch, bos: &[usize]) -> Result<Fence, i32> {
    rs::end(&mut b.cmd);
    if b.cmd.len() as u64 * 4 > c.ib.bytes {
        return Err(-E2BIG);
    }
    if !write_words(adev, &c.ib, 0, &b.cmd) {
        return Err(-ENOMEM);
    }
    let chunk = |va: u64, dw: u32| cs::drm_amdgpu_cs_chunk_ib {
        flags: 0,
        va_start: va,
        ib_bytes: dw * 4,
        ip_type: AMDGPU_HW_IP_COMPUTE,
        ip_instance: 0,
        ring: 0,
    };
    let ibs = [chunk(c.pre.va, c.pre_dw), chunk(c.ib.va, b.cmd.len() as u32)];
    let mut list: Vec<usize> =
        [c.code, c.push, c.sets, c.pre, c.ib, c.scene, c.bins, c.planes, c.tmp].iter().map(|g| g.bo).collect();
    for &h in bos {
        if !list.contains(&h) {
            list.push(h);
        }
    }
    let ctx = c.ctx;
    let (_, fence) = cs::amdgpu_cs_ioctl(adev, &mut c.fpriv, ctx, &ibs, &list)?;
    Ok(fence)
}

/// The dispatch self-test: fill_pattern into a buffer of the compositor,
/// read back.
pub fn self_test(adev: &mut amdgpu_device, c: &mut composer) -> bool {
    const W: u32 = 64;
    const H: u32 = 64;
    const SEED: u32 = 0x5a;
    let pitch = W * 4;
    let Ok(dst) = gem_new(adev, &c.fpriv, &mut c.mm, None, (pitch * H) as u64, AMDGPU_GEM_DOMAIN_GTT) else { return false };
    let ok = (|| -> Result<bool, i32> {
        if !write(adev, &dst, 0, &[0u8; (W * 4 * H) as usize]) {
            return Err(-ENOMEM);
        }
        let mut push = [0u8; 24];
        let mut put = |off: u32, v: &[u8]| push[off as usize..off as usize + v.len()].copy_from_slice(v);
        put(sg::FILL_PATTERN_PUSH_DST, &dst.va.to_le_bytes());
        put(sg::FILL_PATTERN_PUSH_WIDTH, &W.to_le_bytes());
        put(sg::FILL_PATTERN_PUSH_HEIGHT, &H.to_le_bytes());
        put(sg::FILL_PATTERN_PUSH_PITCH_PX, &W.to_le_bytes());
        put(sg::FILL_PATTERN_PUSH_SEED, &SEED.to_le_bytes());
        let mut b = batch::new();
        dispatch(adev, c, &mut b, rs::K_FILL_PATTERN, &push, None, [W.div_ceil(sg::WIDTH), H, 1])?;
        let t0 = crate::linux::now_us();
        let f = submit(adev, c, b, &[dst.bo])?;
        if dma_fence::dma_fence_wait_timeout(adev, &f, 80_000) == 0 {
            return Err(-62);
        }
        let us = crate::linux::now_us() - t0;
        if f.error() != 0 {
            return Err(f.error());
        }
        let mut row = [0u8; 256];
        for y in 0..H {
            crate::amdgpu_gart::kptr_read(adev, dst.k, (y * pitch) as u64, &mut row);
            for x in 0..W {
                let want = SEED << 24 | y << 12 | x;
                let v = u32::from_le_bytes(row[(x * 4) as usize..(x * 4 + 4) as usize].try_into().unwrap());
                if v != want {
                    Line::new().s("gpudev: dispatch self-test read back ").hex(v).s(" at ").dec(x as u64).s(",")
                        .dec(y as u64).s(", expected ").hex(want).dbg();
                    return Ok(false);
                }
            }
        }
        Line::new().s("gpudev: fill_pattern 64x64 on the compute ring in ").dec(us).s(" us, wave64, ")
            .dec(sg::FILL_PATTERN.code.len() as u64).s(" bytes of program").dbg();
        Ok(true)
    })();
    gem_free(adev, &c.fpriv, &mut c.mm, dst);
    match ok {
        Ok(v) => v,
        Err(e) => {
            Line::new().s("gpudev: dispatch self-test failed (").dec((-e) as u64).s(")").say();
            false
        }
    }
}

/// The job's payload, for gpu_scene.
struct job_payload(u32);

impl gpu_scene::scene::payload for job_payload {
    fn read(&self, off: u32, out: &mut [u8]) -> Result<(), i32> {
        if npk_sys::gpudev_job_data(self.0 as i32, off as i32, out) != out.len() as i32 {
            return Err(-EIO);
        }
        Ok(())
    }
}

/// The composer's scene buffer, for gpu_scene.
struct scene_mem<'a>(&'a amdgpu_device, gem);

impl gpu_scene::scene::scene_buffer for scene_mem<'_> {
    fn va(&self) -> u64 {
        self.1.va
    }

    fn bytes(&self) -> u32 {
        self.1.bytes as u32
    }

    fn write(&mut self, off: u32, data: &[u8]) -> bool {
        write(self.0, &self.1, off as u64, data)
    }
}

/// A tiled guest frame a scene may show: the descriptor of the frame
/// (import.rs) and the linear buffer import.comp writes it into, which is
/// the buffer the scene names.
#[derive(Clone, Copy)]
pub struct frame_import {
    pub desc: [u32; 8],
    pub dst: u64,
    pub dst_pitch_px: u32,
    pub width: u32,
    pub height: u32,
}

/// A linear buffer of the compositor for a guest frame, in VRAM: only the
/// GPU writes and reads it, and a frame is larger than one DMA allocation.
pub fn linear_new(adev: &mut amdgpu_device, c: &mut composer, bytes: u64) -> Result<gem, i32> {
    gem_new(adev, &c.fpriv, &mut c.mm, None, bytes.next_multiple_of(PAGE), AMDGPU_GEM_DOMAIN_VRAM)
}

pub fn linear_free(adev: &mut amdgpu_device, c: &mut composer, g: gem) {
    gem_free(adev, &c.fpriv, &mut c.mm, g);
}

/// The import of `f`: a workgroup per `WIDTH` pixels of a row.
fn import_frame(adev: &mut amdgpu_device, c: &composer, b: &mut batch, f: &frame_import) -> Result<(), i32> {
    let mut push = [0u8; 20];
    put(&mut push, sg::IMPORT_PUSH_DST, &f.dst.to_le_bytes());
    put(&mut push, sg::IMPORT_PUSH_DST_PITCH, &f.dst_pitch_px.to_le_bytes());
    put(&mut push, sg::IMPORT_PUSH_WIDTH, &f.width.to_le_bytes());
    put(&mut push, sg::IMPORT_PUSH_HEIGHT, &f.height.to_le_bytes());
    dispatch(adev, c, b, rs::K_IMPORT, &push[..sg::IMPORT.push_bytes as usize], Some(&f.desc),
             [f.width.div_ceil(sg::WIDTH), f.height, 1])
}

/// Runs the scene of job `seq` (`bytes` of payload) into `target_id`.
/// Once the scene is checked, `used` gives the buffer objects of the
/// buffers it names and the tiled guest frames among them, imported
/// first.
pub fn submit_scene(adev: &mut amdgpu_device, c: &mut composer, bufs: &dyn gpu_scene::scene::buffers,
                    used: &dyn Fn() -> (Vec<usize>, Vec<frame_import>), seq: u32, target_id: u32, bytes: u32)
                    -> Result<scene_inflight, i32> {
    let ck = gpu_scene::scene::check_scene(&job_payload(seq), bytes, bufs, &mut scene_mem(adev, c.scene), target_id)?;
    let (bos, imports) = used();
    let mut log = dispatch_log::default();
    let mut b = batch::new();
    for f in &imports {
        import_frame(adev, c, &mut b, f)?;
        log.push(rs::K_IMPORT, &rect { x: 0, y: 0, w: f.width, h: f.height }, 0);
    }
    let mut s = gpu_sink { adev, c, b, target: ck.target, width: ck.width, height: ck.height,
                           radius: ck.radius, log: &mut log };
    let lim = plan::limits { bins_words: BINS_WORDS, plane_blocks: PLANE_BLOCKS };
    plan::plan(&mut s, &lim, &ck.screen, &ck.damage, &ck.shapes, ck.radius)?;
    let b = s.b;
    let t0 = crate::linux::now_us();
    let f = submit(adev, c, b, &bos)?;
    Ok(scene_inflight { f, log, items: ck.shapes.len() as u32, shapes: ck.shapes, t0 })
}

/// A scene on the compute ring: the composer's buffers stay in use until
/// `poll_scene` reports it.
pub struct scene_inflight {
    f: Fence,
    log: dispatch_log,
    shapes: Vec<item_shape>,
    items: u32,
    t0: u64,
}

/// The report of scene `s` once it is done, None while it runs.
pub fn poll_scene(adev: &mut amdgpu_device, s: &scene_inflight) -> Option<Result<scene_report, i32>> {
    if !dma_fence::dma_fence_is_signaled(adev, &s.f) {
        return None;
    }
    let e = s.f.error();
    if e < 0 {
        s.log.say_items(&s.shapes);
        s.log.say();
        return Some(Err(e));
    }
    let us = s.f.timestamp().saturating_sub(s.t0);
    Some(Ok(scene_report { items: s.items, dispatches: s.log.n, gpu_us: us, log: s.log }))
}

/// What one scene did, for the log.
pub struct scene_report {
    pub items: u32,
    pub dispatches: u32,
    pub gpu_us: u64,
    pub log: dispatch_log,
}

const LOGGED: usize = 64;

/// The dispatches of a batch: program, area, and for a compose the list
/// entries over all bins.
#[derive(Clone, Copy)]
pub struct dispatch_log {
    n: u32,
    kernel: [u8; LOGGED],
    area: [rect; LOGGED],
    listed: [u32; LOGGED],
}

impl Default for dispatch_log {
    fn default() -> Self {
        Self { n: 0, kernel: [0; LOGGED], area: [EMPTY; LOGGED], listed: [0; LOGGED] }
    }
}

impl dispatch_log {
    fn push(&mut self, k: usize, area: &rect, listed: u32) {
        if (self.n as usize) < LOGGED {
            self.kernel[self.n as usize] = k as u8;
            self.area[self.n as usize] = *area;
            self.listed[self.n as usize] = listed;
        }
        self.n += 1;
    }

    /// One line per dispatch.
    pub fn say(&self) {
        for i in 0..(self.n as usize).min(LOGGED) {
            let a = self.area[i];
            let mut l = Line::new();
            l.s("gpudev:   ").s(match self.kernel[i] as usize {
                rs::K_COMPOSE => "compose",
                rs::K_BLUR_DOWN => "blur_down",
                rs::K_BLUR_BOX => "blur_box",
                rs::K_IMPORT => "import",
                _ => "?",
            }).s(" ").dec(a.w as u64).s("x").dec(a.h as u64).s("+").dec(a.x as u64).s("+").dec(a.y as u64);
            if self.kernel[i] as usize == rs::K_COMPOSE {
                l.s(", ").dec(self.listed[i] as u64).s(" list entries");
            }
            l.say();
        }
    }

    fn say_items(&self, items: &[item_shape]) {
        for (i, it) in items.iter().enumerate() {
            let r = it.reach;
            Line::new().s("gpudev:   item ").dec(i as u64).s(" kind ").dec(it.kind as u64).s(" reach ")
                .dec(r.w as u64).s("x").dec(r.h as u64).s("+").dec(r.x as u64).s("+").dec(r.y as u64).say();
        }
    }
}

fn put(push: &mut [u8], off: u32, v: &[u8]) {
    push[off as usize..off as usize + v.len()].copy_from_slice(v);
}

/// Emits plan.rs's steps into one batch.
struct gpu_sink<'a> {
    adev: &'a mut amdgpu_device,
    c: &'a composer,
    b: batch,
    target: gpu_scene::scene::surface,
    width: u32,
    height: u32,
    radius: u32,
    log: &'a mut dispatch_log,
}

impl plan::sink for gpu_sink<'_> {
    fn bins(&mut self, at: u32, words: &[u32]) -> Result<(), i32> {
        if !write_words(self.adev, &self.c.bins, at as u64 * 4, words) {
            return Err(-ENOMEM);
        }
        Ok(())
    }

    fn compose(&mut self, area: &rect, t: &bin_table, blur: Option<grid>) -> Result<(), i32> {
        let (a, grid) = gpu_scene::scene::compose(sg::WIDTH, self.c.scene.va, self.c.planes.va,
                                                  self.c.bins.va + t.at as u64 * 4, area, t, blur);
        let mut push = [0u8; 76];
        put(&mut push, sg::COMPOSE_PUSH_FRAME, &a.frame.to_le_bytes());
        put(&mut push, sg::COMPOSE_PUSH_PLANES, &a.planes.to_le_bytes());
        put(&mut push, sg::COMPOSE_PUSH_BINS, &a.bins.to_le_bytes());
        put(&mut push, sg::COMPOSE_PUSH_CLIP_X, &a.clip_x.to_le_bytes());
        put(&mut push, sg::COMPOSE_PUSH_CLIP_Y, &a.clip_y.to_le_bytes());
        put(&mut push, sg::COMPOSE_PUSH_CLIP_W, &a.clip_w.to_le_bytes());
        put(&mut push, sg::COMPOSE_PUSH_CLIP_H, &a.clip_h.to_le_bytes());
        put(&mut push, sg::COMPOSE_PUSH_GX0, &a.gx0.to_le_bytes());
        put(&mut push, sg::COMPOSE_PUSH_GY0, &a.gy0.to_le_bytes());
        put(&mut push, sg::COMPOSE_PUSH_BIN_X0, &a.bin_x0.to_le_bytes());
        put(&mut push, sg::COMPOSE_PUSH_BIN_Y0, &a.bin_y0.to_le_bytes());
        put(&mut push, sg::COMPOSE_PUSH_NBX, &a.nbx.to_le_bytes());
        put(&mut push, sg::COMPOSE_PUSH_BX0, &a.bx0.to_le_bytes());
        put(&mut push, sg::COMPOSE_PUSH_BY0, &a.by0.to_le_bytes());
        put(&mut push, sg::COMPOSE_PUSH_BSW, &a.bsw.to_le_bytes());
        put(&mut push, sg::COMPOSE_PUSH_BSH, &a.bsh.to_le_bytes());
        dispatch(self.adev, self.c, &mut self.b, rs::K_COMPOSE, &push[..sg::COMPOSE.push_bytes as usize], None, grid)?;
        self.log.push(rs::K_COMPOSE, area, t.listed);
        Ok(())
    }

    fn blur_down(&mut self, g0: grid) -> Result<(), i32> {
        let (a, grid) = gpu_scene::scene::blur_down(sg::WIDTH, &self.target, self.c.planes.va, self.width,
                                                    self.height, g0);
        let mut push = [0u8; 44];
        put(&mut push, sg::BLUR_DOWN_PUSH_TARGET, &a.target.to_le_bytes());
        put(&mut push, sg::BLUR_DOWN_PUSH_PLANES, &a.planes.to_le_bytes());
        put(&mut push, sg::BLUR_DOWN_PUSH_PITCH_PX, &a.pitch_px.to_le_bytes());
        put(&mut push, sg::BLUR_DOWN_PUSH_WIDTH, &a.width.to_le_bytes());
        put(&mut push, sg::BLUR_DOWN_PUSH_HEIGHT, &a.height.to_le_bytes());
        put(&mut push, sg::BLUR_DOWN_PUSH_BX0, &a.bx0.to_le_bytes());
        put(&mut push, sg::BLUR_DOWN_PUSH_BY0, &a.by0.to_le_bytes());
        put(&mut push, sg::BLUR_DOWN_PUSH_SW, &a.sw.to_le_bytes());
        put(&mut push, sg::BLUR_DOWN_PUSH_SH, &a.sh.to_le_bytes());
        dispatch(self.adev, self.c, &mut self.b, rs::K_BLUR_DOWN, &push[..sg::BLUR_DOWN.push_bytes as usize], None, grid)?;
        self.log.push(rs::K_BLUR_DOWN, &rect { x: a.bx0, y: a.by0, w: a.sw, h: a.sh }, 0);
        Ok(())
    }

    fn blur_box(&mut self, to_tmp: bool, lines: u32, len: u32, stride: u32, step: u32) -> Result<(), i32> {
        let (src, dst) =
            if to_tmp { (self.c.planes.va, self.c.tmp.va) } else { (self.c.tmp.va, self.c.planes.va) };
        let (a, grid) = gpu_scene::scene::blur_box(sg::WIDTH, src, dst, lines, len, stride, step, self.radius);
        let mut push = [0u8; 36];
        put(&mut push, sg::BLUR_BOX_PUSH_SRC, &a.src.to_le_bytes());
        put(&mut push, sg::BLUR_BOX_PUSH_DST, &a.dst.to_le_bytes());
        put(&mut push, sg::BLUR_BOX_PUSH_LINES, &a.lines.to_le_bytes());
        put(&mut push, sg::BLUR_BOX_PUSH_LEN, &a.len.to_le_bytes());
        put(&mut push, sg::BLUR_BOX_PUSH_STRIDE, &a.stride.to_le_bytes());
        put(&mut push, sg::BLUR_BOX_PUSH_STEP, &a.step.to_le_bytes());
        put(&mut push, sg::BLUR_BOX_PUSH_RADIUS, &a.radius.to_le_bytes());
        dispatch(self.adev, self.c, &mut self.b, rs::K_BLUR_BOX, &push[..sg::BLUR_BOX.push_bytes as usize], None, grid)?;
        self.log.push(rs::K_BLUR_BOX, &rect { x: 0, y: 0, w: len, h: lines }, 0);
        Ok(())
    }
}

/// The buffers a JPEG decode reads: the bitstream and the command stream,
/// kept from frame to frame and grown when a frame needs more.
#[derive(Default)]
pub struct jpeg_bufs {
    bits: Option<gem>,
    ib: Option<gem>,
}

/// One JPEG decode on the JPEG ring (`amdgpu_cs_ioctl`, the IB checked by
/// `amdgpu_jpeg_dec_parse_cs` as a guest's is): `bitstream` from
/// `jpeg_dec::normalize` into the YUYV buffer at `out_va`, `pitch_px`
/// pixels a row. `out_bos` go into the buffer list. Its fence.
pub fn jpeg_submit(adev: &mut amdgpu_device, c: &mut composer, j: &mut jpeg_bufs, bitstream: &[u8], out_va: u64,
                   pitch_px: u32, out_bos: &[usize]) -> Result<Fence, i32> {
    jpeg_submit_regs(adev, c, j, bitstream, out_va, pitch_px >> 4, 0, out_bos)
}

/// `jpeg_submit` with the pitch registers as given.
#[allow(clippy::too_many_arguments)]
fn jpeg_submit_regs(adev: &mut amdgpu_device, c: &mut composer, j: &mut jpeg_bufs, bitstream: &[u8], out_va: u64,
                    pitch: u32, uv_pitch: u32, out_bos: &[usize]) -> Result<Fence, i32> {
    jpeg_submit_variant(adev, c, j, bitstream, out_va, pitch, uv_pitch, 0, out_bos)
}

#[allow(clippy::too_many_arguments)]
fn jpeg_submit_variant(adev: &mut amdgpu_device, c: &mut composer, j: &mut jpeg_bufs, bitstream: &[u8], out_va: u64,
                       pitch: u32, uv_pitch: u32, variant: u32, out_bos: &[usize]) -> Result<Fence, i32> {
    let need = (bitstream.len() as u64).next_multiple_of(PAGE);
    if j.bits.as_ref().is_some_and(|g| g.bytes < need) {
        let g = j.bits.take().unwrap();
        gem_free(adev, &c.fpriv, &mut c.mm, g);
    }
    if j.bits.is_none() {
        j.bits = Some(gem_new(adev, &c.fpriv, &mut c.mm, None, need.next_power_of_two(), AMDGPU_GEM_DOMAIN_GTT)?);
    }
    if j.ib.is_none() {
        j.ib = Some(gem_new(adev, &c.fpriv, &mut c.mm, None, PAGE, AMDGPU_GEM_DOMAIN_GTT)?);
    }
    let (bits, ib) = (j.bits.as_ref().unwrap(), j.ib.as_ref().unwrap());
    if !write(adev, bits, 0, bitstream) {
        return Err(-ENOMEM);
    }
    let words = crate::jpeg_dec::build_ib_variant(bits.va, bitstream.len() as u32, out_va, pitch, uv_pitch, variant);
    if words.len() as u64 * 4 > ib.bytes || !write_words(adev, ib, 0, &words) {
        return Err(-E2BIG);
    }
    let chunk = cs::drm_amdgpu_cs_chunk_ib {
        flags: 0,
        va_start: ib.va,
        ib_bytes: words.len() as u32 * 4,
        ip_type: AMDGPU_HW_IP_VCN_JPEG,
        ip_instance: 0,
        ring: 0,
    };
    let mut list = alloc::vec![bits.bo, ib.bo];
    list.extend_from_slice(out_bos);
    let ctx = c.ctx;
    let (_, fence) = cs::amdgpu_cs_ioctl(adev, &mut c.fpriv, ctx, &[chunk], &list)?;
    Ok(fence)
}

/// Whether a JPEG decode's fence signaled, and with what.
pub fn jpeg_done(adev: &mut amdgpu_device, f: &Fence) -> Option<i32> {
    if !dma_fence::dma_fence_is_signaled(adev, f) {
        return None;
    }
    Some(f.error().min(0))
}

/// What the JPEG self-test writes into its buffers before the decode, to
/// tell whose bytes changed.
const ST_OUT: u8 = 0xAA;
const ST_ALIAS: u8 = 0xBB;

/// `gpu_amd_renoir.jpeg test`: one decode of a 64x16 4:2:2 picture into a
/// buffer of this client, with a second buffer at a VA with the same low
/// 32 bits, and the JPEG ring's fence and the MMHUB contexts read around
/// it. Says where the engine's output lands; offers nothing afterwards.
pub fn jpeg_selftest(adev: &mut amdgpu_device, c: &mut composer, j: &mut jpeg_bufs) {
    const PIC: &[u8] = include_bytes!("jpeg_selftest.jpg");
    let say = |s: &str| Line::new().s("jpeg test: ").s(s).say();
    let Some(r) = adev.jpeg.inst0_ring_dec else { say("no JPEG ring"); return };
    let mut bits = Vec::new();
    let hdr = match crate::jpeg_dec::normalize(PIC, &mut bits) {
        Ok(h) => h,
        Err(_) => { say("test picture refused by normalize"); return }
    };
    // Large enough to catch rows written at a wrong stride.
    const OUT: u64 = 16 * PAGE;
    let Ok(out) = gem_new(adev, &c.fpriv, &mut c.mm, None, OUT, AMDGPU_GEM_DOMAIN_GTT) else { say("no output buffer"); return };
    let alias_va = (out.va & 0xFFFF_FFFF) | (1u64 << 36);
    let alias = gem_new(adev, &c.fpriv, &mut c.mm, Some(alias_va), PAGE, AMDGPU_GEM_DOMAIN_GTT).ok();
    let _ = write(adev, &out, 0, &[ST_OUT; OUT as usize]);
    if let Some(a) = alias.as_ref() { let _ = write(adev, a, 0, &[ST_ALIAS; PAGE as usize]); }
    let fence_word = |adev: &amdgpu_device| -> Option<u32> {
        let drv = &adev.rings[r].fence_drv;
        Some(crate::amdgpu_gart::kptr_r32(adev, adev.wb.wb?, drv.cpu_addr? as u64 * 4))
    };
    let before = fence_word(adev);
    let mut l = Line::new();
    l.s("jpeg test: ").dec(hdr.width as u64).s("x").dec(hdr.height as u64).s(", bitstream ").dec(bits.len() as u64)
        .s(" B, out va 0x").hexw((out.va >> 32) as u32, 8).hexw(out.va as u32, 8)
        .s(", alias va 0x").hexw((alias_va >> 32) as u32, 8).hexw(alias_va as u32, 8)
        .s(if alias.is_some() { "" } else { " (not mapped)" })
        .s(", ring fence gpu 0x").hexw((adev.rings[r].fence_drv.gpu_addr >> 32) as u32, 8)
        .hexw(adev.rings[r].fence_drv.gpu_addr as u32, 8);
    l.say();
    let mut bos = alloc::vec![out.bo];
    if let Some(a) = alias.as_ref() { bos.push(a.bo); }
    let p16 = hdr.width >> 4;
    let variants: [(&str, u32, u32); 4] = [
        ("A pitch px/16, uv 0", p16, 0),
        ("B pitch px/16, uv px/16", p16, p16),
        ("C pitch bytes/16, uv bytes/16", p16 * 2, p16 * 2),
        ("D pitch 0, uv px/16", 0, p16),
    ];
    let mut all_done = true;
    for (name, pitch, uv) in variants {
        let _ = write(adev, &out, 0, &[ST_OUT; OUT as usize]);
        let f = match jpeg_submit_regs(adev, c, j, &bits, out.va, pitch, uv, &bos) {
            Ok(f) => f,
            Err(e) => { Line::new().s("jpeg test: submit failed (").dec((-e) as u64).s(")").say(); return }
        };
        let t0 = crate::linux::now_us();
        let mut done = None;
        // As the service loop: the scheduler hands the job to the ring from
        // its work queue, the interrupt signals the fence.
        while crate::linux::now_us() - t0 < 500_000 {
            crate::workqueue::run_pending(adev);
            crate::amdgpu_irq::amdgpu_ih_process(adev, false);
            crate::workqueue::run_pending(adev);
            if let Some(e) = jpeg_done(adev, &f) { done = Some(e); break; }
            crate::amdgpu_irq::amdgpu_irq_wait(adev, 2);
        }
        let mut buf = alloc::vec![0u8; OUT as usize];
        crate::amdgpu_gart::kptr_read(adev, out.k, 0, &mut buf);
        let changed = buf.iter().filter(|&&b| b != ST_OUT).count();
        let mut runs: Vec<(usize, usize)> = Vec::new();
        for (i, &b) in buf.iter().enumerate() {
            if b == ST_OUT { continue; }
            match runs.last_mut() {
                Some(r) if r.1 == i => r.1 = i + 1,
                _ => runs.push((i, i + 1)),
            }
        }
        let mut l = Line::new();
        l.s("jpeg test ").s(name).s(": ").s(match done { Some(0) => "done", Some(_) => "error", None => "no fence" })
            .s(", ").dec(changed as u64).s(" B written, runs");
        for (a, b) in runs.iter().take(20) { l.s(" ").dec(*a as u64).s("+").dec((b - a) as u64); }
        if runs.len() > 20 { l.s(" ... (").dec(runs.len() as u64).s(")"); }
        l.s(", first");
        for b in &buf[..8] { l.s(" ").hexw(*b as u32, 2); }
        l.say();
        if done.is_none() { all_done = false; break; }
    }
    // The camera's case without the camera: a 1280x720 4:2:2 frame without
    // DHT (ffmpeg's MJPEG, tables cut out), 30 times, the bitstream
    // written anew each time, into a buffer with room behind the frame.
    if all_done {
        const BIG: &[u8] = include_bytes!("jpeg_selftest_720.jpg");
        let mut bigbits = Vec::new();
        match crate::jpeg_dec::normalize(BIG, &mut bigbits) {
            Err(_) => say("720p picture refused by normalize"),
            Ok(h2) => {
                let frame = (h2.width * h2.height * 2) as u64;
                let room = frame.next_multiple_of(PAGE) + 16 * PAGE;
                match gem_new(adev, &c.fpriv, &mut c.mm, None, room, AMDGPU_GEM_DOMAIN_GTT) {
                    Err(_) => say("no 720p output buffer"),
                    Ok(big) => {
                        let mut good = 0u32;
                        let mut first_err = None;
                        for k in 0..30u32 {
                            let _ = write(adev, &big, 0, &alloc::vec![ST_OUT; room as usize]);
                            let mut bos = alloc::vec![big.bo];
                            if let Some(a) = alias.as_ref() { bos.push(a.bo); }
                            let variant = k % 3;
                            let Ok(f) = jpeg_submit_variant(adev, c, j, &bigbits, big.va, h2.width >> 4, 0, variant, &bos) else { say("720p submit failed"); break };
                            let t0 = crate::linux::now_us();
                            let mut d = None;
                            if k == 0 {
                                // Watch the engine through the decode: which register changes
                                // when the last row is written. MMIO offsets of the internal
                                // 0x40xx ones (0x4000 + n is 0x80 + n).
                                let regs = |adev: &amdgpu_device| {
                                    let r = crate::jpeg_v2_0::jpeg_rreg_mm;
                                    [r(adev, 0x9d), r(adev, 0x9e), r(adev, 0x8b), r(adev, 0x80), r(adev, 0x83),
                                     r(adev, 0xaf), crate::jpeg_v2_0::jrbc_status(adev)]
                                };
                                let last_row = (frame - 2560) as u64;
                                let mut prev = [0u32; 7];
                                let mut prev_rows = u64::MAX;
                                let mut lines = 0;
                                let mut row = [0u8; 4];
                                while crate::linux::now_us() - t0 < 20_000 && lines < 40 {
                                    crate::workqueue::run_pending(adev);
                                    crate::amdgpu_irq::amdgpu_ih_process(adev, false);
                                    let v = regs(adev);
                                    // How far the output got: the first written row from the bottom.
                                    let mut rows = 0u64;
                                    for r in (0..h2.height as u64).rev().step_by(16) {
                                        crate::amdgpu_gart::kptr_read(adev, big.k, r * 2560, &mut row);
                                        if row != [ST_OUT; 4] { rows = r + 1; break; }
                                    }
                                    let _ = last_row;
                                    if v != prev || rows != prev_rows {
                                        let mut l = Line::new();
                                        l.s("jpeg watch ").dec(crate::linux::now_us() - t0).s(" us: outbuf wptr ").hex(v[0])
                                            .s(" rptr ").hex(v[1]).s(" int_stat ").hex(v[2]).s(" cntl ").hex(v[3])
                                            .s(" rb_rptr ").hex(v[4]).s(" soft_rst ").hex(v[5]).s(" jrbc_status ").hex(v[6])
                                            .s(", rows ~").dec(rows).s(if jpeg_done(adev, &f).is_some() { ", fence" } else { "" });
                                        l.say();
                                        prev = v;
                                        prev_rows = rows;
                                        lines += 1;
                                    }
                                }
                            }
                            while crate::linux::now_us() - t0 < 300_000 {
                                crate::workqueue::run_pending(adev);
                                crate::amdgpu_irq::amdgpu_ih_process(adev, false);
                                crate::workqueue::run_pending(adev);
                                if let Some(e) = jpeg_done(adev, &f) { d = Some((e, crate::linux::now_us() - t0)); break; }
                                crate::amdgpu_irq::amdgpu_irq_wait(adev, 2);
                            }
                            let mut buf = alloc::vec![0u8; room as usize];
                            crate::amdgpu_gart::kptr_read(adev, big.k, 0, &mut buf);
                            let inside = buf[..frame as usize].iter().filter(|&&b| b != ST_OUT).count();
                            let behind = buf[frame as usize..].iter().filter(|&&b| b != ST_OUT).count();
                            let ok = matches!(d, Some((0, _))) && inside as u64 > frame * 9 / 10 && behind == 0;
                            if k < 9 {
                                let mut l = Line::new();
                                l.s("jpeg test 720p V").dec(variant as u64 + 1).s(" #").dec(k as u64).s(": ");
                                match d { Some((e, us)) => { l.s(if e == 0 { "done in " } else { "error in " }).dec(us).s(" us"); } None => { l.s("no fence"); } }
                                l.s(", ").dec(inside as u64).s(" of ").dec(frame).s(" B written at the fence").say();
                            }
                            if ok { good += 1; } else if first_err.is_none() {
                                first_err = Some(k);
                                let mut l = Line::new();
                                l.s("jpeg test 720p: decode ").dec(k as u64).s(": ").s(match d {
                                    Some((0, _)) => "done", Some(_) => "error", None => "no fence" })
                                    .s(", ").dec(inside as u64).s(" of ").dec(frame).s(" B written inside, ")
                                    .dec(behind as u64).s(" behind the frame");
                                l.say();
                                crate::jpeg_v2_0::amdgpu_jpeg_dump_ip_state(adev);
                                crate::jpeg_v2_0::amdgpu_jpeg_print_ip_state(adev);
                            }
                            if k == 0 {
                                // Contiguous from the start (the engine stopped or still
                                // runs) or scattered (the CPU reads stale cache lines)?
                                let mut runs: Vec<(usize, usize)> = Vec::new();
                                for (i, &b) in buf[..frame as usize].iter().enumerate() {
                                    if b == ST_OUT { continue; }
                                    match runs.last_mut() {
                                        Some(r) if r.1 + 64 >= i => r.1 = i + 1,
                                        _ => runs.push((i, i + 1)),
                                    }
                                }
                                let mut l = Line::new();
                                l.s("jpeg test 720p: written ranges (gaps under 64 B joined): ").dec(runs.len() as u64);
                                for (a, b) in runs.iter().take(12) { l.s(" ").dec(*a as u64).s("+").dec((b - a) as u64); }
                                l.say();
                                // Again after 30 ms: more now means the engine wrote after the fence.
                                let t1 = crate::linux::now_us();
                                while crate::linux::now_us() - t1 < 30_000 {
                                    crate::workqueue::run_pending(adev);
                                    crate::amdgpu_irq::amdgpu_irq_wait(adev, 2);
                                }
                                let mut again = alloc::vec![0u8; frame as usize];
                                crate::amdgpu_gart::kptr_read(adev, big.k, 0, &mut again);
                                let later = again.iter().filter(|&&b| b != ST_OUT).count();
                                Line::new().s("jpeg test 720p: 30 ms later ").dec(later as u64).s(" B written").say();
                                let mut l = Line::new();
                                l.s("jpeg test 720p: first decode ");
                                match d {
                                    Some((0, us)) => { l.s("ok in ").dec(us).s(" us"); }
                                    _ => { l.s("failed"); }
                                }
                                l.s(", first");
                                for b in &buf[..8] { l.s(" ").hexw(*b as u32, 2); }
                                l.s(", row 360");
                                for b in &buf[(2560 * 360) as usize..(2560 * 360 + 8) as usize] { l.s(" ").hexw(*b as u32, 2); }
                                l.say();
                            }
                            if d.is_none() { break; }
                        }
                        Line::new().s("jpeg test 720p: ").dec(good as u64).s(" of 30 decodes clean").say();
                        gem_free(adev, &c.fpriv, &mut c.mm, big);
                    }
                }
            }
        }
    }
    let done = if all_done { Some(0) } else { None };
    let after = fence_word(adev);
    if let Some(a) = alias.as_ref() {
        let mut abuf = alloc::vec![0u8; PAGE as usize];
        crate::amdgpu_gart::kptr_read(adev, a.k, 0, &mut abuf);
        let n = abuf.iter().filter(|&&b| b != ST_ALIAS).count();
        Line::new().s("jpeg test: alias buffer (same low 32 bits) changed ").dec(n as u64).s(" B").say();
    }
    let mut l = Line::new();
    l.s("jpeg test: ring fence word before 0x");
    match before { Some(v) => { l.hexw(v, 8); } None => { l.s("?"); } }
    l.s(", after 0x");
    match after { Some(v) => { l.hexw(v, 8); } None => { l.s("?"); } }
    l.s(", last emitted ").dec(adev.rings[r].fence_drv.sync_seq as u64);
    if let Some((rp, wp)) = crate::amdgpu_ring::amdgpu_ring_get_rptr_wptr(adev, r) {
        l.s(", ring rptr 0x").hexw(rp as u32, 4).s(" wptr 0x").hexw(wp as u32, 4);
    }
    l.say();
    // Which VMID on the MMHUB points at this client's page directory.
    let hub = adev.vmhub[adev.rings[r].vm_hub as usize];
    let root = crate::amdgpu_vm::base(adev, adev.vm(c.fpriv.vm).root).bo;
    let pd = root.map(|b| crate::amdgpu_gmc::amdgpu_gmc_pd_addr(adev, b)).unwrap_or(0);
    let mut l = Line::new();
    l.s("jpeg test: client pd 0x").hexw((pd >> 32) as u32, 8).hexw(pd as u32, 8).s(", mmhub contexts:");
    for vmid in 1..8u32 {
        let lo = adev.rreg32(hub.ctx0_ptb_addr_lo32 + vmid * hub.ctx_addr_distance).unwrap_or(0);
        let hi = adev.rreg32(hub.ctx0_ptb_addr_hi32 + vmid * hub.ctx_addr_distance).unwrap_or(0);
        l.s(" ").dec(vmid as u64).s("=").hexw(hi, 8).hexw(lo, 8);
    }
    l.say();
    crate::jpeg_v2_0::amdgpu_jpeg_dump_ip_state(adev);
    crate::jpeg_v2_0::amdgpu_jpeg_print_ip_state(adev);
    if done.is_some() {
        gem_free(adev, &c.fpriv, &mut c.mm, out);
        if let Some(a) = alias { gem_free(adev, &c.fpriv, &mut c.mm, a); }
    }
}
