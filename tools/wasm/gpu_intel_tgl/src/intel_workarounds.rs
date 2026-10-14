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
//
// Tiger Lake takes one branch of each platform ladder: GRAPHICS_VER 12,
// GRAPHICS_VER_FULL 12.00 (below 12.55, outside 12.70-12.74),
// IS_TIGERLAKE, and none of DG1, DG2, RKL, ADL-S, ADL-P or KBL. Only those
// branches are here; the tgl_info flags they depend on come from
// i915_pci.c.

/// `engine->name` as `intel_engine_setup` writes it ("%s'%u").
fn engine_name(class: u32, instance: u32) -> &'static str {
    const NAMES: [[&str; 8]; 6] = [
        ["rcs'0", "rcs'1", "rcs'2", "rcs'3", "rcs'4", "rcs'5", "rcs'6", "rcs'7"],
        ["vcs'0", "vcs'1", "vcs'2", "vcs'3", "vcs'4", "vcs'5", "vcs'6", "vcs'7"],
        ["vecs'0", "vecs'1", "vecs'2", "vecs'3", "vecs'4", "vecs'5", "vecs'6", "vecs'7"],
        ["bcs'0", "bcs'1", "bcs'2", "bcs'3", "bcs'4", "bcs'5", "bcs'6", "bcs'7"],
        ["other'0", "other'1", "other'2", "other'3", "other'4", "other'5", "other'6", "other'7"],
        ["ccs'0", "ccs'1", "ccs'2", "ccs'3", "ccs'4", "ccs'5", "ccs'6", "ccs'7"],
    ];
    let row = match class {
        RENDER_CLASS => 0,
        VIDEO_DECODE_CLASS => 1,
        VIDEO_ENHANCEMENT_CLASS => 2,
        COPY_ENGINE_CLASS => 3,
        COMPUTE_CLASS => 5,
        _ => 4,
    };
    NAMES[row][(instance as usize).min(7)]
}

/// `engine->flags` of the engine `class`/`instance` names.
fn engine_flags(gt: &intel_gt, class: u32, instance: u32) -> u64 {
    gt.engine.iter().flatten()
        .find(|e| e.class == class && e.instance == instance)
        .map_or(0, |e| e.flags)
}

/// `gen12_ctx_workarounds_init`; the IS_DG1 exception does not apply.
fn gen12_ctx_workarounds_init(wal: &mut i915_wa_list) {
    // Wa_1409142259, Wa_1409347922, Wa_1409252684, Wa_1409217633,
    // Wa_1409207793, Wa_1409178076, Wa_1408979724, Wa_14010443199,
    // Wa_14010698770, Wa_1409342910, Wa_22010465259
    wa_masked_en(wal, GEN11_COMMON_SLICE_CHICKEN3, GEN12_DISABLE_CPS_AWARE_COLOR_PIPE);

    // WaDisableGPGPUMidThreadPreemption:gen12
    wa_masked_field_set(wal, GEN8_CS_CHICKEN1, GEN9_PREEMPT_GPGPU_LEVEL_MASK,
                        GEN9_PREEMPT_GPGPU_THREAD_GROUP_LEVEL);

    // Wa_16011163337 (GS timer) and the TDS timer. FF_MODE2 reads back
    // wrong from the CPU (Wa_1608008084): written whole, not verified.
    wa_add(wal, GEN12_FF_MODE2, !0, FF_MODE2_TDS_TIMER_128 | FF_MODE2_GS_TIMER_224, 0, false);

    // Wa_1806527549
    wa_masked_en(wal, HIZ_CHICKEN, HZ_DEPTH_TEST_LE_GE_OPT_DISABLE);

    // Wa_1606376872
    wa_masked_en(wal, COMMON_SLICE_CHICKEN4, DISABLE_TDC_LOAD_BALANCING_CALC);

    // Needed for the fast-clear performance optimisation.
    wa_mcr_write_or(wal, GEN8_WM_CHICKEN2, WAIT_ON_DEPTH_STALL_DONE_DISABLE);
}

/// `gen12_ctx_gt_mocs_init`: blitter commands without a MOCS field use
/// the index in BLIT_CCTL, programmed to uncached.
fn gen12_ctx_gt_mocs_init(gt: &intel_gt, class: u32, mmio_base: u32, wal: &mut i915_wa_list) {
    if class == COPY_ENGINE_CLASS {
        let mocs = gt.mocs.uc_index as u32;
        wa_write_clr_set(wal, BLIT_CCTL(mmio_base), BLIT_CCTL_MASK, BLIT_CCTL_MOCS(mocs, mocs));
    }
}

