//! display/intel_color.c (Linux 6.18.55): the colour state readout of
//! display version 12 (`tgl_color_funcs`: `skl_get_config`, `icl_read_luts`,
//! `icl_read_csc`), and the commit hooks the crtc sanitize writes back
//! (`icl_color_commit_noarm`, `icl_color_commit_arm`).
//!
//! `icl_read_luts` copies the gamma tables into property blobs that only
//! the state checker compares; that copy is not ported, the modes that
//! select the tables are.

use crate::i915_drv::drm_i915_private;
use crate::intel_de::*;
use crate::intel_display_types::intel_csc_matrix;
use crate::regs::*;

/// `hsw_read_gamma_mode`.
fn hsw_read_gamma_mode(i915: &mut drm_i915_private, pipe: u32) -> u32 {
    intel_de_read(i915, GAMMA_MODE(pipe))
}

/// `ilk_read_csc_mode`.
fn ilk_read_csc_mode(i915: &mut drm_i915_private, pipe: u32) -> u32 {
    intel_de_read(i915, PIPE_CSC_MODE(pipe))
}

/// `skl_get_config`.
fn skl_get_config(i915: &mut drm_i915_private, c: usize) {
    let pipe = i915.display.crtcs[c].pipe;
    let gamma_mode = hsw_read_gamma_mode(i915, pipe);
    let csc_mode = ilk_read_csc_mode(i915, pipe);
    let tmp = intel_de_read(i915, SKL_BOTTOM_COLOR(pipe));

    let state = &mut i915.display.crtcs[c].state;
    state.gamma_mode = gamma_mode;
    state.csc_mode = csc_mode;
    if tmp & SKL_BOTTOM_COLOR_GAMMA_ENABLE != 0 {
        state.gamma_enable = true;
    }
    if tmp & SKL_BOTTOM_COLOR_CSC_ENABLE != 0 {
        state.csc_enable = true;
    }
}

/// `ilk_read_pipe_csc` (display version 7+).
fn ilk_read_pipe_csc(i915: &mut drm_i915_private, pipe: u32) -> intel_csc_matrix {
    let mut csc = intel_csc_matrix::default();
    let u = &i915.uncore;

    csc.preoff[0] = u.intel_uncore_read_fw(PIPE_CSC_PREOFF_HI(pipe)) as u16;
    csc.preoff[1] = u.intel_uncore_read_fw(PIPE_CSC_PREOFF_ME(pipe)) as u16;
    csc.preoff[2] = u.intel_uncore_read_fw(PIPE_CSC_PREOFF_LO(pipe)) as u16;

    let tmp = u.intel_uncore_read_fw(PIPE_CSC_COEFF_RY_GY(pipe));
    csc.coeff[0] = (tmp >> 16) as u16;
    csc.coeff[1] = (tmp & 0xffff) as u16;
    let tmp = u.intel_uncore_read_fw(PIPE_CSC_COEFF_BY(pipe));
    csc.coeff[2] = (tmp >> 16) as u16;

    let tmp = u.intel_uncore_read_fw(PIPE_CSC_COEFF_RU_GU(pipe));
    csc.coeff[3] = (tmp >> 16) as u16;
    csc.coeff[4] = (tmp & 0xffff) as u16;
    let tmp = u.intel_uncore_read_fw(PIPE_CSC_COEFF_BU(pipe));
    csc.coeff[5] = (tmp >> 16) as u16;

    let tmp = u.intel_uncore_read_fw(PIPE_CSC_COEFF_RV_GV(pipe));
    csc.coeff[6] = (tmp >> 16) as u16;
    csc.coeff[7] = (tmp & 0xffff) as u16;
    let tmp = u.intel_uncore_read_fw(PIPE_CSC_COEFF_BV(pipe));
    csc.coeff[8] = (tmp >> 16) as u16;

    csc.postoff[0] = u.intel_uncore_read_fw(PIPE_CSC_POSTOFF_HI(pipe)) as u16;
    csc.postoff[1] = u.intel_uncore_read_fw(PIPE_CSC_POSTOFF_ME(pipe)) as u16;
    csc.postoff[2] = u.intel_uncore_read_fw(PIPE_CSC_POSTOFF_LO(pipe)) as u16;
    csc
}

