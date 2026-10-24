//! display/dc/dpp/dcn10/dcn10_dpp_dscl.c + dcn20_dpp.c's
//! `dscl2_calc_lb_num_partitions` (Linux 6.18.55): the DPP's scaler — mode,
//! recout, MPC size, line buffer, ratios and inits, taps and filter RAM.
//! DCN 2.0 reuses the DCN 1.0 DSCL; its DPP is `dcn20_dpp`.
#![allow(dead_code)]

use crate::amdgpu_device::amdgpu_device;
use crate::dc::dc_debug_options;
use crate::dc_helper::{get_reg_field_value_ex, reg_read, reg_set, reg_update, reg_wait};
use crate::dc_hw_types::{line_buffer_params, rect, scaler_data};
use crate::dcn20_dpp::{dcn20_dpp, dscl_data_processing_format, LB_PIXEL_DEPTH_18BPP, LB_PIXEL_DEPTH_24BPP,
    LB_PIXEL_DEPTH_30BPP, LB_PIXEL_DEPTH_36BPP};
use crate::fixpt31_32::{dc_fixpt_add, dc_fixpt_ceil, dc_fixpt_floor, dc_fixpt_one, dc_fixpt_u0d19, dc_fixpt_u3d19,
    fixed31_32};
use crate::regs::*;

/// `dpp1_dscl_get_pixel_depth_val`.
fn dpp1_dscl_get_pixel_depth_val(depth: u32) -> i32 {
    match depth as i32 {
        LB_PIXEL_DEPTH_30BPP => 0, // 10 bpc
        LB_PIXEL_DEPTH_24BPP => 1, // 8 bpc
        LB_PIXEL_DEPTH_18BPP => 2, // 6 bpc
        LB_PIXEL_DEPTH_36BPP => 3, // 12 bpc
        _ => -1,                   // Unsupported
    }
}

/// `dpp1_dscl_is_video_format`.
fn dpp1_dscl_is_video_format(format: u32) -> bool {
    (PIXEL_FORMAT_VIDEO_BEGIN..=PIXEL_FORMAT_VIDEO_END).contains(&format)
}

/// `dpp1_dscl_is_420_format`.
fn dpp1_dscl_is_420_format(format: u32) -> bool {
    format == PIXEL_FORMAT_420BPP8 || format == PIXEL_FORMAT_420BPP10
}

/// `dpp1_dscl_get_dscl_mode`.
fn dpp1_dscl_get_dscl_mode(dpp: &dcn20_dpp, data: &scaler_data, dbg_always_scale: bool) -> u32 {
    let one = dc_fixpt_one.value;

    if dpp.base.caps.unwrap().dscl_data_proc_format == dscl_data_processing_format::DSCL_DATA_PRCESSING_FIXED_FORMAT {
        // DSCL is processing data in fixed format
        if data.format == PIXEL_FORMAT_FP16 {
            return DSCL_MODE_DSCL_BYPASS;
        }
    }

    if data.ratios.horz.value == one
        && data.ratios.vert.value == one
        && data.ratios.horz_c.value == one
        && data.ratios.vert_c.value == one
        && !dbg_always_scale
    {
        return DSCL_MODE_SCALING_444_BYPASS;
    }

    if !dpp1_dscl_is_420_format(data.format) {
        if dpp1_dscl_is_video_format(data.format) {
            return DSCL_MODE_SCALING_444_YCBCR_ENABLE;
        } else {
            return DSCL_MODE_SCALING_444_RGB_ENABLE;
        }
    }
    if data.ratios.horz.value == one && data.ratios.vert.value == one {
        return DSCL_MODE_SCALING_420_LUMA_BYPASS;
    }
    if data.ratios.horz_c.value == one && data.ratios.vert_c.value == one {
        return DSCL_MODE_SCALING_420_CHROMA_BYPASS;
    }

    DSCL_MODE_SCALING_420_YCBCR_ENABLE
}

