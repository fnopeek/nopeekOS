//! i915_getparam.c and i915_query.c (Linux 6.18.55): what the device says
//! about itself, answered from what the module measured at probe time.
//!
//! Not ported, and answered as Linux answers a kernel without it: perf
//! (`I915_PARAM_PERF_REVISION`, `_OA_TIMESTAMP_FREQUENCY` and the perf
//! query), HuC and PXP (no firmware is loaded), GuC submission. The
//! scheduler reports what execlists does here: enabled and priority
//! ordered, without preemption, semaphores or timeslices.

use crate::i915_drv::drm_i915_private;
use crate::i915_gem_context::{intel_engine_lookup_user, uabi_class, uabi_engines};
use crate::i915_uapi::*;
use crate::intel_ggtt::i915_ggtt;
use crate::intel_gt::intel_gt;

/// The device id the module binds (INTEL_TGL_GT2_IDS).
const TGL_GT2: u32 = 0x9a78;
/// `INTEL_PPGTT_FULL`.
const INTEL_PPGTT_FULL: i32 = 2;
const SCHEDULER_CAPS: i32 = I915_SCHEDULER_CAP_ENABLED | I915_SCHEDULER_CAP_PRIORITY;

/// `i915_getparam_ioctl`: the value, or a negative errno.
pub fn i915_getparam(i915: &drm_i915_private, gt: &intel_gt, ggtt: &i915_ggtt, param: u32) -> Result<i32, i32> {
    let sseu = &gt.info.sseu;
    let has = |class, instance| intel_engine_lookup_user(gt, class, instance).is_some() as i32;
    Ok(match param {
        I915_PARAM_IRQ_ACTIVE | I915_PARAM_ALLOW_BATCHBUFFER | I915_PARAM_LAST_DISPATCH
        | I915_PARAM_HAS_EXEC_CONSTANTS => return Err(-ENODEV),
        I915_PARAM_CHIPSET_ID => TGL_GT2 as i32,
        I915_PARAM_REVISION => i915.revision as i32,
        I915_PARAM_NUM_FENCES_AVAIL => ggtt.num_fences as i32,
        I915_PARAM_HAS_OVERLAY => 0,
        I915_PARAM_HAS_BSD => has(I915_ENGINE_CLASS_VIDEO, 0),
        I915_PARAM_HAS_BLT => has(I915_ENGINE_CLASS_COPY, 0),
        I915_PARAM_HAS_VEBOX => has(I915_ENGINE_CLASS_VIDEO_ENHANCE, 0),
        I915_PARAM_HAS_BSD2 => has(I915_ENGINE_CLASS_VIDEO, 1),
        // HAS_LLC: TGL has a last-level cache shared with the CPU; HAS_WT
        // is Haswell's eLLC.
        I915_PARAM_HAS_LLC => 1,
        I915_PARAM_HAS_WT => 0,
        I915_PARAM_HAS_ALIASING_PPGTT => INTEL_PPGTT_FULL,
        I915_PARAM_HAS_SEMAPHORES => 0,
        // HAS_SECURE_BATCHES needs CAP_SYS_ADMIN.
        I915_PARAM_HAS_SECURE_BATCHES => 0,
        I915_PARAM_CMD_PARSER_VERSION => 0,
        I915_PARAM_SUBSLICE_TOTAL => match (0..sseu.max_slices as usize).map(|s| sseu.subslice_mask[s].count_ones()).sum::<u32>() {
            0 => return Err(-ENODEV),
            v => v as i32,
        },
        I915_PARAM_EU_TOTAL => match sseu.eu_total {
            0 => return Err(-ENODEV),
            v => v as i32,
        },
        // A full GT reset exists, an engine reset does not.
        I915_PARAM_HAS_GPU_RESET => 1,
        I915_PARAM_HAS_RESOURCE_STREAMER => 0,
        I915_PARAM_HAS_POOLED_EU => i915.runtime.has_pooled_eu as i32,
        I915_PARAM_MIN_EU_IN_POOL => 0,
        I915_PARAM_HUC_STATUS | I915_PARAM_PXP_STATUS => return Err(-ENODEV),
        // i915_gem_mmap_gtt_version is 5; virtio-gpu cannot map part of an
        // object, so virglrenderer reports 4.
        I915_PARAM_MMAP_GTT_VERSION => 4,
        I915_PARAM_HAS_SCHEDULER => SCHEDULER_CAPS,
        I915_PARAM_MMAP_VERSION | I915_PARAM_HAS_GEM | I915_PARAM_HAS_PAGEFLIPPING | I915_PARAM_HAS_EXECBUF2
        | I915_PARAM_HAS_RELAXED_FENCING | I915_PARAM_HAS_COHERENT_RINGS | I915_PARAM_HAS_RELAXED_DELTA
        | I915_PARAM_HAS_GEN7_SOL_RESET | I915_PARAM_HAS_WAIT_TIMEOUT | I915_PARAM_HAS_PRIME_VMAP_FLUSH
        | I915_PARAM_HAS_PINNED_BATCHES | I915_PARAM_HAS_EXEC_NO_RELOC | I915_PARAM_HAS_EXEC_HANDLE_LUT
        | I915_PARAM_HAS_COHERENT_PHYS_GTT | I915_PARAM_HAS_EXEC_SOFTPIN | I915_PARAM_HAS_EXEC_ASYNC
        | I915_PARAM_HAS_EXEC_FENCE | I915_PARAM_HAS_EXEC_CAPTURE | I915_PARAM_HAS_EXEC_BATCH_FIRST
        | I915_PARAM_HAS_EXEC_FENCE_ARRAY | I915_PARAM_HAS_EXEC_SUBMIT_FENCE
        | I915_PARAM_HAS_EXEC_TIMELINE_FENCES | I915_PARAM_HAS_USERPTR_PROBE => 1,
        // No GuC submission: the value is -EINVAL and the ioctl succeeds.
        I915_PARAM_HAS_CONTEXT_FREQ_HINT => -EINVAL,
        // intel_engines_has_context_isolation: the uabi classes whose
        // engines have a recorded default image.
        I915_PARAM_HAS_CONTEXT_ISOLATION => {
            let mut which = 0i32;
            for e in gt.engine.iter().flatten() {
                let class = uabi_class(e.class);
                if e.default_state.is_some() && class != I915_ENGINE_CLASS_INVALID_NONE {
                    which |= 1 << class;
                }
            }
            which
        }
        I915_PARAM_SLICE_MASK => match sseu.slice_mask {
            0 => return Err(-ENODEV),
            v => v as i32,
        },
        I915_PARAM_SUBSLICE_MASK => match sseu.subslice_mask[0] {
            0 => return Err(-ENODEV),
            v => v as i32,
        },
        I915_PARAM_CS_TIMESTAMP_FREQUENCY => gt.clock_frequency as i32,
        // GEN7_FEATURES onward have a coherent GGTT.
        I915_PARAM_MMAP_GTT_COHERENT => 1,
        _ => return Err(-EINVAL),
    })
}

