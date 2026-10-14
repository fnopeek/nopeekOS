//! The GT's MMIO probe (Linux 6.18.55): gt/intel_gt.c `intel_gt_init_mmio`
//! and what it calls in gt/intel_gt_clock_utils.c, gt/intel_sseu.c,
//! gt/intel_gt_mcr.c and gt/intel_engine_cs.c, on Tiger Lake.
//!
//! `intel_uc_init_mmio` is left out: it only works out the forcewake
//! domains of the GuC scratch registers, and TGL runs without GuC.

use crate::i915_drv::drm_i915_private;
use crate::intel_engine::has_engine;
use crate::intel_uncore::intel_uncore;
use crate::linux::{self, Line};
use crate::regs::*;
use crate::tables;

/// `struct engine_info` of `intel_engines[]`, mmio_bases flattened.
#[derive(Clone, Copy)]
pub struct engine_info {
    pub class: u32,
    pub instance: u32,
    pub ver0: u32,
    pub base0: u32,
    pub ver1: u32,
    pub base1: u32,
    pub ver2: u32,
    pub base2: u32,
}

/// `struct sseu_dev_info`, the pre-Xe_HP (`hsw`) layout.
#[derive(Clone, Copy, Default)]
pub struct sseu_dev_info {
    pub slice_mask: u32,
    pub subslice_mask: [u32; 3],
    pub eu_mask: [[u16; 6]; 3],
    pub eu_total: u32,
    pub eu_per_subslice: u32,
    pub max_slices: u32,
    pub max_subslices: u32,
    pub max_eus_per_subslice: u32,
    pub has_slice_pg: bool,
}

/// `struct intel_engine_cs`, as far as `intel_engine_setup` fills it.
#[derive(Clone, Copy, Default)]
pub struct intel_engine_cs {
    pub id: u32,
    pub mask: u64,
    pub reset_domain: u32,
    pub mmio_base: u32,
    pub class: u32,
    pub instance: u32,
    pub logical_mask: u32,
    pub flags: u64,
    pub heartbeat_interval_ms: u64,
    pub max_busywait_duration_ns: u64,
    pub preempt_timeout_ms: u64,
    pub stop_timeout_ms: u64,
    pub timeslice_duration_ms: u64,
    pub context_size: u32,
    pub uabi_capabilities: u32,
}

/// `struct intel_mmio_range`, a steering table entry.
#[derive(Clone, Copy)]
pub struct intel_mmio_range {
    pub start: u32,
    pub end: u32,
}

/// `struct intel_gt_info`.
#[derive(Clone, Copy, Default)]
pub struct intel_gt_info {
    pub engine_mask: u64,
    pub num_engines: u32,
    pub sfc_mask: u32,
    pub vdbox_sfc_access: u32,
    pub l3bank_mask: u32,
    pub sseu: sseu_dev_info,
}

/// `struct intel_gt`.
pub struct intel_gt {
    pub info: intel_gt_info,
    pub clock_frequency: u32,
    pub clock_period_ns: u32,
    pub engine: [Option<intel_engine_cs>; I915_NUM_ENGINES as usize],
    pub has_logical_contexts: bool,
    pub wopcm: crate::intel_wopcm::intel_wopcm,
    /// `gt->wa_list`, the GT workarounds.
    pub wa_list: crate::intel_workarounds::i915_wa_list,
    /// `gt->steering_table[L3BANK]` is set (`icl_l3bank_steering_table`).
    pub steering_l3bank: bool,
    /// `gt->default_steering`: (group, instance).
    pub default_steering: (u8, u8),
    pub rc6: crate::intel_rc6::intel_rc6,
    pub rps: crate::intel_rps::intel_rps,
    pub mocs: crate::intel_mocs::intel_mocs,
    /// `gt->pm_ier`, `gt->pm_imr`: the PM interrupt enable and mask.
    pub pm_ier: u32,
    pub pm_imr: u32,
    /// `gt->wakeref` count, and `gt->awake` (the GT IRQ power domain held).
    pub wakeref_count: u32,
    pub awake: bool,
    /// `gt->user_wakeref`.
    pub user_wakeref: u32,
    /// `gt->reset.flags`: I915_WEDGED, I915_WEDGED_ON_INIT, I915_WEDGED_ON_FINI.
    pub wedged: bool,
    pub wedged_on_init: bool,
    pub wedged_on_fini: bool,
    /// `intel_engines_init` has set up the engines' submission backends.
    pub engines_initialized: bool,
}

