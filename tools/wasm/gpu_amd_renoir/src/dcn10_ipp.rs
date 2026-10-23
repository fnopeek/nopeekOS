//! display/dc/inc/hw/ipp.h + dcn10/dcn10_ipp.{h,c} (Linux 6.18.55), with the
//! `ipp_regs`/`ipp_shift`/`ipp_mask` table of resource/dcn21/dcn21_resource.c:
//! the input pixel processor (IPP) of Renoir's resource pool. One per pipe.
//!
//! `dcn21_ipp_create` → `dcn20_ipp_construct` stores the instance, the funcs
//! identity and the register pointers; it writes no register (the funcs ops —
//! cursor set/attr — are hwss-stage). Renoir takes `IPP_REG_LIST_DCN20` /
//! `IPP_MASK_SH_LIST_DCN20`. The field struct is the whole `IPP_DCN10_REG_FIELD_
//! LIST`; `OUTPUT_FP` is DCN10-only and stays 0 for DCN20.
#![allow(dead_code)]

use crate::regs::*;
use alloc::boxed::Box;

/// Which `ipp_funcs` vtable `input_pixel_processor.funcs` points at
/// (dcn10_ipp.c). Only the identity is carried; the ops are hwss-stage.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ipp_funcs {
    dcn20_ipp_funcs,
}

/// `struct input_pixel_processor` (inc/hw/ipp.h): the base object. Linux' `ctx`
/// is not stored (see `clock_source`); `inst` is the pipe index.
#[derive(Default)]
pub struct input_pixel_processor {
    pub inst: u32,
    pub funcs: Option<ipp_funcs>,
}

/// `struct dcn10_ipp_registers` (dcn10_ipp.h), the register offsets one IPP
/// uses. For `IPP_REG_LIST_DCN20` every field but the legacy `CURSOR_SETTINS`
/// (no G) is filled. Field order follows the C struct.
#[derive(Default, Clone, Copy)]
pub struct dcn10_ipp_registers {
    pub CURSOR_SETTINS: u32,
    pub CURSOR_SETTINGS: u32,
    pub CNVC_SURFACE_PIXEL_FORMAT: u32,
    pub CURSOR0_CONTROL: u32,
    pub CURSOR0_COLOR0: u32,
    pub CURSOR0_COLOR1: u32,
    pub FORMAT_CONTROL: u32,
    pub CURSOR_SURFACE_ADDRESS_HIGH: u32,
    pub CURSOR_SURFACE_ADDRESS: u32,
    pub CURSOR_SIZE: u32,
    pub CURSOR_CONTROL: u32,
    pub CURSOR_POSITION: u32,
    pub CURSOR_HOT_SPOT: u32,
    pub CURSOR_DST_OFFSET: u32,
}

/// `struct dcn10_ipp_shift` (IPP_DCN10_REG_FIELD_LIST, dcn10_ipp.h), shift half.
/// `IPP_MASK_SH_LIST_DCN20` fills all but `OUTPUT_FP` (DCN10-only), which is 0.
#[derive(Default, Clone, Copy)]
pub struct dcn10_ipp_shift {
    pub CNVC_SURFACE_PIXEL_FORMAT: u8,
    pub CNVC_BYPASS: u8,
    pub ALPHA_EN: u8,
    pub FORMAT_EXPANSION_MODE: u8,
    pub CURSOR0_DST_Y_OFFSET: u8,
    pub CURSOR0_CHUNK_HDL_ADJUST: u8,
    pub CUR0_MODE: u8,
    pub CUR0_COLOR0: u8,
    pub CUR0_COLOR1: u8,
    pub CUR0_EXPANSION_MODE: u8,
    pub CURSOR_SURFACE_ADDRESS_HIGH: u8,
    pub CURSOR_SURFACE_ADDRESS: u8,
    pub CURSOR_WIDTH: u8,
    pub CURSOR_HEIGHT: u8,
    pub CURSOR_MODE: u8,
    pub CURSOR_2X_MAGNIFY: u8,
    pub CURSOR_PITCH: u8,
    pub CURSOR_LINES_PER_CHUNK: u8,
    pub CURSOR_ENABLE: u8,
    pub CUR0_ENABLE: u8,
    pub CURSOR_X_POSITION: u8,
    pub CURSOR_Y_POSITION: u8,
    pub CURSOR_HOT_SPOT_X: u8,
    pub CURSOR_HOT_SPOT_Y: u8,
    pub CURSOR_DST_X_OFFSET: u8,
    pub OUTPUT_FP: u8,
}