/// `GEN_SSEU_STRIDE`.
fn stride(max: u32) -> u32 {
    max.div_ceil(8)
}

/// `i915_query_ioctl` for one item: fills `data` (the guest's `length`
/// bytes, zeroed) and returns what Linux puts in `item.length` — the
/// total length, or a negative errno.
pub fn i915_query(gt: &intel_gt, query_id: u32, flags: u32, data: &mut [u8]) -> i32 {
    let length = data.len();
    // copy_query_item
    let check = |total: usize| -> Option<i32> {
        if length == 0 {
            Some(total as i32)
        } else if length < total {
            Some(-EINVAL)
        } else {
            None
        }
    };
    match query_id {
        DRM_I915_QUERY_TOPOLOGY_INFO => {
            if flags != 0 {
                return -EINVAL;
            }
            let s = &gt.info.sseu;
            if s.max_slices == 0 {
                return -ENODEV;
            }
            let ss_stride = stride(s.max_subslices);
            let eu_stride = stride(s.max_eus_per_subslice);
            let slice_length = 1u32;
            let subslice_length = s.max_slices * ss_stride;
            let eu_length = s.max_slices * s.max_subslices * eu_stride;
            let total = 16 + slice_length + subslice_length + eu_length;
            if let Some(r) = check(total as usize) {
                return r;
            }
            // struct drm_i915_query_topology_info
            data[0..2].copy_from_slice(&0u16.to_le_bytes());
            data[2..4].copy_from_slice(&(s.max_slices as u16).to_le_bytes());
            data[4..6].copy_from_slice(&(s.max_subslices as u16).to_le_bytes());
            data[6..8].copy_from_slice(&(s.max_eus_per_subslice as u16).to_le_bytes());
            data[8..10].copy_from_slice(&(slice_length as u16).to_le_bytes());
            data[10..12].copy_from_slice(&(ss_stride as u16).to_le_bytes());
            data[12..14].copy_from_slice(&((slice_length + subslice_length) as u16).to_le_bytes());
            data[14..16].copy_from_slice(&(eu_stride as u16).to_le_bytes());
            data[16] = s.slice_mask as u8;
            let mut at = 17usize;
            for sl in 0..s.max_slices as usize {
                for b in 0..ss_stride as usize {
                    data[at] = (s.subslice_mask[sl] >> (8 * b)) as u8;
                    at += 1;
                }
            }
            for sl in 0..s.max_slices as usize {
                for ss in 0..s.max_subslices as usize {
                    for b in 0..eu_stride as usize {
                        data[at] = (s.eu_mask[sl][ss] >> (8 * b)) as u8;
                        at += 1;
                    }
                }
            }
            total as i32
        }
        DRM_I915_QUERY_ENGINE_INFO => {
            if flags != 0 {
                return -EINVAL;
            }
            let (ids, n) = uabi_engines(gt);
            let total = 16 + n * 56;
            if let Some(r) = check(total) {
                return r;
            }
            // struct drm_i915_query_engine_info, drm_i915_engine_info.
            data[0..4].copy_from_slice(&(n as u32).to_le_bytes());
            let mut inst = [0u16; 4];
            for (k, &id) in ids[..n].iter().enumerate() {
                let e = gt.engine[id as usize].unwrap();
                let class = uabi_class(e.class);
                let o = 16 + k * 56;
                data[o..o + 2].copy_from_slice(&class.to_le_bytes());
                data[o + 2..o + 4].copy_from_slice(&inst[class as usize].to_le_bytes());
                inst[class as usize] += 1;
                data[o + 8..o + 16].copy_from_slice(&I915_ENGINE_INFO_HAS_LOGICAL_INSTANCE.to_le_bytes());
                data[o + 16..o + 24].copy_from_slice(&(e.uabi_capabilities as u64).to_le_bytes());
                let logical = if e.logical_mask == 0 { 0 } else { 31 - e.logical_mask.leading_zeros() };
                data[o + 24..o + 26].copy_from_slice(&(logical as u16).to_le_bytes());
            }
            total as i32
        }
        DRM_I915_QUERY_MEMORY_REGIONS => {
            if flags != 0 {
                return -EINVAL;
            }
            // One region on an integrated part: system memory (stolen is
            // private).
            let total = 16 + 88;
            if let Some(r) = check(total) {
                return r;
            }
            let ram = (npk_sys::sys_info(2).max(0) as u64) << 20;
            data[0..4].copy_from_slice(&1u32.to_le_bytes());
            let o = 16;
            data[o..o + 2].copy_from_slice(&INTEL_MEMORY_SYSTEM.to_le_bytes());
            data[o + 8..o + 16].copy_from_slice(&ram.to_le_bytes());
            data[o + 16..o + 24].copy_from_slice(&ram.to_le_bytes());
            data[o + 24..o + 32].copy_from_slice(&ram.to_le_bytes());
            data[o + 32..o + 40].copy_from_slice(&ram.to_le_bytes());
            total as i32
        }
        // No perf, no hwconfig table (TGL has none), geometry subslices from
        // 12.55, no GuC submission.
        DRM_I915_QUERY_PERF_CONFIG | DRM_I915_QUERY_HWCONFIG_BLOB | DRM_I915_QUERY_GEOMETRY_SUBSLICES
        | DRM_I915_QUERY_GUC_SUBMISSION_VERSION => -ENODEV,
        _ => -EINVAL,
    }
}