impl intel_gt {
    pub fn new() -> Self {
        intel_gt {
            // intel_gt_common_init_early: the platform's engine mask.
            info: intel_gt_info { engine_mask: TGL_PLATFORM_ENGINE_MASK as u64, ..Default::default() },
            clock_frequency: 0,
            clock_period_ns: 0,
            engine: [None; I915_NUM_ENGINES as usize],
            has_logical_contexts: false,
            wopcm: Default::default(),
            wa_list: Default::default(),
            steering_l3bank: false,
            default_steering: (0, 0),
            rc6: Default::default(),
            rps: Default::default(),
            mocs: Default::default(),
            pm_ier: 0,
            pm_imr: 0,
            wakeref_count: 0,
            awake: false,
            user_wakeref: 0,
            wedged: false,
            wedged_on_init: false,
            wedged_on_fini: false,
            engines_initialized: false,
        }
    }
}

/// `REG_FIELD_GET(mask, value)`.
pub fn reg_field_get(mask: u32, value: u32) -> u32 {
    (value & mask) >> mask.trailing_zeros()
}

/// `GENMASK(h, l)` for 32 bits.
pub fn genmask(h: u32, l: u32) -> u32 {
    (u32::MAX >> (31 - h)) & (u32::MAX << l)
}

// ── gt/intel_gt_clock_utils.c ────────────────────────────────────

fn read_reference_ts_freq(i915: &mut drm_i915_private) -> u32 {
    let ts_override = i915.uncore.intel_uncore_read(GEN9_TIMESTAMP_OVERRIDE);
    let mut base_freq = ((ts_override & GEN9_TIMESTAMP_OVERRIDE_US_COUNTER_DIVIDER_MASK)
        >> GEN9_TIMESTAMP_OVERRIDE_US_COUNTER_DIVIDER_SHIFT) + 1;
    base_freq *= 1_000_000;
    let mut frac_freq = (ts_override & GEN9_TIMESTAMP_OVERRIDE_US_COUNTER_DENOMINATOR_MASK)
        >> GEN9_TIMESTAMP_OVERRIDE_US_COUNTER_DENOMINATOR_SHIFT;
    frac_freq = 1_000_000 / (frac_freq + 1);
    base_freq + frac_freq
}

fn gen11_get_crystal_clock_freq(rpm_config_reg: u32) -> u32 {
    let crystal_clock = rpm_config_reg & GEN11_RPM_CONFIG0_CRYSTAL_CLOCK_FREQ_MASK;
    match crystal_clock {
        GEN11_RPM_CONFIG0_CRYSTAL_CLOCK_FREQ_24_MHZ => 24_000_000,
        GEN11_RPM_CONFIG0_CRYSTAL_CLOCK_FREQ_19_2_MHZ => 19_200_000,
        GEN11_RPM_CONFIG0_CRYSTAL_CLOCK_FREQ_38_4_MHZ => 38_400_000,
        GEN11_RPM_CONFIG0_CRYSTAL_CLOCK_FREQ_25_MHZ => 25_000_000,
        _ => {
            Line::new().s("warning: missing case crystal clock ").hex(crystal_clock).say();
            0
        }
    }
}

fn gen11_read_clock_frequency(i915: &mut drm_i915_private) -> u32 {
    let ctc_reg = i915.uncore.intel_uncore_read(CTC_MODE);
    if ctc_reg & CTC_SOURCE_PARAMETER_MASK == CTC_SOURCE_DIVIDE_LOGIC {
        read_reference_ts_freq(i915)
    } else {
        let c0 = i915.uncore.intel_uncore_read(RPM_CONFIG0);
        let freq = gen11_get_crystal_clock_freq(c0);
        freq >> (3 - reg_field_get(GEN10_RPM_CONFIG0_CTC_SHIFT_PARAMETER_MASK, c0))
    }
}

/// `intel_gt_clock_interval_to_ns`.
pub fn intel_gt_clock_interval_to_ns(gt: &intel_gt, count: u64) -> u64 {
    count * 1_000_000_000 / gt.clock_frequency as u64
}

/// `intel_gt_init_clock_frequency` (GRAPHICS_VER >= 11, not 11 itself).
fn intel_gt_init_clock_frequency(i915: &mut drm_i915_private, gt: &mut intel_gt) {
    gt.clock_frequency = gen11_read_clock_frequency(i915);
    if gt.clock_frequency != 0 {
        gt.clock_period_ns = intel_gt_clock_interval_to_ns(gt, 1) as u32;
    }
}

// ── gt/intel_sseu.c ──────────────────────────────────────────────

