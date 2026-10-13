//! display/intel_vdsc.c (Linux 6.18.55): the DSC state readout of a pipe on
//! display version 12, where every pipe has its own DSC engine
//! (`is_pipe_dsc`).

use crate::i915_drv::drm_i915_private;
use crate::intel_de::*;
use crate::intel_display_power::*;
use crate::regs::*;

/// `ICL_PIPE_DSS_CTL1(pipe)`; the macro indexes from pipe B, and GCC's
/// value for pipe A is generated separately.
pub fn ICL_PIPE_DSS_CTL1_of(pipe: u32) -> u32 {
    if pipe == PIPE_A { ICL_PIPE_DSS_CTL1_A } else { ICL_PIPE_DSS_CTL1(pipe) }
}

/// `ICL_PIPE_DSS_CTL2(pipe)`.
pub fn ICL_PIPE_DSS_CTL2_of(pipe: u32) -> u32 {
    if pipe == PIPE_A { ICL_PIPE_DSS_CTL2_A } else { ICL_PIPE_DSS_CTL2(pipe) }
}

/// `intel_dsc_source_support`: TGL has DSC on every transcoder when the
/// fuses leave it on.
fn intel_dsc_source_support(i915: &drm_i915_private) -> bool {
    i915.display.runtime.has_dsc
}

/// `intel_dsc_power_domain`: on display version 12 (not RKL) pipe A's
/// DSC/joiner lives in PW2, every other pipe's in the pipe's own well.
pub fn intel_dsc_power_domain(pipe: u32, _cpu_transcoder: u32) -> u32 {
    if pipe == PIPE_A {
        POWER_DOMAIN_TRANSCODER_VDSC_PW2
    } else {
        // is_pipe_dsc is true from display version 12 on.
        POWER_DOMAIN_PIPE(pipe)
    }
}

/// `intel_dsc_get_config`. `intel_dsc_get_pps_config` (the PPS readout into
/// `struct drm_dsc_config`) is not ported; a pipe with compression on is
/// reported.
pub fn intel_dsc_get_config(i915: &mut drm_i915_private, c: usize) {
    if !intel_dsc_source_support(i915) {
        return;
    }
    let pipe = i915.display.crtcs[c].pipe;
    let cpu_transcoder = i915.display.crtcs[c].state.cpu_transcoder;
    let power_domain = intel_dsc_power_domain(pipe, cpu_transcoder);

    if !intel_display_power_get_if_enabled(i915, power_domain) {
        return;
    }

    let dss_ctl1 = intel_de_read(i915, ICL_PIPE_DSS_CTL1_of(pipe));
    let dss_ctl2 = intel_de_read(i915, ICL_PIPE_DSS_CTL2_of(pipe));

    let compression_enable = dss_ctl2 & VDSC0_ENABLE != 0;
    i915.display.crtcs[c].state.dsc.compression_enable = compression_enable;
    if compression_enable {
        let num_streams = if dss_ctl1 & JOINER_ENABLE != 0
            && dss_ctl2 & (VDSC2_ENABLE | SMALL_JOINER_CONFIG_3_ENGINES) != 0
        {
            3
        } else if dss_ctl1 & JOINER_ENABLE != 0 && dss_ctl2 & VDSC1_ENABLE != 0 {
            2
        } else {
            1
        };
        i915.display.crtcs[c].state.dsc.num_streams = num_streams;
        crate::linux::say("warning: DSC is on, intel_dsc_get_pps_config is not ported");
    }

    intel_display_power_put(i915, power_domain);
}
