//! display/skl_watermark.c and intel_wm.c (Linux 6.18.55): `intel_wm_init`
//! for display version 12 — SAGV state and the memory latencies.

use crate::i915_drv::drm_i915_private;
use crate::intel_pcode;
use crate::linux::Line;
use crate::regs::*;

pub const NUM_LATENCY: usize = WM_SKL_LATENCY_LEVELS as usize;

fn reg_field_get(mask: u32, val: u32) -> u16 {
    ((val & mask) >> mask.trailing_zeros()) as u16
}

/// `intel_has_sagv` (HAS_SAGV is true from display version 9, not BXT/GLK).
pub fn intel_has_sagv(i915: &drm_i915_private) -> bool {
    i915.display.sagv_status != I915_SAGV_NOT_CONTROLLED
}

/// `intel_sagv_block_time` (display version 12).
fn intel_sagv_block_time(i915: &mut drm_i915_private) -> u32 {
    let mut val = 0u32;
    if intel_pcode::snb_pcode_read(i915, GEN12_PCODE_READ_SAGV_BLOCK_TIME_US, &mut val, None).is_err() {
        Line::new().s("Couldn't read SAGV block time!").say();
        return 0;
    }
    val
}

/// `intel_sagv_init` (display version 11 and up: the status came from
/// `intel_bw_init_hw`).
fn intel_sagv_init(i915: &mut drm_i915_private) {
    if i915.display.sagv_status == I915_SAGV_UNKNOWN {
        crate::linux::say("warning: SAGV status unknown");
    }

    i915.display.sagv_block_time_us = intel_sagv_block_time(i915);

    Line::new().s("SAGV supported: ").s(if intel_has_sagv(i915) { "yes" } else { "no" })
        .s(", original SAGV block time: ").dec(i915.display.sagv_block_time_us as u64).s(" us").say();

    // Avoid overflow when adding with wm0 latency/etc.
    if i915.display.sagv_block_time_us > u16::MAX as u32 {
        Line::new().s("Excessive SAGV block time ").dec(i915.display.sagv_block_time_us as u64)
            .s(", ignoring").say();
        i915.display.sagv_block_time_us = 0;
    }

    if !intel_has_sagv(i915) {
        i915.display.sagv_block_time_us = 0;
    }
}

/// `adjust_wm_latency`.
fn adjust_wm_latency(i915: &drm_i915_private, wm: &mut [u16; NUM_LATENCY], mut num_levels: usize,
                     read_latency: u16) {
    // A level n (n > 1) with 0 us latency disables every level from n on.
    for level in 1..num_levels {
        if wm[level] == 0 {
            for w in wm.iter_mut().take(num_levels).skip(level + 1) {
                *w = 0;
            }
            num_levels = level;
            break;
        }
    }

    // WaWmMemoryReadLatency: the punit leaves out the read latency when
    // level 0 reads 0 us.
    if wm[0] == 0 {
        for w in wm.iter_mut().take(num_levels) {
            *w += read_latency;
        }
    }

    // WA Level-0 adjustment for 16Gb DIMMs: SKL+
    if i915.dram_info.has_16gb_dimms {
        wm[0] += 1;
    }
}

/// `skl_read_wm_latency` (display version 12, not DG2).
fn skl_read_wm_latency(i915: &mut drm_i915_private, wm: &mut [u16; NUM_LATENCY]) {
    let num_levels = i915.display.wm_num_levels;
    let read_latency = 3;
    let mult = 1;

    // data0 = 0 reads the first set, latencies [0:3]
    let mut val = 0u32;
    if let Err(e) = intel_pcode::snb_pcode_read(i915, GEN9_PCODE_READ_MEM_LATENCY, &mut val, None) {
        Line::new().s("SKL Mailbox read error = ").dec((-e) as u64).say();
        return;
    }
    wm[0] = reg_field_get(GEN9_MEM_LATENCY_LEVEL_0_4_MASK, val) * mult;
    wm[1] = reg_field_get(GEN9_MEM_LATENCY_LEVEL_1_5_MASK, val) * mult;
    wm[2] = reg_field_get(GEN9_MEM_LATENCY_LEVEL_2_6_MASK, val) * mult;
    wm[3] = reg_field_get(GEN9_MEM_LATENCY_LEVEL_3_7_MASK, val) * mult;

    // data0 = 1 reads the second set, latencies [4:7]
    let mut val = 1u32;
    if let Err(e) = intel_pcode::snb_pcode_read(i915, GEN9_PCODE_READ_MEM_LATENCY, &mut val, None) {
        Line::new().s("SKL Mailbox read error = ").dec((-e) as u64).say();
        return;
    }
    wm[4] = reg_field_get(GEN9_MEM_LATENCY_LEVEL_0_4_MASK, val) * mult;
    wm[5] = reg_field_get(GEN9_MEM_LATENCY_LEVEL_1_5_MASK, val) * mult;
    wm[6] = reg_field_get(GEN9_MEM_LATENCY_LEVEL_2_6_MASK, val) * mult;
    wm[7] = reg_field_get(GEN9_MEM_LATENCY_LEVEL_3_7_MASK, val) * mult;

    adjust_wm_latency(i915, wm, num_levels, read_latency);
}

/// `intel_print_wm_latency` (display version 9 and up: microseconds).
fn intel_print_wm_latency(i915: &drm_i915_private, name: &str, wm: &[u16; NUM_LATENCY]) {
    let mut l = Line::new();
    l.s(name).s(" WM latency:");
    for level in 0..i915.display.wm_num_levels {
        l.s(" ").dec(wm[level] as u64);
    }
    l.s(" us").say();
}

/// `skl_setup_wm_latency` (no HW SAGV watermarks before display version 13).
fn skl_setup_wm_latency(i915: &mut drm_i915_private) {
    i915.display.wm_num_levels = 8;
    let mut wm = i915.display.wm_skl_latency;
    skl_read_wm_latency(i915, &mut wm);
    i915.display.wm_skl_latency = wm;
    intel_print_wm_latency(i915, "Gen9 Plane", &wm);
}

/// `intel_wm_init` → `skl_wm_init`.
pub fn intel_wm_init(i915: &mut drm_i915_private) {
    intel_sagv_init(i915);
    skl_setup_wm_latency(i915);
}
