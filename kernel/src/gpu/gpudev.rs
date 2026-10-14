//! gpudev: the kernel side of a GPU driver module (docs/plan/GPU_STACK.md
//! §3.3, v1).
//!
//! Without a module the compositor copies and composes on the CPU (GOP). A
//! module that binds the display controller registers here, binds the
//! kernel's buffers into its own GPU address space and reports what its
//! self-tests proved; only then does the active GPU driver become
//! `GpudevDriver`, which hands the compositor's copies to the module, and
//! `shade::gpu_scene` may hand it scene lists. A rejected or late job, and
//! the module's death, put the work back on the CPU.
//!
//! Buffers belong to the kernel. Buffer 0 is the framebuffer the module took
//! over from the firmware, 1 and 2 are the compositor's shadow buffers. Every
//! buffer a scene reads gets a handle from 3 on when it is first used
//! (`bo_for`); a handle is never reused. When its owner frees a registered
//! buffer (`release`), the pages stay allocated until the module has
//! unbound them.
//!
//! Jobs are done in the order they are queued. Each buffer remembers the
//! last job that reads or writes it (its fence); a CPU writer waits only for
//! that one.

use alloc::boxed::Box;
use alloc::collections::{BTreeMap, VecDeque};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

use super::{FramebufferInfo, GpuError, GpuHal, ModeInfo};
use crate::drivers::pci::PciAddr;
use crate::smp::fiber::{self, Waker, NO_WAKER};

pub const VERSION: i32 = 1;
pub const OP_COPY_RECT: u32 = 1;
pub const OP_BO_ADD: u32 = 2;
pub const OP_BO_REMOVE: u32 = 3;
pub const OP_SCENE: u32 = 4;
/// Bytes per job as `npk_gpudev_poll` writes it: `seq, op, a0..a5`.
pub const JOB_BYTES: usize = 32;
/// Bytes `npk_gpudev_bo_info` writes.
pub const BO_INFO_BYTES: usize = 40;
/// `npk_gpudev_ready` capabilities.
pub const CAP_COPY: i32 = 1;
pub const CAP_SCENE: i32 = 2;
const FIXED_BOS: u32 = 3;
const JOB_TIMEOUT_MS: u64 = 100;
/// Binding a new buffer is a page walk per 4 KB page in the module, once.
const BIND_TIMEOUT_MS: u64 = 2000;
const DRIVER_NAME: &str = "gpudev (driver module)";
/// XRGB8888, the only format the compositor writes.
const FORMAT_XRGB8888: u32 = 0x3432_5258;
const PAGE: u64 = 4096;

#[derive(Clone, Copy, PartialEq, Eq)]
enum BoState {
    /// Queued for the module, or fixed and not yet reported bound.
    Adding,
    Bound,
    /// The module could not bind it; scenes with it stay on the CPU.
    Failed,
}

#[derive(Clone, Copy)]
pub struct Bo {
    /// Kernel (identity-mapped) address of the first pixel; 0 for buffer 0,
    /// which the module finds behind its GGTT node.
    pub addr: u64,
    /// Bytes from the first pixel.
    pub size: u64,
    pub pitch: u32,
    pub width: u32,
    pub height: u32,
    pub format: u32,
    /// Where the module bound the first pixel in its GPU address space.
    pub gpu_addr: Option<u64>,
    state: BoState,
    /// The last job that reads or writes it.
    fence: u32,
}

impl Bo {
    /// Physical page of the first pixel and the pixel's offset in it.
    fn first_page(&self) -> (u64, u32) {
        (self.addr & !(PAGE - 1), (self.addr & (PAGE - 1)) as u32)
    }

    fn pages(&self) -> u32 {
        let (_, off) = self.first_page();
        (off as u64 + self.size).div_ceil(PAGE) as u32
    }
}

#[derive(Clone, Copy)]
struct Job {
    seq: u32,
    op: u32,
    a: [u32; 6],
}