/// `gen12_ctx_gt_fake_wa_init`; `fakewa_disable_nestedbb_mode` is for
/// 12.55 and later.
fn gen12_ctx_gt_fake_wa_init(i915: &drm_i915_private, gt: &intel_gt, class: u32, mmio_base: u32,
                             wal: &mut i915_wa_list) {
    if i915.graphics_ver_full() >= crate::i915_drv::ip_ver(12, 55) {
        Line::new().s("MISSING_CASE fakewa_disable_nestedbb_mode").say();
    }
    gen12_ctx_gt_mocs_init(gt, class, mmio_base, wal);
}

/// `intel_engine_init_ctx_wa` (`__intel_engine_init_ctx_wa`): the
/// context workarounds, emitted into each context by
/// `intel_engine_emit_ctx_wa`.
pub fn intel_engine_init_ctx_wa(i915: &mut drm_i915_private, gt: &intel_gt, class: u32, instance: u32,
                                mmio_base: u32, wal: &mut i915_wa_list) {
    wa_init_start(wal, "context", engine_name(class, instance));

    // Applies to all engines.
    gen12_ctx_gt_fake_wa_init(i915, gt, class, mmio_base, wal);

    if class == RENDER_CLASS {
        gen12_ctx_workarounds_init(wal);
    }

    wa_init_finish(wal);
}

/// `engine_fake_wa_init`: RING_CMD_CCTL points commands without a MOCS
/// field at the uncached entry.
fn engine_fake_wa_init(gt: &intel_gt, class: u32, mmio_base: u32, wal: &mut i915_wa_list) {
    let mocs_w = gt.mocs.uc_index as u32;
    let mut mocs_r = gt.mocs.uc_index as u32;
    if TGL_HAS_L3_CCS_READ != 0 && class == COMPUTE_CLASS {
        mocs_r = gt.mocs.wb_index as u32;
    }
    wa_masked_field_set(wal, RING_CMD_CCTL(mmio_base), CMD_CCTL_MOCS_MASK,
                        CMD_CCTL_MOCS_OVERRIDE(mocs_w, mocs_r));
}

/// `rcs_engine_wa_init`: the IS_TIGERLAKE blocks and the
/// GRAPHICS_VER >= 9 one.
fn rcs_engine_wa_init(wal: &mut i915_wa_list) {
    // Wa_1606700617, Wa_22010271021, Wa_14010826681
    wa_masked_en(wal, GEN9_CS_DEBUG_MODE1, FF_DOP_CLOCK_GATE_DISABLE);

    // Wa_1606931601
    wa_mcr_masked_en(wal, GEN8_ROW_CHICKEN2, GEN12_DISABLE_EARLY_READ);

    // Wa_1407928979, Wa_18011464164, Wa_22010931296
    // BIT(19), an unsigned long in i915_reg.h.
    wa_write_or(wal, GEN7_FF_THREAD_MODE, GEN12_FF_TESSELATION_DOP_GATE_DISABLE as u32);

    // Wa_1406941453
    wa_mcr_masked_en(wal, GEN10_SAMPLER_MODE, ENABLE_SMALLPL);

    // Wa_1409804808
    wa_mcr_masked_en(wal, GEN8_ROW_CHICKEN2, GEN12_PUSH_CONST_DEREF_HOLD_DIS);

    // Wa_14010229206
    wa_mcr_masked_en(wal, GEN9_ROW_CHICKEN4, GEN12_DISABLE_TDL_PUSH);

    // Wa_1607297627, all steppings.
    wa_masked_en(wal, RING_PSMI_CTL(RENDER_RING_BASE),
                 GEN12_WAIT_FOR_EVENT_POWER_DOWN_DISABLE | GEN8_RC_SEMA_IDLE_MSG_DISABLE);

    // Object-level preemption is left to userspace (FF_SLICE_CS_CHICKEN1
    // option 2).
    wa_masked_en(wal, GEN7_FF_SLICE_CS_CHICKEN1, GEN9_FFSC_PERCTX_PREEMPT_CTRL);
}

/// `xcs_engine_wa_init`: the KBL semaphore poll and the blitter
/// scheduling mode of `NEEDS_FASTCOLOR_BLT_WABB` (12.55-12.60) do not
/// apply.
fn xcs_engine_wa_init(_wal: &mut i915_wa_list) {}

/// `add_render_compute_tuning_settings`.
fn add_render_compute_tuning_settings(i915: &drm_i915_private, wal: &mut i915_wa_list) {
    if TGL_TUNING_THREAD_RR_AFTER_DEP != 0 {
        Line::new().s("MISSING_CASE tuning_thread_rr_after_dep").say();
    }
    if i915.graphics_ver() == 12 && i915.graphics_ver_full() < crate::i915_drv::ip_ver(12, 55) {
        wa_write_clr(wal, GEN8_GARBCNTL, GEN12_BUS_HASH_CTL_BIT_EXC);
    }
}