/// `icl_read_output_csc`.
fn icl_read_output_csc(i915: &mut drm_i915_private, pipe: u32) -> intel_csc_matrix {
    let mut csc = intel_csc_matrix::default();
    let u = &i915.uncore;

    csc.preoff[0] = u.intel_uncore_read_fw(PIPE_CSC_OUTPUT_PREOFF_HI(pipe)) as u16;
    csc.preoff[1] = u.intel_uncore_read_fw(PIPE_CSC_OUTPUT_PREOFF_ME(pipe)) as u16;
    csc.preoff[2] = u.intel_uncore_read_fw(PIPE_CSC_OUTPUT_PREOFF_LO(pipe)) as u16;

    let tmp = u.intel_uncore_read_fw(PIPE_CSC_OUTPUT_COEFF_RY_GY(pipe));
    csc.coeff[0] = (tmp >> 16) as u16;
    csc.coeff[1] = (tmp & 0xffff) as u16;
    let tmp = u.intel_uncore_read_fw(PIPE_CSC_OUTPUT_COEFF_BY(pipe));
    csc.coeff[2] = (tmp >> 16) as u16;

    let tmp = u.intel_uncore_read_fw(PIPE_CSC_OUTPUT_COEFF_RU_GU(pipe));
    csc.coeff[3] = (tmp >> 16) as u16;
    csc.coeff[4] = (tmp & 0xffff) as u16;
    let tmp = u.intel_uncore_read_fw(PIPE_CSC_OUTPUT_COEFF_BU(pipe));
    csc.coeff[5] = (tmp >> 16) as u16;

    let tmp = u.intel_uncore_read_fw(PIPE_CSC_OUTPUT_COEFF_RV_GV(pipe));
    csc.coeff[6] = (tmp >> 16) as u16;
    csc.coeff[7] = (tmp & 0xffff) as u16;
    let tmp = u.intel_uncore_read_fw(PIPE_CSC_OUTPUT_COEFF_BV(pipe));
    csc.coeff[8] = (tmp >> 16) as u16;

    csc.postoff[0] = u.intel_uncore_read_fw(PIPE_CSC_OUTPUT_POSTOFF_HI(pipe)) as u16;
    csc.postoff[1] = u.intel_uncore_read_fw(PIPE_CSC_OUTPUT_POSTOFF_ME(pipe)) as u16;
    csc.postoff[2] = u.intel_uncore_read_fw(PIPE_CSC_OUTPUT_POSTOFF_LO(pipe)) as u16;
    csc
}

/// `icl_read_csc`.
fn icl_read_csc(i915: &mut drm_i915_private, c: usize) {
    let pipe = i915.display.crtcs[c].pipe;
    let csc_mode = i915.display.crtcs[c].state.csc_mode;

    // Wa_1406463849: icl (see skl_read_csc)
    if csc_mode & ICL_CSC_ENABLE as u32 != 0 {
        i915.display.crtcs[c].state.csc = ilk_read_pipe_csc(i915, pipe);
    }
    if csc_mode & ICL_OUTPUT_CSC_ENABLE != 0 {
        i915.display.crtcs[c].state.output_csc = icl_read_output_csc(i915, pipe);
    }
}

/// `intel_color_get_config`.
pub fn intel_color_get_config(i915: &mut drm_i915_private, c: usize) {
    skl_get_config(i915, c);
    icl_read_csc(i915, c);
}

