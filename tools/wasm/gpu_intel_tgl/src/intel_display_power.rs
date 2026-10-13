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

/// `__intel_display_power_is_enabled`: no runtime PM, so never suspended.
fn __intel_display_power_is_enabled(i915: &drm_i915_private, domain: u32) -> bool {
    let pd = &i915.display.power;
    for w in (0..pd.power_well_count).rev() {
        let pw = &pd.power_wells[w];
        if !test_bit(&pw.domains, domain) {
            continue;
        }
        if inst(pw).always_on != 0 {
            continue;
        }
        if !pw.hw_enabled {
            return false;
        }
    }
    true
}

/// `intel_display_power_is_enabled`.
pub fn intel_display_power_is_enabled(i915: &drm_i915_private, domain: u32) -> bool {
    __intel_display_power_is_enabled(i915, domain)
}

/// `intel_display_power_get_if_enabled`: true when a reference was taken.
pub fn intel_display_power_get_if_enabled(i915: &mut drm_i915_private, domain: u32) -> bool {
    if __intel_display_power_is_enabled(i915, domain) {
        __intel_display_power_get_domain(i915, domain);
        true
    } else {
        false
    }
}

/// `__intel_display_power_put_domain`.
fn __intel_display_power_put_domain(i915: &mut drm_i915_private, domain: u32) {
    if i915.display.power.domain_use_count[domain as usize] == 0 {
        Line::new().s("warning: use count on domain ").dec(domain as u64).s(" is already zero").say();
    }
    i915.display.power.domain_use_count[domain as usize] -= 1;

    for w in (0..i915.display.power.power_well_count).rev() {
        if test_bit(&i915.display.power.power_wells[w].domains, domain) {
            intel_power_well_put(i915, w);
        }
    }
}

/// `intel_display_power_put` (`intel_display_power_put_unchecked` without
/// the runtime PM debug option).
pub fn intel_display_power_put(i915: &mut drm_i915_private, domain: u32) {
    __intel_display_power_put_domain(i915, domain);
}

/// `struct intel_ddi_port_domains`.
#[derive(Clone, Copy)]
pub struct intel_ddi_port_domains {
    pub port_start: u32,
    pub port_end: u32,
    pub aux_ch_start: u32,
    pub aux_ch_end: u32,
    pub ddi_lanes: u32,
    pub ddi_io: u32,
    pub aux_io: u32,
    pub aux_legacy_usbc: u32,
    pub aux_tbt: u32,
}

/// `intel_port_domains_for_port` (display version 12: `d12_port_domains`).
fn intel_port_domains_for_port(port: u32) -> Option<&'static intel_ddi_port_domains> {
    tables::d12_port_domains.iter().find(|d| port >= d.port_start && port <= d.port_end)
}

/// `intel_port_domains_for_aux_ch`.
fn intel_port_domains_for_aux_ch(aux_ch: u32) -> Option<&'static intel_ddi_port_domains> {
    tables::d12_port_domains.iter().find(|d| aux_ch >= d.aux_ch_start && aux_ch <= d.aux_ch_end)
}

/// `intel_display_power_ddi_io_domain`.
pub fn intel_display_power_ddi_io_domain(port: u32) -> u32 {
    match intel_port_domains_for_port(port) {
        Some(d) if d.ddi_io != POWER_DOMAIN_INVALID => d.ddi_io + (port - d.port_start),
        _ => {
            linux::say("warning: no DDI IO power domain for port");
            POWER_DOMAIN_PORT_DDI_IO_A
        }
    }
}

/// `intel_display_power_ddi_lanes_domain`.
pub fn intel_display_power_ddi_lanes_domain(port: u32) -> u32 {
    match intel_port_domains_for_port(port) {
        Some(d) if d.ddi_lanes != POWER_DOMAIN_INVALID => d.ddi_lanes + (port - d.port_start),
        _ => {
            linux::say("warning: no DDI lanes power domain for port");
            POWER_DOMAIN_PORT_DDI_LANES_A
        }
    }
}

/// `intel_display_power_aux_io_domain`.
pub fn intel_display_power_aux_io_domain(aux_ch: u32) -> u32 {
    match intel_port_domains_for_aux_ch(aux_ch) {
        Some(d) if d.aux_io != POWER_DOMAIN_INVALID => d.aux_io + (aux_ch - d.aux_ch_start),
        _ => {
            linux::say("warning: no AUX IO power domain for AUX channel");
            POWER_DOMAIN_AUX_IO_A
        }
    }
}