struct State {
    owner: Option<PciAddr>,
    fb: Option<FramebufferInfo>,
    bos: BTreeMap<u32, Bo>,
    /// Live dynamic buffers by the address of their first pixel.
    by_addr: BTreeMap<u64, u32>,
    next_id: u32,
    queue: VecDeque<Job>,
    /// Payloads of jobs not yet completed, by sequence number.
    payloads: BTreeMap<u32, Vec<u8>>,
    /// Frames to give back once the module completed the removal job
    /// (`release`), by that job's sequence number.
    pending_free: BTreeMap<u32, (u64, usize)>,
    /// Buffers being added, by the sequence number of their `BO_ADD`.
    adding: BTreeMap<u32, u32>,
    /// While a scene is encoded (`ScenePin`): the buffers it names. One
    /// whose owner frees it meanwhile is removed only after the scene.
    pinning: bool,
    pinned: Vec<u32>,
    /// `BO_ADD`s queued while pinning.
    pin_adds: u32,
    deferred: Vec<(u32, (u64, usize))>,
    caps: i32,
}

static STATE: Mutex<State> = Mutex::new(State {
    owner: None,
    fb: None,
    bos: BTreeMap::new(),
    by_addr: BTreeMap::new(),
    next_id: FIXED_BOS,
    queue: VecDeque::new(),
    payloads: BTreeMap::new(),
    pending_free: BTreeMap::new(),
    adding: BTreeMap::new(),
    pinning: false,
    pinned: Vec::new(),
    pin_adds: 0,
    deferred: Vec::new(),
    caps: 0,
});

/// The module serves jobs (registered, self-test passed, not dead).
static READY: AtomicBool = AtomicBool::new(false);
/// It also runs scene lists (`CAP_SCENE`).
static SCENES: AtomicBool = AtomicBool::new(false);
static NEXT_SEQ: AtomicU32 = AtomicU32::new(1);
/// Highest sequence number completed; jobs complete in order.
static COMPLETED: AtomicU32 = AtomicU32::new(0);
/// Highest sequence number of a job that touches a dynamic buffer: while
/// it is completed, no CPU writer of a dynamic buffer needs to look further.
static DYN_FENCE: AtomicU32 = AtomicU32::new(0);
static WAKER: AtomicU32 = AtomicU32::new(NO_WAKER);

/// Status of the last 64 completed jobs, `seq << 32 | status`.
static STATUS: [AtomicU64; 64] = [const { AtomicU64::new(0) }; 64];

/// `a` is at or after `b` in sequence-number order.
fn seq_ge(a: u32, b: u32) -> bool {
    a.wrapping_sub(b) as i32 >= 0
}

/// `npk_gpudev_register`: the module that owns the display controller at
/// `dev` becomes the GPU service. The fixed buffers are described from the
/// framebuffer the kernel draws into and its two shadows.
pub fn register(dev: PciAddr, version: i32) -> i32 {
    if version != VERSION {
        return -2;
    }
    let Some(fb) = super::framebuffer_info() else { return -1 };
    let Some((pa, pb, pages)) = crate::drivers::framebuffer::shadow_phys_info() else { return -1 };
    if pages == 0 || fb.bpp != 32 {
        return -1;
    }
    let mut st = STATE.lock();
    if st.owner.is_some() {
        return -3;
    }
    let shadow = |addr| Bo {
        addr,
        size: pages as u64 * PAGE,
        pitch: fb.pitch,
        width: fb.width,
        height: fb.height,
        format: FORMAT_XRGB8888,
        gpu_addr: None,
        state: BoState::Adding,
        fence: 0,
    };
    st.owner = Some(dev);
    st.fb = Some(fb);
    st.bos.clear();
    st.by_addr.clear();
    st.bos.insert(0, Bo { addr: 0, size: fb.pitch as u64 * fb.height as u64, ..shadow(0) });
    st.bos.insert(1, shadow(pa));
    st.bos.insert(2, shadow(pb));
    st.queue.clear();
    st.payloads.clear();
    st.adding.clear();
    st.caps = 0;
    crate::kprintln!("[npk] gpudev: {:02x}:{:02x}.{} registered, {}x{} framebuffer, 2 shadow buffers",
        dev.bus, dev.device, dev.function, fb.width, fb.height);
    0
}

fn owned_by(dev: PciAddr) -> bool {
    STATE.lock().owner == Some(dev)
}

