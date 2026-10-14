//! gpudev: the kernel side of a GPU driver module (docs/plan/GPU_STACK.md
//! §3.3, v0).
//!
//! Without a module the compositor copies on the CPU (GOP). A module that
//! binds the display controller registers here, binds the kernel's buffers
//! into its own GPU address space and reports ready after its self-test;
//! only then does the active GPU driver become `GpudevDriver`, which hands
//! the compositor's copies to the module. A rejected or late job, and the
//! module's death, put the copy back on the CPU.
//!
//! Buffers belong to the kernel. Buffer 0 is the framebuffer the module took
//! over from the firmware, 1 and 2 are the compositor's shadow buffers.

use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

use super::{FramebufferInfo, GpuError, GpuHal, ModeInfo};
use crate::drivers::pci::PciAddr;
use crate::smp::fiber::{self, Waker, NO_WAKER};

pub const VERSION: i32 = 0;
pub const OP_COPY_RECT: u32 = 1;
/// Bytes per job as `npk_gpudev_poll` writes it: `seq, op, a0..a5`.
pub const JOB_BYTES: usize = 32;
const NUM_BOS: usize = 3;
const JOB_TIMEOUT_MS: u64 = 100;
const DRIVER_NAME: &str = "gpudev (driver module)";

#[derive(Clone, Copy)]
pub struct Bo {
    pub phys: u64,
    pub size: u64,
    pub pitch: u32,
    pub width: u32,
    pub height: u32,
    pub format: u32,
    /// Where the module bound it in its GPU address space.
    pub gpu_addr: Option<u64>,
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
    bos: [Option<Bo>; NUM_BOS],
    queue: VecDeque<Job>,
}

static STATE: Mutex<State> = Mutex::new(State {
    owner: None,
    fb: None,
    bos: [None; NUM_BOS],
    queue: VecDeque::new(),
});

/// The module serves jobs (registered, self-test passed, not dead).
static READY: AtomicBool = AtomicBool::new(false);
static NEXT_SEQ: AtomicU32 = AtomicU32::new(1);
/// Last completed job: `seq << 32 | status as u32`.
static DONE: AtomicU64 = AtomicU64::new(0);
static WAKER: AtomicU32 = AtomicU32::new(NO_WAKER);

/// XRGB8888, the only format the compositor writes.
const FORMAT_XRGB8888: u32 = 0x3432_5258;

/// `npk_gpudev_register`: the module that owns the display controller at
/// `dev` becomes the GPU service. The buffers are described from the
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
    let shadow = |phys| Bo {
        phys,
        size: pages as u64 * 4096,
        pitch: fb.pitch,
        width: fb.width,
        height: fb.height,
        format: FORMAT_XRGB8888,
        gpu_addr: None,
    };
    st.owner = Some(dev);
    st.fb = Some(fb);
    st.bos = [
        Some(Bo { phys: 0, size: fb.pitch as u64 * fb.height as u64, ..shadow(0) }),
        Some(shadow(pa)),
        Some(shadow(pb)),
    ];
    st.queue.clear();
    crate::kprintln!("[npk] gpudev: {:02x}:{:02x}.{} registered, {}x{} framebuffer, 2 shadow buffers",
        dev.bus, dev.device, dev.function, fb.width, fb.height);
    0
}

fn owned_by(dev: PciAddr) -> bool {
    STATE.lock().owner == Some(dev)
}

/// `npk_gpudev_bo_info`.
pub fn bo_info(dev: PciAddr, id: i32) -> Option<Bo> {
    let st = STATE.lock();
    if st.owner != Some(dev) {
        return None;
    }
    st.bos.get(id as usize).copied().flatten()
}

/// `npk_gpudev_bo_bound`.
pub fn bo_bound(dev: PciAddr, id: i32, gpu_addr: i64) -> i32 {
    let mut st = STATE.lock();
    if st.owner != Some(dev) || gpu_addr < 0 || gpu_addr > u32::MAX as i64 {
        return -1;
    }
    match st.bos.get_mut(id as usize) {
        Some(Some(bo)) => {
            bo.gpu_addr = Some(gpu_addr as u64);
            0
        }
        _ => -1,
    }
}