/// `dpp1_power_on_dscl`. With `enable_mem_low_power.bits.dscl` the power
/// down is deferred to the next bandwidth optimisation; DCN 2.1 does not
/// set it.
fn dpp1_power_on_dscl(adev: &amdgpu_device, dpp: &dcn20_dpp, debug: &dc_debug_options, power_on: bool) {
    if dpp.tf_regs.unwrap().DSCL_MEM_PWR_CTRL != 0 {
        if power_on {
            reg_update!(adev, dpp.rsm(), DSCL_MEM_PWR_CTRL, LUT_MEM_PWR_FORCE, 0);
            reg_wait!(adev, dpp.rsm(), DSCL_MEM_PWR_STATUS, LUT_MEM_PWR_STATE, 0, 1, 5, "dpp1_power_on_dscl");
        } else if debug.enable_mem_low_power_dscl {
            crate::linux::Line::new().s("dpp1_power_on_dscl: deferred DSCL power down not ported").say();
        } else {
            reg_update!(adev, dpp.rsm(), DSCL_MEM_PWR_CTRL, LUT_MEM_PWR_FORCE, 3);
        }
    }
}

/// `dpp1_dscl_set_lb`.
fn dpp1_dscl_set_lb(adev: &amdgpu_device, dpp: &dcn20_dpp, lb_params: &line_buffer_params, mem_size_config: u32) {
    let mut max_partitions = 63; // Currently hardcoded on all ASICs before DCN 3.2

    // LB
    if dpp.base.caps.unwrap().dscl_data_proc_format == dscl_data_processing_format::DSCL_DATA_PRCESSING_FIXED_FORMAT {
        // DSCL caps: pixel data processed in fixed format. PIXEL_DEPTH,
        // PIXEL_EXPAN_MODE, PIXEL_REDUCE_MODE, DYNAMIC_PIXEL_DEPTH and
        // DITHER_EN are not in DCN 2.0's field list: their masks are 0 and
        // Linux' values for them write nothing.
        let _pixel_depth = dpp1_dscl_get_pixel_depth_val(lb_params.depth);

        reg_set!(adev, dpp.rsm(), LB_DATA_FORMAT, 0,
            INTERLEAVE_EN, lb_params.interleave_en as u32, // Interleave source enable
            LB_DATA_FORMAT__ALPHA_EN, lb_params.alpha_en as u32); // Alpha enable
    } else {
        // DSCL caps: pixel data processed in float format
        reg_set!(adev, dpp.rsm(), LB_DATA_FORMAT, 0,
            INTERLEAVE_EN, lb_params.interleave_en as u32, // Interleave source enable
            LB_DATA_FORMAT__ALPHA_EN, lb_params.alpha_en as u32); // Alpha enable
    }

    if dpp.base.caps.unwrap().max_lb_partitions == 31 {
        max_partitions = 31;
    }

    reg_set!(adev, dpp.rsm(), LB_MEMORY_CTRL, 0, MEMORY_CONFIG, mem_size_config, LB_MAX_PARTITIONS, max_partitions);
}

/// `dpp1_dscl_get_filter_coeffs_64p`.
fn dpp1_dscl_get_filter_coeffs_64p(taps: u32, ratio: fixed31_32) -> Option<&'static [u16]> {
    use crate::dce_scl_filters::*;
    match taps {
        8 => Some(get_filter_8tap_64p(ratio)),
        7 => Some(get_filter_7tap_64p(ratio)),
        6 => Some(get_filter_6tap_64p(ratio)),
        5 => Some(get_filter_5tap_64p(ratio)),
        4 => Some(get_filter_4tap_64p(ratio)),
        3 => Some(get_filter_3tap_64p(ratio)),
        2 => Some(get_filter_2tap_64p()),
        // 1 tap needs no filter; any other count is a bug.
        _ => None,
    }
}