/// `npk_gpudev_bo_info`: `{phys, size: u64, pitch, width, height, format,
/// offset, pages: u32}`.
pub fn bo_info(dev: PciAddr, id: i32) -> Option<[u8; BO_INFO_BYTES]> {
    let st = STATE.lock();
    if st.owner != Some(dev) {
        return None;
    }
    let bo = st.bos.get(&(id as u32))?;
    let (phys, off) = bo.first_page();
    let mut o = [0u8; BO_INFO_BYTES];
    o[0..8].copy_from_slice(&phys.to_le_bytes());
    o[8..16].copy_from_slice(&bo.size.to_le_bytes());
    for (i, v) in [bo.pitch, bo.width, bo.height, bo.format, off, bo.pages()].iter().enumerate() {
        o[16 + i * 4..20 + i * 4].copy_from_slice(&v.to_le_bytes());
    }
    Some(o)
}

/// `npk_gpudev_bo_pages`: physical addresses of pages `first..` of buffer
/// `id` into `out` (8 bytes each). Buffer 0 has none: the module knows the
/// framebuffer's pages from its GGTT.
pub fn bo_pages(dev: PciAddr, id: i32, first: u32, out: &mut [u8]) -> i32 {
    let bo = {
        let st = STATE.lock();
        if st.owner != Some(dev) || id <= 0 {
            return -1;
        }
        match st.bos.get(&(id as u32)) {
            Some(b) => *b,
            _ => return -1,
        }
    };
    let (page0, _) = bo.first_page();
    let total = bo.pages();
    let mut n = 0;
    let mut p = first;
    while p < total && (n + 1) * 8 <= out.len() {
        let Some(phys) = crate::mm::paging::translate(page0 + p as u64 * PAGE) else { return -1 };
        out[n * 8..n * 8 + 8].copy_from_slice(&(phys & !(PAGE - 1)).to_le_bytes());
        n += 1;
        p += 1;
    }
    n as i32
}

/// `npk_gpudev_bo_bound`: where the first pixel of buffer `id` lies.
pub fn bo_bound(dev: PciAddr, id: i32, gpu_addr: i64) -> i32 {
    let mut st = STATE.lock();
    if st.owner != Some(dev) || gpu_addr < 0 {
        return -1;
    }
    let fixed = (id as u32) < FIXED_BOS;
    // The fixed buffers are named to the GPU HAL by 32-bit addresses.
    if fixed && gpu_addr > u32::MAX as i64 {
        return -1;
    }
    match st.bos.get_mut(&(id as u32)) {
        Some(bo) => {
            bo.gpu_addr = Some(gpu_addr as u64);
            bo.state = BoState::Bound;
            0
        }
        _ => -1,
    }
}

/// `npk_gpudev_ready`: with a passed self-test and the fixed buffers bound,
/// the compositor's copies go to the module from the next frame on, and with
/// `CAP_SCENE` its scenes may (`shade::gpu_scene` checks them first).
pub fn ready(dev: PciAddr, caps: i32) -> i32 {
    if caps & CAP_COPY == 0 {
        if !owned_by(dev) {
            return -1;
        }
        crate::kprintln!("[npk] gpudev: module self-test failed, compositor stays on the CPU");
        return 0;
    }
    let (fb, a, b) = {
        let mut st = STATE.lock();
        if st.owner != Some(dev) {
            return -1;
        }
        let addr = |i: u32| st.bos.get(&i).and_then(|bo| bo.gpu_addr);
        let (Some(fb), Some(_), Some(a), Some(b)) = (st.fb, addr(0), addr(1), addr(2)) else {
            crate::kprintln!("[npk] gpudev: ready without all buffers bound, staying on the CPU");
            return -1;
        };
        st.caps = caps;
        (fb, a as u32, b as u32)
    };
    // Lock order of the compositor is framebuffer, then GPU; never both here.
    crate::drivers::framebuffer::set_shadow_ggtt(a, b);
    super::install(Box::new(GpudevDriver { fb }));
    READY.store(true, Ordering::Release);
    SCENES.store(caps & CAP_SCENE != 0, Ordering::Release);
    crate::kprintln!("[npk] gpudev: module ready, compositor copies on the GPU{}",
        if caps & CAP_SCENE != 0 { ", scenes offered" } else { "" });
    0
}

/// `npk_gpudev_poll`: up to `out.len() / JOB_BYTES` jobs into `out`.
pub fn poll(dev: PciAddr, out: &mut [u8]) -> i32 {
    let mut st = STATE.lock();
    if st.owner != Some(dev) {
        return -1;
    }
    let mut n = 0;
    while (n + 1) * JOB_BYTES <= out.len() {
        let Some(job) = st.queue.pop_front() else { break };
        let o = &mut out[n * JOB_BYTES..(n + 1) * JOB_BYTES];
        o[0..4].copy_from_slice(&job.seq.to_le_bytes());
        o[4..8].copy_from_slice(&job.op.to_le_bytes());
        for (i, v) in job.a.iter().enumerate() {
            o[8 + i * 4..12 + i * 4].copy_from_slice(&v.to_le_bytes());
        }
        n += 1;
    }
    n as i32
}