/// `intel_display_power_legacy_aux_domain`.
pub fn intel_display_power_legacy_aux_domain(aux_ch: u32) -> u32 {
    match intel_port_domains_for_aux_ch(aux_ch) {
        Some(d) if d.aux_legacy_usbc != POWER_DOMAIN_INVALID => d.aux_legacy_usbc + (aux_ch - d.aux_ch_start),
        _ => {
            linux::say("warning: no legacy AUX power domain for AUX channel");
            POWER_DOMAIN_AUX_A
        }
    }
}

/// `intel_display_power_tbt_aux_domain`.
pub fn intel_display_power_tbt_aux_domain(aux_ch: u32) -> u32 {
    match intel_port_domains_for_aux_ch(aux_ch) {
        Some(d) if d.aux_tbt != POWER_DOMAIN_INVALID => d.aux_tbt + (aux_ch - d.aux_ch_start),
        _ => {
            linux::say("warning: no TBT AUX power domain for AUX channel");
            POWER_DOMAIN_AUX_TBT1
        }
    }
}

/// `intel_display_power_put_async`. Linux delays the release by a work
/// item so a well is not cycled between back-to-back users; there is no
/// work queue here, so the reference is dropped at once.
pub fn intel_display_power_put_async(i915: &mut drm_i915_private, domain: u32) {
    __intel_display_power_put_domain(i915, domain);
}

/// `struct intel_display_power_domain_set`.
#[derive(Clone, Copy, Default)]
pub struct intel_display_power_domain_set {
    pub mask: [u64; DOMAIN_WORDS],
}

/// `intel_display_power_get_in_set_if_enabled`.
pub fn intel_display_power_get_in_set_if_enabled(i915: &mut drm_i915_private,
                                                 set: &mut intel_display_power_domain_set,
                                                 domain: u32) -> bool {
    if test_bit(&set.mask, domain) {
        linux::say("warning: power domain already in the set");
    }
    if !intel_display_power_get_if_enabled(i915, domain) {
        return false;
    }
    set_bit(&mut set.mask, domain);
    true
}

/// `intel_display_power_put_all_in_set` → `intel_display_power_put_mask_in_set`.
pub fn intel_display_power_put_all_in_set(i915: &mut drm_i915_private,
                                          set: &mut intel_display_power_domain_set) {
    for domain in 0..POWER_DOMAIN_NUM {
        if !test_bit(&set.mask, domain) {
            continue;
        }
        intel_display_power_put(i915, domain);
        set.mask[(domain / 64) as usize] &= !(1 << (domain % 64));
    }
}

/// `POWER_DOMAIN_PIPE`.
pub const fn POWER_DOMAIN_PIPE(pipe: u32) -> u32 {
    pipe - PIPE_A + POWER_DOMAIN_PIPE_A
}

/// `POWER_DOMAIN_PIPE_PANEL_FITTER`.
pub const fn POWER_DOMAIN_PIPE_PANEL_FITTER(pipe: u32) -> u32 {
    pipe - PIPE_A + POWER_DOMAIN_PIPE_PANEL_FITTER_A
}