/// `dpp1_dscl_set_scaler_filter`.
fn dpp1_dscl_set_scaler_filter(adev: &amdgpu_device, dpp: &dcn20_dpp, taps: u32, filter_type: u32, filter: &[u16]) {
    let tap_pairs = (taps + 1) / 2;

    reg_set!(adev, dpp.rsm(), SCL_COEF_RAM_TAP_SELECT, 0,
        SCL_COEF_RAM_TAP_PAIR_IDX, 0,
        SCL_COEF_RAM_PHASE, 0,
        SCL_COEF_RAM_FILTER_TYPE, filter_type);

    for phase in 0..(NUM_PHASES / 2 + 1) {
        for pair in 0..tap_pairs {
            let even_coef = filter[(phase * taps + 2 * pair) as usize];
            let odd_coef = if pair * 2 + 1 < taps { filter[(phase * taps + 2 * pair + 1) as usize] } else { 0 };

            reg_set!(adev, dpp.rsm(), SCL_COEF_RAM_TAP_DATA, 0,
                // Even tap coefficient (bits 1:0 fixed to 0)
                SCL_COEF_RAM_EVEN_TAP_COEF, even_coef,
                // Write/read control for even coefficient
                SCL_COEF_RAM_EVEN_TAP_COEF_EN, 1,
                // Odd tap coefficient (bits 1:0 fixed to 0)
                SCL_COEF_RAM_ODD_TAP_COEF, odd_coef,
                // Write/read control for odd coefficient
                SCL_COEF_RAM_ODD_TAP_COEF_EN, 1);
        }
    }
}

/// Pointer identity of two filter tables, as Linux compares the pointers.
fn same_filter(a: Option<&'static [u16]>, b: Option<&'static [u16]>) -> bool {
    match (a, b) {
        (Some(x), Some(y)) => core::ptr::eq(x.as_ptr(), y.as_ptr()),
        (None, None) => true,
        _ => false,
    }
}