/// `npk_gpudev_job_data`: bytes `offset..` of the payload of job `seq`.
pub fn job_data(dev: PciAddr, seq: u32, offset: u32, out: &mut [u8]) -> i32 {
    let st = STATE.lock();
    if st.owner != Some(dev) {
        return -1;
    }
    let Some(p) = st.payloads.get(&seq) else { return -1 };
    let off = offset as usize;
    if off > p.len() {
        return -1;
    }
    let n = out.len().min(p.len() - off);
    out[..n].copy_from_slice(&p[off..off + n]);
    n as i32
}

/// `npk_gpudev_complete`.
pub fn complete(dev: PciAddr, seq: u32, status: i32) -> i32 {
    let free = {
        let mut st = STATE.lock();
        if st.owner != Some(dev) {
            return -1;
        }
        st.payloads.remove(&seq);
        // A buffer the module could not bind fails every scene that names it.
        if let Some(id) = st.adding.remove(&seq) {
            if status < 0 {
                if let Some(bo) = st.bos.get_mut(&id) {
                    bo.state = BoState::Failed;
                }
            }
        }
        st.pending_free.remove(&seq)
    };
    STATUS[seq as usize % STATUS.len()].store((seq as u64) << 32 | status as u32 as u64, Ordering::Release);
    COMPLETED.store(seq, Ordering::Release);
    if let Some((phys, pages)) = free {
        crate::memory::deallocate_contiguous(phys, pages);
    }
    0
}

/// Jobs waiting for the module (`npk_wait(WAIT_GPU_CMD)`).
pub fn pending(dev: PciAddr) -> bool {
    let st = STATE.lock();
    st.owner == Some(dev) && !st.queue.is_empty()
}

pub fn set_waker(dev: PciAddr, w: Waker) {
    if owned_by(dev) {
        WAKER.store(w, Ordering::Release);
    }
}

/// The module that owned `dev` is gone: jobs in flight fail, buffers whose
/// removal it never completed are freed, and the compositor goes back to
/// the CPU, copying into the framebuffer the module last showed. No
/// register is touched.
pub fn module_gone(dev: PciAddr) {
    let (fb, frees) = {
        let mut st = STATE.lock();
        if st.owner != Some(dev) {
            return;
        }
        st.owner = None;
        st.queue.clear();
        st.payloads.clear();
        st.bos.clear();
        st.by_addr.clear();
        st.adding.clear();
        st.caps = 0;
        let mut frees: Vec<(u64, usize)> = core::mem::take(&mut st.pending_free).into_values().collect();
        frees.extend(core::mem::take(&mut st.deferred).into_iter().map(|(_, f)| f));
        (st.fb.take(), frees)
    };
    READY.store(false, Ordering::Release);
    SCENES.store(false, Ordering::Release);
    WAKER.store(NO_WAKER, Ordering::Release);
    for (phys, pages) in frees {
        crate::memory::deallocate_contiguous(phys, pages);
    }
    if super::driver_name() == DRIVER_NAME {
        crate::drivers::framebuffer::set_shadow_ggtt(0, 0);
        if let Some(fb) = fb {
            super::install(Box::new(super::gop::GopDriver::from_framebuffer(fb)));
        }
        crate::kprintln!("[npk] gpudev: module gone, compositor on the CPU");
    }
}

fn wake_module() {
    let w = WAKER.load(Ordering::Acquire);
    if w != NO_WAKER {
        fiber::signal(w, fiber::SIG_GPU);
    }
}

/// Queue a job; the caller holds the state lock. Every buffer in `touches`
/// gets the job as its fence.
fn queue_job(st: &mut State, op: u32, a: [u32; 6], payload: Option<Vec<u8>>, touches: &[u32]) -> u32 {
    let seq = NEXT_SEQ.fetch_add(1, Ordering::Relaxed);
    if let Some(p) = payload {
        st.payloads.insert(seq, p);
    }
    let mut dynamic = false;
    for id in touches {
        if let Some(bo) = st.bos.get_mut(id) {
            bo.fence = seq;
            dynamic |= *id >= FIXED_BOS;
        }
    }
    if dynamic {
        DYN_FENCE.store(seq, Ordering::Release);
    }
    st.queue.push_back(Job { seq, op, a });
    seq
}

