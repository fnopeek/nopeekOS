//! display/dc/hwss/dcn21/dcn21_hwseq.c (Linux 6.18.55): the DCN 2.1 steps of
//! the hw sequencer, as far as the hardware init reaches them.
#![allow(dead_code)]

use crate::amdgpu_device::amdgpu_device;
use crate::dc::dc;
use crate::dc_helper::{reg_get, reg_read};
use crate::dce_hwseq::dce_hwseq;
use crate::dcn20_hubbub::dcn_hubbub_phys_addr_config;

/// `dcn21_s0i3_golden_init_wa` (dcn21_hwseq.c): true, which skips the VBIOS
/// golden init, unless MICROSECOND_TIME_BASE_DIV reads 0x120464.
pub fn dcn21_s0i3_golden_init_wa(adev: &amdgpu_device, dc: &dc) -> bool {
    let hws = dc.hwseq.as_deref().expect("hwseq");
    let value = reg_read!(adev, hws.rsm().0, MICROSECOND_TIME_BASE_DIV);
    value != 0x00120464
}

/// `mmhub_update_page_table_config` (dcn21_hwseq.c): the GART base is the
/// one the MMHUB's VM context 0 holds.
fn mmhub_update_page_table_config(adev: &amdgpu_device, config: &mut dcn_hubbub_phys_addr_config, hws: &dce_hwseq) {
    let page_table_base_hi = reg_get!(adev, hws.rsm(), VM_CONTEXT0_PAGE_TABLE_BASE_ADDR_HI32, PAGE_DIRECTORY_ENTRY_HI32);
    let page_table_base_lo = reg_get!(adev, hws.rsm(), VM_CONTEXT0_PAGE_TABLE_BASE_ADDR_LO32, PAGE_DIRECTORY_ENTRY_LO32);

    config.gart_config.page_table_base_addr = ((page_table_base_hi as u64) << 32) | page_table_base_lo as u64;
}

/// `dcn21_init_sys_ctx` (dcn21_hwseq.c). Returns the number of VMIDs.
pub fn dcn21_init_sys_ctx(adev: &amdgpu_device, dc: &mut dc, pa_config: &crate::dc::dc_phy_addr_space_config) -> i32 {
    let mut config = dcn_hubbub_phys_addr_config::default();
    let a = &pa_config.system_aperture;
    let g = &pa_config.gart_config;

    config.system_aperture.fb_top = a.fb_top;
    config.system_aperture.fb_offset = a.fb_offset;
    config.system_aperture.fb_base = a.fb_base;
    config.system_aperture.agp_top = a.agp_top;
    config.system_aperture.agp_bot = a.agp_bot;
    config.system_aperture.agp_base = a.agp_base;
    config.gart_config.page_table_start_addr = g.page_table_start_addr;
    config.gart_config.page_table_end_addr = g.page_table_end_addr;
    config.gart_config.page_table_base_addr = g.page_table_base_addr;

    mmhub_update_page_table_config(adev, &mut config, dc.hwseq.as_deref().expect("hwseq"));

    let dc_config = dc.config;
    let hubbub = dc.res_pool.as_deref_mut().and_then(|p| p.hubbub.as_deref_mut()).expect("hubbub");
    (hubbub.funcs().init_dchub_sys_ctx.unwrap())(adev, &dc_config, hubbub, &config)
}