/// `struct dcn10_ipp_mask` (IPP_DCN10_REG_FIELD_LIST, dcn10_ipp.h), mask half.
#[derive(Default, Clone, Copy)]
pub struct dcn10_ipp_mask {
    pub CNVC_SURFACE_PIXEL_FORMAT: u32,
    pub CNVC_BYPASS: u32,
    pub ALPHA_EN: u32,
    pub FORMAT_EXPANSION_MODE: u32,
    pub CURSOR0_DST_Y_OFFSET: u32,
    pub CURSOR0_CHUNK_HDL_ADJUST: u32,
    pub CUR0_MODE: u32,
    pub CUR0_COLOR0: u32,
    pub CUR0_COLOR1: u32,
    pub CUR0_EXPANSION_MODE: u32,
    pub CURSOR_SURFACE_ADDRESS_HIGH: u32,
    pub CURSOR_SURFACE_ADDRESS: u32,
    pub CURSOR_WIDTH: u32,
    pub CURSOR_HEIGHT: u32,
    pub CURSOR_MODE: u32,
    pub CURSOR_2X_MAGNIFY: u32,
    pub CURSOR_PITCH: u32,
    pub CURSOR_LINES_PER_CHUNK: u32,
    pub CURSOR_ENABLE: u32,
    pub CUR0_ENABLE: u32,
    pub CURSOR_X_POSITION: u32,
    pub CURSOR_Y_POSITION: u32,
    pub CURSOR_HOT_SPOT_X: u32,
    pub CURSOR_HOT_SPOT_Y: u32,
    pub CURSOR_DST_X_OFFSET: u32,
    pub OUTPUT_FP: u32,
}

/// `struct dcn10_ipp` (dcn10_ipp.h): the concrete IPP. `curs_attr`
/// (dc_cursor_attributes) joins with a cursor funcs op that sets it.
#[derive(Default)]
pub struct dcn10_ipp {
    pub base: input_pixel_processor,
    pub regs: Option<&'static dcn10_ipp_registers>,
    pub ipp_shift: Option<&'static dcn10_ipp_shift>,
    pub ipp_mask: Option<&'static dcn10_ipp_mask>,
}

/// One `ipp_regs[]` entry (dcn21_resource.c), the 13 offsets `IPP_REG_LIST_DCN20`
/// fills for a pipe instance; `CURSOR_SETTINS` (legacy) stays 0.
const fn ipp_regs(
    cursor_settings: u32, cnvc_pix: u32, cur0_ctrl: u32, cur0_color0: u32,
    cur0_color1: u32, format_control: u32, cur_addr_hi: u32, cur_addr: u32,
    cur_size: u32, cur_control: u32, cur_position: u32, cur_hot_spot: u32,
    cur_dst_offset: u32,
) -> dcn10_ipp_registers {
    dcn10_ipp_registers {
        CURSOR_SETTINS: 0,
        CURSOR_SETTINGS: cursor_settings,
        CNVC_SURFACE_PIXEL_FORMAT: cnvc_pix,
        CURSOR0_CONTROL: cur0_ctrl,
        CURSOR0_COLOR0: cur0_color0,
        CURSOR0_COLOR1: cur0_color1,
        FORMAT_CONTROL: format_control,
        CURSOR_SURFACE_ADDRESS_HIGH: cur_addr_hi,
        CURSOR_SURFACE_ADDRESS: cur_addr,
        CURSOR_SIZE: cur_size,
        CURSOR_CONTROL: cur_control,
        CURSOR_POSITION: cur_position,
        CURSOR_HOT_SPOT: cur_hot_spot,
        CURSOR_DST_OFFSET: cur_dst_offset,
    }
}