fn intel_sseu_set_info(sseu: &mut sseu_dev_info, max_slices: u32, max_subslices: u32,
                       max_eus_per_subslice: u32) {
    sseu.max_slices = max_slices;
    sseu.max_subslices = max_subslices;
    sseu.max_eus_per_subslice = max_eus_per_subslice;
}

fn intel_sseu_has_subslice(sseu: &sseu_dev_info, slice: u32, subslice: u32) -> bool {
    if slice >= sseu.max_slices || subslice >= sseu.max_subslices {
        return false;
    }
    sseu.subslice_mask[slice as usize] & (1 << subslice) != 0
}

fn sseu_set_eus(sseu: &mut sseu_dev_info, slice: u32, subslice: u32, eu_mask: u16) {
    sseu.eu_mask[slice as usize][subslice as usize] = eu_mask;
}

fn compute_eu_total(sseu: &sseu_dev_info) -> u32 {
    let mut total = 0;
    for s in 0..sseu.max_slices {
        for ss in 0..sseu.max_subslices {
            total += sseu.eu_mask[s as usize][ss as usize].count_ones();
        }
    }
    total
}

fn gen11_compute_sseu_info(sseu: &mut sseu_dev_info, ss_en: u32, eu_en: u16) {
    let valid_ss_mask = genmask(sseu.max_subslices - 1, 0);
    sseu.slice_mask |= 1;
    sseu.subslice_mask[0] = ss_en & valid_ss_mask;
    for ss in 0..sseu.max_subslices {
        if intel_sseu_has_subslice(sseu, 0, ss) {
            sseu_set_eus(sseu, 0, ss, eu_en);
        }
    }
    sseu.eu_per_subslice = eu_en.count_ones();
    sseu.eu_total = compute_eu_total(sseu);
}

/// `gen12_sseu_info_init` (GRAPHICS_VER 12, below 12.55).
fn gen12_sseu_info_init(i915: &mut drm_i915_private, gt: &mut intel_gt) {
    let sseu = &mut gt.info.sseu;
    let mut eu_en: u16 = 0;

    intel_sseu_set_info(sseu, 1, 6, 16);

    let s_en = reg_field_get(GEN11_GT_S_ENA_MASK, i915.uncore.intel_uncore_read(GEN11_GT_SLICE_ENABLE)) as u8;
    if s_en != 0x1 {
        Line::new().s("warning: slice enable ").hex(s_en as u32).say();
    }
    let g_dss_en = i915.uncore.intel_uncore_read(GEN12_GT_GEOMETRY_DSS_ENABLE);

    // One bit per pair of EUs.
    let eu_en_fuse = !(reg_field_get(GEN11_EU_DIS_MASK, i915.uncore.intel_uncore_read(GEN11_EU_DISABLE)) as u8);
    for eu in 0..sseu.max_eus_per_subslice / 2 {
        if eu_en_fuse & (1 << eu) != 0 {
            eu_en |= (1 << (eu * 2)) | (1 << (eu * 2 + 1));
        }
    }
    gen11_compute_sseu_info(sseu, g_dss_en, eu_en);
    sseu.has_slice_pg = true;
}

// ── gt/intel_gt_mcr.c ────────────────────────────────────────────

/// `intel_gt_mcr_init`, GRAPHICS_VER 11 up to 12.55: the L3 bank steering
/// table (`icl_l3bank_steering_table`) and the L3 bank mask from fuses.
fn intel_gt_mcr_init(i915: &mut drm_i915_private, gt: &mut intel_gt) {
    gt.steering_l3bank = true;
    gt.info.l3bank_mask = !i915.uncore.intel_uncore_read(GEN10_MIRROR_FUSE3) & GEN10_L3BANK_MASK;
    if gt.info.l3bank_mask == 0 {
        linux::say("warning: L3 bank mask is all zero!");
    }
}

/// `reg_needs_read_steering`: only the L3 bank table exists below 12.50.
fn reg_needs_read_steering(gt: &intel_gt, offset: u32) -> bool {
    if !gt.steering_l3bank {
        return false;
    }
    tables::icl_l3bank_steering_table.iter().take_while(|r| r.end != 0)
        .any(|r| offset >= r.start && offset <= r.end)
}

/// `get_nonterminated_steering` for the L3BANK type.
fn get_nonterminated_steering(gt: &intel_gt) -> (u32, u32) {
    (0, gt.info.l3bank_mask.trailing_zeros())
}

