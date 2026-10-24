//! display/dc/dpp/dcn20/dcn20_dpp_cm.c + the CNVC setup of dcn20_dpp.c
//! (Linux 6.18.55): the DPP's format conversion and colour management —
//! input CSC, gamut remap, degamma, HDR multiplier, and the shaper, 3D LUT
//! and blend gamma RAMs. Programming a RAM with a curve joins with colour
//! management; what is ported is the bypass every plane without one takes.
#![allow(dead_code)]

use crate::amdgpu_device::amdgpu_device;
use crate::dc::dc_debug_options;
use crate::dc_helper::{cm_helper_program_color_matrices, color_matrices_reg, ix_reg_get, reg_set, reg_update};
use crate::dc_hw_types::dc_csc_transform;
use crate::dcn20_dpp::dcn20_dpp;
use crate::fixpt31_32::fixed31_32;
use crate::regs::*;

/// `struct pwl_params` (hw_shared.h): a piecewise linear curve for a LUT
/// RAM. Its points join with colour management.
#[derive(Clone, Copy, Default, Debug)]
pub struct pwl_params {}

/// `struct tetrahedral_params` (dc_hw_types.h): a 3D LUT. Its entries join
/// with colour management.
#[derive(Clone, Copy, Default, Debug)]
pub struct tetrahedral_params {}

/// `struct cnv_alpha_2bit_lut` (dpp.h).
#[derive(Clone, Copy, Default, Debug)]
pub struct cnv_alpha_2bit_lut {
    pub lut0: i32,
    pub lut1: i32,
    pub lut2: i32,
    pub lut3: i32,
}

/// `struct out_csc_color_matrix` (hw_shared.h).
#[derive(Clone, Copy, Default, Debug)]
pub struct out_csc_color_matrix {
    /// `enum dc_color_space`.
    pub color_space: u32,
    pub regval: [u16; 12],
}

/// `struct dpp_grph_csc_adjustment` (transform.h), the members DCN 2.0 reads.
#[derive(Clone, Copy, Default, Debug)]
pub struct dpp_grph_csc_adjustment {
    pub temperature_matrix: [fixed31_32; CSC_TEMPERATURE_MATRIX_SIZE as usize],
    /// `enum graphics_gamut_adjust_type`.
    pub gamut_adjust_type: u32,
}

/// `dpp2_enable_cm_block`.
fn dpp2_enable_cm_block(adev: &amdgpu_device, dpp: &dcn20_dpp, debug: &dc_debug_options) {
    let mut cm_bypass_mode = 0;
    // Temp, put CM in bypass mode
    if debug.cm_in_bypass {
        cm_bypass_mode = 1;
    }

    reg_update!(adev, dpp.rsm(), CM_CONTROL, CM_BYPASS, cm_bypass_mode);
}

/// `dpp2_set_degamma`.
pub fn dpp2_set_degamma(adev: &amdgpu_device, dpp: &dcn20_dpp, debug: &dc_debug_options, mode: u32) {
    dpp2_enable_cm_block(adev, dpp, debug);

    match mode {
        // Setting de gamma bypass for now
        IPP_DEGAMMA_MODE_BYPASS => {
            reg_update!(adev, dpp.rsm(), CM_DGAM_CONTROL, CM_DGAM_LUT_MODE, 0);
        }
        IPP_DEGAMMA_MODE_HW_sRGB => {
            reg_update!(adev, dpp.rsm(), CM_DGAM_CONTROL, CM_DGAM_LUT_MODE, 1);
        }
        IPP_DEGAMMA_MODE_HW_xvYCC => {
            reg_update!(adev, dpp.rsm(), CM_DGAM_CONTROL, CM_DGAM_LUT_MODE, 2);
        }
        IPP_DEGAMMA_MODE_USER_PWL => {
            reg_update!(adev, dpp.rsm(), CM_DGAM_CONTROL, CM_DGAM_LUT_MODE, 3);
        }
        _ => {}
    }
}