/// `static const struct dcn10_ipp_registers ipp_regs[]` (dcn21_resource.c), one
/// per pipe instance 0..3.
pub static IPP_REGS: [dcn10_ipp_registers; 4] = [
    ipp_regs(
        IPP0_CURSOR_SETTINGS, IPP0_CNVC_SURFACE_PIXEL_FORMAT, IPP0_CURSOR0_CONTROL,
        IPP0_CURSOR0_COLOR0, IPP0_CURSOR0_COLOR1, IPP0_FORMAT_CONTROL,
        IPP0_CURSOR_SURFACE_ADDRESS_HIGH, IPP0_CURSOR_SURFACE_ADDRESS, IPP0_CURSOR_SIZE,
        IPP0_CURSOR_CONTROL, IPP0_CURSOR_POSITION, IPP0_CURSOR_HOT_SPOT, IPP0_CURSOR_DST_OFFSET,
    ),
    ipp_regs(
        IPP1_CURSOR_SETTINGS, IPP1_CNVC_SURFACE_PIXEL_FORMAT, IPP1_CURSOR0_CONTROL,
        IPP1_CURSOR0_COLOR0, IPP1_CURSOR0_COLOR1, IPP1_FORMAT_CONTROL,
        IPP1_CURSOR_SURFACE_ADDRESS_HIGH, IPP1_CURSOR_SURFACE_ADDRESS, IPP1_CURSOR_SIZE,
        IPP1_CURSOR_CONTROL, IPP1_CURSOR_POSITION, IPP1_CURSOR_HOT_SPOT, IPP1_CURSOR_DST_OFFSET,
    ),
    ipp_regs(
        IPP2_CURSOR_SETTINGS, IPP2_CNVC_SURFACE_PIXEL_FORMAT, IPP2_CURSOR0_CONTROL,
        IPP2_CURSOR0_COLOR0, IPP2_CURSOR0_COLOR1, IPP2_FORMAT_CONTROL,
        IPP2_CURSOR_SURFACE_ADDRESS_HIGH, IPP2_CURSOR_SURFACE_ADDRESS, IPP2_CURSOR_SIZE,
        IPP2_CURSOR_CONTROL, IPP2_CURSOR_POSITION, IPP2_CURSOR_HOT_SPOT, IPP2_CURSOR_DST_OFFSET,
    ),
    ipp_regs(
        IPP3_CURSOR_SETTINGS, IPP3_CNVC_SURFACE_PIXEL_FORMAT, IPP3_CURSOR0_CONTROL,
        IPP3_CURSOR0_COLOR0, IPP3_CURSOR0_COLOR1, IPP3_FORMAT_CONTROL,
        IPP3_CURSOR_SURFACE_ADDRESS_HIGH, IPP3_CURSOR_SURFACE_ADDRESS, IPP3_CURSOR_SIZE,
        IPP3_CURSOR_CONTROL, IPP3_CURSOR_POSITION, IPP3_CURSOR_HOT_SPOT, IPP3_CURSOR_DST_OFFSET,
    ),
];

