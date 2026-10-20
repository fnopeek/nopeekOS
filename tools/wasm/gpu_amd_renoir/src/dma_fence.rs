//! drivers/dma-buf/dma-fence.c (Linux 6.18.55): fences with a context,
//! a sequence number, a callback list and a signaled bit.
//!
//! A fence is an `Rc`: cloning it is `dma_fence_get`, dropping it
//! `dma_fence_put`. The module has one thread, so the fence lock guards
//! nothing. A callback is a variant of `dma_fence_func`, run with the
//! device when the fence signals; a waiter does what the sleeping task's
//! wake-up does in Linux by running the interrupt handler and the pending
//! work itself (`wait_until`).

use crate::amdgpu_device::amdgpu_device;
use alloc::rc::{Rc, Weak};
use alloc::vec::Vec;
use core::cell::{Cell, RefCell};
use core::sync::atomic::{AtomicU64, Ordering};

pub type Fence = Rc<dma_fence>;

pub const DMA_FENCE_FLAG_SIGNALED_BIT: u32 = 1;
pub const DMA_FENCE_FLAG_TIMESTAMP_BIT: u32 = 2;
pub const DMA_FENCE_FLAG_ENABLE_SIGNAL_BIT: u32 = 3;
pub const DMA_FENCE_FLAG_USER_BITS: u32 = 4;

/// What a fence is, in place of `struct dma_fence_ops`.
pub enum dma_fence_ops {
    /// `dma_fence_stub_ops`.
    stub,
    /// `amdgpu_fence_ops`, or `amdgpu_job_fence_ops` when `job`: a ring's
    /// hardware fence, with the ring's write pointer after it
    /// (`amdgpu_fence_save_wptr`) and the `amdgpu_fence`'s context.
    amdgpu_fence { ring: usize, wptr: Cell<u64>, context: u64, job: bool },
    /// `drm_sched_fence_ops_scheduled`: the `drm_sched_fence` it starts is
    /// its finished fence's, reached while that is alive; the scheduler and
    /// owner are kept here too.
    sched_scheduled { finished: Weak<dma_fence>, sched: usize, owner: u64 },
    /// `drm_sched_fence_ops_finished`, with the `drm_sched_fence` it ends.
    sched_finished(crate::drm_sched::drm_sched_fence),
    /// `dma_fence_array_ops`.
    array(dma_fence_array),
}

/// `struct dma_fence_array` without its base.
pub struct dma_fence_array {
    pub fences: Vec<Fence>,
    num_pending: Cell<i32>,
}

/// A callback, in place of `dma_fence_func_t` and the structure embedding
/// the `dma_fence_cb`.
#[derive(Clone)]
pub enum dma_fence_func {
    /// `drm_sched_job_done_cb`: the hardware fence of job `job` on the
    /// scheduler of ring `ring` signaled.
    sched_job_done { ring: usize, job: u64 },
    /// `drm_sched_entity_wakeup`: a dependency of entity `entity` signaled.
    sched_entity_wakeup { entity: usize },
    /// `drm_sched_entity_kill_jobs_cb`.
    sched_entity_kill_jobs { job: u64 },
    /// `dma_fence_array_cb_func`, holding its reference to the array.
    array_cb { array: Fence },
}

pub struct dma_fence {
    pub context: u64,
    pub seqno: u64,
    pub ops: dma_fence_ops,
    flags: Cell<u32>,
    error: Cell<i32>,
    timestamp: Cell<u64>,
    cb_list: RefCell<Vec<(u64, dma_fence_func)>>,
}

impl dma_fence {
    pub fn test_flag(&self, bit: u32) -> bool {
        self.flags.get() & (1 << bit) != 0
    }

    pub fn set_flag(&self, bit: u32) {
        self.flags.set(self.flags.get() | 1 << bit);
    }

    fn test_and_set(&self, bit: u32) -> bool {
        let was = self.test_flag(bit);
        self.flags.set(self.flags.get() | 1 << bit);
        was
    }

    pub fn error(&self) -> i32 {
        self.error.get()
    }

    pub fn timestamp(&self) -> u64 {
        self.timestamp.get()
    }

    pub fn set_timestamp(&self, t: u64) {
        self.timestamp.set(t);
    }
}

static CONTEXT: AtomicU64 = AtomicU64::new(1);
static CB_ID: AtomicU64 = AtomicU64::new(1);

/// `dma_fence_context_alloc`: `num` contexts, the first returned. Context
/// 0 is the stub's.
pub fn dma_fence_context_alloc(num: u32) -> u64 {
    CONTEXT.fetch_add(num as u64, Ordering::Relaxed)
}

/// `dma_fence_init` on a fence not yet shared.
pub fn dma_fence_new(ops: dma_fence_ops, context: u64, seqno: u64) -> dma_fence {
    dma_fence {
        context,
        seqno,
        ops,
        flags: Cell::new(0),
        error: Cell::new(0),
        timestamp: Cell::new(0),
        cb_list: RefCell::new(Vec::new()),
    }
}