/// `rw_with_mcr_steering_fw`, GRAPHICS_VER 11 up to 12.70, read: steer,
/// read, restore the old steering (implicit steering depends on it).
/// Reads stay multicast (Wa_22013088509).
fn rw_with_mcr_steering_fw_read(uncore: &intel_uncore, reg: u32, group: u32, instance: u32) -> u32 {
    let mcr_mask = GEN11_MCR_SLICE_MASK | GEN11_MCR_SUBSLICE_MASK;
    let mcr_ss = GEN11_MCR_SLICE(group) | GEN11_MCR_SUBSLICE(instance);

    let mut mcr = uncore.intel_uncore_read_fw(GEN8_MCR_SELECTOR);
    let old_mcr = mcr;
    mcr &= !mcr_mask;
    mcr |= mcr_ss;
    uncore.intel_uncore_write_fw(GEN8_MCR_SELECTOR, mcr);

    let val = uncore.intel_uncore_read_fw(reg);

    uncore.intel_uncore_write_fw(GEN8_MCR_SELECTOR, old_mcr);
    val
}

/// `intel_gt_mcr_read_any_fw`: a value from a non-terminated instance.
/// The MCR lock is a software lock before 12.70; the module is
/// single-threaded.
pub fn intel_gt_mcr_read_any_fw(uncore: &intel_uncore, gt: &intel_gt, reg: u32) -> u32 {
    if reg_needs_read_steering(gt, reg) {
        let (group, instance) = get_nonterminated_steering(gt);
        return rw_with_mcr_steering_fw_read(uncore, reg, group, instance);
    }
    uncore.intel_uncore_read_fw(reg)
}

/// `intel_gt_mcr_multicast_write_fw`: below 12.70 a plain write reaches
/// every instance.
pub fn intel_gt_mcr_multicast_write_fw(uncore: &intel_uncore, reg: u32, value: u32) {
    uncore.intel_uncore_write_fw(reg, value);
}

// ── gt/intel_engine_cs.c ─────────────────────────────────────────

/// `intel_engine_context_size`.
fn intel_engine_context_size(class: u32) -> u32 {
    match class {
        COMPUTE_CLASS | RENDER_CLASS => GEN11_LR_CONTEXT_RENDER_SIZE as u32,
        _ => GEN8_LR_CONTEXT_OTHER_SIZE as u32,
    }
}

/// `__engine_mmio_base`: the first base whose version the device reaches.
fn __engine_mmio_base(ver: u32, info: &engine_info) -> u32 {
    let bases = [(info.ver0, info.base0), (info.ver1, info.base1), (info.ver2, info.base2)];
    for &(v, b) in bases.iter() {
        if ver >= v {
            return b;
        }
    }
    0
}

/// `get_reset_domain`, GRAPHICS_VER >= 11, for the engines TGL has.
fn get_reset_domain(id: u32) -> u32 {
    match id {
        RCS0 => RESET_DOMAIN_RCS0,
        BCS0 => RESET_DOMAIN_BCS0,
        VCS0 => RESET_DOMAIN_VCS0,
        VCS1 => RESET_DOMAIN_VCS1,
        VCS2 => RESET_DOMAIN_VCS2,
        VCS3 => RESET_DOMAIN_VCS3,
        VECS0 => RESET_DOMAIN_VECS0,
        VECS1 => RESET_DOMAIN_VECS1,
        _ => 0,
    }
}

/// `gen11_vdbox_has_sfc`, MEDIA_VER 12.
fn gen11_vdbox_has_sfc(gt: &intel_gt, physical_vdbox: u32, vdbox_mask: u16) -> bool {
    if gt.info.sfc_mask & (1 << (physical_vdbox / 2)) == 0 {
        false
    } else {
        physical_vdbox % 2 == 0 || (1u16 << (physical_vdbox - 1)) & vdbox_mask == 0
    }
}

/// `engine_mask_apply_media_fuses`, MEDIA_VER 12 below 12.55: the fuse
/// register has disable semantics.
fn engine_mask_apply_media_fuses(i915: &mut drm_i915_private, gt: &mut intel_gt) {
    let media_fuse = !i915.uncore.intel_uncore_read(GEN11_GT_VEBOX_VDBOX_DISABLE);
    let mut vdbox_mask = reg_field_get(GEN11_GT_VDBOX_DISABLE_MASK, media_fuse) as u16;
    let mut vebox_mask = reg_field_get(GEN11_GT_VEBOX_DISABLE_MASK, media_fuse) as u16;
    gt.info.sfc_mask = !0;

    for i in 0..I915_MAX_VCS {
        if !has_engine(gt.info.engine_mask, _VCS(i)) {
            vdbox_mask &= !(1 << i);
            continue;
        }
        if (1 << i) & vdbox_mask == 0 {
            gt.info.engine_mask &= !(1u64 << _VCS(i));
            Line::new().s("vcs").dec(i as u64).s(" fused off").say();
            continue;
        }
        if gen11_vdbox_has_sfc(gt, i, vdbox_mask) {
            gt.info.vdbox_sfc_access |= 1 << i;
        }
    }
    for i in 0..I915_MAX_VECS {
        if !has_engine(gt.info.engine_mask, _VECS(i)) {
            vebox_mask &= !(1 << i);
            continue;
        }
        if (1 << i) & vebox_mask == 0 {
            gt.info.engine_mask &= !(1u64 << _VECS(i));
            Line::new().s("vecs").dec(i as u64).s(" fused off").say();
        }
    }
}