/// `static const struct dcn10_ipp_shift ipp_shift` (dcn21_resource.c),
/// `IPP_MASK_SH_LIST_DCN20(__SHIFT)`.
pub static IPP_SHIFT: dcn10_ipp_shift = dcn10_ipp_shift {
    CNVC_SURFACE_PIXEL_FORMAT: CNVC_CFG0_CNVC_SURFACE_PIXEL_FORMAT__CNVC_SURFACE_PIXEL_FORMAT__SHIFT as u8,
    CNVC_BYPASS: CNVC_CFG0_FORMAT_CONTROL__CNVC_BYPASS__SHIFT as u8,
    ALPHA_EN: CNVC_CFG0_FORMAT_CONTROL__ALPHA_EN__SHIFT as u8,
    FORMAT_EXPANSION_MODE: CNVC_CFG0_FORMAT_CONTROL__FORMAT_EXPANSION_MODE__SHIFT as u8,
    CURSOR0_DST_Y_OFFSET: HUBPREQ0_CURSOR_SETTINGS__CURSOR0_DST_Y_OFFSET__SHIFT as u8,
    CURSOR0_CHUNK_HDL_ADJUST: HUBPREQ0_CURSOR_SETTINGS__CURSOR0_CHUNK_HDL_ADJUST__SHIFT as u8,
    CUR0_MODE: CNVC_CUR0_CURSOR0_CONTROL__CUR0_MODE__SHIFT as u8,
    CUR0_COLOR0: CNVC_CUR0_CURSOR0_COLOR0__CUR0_COLOR0__SHIFT as u8,
    CUR0_COLOR1: CNVC_CUR0_CURSOR0_COLOR1__CUR0_COLOR1__SHIFT as u8,
    CUR0_EXPANSION_MODE: CNVC_CUR0_CURSOR0_CONTROL__CUR0_EXPANSION_MODE__SHIFT as u8,
    CURSOR_SURFACE_ADDRESS_HIGH: CURSOR0_0_CURSOR_SURFACE_ADDRESS_HIGH__CURSOR_SURFACE_ADDRESS_HIGH__SHIFT as u8,
    CURSOR_SURFACE_ADDRESS: CURSOR0_0_CURSOR_SURFACE_ADDRESS__CURSOR_SURFACE_ADDRESS__SHIFT as u8,
    CURSOR_WIDTH: CURSOR0_0_CURSOR_SIZE__CURSOR_WIDTH__SHIFT as u8,
    CURSOR_HEIGHT: CURSOR0_0_CURSOR_SIZE__CURSOR_HEIGHT__SHIFT as u8,
    CURSOR_MODE: CURSOR0_0_CURSOR_CONTROL__CURSOR_MODE__SHIFT as u8,
    CURSOR_2X_MAGNIFY: CURSOR0_0_CURSOR_CONTROL__CURSOR_2X_MAGNIFY__SHIFT as u8,
    CURSOR_PITCH: CURSOR0_0_CURSOR_CONTROL__CURSOR_PITCH__SHIFT as u8,
    CURSOR_LINES_PER_CHUNK: CURSOR0_0_CURSOR_CONTROL__CURSOR_LINES_PER_CHUNK__SHIFT as u8,
    CURSOR_ENABLE: CURSOR0_0_CURSOR_CONTROL__CURSOR_ENABLE__SHIFT as u8,
    CUR0_ENABLE: CNVC_CUR0_CURSOR0_CONTROL__CUR0_ENABLE__SHIFT as u8,
    CURSOR_X_POSITION: CURSOR0_0_CURSOR_POSITION__CURSOR_X_POSITION__SHIFT as u8,
    CURSOR_Y_POSITION: CURSOR0_0_CURSOR_POSITION__CURSOR_Y_POSITION__SHIFT as u8,
    CURSOR_HOT_SPOT_X: CURSOR0_0_CURSOR_HOT_SPOT__CURSOR_HOT_SPOT_X__SHIFT as u8,
    CURSOR_HOT_SPOT_Y: CURSOR0_0_CURSOR_HOT_SPOT__CURSOR_HOT_SPOT_Y__SHIFT as u8,
    CURSOR_DST_X_OFFSET: CURSOR0_0_CURSOR_DST_OFFSET__CURSOR_DST_X_OFFSET__SHIFT as u8,
    OUTPUT_FP: 0,
};