/// `dpp1_dscl_set_scl_filter`.
fn dpp1_dscl_set_scl_filter(adev: &amdgpu_device, dpp: &mut dcn20_dpp, scl_data: &scaler_data, chroma_coef_mode: bool) {
    let h_2tap_sharp_factor = scl_data.sharpness.horz as u32;
    let v_2tap_sharp_factor = scl_data.sharpness.vert as u32;
    let mut filter_h_c = None;
    let mut filter_v_c = None;

    let h_2tap_hardcode_coef_en = scl_data.taps.h_taps < 3 && scl_data.taps.h_taps_c < 3
        && (scl_data.taps.h_taps > 1 && scl_data.taps.h_taps_c > 1);
    let v_2tap_hardcode_coef_en = scl_data.taps.v_taps < 3 && scl_data.taps.v_taps_c < 3
        && (scl_data.taps.v_taps > 1 && scl_data.taps.v_taps_c > 1);

    let h_2tap_sharp_en = h_2tap_hardcode_coef_en && h_2tap_sharp_factor != 0;
    let v_2tap_sharp_en = v_2tap_hardcode_coef_en && v_2tap_sharp_factor != 0;

    reg_update!(adev, dpp.rsm(), DSCL_2TAP_CONTROL,
        SCL_H_2TAP_HARDCODE_COEF_EN, h_2tap_hardcode_coef_en as u32,
        SCL_H_2TAP_SHARP_EN, h_2tap_sharp_en as u32,
        SCL_H_2TAP_SHARP_FACTOR, h_2tap_sharp_factor,
        SCL_V_2TAP_HARDCODE_COEF_EN, v_2tap_hardcode_coef_en as u32,
        SCL_V_2TAP_SHARP_EN, v_2tap_sharp_en as u32,
        SCL_V_2TAP_SHARP_FACTOR, v_2tap_sharp_factor);

    if !v_2tap_hardcode_coef_en || !h_2tap_hardcode_coef_en {
        let filter_h = dpp1_dscl_get_filter_coeffs_64p(scl_data.taps.h_taps, scl_data.ratios.horz);
        let filter_v = dpp1_dscl_get_filter_coeffs_64p(scl_data.taps.v_taps, scl_data.ratios.vert);

        let mut filter_updated = (filter_h.is_some() && !same_filter(filter_h, dpp.filter_h))
            || (filter_v.is_some() && !same_filter(filter_v, dpp.filter_v));

        if chroma_coef_mode {
            filter_h_c = dpp1_dscl_get_filter_coeffs_64p(scl_data.taps.h_taps_c, scl_data.ratios.horz_c);
            filter_v_c = dpp1_dscl_get_filter_coeffs_64p(scl_data.taps.v_taps_c, scl_data.ratios.vert_c);
            filter_updated = filter_updated
                || (filter_h_c.is_some() && !same_filter(filter_h_c, dpp.filter_h_c))
                || (filter_v_c.is_some() && !same_filter(filter_v_c, dpp.filter_v_c));
        }

        if filter_updated {
            let scl_mode = reg_read!(adev, dpp.tf_regs.unwrap(), SCL_MODE);

            if !h_2tap_hardcode_coef_en && let Some(f) = filter_h {
                dpp1_dscl_set_scaler_filter(adev, dpp, scl_data.taps.h_taps, SCL_COEF_LUMA_HORZ_FILTER, f);
            }
            dpp.filter_h = filter_h;
            if !v_2tap_hardcode_coef_en && let Some(f) = filter_v {
                dpp1_dscl_set_scaler_filter(adev, dpp, scl_data.taps.v_taps, SCL_COEF_LUMA_VERT_FILTER, f);
            }
            dpp.filter_v = filter_v;
            if chroma_coef_mode {
                if !h_2tap_hardcode_coef_en && let Some(f) = filter_h_c {
                    dpp1_dscl_set_scaler_filter(adev, dpp, scl_data.taps.h_taps_c, SCL_COEF_CHROMA_HORZ_FILTER, f);
                }
                if !v_2tap_hardcode_coef_en && let Some(f) = filter_v_c {
                    dpp1_dscl_set_scaler_filter(adev, dpp, scl_data.taps.v_taps_c, SCL_COEF_CHROMA_VERT_FILTER, f);
                }
            }
            dpp.filter_h_c = filter_h_c;
            dpp.filter_v_c = filter_v_c;

            let coef_ram_current = get_reg_field_value_ex(scl_mode, dpp.tf_mask.unwrap().SCL_COEF_RAM_SELECT_CURRENT,
                dpp.tf_shift.unwrap().SCL_COEF_RAM_SELECT_CURRENT) != 0;

            // Swap coefficient RAM and set chroma coefficient mode
            reg_set!(adev, dpp.rsm(), SCL_MODE, scl_mode,
                SCL_COEF_RAM_SELECT, !coef_ram_current as u32,
                SCL_CHROMA_COEF_MODE, chroma_coef_mode as u32);
        }
    }
}

/// `dscl2_calc_lb_num_partitions` (dcn20_dpp.c).
pub fn dscl2_calc_lb_num_partitions(scl_data: &scaler_data, lb_config: u32, num_part_y: &mut i32, num_part_c: &mut i32) {
    let mut line_size = scl_data.viewport.width.min(scl_data.recout.width);
    let mut line_size_c = scl_data.viewport_c.width.min(scl_data.recout.width);

    if line_size == 0 {
        line_size = 1;
    }

    if line_size_c == 0 {
        line_size_c = 1;
    }

    let memory_line_size_y = (line_size + 5) / 6; // +5 to ceil
    let memory_line_size_c = (line_size_c + 5) / 6; // +5 to ceil
    let memory_line_size_a = (line_size + 5) / 6; // +5 to ceil

    let (lb_memory_size, lb_memory_size_c, lb_memory_size_a) = if lb_config == LB_MEMORY_CONFIG_1 {
        (970, 970, 970)
    } else if lb_config == LB_MEMORY_CONFIG_2 {
        (1290, 1290, 1290)
    } else if lb_config == LB_MEMORY_CONFIG_3 {
        // 420 mode: using 3rd mem from Y, Cr and Cb
        (970 + 1290 + 484 + 484 + 484, 970 + 1290, 970 + 1290 + 484)
    } else {
        (970 + 1290 + 484, 970 + 1290 + 484, 970 + 1290 + 484)
    };
    *num_part_y = lb_memory_size / memory_line_size_y;
    *num_part_c = lb_memory_size_c / memory_line_size_c;
    let num_partitions_a = lb_memory_size_a / memory_line_size_a;

    if scl_data.lb_params.alpha_en && num_partitions_a < *num_part_y {
        *num_part_y = num_partitions_a;
    }

    if *num_part_y > 64 {
        *num_part_y = 64;
    }
    if *num_part_c > 64 {
        *num_part_c = 64;
    }
}