/// `init_engine_mask`. The compute fuses apply only with more than one
/// CCS, the GSC CS only exists on MTL, the CCS fix-up is DG2's.
fn init_engine_mask(i915: &mut drm_i915_private, gt: &mut intel_gt) -> u64 {
    engine_mask_apply_media_fuses(i915, gt);
    gt.info.engine_mask
}

/// `populate_logical_ids`.
fn populate_logical_ids(gt: &intel_gt, logical_ids: &mut [u8; 9], class: u32, map: &[u8]) {
    let mut current_logical_id = 0u8;
    for &m in map.iter() {
        for (i, e) in tables::intel_engines.iter().enumerate() {
            if !has_engine(gt.info.engine_mask, i as u32) || e.class != class {
                continue;
            }
            if e.instance == m as u32 {
                logical_ids[e.instance as usize] = current_logical_id;
                current_logical_id += 1;
                break;
            }
        }
    }
}

/// `setup_logical_ids`.
fn setup_logical_ids(gt: &intel_gt, logical_ids: &mut [u8; 9], class: u32) {
    if class == VIDEO_DECODE_CLASS {
        let map = [0u8, 2, 4, 6, 1, 3, 5, 7];
        populate_logical_ids(gt, logical_ids, class, &map);
    } else {
        let map: [u8; 9] = core::array::from_fn(|i| i as u8);
        populate_logical_ids(gt, logical_ids, class, &map);
    }
}

/// `intel_engine_setup`.
fn intel_engine_setup(i915: &mut drm_i915_private, gt: &mut intel_gt, id: u32, logical_instance: u8) {
    let info = tables::intel_engines[id as usize];
    let mut engine = intel_engine_cs {
        id,
        mask: 1u64 << id,
        reset_domain: get_reset_domain(id),
        mmio_base: __engine_mmio_base(i915.graphics_ver(), &info),
        class: info.class,
        instance: info.instance,
        logical_mask: 1 << logical_instance,
        ..Default::default()
    };

    // __ffs(CCS_MASK | RCS_MASK) == instance; TGL has RCS0 only.
    if (engine.class == COMPUTE_CLASS || engine.class == RENDER_CLASS) && engine.instance == 0 {
        engine.flags |= I915_ENGINE_FIRST_RENDER_COMPUTE;
    }
    if engine.class == RENDER_CLASS || engine.class == COMPUTE_CLASS {
        engine.flags |= I915_ENGINE_HAS_RCS_REG_STATE;
        engine.flags |= I915_ENGINE_HAS_EU_PRIORITY;
    }

    engine.heartbeat_interval_ms = CONFIG_DRM_I915_HEARTBEAT_INTERVAL as u64;
    engine.max_busywait_duration_ns = CONFIG_DRM_I915_MAX_REQUEST_BUSYWAIT as u64;
    engine.preempt_timeout_ms = CONFIG_DRM_I915_PREEMPT_TIMEOUT as u64;
    engine.stop_timeout_ms = CONFIG_DRM_I915_STOP_TIMEOUT as u64;
    engine.timeslice_duration_ms = CONFIG_DRM_I915_TIMESLICE_DURATION as u64;

    // Mid-thread preemption is not available on Gen12.
    if engine.flags & I915_ENGINE_HAS_RCS_REG_STATE != 0 {
        engine.preempt_timeout_ms = CONFIG_DRM_I915_PREEMPT_TIMEOUT_COMPUTE as u64;
    }
    // CLAMP_PROP: the limits are the kernel's jiffies range
    // (MAX_SCHEDULE_TIMEOUT, 2 jiffies for busywait); the config defaults
    // stay below them, so no property changes.

    engine.context_size = intel_engine_context_size(engine.class);
    if engine.context_size != 0 {
        gt.has_logical_contexts = true;
    }

    // intel_engine_sanitize_mmio: mask off all writes into the unknown HWSP.
    i915.uncore.intel_uncore_write(RING_HWSTAM(engine.mmio_base), !0);

    gt.engine[id as usize] = Some(engine);
}