/// `program_gamut_remap` (dcn20_dpp_cm.c): the coefficients go to the set
/// not in use (A or B), so the change lands on a frame boundary.
fn program_gamut_remap(adev: &amdgpu_device, dpp: &dcn20_dpp, regval: Option<&[u16]>, mut select: u32) {
    let c = dpp.rsm();

    let Some(regval) = regval.filter(|_| select != DCN2_GAMUT_REMAP_BYPASS) else {
        reg_set!(adev, c, CM_GAMUT_REMAP_CONTROL, 0, CM_GAMUT_REMAP_MODE, 0);
        return;
    };

    // The debug register holds one more than the mode we want.
    let cur_select = ix_reg_get!(adev, c, CM_TEST_DEBUG_INDEX, CM_TEST_DEBUG_DATA, CM_TEST_DEBUG_DATA_STATUS_IDX,
        CM_TEST_DEBUG_DATA_GAMUT_REMAP_MODE);

    select = if cur_select != DCN2_GAMUT_REMAP_COEF_A { DCN2_GAMUT_REMAP_COEF_A } else { DCN2_GAMUT_REMAP_COEF_B };

    let mut gam_regs = color_matrices_reg {
        shift_csc_c11: c.1.CM_GAMUT_REMAP_C11,
        mask_csc_c11: c.2.CM_GAMUT_REMAP_C11,
        shift_csc_c12: c.1.CM_GAMUT_REMAP_C12,
        mask_csc_c12: c.2.CM_GAMUT_REMAP_C12,
        ..Default::default()
    };

    if select == DCN2_GAMUT_REMAP_COEF_A {
        gam_regs.csc_c11_c12 = c.0.CM_GAMUT_REMAP_C11_C12;
        gam_regs.csc_c33_c34 = c.0.CM_GAMUT_REMAP_C33_C34;
    } else {
        gam_regs.csc_c11_c12 = c.0.CM_GAMUT_REMAP_B_C11_C12;
        gam_regs.csc_c33_c34 = c.0.CM_GAMUT_REMAP_B_C33_C34;
    }

    cm_helper_program_color_matrices(adev, regval, &gam_regs);

    reg_set!(adev, c, CM_GAMUT_REMAP_CONTROL, 0, CM_GAMUT_REMAP_MODE, select);
}

/// `dpp2_cm_set_gamut_remap`.
pub fn dpp2_cm_set_gamut_remap(adev: &amdgpu_device, dpp: &dcn20_dpp, adjust: &dpp_grph_csc_adjustment) {
    if adjust.gamut_adjust_type != GRAPHICS_GAMUT_ADJUST_TYPE_SW {
        // Bypass if type is bypass or hw
        program_gamut_remap(adev, dpp, None, DCN2_GAMUT_REMAP_BYPASS);
    } else {
        let mut arr_matrix = [fixed31_32::default(); 12];
        let mut arr_reg_val = [0u16; 12];

        arr_matrix.copy_from_slice(&adjust.temperature_matrix[..12]);

        crate::conversion::convert_float_matrix(&mut arr_reg_val, &arr_matrix, 12);

        program_gamut_remap(adev, dpp, Some(&arr_reg_val), DCN2_GAMUT_REMAP_COEF_A);
    }
}

/// `dpp_input_csc_matrix[]` (dpp.h): the colour space (an enum, two
/// halves) and its twelve coefficients, flat in `tables.rs`.
fn dpp_input_csc_matrix(color_space: u32) -> Option<&'static [u16]> {
    crate::tables::dpp_input_csc_matrix
        .chunks(14)
        .find(|e| (e[0] as u32 | (e[1] as u32) << 16) == color_space)
        .map(|e| &e[2..14])
}

