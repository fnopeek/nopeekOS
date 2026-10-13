//! i915_drv.h, intel_device_info.c, intel_step.c and the start of
//! display/intel_display_device.c (Linux 6.18.55): the device state and
//! the probe steps that need no register access.
//!
//! The device is the one `tgl_info` describes; its values come from
//! i915_pci.c through gen_regs.py (`TGL_*` in regs.rs).

use crate::intel_uncore::intel_uncore;
use crate::linux::Line;
use crate::regs::*;
use crate::tables;

/// `struct intel_ip_version`.
#[derive(Clone, Copy, Default)]
pub struct intel_ip_version {
    pub ver: u32,
    pub rel: u32,
    pub step: u32,
}

/// `struct intel_step_info`.
#[derive(Clone, Copy, Default)]
pub struct intel_step_info {
    pub graphics_step: u32,
    pub media_step: u32,
}

/// `struct intel_runtime_info`.
#[derive(Clone, Copy, Default)]
pub struct intel_runtime_info {
    pub graphics_ip: intel_ip_version,
    pub media_ip: intel_ip_version,
    pub platform_mask: [u32; 2],
    pub device_id: u16,
    pub step: intel_step_info,
    pub page_sizes: u32,
    pub ppgtt_type: u32,
    pub ppgtt_size: u32,
    pub has_pooled_eu: bool,
}

/// The parts of `struct intel_display` the probe sets.
#[derive(Clone, Copy, Default)]
pub struct intel_display {
    pub has_display: bool,
    /// `display->platform.tigerlake` / `.tigerlake_uy`.
    pub platform_tigerlake: bool,
    pub platform_tigerlake_uy: bool,
    pub ip: intel_ip_version,
    pub step: u32,
    pub params: crate::intel_display_power::intel_display_params,
    /// `display->power.domains`.
    pub power: crate::intel_display_power::i915_power_domains,
    /// `intel_dmc_has_payload`: no DMC firmware is loaded by this port.
    pub dmc_has_payload: bool,
    pub cdclk: crate::intel_cdclk::intel_display_cdclk,
    /// `display->dbuf.enabled_slices`.
    pub dbuf_enabled_slices: u8,
    /// The encoders, digital ports and connectors `intel_setup_outputs`
    /// creates.
    pub outputs: crate::intel_encoder::intel_outputs,
}

/// `struct drm_i915_private`, as far as this stage needs it.
pub struct drm_i915_private {
    /// PCI revision (`INTEL_REVID`).
    pub revision: u8,
    pub runtime: intel_runtime_info,
    pub display: intel_display,
    pub uncore: intel_uncore,
    /// `intel_irqs_enabled`.
    pub irqs_enabled: bool,
    /// `i915->dram_info`.
    pub dram_info: crate::intel_dram::dram_info,
}

const BITS_PER_U32: u32 = 32;

/// `__platform_mask_index`.
fn __platform_mask_index(p: u32) -> usize {
    let pbits = BITS_PER_U32 - INTEL_SUBPLATFORM_BITS;
    (p / pbits) as usize
}

/// `__platform_mask_bit`.
fn __platform_mask_bit(p: u32) -> u32 {
    let pbits = BITS_PER_U32 - INTEL_SUBPLATFORM_BITS;
    p % pbits + INTEL_SUBPLATFORM_BITS
}

impl drm_i915_private {
    /// `IS_PLATFORM(i915, p)`.
    pub fn is_platform(&self, p: u32) -> bool {
        self.runtime.platform_mask[__platform_mask_index(p)] & (1 << __platform_mask_bit(p)) != 0
    }

    /// `IS_SUBPLATFORM(i915, p, s)`.
    pub fn is_subplatform(&self, p: u32, s: u32) -> bool {
        let mask = self.runtime.platform_mask[__platform_mask_index(p)];
        let pb = __platform_mask_bit(p);
        mask & (1 << pb) != 0 && mask & (1 << s) != 0
    }

    pub fn graphics_ver(&self) -> u32 {
        self.runtime.graphics_ip.ver
    }

