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

/// `ICL_DSC0_PPS(pipe, pps)` / `ICL_DSC1_PPS(pipe, pps)`: the macros index
/// from pipe B, and GCC's value for each pipe is generated separately.
fn icl_dsc_pps(engine: usize, pipe: u32, pps: u32) -> u32 {
    let base = match (engine, pipe) {
        (0, 0) => ICL_DSC0_PPS_0_A,
        (0, 1) => ICL_DSC0_PPS_0_B,
        (0, 2) => ICL_DSC0_PPS_0_C,
        (0, _) => ICL_DSC0_PPS_0_D,
        (_, 0) => ICL_DSC1_PPS_0_A,
        (_, 1) => ICL_DSC1_PPS_0_B,
        (_, 2) => ICL_DSC1_PPS_0_C,
        _ => ICL_DSC1_PPS_0_D,
    };
    base + pps * 4
}

/// `intel_dsc_get_vdsc_per_pipe`.
fn intel_dsc_get_vdsc_per_pipe(cs: &crate::intel_display_types::intel_crtc_state) -> i32 {
    cs.dsc.num_streams
}

/// `intel_dsc_get_num_vdsc_instances`.
fn intel_dsc_get_num_vdsc_instances(cs: &crate::intel_display_types::intel_crtc_state, pipe: u32) -> i32 {
    intel_dsc_get_vdsc_per_pipe(cs) * crate::intel_display::intel_crtc_num_joined_pipes(cs, pipe)
}

/// `intel_dsc_pps_read_and_verify` (`intel_dsc_pps_read` with
/// `intel_dsc_get_pps_reg`): every engine of the pipe must hold the same.
/// Display version 12 has no third engine (`BMG_DSC2_PPS`).
fn intel_dsc_pps_read_and_verify(i915: &mut drm_i915_private, cs: &crate::intel_display_types::intel_crtc_state,
                                 pipe: u32, pps: u32) -> u32 {
    let vdsc_per_pipe = intel_dsc_get_vdsc_per_pipe(cs);
    let dsc_reg_num = vdsc_per_pipe.clamp(0, 2) as usize;
    if dsc_reg_num < vdsc_per_pipe as usize {
        crate::linux::say("warning: more DSC engines on a pipe than display version 12 has");
    }
    let val = intel_de_read(i915, icl_dsc_pps(0, pipe, pps));
    for e in 1..dsc_reg_num {
        if intel_de_read(i915, icl_dsc_pps(e, pipe, pps)) != val {
            crate::linux::say("warning: the DSC engines of a pipe hold different PPS");
            break;
        }
    }
    val
}

