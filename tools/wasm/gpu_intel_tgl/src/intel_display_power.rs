//! display/intel_display_power.c, intel_display_power_map.c and
//! intel_display_power_well.c (Linux 6.18.55) for a Tiger Lake display:
//! the power well table, the domain reference counts and the DC states.
//!
//! The wells come from `tgl_power_wells`, flattened by gen_regs.py into
//! `tables::tgl_power_well_insts`; a well refers to its row by index.
//! `intel_display_rpm_get` and the async-put machinery have nothing to do
//! here: there is no runtime PM and nothing is put asynchronously.

use crate::i915_drv::drm_i915_private;
use crate::intel_de::*;
use crate::linux::{self, Line};
use crate::regs::*;
use crate::tables;

pub const DOMAIN_WORDS: usize = (POWER_DOMAIN_NUM as usize).div_ceil(64);
const WELL_COUNT: usize = tables::tgl_power_well_insts.len();

/// `struct i915_power_well_instance` and its `struct i915_power_well_desc`,
/// one row per instance (see tables.list).
#[derive(Clone, Copy)]
pub struct power_well_inst {
    pub desc: u32,
    pub instance_idx: u32,
    pub name: &'static [u8],
    pub id: u32,
    pub hsw_idx: u32,
    pub ops: power_well_inst_ops,
    pub irq_pipe_mask: u32,
    pub always_on: u32,
    pub fixed_enable_delay: u32,
    pub has_vga: u32,
    pub has_fuses: u32,
    pub is_tc_tbt: u32,
    pub enable_timeout: u32,
    /// 0: `I915_PW_DOMAINS_NONE`, 1: `I915_PW_DOMAINS_ALL`, 2: a list.
    pub domain_list: u32,
}

/// The `struct i915_power_well_ops` tables `tgl_power_wells` uses.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum power_well_inst_ops {
    i9xx_always_on_power_well_ops,
    hsw_power_well_ops,
    gen9_dc_off_power_well_ops,
    icl_ddi_power_well_ops,
    icl_aux_power_well_ops,
    tgl_tc_cold_off_ops,
}

/// One entry of an instance's `domain_list`.
#[derive(Clone, Copy)]
pub struct power_well_domain {
    pub well: u32,
    pub domain: u32,
}

/// `struct i915_power_well`; `inst` indexes `tgl_power_well_insts`.
#[derive(Clone, Copy, Default)]
pub struct i915_power_well {
    pub inst: usize,
    pub domains: [u64; DOMAIN_WORDS],
    pub count: i32,
    pub hw_enabled: bool,
}

/// `struct i915_power_domains`.
#[derive(Clone, Copy)]
pub struct i915_power_domains {
    pub initializing: bool,
    pub allowed_dc_mask: u32,
    pub target_dc_state: u32,
    pub dc_state: u32,
    pub power_well_count: usize,
    pub power_wells: [i915_power_well; WELL_COUNT],
    pub domain_use_count: [i32; POWER_DOMAIN_NUM as usize],
    pub init_wakeref: bool,
    pub disable_wakeref: bool,
}

impl Default for i915_power_domains {
    fn default() -> Self {
        i915_power_domains {
            initializing: false,
            allowed_dc_mask: 0,
            target_dc_state: 0,
            dc_state: 0,
            power_well_count: 0,
            power_wells: [i915_power_well::default(); WELL_COUNT],
            domain_use_count: [0; POWER_DOMAIN_NUM as usize],
            init_wakeref: false,
            disable_wakeref: false,
        }
    }
}

/// `struct intel_display_params`, the two the power code reads.
#[derive(Clone, Copy)]
pub struct intel_display_params {
    pub enable_dc: i32,
    pub disable_power_well: i32,
}

impl Default for intel_display_params {
    fn default() -> Self {
        intel_display_params { enable_dc: ENABLE_DC_DEFAULT, disable_power_well: DISABLE_POWER_WELL_DEFAULT }
    }
}

fn inst(pw: &i915_power_well) -> &'static power_well_inst {
    &tables::tgl_power_well_insts[pw.inst]
}

