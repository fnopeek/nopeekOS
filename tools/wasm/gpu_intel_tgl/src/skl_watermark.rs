//! display/skl_watermark.c and intel_wm.c (Linux 6.18.55) for display
//! version 12: `intel_wm_init` (SAGV state, memory latencies), the DDB and
//! watermark readout and the DBUF sanitize check.

use crate::i915_drv::drm_i915_private;
use crate::intel_de::intel_de_read;
use crate::intel_display_types::{intel_crtc_state, skl_ddb_entry, skl_pipe_wm, skl_wm_level};
use crate::intel_pcode;
use crate::linux::Line;
use crate::regs::*;
use crate::tables;

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

// ── DDB and watermark readout (`skl_wm_get_hw_state`) and sanitize ────

const MAX_PIPES: usize = I915_MAX_PIPES as usize;

/// `struct intel_dbuf_state`.
#[derive(Clone, Copy, Default)]
pub struct intel_dbuf_state {
    pub ddb: [skl_ddb_entry; MAX_PIPES],
    pub weight: [u32; MAX_PIPES],
    pub slices: [u8; MAX_PIPES],
    pub enabled_slices: u8,
    pub active_pipes: u8,
    pub mdclk_cdclk_ratio: u8,
    pub joined_mbus: bool,
}

/// One row of `tgl_allowed_dbufs`: `dbuf_mask[pipe]` is `m<pipe>`.
#[derive(Clone, Copy)]
pub struct dbuf_slice_conf_entry {
    pub active_pipes: u32,
    pub join_mbus: u32,
    pub m0: u32,
    pub m1: u32,
    pub m2: u32,
    pub m3: u32,
}

/// `skl_ddb_entry_size`.
pub fn skl_ddb_entry_size(entry: &skl_ddb_entry) -> u16 {
    entry.end - entry.start
}

/// `skl_ddb_entry_init_from_hw`.
fn skl_ddb_entry_init_from_hw(entry: &mut skl_ddb_entry, reg: u32) {
    entry.start = reg_field_get(PLANE_BUF_START_MASK, reg);
    entry.end = reg_field_get(PLANE_BUF_END_MASK, reg);
    if entry.end != 0 {
        entry.end += 1;
    }
}

/// `skl_ddb_get_hw_plane_state` (display version 11-29: no NV12 Y buffer,
/// no minimum/interim buffer registers).
fn skl_ddb_get_hw_plane_state(i915: &mut drm_i915_private, pipe: u32, plane_id: u32, ddb: &mut skl_ddb_entry) {
    // Cursor doesn't support NV12/planar, so no extra calculation needed
    if plane_id == PLANE_CURSOR {
        let val = intel_de_read(i915, CUR_BUF_CFG(pipe));
        skl_ddb_entry_init_from_hw(ddb, val);
        return;
    }
    let val = intel_de_read(i915, PLANE_BUF_CFG(pipe, plane_id));
    skl_ddb_entry_init_from_hw(ddb, val);
}

/// `skl_ddb_entry_union`.
fn skl_ddb_entry_union(a: &mut skl_ddb_entry, b: &skl_ddb_entry) {
    if a.end != 0 && b.end != 0 {
        a.start = a.start.min(b.start);
        a.end = a.end.max(b.end);
    } else if b.end != 0 {
        a.start = b.start;
        a.end = b.end;
    }
}

/// `skl_wm_level_from_reg_val` (no auto minimum allocation before
/// display version 30).
fn skl_wm_level_from_reg_val(val: u32, level: &mut skl_wm_level) {
    level.enable = val & PLANE_WM_EN != 0;
    level.ignore_lines = val & PLANE_WM_IGNORE_LINES != 0;
    level.blocks = reg_field_get(PLANE_WM_BLOCKS_MASK, val);
    level.lines = reg_field_get(PLANE_WM_LINES_MASK, val) as u8;
    level.auto_min_alloc_wm_enable = false;
}

/// `skl_pipe_wm_get_hw_state` (no HW SAGV watermarks before display
/// version 13: the SAGV levels copy level 0 and the transition level).
fn skl_pipe_wm_get_hw_state(i915: &mut drm_i915_private, c: usize, out: &mut skl_pipe_wm) {
    let pipe = i915.display.crtcs[c].pipe;
    let ids: [u32; I915_MAX_PLANES as usize] = core::array::from_fn(|i| i as u32);
    let mask = i915.display.crtcs[c].plane_ids_mask;
    for plane_id in ids.into_iter().filter(|id| mask & (1 << id) != 0) {
        let wm = &mut out.planes[plane_id as usize];
        for level in 0..i915.display.wm_num_levels {
            let val = if plane_id != PLANE_CURSOR {
                intel_de_read(i915, PLANE_WM(pipe, plane_id, level as u32))
            } else {
                intel_de_read(i915, CUR_WM(pipe, level as u32))
            };
            skl_wm_level_from_reg_val(val, &mut wm.wm[level]);
        }

        let val = if plane_id != PLANE_CURSOR {
            intel_de_read(i915, PLANE_WM_TRANS(pipe, plane_id))
        } else {
            intel_de_read(i915, CUR_WM_TRANS(pipe))
        };
        skl_wm_level_from_reg_val(val, &mut wm.trans_wm);

        wm.sagv_wm0 = wm.wm[0];
        wm.sagv_trans_wm = wm.trans_wm;
    }
}