/// `dpp2_program_input_csc`: the coefficients go to the set not in use.
pub fn dpp2_program_input_csc(adev: &amdgpu_device, dpp: &dcn20_dpp, color_space: u32, input_select: u32,
                              tbl_entry: Option<&out_csc_color_matrix>) {
    let c = dpp.rsm();

    if input_select == DCN2_ICSC_SELECT_BYPASS {
        reg_set!(adev, c, CM_ICSC_CONTROL, 0, CM_ICSC_MODE, 0);
        return;
    }

    let regval: &[u16] = match tbl_entry {
        None => match dpp_input_csc_matrix(color_space) {
            Some(r) => r,
            None => return,
        },
        Some(t) => &t.regval,
    };

    // determine which CSC coefficients (A or B) we are using currently.
    // select the alternate set to double buffer the CSC update so CSC is
    // updated on frame boundary
    let cur_select = ix_reg_get!(adev, c, CM_TEST_DEBUG_INDEX, CM_TEST_DEBUG_DATA, CM_TEST_DEBUG_DATA_STATUS_IDX,
        CM_TEST_DEBUG_DATA_ICSC_MODE);

    let select = if cur_select != DCN2_ICSC_SELECT_ICSC_A { DCN2_ICSC_SELECT_ICSC_A } else { DCN2_ICSC_SELECT_ICSC_B };

    let mut icsc_regs = color_matrices_reg {
        shift_csc_c11: c.1.CM_ICSC_C11,
        mask_csc_c11: c.2.CM_ICSC_C11,
        shift_csc_c12: c.1.CM_ICSC_C12,
        mask_csc_c12: c.2.CM_ICSC_C12,
        ..Default::default()
    };

    if select == DCN2_ICSC_SELECT_ICSC_A {
        icsc_regs.csc_c11_c12 = c.0.CM_ICSC_C11_C12;
        icsc_regs.csc_c33_c34 = c.0.CM_ICSC_C33_C34;
    } else {
        icsc_regs.csc_c11_c12 = c.0.CM_ICSC_B_C11_C12;
        icsc_regs.csc_c33_c34 = c.0.CM_ICSC_B_C33_C34;
    }

    cm_helper_program_color_matrices(adev, regval, &icsc_regs);

    reg_set!(adev, c, CM_ICSC_CONTROL, 0, CM_ICSC_MODE, select);
}

/// `dpp2_power_on_obuf` (dcn20_dpp.c).
pub fn dpp2_power_on_obuf(adev: &amdgpu_device, dpp: &dcn20_dpp, power_on: bool) {
    let c = dpp.rsm();
    reg_update!(adev, c, CM_MEM_PWR_CTRL, SHARED_MEM_PWR_DIS, power_on as u32);

    reg_update!(adev, c, OBUF_MEM_PWR_CTRL, OBUF_MEM_PWR_FORCE, if power_on { 0 } else { 1 });

    reg_update!(adev, c, DSCL_MEM_PWR_CTRL, LUT_MEM_PWR_FORCE, if power_on { 0 } else { 1 });
}