fn test_bit(bits: &[u64; DOMAIN_WORDS], n: u32) -> bool {
    bits[(n / 64) as usize] & (1 << (n % 64)) != 0
}

fn set_bit(bits: &mut [u64; DOMAIN_WORDS], n: u32) {
    bits[(n / 64) as usize] |= 1 << (n % 64);
}

fn name(pw: &i915_power_well) -> &'static str {
    core::str::from_utf8(inst(pw).name).unwrap_or("?")
}

/// `intel_power_well_name`.
pub fn power_well_name(i915: &drm_i915_private, w: usize) -> &'static str {
    name(&i915.display.power.power_wells[w])
}

// ── intel_display_power_map.c ──────────────────────────────────────────

/// `init_power_well_domains`.
fn init_power_well_domains(idx: usize, power_well: &mut i915_power_well) {
    let i = &tables::tgl_power_well_insts[idx];
    if i.domain_list == 0 {
        return;
    }
    if i.domain_list == 1 {
        for d in 0..POWER_DOMAIN_NUM {
            set_bit(&mut power_well.domains, d);
        }
        return;
    }
    for e in tables::tgl_power_well_domains.iter().filter(|e| e.well as usize == idx) {
        set_bit(&mut power_well.domains, e.domain);
    }
}

/// `__set_power_wells` for `tgl_power_wells`.
fn __set_power_wells(power_domains: &mut i915_power_domains) {
    let mut power_well_ids = 0u64;
    power_domains.power_well_count = WELL_COUNT;
    for plt_idx in 0..WELL_COUNT {
        let pw = &mut power_domains.power_wells[plt_idx];
        *pw = i915_power_well { inst: plt_idx, ..Default::default() };
        init_power_well_domains(plt_idx, pw);
        let id = tables::tgl_power_well_insts[plt_idx].id;
        if id == DISP_PW_ID_NONE {
            continue;
        }
        if id >= 64 || power_well_ids & (1 << id) != 0 {
            linux::drm_err("power well id out of range or duplicated");
        }
        power_well_ids |= 1 << id;
    }
}

/// `intel_display_power_map_init`: display version 12, not DG1/RKL/ADL-S.
fn intel_display_power_map_init(i915: &drm_i915_private, power_domains: &mut i915_power_domains) {
    if !i915.display.has_display {
        power_domains.power_well_count = 0;
        return;
    }
    __set_power_wells(power_domains);
}

// ── intel_display_power.c ──────────────────────────────────────────────

/// `sanitize_target_dc_state`.
fn sanitize_target_dc_state(power_domains: &i915_power_domains, mut target_dc_state: u32) -> u32 {
    let states = [DC_STATE_EN_UPTO_DC6, DC_STATE_EN_UPTO_DC5, DC_STATE_EN_DC3CO, DC_STATE_DISABLE];
    for i in 0..states.len() - 1 {
        if target_dc_state != states[i] {
            continue;
        }
        if power_domains.allowed_dc_mask & target_dc_state != 0 {
            break;
        }
        target_dc_state = states[i + 1];
    }
    target_dc_state
}

/// `sanitize_disable_power_well_option`.
fn sanitize_disable_power_well_option(disable_power_well: i32) -> i32 {
    if disable_power_well >= 0 {
        return (disable_power_well != 0) as i32;
    }
    1
}

/// `get_allowed_dc_mask` for display version 12 (not DG1/DG2).
fn get_allowed_dc_mask(i915: &drm_i915_private, params: &intel_display_params, enable_dc: i32) -> u32 {
    if !i915.display.has_display {
        return 0;
    }
    let mut max_dc = 4;
    // DC9 has its own flow, independent of the DMC, and suspend needs it.
    let mut mask = DC_STATE_EN_DC9;

    if params.disable_power_well == 0 {
        max_dc = 0;
    }

    let requested_dc = if enable_dc >= 0 && enable_dc <= max_dc {
        enable_dc
    } else if enable_dc == -1 {
        max_dc
    } else if enable_dc > max_dc && enable_dc <= 4 {
        max_dc
    } else {
        Line::new().s("Unexpected value for enable_dc (").dec(enable_dc as u64).s(")").say();
        max_dc
    };

    match requested_dc {
        4 => mask |= DC_STATE_EN_DC3CO | DC_STATE_EN_UPTO_DC6,
        3 => mask |= DC_STATE_EN_DC3CO | DC_STATE_EN_UPTO_DC5,
        2 => mask |= DC_STATE_EN_UPTO_DC6,
        1 => mask |= DC_STATE_EN_UPTO_DC5,
        _ => {}
    }
    mask
}