fn gen8_report_fault(fault: u32, fault_data0: u32, fault_data1: u32) {
    let fault_addr = ((fault_data1 & FAULT_VA_HIGH_BITS) as u64) << 44 | (fault_data0 as u64) << 12;
    Line::new().s("unexpected fault at ").hex((fault_addr >> 32) as u32).s("_").hex(fault_addr as u32)
        .s(if fault_data1 & FAULT_GTT_SEL != 0 { " GGTT" } else { " PPGTT" })
        .s(" engine ").dec(reg_field_get(RING_FAULT_ENGINE_ID_MASK, fault) as u64)
        .s(" source ").dec(reg_field_get(RING_FAULT_SRCID_MASK, fault) as u64)
        .s(" type ").dec(reg_field_get(RING_FAULT_FAULT_TYPE_MASK, fault) as u64).say();
}

/// `gen8_check_faults`, GRAPHICS_VER >= 12 registers.
fn gen8_check_faults(i915: &mut drm_i915_private) {
    let fault = i915.uncore.intel_uncore_read(GEN12_RING_FAULT_REG);
    if fault & RING_FAULT_VALID != 0 {
        let d0 = i915.uncore.intel_uncore_read(GEN12_FAULT_TLB_DATA0);
        let d1 = i915.uncore.intel_uncore_read(GEN12_FAULT_TLB_DATA1);
        gen8_report_fault(fault, d0, d1);
    }
}

/// `intel_gt_clear_error_registers`, GRAPHICS_VER 12 below 12.55.
pub fn intel_gt_clear_error_registers(i915: &mut drm_i915_private) {
    let u = &mut i915.uncore;
    u.intel_uncore_write(PGTBL_ER, 0);
    u.intel_uncore_write(IPEIR_I965, 0);
    u.intel_uncore_write(EIR, 0);
    let eir = u.intel_uncore_read(EIR);
    if eir != 0 {
        Line::new().s("EIR stuck: ").hex(eir).s(", masking").say();
        u.intel_uncore_rmw(EMR, 0, eir);
        u.intel_uncore_write(GEN2_IIR, I915_MASTER_ERROR_INTERRUPT);
    }
    u.intel_uncore_rmw(GEN12_RING_FAULT_REG, RING_FAULT_VALID, 0);
    let _ = u.intel_uncore_read_notrace(GEN12_RING_FAULT_REG);
}

/// `intel_gt_check_and_clear_faults`.
fn intel_gt_check_and_clear_faults(i915: &mut drm_i915_private) {
    gen8_check_faults(i915);
    intel_gt_clear_error_registers(i915);
}

/// `__setup_engine_capabilities` (GRAPHICS_VER >= 11).
fn __setup_engine_capabilities(gt: &intel_gt, engine: &mut intel_engine_cs) {
    if engine.class == VIDEO_DECODE_CLASS {
        engine.uabi_capabilities |= I915_VIDEO_CLASS_CAPABILITY_HEVC;
        if gt.info.vdbox_sfc_access & (1 << engine.instance) != 0 {
            engine.uabi_capabilities |= I915_VIDEO_AND_ENHANCE_CLASS_CAPABILITY_SFC;
        }
    } else if engine.class == VIDEO_ENHANCEMENT_CLASS
        && gt.info.sfc_mask & (1 << engine.instance) != 0 {
        engine.uabi_capabilities |= I915_VIDEO_AND_ENHANCE_CLASS_CAPABILITY_SFC;
    }
}

/// `intel_engines_init_mmio`.
fn intel_engines_init_mmio(i915: &mut drm_i915_private, gt: &mut intel_gt) {
    let engine_mask = init_engine_mask(i915, gt);
    let mut mask = 0u64;
    let mut logical_ids = [0u8; 9];

    if engine_mask == 0 {
        linux::say("warning: engine mask is empty");
    }
    for class in 0..=MAX_ENGINE_CLASS {
        setup_logical_ids(gt, &mut logical_ids, class);
        for (i, e) in tables::intel_engines.iter().enumerate() {
            if e.class != class || !has_engine(gt.info.engine_mask, i as u32) {
                continue;
            }
            intel_engine_setup(i915, gt, i as u32, logical_ids[e.instance as usize]);
            mask |= 1 << i;
        }
    }
    if mask != engine_mask {
        linux::say("warning: engine table and engine mask disagree");
        gt.info.engine_mask = mask;
    }
    gt.info.num_engines = mask.count_ones();

    intel_gt_check_and_clear_faults(i915);

    for i in 0..I915_NUM_ENGINES as usize {
        if let Some(mut e) = gt.engine[i] {
            __setup_engine_capabilities(gt, &mut e);
            gt.engine[i] = Some(e);
        }
    }

    i915.uncore.intel_uncore_prune_engine_fw_domains(gt.info.engine_mask);
}