/// `dpp2_cnv_setup` (dcn20_dpp.c): the CNVC's pixel format, alpha and input
/// CSC for a surface format.
pub fn dpp2_cnv_setup(adev: &amdgpu_device, dpp: &dcn20_dpp, format: u32, mode: u32,
                      input_csc_color_matrix: dc_csc_transform, input_color_space: u32,
                      alpha_2bit_lut: Option<&cnv_alpha_2bit_lut>) {
    let c = dpp.rsm();
    let mut pixel_format = 0;
    let mut alpha_en = 1;
    let mut color_space = COLOR_SPACE_SRGB;
    let mut select = DCN2_ICSC_SELECT_BYPASS;
    let mut force_disable_cursor = false;
    let mut is_2bit = 0;

    reg_set!(adev, c, FORMAT_CONTROL, 0, CNVC_BYPASS, 0, FORMAT_EXPANSION_MODE, mode);

    // hardcode default
    // FORMAT_CONTROL. FORMAT_CNV16          default 0: U0.16/S.1.15; 1: U1.15/ S.1.14
    // FORMAT_CONTROL. CNVC_BYPASS_MSB_ALIGN default 0: disabled 1: enabled
    // FORMAT_CONTROL. CLAMP_POSITIVE        default 0: disabled 1: enabled
    // FORMAT_CONTROL. CLAMP_POSITIVE_C      default 0: disabled 1: enabled
    reg_update!(adev, c, FORMAT_CONTROL, FORMAT_CNV16, 0);
    reg_update!(adev, c, FORMAT_CONTROL, CNVC_BYPASS_MSB_ALIGN, 0);
    reg_update!(adev, c, FORMAT_CONTROL, CLAMP_POSITIVE, 0);
    reg_update!(adev, c, FORMAT_CONTROL, CLAMP_POSITIVE_C, 0);

    match format {
        SURFACE_PIXEL_FORMAT_GRPH_ARGB1555 => pixel_format = 1,
        SURFACE_PIXEL_FORMAT_GRPH_RGB565 => {
            pixel_format = 3;
            alpha_en = 0;
        }
        SURFACE_PIXEL_FORMAT_GRPH_ARGB8888 | SURFACE_PIXEL_FORMAT_GRPH_ABGR8888 => pixel_format = 8,
        SURFACE_PIXEL_FORMAT_GRPH_ARGB2101010 | SURFACE_PIXEL_FORMAT_GRPH_ABGR2101010 => {
            pixel_format = 10;
            is_2bit = 1;
        }
        SURFACE_PIXEL_FORMAT_VIDEO_420_YCbCr => {
            force_disable_cursor = false;
            pixel_format = 65;
            color_space = COLOR_SPACE_YCBCR709;
            select = DCN2_ICSC_SELECT_ICSC_A;
        }
        SURFACE_PIXEL_FORMAT_VIDEO_420_YCrCb => {
            force_disable_cursor = true;
            pixel_format = 64;
            color_space = COLOR_SPACE_YCBCR709;
            select = DCN2_ICSC_SELECT_ICSC_A;
        }
        SURFACE_PIXEL_FORMAT_VIDEO_420_10bpc_YCbCr => {
            force_disable_cursor = true;
            pixel_format = 67;
            color_space = COLOR_SPACE_YCBCR709;
            select = DCN2_ICSC_SELECT_ICSC_A;
        }
        SURFACE_PIXEL_FORMAT_VIDEO_420_10bpc_YCrCb => {
            force_disable_cursor = true;
            pixel_format = 66;
            color_space = COLOR_SPACE_YCBCR709;
            select = DCN2_ICSC_SELECT_ICSC_A;
        }
        SURFACE_PIXEL_FORMAT_GRPH_ARGB16161616 | SURFACE_PIXEL_FORMAT_GRPH_ABGR16161616 => {
            pixel_format = 26; // ARGB16161616_UNORM
        }
        SURFACE_PIXEL_FORMAT_GRPH_ARGB16161616F => pixel_format = 24,
        SURFACE_PIXEL_FORMAT_GRPH_ABGR16161616F => pixel_format = 25,
        SURFACE_PIXEL_FORMAT_VIDEO_AYCrCb8888 => {
            pixel_format = 12;
            color_space = COLOR_SPACE_YCBCR709;
            select = DCN2_ICSC_SELECT_ICSC_A;
        }
        SURFACE_PIXEL_FORMAT_GRPH_RGB111110_FIX => {
            pixel_format = 112;
            alpha_en = 0;
        }
        SURFACE_PIXEL_FORMAT_GRPH_BGR101111_FIX => {
            pixel_format = 113;
            alpha_en = 0;
        }
        SURFACE_PIXEL_FORMAT_VIDEO_ACrYCb2101010 => {
            pixel_format = 114;
            color_space = COLOR_SPACE_YCBCR709;
            select = DCN2_ICSC_SELECT_ICSC_A;
            is_2bit = 1;
        }
        SURFACE_PIXEL_FORMAT_VIDEO_CrYCbA1010102 => {
            pixel_format = 115;
            color_space = COLOR_SPACE_YCBCR709;
            select = DCN2_ICSC_SELECT_ICSC_A;
            is_2bit = 1;
        }
        SURFACE_PIXEL_FORMAT_GRPH_RGB111110_FLOAT => {
            pixel_format = 118;
            alpha_en = 0;
        }
        SURFACE_PIXEL_FORMAT_GRPH_BGR101111_FLOAT => {
            pixel_format = 119;
            alpha_en = 0;
        }
        _ => {}
    }

    // Set default color space based on format if none is given.
    color_space = if input_color_space != 0 { input_color_space } else { color_space };

    if is_2bit == 1 && let Some(lut) = alpha_2bit_lut {
        reg_update!(adev, c, ALPHA_2BIT_LUT, ALPHA_2BIT_LUT0, lut.lut0);
        reg_update!(adev, c, ALPHA_2BIT_LUT, ALPHA_2BIT_LUT1, lut.lut1);
        reg_update!(adev, c, ALPHA_2BIT_LUT, ALPHA_2BIT_LUT2, lut.lut2);
        reg_update!(adev, c, ALPHA_2BIT_LUT, ALPHA_2BIT_LUT3, lut.lut3);
    }

    reg_set!(adev, c, CNVC_SURFACE_PIXEL_FORMAT, 0, CNVC_SURFACE_PIXEL_FORMAT, pixel_format);
    reg_update!(adev, c, FORMAT_CONTROL, FORMAT_CONTROL__ALPHA_EN, alpha_en);

    // if input adjustments exist, program icsc with those values
    if input_csc_color_matrix.enable_adjustment {
        let tbl_entry = out_csc_color_matrix { color_space: input_color_space, regval: input_csc_color_matrix.matrix };

        select = if color_space >= COLOR_SPACE_YCBCR601 { DCN2_ICSC_SELECT_ICSC_A } else { DCN2_ICSC_SELECT_BYPASS };

        dpp2_program_input_csc(adev, dpp, color_space, select, Some(&tbl_entry));
    } else {
        dpp2_program_input_csc(adev, dpp, color_space, select, None);
    }

    if force_disable_cursor {
        reg_update!(adev, c, CURSOR_CONTROL, CURSOR_ENABLE, 0);
        reg_update!(adev, c, CURSOR0_CONTROL, CUR0_ENABLE, 0);
    }
    dpp2_power_on_obuf(adev, dpp, true);
}