/// `intel_power_domains_init`.
pub fn intel_power_domains_init(i915: &mut drm_i915_private) {
    let mut params = i915.display.params;
    params.disable_power_well = sanitize_disable_power_well_option(params.disable_power_well);
    i915.display.params = params;

    let mut pd = i915_power_domains::default();
    pd.allowed_dc_mask = get_allowed_dc_mask(i915, &params, params.enable_dc);
    pd.target_dc_state = sanitize_target_dc_state(&pd, DC_STATE_EN_UPTO_DC6);
    intel_display_power_map_init(i915, &mut pd);
    i915.display.power = pd;
}

/// `lookup_power_well`.
pub fn lookup_power_well(i915: &drm_i915_private, power_well_id: u32) -> usize {
    let pd = &i915.display.power;
    for i in 0..pd.power_well_count {
        if inst(&pd.power_wells[i]).id == power_well_id {
            return i;
        }
    }
    Line::new().s("warning: power well ").dec(power_well_id as u64).s(" not defined for this platform").say();
    0
}

/// `intel_power_well_enable`.
pub fn intel_power_well_enable(i915: &mut drm_i915_private, w: usize) {
    power_well_ops_enable(i915, w);
    i915.display.power.power_wells[w].hw_enabled = true;
}

/// `intel_power_well_disable`.
pub fn intel_power_well_disable(i915: &mut drm_i915_private, w: usize) {
    i915.display.power.power_wells[w].hw_enabled = false;
    power_well_ops_disable(i915, w);
}

/// `intel_power_well_sync_hw`.
pub fn intel_power_well_sync_hw(i915: &mut drm_i915_private, w: usize) {
    power_well_ops_sync_hw(i915, w);
    let en = power_well_ops_is_enabled(i915, w);
    i915.display.power.power_wells[w].hw_enabled = en;
}

/// `intel_power_well_get`.
pub fn intel_power_well_get(i915: &mut drm_i915_private, w: usize) {
    let c = i915.display.power.power_wells[w].count;
    i915.display.power.power_wells[w].count = c + 1;
    if c == 0 {
        intel_power_well_enable(i915, w);
    }
}

/// `intel_power_well_put`.
pub fn intel_power_well_put(i915: &mut drm_i915_private, w: usize) {
    if i915.display.power.power_wells[w].count == 0 {
        Line::new().s("warning: use count on power well ").s(name(&i915.display.power.power_wells[w]))
            .s(" is already zero").say();
    }
    i915.display.power.power_wells[w].count -= 1;
    if i915.display.power.power_wells[w].count == 0 {
        intel_power_well_disable(i915, w);
    }
}

/// `intel_power_well_is_enabled`.
pub fn intel_power_well_is_enabled(i915: &mut drm_i915_private, w: usize) -> bool {
    power_well_ops_is_enabled(i915, w)
}

/// `__intel_display_power_get_domain`.
fn __intel_display_power_get_domain(i915: &mut drm_i915_private, domain: u32) {
    for w in 0..i915.display.power.power_well_count {
        if test_bit(&i915.display.power.power_wells[w].domains, domain) {
            intel_power_well_get(i915, w);
        }
    }
    i915.display.power.domain_use_count[domain as usize] += 1;
}

/// `intel_display_power_get`.
pub fn intel_display_power_get(i915: &mut drm_i915_private, domain: u32) {
    __intel_display_power_get_domain(i915, domain);
}

