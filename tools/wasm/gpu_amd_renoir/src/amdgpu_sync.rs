//! amdgpu/amdgpu_sync.c (Linux 6.18.55): a set of fences to wait for,
//! the latest one per context.
//!
//! The hashtable keeps Linux's shape, 16 buckets by `hash_64(context)`
//! with new entries at the head, so it is walked in the same order.

use crate::amdgpu_device::amdgpu_device;
use crate::dma_fence::{self, Fence};
use alloc::vec::Vec;

pub const AMDGPU_FENCE_OWNER_UNDEFINED: u64 = 0;
pub const AMDGPU_FENCE_OWNER_VM: u64 = 1;
pub const AMDGPU_FENCE_OWNER_KFD: u64 = 2;

/// `enum amdgpu_sync_mode`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum amdgpu_sync_mode {
    AMDGPU_SYNC_ALWAYS,
    AMDGPU_SYNC_NE_OWNER,
    AMDGPU_SYNC_EQ_OWNER,
    AMDGPU_SYNC_EXPLICIT,
}

const HASH_BITS: u32 = 4;

/// `struct amdgpu_sync`: `DECLARE_HASHTABLE(fences, 4)`.
#[derive(Default)]
pub struct amdgpu_sync {
    fences: [Vec<Fence>; 1 << HASH_BITS],
}

/// `hash_min(context, 4)`.
fn bucket(context: u64) -> usize {
    (context.wrapping_mul(0x61C8_8646_80B5_83EB) >> (64 - HASH_BITS)) as usize
}

/// `amdgpu_sync_create`.
pub fn amdgpu_sync_create() -> amdgpu_sync {
    amdgpu_sync::default()
}

/// `amdgpu_sync_same_dev`: there is one device.
fn amdgpu_sync_same_dev(f: &Fence) -> bool {
    crate::drm_sched::to_drm_sched_fence(f).is_some()
}

/// `amdgpu_sync_get_owner`: no KFD fences exist here.
fn amdgpu_sync_get_owner(f: Option<&Fence>) -> u64 {
    let Some(f) = f else { return AMDGPU_FENCE_OWNER_UNDEFINED };
    match crate::drm_sched::to_drm_sched_fence(f) {
        Some(s) => s.owner,
        None => AMDGPU_FENCE_OWNER_UNDEFINED,
    }
}

/// `amdgpu_sync_keep_later`.
fn amdgpu_sync_keep_later(keep: &mut Fence, fence: &Fence) {
    if dma_fence::dma_fence_is_later(keep, fence) {
        return;
    }
    *keep = fence.clone();
}

/// `amdgpu_sync_add_later`.
fn amdgpu_sync_add_later(adev: &mut amdgpu_device, sync: &mut amdgpu_sync, f: &Fence) -> bool {
    let b = bucket(f.context);
    for i in 0..sync.fences[b].len() {
        let e = sync.fences[b][i].clone();
        if dma_fence::dma_fence_is_signaled(adev, &e) {
            sync.fences[b][i] = f.clone();
            return true;
        }
        if e.context == f.context {
            amdgpu_sync_keep_later(&mut sync.fences[b][i], f);
            return true;
        }
    }
    false
}

/// `amdgpu_sync_fence`.
pub fn amdgpu_sync_fence(adev: &mut amdgpu_device, sync: &mut amdgpu_sync, f: Option<&Fence>) -> i32 {
    let Some(f) = f else { return 0 };
    if amdgpu_sync_add_later(adev, sync, f) {
        return 0;
    }
    sync.fences[bucket(f.context)].insert(0, f.clone());
    0
}

/// `amdgpu_sync_test_fence`: whether `f` is to be waited for.
fn amdgpu_sync_test_fence(mode: amdgpu_sync_mode, owner: u64, f: &Fence) -> bool {
    let fence_owner = amdgpu_sync_get_owner(Some(f));
    // Always sync to moves, no matter what
    if fence_owner == AMDGPU_FENCE_OWNER_UNDEFINED {
        return true;
    }
    // We only want to trigger KFD eviction fences on evict or move jobs.
    // Skip KFD fences otherwise.
    if fence_owner == AMDGPU_FENCE_OWNER_KFD && owner != AMDGPU_FENCE_OWNER_UNDEFINED {
        return false;
    }
    // Never sync to VM updates either.
    if fence_owner == AMDGPU_FENCE_OWNER_VM && owner != AMDGPU_FENCE_OWNER_UNDEFINED
        && owner != AMDGPU_FENCE_OWNER_KFD {
        return false;
    }
    // Ignore fences depending on the sync mode
    match mode {
        amdgpu_sync_mode::AMDGPU_SYNC_ALWAYS => true,
        amdgpu_sync_mode::AMDGPU_SYNC_NE_OWNER => !(amdgpu_sync_same_dev(f) && fence_owner == owner),
        amdgpu_sync_mode::AMDGPU_SYNC_EQ_OWNER => !(amdgpu_sync_same_dev(f) && fence_owner != owner),
        amdgpu_sync_mode::AMDGPU_SYNC_EXPLICIT => false,
    }
}