/// `dma_fence_init`.
pub fn dma_fence_init(ops: dma_fence_ops, context: u64, seqno: u64) -> Fence {
    Rc::new(dma_fence_new(ops, context, seqno))
}

/// `dma_fence_get_stub`: a fence that is signaled already.
pub fn dma_fence_get_stub() -> Fence {
    let f = dma_fence_init(dma_fence_ops::stub, 0, 0);
    f.flags.set(1 << DMA_FENCE_FLAG_SIGNALED_BIT | 1 << DMA_FENCE_FLAG_TIMESTAMP_BIT);
    f
}

/// `dma_fence_set_error`.
pub fn dma_fence_set_error(f: &Fence, error: i32) {
    if !f.test_flag(DMA_FENCE_FLAG_SIGNALED_BIT) && error < 0 {
        f.error.set(error);
    }
}

/// `dma_fence_signal_timestamp_locked`: -22 when it was signaled before.
pub fn dma_fence_signal(adev: &mut amdgpu_device, f: &Fence) -> i32 {
    if f.test_and_set(DMA_FENCE_FLAG_SIGNALED_BIT) {
        return -22;
    }
    // Stash the cb_list before replacing it with the timestamp
    let cbs = core::mem::take(&mut *f.cb_list.borrow_mut());
    f.timestamp.set(crate::linux::now_us());
    f.flags.set(f.flags.get() | 1 << DMA_FENCE_FLAG_TIMESTAMP_BIT);
    for (_, func) in cbs {
        run_callback(adev, f, func);
    }
    0
}

fn run_callback(adev: &mut amdgpu_device, f: &Fence, func: dma_fence_func) {
    match func {
        dma_fence_func::sched_job_done { ring, job } => crate::drm_sched::drm_sched_job_done_cb(adev, f, ring, job),
        dma_fence_func::sched_entity_wakeup { entity } => crate::drm_sched::drm_sched_entity_wakeup(adev, entity),
        dma_fence_func::sched_entity_kill_jobs { job } => crate::drm_sched::drm_sched_entity_kill_jobs_cb(adev, job),
        dma_fence_func::array_cb { array } => dma_fence_array_cb_func(adev, f, array),
    }
}

/// `dma_fence_is_signaled`: of the fences here only an array has
/// `ops->signaled`.
pub fn dma_fence_is_signaled(adev: &mut amdgpu_device, f: &Fence) -> bool {
    if f.test_flag(DMA_FENCE_FLAG_SIGNALED_BIT) {
        return true;
    }
    if let dma_fence_ops::array(a) = &f.ops {
        if dma_fence_array_signaled(adev, f, a) {
            dma_fence_signal(adev, f);
            return true;
        }
    }
    false
}

/// `__dma_fence_enable_signaling`: false when the fence is signaled.
fn __dma_fence_enable_signaling(adev: &mut amdgpu_device, f: &Fence) -> bool {
    let was_set = f.test_and_set(DMA_FENCE_FLAG_ENABLE_SIGNAL_BIT);
    if f.test_flag(DMA_FENCE_FLAG_SIGNALED_BIT) {
        return false;
    }
    if !was_set {
        let keep = match &f.ops {
            dma_fence_ops::amdgpu_fence { ring, .. } => {
                crate::amdgpu_fence::amdgpu_fence_enable_signaling(adev, *ring)
            }
            dma_fence_ops::array(_) => dma_fence_array_enable_signaling(adev, f),
            dma_fence_ops::sched_finished(_) | dma_fence_ops::sched_scheduled { .. } | dma_fence_ops::stub => true,
        };
        if !keep {
            dma_fence_signal(adev, f);
            return false;
        }
    }
    true
}

/// `dma_fence_enable_sw_signaling`.
pub fn dma_fence_enable_sw_signaling(adev: &mut amdgpu_device, f: &Fence) {
    __dma_fence_enable_signaling(adev, f);
}

/// `dma_fence_add_callback`: the callback's id, or -2 (ENOENT) when the
/// fence is signaled and the callback was not added.
pub fn dma_fence_add_callback(adev: &mut amdgpu_device, f: &Fence, func: dma_fence_func) -> Result<u64, i32> {
    if f.test_flag(DMA_FENCE_FLAG_SIGNALED_BIT) {
        return Err(-2);
    }
    if !__dma_fence_enable_signaling(adev, f) {
        return Err(-2);
    }
    let id = CB_ID.fetch_add(1, Ordering::Relaxed);
    f.cb_list.borrow_mut().push((id, func));
    Ok(id)
}

/// `dma_fence_remove_callback`: true when it was still pending.
pub fn dma_fence_remove_callback(f: &Fence, id: u64) -> bool {
    let mut l = f.cb_list.borrow_mut();
    match l.iter().position(|e| e.0 == id) {
        Some(i) => {
            l.remove(i);
            true
        }
        None => false,
    }
}