/// `intel_power_domains_sync_hw`.
fn intel_power_domains_sync_hw(i915: &mut drm_i915_private) {
    for w in 0..i915.display.power.power_well_count {
        intel_power_well_sync_hw(i915, w);
    }
}

/// `intel_pch_reset_handshake` (not Ivy Bridge, display version below 14).
fn intel_pch_reset_handshake(i915: &mut drm_i915_private, enable: bool) {
    let reg = HSW_NDE_RSTWRN_OPT;
    let reset_bits = RESET_PCH_HANDSHAKE_ENABLE;
    intel_de_rmw(i915, reg, reset_bits, if enable { reset_bits } else { 0 });
}

/// `icl_display_core_init`, the steps before the combo PHYs: DC states
/// off, Wa_14011294188, the PCH reset handshake.
fn icl_display_core_init(i915: &mut drm_i915_private, pch_type: i32, _resume: bool) {
    gen9_set_dc_state(i915, DC_STATE_DISABLE);

    // Wa_14011294188:ehl,jsl,tgl,rkl,adl-s
    if pch_type >= PCH_TGP as i32 && pch_type < PCH_DG1 as i32 {
        intel_de_rmw(i915, SOUTH_DSPCLK_GATE_D, 0, PCH_DPMGUNIT_CLOCK_GATE_DISABLE);
    }

    // 1. Enable PCH reset handshake.
    intel_pch_reset_handshake(i915, pch_type != PCH_NOP);

    if !i915.display.has_display {
        return;
    }
}

/// `intel_power_domains_init_hw`, display version 11 and up, up to the
/// point that is ported.
pub fn intel_power_domains_init_hw(i915: &mut drm_i915_private, pch_type: i32, resume: bool) {
    i915.display.power.initializing = true;
    icl_display_core_init(i915, pch_type, resume);
    i915.display.power.initializing = false;
}

// ── intel_display_power_well.c ─────────────────────────────────────────

/// `struct i915_power_well_regs`.
#[derive(Clone, Copy)]
struct i915_power_well_regs {
    bios: u32,
    driver: u32,
    kvmr: u32,
    debug: u32,
}

const hsw_power_well_regs: i915_power_well_regs = i915_power_well_regs {
    bios: HSW_PWR_WELL_REGS_BIOS, driver: HSW_PWR_WELL_REGS_DRIVER,
    kvmr: HSW_PWR_WELL_REGS_KVMR, debug: HSW_PWR_WELL_REGS_DEBUG,
};
const icl_aux_power_well_regs: i915_power_well_regs = i915_power_well_regs {
    bios: ICL_AUX_PW_REGS_BIOS, driver: ICL_AUX_PW_REGS_DRIVER,
    kvmr: ICL_AUX_PW_REGS_KVMR, debug: ICL_AUX_PW_REGS_DEBUG,
};
const icl_ddi_power_well_regs: i915_power_well_regs = i915_power_well_regs {
    bios: ICL_DDI_PW_REGS_BIOS, driver: ICL_DDI_PW_REGS_DRIVER,
    kvmr: ICL_DDI_PW_REGS_KVMR, debug: ICL_DDI_PW_REGS_DEBUG,
};

fn regs_of(i: &power_well_inst) -> i915_power_well_regs {
    match i.ops {
        power_well_inst_ops::icl_aux_power_well_ops => icl_aux_power_well_regs,
        power_well_inst_ops::icl_ddi_power_well_ops => icl_ddi_power_well_regs,
        _ => hsw_power_well_regs,
    }
}

fn power_well_ops_sync_hw(i915: &mut drm_i915_private, w: usize) {
    match inst(&i915.display.power.power_wells[w]).ops {
        power_well_inst_ops::hsw_power_well_ops
        | power_well_inst_ops::icl_aux_power_well_ops
        | power_well_inst_ops::icl_ddi_power_well_ops => hsw_power_well_sync_hw(i915, w),
        power_well_inst_ops::tgl_tc_cold_off_ops => tgl_tc_cold_off_power_well_sync_hw(i915, w),
        power_well_inst_ops::i9xx_always_on_power_well_ops
        | power_well_inst_ops::gen9_dc_off_power_well_ops => {}
    }
}

