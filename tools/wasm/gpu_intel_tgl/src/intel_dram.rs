//! soc/intel_dram.c (Linux 6.18.55): eDRAM and DRAM info, Tiger Lake.

use crate::i915_drv::drm_i915_private;
use crate::intel_pcode;
use crate::linux::{self, Line};
use crate::regs::*;

/// `struct dram_info`, as `icl_pcode_read_mem_global_info` fills it.
#[derive(Clone, Copy, Default)]
pub struct dram_info {
    /// `enum intel_dram_type`.
    pub ty: u32,
    pub type_name: &'static str,
    pub num_channels: u32,
    pub num_qgv_points: u32,
    pub num_psf_gv_points: u32,
}

/// `intel_dram_edram_detect`. The size computation (`gen9_edram_size_mb`)
/// is not ported: Tiger Lake has no eDRAM, and a set enable bit is reported.
pub fn intel_dram_edram_detect(i915: &mut drm_i915_private) {
    let edram_cap = i915.uncore.intel_uncore_read_fw(HSW_EDRAM_CAP);
    if edram_cap & EDRAM_ENABLED == 0 {
        return;
    }
    Line::new().s("eDRAM reported enabled (cap ").hex(edram_cap).s("), size not computed").say();
}

/// `icl_pcode_read_mem_global_info`, GRAPHICS_VER 12.
fn icl_pcode_read_mem_global_info(i915: &mut drm_i915_private, info: &mut dram_info) -> Result<(), i32> {
    let mut val = 0u32;
    intel_pcode::snb_pcode_read(i915, ICL_PCODE_MEM_SUBSYSYSTEM_INFO | ICL_PCODE_MEM_SS_READ_GLOBAL_INFO,
                                &mut val, None)?;
    (info.ty, info.type_name) = match val & 0xf {
        0 => (INTEL_DRAM_DDR4, "DDR4"),
        1 => (INTEL_DRAM_DDR5, "DDR5"),
        2 => (INTEL_DRAM_LPDDR5, "LPDDR5"),
        3 => (INTEL_DRAM_LPDDR4, "LPDDR4"),
        4 => (INTEL_DRAM_DDR3, "DDR3"),
        5 => (INTEL_DRAM_LPDDR3, "LPDDR3"),
        other => {
            Line::new().s("warning: missing case DRAM type ").dec(other as u64).say();
            return Err(-(EINVAL as i32));
        }
    };
    info.num_channels = (val & 0xf0) >> 4;
    info.num_qgv_points = (val & 0xf00) >> 8;
    info.num_psf_gv_points = (val & 0x3000) >> 12;
    Ok(())
}

/// `intel_dram_detect`: `gen12_get_dram_info`. A failure leaves the info
/// empty and does not fail the probe.
pub fn intel_dram_detect(i915: &mut drm_i915_private) -> dram_info {
    let mut info = dram_info::default();
    if icl_pcode_read_mem_global_info(i915, &mut info).is_err() {
        linux::say("DRAM detection failed");
    }
    info
}