/// `dpp1_dscl_is_lb_conf_valid`.
pub fn dpp1_dscl_is_lb_conf_valid(ceil_vratio: i32, num_partitions: i32, vtaps: i32) -> bool {
    if ceil_vratio > 2 {
        vtaps <= num_partitions - ceil_vratio + 2
    } else {
        vtaps <= num_partitions
    }
}

/// `dpp1_dscl_find_lb_memory_config`.
fn dpp1_dscl_find_lb_memory_config(dpp: &dcn20_dpp, debug: &dc_debug_options, scl_data: &scaler_data) -> u32 {
    let mut num_part_y = 0;
    let mut num_part_c = 0;
    let vtaps = scl_data.taps.v_taps as i32;
    let vtaps_c = scl_data.taps.v_taps_c as i32;
    let ceil_vratio = dc_fixpt_ceil(scl_data.ratios.vert);
    let ceil_vratio_c = dc_fixpt_ceil(scl_data.ratios.vert_c);
    let calc = dpp.base.caps.unwrap().dscl_calc_lb_num_partitions.unwrap();

    if debug.use_max_lb {
        if scl_data.format == PIXEL_FORMAT_420BPP8 || scl_data.format == PIXEL_FORMAT_420BPP10 {
            return LB_MEMORY_CONFIG_3;
        }
        return LB_MEMORY_CONFIG_0;
    }

    calc(scl_data, LB_MEMORY_CONFIG_1, &mut num_part_y, &mut num_part_c);

    if dpp1_dscl_is_lb_conf_valid(ceil_vratio, num_part_y, vtaps)
        && dpp1_dscl_is_lb_conf_valid(ceil_vratio_c, num_part_c, vtaps_c)
    {
        return LB_MEMORY_CONFIG_1;
    }

    calc(scl_data, LB_MEMORY_CONFIG_2, &mut num_part_y, &mut num_part_c);

    if dpp1_dscl_is_lb_conf_valid(ceil_vratio, num_part_y, vtaps)
        && dpp1_dscl_is_lb_conf_valid(ceil_vratio_c, num_part_c, vtaps_c)
    {
        return LB_MEMORY_CONFIG_2;
    }

    if scl_data.format == PIXEL_FORMAT_420BPP8 || scl_data.format == PIXEL_FORMAT_420BPP10 {
        calc(scl_data, LB_MEMORY_CONFIG_3, &mut num_part_y, &mut num_part_c);

        if dpp1_dscl_is_lb_conf_valid(ceil_vratio, num_part_y, vtaps)
            && dpp1_dscl_is_lb_conf_valid(ceil_vratio_c, num_part_c, vtaps_c)
        {
            return LB_MEMORY_CONFIG_3;
        }
    }

    calc(scl_data, LB_MEMORY_CONFIG_0, &mut num_part_y, &mut num_part_c);

    LB_MEMORY_CONFIG_0
}