/// `intel_gt_init_mmio`.
pub fn intel_gt_init_mmio(i915: &mut drm_i915_private, gt: &mut intel_gt) {
    intel_gt_init_clock_frequency(i915, gt);
    gen12_sseu_info_init(i915, gt);
    intel_gt_mcr_init(i915, gt);
    intel_engines_init_mmio(i915, gt);
}

/// `intel_engine_class_repr`, for the log.
pub fn intel_engine_class_repr(class: u32) -> &'static str {
    match class {
        RENDER_CLASS => "rcs",
        VIDEO_DECODE_CLASS => "vcs",
        VIDEO_ENHANCEMENT_CLASS => "vecs",
        COPY_ENGINE_CLASS => "bcs",
        COMPUTE_CLASS => "ccs",
        OTHER_CLASS => "other",
        _ => "xxx",
    }
}


// ── gt/intel_reset.c (wedging) ───────────────────────────────────

/// `intel_gt_is_wedged`.
pub fn intel_gt_is_wedged(gt: &intel_gt) -> bool {
    gt.wedged
}

/// `intel_gt_has_unrecoverable_error`.
pub fn intel_gt_has_unrecoverable_error(gt: &intel_gt) -> bool {
    gt.wedged_on_init || gt.wedged_on_fini
}

/// `reset_prepare`: per engine a FORCEWAKE_ALL grab (`reset_prepare_engine`);
/// no engine is awake before the engines are set up. No GuC submission.
fn reset_prepare(i915: &mut drm_i915_private, gt: &intel_gt) {
    for _ in gt.engine.iter().flatten() {
        i915.uncore.intel_uncore_forcewake_get(FORCEWAKE_ALL);
    }
    if gt.engines_initialized {
        linux::say("not ported yet: engine reset.prepare (execlists)");
    }
}

/// `reset_finish`.
fn reset_finish(i915: &mut drm_i915_private, gt: &intel_gt) {
    if gt.engines_initialized {
        linux::say("not ported yet: engine reset.finish, breadcrumbs (execlists)");
    }
    for _ in gt.engine.iter().flatten() {
        i915.uncore.intel_uncore_forcewake_put(FORCEWAKE_ALL);
    }
}

/// `__intel_gt_set_wedged`: stop submission, reset the GT (it does not
/// clobber the display on TGL), mark wedged, cancel what was in flight.
fn __intel_gt_set_wedged(i915: &mut drm_i915_private, gt: &mut intel_gt) {
    if gt.wedged {
        return;
    }

    reset_prepare(i915, gt);

    // Even if the GPU reset fails, it should still stop the engines.
    if !crate::intel_reset::intel_gt_gpu_reset_clobbers_display() {
        let _ = crate::intel_reset::intel_gt_reset_all_engines(i915, gt);
    }

    // engine->submit_request = nop_submit_request, engine->reset.cancel:
    // the request side, set up with the engines.
    gt.wedged = true;

    reset_finish(i915, gt);
}

/// `intel_gt_set_wedged`.
pub fn intel_gt_set_wedged(i915: &mut drm_i915_private, gt: &mut intel_gt) {
    if gt.wedged {
        return;
    }
    __intel_gt_set_wedged(i915, gt);
}

/// `intel_gt_set_wedged_on_init`: non-recoverable.
fn intel_gt_set_wedged_on_init(i915: &mut drm_i915_private, gt: &mut intel_gt) {
    intel_gt_set_wedged(i915, gt);
    gt.wedged_on_init = true;
}

/// `intel_gt_set_wedged_on_fini`. The error-state capture is not kept, and
/// retiring requests waits for the request side.
fn intel_gt_set_wedged_on_fini(i915: &mut drm_i915_private, gt: &mut intel_gt) {
    intel_gt_set_wedged(i915, gt);
    gt.wedged_on_fini = true;
}

// ── gt/intel_gt.c ────────────────────────────────────────────────