fn power_well_ops_enable(i915: &mut drm_i915_private, w: usize) {
    match inst(&i915.display.power.power_wells[w]).ops {
        power_well_inst_ops::i9xx_always_on_power_well_ops => {}
        power_well_inst_ops::hsw_power_well_ops
        | power_well_inst_ops::icl_ddi_power_well_ops => hsw_power_well_enable(i915, w),
        power_well_inst_ops::icl_aux_power_well_ops => icl_aux_power_well_enable(i915, w),
        power_well_inst_ops::gen9_dc_off_power_well_ops => gen9_dc_off_power_well_enable(i915),
        power_well_inst_ops::tgl_tc_cold_off_ops => tgl_tc_cold_request(i915, true),
    }
}

fn power_well_ops_disable(i915: &mut drm_i915_private, w: usize) {
    match inst(&i915.display.power.power_wells[w]).ops {
        power_well_inst_ops::i9xx_always_on_power_well_ops => {}
        power_well_inst_ops::hsw_power_well_ops
        | power_well_inst_ops::icl_ddi_power_well_ops
        | power_well_inst_ops::icl_aux_power_well_ops => hsw_power_well_disable(i915, w),
        power_well_inst_ops::gen9_dc_off_power_well_ops => gen9_dc_off_power_well_disable(i915),
        power_well_inst_ops::tgl_tc_cold_off_ops => tgl_tc_cold_request(i915, false),
    }
}

fn power_well_ops_is_enabled(i915: &mut drm_i915_private, w: usize) -> bool {
    match inst(&i915.display.power.power_wells[w]).ops {
        power_well_inst_ops::i9xx_always_on_power_well_ops => true,
        power_well_inst_ops::hsw_power_well_ops
        | power_well_inst_ops::icl_ddi_power_well_ops
        | power_well_inst_ops::icl_aux_power_well_ops => hsw_power_well_enabled(i915, w),
        power_well_inst_ops::gen9_dc_off_power_well_ops => gen9_dc_off_power_well_enabled(i915),
        // There is no way to read it from PCODE.
        power_well_inst_ops::tgl_tc_cold_off_ops => i915.display.power.power_wells[w].count != 0,
    }
}

/// `pw_idx_to_pg` (display version 11 and up).
fn pw_idx_to_pg(pw_idx: u32) -> u32 {
    pw_idx - ICL_PW_CTL_IDX_PW_1 + SKL_PG1
}

/// `hsw_power_well_post_enable`.
fn hsw_power_well_post_enable(i915: &mut drm_i915_private, irq_pipe_mask: u32, has_vga: bool) {
    if has_vga {
        intel_vga_reset_io_mem();
    }
    if irq_pipe_mask != 0 {
        gen8_irq_power_well_post_enable(i915, irq_pipe_mask);
    }
}

/// `hsw_power_well_pre_disable`.
fn hsw_power_well_pre_disable(i915: &mut drm_i915_private, irq_pipe_mask: u32) {
    if irq_pipe_mask != 0 {
        gen8_irq_power_well_pre_disable(i915, irq_pipe_mask);
    }
}

/// `intel_vga_reset_io_mem`: touch the VGA MSR so 0x3d5 stops raising
/// unclaimed-register errors after the VGA well came back.
fn intel_vga_reset_io_mem() {
    if npk_sys::vga_msr_touch() != 0 {
        linux::say("warning: VGA MSR touch refused");
    }
}

/// `gen8_irq_power_well_post_enable`: nothing to restore while display
/// interrupts are not installed.
fn gen8_irq_power_well_post_enable(i915: &mut drm_i915_private, _pipe_mask: u32) {
    if !i915.irqs_enabled {
        return;
    }
    linux::say("warning: gen8_irq_power_well_post_enable with interrupts on is not ported");
}