/// `dpp20_program_blnd_lut`: a blend gamma curve joins with colour
/// management; without one the RAM is bypassed.
pub fn dpp20_program_blnd_lut(adev: &amdgpu_device, dpp: &dcn20_dpp, params: Option<&pwl_params>) -> bool {
    if params.is_none() {
        reg_set!(adev, dpp.rsm(), CM_BLNDGAM_CONTROL, 0, CM_BLNDGAM_LUT_MODE, 0);
        return false;
    }
    crate::linux::Line::new().s("dpp20_program_blnd_lut: blend gamma RAM not ported").say();
    false
}

/// `dpp20_program_shaper`: a shaper curve joins with colour management;
/// without one the shaper is bypassed.
pub fn dpp20_program_shaper(adev: &amdgpu_device, dpp: &dcn20_dpp, params: Option<&pwl_params>) -> bool {
    if params.is_none() {
        reg_set!(adev, dpp.rsm(), CM_SHAPER_CONTROL, 0, CM_SHAPER_LUT_MODE, 0);
        return false;
    }
    crate::linux::Line::new().s("dpp20_program_shaper: shaper RAM not ported").say();
    false
}

/// `dpp20_set_3dlut_mode`.
fn dpp20_set_3dlut_mode(adev: &amdgpu_device, dpp: &dcn20_dpp, mode: u32, _is_color_channel_12bits: bool,
                        is_lut_size17x17x17: bool) {
    let lut_mode = if mode == LUT_BYPASS {
        0
    } else if mode == LUT_RAM_A {
        1
    } else {
        2
    };

    reg_update!(adev, dpp.rsm(), CM_3DLUT_MODE,
        CM_3DLUT_MODE, lut_mode,
        CM_3DLUT_SIZE, if is_lut_size17x17x17 { 0 } else { 1 });
}

/// `dpp20_program_3dlut`: a 3D LUT joins with colour management; without
/// one the LUT is bypassed.
pub fn dpp20_program_3dlut(adev: &amdgpu_device, dpp: &dcn20_dpp, params: Option<&tetrahedral_params>) -> bool {
    if params.is_none() {
        dpp20_set_3dlut_mode(adev, dpp, LUT_BYPASS, false, false);
        return false;
    }
    crate::linux::Line::new().s("dpp20_program_3dlut: 3D LUT RAM not ported").say();
    false
}

/// `dpp2_set_hdr_multiplier`.
pub fn dpp2_set_hdr_multiplier(adev: &amdgpu_device, dpp: &dcn20_dpp, multiplier: u32) {
    reg_update!(adev, dpp.rsm(), CM_HDR_MULT_COEF, CM_HDR_MULT_COEF, multiplier);
}
