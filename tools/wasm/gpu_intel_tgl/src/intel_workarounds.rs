//! gt/intel_workarounds.c (Linux 6.18.55): workaround lists, their
//! application and verification; the GT list for Tiger Lake.
//!
//! A list is kept sorted by register offset, and two entries for one
//! register are merged, as `_wa_add` does. The engine, context and
//! whitelist lists use the same type; they need engine objects.

use crate::i915_drv::drm_i915_private;
use crate::intel_gt::{intel_gt, intel_gt_mcr_multicast_write_fw, intel_gt_mcr_read_any_fw};
use crate::intel_uncore::intel_uncore;
use crate::linux::Line;
use crate::regs::*;

/// Entries one list holds; the module has no heap that frees, so the
/// list is a fixed array (Linux grows it in `WA_LIST_CHUNK` steps).
pub const WA_LIST_MAX: usize = 64;

/// `struct i915_wa`. `reg` is the offset for MCR and plain registers alike.
#[derive(Clone, Copy, Default)]
pub struct i915_wa {
    pub reg: u32,
    pub clr: u32,
    pub set: u32,
    pub read: u32,
    pub masked_reg: bool,
    pub is_mcr: bool,
}

/// `struct i915_wa_list`.
#[derive(Clone, Copy)]
pub struct i915_wa_list {
    pub name: &'static str,
    pub engine_name: &'static str,
    pub list: [i915_wa; WA_LIST_MAX],
    pub count: usize,
    pub wa_count: usize,
}

impl Default for i915_wa_list {
    fn default() -> Self {
        Self { name: "", engine_name: "", list: [i915_wa::default(); WA_LIST_MAX], count: 0, wa_count: 0 }
    }
}

/// `_MASKED_BIT_ENABLE`.
pub const fn masked_bit_enable(a: u32) -> u32 {
    (a << 16) | a
}

/// `_MASKED_BIT_DISABLE`.
pub const fn masked_bit_disable(a: u32) -> u32 {
    a << 16
}

/// `_MASKED_FIELD`.
pub const fn masked_field(mask: u32, value: u32) -> u32 {
    (mask << 16) | value
}

/// `wa_init_start`.
pub fn wa_init_start(wal: &mut i915_wa_list, name: &'static str, engine_name: &'static str) {
    wal.name = name;
    wal.engine_name = engine_name;
}

/// `wa_init_finish`.
pub fn wa_init_finish(wal: &i915_wa_list) {
    if wal.count == 0 {
        return;
    }
    Line::new().s("Initialized ").dec(wal.wa_count as u64).s(" ").s(wal.name).s(" workarounds on ")
        .s(wal.engine_name).say();
}

/// `wal_get_fw_for_rmw`.
fn wal_get_fw_for_rmw(uncore: &intel_uncore, wal: &i915_wa_list) -> u32 {
    let mut fw = 0;
    for wa in &wal.list[..wal.count] {
        fw |= uncore.intel_uncore_forcewake_for_reg(wa.reg, FW_REG_READ | FW_REG_WRITE);
    }
    fw
}

/// `_wa_add`: insert sorted by offset, or merge into the entry already
/// there.
fn _wa_add(wal: &mut i915_wa_list, wa: &i915_wa) {
    let addr = wa.reg;
    let mut start = 0;
    let mut end = wal.count;

    while start < end {
        let mid = start + (end - start) / 2;
        if wal.list[mid].reg < addr {
            start = mid + 1;
        } else if wal.list[mid].reg > addr {
            end = mid;
        } else {
            let wa_ = &mut wal.list[mid];
            if (wa.clr | wa_.clr) != 0 && wa.clr & !wa_.clr == 0 {
                Line::new().s("error: Discarding overwritten w/a for reg ").hex(wa_.reg).s(" (clear: ")
                    .hex(wa_.clr).s(", set: ").hex(wa_.set).s(")").say();
                wa_.set &= !wa.clr;
            }
            wal.wa_count += 1;
            wa_.set |= wa.set;
            wa_.clr |= wa.clr;
            wa_.read |= wa.read;
            return;
        }
    }

    if wal.count == WA_LIST_MAX {
        crate::linux::drm_err("No space for workaround init!");
        return;
    }
    wal.wa_count += 1;
    let mut i = wal.count;
    wal.list[i] = *wa;
    wal.count += 1;
    while i > 0 {
        if wal.list[i].reg > wal.list[i - 1].reg {
            break;
        }
        wal.list.swap(i, i - 1);
        i -= 1;
    }
}