/// `gen8_irq_power_well_pre_disable`.
fn gen8_irq_power_well_pre_disable(i915: &mut drm_i915_private, _pipe_mask: u32) {
    if !i915.irqs_enabled {
        return;
    }
    linux::say("warning: gen8_irq_power_well_pre_disable with interrupts on is not ported");
}

/// `hsw_wait_for_power_well_enable` (not DG2).
fn hsw_wait_for_power_well_enable(i915: &mut drm_i915_private, w: usize, timeout_expected: bool) {
    let pw = i915.display.power.power_wells[w];
    let i = inst(&pw);
    let regs = regs_of(i);
    let timeout = if i.enable_timeout != 0 { i.enable_timeout } else { 1 };

    // Timeout for PW1:10 us, AUX:not specified, other PWs:20 us.
    if intel_de_wait_for_set(i915, regs.driver, HSW_PWR_WELL_CTL_STATE(i.hsw_idx), timeout).is_err() {
        Line::new().s(name(&pw)).s(" power well enable timeout").say();
        if !timeout_expected {
            linux::say("warning: power well enable timed out");
        }
    }
}

/// `hsw_power_well_requesters`.
fn hsw_power_well_requesters(i915: &mut drm_i915_private, regs: &i915_power_well_regs, pw_idx: u32) -> u32 {
    let req_mask = HSW_PWR_WELL_CTL_REQ(pw_idx);
    let mut ret = if intel_de_read(i915, regs.bios) & req_mask != 0 { 1 } else { 0 };
    ret |= if intel_de_read(i915, regs.driver) & req_mask != 0 { 2 } else { 0 };
    if regs.kvmr != 0 {
        ret |= if intel_de_read(i915, regs.kvmr) & req_mask != 0 { 4 } else { 0 };
    }
    ret |= if intel_de_read(i915, regs.debug) & req_mask != 0 { 8 } else { 0 };
    ret
}

/// `hsw_wait_for_power_well_disable`.
fn hsw_wait_for_power_well_disable(i915: &mut drm_i915_private, w: usize) {
    let pw = i915.display.power.power_wells[w];
    let i = inst(&pw);
    let regs = regs_of(i);
    let pw_idx = i.hsw_idx;

    // Bspec does not require waiting; a KVMR or DMC request keeps a well
    // on, so the wait is skipped when any request bit is set.
    let mut reqs = hsw_power_well_requesters(i915, &regs, pw_idx);
    let ret = intel_de_wait_for_clear(i915, regs.driver, HSW_PWR_WELL_CTL_STATE(pw_idx),
                                      if reqs != 0 { 0 } else { 1 });
    if ret.is_ok() {
        return;
    }
    if reqs == 0 {
        reqs = hsw_power_well_requesters(i915, &regs, pw_idx);
    }
    Line::new().s(name(&pw)).s(" forced on (bios:").dec((reqs & 1 != 0) as u64)
        .s(" driver:").dec((reqs & 2 != 0) as u64).s(" kvmr:").dec((reqs & 4 != 0) as u64)
        .s(" debug:").dec((reqs & 8 != 0) as u64).s(")").say();
}

/// `gen9_wait_for_power_well_fuses`.
fn gen9_wait_for_power_well_fuses(i915: &mut drm_i915_private, pg: u32) {
    // Timeout 5us for PG#0, for other PGs 1us
    if intel_de_wait_for_set(i915, SKL_FUSE_STATUS, SKL_FUSE_PG_DIST_STATUS(pg), 1).is_err() {
        Line::new().s("warning: power gate ").dec(pg as u64).s(" fuse distribution timed out").say();
    }
}

/// `hsw_power_well_enable` (not Alder Lake P).
fn hsw_power_well_enable(i915: &mut drm_i915_private, w: usize) {
    let pw = i915.display.power.power_wells[w];
    let i = inst(&pw);
    let regs = regs_of(i);
    let pw_idx = i.hsw_idx;

    if i.has_fuses != 0 {
        let pg = pw_idx_to_pg(pw_idx);
        // PW1 waits for PG0's fuses before and its own after enabling;
        // the other wells only for their own, after.
        if pg == SKL_PG1 {
            gen9_wait_for_power_well_fuses(i915, SKL_PG0);
        }
    }

    intel_de_rmw(i915, regs.driver, 0, HSW_PWR_WELL_CTL_REQ(pw_idx));

    hsw_wait_for_power_well_enable(i915, w, false);

    if i.has_fuses != 0 {
        let pg = pw_idx_to_pg(pw_idx);
        gen9_wait_for_power_well_fuses(i915, pg);
    }

    hsw_power_well_post_enable(i915, i.irq_pipe_mask, i.has_vga != 0);
}

