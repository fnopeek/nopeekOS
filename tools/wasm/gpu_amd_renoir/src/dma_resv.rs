//! drivers/dma-buf/dma-resv.c (Linux 6.18.55): the fences a buffer's users
//! leave on it, each with its usage. One thread, so the lock is held
//! wherever it is asked for and slots need no reserving.

use crate::amdgpu_device::amdgpu_device;
use crate::dma_fence::{self, Fence};
use alloc::vec::Vec;

/// `enum dma_resv_usage`, in its order.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum dma_resv_usage {
    DMA_RESV_USAGE_KERNEL,
    DMA_RESV_USAGE_WRITE,
    DMA_RESV_USAGE_READ,
    DMA_RESV_USAGE_BOOKKEEP,
}

/// `struct dma_resv`: its fence list.
#[derive(Default, Clone)]
pub struct dma_resv {
    fences: Vec<(Fence, dma_resv_usage)>,
}

/// `dma_resv_usage_rw`: a new write waits for readers, a read for writers.
pub fn dma_resv_usage_rw(write: bool) -> dma_resv_usage {
    if write { dma_resv_usage::DMA_RESV_USAGE_READ } else { dma_resv_usage::DMA_RESV_USAGE_WRITE }
}

/// `dma_fence_is_later_or_same`.
fn dma_fence_is_later_or_same(f1: &Fence, f2: &Fence) -> bool {
    alloc::rc::Rc::ptr_eq(f1, f2) || dma_fence::dma_fence_is_later(f1, f2)
}

/// `dma_resv_reserve_fences`: the list grows as fences are added.
pub fn dma_resv_reserve_fences(_obj: &mut dma_resv, _num_fences: u32) -> i32 {
    0
}

/// `dma_resv_add_fence`.
pub fn dma_resv_add_fence(adev: &mut amdgpu_device, obj: &mut dma_resv, fence: &Fence, usage: dma_resv_usage) {
    for i in 0..obj.fences.len() {
        let (old, old_usage) = obj.fences[i].clone();
        if (old.context == fence.context && old_usage >= usage && dma_fence_is_later_or_same(fence, &old))
            || dma_fence::dma_fence_is_signaled(adev, &old) {
            obj.fences[i] = (fence.clone(), usage);
            return;
        }
    }
    obj.fences.push((fence.clone(), usage));
}

/// `dma_resv_for_each_fence` (locked): every fence up to `usage`,
/// signaled or not.
pub fn dma_resv_fences(obj: &dma_resv, usage: dma_resv_usage) -> Vec<Fence> {
    obj.fences.iter().filter(|e| e.1 <= usage).map(|e| e.0.clone()).collect()
}

/// `dma_resv_for_each_fence_unlocked`: the unsignaled ones.
fn dma_resv_fences_unlocked(adev: &mut amdgpu_device, obj: &dma_resv, usage: dma_resv_usage) -> Vec<Fence> {
    let mut out = Vec::new();
    for f in dma_resv_fences(obj, usage) {
        if !dma_fence::dma_fence_is_signaled(adev, &f) {
            out.push(f);
        }
    }
    out
}

/// `dma_resv_get_fences`.
pub fn dma_resv_get_fences(adev: &mut amdgpu_device, obj: &dma_resv, usage: dma_resv_usage) -> Vec<Fence> {
    dma_resv_fences_unlocked(adev, obj, usage)
}

/// `dma_resv_get_singleton`.
pub fn dma_resv_get_singleton(adev: &mut amdgpu_device, obj: &dma_resv, usage: dma_resv_usage) -> Option<Fence> {
    let mut fences = dma_resv_get_fences(adev, obj, usage);
    match fences.len() {
        0 => None,
        1 => fences.pop(),
        _ => Some(dma_fence::dma_fence_array_create(fences, dma_fence::dma_fence_context_alloc(1), 1, false)),
    }
}

/// `dma_resv_wait_timeout`: the time left, or 0.
pub fn dma_resv_wait_timeout(adev: &mut amdgpu_device, obj: &dma_resv, usage: dma_resv_usage, timeout: i64) -> i64 {
    let mut timeout = timeout;
    let mut ret = if timeout != 0 { timeout } else { 1 };
    for f in dma_resv_fences_unlocked(adev, obj, usage) {
        ret = dma_fence::dma_fence_wait_timeout(adev, &f, timeout);
        if ret <= 0 {
            break;
        }
        if timeout != 0 {
            timeout = ret;
        }
    }
    ret
}

/// `dma_resv_test_signaled`.
pub fn dma_resv_test_signaled(adev: &mut amdgpu_device, obj: &dma_resv, usage: dma_resv_usage) -> bool {
    dma_resv_fences_unlocked(adev, obj, usage).is_empty()
}