/// `general_render_compute_wa_init`: registers in the shared render
/// reset domain, on the first render/compute engine's list only.
fn general_render_compute_wa_init(i915: &drm_i915_private, wal: &mut i915_wa_list) {
    add_render_compute_tuning_settings(i915, wal);

    // Sampler border colours relative to DynamicStateBaseAddress.
    wa_mcr_masked_en(wal, GEN10_SAMPLER_MODE, GEN11_INDIRECT_STATE_BASE_ADDR_OVERRIDE);
}

/// `intel_engine_init_workarounds` (`engine_init_workarounds`);
/// `ccs_engine_wa_mode` is DG2's.
pub fn intel_engine_init_workarounds(i915: &mut drm_i915_private, gt: &intel_gt, class: u32, instance: u32,
                                     mmio_base: u32, wal: &mut i915_wa_list) {
    wa_init_start(wal, "engine", engine_name(class, instance));

    engine_fake_wa_init(gt, class, mmio_base, wal);

    if engine_flags(gt, class, instance) & I915_ENGINE_FIRST_RENDER_COMPUTE != 0 {
        general_render_compute_wa_init(i915, wal);
    }

    // ccs_engine_wa_init has no entries.
    if class == RENDER_CLASS {
        rcs_engine_wa_init(wal);
    } else if class != COMPUTE_CLASS {
        xcs_engine_wa_init(wal);
    }

    wa_init_finish(wal);
}

/// `intel_engine_apply_workarounds`.
pub fn intel_engine_apply_workarounds(i915: &mut drm_i915_private, gt: &intel_gt, wal: &i915_wa_list) {
    wa_list_apply(i915, gt, wal);
}

/// `whitelist_reg_ext`: the access flags travel in the register offset.
/// The slot and flag checks are GEM_DEBUG_WARN_ON, which compiles to
/// nothing without CONFIG_DRM_I915_DEBUG_GEM.
fn whitelist_reg_ext(wal: &mut i915_wa_list, reg: u32, flags: u32) {
    _wa_add(wal, &i915_wa { reg: reg | flags, ..Default::default() });
}

/// `whitelist_reg`.
fn whitelist_reg(wal: &mut i915_wa_list, reg: u32) {
    whitelist_reg_ext(wal, reg, RING_FORCE_TO_NONPRIV_ACCESS_RW);
}

/// `allow_read_ctx_timestamp`.
fn allow_read_ctx_timestamp(class: u32, mmio_base: u32, w: &mut i915_wa_list) {
    if class != RENDER_CLASS {
        whitelist_reg_ext(w, RING_CTX_TIMESTAMP(mmio_base), RING_FORCE_TO_NONPRIV_ACCESS_RD);
    }
}

/// `tgl_whitelist_build`.
fn tgl_whitelist_build(class: u32, mmio_base: u32, w: &mut i915_wa_list) {
    allow_read_ctx_timestamp(class, mmio_base, w);

    if class == RENDER_CLASS {
        // WaAllowPMDepthAndInvocationCountAccessFromUMD, Wa_1408556865:
        // PS_INVOCATION_COUNT, its UDW, PS_DEPTH_COUNT and its UDW.
        whitelist_reg_ext(w, PS_INVOCATION_COUNT,
                          RING_FORCE_TO_NONPRIV_ACCESS_RD | RING_FORCE_TO_NONPRIV_RANGE_4);

        // Wa_1808121037, Wa_1508744258
        whitelist_reg(w, GEN7_COMMON_SLICE_CHICKEN1);

        // Wa_1806527549
        whitelist_reg(w, HIZ_CHICKEN);

        // Needed by a recommended tuning setting.
        whitelist_reg(w, GEN11_COMMON_SLICE_CHICKEN3);
    }
}

/// `intel_engine_init_whitelist`: the primary GT, GRAPHICS_VER 12.
pub fn intel_engine_init_whitelist(_i915: &mut drm_i915_private, _gt: &intel_gt, class: u32, instance: u32,
                                   mmio_base: u32, wal: &mut i915_wa_list) {
    wa_init_start(wal, "whitelist", engine_name(class, instance));
    tgl_whitelist_build(class, mmio_base, wal);
    wa_init_finish(wal);
}

/// `intel_engine_apply_whitelist`: one RING_FORCE_TO_NONPRIV slot per
/// entry, the rest pointed at RING_NOPID.
pub fn intel_engine_apply_whitelist(i915: &mut drm_i915_private, _gt: &intel_gt, mmio_base: u32,
                                    wal: &i915_wa_list) {
    if wal.count == 0 {
        return;
    }
    let mut i = 0;
    while i < wal.count as u32 {
        i915.uncore.intel_uncore_write(RING_FORCE_TO_NONPRIV(mmio_base, i), wal.list[i as usize].reg);
        i += 1;
    }
    // And clear the rest just in case of garbage.
    while i < RING_MAX_NONPRIV_SLOTS {
        i915.uncore.intel_uncore_write(RING_FORCE_TO_NONPRIV(mmio_base, i), RING_NOPID(mmio_base));
        i += 1;
    }
}