/// `hsw_power_well_disable`.
fn hsw_power_well_disable(i915: &mut drm_i915_private, w: usize) {
    let pw = i915.display.power.power_wells[w];
    let i = inst(&pw);
    let regs = regs_of(i);
    let pw_idx = i.hsw_idx;

    hsw_power_well_pre_disable(i915, i.irq_pipe_mask);

    intel_de_rmw(i915, regs.driver, HSW_PWR_WELL_CTL_REQ(pw_idx), 0);
    hsw_wait_for_power_well_disable(i915, w);
}

/// `icl_aux_power_well_enable`. The PHY comes from the digital port that
/// owns the AUX channel; no encoder is registered at this stage, so
/// `aux_ch_to_digital_port` finds none, the PHY is `PHY_NONE` and the
/// well takes the plain path, as in Linux before the outputs are set up.
fn icl_aux_power_well_enable(i915: &mut drm_i915_private, w: usize) {
    hsw_power_well_enable(i915, w);
}

/// `hsw_power_well_enabled` (display version 12).
fn hsw_power_well_enabled(i915: &mut drm_i915_private, w: usize) -> bool {
    let pw = i915.display.power.power_wells[w];
    let i = inst(&pw);
    let regs = regs_of(i);
    let mask = HSW_PWR_WELL_CTL_REQ(i.hsw_idx) | HSW_PWR_WELL_CTL_STATE(i.hsw_idx);
    let val = intel_de_read(i915, regs.driver);
    val & mask == mask
}

/// `hsw_power_well_sync_hw`: take over the request bit if the BIOS set it.
fn hsw_power_well_sync_hw(i915: &mut drm_i915_private, w: usize) {
    let pw = i915.display.power.power_wells[w];
    let i = inst(&pw);
    let regs = regs_of(i);
    let mask = HSW_PWR_WELL_CTL_REQ(i.hsw_idx);
    let bios_req = intel_de_read(i915, regs.bios);

    if bios_req & mask != 0 {
        let drv_req = intel_de_read(i915, regs.driver);
        if drv_req & mask == 0 {
            intel_de_write(i915, regs.driver, drv_req | mask);
        }
        intel_de_write(i915, regs.bios, bios_req & !mask);
    }
}

/// `gen9_write_dc_state`: a DC6 disable sometimes does not stick, so the
/// value is rewritten until it reads back stable.
fn gen9_write_dc_state(i915: &mut drm_i915_private, state: u32) {
    let mut rewrites = 0;
    let mut rereads = 0;
    let mut v;

    intel_de_write(i915, DC_STATE_EN, state);

    loop {
        v = intel_de_read(i915, DC_STATE_EN);
        if v != state {
            intel_de_write(i915, DC_STATE_EN, state);
            rewrites += 1;
            rereads = 0;
        } else {
            let r = rereads;
            rereads += 1;
            if r > 5 {
                break;
            }
        }
        if rewrites >= 100 {
            break;
        }
    }

    if v != state {
        Line::new().s("Writing dc state to ").hex(state).s(" failed, now ").hex(v).say();
    }
    if rewrites > 1 {
        Line::new().s("Rewrote dc state to ").hex(state).s(" ").dec(rewrites as u64).s(" times").say();
    }
}

/// `gen9_dc_mask` (display version 12).
fn gen9_dc_mask() -> u32 {
    DC_STATE_EN_UPTO_DC5 | DC_STATE_EN_DC3CO | DC_STATE_EN_UPTO_DC6 | DC_STATE_EN_DC9
}