/// The handle of the buffer whose first pixel is at kernel address `addr`,
/// registering it with the module on first use. None when there is no
/// module serving scenes, or the module could not bind it.
pub fn bo_for(addr: u64, size: u64, width: u32, height: u32, pitch: u32) -> Option<u32> {
    if !SCENES.load(Ordering::Acquire) || addr == 0 || size == 0 {
        return None;
    }
    let mut st = STATE.lock();
    st.owner?;
    // A fixed buffer is named by its own handle.
    if let Some((&id, _)) = st.bos.iter().take(FIXED_BOS as usize)
        .find(|(id, b)| **id != 0 && b.addr == addr && b.pitch == pitch && b.width == width && b.height == height)
    {
        return Some(id);
    }
    if let Some(&id) = st.by_addr.get(&addr) {
        let bo = st.bos.get(&id).copied()?;
        if bo.size == size && bo.width == width && bo.height == height && bo.pitch == pitch {
            if bo.state == BoState::Failed {
                return None;
            }
            if st.pinning {
                st.pinned.push(id);
            }
            return Some(id);
        }
        // Freed without `release` and the address reused for a different
        // buffer: the module forgets the old one.
        st.by_addr.remove(&addr);
        retire(&mut st, id, None);
    }
    let id = st.next_id;
    st.next_id += 1;
    st.bos.insert(id, Bo {
        addr, size, pitch, width, height, format: FORMAT_XRGB8888,
        gpu_addr: None, state: BoState::Adding, fence: 0,
    });
    st.by_addr.insert(addr, id);
    let seq = queue_job(&mut st, OP_BO_ADD, [id, 0, 0, 0, 0, 0], None, &[id]);
    st.adding.insert(seq, id);
    if st.pinning {
        st.pinned.push(id);
        st.pin_adds += 1;
    }
    drop(st);
    wake_module();
    Some(id)
}

/// Queue the removal of buffer `id`; `free` is given back once the module
/// completed it.
fn retire(st: &mut State, id: u32, free: Option<(u64, usize)>) {
    st.bos.remove(&id);
    let seq = queue_job(st, OP_BO_REMOVE, [id, 0, 0, 0, 0, 0], None, &[]);
    if let Some(f) = free {
        st.pending_free.insert(seq, f);
    }
}

/// Held while a scene is encoded and handed over: the buffers `bo_for`
/// names stay known to the module until the scene is queued, so a buffer
/// replaced meanwhile (a widget raster, a guest frame) cannot be removed
/// ahead of the scene that reads it.
pub struct ScenePin;

impl ScenePin {
    pub fn new() -> Self {
        let mut st = STATE.lock();
        st.pinning = true;
        st.pinned.clear();
        st.pin_adds = 0;
        ScenePin
    }

    /// New buffers were queued for binding while pinned.
    pub fn added(&self) -> bool {
        STATE.lock().pin_adds > 0
    }
}

impl Drop for ScenePin {
    fn drop(&mut self) {
        let mut st = STATE.lock();
        st.pinning = false;
        st.pinned.clear();
        let deferred = core::mem::take(&mut st.deferred);
        let any = !deferred.is_empty();
        for (id, free) in deferred {
            retire(&mut st, id, Some(free));
        }
        drop(st);
        if any {
            wake_module();
        }
    }
}

/// The owner of the frames `phys..+pages`, with its first pixel at `addr`,
/// gives them up. True when the caller frees them now; false when gpudev
/// does, after the module unbound them.
pub fn release(addr: u64, phys: u64, pages: usize) -> bool {
    let mut st = STATE.lock();
    let Some(id) = st.by_addr.remove(&addr) else { return true };
    if st.owner.is_none() {
        st.bos.remove(&id);
        return true;
    }
    if st.pinned.contains(&id) {
        st.deferred.push((id, (phys, pages)));
        return false;
    }
    retire(&mut st, id, Some((phys, pages)));
    drop(st);
    wake_module();
    false
}