/// `intel_dbuf_slice_size`.
fn intel_dbuf_slice_size() -> u32 {
    TGL_DISPLAY_DBUF_SIZE / TGL_DISPLAY_DBUF_SLICE_MASK.count_ones()
}

/// `skl_ddb_entry_for_slices`.
fn skl_ddb_entry_for_slices(slice_mask: u8) -> skl_ddb_entry {
    let slice_size = intel_dbuf_slice_size();
    if slice_mask == 0 {
        return skl_ddb_entry::default();
    }
    let start = slice_mask.trailing_zeros() * slice_size;
    let end = (8 - slice_mask.leading_zeros()) * slice_size;
    if start >= end || end > TGL_DISPLAY_DBUF_SIZE {
        crate::linux::say("warning: DBuf slice range out of bounds");
    }
    skl_ddb_entry { start: start as u16, end: end as u16 }
}

/// `mbus_ddb_offset` (slices S3/S4 do not exist on Tiger Lake).
fn mbus_ddb_offset(mut slice_mask: u8) -> u32 {
    if slice_mask & ((1 << DBUF_S1) | (1 << DBUF_S2)) as u8 != 0 {
        slice_mask = (1 << DBUF_S1) as u8;
    }
    skl_ddb_entry_for_slices(slice_mask).start as u32
}

/// `skl_ddb_dbuf_slice_mask`.
pub fn skl_ddb_dbuf_slice_mask(entry: &skl_ddb_entry) -> u8 {
    let slice_size = intel_dbuf_slice_size();
    if skl_ddb_entry_size(entry) == 0 {
        return 0;
    }
    let mut start_slice = entry.start as u32 / slice_size;
    let end_slice = (entry.end as u32 - 1) / slice_size;
    let mut slice_mask = 0u8;
    // A plane's DDB entry may span slices, but it is contiguous.
    while start_slice <= end_slice {
        slice_mask |= 1 << start_slice;
        start_slice += 1;
    }
    slice_mask
}

/// `compute_dbuf_slices` with `tgl_allowed_dbufs` (`tgl_compute_dbuf_slices`).
fn skl_compute_dbuf_slices(pipe: u32, active_pipes: u8, join_mbus: bool) -> u8 {
    for e in tables::tgl_allowed_dbufs.iter() {
        if e.active_pipes == 0 {
            break;
        }
        if e.active_pipes == active_pipes as u32 && (e.join_mbus != 0) == join_mbus {
            return [e.m0, e.m1, e.m2, e.m3][pipe as usize] as u8;
        }
    }
    0
}

/// `intel_crtc_ddb_weight`.
fn intel_crtc_ddb_weight(state: &intel_crtc_state) -> u32 {
    if !state.hw.active {
        return 0;
    }
    // drm_mode_get_hv_timing: with CRTC_STEREO_DOUBLE_ONLY only the
    // vertical timings of a stereo mode change, so this is hdisplay.
    state.hw.pipe_mode.hdisplay as u32
}