/// `gen9_sanitize_dc_state`.
pub fn gen9_sanitize_dc_state(i915: &mut drm_i915_private) {
    if !i915.display.has_display {
        return;
    }
    let val = intel_de_read(i915, DC_STATE_EN) & gen9_dc_mask();
    i915.display.power.dc_state = val;
}

/// `gen9_set_dc_state`. `intel_dmc_update_dc6_allowed_count` only acts from
/// display version 14 on.
pub fn gen9_set_dc_state(i915: &mut drm_i915_private, mut state: u32) {
    if !i915.display.has_display {
        return;
    }
    let pd = i915.display.power;
    if state & !pd.allowed_dc_mask != 0 {
        linux::say("warning: DC state outside the allowed mask");
        state &= pd.allowed_dc_mask;
    }
    if !pd.initializing {
        linux::say("warning: intel_psr_notify_dc5_dc6 is not ported");
    }

    let mut val = intel_de_read(i915, DC_STATE_EN);
    let mask = gen9_dc_mask();
    Line::new().s("Setting DC state from ").hex(val & mask).s(" to ").hex(state).say();

    // Check if DMC is ignoring our DC state requests
    if val & mask != pd.dc_state {
        Line::new().s("DC state mismatch (").hex(pd.dc_state).s(" -> ").hex(val & mask).s(")").say();
    }

    val &= !mask;
    val |= state;

    gen9_write_dc_state(i915, val);

    i915.display.power.dc_state = val & mask;
}

/// `gen9_dc_off_power_well_enabled`.
fn gen9_dc_off_power_well_enabled(i915: &mut drm_i915_private) -> bool {
    intel_de_read(i915, DC_STATE_EN) & DC_STATE_EN_DC3CO == 0
        && intel_de_read(i915, DC_STATE_EN) & DC_STATE_EN_UPTO_DC5_DC6_MASK == 0
}

/// `gen9_dc_off_power_well_enable` → `gen9_disable_dc_states`.
fn gen9_dc_off_power_well_enable(_i915: &mut drm_i915_private) {
    linux::say("warning: gen9_disable_dc_states needs the cdclk and dbuf state, not ported yet");
}

/// `gen9_dc_off_power_well_disable`: without a DMC payload the DC states
/// stay off.
fn gen9_dc_off_power_well_disable(i915: &mut drm_i915_private) {
    if !i915.display.dmc_has_payload {
        return;
    }
    linux::say("warning: DC state enable with a DMC payload is not ported");
}

/// `tgl_tc_cold_request`.
fn tgl_tc_cold_request(i915: &mut drm_i915_private, block: bool) {
    let mut tries = 0u8;
    let mut ret: Result<(), i32>;

    loop {
        let mut low_val = if block {
            TGL_PCODE_EXIT_TCCOLD_DATA_L_BLOCK_REQ
        } else {
            TGL_PCODE_EXIT_TCCOLD_DATA_L_UNBLOCK_REQ
        };
        let mut high_val = 0u32;

        // The spec times the request out after 200 us; the mailbox read
        // waits up to 500 us.
        ret = crate::intel_pcode::snb_pcode_read(i915, TGL_PCODE_TCCOLD, &mut low_val, Some(&mut high_val));
        if ret.is_ok() {
            if block && low_val & TGL_PCODE_EXIT_TCCOLD_DATA_L_EXIT_FAILED != 0 {
                ret = Err(-(EIO as i32));
            } else {
                break;
            }
        }

        tries += 1;
        if tries == 3 {
            break;
        }
        linux::udelay(1000);
    }

    if ret.is_err() {
        Line::new().s("TC cold ").s(if block { "" } else { "un" }).s("block failed").say();
    }
}

/// `tgl_tc_cold_off_power_well_sync_hw`.
fn tgl_tc_cold_off_power_well_sync_hw(i915: &mut drm_i915_private, w: usize) {
    if i915.display.power.power_wells[w].count > 0 {
        tgl_tc_cold_request(i915, true);
    } else {
        tgl_tc_cold_request(i915, false);
    }
}