    /// `GRAPHICS_VER_FULL`.
    pub fn graphics_ver_full(&self) -> u32 {
        ip_ver(self.runtime.graphics_ip.ver, self.runtime.graphics_ip.rel)
    }

    /// `IS_GRAPHICS_STEP(i915, since, until)`.
    pub fn is_graphics_step(&self, since: u32, until: u32) -> bool {
        let s = self.runtime.step.graphics_step;
        s >= since && s < until
    }
}

/// `IP_VER(ver, rel)`.
pub const fn ip_ver(ver: u32, rel: u32) -> u32 {
    (ver << 8) | rel
}

/// `intel_device_info_driver_create`: runtime info starts as a copy of
/// `tgl_info.__runtime`, with the device id from PCI.
pub fn intel_device_info_driver_create(device_id: u16) -> intel_runtime_info {
    intel_runtime_info {
        graphics_ip: intel_ip_version { ver: TGL_GRAPHICS_VER, rel: TGL_GRAPHICS_REL, step: 0 },
        // GEN(x) sets the media IP version to the graphics one.
        media_ip: intel_ip_version { ver: TGL_GRAPHICS_VER, rel: TGL_GRAPHICS_REL, step: 0 },
        platform_mask: [0; 2],
        device_id,
        step: intel_step_info::default(),
        page_sizes: TGL_PAGE_SIZES,
        ppgtt_type: TGL_PPGTT_TYPE,
        ppgtt_size: TGL_PPGTT_SIZE,
        has_pooled_eu: false,
    }
}

/// `get_pre_gmdid_step` over the TGL UY map (the subplatform's stepping
/// trumps the platform's).
fn get_pre_gmdid_step(revision: u8) -> u32 {
    let map = &tables::tgl_uy_steppings;
    let size = map.len();
    let mut revision = revision as usize;
    if revision < size && map[revision] != STEP_NONE {
        return map[revision];
    }
    Line::new().s("warning: unknown display revision ").hex(revision as u32).say();
    while revision < size && map[revision] == STEP_NONE {
        revision += 1;
    }
    if revision < size { map[revision] } else { STEP_FUTURE }
}

/// `intel_display_device_probe` for 8086:9A78: `tgl_desc` with its UY
/// subplatform, IP 12.0 from `XE_D_DISPLAY`, stepping from the revision.
/// 9A78 is not in `has_no_display`'s list (IVB Q only).
pub fn intel_display_device_probe(revision: u8) -> intel_display {
    let d = intel_display {
        has_display: true,
        platform_tigerlake: true,
        platform_tigerlake_uy: true,
        ip: intel_ip_version { ver: 12, rel: 0, step: 0 },
        step: get_pre_gmdid_step(revision),
        ..Default::default()
    };
    Line::new().s("Found TIGERLAKE/UY (device ID 9a78) integrated display version 12.00 stepping ")
        .s(intel_step_name(d.step)).say();
    d
}

/// `find_devid`.
fn find_devid(id: u16, p: &[u16]) -> bool {
    p.contains(&id)
}

