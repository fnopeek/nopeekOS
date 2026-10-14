//! gt/intel_llc.c (Linux 6.18.55): the ring (LLC) frequency table the
//! PCU uses per GPU frequency.

use crate::i915_drv::drm_i915_private;
use crate::intel_gt::intel_gt;
use crate::intel_rps::{intel_rps_get_max_raw_freq, intel_rps_get_min_raw_freq};
use crate::linux::Line;
use crate::regs::*;

/// `struct ia_constants`.
struct ia_constants {
    min_gpu_freq: u32,
    max_gpu_freq: u32,
    min_ring_freq: u32,
}

/// `get_ia_constants`. `max_ia_freq` (from cpufreq or the TSC) only feeds
/// the pre-Haswell branch of `calc_ia_freq`, so it is not read.
fn get_ia_constants(i915: &mut drm_i915_private, gt: &intel_gt) -> Option<ia_constants> {
    if TGL_HAS_LLC == 0 {
        return None;
    }

    let mut min_ring_freq = i915.uncore.intel_uncore_read(DCLK) & 0xf;
    // convert DDR frequency from units of 266.6MHz to bandwidth
    min_ring_freq = min_ring_freq * 8 / 3;

    Some(ia_constants {
        min_gpu_freq: intel_rps_get_min_raw_freq(gt),
        max_gpu_freq: intel_rps_get_max_raw_freq(gt),
        min_ring_freq,
    })
}

/// `calc_ia_freq`, GRAPHICS_VER >= 9: ring_freq = 2 * GT in 100 MHz units,
/// no floor, IA frequency left to the PCU.
fn calc_ia_freq(gpu_freq: u32) -> (u32, u32) {
    (0, gpu_freq)
}

/// `gen6_update_ring_freq`: one PCU table entry per GPU frequency, from
/// the highest down.
fn gen6_update_ring_freq(i915: &mut drm_i915_private, gt: &intel_gt) {
    let Some(consts) = get_ia_constants(i915, gt) else {
        return;
    };

    if consts.max_gpu_freq <= consts.min_gpu_freq {
        return;
    }

    let mut written = 0;
    let mut gpu_freq = consts.max_gpu_freq;
    while gpu_freq >= consts.min_gpu_freq {
        let (ia_freq, ring_freq) = calc_ia_freq(gpu_freq);
        let _ = crate::intel_pcode::intel_pcode_write(i915, GEN6_PCODE_WRITE_MIN_FREQ_TABLE,
            ia_freq << GEN6_PCODE_FREQ_IA_RATIO_SHIFT | ring_freq << GEN6_PCODE_FREQ_RING_RATIO_SHIFT | gpu_freq);
        written += 1;
        if gpu_freq == 0 {
            break;
        }
        gpu_freq -= 1;
    }
    Line::new().s("LLC: ring frequency table, ").dec(written).s(" entries (GPU ")
        .dec(consts.min_gpu_freq as u64 * 50).s("-").dec(consts.max_gpu_freq as u64 * 50)
        .s(" MHz, DDR floor ").dec(consts.min_ring_freq as u64).s(")").say();
}

/// `intel_llc_enable`.
pub fn intel_llc_enable(i915: &mut drm_i915_private, gt: &intel_gt) {
    gen6_update_ring_freq(i915, gt);
}

/// `intel_llc_disable`: there is no hardware configuration to undo.
pub fn intel_llc_disable() {}