/// `amdgpu_sync_resv`: every fence of `resv` up to READ usage that the
/// mode and owner select. No fence here is a chain.
pub fn amdgpu_sync_resv(adev: &mut amdgpu_device, sync: &mut amdgpu_sync, resv: &crate::dma_resv::dma_resv,
                        mode: amdgpu_sync_mode, owner: u64) -> i32 {
    let fences = crate::dma_resv::dma_resv_fences(resv, crate::dma_resv::dma_resv_usage::DMA_RESV_USAGE_READ);
    for f in fences {
        if amdgpu_sync_test_fence(mode, owner, &f) {
            let r = amdgpu_sync_fence(adev, sync, Some(&f));
            if r != 0 {
                return r;
            }
        }
    }
    0
}

/// `amdgpu_sync_peek_fence`: the next fence to wait for, only until it is
/// scheduled when it runs on `ring`'s own scheduler.
pub fn amdgpu_sync_peek_fence(adev: &mut amdgpu_device, sync: &mut amdgpu_sync, ring: Option<usize>)
                              -> Option<Fence> {
    for b in 0..sync.fences.len() {
        let mut i = 0;
        while i < sync.fences[b].len() {
            let f = sync.fences[b][i].clone();
            if dma_fence::dma_fence_is_signaled(adev, &f) {
                sync.fences[b].remove(i);
                continue;
            }
            if let (Some(r), Some(s)) = (ring, crate::drm_sched::to_drm_sched_fence(&f)) {
                // For fences from the same ring it is sufficient when they
                // are scheduled.
                if s.sched == r {
                    if dma_fence::dma_fence_is_signaled(adev, &s.scheduled) {
                        i += 1;
                        continue;
                    }
                    return Some(s.scheduled);
                }
            }
            return Some(f);
        }
    }
    None
}

/// `amdgpu_sync_get_fence`: takes the next unsignaled fence out.
pub fn amdgpu_sync_get_fence(adev: &mut amdgpu_device, sync: &mut amdgpu_sync) -> Option<Fence> {
    for b in 0..sync.fences.len() {
        while !sync.fences[b].is_empty() {
            let f = sync.fences[b].remove(0);
            if !dma_fence::dma_fence_is_signaled(adev, &f) {
                return Some(f);
            }
        }
    }
    None
}

/// `amdgpu_sync_clone`.
pub fn amdgpu_sync_clone(adev: &mut amdgpu_device, source: &mut amdgpu_sync, clone: &mut amdgpu_sync) -> i32 {
    for b in 0..source.fences.len() {
        let mut i = 0;
        while i < source.fences[b].len() {
            let f = source.fences[b][i].clone();
            if !dma_fence::dma_fence_is_signaled(adev, &f) {
                let r = amdgpu_sync_fence(adev, clone, Some(&f));
                if r != 0 {
                    return r;
                }
                i += 1;
            } else {
                source.fences[b].remove(i);
            }
        }
    }
    0
}

/// `amdgpu_sync_move`.
pub fn amdgpu_sync_move(src: &mut amdgpu_sync, dst: &mut amdgpu_sync) {
    *dst = core::mem::take(src);
}

/// `amdgpu_sync_push_to_job`.
pub fn amdgpu_sync_push_to_job(adev: &mut amdgpu_device, sync: &mut amdgpu_sync, job: u64) -> i32 {
    for b in 0..sync.fences.len() {
        let mut i = 0;
        while i < sync.fences[b].len() {
            let f = sync.fences[b][i].clone();
            if dma_fence::dma_fence_is_signaled(adev, &f) {
                sync.fences[b].remove(i);
                continue;
            }
            let r = crate::drm_sched::drm_sched_job_add_dependency(adev, job, Some(f));
            if r != 0 {
                return r;
            }
            i += 1;
        }
    }
    0
}

/// `amdgpu_sync_wait`.
pub fn amdgpu_sync_wait(adev: &mut amdgpu_device, sync: &mut amdgpu_sync) -> i32 {
    for b in 0..sync.fences.len() {
        while let Some(f) = sync.fences[b].first().cloned() {
            let r = dma_fence::dma_fence_wait(adev, &f);
            if r != 0 {
                return r as i32;
            }
            sync.fences[b].remove(0);
        }
    }
    0
}

/// `amdgpu_sync_free`.
pub fn amdgpu_sync_free(sync: &mut amdgpu_sync) {
    for b in sync.fences.iter_mut() {
        b.clear();
    }
}