/// `dpp1_dscl_set_recout`.
fn dpp1_dscl_set_recout(adev: &amdgpu_device, dpp: &dcn20_dpp, recout: &rect) {
    reg_set!(adev, dpp.rsm(), RECOUT_START, 0,
        // First pixel of RECOUT in the active OTG area
        RECOUT_START_X, recout.x,
        // First line of RECOUT in the active OTG area
        RECOUT_START_Y, recout.y);

    reg_set!(adev, dpp.rsm(), RECOUT_SIZE, 0,
        // Number of RECOUT horizontal pixels
        RECOUT_WIDTH, recout.width,
        // Number of RECOUT vertical lines
        RECOUT_HEIGHT, recout.height);
}

/// `dpp1_dscl_set_manual_ratio_init`: the ratios in 3.19, the inits'
/// fractions in 0.24 with the first five bits zeroed.
fn dpp1_dscl_set_manual_ratio_init(adev: &amdgpu_device, dpp: &dcn20_dpp, data: &scaler_data) {
    let c = dpp.rsm();

    reg_set!(adev, c, SCL_HORZ_FILTER_SCALE_RATIO, 0, SCL_H_SCALE_RATIO, dc_fixpt_u3d19(data.ratios.horz) << 5);

    reg_set!(adev, c, SCL_VERT_FILTER_SCALE_RATIO, 0, SCL_V_SCALE_RATIO, dc_fixpt_u3d19(data.ratios.vert) << 5);

    reg_set!(adev, c, SCL_HORZ_FILTER_SCALE_RATIO_C, 0, SCL_H_SCALE_RATIO_C, dc_fixpt_u3d19(data.ratios.horz_c) << 5);

    reg_set!(adev, c, SCL_VERT_FILTER_SCALE_RATIO_C, 0, SCL_V_SCALE_RATIO_C, dc_fixpt_u3d19(data.ratios.vert_c) << 5);

    // 0.24 format for fraction, first five bits zeroed
    let mut init_frac = dc_fixpt_u0d19(data.inits.h) << 5;
    let mut init_int = dc_fixpt_floor(data.inits.h);
    reg_set!(adev, c, SCL_HORZ_FILTER_INIT, 0, SCL_H_INIT_FRAC, init_frac, SCL_H_INIT_INT, init_int);

    init_frac = dc_fixpt_u0d19(data.inits.h_c) << 5;
    init_int = dc_fixpt_floor(data.inits.h_c);
    reg_set!(adev, c, SCL_HORZ_FILTER_INIT_C, 0, SCL_H_INIT_FRAC_C, init_frac, SCL_H_INIT_INT_C, init_int);

    init_frac = dc_fixpt_u0d19(data.inits.v) << 5;
    init_int = dc_fixpt_floor(data.inits.v);
    reg_set!(adev, c, SCL_VERT_FILTER_INIT, 0, SCL_V_INIT_FRAC, init_frac, SCL_V_INIT_INT, init_int);

    if c.0.SCL_VERT_FILTER_INIT_BOT != 0 {
        let bot = dc_fixpt_add(data.inits.v, data.ratios.vert);

        init_frac = dc_fixpt_u0d19(bot) << 5;
        init_int = dc_fixpt_floor(bot);
        reg_set!(adev, c, SCL_VERT_FILTER_INIT_BOT, 0, SCL_V_INIT_FRAC_BOT, init_frac, SCL_V_INIT_INT_BOT, init_int);
    }

    init_frac = dc_fixpt_u0d19(data.inits.v_c) << 5;
    init_int = dc_fixpt_floor(data.inits.v_c);
    reg_set!(adev, c, SCL_VERT_FILTER_INIT_C, 0, SCL_V_INIT_FRAC_C, init_frac, SCL_V_INIT_INT_C, init_int);

    if c.0.SCL_VERT_FILTER_INIT_BOT_C != 0 {
        let bot = dc_fixpt_add(data.inits.v_c, data.ratios.vert_c);

        init_frac = dc_fixpt_u0d19(bot) << 5;
        init_int = dc_fixpt_floor(bot);
        reg_set!(adev, c, SCL_VERT_FILTER_INIT_BOT_C, 0,
            SCL_V_INIT_FRAC_BOT_C, init_frac, SCL_V_INIT_INT_BOT_C, init_int);
    }
}