/// `dma_fence_get_status`: 0 pending, 1 signaled, an error.
pub fn dma_fence_get_status(adev: &mut amdgpu_device, f: &Fence) -> i32 {
    if dma_fence_is_signaled(adev, f) {
        let e = f.error.get();
        if e < 0 { e } else { 1 }
    } else {
        0
    }
}

/// `dma_fence_wait_timeout` with `dma_fence_default_wait` (`timeout` in
/// microseconds, `i64::MAX` for none): the time left, at least 1, or 0.
/// Where Linux sleeps until the callback wakes it, this runs what would
/// run meanwhile: the interrupt handler and the pending work.
pub fn dma_fence_wait_timeout(adev: &mut amdgpu_device, f: &Fence, timeout: i64) -> i64 {
    dma_fence_enable_sw_signaling(adev, f);
    if dma_fence_is_signaled(adev, f) {
        return timeout.max(1);
    }
    if timeout == 0 {
        return 0;
    }
    let f2 = f.clone();
    crate::workqueue::wait_until(adev, timeout, move |adev| dma_fence_is_signaled(adev, &f2))
}

/// `dma_fence_wait`: uninterruptible, no timeout; 0 or an error.
pub fn dma_fence_wait(adev: &mut amdgpu_device, f: &Fence) -> i64 {
    if dma_fence_wait_timeout(adev, f, i64::MAX) > 0 { 0 } else { -110 }
}

/// `dma_fence_is_later`: `f1` comes after `f2` on their common context.
pub fn dma_fence_is_later(f1: &Fence, f2: &Fence) -> bool {
    if f1.context != f2.context {
        return false;
    }
    __dma_fence_is_later(f1.seqno, f2.seqno, f1)
}

/// `__dma_fence_is_later`: none of these fences has 64-bit sequence
/// numbers, so they wrap at 32 bits.
pub fn __dma_fence_is_later(f1: u64, f2: u64, _f: &Fence) -> bool {
    (f1 as u32).wrapping_sub(f2 as u32) as i32 > 0
}

/// `PENDING_ERROR`: the array's error until it signals.
const PENDING_ERROR: i32 = 1;

fn dma_fence_array_set_pending_error(array: &Fence, error: i32) {
    if error != 0 && array.error.get() == PENDING_ERROR {
        array.error.set(error);
    }
}

fn dma_fence_array_clear_pending_error(array: &Fence) {
    if array.error.get() == PENDING_ERROR {
        array.error.set(0);
    }
}

fn array_of(f: &Fence) -> &dma_fence_array {
    match &f.ops {
        dma_fence_ops::array(a) => a,
        _ => unreachable!(),
    }
}

/// `dma_fence_array_cb_func`: the irq work runs at once.
fn dma_fence_array_cb_func(adev: &mut amdgpu_device, f: &Fence, array: Fence) {
    dma_fence_array_set_pending_error(&array, f.error.get());
    let a = array_of(&array);
    a.num_pending.set(a.num_pending.get() - 1);
    if a.num_pending.get() == 0 {
        // irq_dma_fence_array_work
        dma_fence_array_clear_pending_error(&array);
        dma_fence_signal(adev, &array);
    }
}

/// `dma_fence_array_enable_signaling`.
fn dma_fence_array_enable_signaling(adev: &mut amdgpu_device, f: &Fence) -> bool {
    let fences = array_of(f).fences.clone();
    for g in &fences {
        if dma_fence_add_callback(adev, g, dma_fence_func::array_cb { array: f.clone() }).is_err() {
            dma_fence_array_set_pending_error(f, g.error.get());
            let a = array_of(f);
            a.num_pending.set(a.num_pending.get() - 1);
            if a.num_pending.get() == 0 {
                dma_fence_array_clear_pending_error(f);
                return false;
            }
        }
    }
    true
}

/// `dma_fence_array_signaled`.
fn dma_fence_array_signaled(adev: &mut amdgpu_device, f: &Fence, a: &dma_fence_array) -> bool {
    let mut num_pending = a.num_pending.get();
    let signal = if f.test_flag(DMA_FENCE_FLAG_ENABLE_SIGNAL_BIT) {
        num_pending <= 0
    } else {
        let fences = a.fences.clone();
        fences.iter().any(|g| {
            if dma_fence_is_signaled(adev, g) {
                num_pending -= 1;
                num_pending == 0
            } else {
                false
            }
        })
    };
    if signal {
        dma_fence_array_clear_pending_error(f);
    }
    signal
}

/// `dma_fence_array_create`.
pub fn dma_fence_array_create(fences: Vec<Fence>, context: u64, seqno: u32, signal_on_any: bool) -> Fence {
    let n = fences.len() as i32;
    let f = dma_fence_init(dma_fence_ops::array(dma_fence_array {
        fences,
        num_pending: Cell::new(if signal_on_any { 1 } else { n }),
    }), context, seqno as u64);
    f.error.set(PENDING_ERROR);
    f
}