/// `ilk_update_pipe_csc` (display version 7+).
fn ilk_update_pipe_csc(i915: &mut drm_i915_private, pipe: u32, csc: &intel_csc_matrix) {
    intel_de_write(i915, PIPE_CSC_PREOFF_HI(pipe), csc.preoff[0] as u32);
    intel_de_write(i915, PIPE_CSC_PREOFF_ME(pipe), csc.preoff[1] as u32);
    intel_de_write(i915, PIPE_CSC_PREOFF_LO(pipe), csc.preoff[2] as u32);

    intel_de_write(i915, PIPE_CSC_COEFF_RY_GY(pipe), (csc.coeff[0] as u32) << 16 | csc.coeff[1] as u32);
    intel_de_write(i915, PIPE_CSC_COEFF_BY(pipe), (csc.coeff[2] as u32) << 16);

    intel_de_write(i915, PIPE_CSC_COEFF_RU_GU(pipe), (csc.coeff[3] as u32) << 16 | csc.coeff[4] as u32);
    intel_de_write(i915, PIPE_CSC_COEFF_BU(pipe), (csc.coeff[5] as u32) << 16);

    intel_de_write(i915, PIPE_CSC_COEFF_RV_GV(pipe), (csc.coeff[6] as u32) << 16 | csc.coeff[7] as u32);
    intel_de_write(i915, PIPE_CSC_COEFF_BV(pipe), (csc.coeff[8] as u32) << 16);

    intel_de_write(i915, PIPE_CSC_POSTOFF_HI(pipe), csc.postoff[0] as u32);
    intel_de_write(i915, PIPE_CSC_POSTOFF_ME(pipe), csc.postoff[1] as u32);
    intel_de_write(i915, PIPE_CSC_POSTOFF_LO(pipe), csc.postoff[2] as u32);
}

/// `icl_update_output_csc`.
fn icl_update_output_csc(i915: &mut drm_i915_private, pipe: u32, csc: &intel_csc_matrix) {
    intel_de_write(i915, PIPE_CSC_OUTPUT_PREOFF_HI(pipe), csc.preoff[0] as u32);
    intel_de_write(i915, PIPE_CSC_OUTPUT_PREOFF_ME(pipe), csc.preoff[1] as u32);
    intel_de_write(i915, PIPE_CSC_OUTPUT_PREOFF_LO(pipe), csc.preoff[2] as u32);

    intel_de_write(i915, PIPE_CSC_OUTPUT_COEFF_RY_GY(pipe), (csc.coeff[0] as u32) << 16 | csc.coeff[1] as u32);
    intel_de_write(i915, PIPE_CSC_OUTPUT_COEFF_BY(pipe), (csc.coeff[2] as u32) << 16);

    intel_de_write(i915, PIPE_CSC_OUTPUT_COEFF_RU_GU(pipe), (csc.coeff[3] as u32) << 16 | csc.coeff[4] as u32);
    intel_de_write(i915, PIPE_CSC_OUTPUT_COEFF_BU(pipe), (csc.coeff[5] as u32) << 16);

    intel_de_write(i915, PIPE_CSC_OUTPUT_COEFF_RV_GV(pipe), (csc.coeff[6] as u32) << 16 | csc.coeff[7] as u32);
    intel_de_write(i915, PIPE_CSC_OUTPUT_COEFF_BV(pipe), (csc.coeff[8] as u32) << 16);

    intel_de_write(i915, PIPE_CSC_OUTPUT_POSTOFF_HI(pipe), csc.postoff[0] as u32);
    intel_de_write(i915, PIPE_CSC_OUTPUT_POSTOFF_ME(pipe), csc.postoff[1] as u32);
    intel_de_write(i915, PIPE_CSC_OUTPUT_POSTOFF_LO(pipe), csc.postoff[2] as u32);
}

/// `intel_color_commit_noarm` → `icl_color_commit_noarm` →
/// `icl_load_csc_matrix`.
pub fn intel_color_commit_noarm(i915: &mut drm_i915_private, c: usize) {
    let crtc = i915.display.crtcs[c];
    let cs = &crtc.state;
    if cs.csc_mode & ICL_CSC_ENABLE as u32 != 0 {
        ilk_update_pipe_csc(i915, crtc.pipe, &cs.csc);
    }
    if cs.csc_mode & ICL_OUTPUT_CSC_ENABLE != 0 {
        icl_update_output_csc(i915, crtc.pipe, &cs.output_csc);
    }
}

/// `intel_color_commit_arm` → `icl_color_commit_arm`: the pipe background
/// is forced to black, gamma and CSC modes are written from the state.
pub fn intel_color_commit_arm(i915: &mut drm_i915_private, c: usize) {
    let crtc = i915.display.crtcs[c];
    intel_de_write(i915, SKL_BOTTOM_COLOR(crtc.pipe), 0);
    intel_de_write(i915, GAMMA_MODE(crtc.pipe), crtc.state.gamma_mode);
    intel_de_write(i915, PIPE_CSC_MODE(crtc.pipe), crtc.state.csc_mode);
}