/// Wait until job `seq` is completed, at most until `deadline` (TSC). Its
/// status: 0, the module's error, or -ETIME (62).
fn wait_seq(seq: u32, deadline: u64) -> i32 {
    loop {
        if seq_ge(COMPLETED.load(Ordering::Acquire), seq) {
            let s = STATUS[seq as usize % STATUS.len()].load(Ordering::Acquire);
            return if (s >> 32) as u32 == seq { s as u32 as i32 } else { -5 };
        }
        if !READY.load(Ordering::Acquire) || crate::interrupts::rdtsc() >= deadline {
            // A job the module never took must not run after the CPU redid it.
            let mut st = STATE.lock();
            st.queue.retain(|j| j.seq != seq);
            st.payloads.remove(&seq);
            return -62;
        }
        core::hint::spin_loop();
    }
}

fn deadline() -> u64 {
    crate::interrupts::rdtsc() + JOB_TIMEOUT_MS * (crate::interrupts::tsc_freq() / 1000)
}

/// Before the CPU writes the buffer whose first pixel is at `addr`: wait
/// for the GPU job that last read it. Cheap when no job on a dynamic buffer
/// is in flight.
pub fn wait_cpu_access(addr: u64) {
    if seq_ge(COMPLETED.load(Ordering::Acquire), DYN_FENCE.load(Ordering::Acquire)) {
        return;
    }
    let fence = {
        let st = STATE.lock();
        st.by_addr.get(&addr).and_then(|id| st.bos.get(id)).map(|b| b.fence)
    };
    if let Some(f) = fence {
        if f != 0 {
            wait_seq(f, deadline());
        }
    }
}

/// Copies the module completed, and those that failed or timed out (the
/// CPU redid them), for `gpu`.
static COPIES_DONE: AtomicU64 = AtomicU64::new(0);
static COPIES_FAILED: AtomicU64 = AtomicU64::new(0);

/// The copy handed to the module and not yet waited for (0: none).
static COPY_INFLIGHT: AtomicU32 = AtomicU32::new(0);
static COPY_DEADLINE: AtomicU64 = AtomicU64::new(0);

/// Hand one copy to the module without waiting for it. The copy reads a
/// shadow buffer and writes the framebuffer, so the CPU must not touch
/// either until it is done: `framebuffer::with_fb`, the way into both,
/// calls `wait_framebuffer` first. False when the job was not taken, so the
/// caller copies on the CPU now.
fn submit_copy(src_gpu: u32, src_pitch: u32, dst_gpu: u32, dst_pitch: u32,
               x: u32, y: u32, w: u32, h: u32) -> bool {
    if !READY.load(Ordering::Acquire) || !wait_framebuffer() {
        return false;
    }
    let seq = {
        let mut st = STATE.lock();
        let find = |addr: u32, pitch: u32| {
            (0..FIXED_BOS).find(|i| {
                st.bos.get(i).is_some_and(|b| b.gpu_addr == Some(addr as u64) && b.pitch == pitch)
            })
        };
        let (Some(src), Some(dst)) = (find(src_gpu, src_pitch), find(dst_gpu, dst_pitch)) else {
            return false;
        };
        queue_job(&mut st, OP_COPY_RECT, [src, dst, x, y, w, h], None, &[src, dst])
    };
    COPY_DEADLINE.store(deadline(), Ordering::Release);
    COPY_INFLIGHT.store(seq, Ordering::Release);
    wake_module();
    true
}

/// Wait for the fences of the framebuffer and both shadows. False when the
/// last copy failed, timed out or its module died: the framebuffer may not
/// show the last frame, and the caller copies it on the CPU.
pub fn wait_framebuffer() -> bool {
    let fences: [u32; FIXED_BOS as usize] = {
        let st = STATE.lock();
        core::array::from_fn(|i| st.bos.get(&(i as u32)).map_or(0, |b| b.fence))
    };
    let copy = COPY_INFLIGHT.swap(0, Ordering::AcqRel);
    let mut ok = true;
    if copy != 0 {
        ok = wait_seq(copy, COPY_DEADLINE.load(Ordering::Acquire)) == 0;
        if ok { &COPIES_DONE } else { &COPIES_FAILED }.fetch_add(1, Ordering::Relaxed);
    }
    let dl = deadline();
    for f in fences {
        if f != 0 && f != copy {
            wait_seq(f, dl);
        }
    }
    ok
}

