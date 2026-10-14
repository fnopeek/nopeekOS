//! gt/intel_mocs.c (Linux 6.18.55): the memory object control state
//! table of Tiger Lake — the global MOCS registers and the L3 control
//! (LNCFCMOCS) pairs.

use crate::i915_drv::drm_i915_private;
use crate::intel_gt::intel_gt;
use crate::regs::*;
use crate::tables;

/// `struct drm_i915_mocs_entry`.
#[derive(Clone, Copy)]
pub struct drm_i915_mocs_entry {
    pub control_value: u32,
    pub l3cc_value: u16,
    pub used: u16,
}

/// `struct drm_i915_mocs_table`.
#[derive(Clone, Copy)]
pub struct drm_i915_mocs_table {
    pub size: u32,
    pub n_entries: u32,
    pub table: &'static [drm_i915_mocs_entry],
    pub uc_index: u8,
    pub wb_index: u8,
    pub unused_entries_index: u8,
}

/// `gt->mocs`.
#[derive(Clone, Copy, Default)]
pub struct intel_mocs {
    pub uc_index: u8,
    pub wb_index: u8,
}

const HAS_GLOBAL_MOCS: u32 = 1 << 0;
const HAS_ENGINE_MOCS: u32 = 1 << 1;
const HAS_RENDER_L3CC: u32 = 1 << 2;

/// `get_mocs_settings`, the Tiger Lake branch. `has_mocs` is !IS_DGFX,
/// `has_l3cc` is always true; GRAPHICS_VER 12 needs no
/// WaDisableSkipCaching check.
fn get_mocs_settings() -> (u32, drm_i915_mocs_table) {
    let table = drm_i915_mocs_table {
        size: tables::tgl_mocs_table.len() as u32,
        n_entries: GEN9_NUM_MOCS_ENTRIES,
        table: &tables::tgl_mocs_table,
        uc_index: 3,
        wb_index: 0,
        unused_entries_index: I915_MOCS_PTE as u8,
    };

    if table.size > table.n_entries {
        return (0, table);
    }

    let mut flags = 0;
    if TGL_HAS_GLOBAL_MOCS != 0 {
        flags |= HAS_GLOBAL_MOCS;
    } else {
        flags |= HAS_ENGINE_MOCS;
    }
    flags |= HAS_RENDER_L3CC;
    (flags, table)
}

/// `get_entry_control`.
fn get_entry_control(table: &drm_i915_mocs_table, index: u32) -> u32 {
    if index < table.size && table.table[index as usize].used != 0 {
        return table.table[index as usize].control_value;
    }
    table.table[table.unused_entries_index as usize].control_value
}

/// `__init_mocs_table`.
fn __init_mocs_table(i915: &mut drm_i915_private, table: &drm_i915_mocs_table, addr: u32) {
    if table.unused_entries_index == 0 {
        crate::linux::say("warning: Unused entries index should have been defined");
    }
    for i in 0..table.n_entries {
        let mocs = get_entry_control(table, i);
        i915.uncore.intel_uncore_write_fw(addr + i * 4, mocs);
    }
}

/// `get_entry_l3cc`.
fn get_entry_l3cc(table: &drm_i915_mocs_table, index: u32) -> u16 {
    if index < table.size && table.table[index as usize].used != 0 {
        return table.table[index as usize].l3cc_value;
    }
    table.table[table.unused_entries_index as usize].l3cc_value
}

/// `l3cc_combine`.
fn l3cc_combine(low: u16, high: u16) -> u32 {
    low as u32 | (high as u32) << 16
}

/// `init_l3cc_table`: below 12.55 the LNCFCMOCS registers are plain.
pub fn init_l3cc_table(i915: &mut drm_i915_private, table: &drm_i915_mocs_table) {
    for i in 0..(table.n_entries + 1) / 2 {
        let l3cc = l3cc_combine(get_entry_l3cc(table, 2 * i), get_entry_l3cc(table, 2 * i + 1));
        i915.uncore.intel_uncore_write_fw(GEN9_LNCFCMOCS(i), l3cc);
    }
}

/// `global_mocs_offset`.
fn global_mocs_offset() -> u32 {
    GEN12_GLOBAL_MOCS(0)
}

/// `intel_set_mocs_index`.
pub fn intel_set_mocs_index(gt: &mut intel_gt) {
    let (_, table) = get_mocs_settings();
    gt.mocs.uc_index = table.uc_index;
    if TGL_HAS_L3_CCS_READ != 0 {
        gt.mocs.wb_index = table.wb_index;
    }
}

/// `intel_mocs_init`: the global table, then the L3CC table so that the
/// LNCFCMOCS registers are set before any memory transaction.
pub fn intel_mocs_init(i915: &mut drm_i915_private) -> u32 {
    let (flags, table) = get_mocs_settings();
    if flags & HAS_GLOBAL_MOCS != 0 {
        __init_mocs_table(i915, &table, global_mocs_offset());
    }
    if flags & HAS_RENDER_L3CC != 0 {
        init_l3cc_table(i915, &table);
    }
    table.n_entries
}

/// `intel_mocs_init_engine`. TGL has global MOCS, so the per-engine
/// table (`init_mocs_table`) is never written; the render engine rewrites
/// the L3CC table.
pub fn intel_mocs_init_engine(i915: &mut drm_i915_private, class: u32) {
    let (flags, table) = get_mocs_settings();
    if flags == 0 {
        return;
    }
    if flags & HAS_RENDER_L3CC != 0 && class == RENDER_CLASS {
        init_l3cc_table(i915, &table);
    }
}