/// `wa_add`.
pub fn wa_add(wal: &mut i915_wa_list, reg: u32, clear: u32, set: u32, read_mask: u32, masked_reg: bool) {
    _wa_add(wal, &i915_wa { reg, clr: clear, set, read: read_mask, masked_reg, is_mcr: false });
}

/// `wa_mcr_add`.
pub fn wa_mcr_add(wal: &mut i915_wa_list, reg: u32, clear: u32, set: u32, read_mask: u32, masked_reg: bool) {
    _wa_add(wal, &i915_wa { reg, clr: clear, set, read: read_mask, masked_reg, is_mcr: true });
}

/// `wa_write_clr_set`.
pub fn wa_write_clr_set(wal: &mut i915_wa_list, reg: u32, clear: u32, set: u32) {
    wa_add(wal, reg, clear, set, clear | set, false);
}

/// `wa_mcr_write_clr_set`.
pub fn wa_mcr_write_clr_set(wal: &mut i915_wa_list, reg: u32, clear: u32, set: u32) {
    wa_mcr_add(wal, reg, clear, set, clear | set, false);
}

/// `wa_write`.
pub fn wa_write(wal: &mut i915_wa_list, reg: u32, set: u32) {
    wa_write_clr_set(wal, reg, !0, set);
}

/// `wa_write_or`.
pub fn wa_write_or(wal: &mut i915_wa_list, reg: u32, set: u32) {
    wa_write_clr_set(wal, reg, set, set);
}

/// `wa_mcr_write_or`.
pub fn wa_mcr_write_or(wal: &mut i915_wa_list, reg: u32, set: u32) {
    wa_mcr_write_clr_set(wal, reg, set, set);
}

/// `wa_write_clr`.
pub fn wa_write_clr(wal: &mut i915_wa_list, reg: u32, clr: u32) {
    wa_write_clr_set(wal, reg, clr, 0);
}

/// `wa_mcr_write_clr`.
pub fn wa_mcr_write_clr(wal: &mut i915_wa_list, reg: u32, clr: u32) {
    wa_mcr_write_clr_set(wal, reg, clr, 0);
}

/// `wa_masked_en`.
pub fn wa_masked_en(wal: &mut i915_wa_list, reg: u32, val: u32) {
    wa_add(wal, reg, 0, masked_bit_enable(val), val, true);
}

/// `wa_mcr_masked_en`.
pub fn wa_mcr_masked_en(wal: &mut i915_wa_list, reg: u32, val: u32) {
    wa_mcr_add(wal, reg, 0, masked_bit_enable(val), val, true);
}

/// `wa_masked_dis`.
pub fn wa_masked_dis(wal: &mut i915_wa_list, reg: u32, val: u32) {
    wa_add(wal, reg, 0, masked_bit_disable(val), val, true);
}

/// `wa_mcr_masked_dis`.
pub fn wa_mcr_masked_dis(wal: &mut i915_wa_list, reg: u32, val: u32) {
    wa_mcr_add(wal, reg, 0, masked_bit_disable(val), val, true);
}

/// `wa_masked_field_set`.
pub fn wa_masked_field_set(wal: &mut i915_wa_list, reg: u32, mask: u32, val: u32) {
    wa_add(wal, reg, 0, masked_field(mask, val), mask, true);
}

/// `wa_mcr_masked_field_set`.
pub fn wa_mcr_masked_field_set(wal: &mut i915_wa_list, reg: u32, mask: u32, val: u32) {
    wa_mcr_add(wal, reg, 0, masked_field(mask, val), mask, true);
}