/// `POWER_DOMAIN_TRANSCODER`.
pub const fn POWER_DOMAIN_TRANSCODER(tran: u32) -> u32 {
    if tran == TRANSCODER_EDP {
        POWER_DOMAIN_TRANSCODER_EDP
    } else {
        tran - TRANSCODER_A + POWER_DOMAIN_TRANSCODER_A
    }
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

/// `gen9_dbuf_slice_set`.
fn gen9_dbuf_slice_set(i915: &mut drm_i915_private, slice: u32, enable: bool) {
    let reg = DBUF_CTL_S(slice);
    intel_de_rmw(i915, reg, DBUF_POWER_REQUEST, if enable { DBUF_POWER_REQUEST } else { 0 });
    intel_de_read(i915, reg);
    linux::udelay(10);

    let state = intel_de_read(i915, reg) & DBUF_POWER_STATE != 0;
    if enable != state {
        Line::new().s("DBuf slice ").dec(slice as u64).s(" power ")
            .s(if enable { "enable" } else { "disable" }).s(" timeout!").say();
    }
}

/// `for_each_dbuf_slice`.
fn dbuf_slices() -> impl Iterator<Item = u32> {
    (DBUF_S1..I915_MAX_DBUF_SLICES).filter(|s| TGL_DISPLAY_DBUF_SLICE_MASK & (1 << s) != 0)
}

/// `gen9_dbuf_slices_update`.
pub fn gen9_dbuf_slices_update(i915: &mut drm_i915_private, req_slices: u8) {
    let slice_mask = TGL_DISPLAY_DBUF_SLICE_MASK as u8;
    if req_slices & !slice_mask != 0 {
        Line::new().s("Invalid set of dbuf slices (").hex(req_slices as u32)
            .s(") requested (total dbuf slices ").hex(slice_mask as u32).s(")").say();
    }
    for slice in dbuf_slices() {
        gen9_dbuf_slice_set(i915, slice, req_slices as u32 & (1 << slice) != 0);
    }
    i915.display.dbuf_enabled_slices = req_slices;
}

/// `intel_enabled_dbuf_slices_mask` (skl_watermark.c).
pub fn intel_enabled_dbuf_slices_mask(i915: &mut drm_i915_private) -> u8 {
    let mut enabled_slices = 0u8;
    for slice in dbuf_slices() {
        if intel_de_read(i915, DBUF_CTL_S(slice)) & DBUF_POWER_STATE != 0 {
            enabled_slices |= 1 << slice;
        }
    }
    enabled_slices
}

/// `gen9_dbuf_enable`: power up at least one slice; which ones are needed
/// is decided later.
fn gen9_dbuf_enable(i915: &mut drm_i915_private) {
    i915.display.dbuf_enabled_slices = intel_enabled_dbuf_slices_mask(i915);
    let slices_mask = (1 << DBUF_S1) as u8 | i915.display.dbuf_enabled_slices;
    gen9_dbuf_slices_update(i915, slices_mask);
}

/// `gen12_dbuf_slices_config`.
fn gen12_dbuf_slices_config(i915: &mut drm_i915_private) {
    for slice in dbuf_slices() {
        intel_de_rmw(i915, DBUF_CTL_S(slice), DBUF_TRACKER_STATE_SERVICE_MASK, DBUF_TRACKER_STATE_SERVICE(8));
    }
}

/// `icl_mbus_init` (display version 12).
fn icl_mbus_init(i915: &mut drm_i915_private) {
    let mut abox_regs = TGL_DISPLAY_ABOX_MASK;
    let mask = MBUS_ABOX_BT_CREDIT_POOL1_MASK | MBUS_ABOX_BT_CREDIT_POOL2_MASK
        | MBUS_ABOX_B_CREDIT_MASK | MBUS_ABOX_BW_CREDIT_MASK;
    let val = MBUS_ABOX_BT_CREDIT_POOL1(16) | MBUS_ABOX_BT_CREDIT_POOL2(16)
        | MBUS_ABOX_B_CREDIT(1) | MBUS_ABOX_BW_CREDIT(1);

    // Display version 12 reads pixel data through abox1 and abox2 and
    // still expects abox_ctl0 to be programmed.
    abox_regs |= 1 << 0;

    let regs = [MBUS_ABOX_CTL_0, MBUS_ABOX_CTL_1, MBUS_ABOX_CTL_2];
    for i in 0..32 {
        if abox_regs & (1 << i) != 0 {
            intel_de_rmw(i915, regs[i], mask, val);
        }
    }
}

/// `struct buddy_page_mask`.
#[derive(Clone, Copy)]
pub struct buddy_page_mask {
    pub page_mask: u32,
    pub ty: u32,
    pub num_channels: u32,
}

/// `tgl_bw_buddy_init` (integrated, not ADL-S or RKL A0).
fn tgl_bw_buddy_init(i915: &mut drm_i915_private) {
    let dram_info = i915.dram_info;
    let table = &tables::tgl_buddy_page_masks;
    let abox_mask = TGL_DISPLAY_ABOX_MASK;

    let mut config = 0;
    while table[config].page_mask != 0 {
        if table[config].num_channels == dram_info.num_channels && table[config].ty == dram_info.ty {
            break;
        }
        config += 1;
    }

    if table[config].page_mask == 0 {
        linux::say("Unknown memory configuration; disabling address buddy logic.");
        for i in 0..32 {
            if abox_mask & (1 << i) != 0 {
                intel_de_write(i915, BW_BUDDY_CTL(i), BW_BUDDY_DISABLE);
            }
        }
    } else {
        for i in 0..32 {
            if abox_mask & (1 << i) != 0 {
                intel_de_write(i915, BW_BUDDY_PAGE_MASK(i), table[config].page_mask);
                // Wa_22010178259:tgl,dg1,rkl,adl-s
                intel_de_rmw(i915, BW_BUDDY_CTL(i), BW_BUDDY_TLB_REQ_TIMER_MASK, BW_BUDDY_TLB_REQ_TIMER(0x8));
            }
        }
    }
}

/// `icl_display_core_init` (display version 12, integrated, not DG2).
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

    // 2. Initialize all combo phys
    crate::intel_combo_phy::intel_combo_phy_init(i915);

    // 3. Enable Power Well 1 (PG1). The AUX IO power wells are enabled on
    // demand.
    let well = lookup_power_well(i915, SKL_DISP_PW_1);
    intel_power_well_enable(i915, well);

    // 4. Enable CDCLK.
    let mut cd = i915.display.cdclk;
    crate::intel_cdclk::intel_cdclk_init_hw(i915, &mut cd);
    i915.display.cdclk = cd;

    gen12_dbuf_slices_config(i915);

    // 5. Enable DBUF.
    gen9_dbuf_enable(i915);

    // 6. Setup MBUS.
    icl_mbus_init(i915);

    // 7. Program arbiter BW_BUDDY registers
    tgl_bw_buddy_init(i915);

    // Wa_14011508470:tgl,dg1,rkl,adl-s,adl-p,dg2
    intel_de_rmw(i915, GEN11_CHICKEN_DCPR_2, 0,
                 DCPR_CLEAR_MEMSTAT_DIS | DCPR_SEND_RESP_IMM | DCPR_MASK_LPMODE
                     | DCPR_MASK_MAXLATENCY_MEMUP_CLR);
}