/// `dpp1_dscl_set_scaler_manual_scale`: nothing is written when the scaler
/// data is the one programmed last.
pub fn dpp1_dscl_set_scaler_manual_scale(adev: &amdgpu_device, dpp: &mut dcn20_dpp, debug: &dc_debug_options,
                                         scl_data: &scaler_data) {
    let dscl_mode = dpp1_dscl_get_dscl_mode(dpp, scl_data, debug.always_scale);
    let ycbcr = scl_data.format >= PIXEL_FORMAT_VIDEO_BEGIN && scl_data.format <= PIXEL_FORMAT_VIDEO_END;

    if dpp.scl_data == *scl_data {
        return;
    }

    dpp.scl_data = *scl_data;

    if debug.enable_mem_low_power_dscl && dscl_mode != DSCL_MODE_DSCL_BYPASS {
        dpp1_power_on_dscl(adev, dpp, debug, true);
    }

    // Autocal off
    reg_set!(adev, dpp.rsm(), DSCL_AUTOCAL, 0,
        AUTOCAL_MODE, AUTOCAL_MODE_OFF,
        AUTOCAL_NUM_PIPE, 0,
        AUTOCAL_PIPE_ID, 0);

    // clean scaler boundary mode when Autocal off
    reg_set!(adev, dpp.rsm(), DSCL_CONTROL, 0, SCL_BOUNDARY_MODE, 0);

    // Recout
    dpp1_dscl_set_recout(adev, dpp, &scl_data.recout);

    // MPC Size
    reg_set!(adev, dpp.rsm(), MPC_SIZE, 0,
        // Number of horizontal pixels of MPC
        MPC_WIDTH, scl_data.h_active,
        // Number of vertical lines of MPC
        MPC_HEIGHT, scl_data.v_active);

    // SCL mode
    reg_update!(adev, dpp.rsm(), SCL_MODE, DSCL_MODE, dscl_mode);

    if dscl_mode == DSCL_MODE_DSCL_BYPASS {
        if debug.enable_mem_low_power_dscl {
            dpp1_power_on_dscl(adev, dpp, debug, false);
        }
        return;
    }

    // LB
    let lb_config = dpp1_dscl_find_lb_memory_config(dpp, debug, scl_data);
    dpp1_dscl_set_lb(adev, dpp, &scl_data.lb_params, lb_config);

    if dscl_mode == DSCL_MODE_SCALING_444_BYPASS {
        return;
    }

    // Black offsets
    if dpp.tf_regs.unwrap().SCL_BLACK_OFFSET != 0 {
        if ycbcr {
            reg_set!(adev, dpp.rsm(), SCL_BLACK_OFFSET, 0,
                SCL_BLACK_OFFSET_RGB_Y, BLACK_OFFSET_RGB_Y,
                SCL_BLACK_OFFSET_CBCR, BLACK_OFFSET_CBCR);
        } else {
            reg_set!(adev, dpp.rsm(), SCL_BLACK_OFFSET, 0,
                SCL_BLACK_OFFSET_RGB_Y, BLACK_OFFSET_RGB_Y,
                SCL_BLACK_OFFSET_CBCR, BLACK_OFFSET_RGB_Y);
        }
    }

    // Manually calculate scale ratio and init values
    dpp1_dscl_set_manual_ratio_init(adev, dpp, scl_data);

    // HTaps/VTaps
    reg_set!(adev, dpp.rsm(), SCL_TAP_CONTROL, 0,
        SCL_V_NUM_TAPS, scl_data.taps.v_taps - 1,
        SCL_H_NUM_TAPS, scl_data.taps.h_taps - 1,
        SCL_V_NUM_TAPS_C, scl_data.taps.v_taps_c - 1,
        SCL_H_NUM_TAPS_C, scl_data.taps.h_taps_c - 1);

    dpp1_dscl_set_scl_filter(adev, dpp, scl_data, ycbcr);
}