/// `intel_gt_init_hw`. On TGL there is no eDRAM hash, no HSW predicate,
/// swizzling is off from GRAPHICS_VER 8 (`detect_bit_6_swizzle`), no
/// unused rings need stopping (gen2/3), `gtt_write_workarounds` has no
/// GRAPHICS_VER 12 entries, PPGTT needs no enable bit, and there is no
/// GuC/HuC firmware (`uc_expand_default_options`: TGL → enable_guc = 0).
pub fn intel_gt_init_hw(i915: &mut drm_i915_private, gt: &mut intel_gt) -> Result<(), i32> {
    i915.uncore.intel_uncore_forcewake_get(FORCEWAKE_ALL);

    // Apply the GT workarounds and determine whether they are sticking.
    crate::intel_workarounds::intel_gt_apply_workarounds(i915, gt);
    let ok = crate::intel_workarounds::intel_gt_verify_workarounds(i915, gt, "init");
    Line::new().s("GT workarounds: ").dec(gt.wa_list.count as u64).s(" registers applied, verify ")
        .s(if ok { "ok" } else { "FAILED" }).say();

    let n = crate::intel_mocs::intel_mocs_init(i915);
    Line::new().s("MOCS: ").dec(tables::tgl_mocs_table.len() as u64).s(" table entries, ").dec(n as u64)
        .s(" global MOCS and ").dec(((n + 1) / 2) as u64).s(" LNCFCMOCS registers written").say();

    i915.uncore.intel_uncore_forcewake_put(FORCEWAKE_ALL);
    Ok(())
}

/// `intel_gt_init`, the hardware side. Scratch page, kernel address
/// space, engines and the default context recording are set up with the
/// engines; no uC and no hwconfig table on TGL (12.55+ / ADL-P);
/// `__engines_verify_workarounds` only runs under DRM_I915_DEBUG_GEM;
/// no migrate context (it needs flat CCS).
pub fn intel_gt_init(i915: &mut drm_i915_private, gt: &mut intel_gt, _ggtt: &mut crate::intel_ggtt::i915_ggtt,
                     _dsm: Option<&mut crate::intel_ggtt::i915_dsm>) -> Result<(), i32> {
    crate::intel_workarounds::intel_gt_init_workarounds(gt);

    // Held over the whole init: the first TLBs the CS uses have been seen
    // stale otherwise.
    i915.uncore.intel_uncore_forcewake_get(FORCEWAKE_ALL);

    linux::say("not ported yet: intel_gt_init_scratch");

    crate::intel_gt_pm::intel_gt_pm_init(i915, gt);

    linux::say("not ported yet: kernel_vm (i915_ppgtt_create)");

    crate::intel_mocs::intel_set_mocs_index(gt);

    linux::say("not ported yet: intel_engines_init");

    let err = crate::intel_gt_pm::intel_gt_resume(i915, gt);

    if err.is_ok() {
        linux::say("not ported yet: __engines_record_defaults");
    } else {
        // err_uc_init, err_engines (nothing set up yet), err_pm.
        crate::intel_gt_pm::intel_gt_pm_fini(i915, gt);
        intel_gt_set_wedged_on_init(i915, gt);
    }

    i915.uncore.intel_uncore_forcewake_put(FORCEWAKE_ALL);

    crate::intel_rc6::report(i915, gt, "after init");
    crate::intel_rps::report(i915, gt, "after init");
    if let Err(e) = err {
        Line::new().s("intel_gt_init failed: ").dec((-e) as u64).say();
    }
    err
}

/// `__intel_gt_disable`.
fn __intel_gt_disable(i915: &mut drm_i915_private, gt: &mut intel_gt) {
    intel_gt_set_wedged_on_fini(i915, gt);

    crate::intel_gt_pm::intel_gt_suspend_prepare(i915, gt);
    crate::intel_gt_pm::intel_gt_suspend_late(gt);

    if crate::intel_gt_pm::intel_gt_pm_is_awake(gt) {
        linux::say("warning: GT awake after disable");
    }
}

/// `intel_gt_driver_remove`, then of `intel_gt_driver_release` what
/// touches the hardware. The card is handed back with the display still
/// scanning out, so the GT is stopped like a suspend-to-idle
/// (`intel_gt_suspend_late`), not like an unbind: `intel_gt_pm_fini`
/// (`intel_rc6_fini`) is left out, because with RC6 off and no driver
/// left nothing would ever let the GT idle again. RC6 stays armed and
/// parked, RPS at its idle frequency.
pub fn intel_gt_driver_remove(i915: &mut drm_i915_private, gt: &mut intel_gt,
                              _ggtt: &mut crate::intel_ggtt::i915_ggtt) {
    __intel_gt_disable(i915, gt);

    if gt.engines_initialized {
        linux::say("not ported yet: intel_engines_release");
    }

    crate::intel_rc6::report(i915, gt, "at hand-back");
    crate::intel_rps::report(i915, gt, "at hand-back");
}
