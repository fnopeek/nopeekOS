//! display/intel_display_device.c `intel_display_device_info_runtime_init`
//! and display/intel_cdclk.c `intel_read_rawclk` (Linux 6.18.55), for a
//! Tiger Lake display (version 12, PCH TGP).
//!
//! `intel_de_read`/`intel_de_write` take the DMC wakelock only when
//! `enable_dmc_wl` is set, which `intel_dmc_wl_sanitize_param` leaves off
//! below display version 20; here they are plain uncore accesses.

use crate::i915_drv::drm_i915_private;
use crate::linux::Line;
use crate::regs::*;

/// `struct intel_display_runtime_info`, the fields the probe fills.
#[derive(Clone, Copy, Default)]
pub struct intel_display_runtime_info {
    pub pipe_mask: u32,
    pub cpu_transcoder_mask: u32,
    pub fbc_mask: u32,
    pub port_mask: u32,
    pub has_dmc: bool,
    pub has_dsc: bool,
    pub has_hdcp: bool,
    pub num_scalers: [u32; I915_MAX_PIPES as usize],
    pub num_sprites: [u32; I915_MAX_PIPES as usize],
    pub rawclk_freq: u32,
}

/// The `__runtime_defaults` of `tgl_desc` (XE_D_DISPLAY).
pub fn tgl_runtime_defaults() -> intel_display_runtime_info {
    intel_display_runtime_info {
        pipe_mask: TGL_DISPLAY_PIPE_MASK,
        cpu_transcoder_mask: TGL_DISPLAY_TRANSCODER_MASK,
        fbc_mask: TGL_DISPLAY_FBC_MASK,
        port_mask: TGL_DISPLAY_PORT_MASK,
        has_dmc: TGL_DISPLAY_HAS_DMC != 0,
        has_dsc: TGL_DISPLAY_HAS_DSC != 0,
        has_hdcp: TGL_DISPLAY_HAS_HDCP != 0,
        ..Default::default()
    }
}

/// `cnp_rawclk`: reads the strap, programs PCH_RAWCLK_FREQ, returns kHz.
fn cnp_rawclk(i915: &mut drm_i915_private, pch_type: i32) -> u32 {
    let (divider, fraction): (u32, u32) =
        if i915.uncore.intel_uncore_read(SFUSE_STRAP) & SFUSE_STRAP_RAW_FREQUENCY != 0 {
            (24000, 0)
        } else {
            (19000, 200)
        };
    let mut rawclk = CNP_RAWCLK_DIV(divider / 1000);
    if fraction != 0 {
        let numerator = 1u32;
        // DIV_ROUND_CLOSEST(numerator * 1000, fraction) - 1
        rawclk |= CNP_RAWCLK_DEN((numerator * 1000 + fraction / 2) / fraction - 1);
        if pch_type >= PCH_ICP as i32 {
            rawclk |= ICP_RAWCLK_NUM(numerator);
        }
    }
    i915.uncore.intel_uncore_write(PCH_RAWCLK_FREQ, rawclk);
    divider + fraction
}

/// `intel_read_rawclk`: PCH TGP is at least CNP and below the fake DG1 PCH.
fn intel_read_rawclk(i915: &mut drm_i915_private, pch_type: i32) -> u32 {
    if pch_type >= PCH_CNP as i32 {
        cnp_rawclk(i915, pch_type)
    } else {
        Line::new().s("warning: rawclk for PCH type ").dec(pch_type as u64).s(" not ported").say();
        0
    }
}

/// `__intel_display_device_info_runtime_init` for display version 12.
pub fn intel_display_device_info_runtime_init(i915: &mut drm_i915_private,
                                              rt: &mut intel_display_runtime_info, pch_type: i32) {
    for pipe in 0..I915_MAX_PIPES {
        if rt.pipe_mask & (1 << pipe) != 0 {
            rt.num_scalers[pipe as usize] = 2;
            rt.num_sprites[pipe as usize] = 6;
        }
    }

    let dfsm = i915.uncore.intel_uncore_read(SKL_DFSM);
    if dfsm & SKL_DFSM_PIPE_A_DISABLE != 0 {
        rt.pipe_mask &= !(1 << PIPE_A);
        rt.cpu_transcoder_mask &= !(1 << TRANSCODER_A);
        rt.fbc_mask &= !(1 << INTEL_FBC_A);
    }
    if dfsm & SKL_DFSM_PIPE_B_DISABLE != 0 {
        rt.pipe_mask &= !(1 << PIPE_B);
        rt.cpu_transcoder_mask &= !(1 << TRANSCODER_B);
        rt.fbc_mask &= !(1 << INTEL_FBC_B);
    }
    if dfsm & SKL_DFSM_PIPE_C_DISABLE != 0 {
        rt.pipe_mask &= !(1 << PIPE_C);
        rt.cpu_transcoder_mask &= !(1 << TRANSCODER_C);
        rt.fbc_mask &= !(1 << INTEL_FBC_C);
    }
    if dfsm & TGL_DFSM_PIPE_D_DISABLE != 0 {
        rt.pipe_mask &= !(1 << PIPE_D);
        rt.cpu_transcoder_mask &= !(1 << TRANSCODER_D);
        rt.fbc_mask &= !(1 << INTEL_FBC_D);
    }
    if rt.pipe_mask == 0 {
        *rt = intel_display_runtime_info::default();
        return;
    }
    if dfsm & SKL_DFSM_DISPLAY_HDCP_DISABLE != 0 {
        rt.has_hdcp = false;
    }
    if dfsm & SKL_DFSM_DISPLAY_PM_DISABLE != 0 {
        rt.fbc_mask = 0;
    }
    if dfsm & ICL_DFSM_DMC_DISABLE != 0 {
        rt.has_dmc = false;
    }
    if dfsm & GLK_DFSM_DISPLAY_DSC_DISABLE != 0 {
        rt.has_dsc = false;
    }

    rt.rawclk_freq = intel_read_rawclk(i915, pch_type);
}