/// `intel_device_info_subplatform_init`.
fn intel_device_info_subplatform_init(rt: &mut intel_runtime_info) {
    let pi = __platform_mask_index(TGL_PLATFORM);
    let pb = __platform_mask_bit(TGL_PLATFORM);
    let devid = rt.device_id;
    let mut mask = 0u32;

    rt.platform_mask[pi] = 1 << pb;

    if find_devid(devid, &tables::subplatform_ult_ids) {
        mask = 1 << INTEL_SUBPLATFORM_ULT;
    } else if find_devid(devid, &tables::subplatform_ulx_ids) {
        mask = 1 << INTEL_SUBPLATFORM_ULX;
        // ULX is also ULT on Haswell and Broadwell only.
    } else if find_devid(devid, &tables::subplatform_portf_ids) {
        mask = 1 << INTEL_SUBPLATFORM_PORTF;
    } else if find_devid(devid, &tables::subplatform_uy_ids) {
        mask = 1 << INTEL_SUBPLATFORM_UY;
    } else if find_devid(devid, &tables::subplatform_n_ids) {
        mask = 1 << INTEL_SUBPLATFORM_N;
    } else if find_devid(devid, &tables::subplatform_rpl_ids) {
        mask = 1 << INTEL_SUBPLATFORM_RPL;
        if find_devid(devid, &tables::subplatform_rplu_ids) {
            mask |= 1 << INTEL_SUBPLATFORM_RPLU;
        }
    } else if find_devid(devid, &tables::subplatform_g10_ids) {
        mask = 1 << INTEL_SUBPLATFORM_G10;
    } else if find_devid(devid, &tables::subplatform_g11_ids) {
        mask = 1 << INTEL_SUBPLATFORM_G11;
    } else if find_devid(devid, &tables::subplatform_g12_ids) {
        mask = 1 << INTEL_SUBPLATFORM_G12;
    } else if find_devid(devid, &tables::subplatform_arl_h_ids) {
        mask = 1 << INTEL_SUBPLATFORM_ARL_H;
    } else if find_devid(devid, &tables::subplatform_arl_u_ids) {
        mask = 1 << INTEL_SUBPLATFORM_ARL_U;
    } else if find_devid(devid, &tables::subplatform_arl_s_ids) {
        mask = 1 << INTEL_SUBPLATFORM_ARL_S;
    }
    if find_devid(devid, &tables::subplatform_dg2_d_ids) {
        mask |= 1 << INTEL_SUBPLATFORM_D;
    }
    rt.platform_mask[pi] |= mask;
}

/// `intel_ipver_early_init`: no GMD_ID, so media shares the graphics IP.
fn intel_ipver_early_init(rt: &mut intel_runtime_info) {
    if rt.graphics_ip.ver > 12 {
        crate::linux::say("warning: graphics IP above 12 without GMD_ID");
    }
    rt.media_ip = rt.graphics_ip;
}

/// `intel_device_info_runtime_init_early`.
pub fn intel_device_info_runtime_init_early(rt: &mut intel_runtime_info) {
    intel_ipver_early_init(rt);
    intel_device_info_subplatform_init(rt);
}

/// `intel_step_init`, Tiger Lake UY branch (`tgl_uy_revids`).
pub fn intel_step_init(i915: &mut drm_i915_private) {
    let revids = &tables::tgl_uy_revids;
    let size = revids.len();
    let mut revid = i915.revision as usize;
    let step;
    if revid < size && revids[revid].graphics_step != STEP_NONE {
        step = revids[revid];
    } else {
        Line::new().s("warning: unknown revid ").hex(revid as u32).say();
        while revid < size && revids[revid].graphics_step == STEP_NONE {
            revid += 1;
        }
        step = if revid < size {
            revids[revid]
        } else {
            intel_step_info { graphics_step: STEP_FUTURE, media_step: 0 }
        };
    }
    if step.graphics_step == STEP_NONE {
        return;
    }
    i915.runtime.step = step;
}

/// `intel_step_name`: STEP_NAME_LIST runs A0..J3 from 1, four per letter.
pub fn intel_step_name(step: u32) -> &'static str {
    const NAMES: [&str; 40] = [
        "A0", "A1", "A2", "A3", "B0", "B1", "B2", "B3", "C0", "C1", "C2", "C3",
        "D0", "D1", "D2", "D3", "E0", "E1", "E2", "E3", "F0", "F1", "F2", "F3",
        "G0", "G1", "G2", "G3", "H0", "H1", "H2", "H3", "I0", "I1", "I2", "I3",
        "J0", "J1", "J2", "J3",
    ];
    if (STEP_A0..=STEP_J3).contains(&step) {
        NAMES[(step - STEP_A0) as usize]
    } else {
        "**"
    }
}

/// `intel_detect_preproduction_hw`: Tiger Lake below revision 1.
pub fn intel_detect_preproduction_hw(i915: &drm_i915_private) {
    if i915.is_platform(INTEL_TIGERLAKE) && i915.revision < 0x1 {
        crate::linux::drm_err("This is a pre-production stepping. It may not be fully functional.");
    }
}