/// Hand a scene to the module: `payload` per §3.3 v1 into the screen-sized
/// buffer `target` (a shadow, or a registered copy of one), reading
/// `sources`. Returns the job's sequence number.
pub fn submit_scene(target: u32, payload: Vec<u8>, sources: &[u32]) -> Option<u32> {
    if !SCENES.load(Ordering::Acquire) || target == 0 {
        return None;
    }
    let seq = {
        let mut st = STATE.lock();
        st.owner?;
        let mut touches = Vec::with_capacity(sources.len() + 1);
        touches.push(target);
        touches.extend_from_slice(sources);
        let len = payload.len() as u32;
        queue_job(&mut st, OP_SCENE, [target, len, 0, 0, 0, 0], Some(payload), &touches)
    };
    wake_module();
    Some(seq)
}

/// Wait for scene job `seq`: 0 when the module composed it, else why not.
pub fn wait_scene(seq: u32) -> i32 {
    wait_seq(seq, deadline())
}

/// `wait_scene` for a scene queued behind new buffers: binding them is a
/// page walk per 4 KB page in the module, once.
pub fn wait_scene_bound(seq: u32) -> i32 {
    wait_seq(seq, crate::interrupts::rdtsc() + BIND_TIMEOUT_MS * (crate::interrupts::tsc_freq() / 1000))
}

/// The fixed buffer (1 or 2) whose first pixel is at `addr`.
pub fn shadow_bo(addr: u64) -> Option<u32> {
    let st = STATE.lock();
    (1..FIXED_BOS).find(|i| st.bos.get(i).is_some_and(|b| b.addr == addr && b.state == BoState::Bound))
}

pub fn scenes_offered() -> bool {
    SCENES.load(Ordering::Acquire)
}

/// What `gpu` reports about the GPU service.
pub struct Status {
    pub owner: Option<PciAddr>,
    pub ready: bool,
    pub scenes: bool,
    pub copies_done: u64,
    pub copies_failed: u64,
    pub buffers: usize,
    pub buffer_bytes: u64,
    pub pending_free: usize,
}

pub fn status() -> Status {
    let st = STATE.lock();
    Status {
        owner: st.owner,
        ready: READY.load(Ordering::Acquire),
        scenes: SCENES.load(Ordering::Acquire),
        copies_done: COPIES_DONE.load(Ordering::Relaxed),
        copies_failed: COPIES_FAILED.load(Ordering::Relaxed),
        buffers: st.bos.len().saturating_sub(FIXED_BOS as usize),
        buffer_bytes: st.bos.iter().filter(|(id, _)| **id >= FIXED_BOS).map(|(_, b)| b.size).sum(),
        pending_free: st.pending_free.len(),
    }
}

/// The active GPU driver while a module serves: display state is the
/// firmware's (no modeset, no flip), copies go to the module.
pub struct GpudevDriver {
    fb: FramebufferInfo,
}

impl GpuHal for GpudevDriver {
    fn name(&self) -> &'static str {
        DRIVER_NAME
    }

    fn set_mode(&mut self, _width: u32, _height: u32, _hz: u8) -> Result<FramebufferInfo, GpuError> {
        Err(GpuError::UnsupportedMode)
    }

    fn framebuffer(&self) -> FramebufferInfo {
        self.fb
    }

    fn supported_modes(&self) -> Vec<ModeInfo> {
        Vec::new()
    }

    fn current_hz(&self) -> u8 {
        0
    }

    fn is_native(&self) -> bool {
        true
    }

    fn flip(&mut self, _surface_addr: u64) {}

    fn wait_vblank(&self) {}

    fn supports_flip(&self) -> bool {
        false
    }

    fn supports_blit(&self) -> bool {
        READY.load(Ordering::Acquire)
    }

    fn blit_rect_hw(&mut self, src_ggtt: u32, src_pitch: u32, dst_ggtt: u32, dst_pitch: u32,
                    x: u32, y: u32, w: u32, h: u32) -> bool {
        submit_copy(src_ggtt, src_pitch, dst_ggtt, dst_pitch, x, y, w, h)
    }

    fn fb_gpu_addr(&self) -> u32 {
        STATE.lock().bos.get(&0).and_then(|b| b.gpu_addr).unwrap_or(0) as u32
    }

    fn shadow_gpu_addr(&self) -> (u32, u32) {
        let st = STATE.lock();
        let a = |i: u32| st.bos.get(&i).and_then(|b| b.gpu_addr).unwrap_or(0) as u32;
        (a(1), a(2))
    }
}