/// `npk_gpudev_ready`: with a passed self-test and all buffers bound, the
/// compositor's copies go to the module from the next frame on.
pub fn ready(dev: PciAddr, ok: bool) -> i32 {
    if !ok {
        if !owned_by(dev) {
            return -1;
        }
        crate::kprintln!("[npk] gpudev: module self-test failed, compositor stays on the CPU");
        return 0;
    }
    let (fb, a, b) = {
        let st = STATE.lock();
        if st.owner != Some(dev) {
            return -1;
        }
        let addr = |i: usize| st.bos[i].and_then(|bo| bo.gpu_addr);
        let (Some(fb), Some(_), Some(a), Some(b)) = (st.fb, addr(0), addr(1), addr(2)) else {
            crate::kprintln!("[npk] gpudev: ready without all buffers bound, staying on the CPU");
            return -1;
        };
        (fb, a as u32, b as u32)
    };
    // Lock order of the compositor is framebuffer, then GPU; never both here.
    crate::drivers::framebuffer::set_shadow_ggtt(a, b);
    super::install(Box::new(GpudevDriver { fb }));
    READY.store(true, Ordering::Release);
    crate::kprintln!("[npk] gpudev: module ready, compositor copies on the GPU");
    0
}

/// `npk_gpudev_poll`: up to `max` jobs into `out`, `JOB_BYTES` each.
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

/// `npk_gpudev_complete`.
pub fn complete(dev: PciAddr, seq: u32, status: i32) -> i32 {
    if !owned_by(dev) {
        return -1;
    }
    DONE.store((seq as u64) << 32 | status as u32 as u64, Ordering::Release);
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

/// The module that owned `dev` is gone: jobs in flight fail, and the
/// compositor goes back to copying on the CPU into the framebuffer the
/// module last showed. No register is touched.
pub fn module_gone(dev: PciAddr) {
    let fb = {
        let mut st = STATE.lock();
        if st.owner != Some(dev) {
            return;
        }
        st.owner = None;
        st.queue.clear();
        st.bos = [None; NUM_BOS];
        st.fb.take()
    };
    READY.store(false, Ordering::Release);
    WAKER.store(NO_WAKER, Ordering::Release);
    if super::driver_name() == DRIVER_NAME {
        crate::drivers::framebuffer::set_shadow_ggtt(0, 0);
        if let Some(fb) = fb {
            super::install(Box::new(super::gop::GopDriver::from_framebuffer(fb)));
        }
        crate::kprintln!("[npk] gpudev: module gone, compositor copies on the CPU");
    }
}

/// Hand one copy to the module and wait for it. False on any failure, so
/// the caller copies on the CPU.
fn submit_copy(src_gpu: u32, src_pitch: u32, dst_gpu: u32, dst_pitch: u32,
               x: u32, y: u32, w: u32, h: u32) -> bool {
    if !READY.load(Ordering::Acquire) {
        return false;
    }
    let seq = {
        let mut st = STATE.lock();
        let find = |addr: u32, pitch: u32| {
            st.bos.iter().position(|b| {
                b.is_some_and(|b| b.gpu_addr == Some(addr as u64) && b.pitch == pitch)
            })
        };
        let (Some(src), Some(dst)) = (find(src_gpu, src_pitch), find(dst_gpu, dst_pitch)) else {
            return false;
        };
        let seq = NEXT_SEQ.fetch_add(1, Ordering::Relaxed);
        st.queue.push_back(Job { seq, op: OP_COPY_RECT, a: [src as u32, dst as u32, x, y, w, h] });
        seq
    };
    let w = WAKER.load(Ordering::Acquire);
    if w != NO_WAKER {
        fiber::signal(w, fiber::SIG_GPU);
    }
    let freq = crate::interrupts::tsc_freq();
    let deadline = crate::interrupts::rdtsc() + JOB_TIMEOUT_MS * (freq / 1000);
    loop {
        let done = DONE.load(Ordering::Acquire);
        if (done >> 32) as u32 == seq {
            return done as u32 as i32 == 0;
        }
        if !READY.load(Ordering::Acquire) || crate::interrupts::rdtsc() >= deadline {
            // A job the module never took must not run after the CPU copy.
            STATE.lock().queue.retain(|j| j.seq != seq);
            return false;
        }
        core::hint::spin_loop();
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

    fn blit_verified(&self) -> bool {
        READY.load(Ordering::Acquire)
    }

    fn blit_rect_hw(&mut self, src_ggtt: u32, src_pitch: u32, dst_ggtt: u32, dst_pitch: u32,
                    x: u32, y: u32, w: u32, h: u32) -> bool {
        submit_copy(src_ggtt, src_pitch, dst_ggtt, dst_pitch, x, y, w, h)
    }

    fn fb_gpu_addr(&self) -> u32 {
        STATE.lock().bos[0].and_then(|b| b.gpu_addr).unwrap_or(0) as u32
    }

    fn shadow_gpu_addr(&self) -> (u32, u32) {
        let st = STATE.lock();
        let a = |i: usize| st.bos[i].and_then(|b| b.gpu_addr).unwrap_or(0) as u32;
        (a(1), a(2))
    }
}