/// `intel_dsc_get_pps_config`.
fn intel_dsc_get_pps_config(i915: &mut drm_i915_private, c: usize) {
    let pipe = i915.display.crtcs[c].pipe;
    let cs = i915.display.crtcs[c].state;
    let num_vdsc_instances = intel_dsc_get_num_vdsc_instances(&cs, pipe);
    let get = |mask: u32, v: u32| crate::intel_gt::reg_field_get(mask, v);
    let mut cfg = cs.dsc.config;

    // PPS 0
    let p = intel_dsc_pps_read_and_verify(i915, &cs, pipe, 0);
    cfg.bits_per_component = get(DSC_PPS0_BPC_MASK, p) as u8;
    cfg.line_buf_depth = get(DSC_PPS0_LINE_BUF_DEPTH_MASK, p) as u8;
    cfg.block_pred_enable = p & DSC_PPS0_BLOCK_PREDICTION != 0;
    cfg.convert_rgb = p & DSC_PPS0_COLOR_SPACE_CONVERSION != 0;
    cfg.simple_422 = p & DSC_PPS0_422_ENABLE != 0;
    cfg.native_422 = p & DSC_PPS0_NATIVE_422_ENABLE != 0;
    cfg.native_420 = p & DSC_PPS0_NATIVE_420_ENABLE != 0;
    cfg.vbr_enable = p & DSC_PPS0_VBR_ENABLE != 0;

    // PPS 1
    let p = intel_dsc_pps_read_and_verify(i915, &cs, pipe, 1);
    cfg.bits_per_pixel = get(DSC_PPS1_BPP_MASK, p) as u16;
    if cfg.native_420 {
        cfg.bits_per_pixel >>= 1;
    }
    let compressed_bpp_x16 = cfg.bits_per_pixel;

    // PPS 2
    let p = intel_dsc_pps_read_and_verify(i915, &cs, pipe, 2);
    cfg.pic_width = (get(DSC_PPS2_PIC_WIDTH_MASK, p) as i32 * num_vdsc_instances) as u16;
    cfg.pic_height = get(DSC_PPS2_PIC_HEIGHT_MASK, p) as u16;

    // PPS 3
    let p = intel_dsc_pps_read_and_verify(i915, &cs, pipe, 3);
    cfg.slice_width = get(DSC_PPS3_SLICE_WIDTH_MASK, p) as u16;
    cfg.slice_height = get(DSC_PPS3_SLICE_HEIGHT_MASK, p) as u16;

    // PPS 4
    let p = intel_dsc_pps_read_and_verify(i915, &cs, pipe, 4);
    cfg.initial_dec_delay = get(DSC_PPS4_INITIAL_DEC_DELAY_MASK, p) as u16;
    cfg.initial_xmit_delay = get(DSC_PPS4_INITIAL_XMIT_DELAY_MASK, p) as u16;

    // PPS 5
    let p = intel_dsc_pps_read_and_verify(i915, &cs, pipe, 5);
    cfg.scale_decrement_interval = get(DSC_PPS5_SCALE_DEC_INT_MASK, p) as u16;
    cfg.scale_increment_interval = get(DSC_PPS5_SCALE_INC_INT_MASK, p) as u16;

    // PPS 6
    let p = intel_dsc_pps_read_and_verify(i915, &cs, pipe, 6);
    cfg.initial_scale_value = get(DSC_PPS6_INITIAL_SCALE_VALUE_MASK, p) as u16;
    cfg.first_line_bpg_offset = get(DSC_PPS6_FIRST_LINE_BPG_OFFSET_MASK, p) as u8;
    cfg.flatness_min_qp = get(DSC_PPS6_FLATNESS_MIN_QP_MASK, p) as u8;
    cfg.flatness_max_qp = get(DSC_PPS6_FLATNESS_MAX_QP_MASK, p) as u8;

    // PPS 7
    let p = intel_dsc_pps_read_and_verify(i915, &cs, pipe, 7);
    cfg.nfl_bpg_offset = get(DSC_PPS7_NFL_BPG_OFFSET_MASK, p) as u16;
    cfg.slice_bpg_offset = get(DSC_PPS7_SLICE_BPG_OFFSET_MASK, p) as u16;

    // PPS 8
    let p = intel_dsc_pps_read_and_verify(i915, &cs, pipe, 8);
    cfg.initial_offset = get(DSC_PPS8_INITIAL_OFFSET_MASK, p) as u16;
    cfg.final_offset = get(DSC_PPS8_FINAL_OFFSET_MASK, p) as u16;

    // PPS 9
    let p = intel_dsc_pps_read_and_verify(i915, &cs, pipe, 9);
    cfg.rc_model_size = get(DSC_PPS9_RC_MODEL_SIZE_MASK, p) as u16;

    // PPS 10
    let p = intel_dsc_pps_read_and_verify(i915, &cs, pipe, 10);
    cfg.rc_quant_incr_limit0 = get(DSC_PPS10_RC_QUANT_INC_LIMIT0_MASK, p) as u8;
    cfg.rc_quant_incr_limit1 = get(DSC_PPS10_RC_QUANT_INC_LIMIT1_MASK, p) as u8;

    // PPS 16
    let p = intel_dsc_pps_read_and_verify(i915, &cs, pipe, 16);
    cfg.slice_chunk_size = get(DSC_PPS16_SLICE_CHUNK_SIZE_MASK, p) as u16;

    let st = &mut i915.display.crtcs[c].state.dsc;
    st.config = cfg;
    st.compressed_bpp_x16 = compressed_bpp_x16;
}

/// `intel_dsc_get_config`.
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
        intel_dsc_get_pps_config(i915, c);
    }

    intel_display_power_put(i915, power_domain);
}
