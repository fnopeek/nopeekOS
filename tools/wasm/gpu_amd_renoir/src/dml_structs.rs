//! display/dc/dml/display_mode_lib.c (Linux 6.18.55): `dml_init_instance`,
//! `dml_get_status_message`.
//! The DML types themselves are generated (display_mode_structs.rs, from
//! display_mode_structs.h / display_mode_vba.h / display_mode_lib.h).
#![allow(dead_code, non_snake_case)]

pub use crate::display_mode_structs::{
    _vcs_dpi_ip_params_st, _vcs_dpi_soc_bounding_box_st, _vcs_dpi_voltage_scaling_st, display_mode_lib, dml_funcs,
    DML_PROJECT_DCN21,
};

/// A DML instance on the heap, as `dc.dml` and every context's
/// `bw_ctx.dml` hold one. It is 200 KB: cloning goes heap to heap, never
/// through the stack.
pub struct DmlBox(pub alloc::boxed::Box<display_mode_lib>);

impl DmlBox {
    pub fn zero() -> Self {
        DmlBox(display_mode_lib::boxed_zero())
    }
}

impl Clone for DmlBox {
    fn clone(&self) -> Self {
        DmlBox(self.0.boxed_clone())
    }
}

impl core::ops::Deref for DmlBox {
    type Target = display_mode_lib;
    fn deref(&self) -> &display_mode_lib {
        &self.0
    }
}

impl core::ops::DerefMut for DmlBox {
    fn deref_mut(&mut self) -> &mut display_mode_lib {
        &mut self.0
    }
}

/// `static const struct dml_funcs dml21_funcs` (display_mode_lib.c).
pub const dml21_funcs: dml_funcs = dml_funcs {
    validate: Some(crate::display_mode_vba_21::dml21_ModeSupportAndSystemConfigurationFull),
    recalculate: Some(crate::display_mode_vba_21::dml21_recalculate),
    rq_dlg_get_dlg_reg: Some(crate::display_rq_dlg_calc_21::dml21_rq_dlg_get_dlg_reg),
    rq_dlg_get_rq_reg: Some(crate::display_rq_dlg_calc_21::dml21_rq_dlg_get_rq_reg),
    ..dml_funcs::ZERO
};

/// `dml_init_instance`: copy the SoC/IP inputs into the instance and bind
/// the per-project vtable. The other projects' tables (dml20/30/31/32 …)
/// belong to other DCN families and are not built.
pub fn dml_init_instance(lib: &mut display_mode_lib, soc_bb: &_vcs_dpi_soc_bounding_box_st,
                         ip_params: &_vcs_dpi_ip_params_st, project: u32) {
    lib.soc = *soc_bb;
    lib.ip = *ip_params;
    lib.project = project;
    if project == DML_PROJECT_DCN21 {
        lib.funcs = dml21_funcs;
    } else {
        crate::linux::Line::new().s("dml_init_instance: project ").dec(project as u64).s(" not built").say();
    }
}

/// `dml_get_status_message`.
pub fn dml_get_status_message(status: u32) -> &'static str {
    use crate::display_mode_structs::*;
    match status {
        DML_VALIDATION_OK => "Validation OK",
        DML_FAIL_SCALE_RATIO_TAP => "Scale ratio/tap",
        DML_FAIL_SOURCE_PIXEL_FORMAT => "Source pixel format",
        DML_FAIL_VIEWPORT_SIZE => "Viewport size",
        DML_FAIL_TOTAL_V_ACTIVE_BW => "Total vertical active bandwidth",
        DML_FAIL_DIO_SUPPORT => "DIO support",
        DML_FAIL_NOT_ENOUGH_DSC => "Not enough DSC Units",
        DML_FAIL_DSC_CLK_REQUIRED => "DSC clock required",
        DML_FAIL_URGENT_LATENCY => "Urgent latency",
        DML_FAIL_REORDERING_BUFFER => "Re-ordering buffer",
        DML_FAIL_DISPCLK_DPPCLK => "Dispclk and Dppclk",
        DML_FAIL_TOTAL_AVAILABLE_PIPES => "Total available pipes",
        DML_FAIL_NUM_OTG => "Number of OTG",
        DML_FAIL_WRITEBACK_MODE => "Writeback mode",
        DML_FAIL_WRITEBACK_LATENCY => "Writeback latency",
        DML_FAIL_WRITEBACK_SCALE_RATIO_TAP => "Writeback scale ratio/tap",
        DML_FAIL_CURSOR_SUPPORT => "Cursor support",
        DML_FAIL_PITCH_SUPPORT => "Pitch support",
        DML_FAIL_PTE_BUFFER_SIZE => "PTE buffer size",
        DML_FAIL_DSC_INPUT_BPC => "DSC input bpc",
        DML_FAIL_PREFETCH_SUPPORT => "Prefetch support",
        DML_FAIL_V_RATIO_PREFETCH => "Vertical ratio prefetch",
        _ => "Unknown Status",
    }
}