/// `__set_mcr_steering`.
fn __set_mcr_steering(wal: &mut i915_wa_list, steering_reg: u32, slice: u32, subslice: u32) {
    let mcr = GEN11_MCR_SLICE(slice) | GEN11_MCR_SUBSLICE(subslice);
    let mcr_mask = GEN11_MCR_SLICE_MASK | GEN11_MCR_SUBSLICE_MASK;
    wa_write_clr_set(wal, steering_reg, mcr_mask, mcr);
}

/// `__add_mcr_wa`; `debug_dump_steering` only prints under drm debug.
fn __add_mcr_wa(gt: &mut intel_gt, slice: u32, subslice: u32) {
    __set_mcr_steering(&mut gt.wa_list, GEN8_MCR_SELECTOR, slice, subslice);
    gt.default_steering = (slice as u8, subslice as u8);
}

/// `icl_wa_init_mcr`: steer reads to the lowest subslice that is not fused
/// off; if it also has an L3 bank, the L3 bank table is not needed.
fn icl_wa_init_mcr(gt: &mut intel_gt) {
    // intel_sseu_get_hsw_subslices(sseu, 0)
    let subslice = gt.info.sseu.subslice_mask[0].trailing_zeros();
    if gt.info.l3bank_mask & (1 << subslice) != 0 {
        gt.steering_l3bank = false;
    }
    __add_mcr_wa(gt, 0, subslice);
}

/// `wa_14011060649`: every even VDBOX.
fn wa_14011060649(gt: &mut intel_gt) {
    for id in 0..I915_NUM_ENGINES as usize {
        let Some(engine) = gt.engine[id] else { continue };
        if engine.class != VIDEO_DECODE_CLASS || engine.instance % 2 != 0 {
            continue;
        }
        wa_write_or(&mut gt.wa_list, VDBOX_CGCTL3F10(engine.mmio_base), IECPUNIT_CLKGATE_DIS);
    }
}

/// `gen12_gt_workarounds_init`.
fn gen12_gt_workarounds_init(gt: &mut intel_gt) {
    icl_wa_init_mcr(gt);

    // Wa_14011060649:tgl,rkl,dg1,adl-s,adl-p
    wa_14011060649(gt);

    // Wa_14011059788:tgl,rkl,adl-s,dg1,adl-p
    wa_mcr_write_or(&mut gt.wa_list, GEN10_DFR_RATIO_EN_AND_CHICKEN, DFR_DISABLE);

    // Wa_14015795083: firmware may lock MISCCPCTL, so there is no
    // readback verification.
    wa_add(&mut gt.wa_list, GEN7_MISCCPCTL, GEN12_DOP_CLOCK_GATE_RENDER_ENABLE, 0, 0, false);
}

/// `gt_init_workarounds`. `gt_tuning_settings` only acts on 12.70+ and
/// DG2; TGL is the GRAPHICS_VER 12 branch.
fn gt_init_workarounds(gt: &mut intel_gt) {
    gen12_gt_workarounds_init(gt);
}

/// `intel_gt_init_workarounds`.
pub fn intel_gt_init_workarounds(gt: &mut intel_gt) {
    wa_init_start(&mut gt.wa_list, "GT", "global");
    gt_init_workarounds(gt);
    wa_init_finish(&gt.wa_list);
}

/// `wa_verify`.
fn wa_verify(wa: &i915_wa, cur: u32, name: &str, from: &str) -> bool {
    if (cur ^ wa.set) & wa.read != 0 {
        Line::new().s("error: ").s(name).s(" workaround lost on ").s(from).s("! (reg[").hex(wa.reg)
            .s("]=").hex(cur).s(", relevant bits were ").hex(cur & wa.read).s(" vs expected ")
            .hex(wa.set & wa.read).s(")").say();
        return false;
    }
    true
}

