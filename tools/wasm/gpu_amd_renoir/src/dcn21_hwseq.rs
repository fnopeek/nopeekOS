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

/// `dcn21_exit_optimized_pwr_state` (dcn21_hwseq.c).
pub fn dcn21_exit_optimized_pwr_state(adev: &amdgpu_device, dc: &mut dc, context: &mut crate::dc_state::dc_state) {
    crate::clk_mgr::clk_mgr_update_clocks(adev, dc, context, false);
}

/// `dcn21_optimize_pwr_state` (dcn21_hwseq.c).
pub fn dcn21_optimize_pwr_state(adev: &amdgpu_device, dc: &mut dc, context: &mut crate::dc_state::dc_state) {
    crate::clk_mgr::clk_mgr_update_clocks(adev, dc, context, true);
}

/// `dcn21_is_abm_supported` (dcn21_hwseq.c): ABM works on a stream that
/// is not split across ODM pipes.
pub fn dcn21_is_abm_supported(dc: &dc, context: &crate::dc_state::dc_state, stream: usize) -> bool {
    for i in 0..dc.res_pool.as_deref().unwrap().pipe_count as usize {
        let pipe_ctx = &context.res_ctx.pipe_ctx[i];

        if pipe_ctx.stream == Some(stream) && pipe_ctx.prev_odm_pipe.is_none() && pipe_ctx.next_odm_pipe.is_none() {
            return true;
        }
    }
    false
}

/// `dcn21_set_abm_immediate_disable` (dcn21_hwseq.c). A short-term
/// workaround for backlight ramping that paused midway leaves ABM alone at
/// level 0 or already disabled; DCN 2.1 has no DMCU.
pub fn dcn21_set_abm_immediate_disable(_adev: &amdgpu_device, dc: &mut dc, context: &crate::dc_state::dc_state, p: usize) {
    let pipe_ctx = &context.res_ctx.pipe_ctx[p];
    let stream = dc.streams[pipe_ctx.stream.unwrap()].as_deref().unwrap();

    if stream.abm_level == 0 || stream.abm_level == crate::regs::ABM_LEVEL_IMMEDIATE_DISABLE {
        return;
    }

    if pipe_ctx.stream_res.abm && dc.links[stream.link].panel_cntl.is_some() {
        crate::linux::Line::new().s("dcn21_set_abm_immediate_disable: dmub_abm_set_pipe, store_backlight_level not ported").say();
    }
}

/// `dcn21_set_pipe` (dcn21_hwseq.c): tell the ABM firmware which OTG and
/// panel the pipe drives. Renoir runs ABM on the DMCUB, so there is no
/// DMCU path.
pub fn dcn21_set_pipe(adev: &amdgpu_device, dc: &mut dc, pipe_ctx: &crate::dc_state::pipe_ctx) {
    let stream = dc.streams[pipe_ctx.stream.unwrap()].as_deref().unwrap();
    let Some(panel_cntl) = dc.links[stream.link].panel_cntl.as_deref() else { return };
    let (panel_inst, pwrseq_inst) = (panel_cntl.inst, panel_cntl.pwrseq_inst);
    let pool = dc.res_pool.as_deref().unwrap();
    let Some(abm) = pool.abm.as_deref().filter(|_| pipe_ctx.stream_res.abm) else { return };
    let Some(tg) = pipe_ctx.stream_res.tg else { return };

    let otg_inst = pool.timing_generators[tg].as_deref().unwrap().base.inst;

    let set_pipe_ex = abm.funcs().set_pipe_ex.unwrap();
    set_pipe_ex(adev, dc, otg_inst, crate::regs::SET_ABM_PIPE_NORMAL, panel_inst, pwrseq_inst);
}