/// `static const struct dcn10_ipp_mask ipp_mask` (dcn21_resource.c),
/// `IPP_MASK_SH_LIST_DCN20(_MASK)`.
pub static IPP_MASK: dcn10_ipp_mask = dcn10_ipp_mask {
    CNVC_SURFACE_PIXEL_FORMAT: CNVC_CFG0_CNVC_SURFACE_PIXEL_FORMAT__CNVC_SURFACE_PIXEL_FORMAT_MASK,
    CNVC_BYPASS: CNVC_CFG0_FORMAT_CONTROL__CNVC_BYPASS_MASK,
    ALPHA_EN: CNVC_CFG0_FORMAT_CONTROL__ALPHA_EN_MASK,
    FORMAT_EXPANSION_MODE: CNVC_CFG0_FORMAT_CONTROL__FORMAT_EXPANSION_MODE_MASK,
    CURSOR0_DST_Y_OFFSET: HUBPREQ0_CURSOR_SETTINGS__CURSOR0_DST_Y_OFFSET_MASK,
    CURSOR0_CHUNK_HDL_ADJUST: HUBPREQ0_CURSOR_SETTINGS__CURSOR0_CHUNK_HDL_ADJUST_MASK,
    CUR0_MODE: CNVC_CUR0_CURSOR0_CONTROL__CUR0_MODE_MASK,
    CUR0_COLOR0: CNVC_CUR0_CURSOR0_COLOR0__CUR0_COLOR0_MASK,
    CUR0_COLOR1: CNVC_CUR0_CURSOR0_COLOR1__CUR0_COLOR1_MASK,
    CUR0_EXPANSION_MODE: CNVC_CUR0_CURSOR0_CONTROL__CUR0_EXPANSION_MODE_MASK,
    CURSOR_SURFACE_ADDRESS_HIGH: CURSOR0_0_CURSOR_SURFACE_ADDRESS_HIGH__CURSOR_SURFACE_ADDRESS_HIGH_MASK,
    CURSOR_SURFACE_ADDRESS: CURSOR0_0_CURSOR_SURFACE_ADDRESS__CURSOR_SURFACE_ADDRESS_MASK,
    CURSOR_WIDTH: CURSOR0_0_CURSOR_SIZE__CURSOR_WIDTH_MASK,
    CURSOR_HEIGHT: CURSOR0_0_CURSOR_SIZE__CURSOR_HEIGHT_MASK,
    CURSOR_MODE: CURSOR0_0_CURSOR_CONTROL__CURSOR_MODE_MASK,
    CURSOR_2X_MAGNIFY: CURSOR0_0_CURSOR_CONTROL__CURSOR_2X_MAGNIFY_MASK,
    CURSOR_PITCH: CURSOR0_0_CURSOR_CONTROL__CURSOR_PITCH_MASK,
    CURSOR_LINES_PER_CHUNK: CURSOR0_0_CURSOR_CONTROL__CURSOR_LINES_PER_CHUNK_MASK,
    CURSOR_ENABLE: CURSOR0_0_CURSOR_CONTROL__CURSOR_ENABLE_MASK,
    CUR0_ENABLE: CNVC_CUR0_CURSOR0_CONTROL__CUR0_ENABLE_MASK,
    CURSOR_X_POSITION: CURSOR0_0_CURSOR_POSITION__CURSOR_X_POSITION_MASK,
    CURSOR_Y_POSITION: CURSOR0_0_CURSOR_POSITION__CURSOR_Y_POSITION_MASK,
    CURSOR_HOT_SPOT_X: CURSOR0_0_CURSOR_HOT_SPOT__CURSOR_HOT_SPOT_X_MASK,
    CURSOR_HOT_SPOT_Y: CURSOR0_0_CURSOR_HOT_SPOT__CURSOR_HOT_SPOT_Y_MASK,
    CURSOR_DST_X_OFFSET: CURSOR0_0_CURSOR_DST_OFFSET__CURSOR_DST_X_OFFSET_MASK,
    OUTPUT_FP: 0,
};

/// `dcn20_ipp_construct` (dcn10_ipp.c): store the instance, funcs and registers.
pub fn dcn20_ipp_construct(
    ipp: &mut dcn10_ipp,
    inst: u32,
    regs: &'static dcn10_ipp_registers,
    ipp_shift: &'static dcn10_ipp_shift,
    ipp_mask: &'static dcn10_ipp_mask,
) {
    ipp.base.inst = inst;
    ipp.base.funcs = Some(ipp_funcs::dcn20_ipp_funcs);
    ipp.regs = Some(regs);
    ipp.ipp_shift = Some(ipp_shift);
    ipp.ipp_mask = Some(ipp_mask);
}

/// `dcn21_ipp_create` (dcn21_resource.c): allocate a `dcn10_ipp` for pipe `inst`
/// and construct it.
pub fn dcn21_ipp_create(inst: u32) -> Box<dcn10_ipp> {
    let mut ipp = Box::new(dcn10_ipp::default());
    dcn20_ipp_construct(&mut ipp, inst, &IPP_REGS[inst as usize], &IPP_SHIFT, &IPP_MASK);
    ipp
}