/// `wa_list_apply`: the open-coded read-modify-write under one forcewake
/// grab. The MCR lock is a software lock before 12.70; the module is
/// single-threaded. The readback under CONFIG_DRM_I915_DEBUG_GEM is not
/// built.
pub fn wa_list_apply(i915: &mut drm_i915_private, gt: &intel_gt, wal: &i915_wa_list) {
    if wal.count == 0 {
        return;
    }
    let fw = wal_get_fw_for_rmw(&i915.uncore, wal);
    i915.uncore.intel_uncore_forcewake_get__locked(fw);

    for wa in &wal.list[..wal.count] {
        let mut old = 0;
        if wa.clr != 0 {
            old = if wa.is_mcr {
                intel_gt_mcr_read_any_fw(&i915.uncore, gt, wa.reg)
            } else {
                i915.uncore.intel_uncore_read_fw(wa.reg)
            };
        }
        let val = (old & !wa.clr) | wa.set;
        if val != old || wa.clr == 0 {
            if wa.is_mcr {
                intel_gt_mcr_multicast_write_fw(&i915.uncore, wa.reg, val);
            } else {
                i915.uncore.intel_uncore_write_fw(wa.reg, val);
            }
        }
    }

    i915.uncore.intel_uncore_forcewake_put__locked(fw);
}

/// `intel_gt_apply_workarounds`.
pub fn intel_gt_apply_workarounds(i915: &mut drm_i915_private, gt: &intel_gt) {
    let wal = gt.wa_list;
    wa_list_apply(i915, gt, &wal);
}

/// `wa_list_verify`.
pub fn wa_list_verify(i915: &mut drm_i915_private, gt: &intel_gt, wal: &i915_wa_list, from: &str) -> bool {
    let fw = wal_get_fw_for_rmw(&i915.uncore, wal);
    i915.uncore.intel_uncore_forcewake_get__locked(fw);

    let mut ok = true;
    for wa in &wal.list[..wal.count] {
        let cur = if wa.is_mcr {
            intel_gt_mcr_read_any_fw(&i915.uncore, gt, wa.reg)
        } else {
            i915.uncore.intel_uncore_read_fw(wa.reg)
        };
        ok &= wa_verify(wa, cur, wal.name, from);
    }

    i915.uncore.intel_uncore_forcewake_put__locked(fw);
    ok
}

/// `intel_gt_verify_workarounds`.
pub fn intel_gt_verify_workarounds(i915: &mut drm_i915_private, gt: &intel_gt, from: &str) -> bool {
    let wal = gt.wa_list;
    wa_list_verify(i915, gt, &wal, from)
}

// Engine, context and whitelist lists. `class`/`instance`/`mmio_base`
// identify the engine (`engine->class`, `engine->instance`,
// `engine->mmio_base`).

/// `intel_engine_init_workarounds`.
pub fn intel_engine_init_workarounds(_i915: &mut drm_i915_private, _gt: &intel_gt, _class: u32, _instance: u32,
                                     _mmio_base: u32, wal: &mut i915_wa_list) {
    wa_init_start(wal, "engine", "");
    wa_init_finish(wal);
}

/// `intel_engine_apply_workarounds`.
pub fn intel_engine_apply_workarounds(i915: &mut drm_i915_private, gt: &intel_gt, wal: &i915_wa_list) {
    wa_list_apply(i915, gt, wal);
}

/// `intel_engine_init_whitelist`.
pub fn intel_engine_init_whitelist(_i915: &mut drm_i915_private, _gt: &intel_gt, _class: u32, _instance: u32,
                                   _mmio_base: u32, wal: &mut i915_wa_list) {
    wa_init_start(wal, "whitelist", "");
    wa_init_finish(wal);
}

/// `intel_engine_apply_whitelist`.
pub fn intel_engine_apply_whitelist(_i915: &mut drm_i915_private, _gt: &intel_gt, _mmio_base: u32,
                                    _wal: &i915_wa_list) {}

/// `intel_engine_init_ctx_wa` (`__intel_engine_init_ctx_wa`): the
/// context workarounds, emitted into each context by
/// `intel_engine_emit_ctx_wa`.
pub fn intel_engine_init_ctx_wa(_i915: &mut drm_i915_private, _gt: &intel_gt, _class: u32, _instance: u32,
                                _mmio_base: u32, wal: &mut i915_wa_list) {
    wa_init_start(wal, "context", "");
    wa_init_finish(wal);
}