/// `intel_power_domains_init_hw` (display version 11 and up). The wells of
/// `POWER_DOMAIN_INIT` stay referenced until the display readout is done.
pub fn intel_power_domains_init_hw(i915: &mut drm_i915_private, pch_type: i32, resume: bool) {
    i915.display.power.initializing = true;

    icl_display_core_init(i915, pch_type, resume);

    if i915.display.power.init_wakeref {
        linux::say("warning: init wakeref already held");
    }
    intel_display_power_get(i915, POWER_DOMAIN_INIT);
    i915.display.power.init_wakeref = true;

    // Disable power support if the user asked so.
    if i915.display.params.disable_power_well == 0 {
        intel_display_power_get(i915, POWER_DOMAIN_INIT);
        i915.display.power.disable_wakeref = true;
    }
    intel_power_domains_sync_hw(i915);

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

/// `tgl_disable_dc3co`.
fn tgl_disable_dc3co(i915: &mut drm_i915_private) {
    intel_de_rmw(i915, DC_STATE_EN, DC_STATE_DC3CO_STATUS, 0);
    gen9_set_dc_state(i915, DC_STATE_DISABLE);
    // Delay of 200us DC3CO Exit time B.Spec 49196
    linux::udelay(200);
}

/// `gen9_assert_dbuf_enabled`.
fn gen9_assert_dbuf_enabled(i915: &mut drm_i915_private) {
    let hw_enabled_dbuf_slices = intel_enabled_dbuf_slices_mask(i915);
    let enabled_dbuf_slices = i915.display.dbuf_enabled_slices;
    if hw_enabled_dbuf_slices != enabled_dbuf_slices {
        Line::new().s("Unexpected DBuf power power state (").hex(hw_enabled_dbuf_slices as u32)
            .s(", expected ").hex(enabled_dbuf_slices as u32).s(")").say();
    }
}

/// `gen9_disable_dc_states` (display version 12; the DMC wakelock is
/// only used from version 20 on).
pub fn gen9_disable_dc_states(i915: &mut drm_i915_private) {
    if i915.display.power.target_dc_state == DC_STATE_EN_DC3CO {
        tgl_disable_dc3co(i915);
        return;
    }

    if !i915.display.has_display {
        gen9_set_dc_state(i915, DC_STATE_DISABLE);
        return;
    }
    gen9_set_dc_state(i915, DC_STATE_DISABLE);

    let mut cdclk_config = crate::intel_cdclk::intel_cdclk_config::default();
    crate::intel_cdclk::intel_cdclk_get_cdclk(i915, &mut cdclk_config);
    // The voltage level cannot be read out, so only the clocks are compared.
    if crate::intel_cdclk::intel_cdclk_clock_changed(&i915.display.cdclk.hw, &cdclk_config) {
        linux::say("warning: CDCLK changed while DC states were enabled");
    }

    gen9_assert_dbuf_enabled(i915);

    // The DMC keeps the HW context of port A only; combo PHY B loses it
    // across DC transitions.
    crate::intel_combo_phy::intel_combo_phy_init(i915);
}

/// `gen9_dc_off_power_well_enable`.
fn gen9_dc_off_power_well_enable(i915: &mut drm_i915_private) {
    gen9_disable_dc_states(i915);
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