/// `skl_wm_get_hw_state`. MBUS joining comes with display version 13, and
/// MDCLK is CD2XCLK before version 20 (`intel_mdclk_cdclk_ratio` = 2).
pub fn skl_wm_get_hw_state(i915: &mut drm_i915_private) {
    let mut dbuf_state = i915.display.dbuf_state;

    dbuf_state.mdclk_cdclk_ratio = 2;
    dbuf_state.active_pipes = 0;

    for c in 0..MAX_PIPES {
        if !i915.display.crtc_valid[c] {
            continue;
        }
        let pipe = i915.display.crtcs[c].pipe as usize;
        let mut cs = i915.display.crtcs[c].state;

        cs.wm_skl.optimal = skl_pipe_wm::default();
        if cs.hw.active {
            let mut optimal = skl_pipe_wm::default();
            skl_pipe_wm_get_hw_state(i915, c, &mut optimal);
            cs.wm_skl.optimal = optimal;
            dbuf_state.active_pipes |= 1 << pipe;
        }
        cs.wm_skl.raw = cs.wm_skl.optimal;

        dbuf_state.ddb[pipe] = skl_ddb_entry::default();

        let ids: [u32; I915_MAX_PLANES as usize] = core::array::from_fn(|i| i as u32);
        let mask = i915.display.crtcs[c].plane_ids_mask;
    for plane_id in ids.into_iter().filter(|id| mask & (1 << id) != 0) {
            if !cs.hw.active {
                continue;
            }
            let mut ddb = skl_ddb_entry::default();
            skl_ddb_get_hw_plane_state(i915, pipe as u32, plane_id, &mut ddb);
            cs.wm_skl.plane_ddb[plane_id as usize] = ddb;

            let ddb_y = cs.wm_skl.plane_ddb_y[plane_id as usize];
            skl_ddb_entry_union(&mut dbuf_state.ddb[pipe], &ddb);
            skl_ddb_entry_union(&mut dbuf_state.ddb[pipe], &ddb_y);
        }

        dbuf_state.weight[pipe] = intel_crtc_ddb_weight(&cs);

        // Absolute offsets, for the overlap checks.
        let slices = skl_compute_dbuf_slices(pipe as u32, dbuf_state.active_pipes, dbuf_state.joined_mbus);
        let mbus_offset = mbus_ddb_offset(slices);
        cs.wm_skl.ddb.start = (mbus_offset + dbuf_state.ddb[pipe].start as u32) as u16;
        cs.wm_skl.ddb.end = (mbus_offset + dbuf_state.ddb[pipe].end as u32) as u16;

        // The slices actually used by the planes on the pipe
        dbuf_state.slices[pipe] = skl_ddb_dbuf_slice_mask(&cs.wm_skl.ddb);

        Line::new().s("pipe ").dec(pipe as u64).s(": dbuf slices ").hex(dbuf_state.slices[pipe] as u32)
            .s(", ddb (").dec(dbuf_state.ddb[pipe].start as u64).s(" - ").dec(dbuf_state.ddb[pipe].end as u64)
            .s("), active pipes ").hex(dbuf_state.active_pipes as u32).s(", mbus joined: no").say();

        i915.display.crtcs[c].state = cs;
    }

    dbuf_state.enabled_slices = i915.display.dbuf_enabled_slices;
    i915.display.dbuf_state = dbuf_state;
}

/// `skl_ddb_allocation_overlaps`.
fn skl_ddb_allocation_overlaps(ddb: &skl_ddb_entry, entries: &[skl_ddb_entry], ignore_idx: usize) -> bool {
    entries.iter().enumerate().any(|(i, e)| i != ignore_idx && ddb.start < e.end && e.start < ddb.end)
}

/// `skl_dbuf_is_misconfigured`.
fn skl_dbuf_is_misconfigured(i915: &drm_i915_private) -> bool {
    let dbuf_state = &i915.display.dbuf_state;
    let mut entries = [skl_ddb_entry::default(); MAX_PIPES];

    for c in 0..MAX_PIPES {
        if i915.display.crtc_valid[c] {
            entries[i915.display.crtcs[c].pipe as usize] = i915.display.crtcs[c].state.wm_skl.ddb;
        }
    }

    for c in 0..MAX_PIPES {
        if !i915.display.crtc_valid[c] {
            continue;
        }
        let pipe = i915.display.crtcs[c].pipe;
        let slices = skl_compute_dbuf_slices(pipe, dbuf_state.active_pipes, dbuf_state.joined_mbus);
        if dbuf_state.slices[pipe as usize] & !slices != 0 {
            return true;
        }
        if skl_ddb_allocation_overlaps(&i915.display.crtcs[c].state.wm_skl.ddb, &entries, pipe as usize) {
            return true;
        }
    }
    false
}

/// `skl_dbuf_sanitize`. The BIOS on TGL/RKL is known to place planes in
/// the wrong DBUF slices; Linux then turns every primary plane off
/// (`intel_plane_disable_noatomic`), which needs the vblank-synchronised
/// plane disable that is not ported. It is reported instead, and the
/// planes stay on.
fn skl_dbuf_sanitize(i915: &mut drm_i915_private) {
    if !skl_dbuf_is_misconfigured(i915) {
        return;
    }
    Line::new().s("BIOS has misprogrammed the DBUF; Linux disables all planes here, not ported").say();
}

/// `intel_wm_sanitize` → `skl_wm_sanitize`; `skl_mbus_sanitize` needs MBUS
/// joining (display version 13+).
pub fn intel_wm_sanitize(i915: &mut drm_i915_private) {
    skl_dbuf_sanitize(i915);
}
