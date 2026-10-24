//! display/dc/hwss/dcn20/dcn20_hwseq.c (Linux 6.18.55): the DCN 2.0 hw
//! sequencer steps DCN 2.1 binds, as far as the hardware init reaches them —
//! VGA off, the power-gating domains, plane disable and the init-time blank.
//! A pipe is named by its index in the current state's `pipe_ctx`; the
//! `DOMAIN8`..`11` and `19`..`21` registers are not in DCN 2.1's list, so
//! Linux' `REG(...)` checks on them are false and their updates are absent.
#![allow(dead_code)]

use crate::amdgpu_device::amdgpu_device;
use crate::dc::dc;
use crate::dc_helper::{reg_get, reg_set, reg_update, reg_wait, reg_write};
use crate::dce_hwseq::dce_hwseq;
use crate::dcn20_opp::{dcn20_opp, tg_color};
use crate::dcn21_hubp::OPP_ID_INVALID;
use crate::hw_sequencer::color_space_to_black_color;
use crate::regs::*;

/// `dcn20_disable_vga` (dcn20_hwseq.c).
pub fn dcn20_disable_vga(adev: &amdgpu_device, hws: &dce_hwseq) {
    let r = hws.rsm().0;
    reg_write!(adev, r, D1VGA_CONTROL, 0);
    reg_write!(adev, r, D2VGA_CONTROL, 0);
    reg_write!(adev, r, D3VGA_CONTROL, 0);
    reg_write!(adev, r, D4VGA_CONTROL, 0);
    reg_write!(adev, r, D5VGA_CONTROL, 0);
    reg_write!(adev, r, D6VGA_CONTROL, 0);
}

/// `dcn20_enable_power_gating_plane` (dcn20_hwseq.c): let the HUBP, DPP and
/// DSC domains power gate (or force them on).
pub fn dcn20_enable_power_gating_plane(adev: &amdgpu_device, hws: &dce_hwseq, enable: bool) {
    let c = hws.rsm();
    // disable power gating
    let mut force_on = true;

    if enable {
        force_on = false;
    }

    let org_ip_request_cntl = reg_get!(adev, c, DC_IP_REQUEST_CNTL, IP_REQUEST_EN);
    if org_ip_request_cntl == 0 {
        reg_set!(adev, c, DC_IP_REQUEST_CNTL, 0, IP_REQUEST_EN, 1);
    }

    // DCHUBP0/1/2/3
    reg_update!(adev, c, DOMAIN0_PG_CONFIG, DOMAIN0_POWER_FORCEON, force_on);
    reg_update!(adev, c, DOMAIN2_PG_CONFIG, DOMAIN2_POWER_FORCEON, force_on);
    reg_update!(adev, c, DOMAIN4_PG_CONFIG, DOMAIN4_POWER_FORCEON, force_on);
    reg_update!(adev, c, DOMAIN6_PG_CONFIG, DOMAIN6_POWER_FORCEON, force_on);

    // DPP0/1/2/3
    reg_update!(adev, c, DOMAIN1_PG_CONFIG, DOMAIN1_POWER_FORCEON, force_on);
    reg_update!(adev, c, DOMAIN3_PG_CONFIG, DOMAIN3_POWER_FORCEON, force_on);
    reg_update!(adev, c, DOMAIN5_PG_CONFIG, DOMAIN5_POWER_FORCEON, force_on);
    reg_update!(adev, c, DOMAIN7_PG_CONFIG, DOMAIN7_POWER_FORCEON, force_on);

    // DCS0/1/2
    reg_update!(adev, c, DOMAIN16_PG_CONFIG, DOMAIN16_POWER_FORCEON, force_on);
    reg_update!(adev, c, DOMAIN17_PG_CONFIG, DOMAIN17_POWER_FORCEON, force_on);
    reg_update!(adev, c, DOMAIN18_PG_CONFIG, DOMAIN18_POWER_FORCEON, force_on);

    if org_ip_request_cntl == 0 {
        reg_set!(adev, c, DC_IP_REQUEST_CNTL, 0, IP_REQUEST_EN, 0);
    }
}

/// `dcn20_dsc_pg_control` (dcn20_hwseq.c).
pub fn dcn20_dsc_pg_control(adev: &amdgpu_device, dc: &dc, dsc_inst: u32, power_on: bool) {
    let hws = dc.hwseq.as_deref().expect("hwseq");
    let c = hws.rsm();
    let power_gate: u32 = if power_on { 0 } else { 1 };
    let pwr_status: u32 = if power_on { 0 } else { 2 };

    if dc.debug.disable_dsc_power_gate {
        return;
    }

    if c.0.DOMAIN16_PG_CONFIG == 0 {
        return;
    }

    let org_ip_request_cntl = reg_get!(adev, c, DC_IP_REQUEST_CNTL, IP_REQUEST_EN);
    if org_ip_request_cntl == 0 {
        reg_set!(adev, c, DC_IP_REQUEST_CNTL, 0, IP_REQUEST_EN, 1);
    }

    match dsc_inst {
        0 => {
            // DSC0
            reg_update!(adev, c, DOMAIN16_PG_CONFIG, DOMAIN16_POWER_GATE, power_gate);
            reg_wait!(adev, c, DOMAIN16_PG_STATUS, DOMAIN16_PGFSM_PWR_STATUS, pwr_status, 1, 1000,
                "dcn20_dsc_pg_control");
        }
        1 => {
            // DSC1
            reg_update!(adev, c, DOMAIN17_PG_CONFIG, DOMAIN17_POWER_GATE, power_gate);
            reg_wait!(adev, c, DOMAIN17_PG_STATUS, DOMAIN17_PGFSM_PWR_STATUS, pwr_status, 1, 1000,
                "dcn20_dsc_pg_control");
        }
        2 => {
            // DSC2
            reg_update!(adev, c, DOMAIN18_PG_CONFIG, DOMAIN18_POWER_GATE, power_gate);
            reg_wait!(adev, c, DOMAIN18_PG_STATUS, DOMAIN18_PGFSM_PWR_STATUS, pwr_status, 1, 1000,
                "dcn20_dsc_pg_control");
        }
        // DSC3..5: DOMAIN19..21 are not in DCN 2.1's list (REG is 0).
        3..=5 => {
            crate::linux::Line::new().s("dcn20_dsc_pg_control: no DOMAIN").dec(16 + dsc_inst as u64).s(" on DCN 2.1").say();
        }
        _ => {}
    }

    if org_ip_request_cntl == 0 {
        reg_set!(adev, c, DC_IP_REQUEST_CNTL, 0, IP_REQUEST_EN, 0);
    }
}

/// `dcn20_dpp_pg_control` (dcn20_hwseq.c).
pub fn dcn20_dpp_pg_control(adev: &amdgpu_device, dc: &dc, dpp_inst: u32, power_on: bool) {
    let hws = dc.hwseq.as_deref().expect("hwseq");
    let c = hws.rsm();
    let power_gate: u32 = if power_on { 0 } else { 1 };
    let pwr_status: u32 = if power_on { 0 } else { 2 };

    if dc.debug.disable_dpp_power_gate {
        return;
    }
    if c.0.DOMAIN1_PG_CONFIG == 0 {
        return;
    }

    match dpp_inst {
        0 => {
            // DPP0
            reg_update!(adev, c, DOMAIN1_PG_CONFIG, DOMAIN1_POWER_GATE, power_gate);
            reg_wait!(adev, c, DOMAIN1_PG_STATUS, DOMAIN1_PGFSM_PWR_STATUS, pwr_status, 1, 1000,
                "dcn20_dpp_pg_control");
        }
        1 => {
            // DPP1
            reg_update!(adev, c, DOMAIN3_PG_CONFIG, DOMAIN3_POWER_GATE, power_gate);
            reg_wait!(adev, c, DOMAIN3_PG_STATUS, DOMAIN3_PGFSM_PWR_STATUS, pwr_status, 1, 1000,
                "dcn20_dpp_pg_control");
        }
        2 => {
            // DPP2
            reg_update!(adev, c, DOMAIN5_PG_CONFIG, DOMAIN5_POWER_GATE, power_gate);
            reg_wait!(adev, c, DOMAIN5_PG_STATUS, DOMAIN5_PGFSM_PWR_STATUS, pwr_status, 1, 1000,
                "dcn20_dpp_pg_control");
        }
        3 => {
            // DPP3
            reg_update!(adev, c, DOMAIN7_PG_CONFIG, DOMAIN7_POWER_GATE, power_gate);
            reg_wait!(adev, c, DOMAIN7_PG_STATUS, DOMAIN7_PGFSM_PWR_STATUS, pwr_status, 1, 1000,
                "dcn20_dpp_pg_control");
        }
        // DPP4 (DOMAIN9) is not in DCN 2.1's list; DPP5 is never power
        // gated (de-featured, gating it can hard-reset).
        4 | 5 => {
            crate::linux::Line::new().s("dcn20_dpp_pg_control: no DPP").dec(dpp_inst as u64).s(" on DCN 2.1").say();
        }
        _ => {}
    }
}

/// `dcn20_hubp_pg_control` (dcn20_hwseq.c).
pub fn dcn20_hubp_pg_control(adev: &amdgpu_device, dc: &dc, hubp_inst: u32, power_on: bool) {
    let hws = dc.hwseq.as_deref().expect("hwseq");
    let c = hws.rsm();
    let power_gate: u32 = if power_on { 0 } else { 1 };
    let pwr_status: u32 = if power_on { 0 } else { 2 };

    if dc.debug.disable_hubp_power_gate {
        return;
    }
    if c.0.DOMAIN0_PG_CONFIG == 0 {
        return;
    }

    match hubp_inst {
        0 => {
            // DCHUBP0
            reg_update!(adev, c, DOMAIN0_PG_CONFIG, DOMAIN0_POWER_GATE, power_gate);
            reg_wait!(adev, c, DOMAIN0_PG_STATUS, DOMAIN0_PGFSM_PWR_STATUS, pwr_status, 1, 1000,
                "dcn20_hubp_pg_control");
        }
        1 => {
            // DCHUBP1
            reg_update!(adev, c, DOMAIN2_PG_CONFIG, DOMAIN2_POWER_GATE, power_gate);
            reg_wait!(adev, c, DOMAIN2_PG_STATUS, DOMAIN2_PGFSM_PWR_STATUS, pwr_status, 1, 1000,
                "dcn20_hubp_pg_control");
        }
        2 => {
            // DCHUBP2
            reg_update!(adev, c, DOMAIN4_PG_CONFIG, DOMAIN4_POWER_GATE, power_gate);
            reg_wait!(adev, c, DOMAIN4_PG_STATUS, DOMAIN4_PGFSM_PWR_STATUS, pwr_status, 1, 1000,
                "dcn20_hubp_pg_control");
        }
        3 => {
            // DCHUBP3
            reg_update!(adev, c, DOMAIN6_PG_CONFIG, DOMAIN6_POWER_GATE, power_gate);
            reg_wait!(adev, c, DOMAIN6_PG_STATUS, DOMAIN6_PGFSM_PWR_STATUS, pwr_status, 1, 1000,
                "dcn20_hubp_pg_control");
        }
        // DCHUBP4 (DOMAIN8) is not in DCN 2.1's list; DCHUBP5 is never
        // power gated.
        4 | 5 => {
            crate::linux::Line::new().s("dcn20_hubp_pg_control: no DCHUBP").dec(hubp_inst as u64).s(" on DCN 2.1").say();
        }
        _ => {}
    }
}

/// `dcn20_init_blank` (dcn20_hwseq.c): blank a running OTG's OPP(s) to
/// black through the display pattern generator, unless already blanked.
pub fn dcn20_init_blank(adev: &amdgpu_device, dc: &dc, tg_idx: usize) {
    let hws = dc.hwseq.as_deref().expect("hwseq");
    let pool = dc.res_pool.as_deref().expect("pool");
    let tg = pool.timing_generators[tg_idx].as_deref().expect("tg");
    let mut black_color = tg_color::default();
    let (mut num_opps, mut opp_id_src0, mut opp_id_src1) = (0u32, 0u32, 0u32);
    let (mut otg_active_width, mut otg_active_height) = (0u32, 0u32);

    // program opp dpg blank color
    let color_space = COLOR_SPACE_SRGB;
    color_space_to_black_color(color_space, &mut black_color);

    // get the OTG active size
    (tg.funcs().get_otg_active_size.unwrap())(adev, tg, &mut otg_active_width, &mut otg_active_height);

    // get the OPTC source
    (tg.funcs().get_optc_source.unwrap())(adev, tg, &mut num_opps, &mut opp_id_src0, &mut opp_id_src1);

    let num_opp = pool.res_cap.expect("res_cap").num_opp;
    if opp_id_src0 >= num_opp as u32 {
        return;
    }
    let opp = pool.opps[opp_id_src0 as usize].as_deref().expect("opp");

    // don't override the blank pattern if already enabled with the correct one.
    if let Some(f) = opp.funcs().dpg_is_blanked
        && f(adev, opp)
    {
        return;
    }

    let mut bottom_opp: Option<&dcn20_opp> = None;
    if num_opps == 2 {
        otg_active_width /= 2;

        if opp_id_src1 >= num_opp as u32 {
            return;
        }
        bottom_opp = pool.opps[opp_id_src1 as usize].as_deref();
    }

    (opp.funcs().opp_set_disp_pattern_generator.unwrap())(adev, opp, CONTROLLER_DP_TEST_PATTERN_SOLID_COLOR,
        CONTROLLER_DP_COLOR_SPACE_UDEFINED, COLOR_DEPTH_UNDEFINED, &black_color, otg_active_width as i32,
        otg_active_height as i32, 0);

    if num_opps == 2 {
        let bottom_opp = bottom_opp.expect("bottom opp");
        (bottom_opp.funcs().opp_set_disp_pattern_generator.unwrap())(adev, bottom_opp,
            CONTROLLER_DP_TEST_PATTERN_SOLID_COLOR, CONTROLLER_DP_COLOR_SPACE_UDEFINED, COLOR_DEPTH_UNDEFINED,
            &black_color, otg_active_width as i32, otg_active_height as i32, 0);
    }

    (hws.funcs.wait_for_blank_complete.unwrap())(adev, opp);
}

/// `dcn20_wait_for_blank_complete` (dcn20_hwseq.c): up to 100 ms for the
/// pattern generator to take the blank.
pub fn dcn20_wait_for_blank_complete(adev: &amdgpu_device, opp: &dcn20_opp) -> bool {
    let mut counter = 0;
    while counter < 1000 {
        if !(opp.funcs().dpg_is_pending.unwrap())(adev, opp) {
            break;
        }

        crate::linux::udelay(100);
        counter += 1;
    }

    if counter == 1000 {
        crate::linux::Line::new().s("DC: failed to blank crtc!").say();
        return false;
    }

    (opp.funcs().dpg_is_blanked.unwrap())(adev, opp)
}

/// `dcn20_set_flip_control_gsl` (dcn20_hwseq.c).
pub fn dcn20_set_flip_control_gsl(adev: &amdgpu_device, dc: &dc, pipe_idx: usize, flip_immediate: bool) {
    let pipe = &dc.current_state.as_deref().expect("state").res_ctx.pipe_ctx[pipe_idx];
    let pool = dc.res_pool.as_deref().expect("pool");
    let hubp = pool.hubps[pipe.plane_res.hubp.expect("hubp")].as_deref().expect("hubp");
    if let Some(f) = hubp.funcs().hubp_set_flip_control_surface_gsl {
        f(adev, hubp, flip_immediate);
    }
}

/// `dcn20_plane_atomic_disable` (dcn20_hwseq.c): stop the plane's clocks,
/// power its HUBP and DPP down and clear the pipe. A GSL group is only set
/// by a flip-immediate split, which no pipe has before the first commit.
pub fn dcn20_plane_atomic_disable(adev: &amdgpu_device, dc: &mut dc, pipe_idx: usize) {
    let pipe = dc.current_state.as_deref().expect("state").res_ctx.pipe_ctx[pipe_idx];
    let hubp_idx = pipe.plane_res.hubp.expect("hubp");
    let dpp_idx = pipe.plane_res.dpp.expect("dpp");

    (dc.hwss.wait_for_mpcc_disconnect.unwrap())(adev, dc, &pipe);

    // In flip immediate with pipe splitting case GSL is used for
    // synchronization so we must disable it when the plane is disabled.
    if pipe.stream_res.gsl_group != 0 {
        crate::linux::Line::new().s("dcn20_plane_atomic_disable: dcn20_setup_gsl_group_as_lock not ported").say();
    }

    // hubp_update_mall_sel is not a member of dcn21_hubp_funcs.

    (dc.hwss.set_flip_control_gsl.unwrap())(adev, dc, pipe_idx, false);

    {
        let pool = dc.res_pool.as_deref_mut().expect("pool");
        let hubp = pool.hubps[hubp_idx].as_deref_mut().expect("hubp");
        (hubp.funcs().hubp_clk_cntl.unwrap())(adev, hubp, false);

        let dpp = pool.dpps[dpp_idx].as_deref().expect("dpp");
        (dpp.funcs().dpp_dppclk_control.unwrap())(adev, dpp, false, false);

        pool.hubps[hubp_idx].as_deref_mut().unwrap().base.power_gated = true;
    }

    let f = dc.hwseq.as_deref().expect("hwseq").funcs.plane_atomic_power_down.unwrap();
    f(adev, dc, dpp_idx, hubp_idx);

    let pipe = &mut dc.current_state.as_deref_mut().expect("state").res_ctx.pipe_ctx[pipe_idx];
    pipe.stream = None;
    pipe.stream_res = Default::default();
    pipe.plane_res = Default::default();
    pipe.top_pipe = None;
    pipe.bottom_pipe = None;
    pipe.prev_odm_pipe = None;
    pipe.next_odm_pipe = None;
    pipe.plane_state = None;
}

/// `dcn20_disable_plane` (dcn20_hwseq.c). A phantom pipe belongs to SubVP,
/// which DCN 2.1 does not have.
pub fn dcn20_disable_plane(adev: &amdgpu_device, dc: &mut dc, pipe_idx: usize) {
    let state = dc.current_state.as_deref().expect("state");
    let pipe = state.res_ctx.pipe_ctx[pipe_idx];
    let is_phantom = crate::dc_state::dc_state_get_pipe_subvp_type(state, &pipe) == SUBVP_PHANTOM;

    let Some(hubp_idx) = pipe.plane_res.hubp else { return };
    if dc.res_pool.as_deref().unwrap().hubps[hubp_idx].as_deref().unwrap().base.power_gated {
        return;
    }

    dcn20_plane_atomic_disable(adev, dc, pipe_idx);

    // Turn back off the phantom OTG after the phantom plane is fully disabled
    if is_phantom {
        crate::linux::Line::new().s("dcn20_disable_plane: disable_phantom_crtc not ported").say();
    }
}

/// `OPP_ID_INVALID` as the u32 the DSC and OPTC sources compare against.
pub const OPP_ID_INVALID_U32: u32 = OPP_ID_INVALID as u32;

/// `dcn20_prepare_bandwidth` (dcn20_hwseq.c): raise the clocks and the
/// watermarks the new state needs before it is applied. A SubVP pipe's
/// watermark override and the compbuf size are not DCN 2.1's.
pub fn dcn20_prepare_bandwidth(adev: &amdgpu_device, dc: &mut dc, context: &mut crate::dc_state::dc_state) {
    crate::clk_mgr::clk_mgr_update_clocks(adev, dc, context, false);

    // program dchubbub watermarks: optimized_required takes |=, since it must
    // not be cleared before the optimize has happened
    let refclk_mhz = dc.res_pool.as_deref().unwrap().ref_clocks.dchub_ref_clock_inKhz / 1000;
    let mut hubbub = dc.res_pool.as_deref_mut().unwrap().hubbub.take().unwrap();
    let pending = (hubbub.funcs().program_watermarks.unwrap())(adev, dc, &mut hubbub, &context.bw_ctx.dcn.watermarks,
        refclk_mhz, false);
    dc.res_pool.as_deref_mut().unwrap().hubbub = Some(hubbub);
    dc.optimized_required |= pending;

    // hubbub21_funcs has no program_compbuf_size.
}

/// `dcn20_reset_hw_ctx_wrap` (dcn20_hwseq.c): reset the back end of every
/// OTG master of the current state the new one drops or reprograms.
pub fn dcn20_reset_hw_ctx_wrap(adev: &amdgpu_device, dc: &mut dc, context: &mut crate::dc_state::dc_state) {
    let pipe_count = dc.res_pool.as_deref().unwrap().pipe_count as usize;

    // Reset Back End
    for i in (0..pipe_count).rev() {
        let pipe_ctx_old = dc.current_state.as_deref().unwrap().res_ctx.pipe_ctx[i];
        let pipe_ctx = &context.res_ctx.pipe_ctx[i];

        if pipe_ctx_old.stream.is_none() {
            continue;
        }

        if pipe_ctx_old.top_pipe.is_some() || pipe_ctx_old.prev_odm_pipe.is_some() {
            continue;
        }

        let _ = (adev, pipe_ctx);
        crate::linux::Line::new().s("dcn20_reset_hw_ctx_wrap: pipe_need_reprogram, dcn20_reset_back_end_for_pipe not ported").say();
    }
}

/// `dcn20_disable_stream_gating` (dcn20_hwseq.c): power the stream's DSCs.
pub fn dcn20_disable_stream_gating(adev: &amdgpu_device, dc: &mut dc, context: &crate::dc_state::dc_state, p: usize) {
    let pipe_ctx = &context.res_ctx.pipe_ctx[p];

    if let Some(dsc) = pipe_ctx.stream_res.dsc {
        let pg = dc.hwseq.as_deref().unwrap().funcs.dsc_pg_control.unwrap();
        let inst = dc.res_pool.as_deref().unwrap().dscs[dsc].as_deref().unwrap().base.inst;
        pg(adev, dc, inst, true);
        let mut odm_pipe = pipe_ctx.next_odm_pipe;
        while let Some(o) = odm_pipe {
            let dsc = context.res_ctx.pipe_ctx[o].stream_res.dsc.unwrap();
            let inst = dc.res_pool.as_deref().unwrap().dscs[dsc].as_deref().unwrap().base.inst;
            pg(adev, dc, inst, true);
            odm_pipe = context.res_ctx.pipe_ctx[o].next_odm_pipe;
        }
    }
}

/// `calc_mpc_flow_ctrl_cnt` (dcn20_hwseq.c).
fn calc_mpc_flow_ctrl_cnt(stream: &crate::dc_stream::dc_stream_state, opp_cnt: i32, is_two_pixels_per_container: bool)
                          -> i32 {
    let mut hblank_halved = is_two_pixels_per_container;

    if opp_cnt >= 2 {
        hblank_halved = true;
    }

    let mut flow_ctrl_cnt = (stream.timing.h_total - stream.timing.h_addressable - stream.timing.h_border_left
        - stream.timing.h_border_right) as i32;

    if hblank_halved {
        flow_ctrl_cnt /= 2;
    }

    // ODM combine 4:1 case
    if opp_cnt == 4 {
        flow_ctrl_cnt /= 2;
    }

    flow_ctrl_cnt
}

/// `dcn20_enable_stream_timing` (dcn20_hwseq.c): the OTG's clock, the
/// pixel clock, the timing, the MPC rate control and the OPPs behind it,
/// blanked, then the OTG runs. The pipe is the OTG master; its children
/// share its back end. DCN 2.1 has no pixel rate divider, no 128b/132b
/// and no `PLAT_58856_wa` for DP.
pub fn dcn20_enable_stream_timing(adev: &amdgpu_device, dc: &mut dc, context: &mut crate::dc_state::dc_state, p: usize)
                                  -> crate::dc_types::dc_status {
    use crate::dc_types::dc_status;
    let pipe_ctx = context.res_ctx.pipe_ctx[p];
    let s = pipe_ctx.stream.unwrap();
    let tg_idx = pipe_ctx.stream_res.tg.unwrap();
    let mut opp_inst = [0i32; crate::dc_types::MAX_PIPES];

    let stream = dc.streams[s].as_deref().unwrap();
    let interlace = stream.timing.flags.INTERLACE;
    let tg = dc.res_pool.as_deref().unwrap().timing_generators[tg_idx].as_deref().unwrap();
    let is_two_pixels_per_container = (tg.funcs().is_two_pixels_per_container.unwrap())(&stream.timing);
    let mut rate_control_2x_pclk = interlace || is_two_pixels_per_container;

    // by upper caller loop, pipe0 is parent pipe and be called first. back
    // end is set up by for pipe0. Other children pipe share back end with
    // pipe 0. No program is needed.
    if pipe_ctx.top_pipe.is_some() {
        return dc_status::DC_OK;
    }

    // TODO check if timing_changed, disable stream if timing changed

    let opp_heads = crate::dc_resource::resource_get_opp_heads_for_otg_master(&context.res_ctx, p);
    let opp_cnt = opp_heads.len() as i32;
    let pool = dc.res_pool.as_deref().unwrap();
    for (i, &h) in opp_heads.iter().enumerate() {
        opp_inst[i] = pool.opps[context.res_ctx.pipe_ctx[h].stream_res.opp.unwrap()].as_deref().unwrap().base.inst as i32;
    }

    let _odm_slice_width = crate::dc_resource::resource_get_odm_slice_dst_width(&context.res_ctx, p, stream, false);
    let _last_odm_slice_width = crate::dc_resource::resource_get_odm_slice_dst_width(&context.res_ctx, p, stream, true);
    if opp_cnt > 1 {
        crate::linux::Line::new().s("dcn20_enable_stream_timing: optc2_set_odm_combine not ported").say();
    }

    // HW program guide assume display already disable by unplug sequence.
    // OTG assume stop.
    (tg.funcs().enable_optc_clock.unwrap())(adev, tg, true);

    let cs = pipe_ctx.clock_source.unwrap();
    let program_pix_clk = crate::dc_resource::clock_source_of(pool, cs).funcs().program_pix_clk.unwrap();
    let encoding = crate::link_dp_capability::link_dp_get_encoding_format(&pipe_ctx.link_config.dp_link_settings);
    if !program_pix_clk(adev, dc, cs, &pipe_ctx.stream_res.pix_clk_params, encoding as u32, &pipe_ctx.pll_settings) {
        return dc_status::DC_ERROR_UNEXPECTED;
    }

    // dp_is_128b_132b_signal is false on DCN 2.1.

    let stream = dc.streams[s].as_deref().unwrap();
    if crate::link_detection::dc_is_hdmi_tmds_signal(stream.signal) {
        crate::linux::Line::new().s("dcn20_enable_stream_timing: symclk state not ported").say();
    }

    // PLAT_58856_wa applies to a non-DP signal only.
    if !crate::link_detection::dc_is_dp_signal(stream.signal) {
        crate::linux::Line::new().s("dcn20_enable_stream_timing: dcn21_PLAT_58856_wa not ported").say();
    }

    let timing = stream.timing;
    let signal = stream.signal;
    let dlg = pipe_ctx.pipe_dlg_param;
    {
        let tg = dc.res_pool.as_deref_mut().unwrap().timing_generators[tg_idx].as_deref_mut().unwrap();
        (tg.funcs().program_timing.unwrap())(adev, tg, &timing, dlg.vready_offset as i32, dlg.vstartup_start as i32,
            dlg.vupdate_offset as i32, dlg.vupdate_width as i32, dlg.pstate_keepout as i32, signal, true);
    }

    // The MPC's output rate control (2x pixel clock, flow control counts
    // from calc_mpc_flow_ctrl_cnt) is not a member of dcn20_mpc_funcs.
    rate_control_2x_pclk = rate_control_2x_pclk || opp_cnt > 1;
    let _ = (rate_control_2x_pclk, opp_inst, calc_mpc_flow_ctrl_cnt(dc.streams[s].as_deref().unwrap(), opp_cnt,
        is_two_pixels_per_container));

    {
        let pool = dc.res_pool.as_deref().unwrap();
        let pixel_encoding = dc.streams[s].as_deref().unwrap().timing.pixel_encoding;
        for &h in &opp_heads {
            let head = &context.res_ctx.pipe_ctx[h];
            let opp = pool.opps[head.stream_res.opp.unwrap()].as_deref().unwrap();
            (opp.funcs().opp_pipe_clock_control.unwrap())(adev, opp, true);
            (opp.funcs().opp_program_left_edge_extra_pixel.unwrap())(adev, &dc.debug, opp, pixel_encoding,
                crate::dc_resource::resource_is_pipe_type(head, crate::dc_resource::pipe_type::OTG_MASTER));
        }
    }

    (dc.hwseq.as_deref().unwrap().funcs.blank_pixel_data.unwrap())(adev, dc, context, p, true);

    // VTG is  within DCHUB command block. DCFCLK is always on
    let tg = dc.res_pool.as_deref().unwrap().timing_generators[tg_idx].as_deref().unwrap();
    if !(tg.funcs().enable_crtc.unwrap())(adev, tg) {
        return dc_status::DC_ERROR_UNEXPECTED;
    }

    let stream = dc.streams[s].as_deref().unwrap();
    crate::linux::udelay((stream.timing.v_total * (stream.timing.h_total * 10000 / stream.timing.pix_clk_100hz)) as u64);

    let params = crate::dcn20_optc::drr_params {
        vertical_total_min: stream.adjust.v_total_min,
        vertical_total_max: stream.adjust.v_total_max,
        vertical_total_mid: stream.adjust.v_total_mid,
        vertical_total_mid_frame_num: stream.adjust.v_total_mid_frame_num,
    };
    let mut event_triggers = 0;
    if stream.adjust.v_total_min != 0 && stream.adjust.v_total_max != 0 {
        event_triggers = 0x80;
    }
    crate::hw_sequencer::set_drr_and_clear_adjust_pending(adev, dc, &pipe_ctx, Some(s), Some(&params));

    // DRR should set trigger event to monitor surface update event. Event
    // triggers and num frames initialized for DRR, but can be later updated
    // for PSR use. Note DRR trigger events are generated regardless of
    // whether num frames met.
    let tg = dc.res_pool.as_deref().unwrap().timing_generators[tg_idx].as_deref().unwrap();
    if let Some(f) = tg.funcs().set_static_screen_control {
        f(adev, tg, event_triggers, 2);
    }

    // A SubVP phantom is not DCN 2.1's.

    dc_status::DC_OK
}

/// `dcn20_set_disp_pattern_generator` (dcn20_hwseq.c).
#[allow(clippy::too_many_arguments)]
pub fn dcn20_set_disp_pattern_generator(adev: &amdgpu_device, dc: &mut dc, context: &crate::dc_state::dc_state,
                                        p: usize, test_pattern: u32, color_space: u32, color_depth: u32,
                                        solid_color: Option<&crate::dcn20_opp::tg_color>, width: i32, height: i32,
                                        offset: i32) {
    let opp = dc.res_pool.as_deref().unwrap().opps[context.res_ctx.pipe_ctx[p].stream_res.opp.unwrap()]
        .as_deref().unwrap();
    let black = crate::dcn20_opp::tg_color::default();
    (opp.funcs().opp_set_disp_pattern_generator.unwrap())(adev, opp, test_pattern, color_space, color_depth,
        solid_color.unwrap_or(&black), width, height, offset);
}

/// `dcn20_blank_pixel_data` (dcn20_hwseq.c): a blanked stream shows the
/// black of its colour space from the OPP's pattern generator (colour
/// squares with visual confirm), an unblanked one its video.
pub fn dcn20_blank_pixel_data(adev: &amdgpu_device, dc: &mut dc, context: &crate::dc_state::dc_state, p: usize,
                              blank: bool) {
    let mut black_color = crate::dcn20_opp::tg_color::default();
    let pipe_ctx = context.res_ctx.pipe_ctx[p];
    let s = pipe_ctx.stream.unwrap();
    let stream = dc.streams[s].as_deref().unwrap();
    let color_space = stream.output_color_space;
    let mut test_pattern = CONTROLLER_DP_TEST_PATTERN_SOLID_COLOR;
    let mut test_pattern_color_space = CONTROLLER_DP_COLOR_SPACE_UDEFINED;

    if dc.links[stream.link].test_pattern_enabled {
        return;
    }

    // get opp dpg blank color
    crate::hw_sequencer::color_space_to_black_color(color_space, &mut black_color);

    if blank {
        (dc.hwss.set_abm_immediate_disable.unwrap())(adev, dc, context, p);

        if dc.debug.visual_confirm != VISUAL_CONFIRM_DISABLE {
            test_pattern = CONTROLLER_DP_TEST_PATTERN_COLORSQUARES;
            test_pattern_color_space = CONTROLLER_DP_COLOR_SPACE_RGB;
        }
    } else {
        test_pattern = CONTROLLER_DP_TEST_PATTERN_VIDEOMODE;
    }

    let depth = dc.streams[s].as_deref().unwrap().timing.display_color_depth;
    let mut odm_pipe = p;

    while let Some(next) = context.res_ctx.pipe_ctx[odm_pipe].next_odm_pipe {
        let src = crate::dc_resource::resource_get_odm_slice_src_rect(&dc.debug, &context.res_ctx, odm_pipe,
            dc.streams[s].as_deref().unwrap());
        (dc.hwss.set_disp_pattern_generator.unwrap())(adev, dc, context, odm_pipe, test_pattern,
            test_pattern_color_space, depth, Some(&black_color), src.width, src.height, src.x);
        odm_pipe = next;
    }

    let src = crate::dc_resource::resource_get_odm_slice_src_rect(&dc.debug, &context.res_ctx, odm_pipe,
        dc.streams[s].as_deref().unwrap());
    (dc.hwss.set_disp_pattern_generator.unwrap())(adev, dc, context, odm_pipe, test_pattern, test_pattern_color_space,
        depth, Some(&black_color), src.width, src.height, src.x);

    if !blank && pipe_ctx.stream_res.abm {
        (dc.hwss.set_pipe.unwrap())(adev, dc, &pipe_ctx);
        let abm_level = dc.streams[s].as_deref().unwrap().abm_level;
        let abm = dc.res_pool.as_deref().unwrap().abm.as_deref().unwrap();
        (abm.funcs().set_abm_level.unwrap())(adev, dc, abm_level);
    }
}

/// `dcn20_setup_vupdate_interrupt` (dcn20_hwseq.c).
pub fn dcn20_setup_vupdate_interrupt(adev: &amdgpu_device, dc: &mut dc, context: &crate::dc_state::dc_state, p: usize) {
    let mut start_line = (dc.hwss.get_vupdate_offset_from_vsync.unwrap())(dc, context, p);

    if start_line < 0 {
        start_line = 0;
    }

    let tg = dc.res_pool.as_deref().unwrap().timing_generators[context.res_ctx.pipe_ctx[p].stream_res.tg.unwrap()]
        .as_deref().unwrap();
    if let Some(f) = tg.funcs().setup_vertical_interrupt2 {
        f(adev, tg, start_line as u32);
    }
}

/// `dcn20_enable_stream` (dcn20_hwseq.c): the DIG front end onto the back
/// end, the info frames, and the early control (empty on DCN 2.x). DCN 2.1
/// has no 128b/132b, no per-stream symclk control and no pixel rate
/// divider.
pub fn dcn20_enable_stream(adev: &amdgpu_device, dc: &mut dc, context: &mut crate::dc_state::dc_state, p: usize) {
    let pipe_ctx = context.res_ctx.pipe_ctx[p];
    let s = pipe_ctx.stream.unwrap();
    let li = dc.streams[s].as_deref().unwrap().link;
    let lane_count = dc.links[li].cur_link_settings.lane_count;
    let link_hwss = crate::link_hwss::get_link_hwss(dc, li, &pipe_ctx.link_res);

    crate::link_hwss::setup_stream_encoder(adev, dc, context, p, link_hwss);

    // program_dmdata_engine is for a plane that does not flip immediately;
    // the commit's pipes have no plane yet.
    if let Some(plane) = pipe_ctx.plane_state
        && !dc.planes[plane].as_deref().unwrap().flip_immediate
    {
        crate::linux::Line::new().s("dcn20_enable_stream: dcn20_program_dmdata_engine not ported").say();
    }

    (dc.hwss.update_info_frame.unwrap())(adev, dc, context, p);

    let signal = dc.streams[s].as_deref().unwrap().signal;
    if crate::link_detection::dc_is_dp_signal(signal) {
        crate::link_dpms::dp_trace_source_sequence(adev, dc, li, DPCD_SOURCE_SEQ_AFTER_UPDATE_INFO_FRAME);
    }

    // enable early control to avoid corruption on DP monitor
    let timing = &dc.streams[s].as_deref().unwrap().timing;
    let active_total_with_borders = timing.h_addressable + timing.h_border_left + timing.h_border_right;
    let mut early_control = 0;

    if lane_count != 0 {
        early_control = active_total_with_borders % lane_count;
    }

    if early_control == 0 {
        early_control = lane_count;
    }

    let tg = dc.res_pool.as_deref().unwrap().timing_generators[pipe_ctx.stream_res.tg.unwrap()].as_deref().unwrap();
    (tg.funcs().set_early_control.unwrap())(adev, tg, early_control);
}

/// `dcn20_unblank_stream` (dcn20_hwseq.c): the stream encoder sends video;
/// an eDP's backlight goes on.
pub fn dcn20_unblank_stream(adev: &amdgpu_device, dc: &mut dc, context: &mut crate::dc_state::dc_state, p: usize,
                            link_settings: &crate::dc_dp_types::dc_link_settings) {
    let pipe_ctx = context.res_ctx.pipe_ctx[p];
    let s = pipe_ctx.stream.unwrap();
    let stream = dc.streams[s].as_deref().unwrap();
    let li = stream.link;
    let pool = dc.res_pool.as_deref().unwrap();
    let tg = pool.timing_generators[pipe_ctx.stream_res.tg.unwrap()].as_deref().unwrap();
    let is_two_pixels_per_container = (tg.funcs().is_two_pixels_per_container.unwrap())(&stream.timing);

    let mut params = crate::dcn20_stream_encoder::encoder_unblank_param { opp_cnt: 1, ..Default::default() };

    let mut odm_pipe = pipe_ctx.next_odm_pipe;
    while let Some(o) = odm_pipe {
        params.opp_cnt += 1;
        odm_pipe = context.res_ctx.pipe_ctx[o].next_odm_pipe;
    }
    // only 3 items below are used by unblank
    params.timing = stream.timing;

    params.link_settings.link_rate = link_settings.link_rate;

    if crate::link_detection::dc_is_dp_signal(stream.signal) {
        if is_two_pixels_per_container || params.opp_cnt > 1 {
            params.timing.pix_clk_100hz /= 2;
        }
        let se_idx = pipe_ctx.stream_res.stream_enc.unwrap();
        let se = pool.stream_enc[se_idx].as_ref().unwrap();
        if let Some(f) = se.funcs().dp_set_odm_combine {
            f(adev, se, params.opp_cnt > 1);
        }
        (se.funcs().dp_unblank.unwrap())(adev, dc, li, se_idx, &params);
    }

    if dc.links[li].local_sink.as_deref().is_some_and(|s| s.sink_signal == LF_SIGNAL_TYPE_EDP) {
        (dc.hwseq.as_deref().unwrap().funcs.edp_backlight_control.unwrap())(adev, dc, li, true);
    }
}

/// `dcn20_detect_pipe_changes` (dcn20_hwseq.c): what of a pipe the new
/// state changes, as update flags on the new pipe. DCN 2.1 has no SubVP
/// phantoms; a stream's test pattern parameters join with test patterns.
fn dcn20_detect_pipe_changes(old_pipe: &crate::dc_state::pipe_ctx, new_pipe: &mut crate::dc_state::pipe_ctx,
                             old_res: &crate::dc_state::resource_context, new_res: &crate::dc_state::resource_context) {
    use crate::dc_resource::{pipe_type, resource_is_pipe_type};
    use crate::dc_state::*;

    new_pipe.update_flags = 0;

    if resource_is_pipe_type(new_pipe, pipe_type::OTG_MASTER)
        && crate::dc_resource::resource_is_odm_topology_changed(new_res, new_pipe, old_res, old_pipe)
    {
        // Detect odm changes
        new_pipe.update_flags |= PIPE_UPDATE_ODM;
    }

    // Exit on unchanged, unused pipe
    if old_pipe.plane_state.is_none() && new_pipe.plane_state.is_none() {
        return;
    }
    // Detect pipe enable/disable
    if old_pipe.plane_state.is_none() && new_pipe.plane_state.is_some() {
        new_pipe.update_flags |= PIPE_UPDATE_ENABLE | PIPE_UPDATE_MPCC | PIPE_UPDATE_DPPCLK
            | PIPE_UPDATE_HUBP_INTERDEPENDENT | PIPE_UPDATE_HUBP_RQ_DLG_TTU | PIPE_UPDATE_UNBOUNDED_REQ
            | PIPE_UPDATE_GAMUT_REMAP | PIPE_UPDATE_SCALER | PIPE_UPDATE_VIEWPORT | PIPE_UPDATE_DET_SIZE;
        // A stream's test pattern (other than video mode) joins with test
        // patterns.
        if new_pipe.top_pipe.is_none() && new_pipe.prev_odm_pipe.is_none() {
            new_pipe.update_flags |= PIPE_UPDATE_ODM | PIPE_UPDATE_GLOBAL_SYNC;
        }
        return;
    }

    if old_pipe.plane_state.is_some() && new_pipe.plane_state.is_none() {
        new_pipe.update_flags |= PIPE_UPDATE_DISABLE;
        return;
    }

    // Detect plane change
    if old_pipe.plane_state != new_pipe.plane_state {
        new_pipe.update_flags |= PIPE_UPDATE_PLANE_CHANGED;
    }

    // Detect top pipe only changes
    if resource_is_pipe_type(new_pipe, pipe_type::OTG_MASTER) {
        // Detect global sync changes
        let (o, n) = (&old_pipe.pipe_dlg_param, &new_pipe.pipe_dlg_param);
        if o.vready_offset != n.vready_offset || o.vstartup_start != n.vstartup_start
            || o.vupdate_offset != n.vupdate_offset || o.vupdate_width != n.vupdate_width
        {
            new_pipe.update_flags |= PIPE_UPDATE_GLOBAL_SYNC;
        }
    }

    if old_pipe.det_buffer_size_kb != new_pipe.det_buffer_size_kb {
        new_pipe.update_flags |= PIPE_UPDATE_DET_SIZE;
    }

    // Detect opp / tg change, only set on change, not on enable. This
    // assumes mpcc inst = pipe index, as the whole sequence does (hubp reset
    // is matched to the same index mpcc reset).
    if old_pipe.stream_res.opp != new_pipe.stream_res.opp {
        new_pipe.update_flags |= PIPE_UPDATE_OPP_CHANGED;
    }
    if old_pipe.stream_res.tg != new_pipe.stream_res.tg {
        new_pipe.update_flags |= PIPE_UPDATE_TG_CHANGED;
    }

    // Detect mpcc blending changes, only dpp inst and opp matter here,
    // mpccs getting removed/inserted update connected ones during their own
    // programming
    if old_pipe.plane_res.dpp != new_pipe.plane_res.dpp || old_pipe.stream_res.opp != new_pipe.stream_res.opp {
        new_pipe.update_flags |= PIPE_UPDATE_MPCC;
    }

    // Detect dppclk change
    if old_pipe.plane_res.bw_dppclk_khz != new_pipe.plane_res.bw_dppclk_khz {
        new_pipe.update_flags |= PIPE_UPDATE_DPPCLK;
    }

    // Check for scl update
    if old_pipe.plane_res.scl_data != new_pipe.plane_res.scl_data {
        new_pipe.update_flags |= PIPE_UPDATE_SCALER;
    }
    // Check for vp update
    if old_pipe.plane_res.scl_data.viewport != new_pipe.plane_res.scl_data.viewport
        || old_pipe.plane_res.scl_data.viewport_c != new_pipe.plane_res.scl_data.viewport_c
    {
        new_pipe.update_flags |= PIPE_UPDATE_VIEWPORT;
    }

    // Detect dlg/ttu/rq updates
    {
        use crate::dml_math::BitsEq;
        let mut old_dlg_attr = old_pipe.dlg_regs;
        let mut old_ttu_attr = old_pipe.ttu_regs;
        let new_dlg_attr = &new_pipe.dlg_regs;
        let new_ttu_attr = &new_pipe.ttu_regs;

        // Detect pipe interdependent updates
        if old_dlg_attr.dst_y_prefetch != new_dlg_attr.dst_y_prefetch
            || old_dlg_attr.vratio_prefetch != new_dlg_attr.vratio_prefetch
            || old_dlg_attr.vratio_prefetch_c != new_dlg_attr.vratio_prefetch_c
            || old_dlg_attr.dst_y_per_vm_vblank != new_dlg_attr.dst_y_per_vm_vblank
            || old_dlg_attr.dst_y_per_row_vblank != new_dlg_attr.dst_y_per_row_vblank
            || old_dlg_attr.dst_y_per_vm_flip != new_dlg_attr.dst_y_per_vm_flip
            || old_dlg_attr.dst_y_per_row_flip != new_dlg_attr.dst_y_per_row_flip
            || old_dlg_attr.refcyc_per_meta_chunk_vblank_l != new_dlg_attr.refcyc_per_meta_chunk_vblank_l
            || old_dlg_attr.refcyc_per_meta_chunk_vblank_c != new_dlg_attr.refcyc_per_meta_chunk_vblank_c
            || old_dlg_attr.refcyc_per_meta_chunk_flip_l != new_dlg_attr.refcyc_per_meta_chunk_flip_l
            || old_dlg_attr.refcyc_per_line_delivery_pre_l != new_dlg_attr.refcyc_per_line_delivery_pre_l
            || old_dlg_attr.refcyc_per_line_delivery_pre_c != new_dlg_attr.refcyc_per_line_delivery_pre_c
            || old_ttu_attr.refcyc_per_req_delivery_pre_l != new_ttu_attr.refcyc_per_req_delivery_pre_l
            || old_ttu_attr.refcyc_per_req_delivery_pre_c != new_ttu_attr.refcyc_per_req_delivery_pre_c
            || old_ttu_attr.refcyc_per_req_delivery_pre_cur0 != new_ttu_attr.refcyc_per_req_delivery_pre_cur0
            || old_ttu_attr.refcyc_per_req_delivery_pre_cur1 != new_ttu_attr.refcyc_per_req_delivery_pre_cur1
            || old_ttu_attr.min_ttu_vblank != new_ttu_attr.min_ttu_vblank
            || old_ttu_attr.qos_level_flip != new_ttu_attr.qos_level_flip
        {
            old_dlg_attr.dst_y_prefetch = new_dlg_attr.dst_y_prefetch;
            old_dlg_attr.vratio_prefetch = new_dlg_attr.vratio_prefetch;
            old_dlg_attr.vratio_prefetch_c = new_dlg_attr.vratio_prefetch_c;
            old_dlg_attr.dst_y_per_vm_vblank = new_dlg_attr.dst_y_per_vm_vblank;
            old_dlg_attr.dst_y_per_row_vblank = new_dlg_attr.dst_y_per_row_vblank;
            old_dlg_attr.dst_y_per_vm_flip = new_dlg_attr.dst_y_per_vm_flip;
            old_dlg_attr.dst_y_per_row_flip = new_dlg_attr.dst_y_per_row_flip;
            old_dlg_attr.refcyc_per_meta_chunk_vblank_l = new_dlg_attr.refcyc_per_meta_chunk_vblank_l;
            old_dlg_attr.refcyc_per_meta_chunk_vblank_c = new_dlg_attr.refcyc_per_meta_chunk_vblank_c;
            old_dlg_attr.refcyc_per_meta_chunk_flip_l = new_dlg_attr.refcyc_per_meta_chunk_flip_l;
            old_dlg_attr.refcyc_per_line_delivery_pre_l = new_dlg_attr.refcyc_per_line_delivery_pre_l;
            old_dlg_attr.refcyc_per_line_delivery_pre_c = new_dlg_attr.refcyc_per_line_delivery_pre_c;
            old_ttu_attr.refcyc_per_req_delivery_pre_l = new_ttu_attr.refcyc_per_req_delivery_pre_l;
            old_ttu_attr.refcyc_per_req_delivery_pre_c = new_ttu_attr.refcyc_per_req_delivery_pre_c;
            old_ttu_attr.refcyc_per_req_delivery_pre_cur0 = new_ttu_attr.refcyc_per_req_delivery_pre_cur0;
            old_ttu_attr.refcyc_per_req_delivery_pre_cur1 = new_ttu_attr.refcyc_per_req_delivery_pre_cur1;
            old_ttu_attr.min_ttu_vblank = new_ttu_attr.min_ttu_vblank;
            old_ttu_attr.qos_level_flip = new_ttu_attr.qos_level_flip;
            new_pipe.update_flags |= PIPE_UPDATE_HUBP_INTERDEPENDENT;
        }
        // Detect any other updates to ttu/rq/dlg
        if !old_dlg_attr.bits_eq(&new_pipe.dlg_regs)
            || !old_ttu_attr.bits_eq(&new_pipe.ttu_regs)
            || !old_pipe.rq_regs.bits_eq(&new_pipe.rq_regs)
        {
            new_pipe.update_flags |= PIPE_UPDATE_HUBP_RQ_DLG_TTU;
        }
    }

    if old_pipe.unbounded_req != new_pipe.unbounded_req {
        new_pipe.update_flags |= PIPE_UPDATE_UNBOUNDED_REQ;
    }
}

/// `dcn20_update_odm` (dcn20_hwseq.c).
pub fn dcn20_update_odm(adev: &amdgpu_device, dc: &mut dc, context: &mut crate::dc_state::dc_state, p: usize) {
    let pipe_ctx = context.res_ctx.pipe_ctx[p];
    let mut opp_cnt = 1;

    let mut odm_pipe = pipe_ctx.next_odm_pipe;
    while let Some(o) = odm_pipe {
        opp_cnt += 1;
        odm_pipe = context.res_ctx.pipe_ctx[o].next_odm_pipe;
    }

    if opp_cnt > 1 {
        crate::linux::Line::new().s("dcn20_update_odm: optc2_set_odm_combine not ported").say();
    } else {
        let timing = dc.streams[pipe_ctx.stream.unwrap()].as_deref().unwrap().timing;
        let tg = dc.res_pool.as_deref_mut().unwrap().timing_generators[pipe_ctx.stream_res.tg.unwrap()]
            .as_deref_mut().unwrap();
        (tg.funcs().set_odm_bypass.unwrap())(adev, tg, &timing);
    }
}

/// `dcn20_program_front_end_for_ctx` (dcn20_hwseq.c): the pipes' update
/// flags, the OTG blank before front ends go, the MPCC disconnects, ODM,
/// then every updated pipe top to bottom. Triple buffering is off, DCN 2.1
/// has no SubVP phantoms, no `force_pstate_change_control`, no DET size
/// programming and no writeback in the tree.
pub fn dcn20_program_front_end_for_ctx(adev: &amdgpu_device, dc: &mut dc, context: &mut crate::dc_state::dc_state) {
    use crate::dc_resource::{pipe_type, resource_is_pipe_type};
    use crate::dc_state::*;
    let pipe_count = dc.res_pool.as_deref().unwrap().pipe_count as usize;
    let funcs = dc.hwseq.as_deref().unwrap().funcs;

    // resource_is_pipe_topology_changed / resource_log_pipe_topology_update
    // log only.

    let mut prev_hubp_count = 0;
    let mut hubp_count = 0;
    for i in 0..pipe_count {
        if dc.current_state.as_deref().unwrap().res_ctx.pipe_ctx[i].plane_state.is_some() {
            prev_hubp_count += 1;
        }
        if context.res_ctx.pipe_ctx[i].plane_state.is_some() {
            hubp_count += 1;
        }
    }

    if prev_hubp_count == 0 && hubp_count > 0 {
        // hubbub21_funcs has no force_pstate_change_control.
        crate::linux::udelay(500);
    }

    // Set pipe update flags and lock pipes
    for i in 0..pipe_count {
        let old_res = dc.current_state.as_deref().unwrap().res_ctx;
        let new_res = context.res_ctx;
        dcn20_detect_pipe_changes(&old_res.pipe_ctx[i], &mut context.res_ctx.pipe_ctx[i], &old_res, &new_res);
    }

    // OTG blank before disabling all front ends
    for i in 0..pipe_count {
        let pipe = &context.res_ctx.pipe_ctx[i];
        if pipe.update_flags & PIPE_UPDATE_DISABLE != 0 && pipe.top_pipe.is_none() && pipe.prev_odm_pipe.is_none()
            && pipe.stream.is_some()
        {
            (funcs.blank_pixel_data.unwrap())(adev, dc, context, i, true);
        }
    }

    // Disconnect mpcc
    for i in 0..pipe_count {
        let flags = context.res_ctx.pipe_ctx[i].update_flags;
        if flags & (PIPE_UPDATE_DISABLE | PIPE_UPDATE_OPP_CHANGED) != 0 {
            // hubbub21_funcs has neither program_det_size nor
            // program_det_segments.
            (funcs.plane_atomic_disconnect.unwrap())(adev, dc, i);
        }
    }

    // update ODM for blanked OTG master pipes
    for i in 0..pipe_count {
        let pipe = &context.res_ctx.pipe_ctx[i];
        if resource_is_pipe_type(pipe, pipe_type::OTG_MASTER) && !resource_is_pipe_type(pipe, pipe_type::DPP_PIPE)
            && pipe.update_flags & PIPE_UPDATE_ODM != 0
            && let Some(update_odm) = funcs.update_odm
        {
            update_odm(adev, dc, context, i);
        }
    }

    // Program all updated pipes, order matters for mpcc setup. Start with
    // top pipe and program all pipes that follow in order
    for i in 0..pipe_count {
        let pipe = &context.res_ctx.pipe_ctx[i];

        if pipe.plane_state.is_some() && pipe.top_pipe.is_none() {
            let mut p = Some(i);
            while let Some(q) = p {
                if context.res_ctx.pipe_ctx[q].stream.is_some() {
                    dcn20_program_pipe(adev, dc, context, q);
                }
                p = context.res_ctx.pipe_ctx[q].bottom_pipe;
            }
        }

        // Program secondary blending tree and writeback pipes: DCN 2.1 has no
        // program_all_writeback_pipes_in_tree, nor the MPO hubpret wait.
    }
}

/// `dcn20_post_unlock_program_front_end` (dcn20_hwseq.c): disable what the
/// new state drops, wait for enabled planes' first flips, then the
/// DEDCN21-147 watermark workaround. DCN 2.1 has no ODM double buffer
/// pending flag, no SubVP phantoms, no forced p-state and no MALL.
pub fn dcn20_post_unlock_program_front_end(adev: &amdgpu_device, dc: &mut dc, context: &mut crate::dc_state::dc_state) {
    use crate::dc_resource::{pipe_type, resource_is_pipe_type};
    use crate::dc_state::*;
    // Timeout for pipe enable
    let timeout_us = 100000;
    let polling_interval_us = 1;
    let pipe_count = dc.res_pool.as_deref().unwrap().pipe_count as usize;

    for i in 0..pipe_count {
        if resource_is_pipe_type(&dc.current_state.as_deref().unwrap().res_ctx.pipe_ctx[i], pipe_type::OPP_HEAD)
            && !resource_is_pipe_type(&context.res_ctx.pipe_ctx[i], pipe_type::OPP_HEAD)
        {
            crate::linux::Line::new().s("dcn20_post_unlock_program_front_end: dcn20_post_unlock_reset_opp not ported").say();
        }
    }

    for i in 0..pipe_count {
        if context.res_ctx.pipe_ctx[i].update_flags & PIPE_UPDATE_DISABLE != 0 {
            (dc.hwss.disable_plane.unwrap())(adev, dc, i);
        }
    }

    // If we are enabling a pipe, we need to wait for pending clear as this is
    // a critical part of the enable operation otherwise, DM may request an
    // immediate flip which will cause HW to perform an "immediate enable"
    // (as opposed to "vsync enable") which is unsupported on DCN.
    for i in 0..pipe_count {
        let pipe = &context.res_ctx.pipe_ctx[i];
        if pipe.plane_state.is_some() && pipe.top_pipe.is_none() && pipe.update_flags & PIPE_UPDATE_ENABLE != 0 {
            let hubp = dc.res_pool.as_deref().unwrap().hubps[pipe.plane_res.hubp.unwrap()].as_deref().unwrap();
            let mut j = 0;
            while j < timeout_us / polling_interval_us && (hubp.funcs().hubp_is_flip_pending.unwrap())(adev, hubp) {
                crate::linux::udelay(polling_interval_us as u64);
                j += 1;
            }
        }
    }

    // get_optc_double_buffer_pending is not a member of dcn20_tg_funcs, nor
    // force_pstate_change_control of hubbub21_funcs.

    let hwseq = dc.hwseq.as_deref().unwrap();

    // WA to apply WM setting
    if hwseq.wa.DEGVIDCN21 {
        let hubbub = dc.res_pool.as_deref().unwrap().hubbub.as_deref().unwrap();
        (hubbub.funcs().apply_DEDCN21_147_wa.unwrap())(adev, hubbub);
    }

    // WA for stutter underflow during MPO transitions when adding 2nd plane
    if hwseq.wa.disallow_self_refresh_during_multi_plane_transition
        && dc.current_state.as_deref().unwrap().stream_status[0].plane_count == 1
        && context.stream_status[0].plane_count > 1
    {
        crate::linux::Line::new().s("dcn20_post_unlock_program_front_end: multi-plane self refresh workaround not ported").say();
    }
}

/// `dcn20_optimize_bandwidth` (dcn20_hwseq.c): lower the watermarks and
/// clocks to what the applied state needs. DCN 2.1 has no SubVP, no DC
/// mode soft max, no compbuf, no FW-based MCLK switching and no extended
/// blank.
pub fn dcn20_optimize_bandwidth(adev: &amdgpu_device, dc: &mut dc, context: &mut crate::dc_state::dc_state) {
    // program dchubbub watermarks
    let refclk_mhz = dc.res_pool.as_deref().unwrap().ref_clocks.dchub_ref_clock_inKhz / 1000;
    let mut hubbub = dc.res_pool.as_deref_mut().unwrap().hubbub.take().unwrap();
    (hubbub.funcs().program_watermarks.unwrap())(adev, dc, &mut hubbub, &context.bw_ctx.dcn.watermarks, refclk_mhz,
        true);
    dc.res_pool.as_deref_mut().unwrap().hubbub = Some(hubbub);

    if context.bw_ctx.dcn.clk.fw_based_mclk_switching {
        crate::linux::Line::new().s("dcn20_optimize_bandwidth: dc_dmub_srv_p_state_delegate not ported").say();
    } else {
        dc.clk_mgr.as_deref_mut().unwrap().base.clks.fw_based_mclk_switching = false;
    }

    crate::clk_mgr::clk_mgr_update_clocks(adev, dc, context, true);
    if context.bw_ctx.dcn.clk.zstate_support == DCN_ZSTATE_SUPPORT_ALLOW && !dc.debug.disable_extblankadj {
        // hubp program_extended_blank is not a member of dcn21_hubp_funcs.
    }
}

/// `dcn20_program_triple_buffer` (dcn20_hwseq.c).
pub fn dcn20_program_triple_buffer(adev: &amdgpu_device, dc: &dc, pipe_ctx: &crate::dc_state::pipe_ctx,
                                   enable_triple_buffer: bool) {
    if let Some(h) = pipe_ctx.plane_res.hubp {
        let hubp = dc.res_pool.as_deref().unwrap().hubps[h].as_deref().unwrap();
        (hubp.funcs().hubp_enable_tripleBuffer.unwrap())(adev, hubp, enable_triple_buffer);
    }
}

/// `dcn20_pipe_control_lock` (dcn20_hwseq.c): lock or unlock everything on
/// a top pipe's TG with its master update lock. `res_ctx` is the state the
/// pipe is in. Neither PSR nor Replay is set up on DCN 2.1, so the DMUB HW
/// lock is not used; triple buffering stays off.
pub fn dcn20_pipe_control_lock(adev: &amdgpu_device, dc: &mut dc, res_ctx: &crate::dc_state::resource_context,
                               p: usize, lock: bool) {
    let pipe = &res_ctx.pipe_ctx[p];
    let plane = |pc: &crate::dc_state::pipe_ctx| pc.plane_state.map(|i| *dc.planes[i].as_deref().unwrap());
    let mut flip_immediate = false;

    // use TG master update lock to lock everything on the TG therefore only
    // top pipe need to lock
    if pipe.top_pipe.is_some() {
        return;
    }

    if let Some(ps) = plane(pipe) {
        flip_immediate = ps.flip_immediate;
    }

    if pipe.stream_res.gsl_group > 0 {
        let mut temp_pipe = pipe.bottom_pipe;
        while !flip_immediate && let Some(t) = temp_pipe {
            if let Some(ps) = plane(&res_ctx.pipe_ctx[t]) {
                flip_immediate = ps.flip_immediate;
            }
            temp_pipe = res_ctx.pipe_ctx[t].bottom_pipe;
        }
    }

    if flip_immediate && lock {
        const TIMEOUT_FOR_FLIP_PENDING_US: u32 = 100000;
        let polling_interval_us = 1;

        let mut temp_pipe = Some(p);
        while let Some(t) = temp_pipe {
            let tp = &res_ctx.pipe_ctx[t];
            if plane(tp).is_some_and(|ps| ps.flip_immediate) {
                let hubp = dc.res_pool.as_deref().unwrap().hubps[tp.plane_res.hubp.unwrap()].as_deref().unwrap();
                for _ in 0..TIMEOUT_FOR_FLIP_PENDING_US / polling_interval_us {
                    if !(hubp.funcs().hubp_is_flip_pending.unwrap())(adev, hubp) {
                        break;
                    }
                    crate::linux::udelay(polling_interval_us as u64);
                }
            }
            temp_pipe = tp.bottom_pipe;
        }
    }

    // In flip immediate and pipe splitting case, we need to use GSL for
    // synchronization. Only do setup on locking and on flip type change.
    if lock && (pipe.bottom_pipe.is_some() || !flip_immediate)
        && ((flip_immediate && pipe.stream_res.gsl_group == 0) || (!flip_immediate && pipe.stream_res.gsl_group > 0))
    {
        crate::linux::Line::new().s("dcn20_pipe_control_lock: dcn20_setup_gsl_group_as_lock not ported").say();
    }

    if let Some(ps) = plane(pipe) {
        flip_immediate = ps.flip_immediate;
    }

    let mut temp_pipe = pipe.bottom_pipe;
    while flip_immediate && let Some(t) = temp_pipe {
        if let Some(ps) = plane(&res_ctx.pipe_ctx[t]) {
            flip_immediate = ps.flip_immediate;
        }
        temp_pipe = res_ctx.pipe_ctx[t].bottom_pipe;
    }

    if !lock && pipe.stream_res.gsl_group > 0 && pipe.plane_state.is_some() && !flip_immediate {
        crate::linux::Line::new().s("dcn20_pipe_control_lock: dcn20_setup_gsl_group_as_lock not ported").say();
    }

    let li = pipe.stream.map(|s| dc.streams[s].as_deref().unwrap().link);
    let tg = dc.res_pool.as_deref().unwrap().timing_generators[pipe.stream_res.tg.unwrap()].as_deref().unwrap();
    if li.is_some_and(|li| crate::dc::should_use_dmub_lock(dc, li)) {
        crate::linux::Line::new().s("dcn20_pipe_control_lock: dmub_hw_lock_mgr_cmd not ported").say();
    } else if plane(pipe).is_some_and(|ps| ps.triplebuffer_flips) {
        crate::linux::Line::new().s("dcn20_pipe_control_lock: optc2_triplebuffer_lock not ported").say();
    } else if lock {
        (tg.funcs().lock.unwrap())(adev, tg);
    } else {
        // hwseq has no perform_3dlut_wa_unlock on DCN 2.1
        (tg.funcs().unlock.unwrap())(adev, tg);
    }
}

/// `dcn20_calculate_vready_offset_for_group` (dcn20_hwseq.c): the largest
/// vready_offset of all connected pipes.
fn dcn20_calculate_vready_offset_for_group(res_ctx: &crate::dc_state::resource_context, p: usize) -> i32 {
    let pipe = &res_ctx.pipe_ctx[p];
    let mut vready_offset = pipe.pipe_dlg_param.vready_offset;

    let mut walk = |first: Option<usize>, next: fn(&crate::dc_state::pipe_ctx) -> Option<usize>| {
        let mut other_pipe = first;
        while let Some(o) = other_pipe {
            let op = &res_ctx.pipe_ctx[o];
            if op.pipe_dlg_param.vready_offset > vready_offset {
                vready_offset = op.pipe_dlg_param.vready_offset;
            }
            other_pipe = next(op);
        }
    };
    walk(pipe.bottom_pipe, |p| p.bottom_pipe);
    walk(pipe.top_pipe, |p| p.top_pipe);
    walk(pipe.next_odm_pipe, |p| p.next_odm_pipe);
    walk(pipe.prev_odm_pipe, |p| p.prev_odm_pipe);

    vready_offset as i32
}

/// `dcn20_program_tg` (dcn20_hwseq.c): global sync, then wait for active
/// and the VTG parameters. DCN 2.1 has no SubVP phantoms.
fn dcn20_program_tg(adev: &amdgpu_device, dc: &mut dc, context: &crate::dc_state::dc_state, p: usize) {
    let pipe_ctx = context.res_ctx.pipe_ctx[p];
    let vready = dcn20_calculate_vready_offset_for_group(&context.res_ctx, p);
    let d = &pipe_ctx.pipe_dlg_param;
    let timing = dc.streams[pipe_ctx.stream.unwrap()].as_deref().unwrap().timing;
    {
        let tg = dc.res_pool.as_deref_mut().unwrap().timing_generators[pipe_ctx.stream_res.tg.unwrap()]
            .as_deref_mut().unwrap();
        (tg.funcs().program_global_sync.unwrap())(adev, tg, vready, d.vstartup_start as i32, d.vupdate_offset as i32,
            d.vupdate_width as i32, d.pstate_keepout as i32);

        (tg.funcs().wait_for_state.unwrap())(adev, tg, CRTC_STATE_VACTIVE);

        (tg.funcs().set_vtg_params.unwrap())(adev, tg, &timing, true);
    }

    if let Some(setup_vupdate_interrupt) = dc.hwseq.as_deref().unwrap().funcs.setup_vupdate_interrupt {
        setup_vupdate_interrupt(adev, dc, context, p);
    }
}

/// `dcn20_power_on_plane_resources` (dcn20_hwseq.c): ungate the pipe's DPP
/// and HUBP under an IP request.
fn dcn20_power_on_plane_resources(adev: &amdgpu_device, dc: &dc, pipe_ctx: &crate::dc_state::pipe_ctx) {
    let hws = dc.hwseq.as_deref().unwrap();
    let pool = dc.res_pool.as_deref().unwrap();
    let dpp_inst = pool.dpps[pipe_ctx.plane_res.dpp.unwrap()].as_deref().unwrap().base.inst;
    let hubp_inst = pool.hubps[pipe_ctx.plane_res.hubp.unwrap()].as_deref().unwrap().base.inst;

    if let Some(dpp_root_clock_control) = hws.funcs.dpp_root_clock_control {
        dpp_root_clock_control(adev, dc, dpp_inst, true);
    }

    if hws.rsm().0.DC_IP_REQUEST_CNTL != 0 {
        let org_ip_request_cntl = reg_get!(adev, hws.rsm(), DC_IP_REQUEST_CNTL, IP_REQUEST_EN);
        if org_ip_request_cntl == 0 {
            reg_set!(adev, hws.rsm(), DC_IP_REQUEST_CNTL, 0, IP_REQUEST_EN, 1);
        }

        if let Some(dpp_pg_control) = hws.funcs.dpp_pg_control {
            dpp_pg_control(adev, dc, dpp_inst, true);
        }

        if let Some(hubp_pg_control) = hws.funcs.hubp_pg_control {
            hubp_pg_control(adev, dc, hubp_inst, true);
        }

        if org_ip_request_cntl == 0 {
            reg_set!(adev, hws.rsm(), DC_IP_REQUEST_CNTL, 0, IP_REQUEST_EN, 0);
        }
    }
}

/// `dcn20_enable_plane` (dcn20_hwseq.c): power the front end, clock and
/// init the HUBP, the OPP pipe clock, the system aperture.
pub fn dcn20_enable_plane(adev: &amdgpu_device, dc: &mut dc, context: &crate::dc_state::dc_state, p: usize) {
    let pipe_ctx = context.res_ctx.pipe_ctx[p];
    dcn20_power_on_plane_resources(adev, dc, &pipe_ctx);

    let vm_pa_config = dc.vm_pa_config;
    let plane = pipe_ctx.plane_state.map(|i| *dc.planes[i].as_deref().unwrap());
    let pool = dc.res_pool.as_deref_mut().unwrap();
    let hubp = pool.hubps[pipe_ctx.plane_res.hubp.unwrap()].as_deref_mut().unwrap();

    // enable DCFCLK current DCHUB
    (hubp.funcs().hubp_clk_cntl.unwrap())(adev, hubp, true);

    // initialize HUBP on power up
    (hubp.funcs().hubp_init.unwrap())(adev, hubp);

    // make sure OPP_PIPE_CLOCK_EN = 1
    let opp = pool.opps[pipe_ctx.stream_res.opp.unwrap()].as_deref().unwrap();
    (opp.funcs().opp_pipe_clock_control.unwrap())(adev, opp, true);

    let hubp = pool.hubps[pipe_ctx.plane_res.hubp.unwrap()].as_deref().unwrap();
    if vm_pa_config.valid {
        let apt = crate::dcn21_hubp::vm_system_aperture_param {
            sys_default: 0,
            sys_low: vm_pa_config.system_aperture.start_addr,
            sys_high: vm_pa_config.system_aperture.end_addr,
        };

        // Program system aperture settings
        (hubp.funcs().hubp_set_vm_system_aperture_settings.unwrap())(adev, hubp, &apt);
    }

    if pipe_ctx.top_pipe.is_none() && plane.is_some_and(|ps| ps.flip_int_enabled)
        && let Some(set_flip_int) = hubp.funcs().hubp_set_flip_int
    {
        set_flip_int(adev, hubp);
    }
}

/// `dcn20_update_mpcc` (dcn20_hwseq.c): the plane's blending, and its MPCC
/// (inst = HUBP inst) into the OPP's tree on a full update.
pub fn dcn20_update_mpcc(adev: &amdgpu_device, dc: &mut dc, context: &crate::dc_state::dc_state, p: usize) {
    use crate::dc_stream::SURFACE_UPDATE_FULL_UPDATE;
    use crate::dc_state::PIPE_UPDATE_MPCC;
    let pipe_ctx = context.res_ctx.pipe_ctx[p];
    let plane_state = *dc.planes[pipe_ctx.plane_state.unwrap()].as_deref().unwrap();
    let mut blnd_cfg = crate::dcn20_mpc::mpcc_blnd_cfg::default();
    let per_pixel_alpha = plane_state.per_pixel_alpha;

    blnd_cfg.overlap_only = false;
    blnd_cfg.global_gain = 0xff;

    if per_pixel_alpha {
        blnd_cfg.pre_multiplied_alpha = plane_state.pre_multiplied_alpha;
        if plane_state.global_alpha {
            blnd_cfg.alpha_mode = MPCC_ALPHA_BLEND_MODE_PER_PIXEL_ALPHA_COMBINED_GLOBAL_GAIN;
            blnd_cfg.global_gain = plane_state.global_alpha_value;
        } else {
            blnd_cfg.alpha_mode = MPCC_ALPHA_BLEND_MODE_PER_PIXEL_ALPHA;
        }
    } else {
        blnd_cfg.pre_multiplied_alpha = false;
        blnd_cfg.alpha_mode = MPCC_ALPHA_BLEND_MODE_GLOBAL_ALPHA;
    }

    blnd_cfg.global_alpha = if plane_state.global_alpha { plane_state.global_alpha_value } else { 0xff };

    blnd_cfg.background_color_bpc = 4;
    blnd_cfg.bottom_gain_mode = 0;
    blnd_cfg.top_gain = 0x1f000;
    blnd_cfg.bottom_inside_gain = 0x1f000;
    blnd_cfg.bottom_outside_gain = 0x1f000;

    if plane_state.format == SURFACE_PIXEL_FORMAT_GRPH_RGBE_ALPHA {
        blnd_cfg.pre_multiplied_alpha = false;
    }

    // MPCC inst = HUBP inst: init_hw can leave MPCC0 unable to idle after
    // a BIOS resume, and picking MPCC1 there hangs p-state.
    let sanity_checks = dc.debug.sanity_checks;
    let update_visual_confirm_color = dc.hwss.update_visual_confirm_color.unwrap();
    let pool = dc.res_pool.as_deref_mut().unwrap();
    let hubp_idx = pipe_ctx.plane_res.hubp.unwrap();
    let mpcc_id = pool.hubps[hubp_idx].as_deref().unwrap().base.inst as i32;
    let opp_idx = pipe_ctx.stream_res.opp.unwrap();

    // If there is no full update, don't need to touch MPC tree
    if plane_state.update_flags & SURFACE_UPDATE_FULL_UPDATE == 0 && pipe_ctx.update_flags & PIPE_UPDATE_MPCC == 0 {
        let mpc = pool.mpc.as_deref_mut().unwrap();
        (mpc.funcs().update_blending.unwrap())(adev, mpc, &blnd_cfg, mpcc_id);
        update_visual_confirm_color(adev, dc, &pipe_ctx, mpcc_id);
        return;
    }

    let mpc = pool.mpc.as_deref_mut().unwrap();
    let opp = pool.opps[opp_idx].as_deref_mut().unwrap();
    let mpc_tree_params = &mut opp.base.mpc_tree_params;

    // check if this MPCC is already being used
    let new_mpcc = (mpc.funcs().get_mpcc_for_dpp.unwrap())(mpc, mpc_tree_params, mpcc_id);
    // remove MPCC if being used
    if let Some(m) = new_mpcc {
        (mpc.funcs().remove_mpcc.unwrap())(adev, mpc, mpc_tree_params, m);
    } else if sanity_checks {
        (mpc.funcs().assert_mpcc_idle_before_connect.unwrap())(adev, mpc, mpcc_id);
    }

    // Call MPC to insert new plane
    let hubp_inst = mpcc_id;
    (mpc.funcs().insert_plane.unwrap())(adev, mpc, mpc_tree_params, &blnd_cfg, None, None, hubp_inst, mpcc_id);
    let opp_inst = opp.base.inst as i32;
    update_visual_confirm_color(adev, dc, &pipe_ctx, mpcc_id);

    let hubp = dc.res_pool.as_deref_mut().unwrap().hubps[hubp_idx].as_deref_mut().unwrap();
    hubp.base.opp_id = opp_inst;
    hubp.base.mpcc_id = mpcc_id;
}

/// `dcn20_program_output_csc` (dcn20_hwseq.c): the MPC's output CSC, the
/// stream's own matrix or its colour space's default, set A.
pub fn dcn20_program_output_csc(adev: &amdgpu_device, dc: &mut dc, context: &crate::dc_state::dc_state, p: usize,
                                colorspace: u32, matrix: &[u16; 12], opp_id: i32) {
    let pipe_ctx = &context.res_ctx.pipe_ctx[p];
    let enable_adjustment = dc.streams[pipe_ctx.stream.unwrap()].as_deref().unwrap().csc_color_matrix.enable_adjustment;
    let pool = dc.res_pool.as_deref().unwrap();
    let mpc = pool.mpc.as_deref().unwrap();
    let ocsc_mode = MPC_OUTPUT_CSC_COEF_A;
    let mpcc_id = pool.hubps[pipe_ctx.plane_res.hubp.unwrap()].as_deref().unwrap().base.inst as i32;

    if let Some(power_on_mpc_mem_pwr) = mpc.funcs().power_on_mpc_mem_pwr {
        power_on_mpc_mem_pwr(adev, mpc, mpcc_id, true);
    }

    if enable_adjustment {
        if let Some(set_output_csc) = mpc.funcs().set_output_csc {
            set_output_csc(adev, mpc, opp_id, Some(matrix), ocsc_mode);
        }
    } else if let Some(set_ocsc_default) = mpc.funcs().set_ocsc_default {
        set_ocsc_default(adev, mpc, opp_id, colorspace, ocsc_mode);
    }
}

/// `patch_address_for_sbs_tb_stereo` (dcn20_hwseq.c): the secondary split
/// pipe of side-by-side or top-and-bottom stereo scans out the right eye.
fn patch_address_for_sbs_tb_stereo(dc: &mut dc, res_ctx: &crate::dc_state::resource_context, p: usize,
                                   addr: &mut u64) -> bool {
    let pipe_ctx = &res_ctx.pipe_ctx[p];
    let timing_3d_format = dc.streams[pipe_ctx.stream.unwrap()].as_deref().unwrap().timing.timing_3d_format;
    let view_format = dc.streams[pipe_ctx.stream.unwrap()].as_deref().unwrap().view_format;
    let sec_split = pipe_ctx.top_pipe.is_some_and(|t| res_ctx.pipe_ctx[t].plane_state == pipe_ctx.plane_state);
    let plane_state = dc.planes[pipe_ctx.plane_state.unwrap()].as_deref_mut().unwrap();
    if sec_split && plane_state.address.r#type == PLN_ADDR_TYPE_GRPH_STEREO
        && (timing_3d_format == TIMING_3D_FORMAT_SIDE_BY_SIDE || timing_3d_format == TIMING_3D_FORMAT_TOP_AND_BOTTOM)
    {
        *addr = plane_state.address.left_addr();
        plane_state.address.u[0] = plane_state.address.right_addr();
        return true;
    }

    if view_format != VIEW_3D_FORMAT_NONE && plane_state.address.r#type != PLN_ADDR_TYPE_GRPH_STEREO {
        plane_state.address.r#type = PLN_ADDR_TYPE_GRPH_STEREO;
        plane_state.address.u[3] = plane_state.address.left_addr();
        plane_state.address.u[4] = plane_state.address.left_meta_addr();
    }
    false
}

/// `vm_helper_mark_vmid_used` (dc_vm_helper.c): Linux updates a copy of the
/// HUBP's VMID usage, so the call leaves the helper as it was.
fn vm_helper_mark_vmid_used(_pos: u32, _hubp_idx: u8) {}

/// `dcn20_update_plane_addr` (dcn20_hwseq.c).
pub fn dcn20_update_plane_addr(adev: &amdgpu_device, dc: &mut dc, context: &crate::dc_state::dc_state, p: usize) {
    let pipe_ctx = context.res_ctx.pipe_ctx[p];
    let mut addr = 0u64;

    let Some(plane) = pipe_ctx.plane_state else { return };

    let addr_patched = patch_address_for_sbs_tb_stereo(dc, &context.res_ctx, p, &mut addr);

    let dc_ref = &mut *dc;
    let plane_state = dc_ref.planes[plane].as_deref_mut().unwrap();
    let hubp = dc_ref.res_pool.as_deref_mut().unwrap().hubps[pipe_ctx.plane_res.hubp.unwrap()].as_deref_mut().unwrap();

    // Call Helper to track VMID use
    vm_helper_mark_vmid_used(plane_state.address.vmid, hubp.base.inst as u8);

    (hubp.funcs().hubp_program_surface_flip_and_addr.unwrap())(adev, hubp, &plane_state.address,
        plane_state.flip_immediate);

    plane_state.status.requested_address = plane_state.address;

    if plane_state.flip_immediate {
        plane_state.status.current_address = plane_state.address;
    }

    if addr_patched {
        plane_state.address.u[0] = addr;
    }
}

/// `dcn20_set_shaper_3dlut` (dcn20_hwseq.c). A shaper curve and a 3D LUT
/// join with colour management; without them both are bypassed.
pub fn dcn20_set_shaper_3dlut(adev: &amdgpu_device, dc: &dc, pipe_ctx: &crate::dc_state::pipe_ctx, plane: usize) -> bool {
    let plane_state = dc.planes[plane].as_deref().unwrap();
    let dpp_base = dc.res_pool.as_deref().unwrap().dpps[pipe_ctx.plane_res.dpp.unwrap()].as_deref().unwrap();

    // Linux overwrites the shaper's result with the 3D LUT's.
    if plane_state.in_shaper_func.r#type == TF_TYPE_HWPWL
        || plane_state.in_shaper_func.r#type == TF_TYPE_DISTRIBUTED_POINTS
    {
        crate::linux::Line::new().s("dcn20_set_shaper_3dlut: shaper curve not ported").say();
    } else {
        (dpp_base.funcs().dpp_program_shaper_lut.unwrap())(adev, dpp_base, None);
    }
    if plane_state.lut3d_state & 1 == 1 {
        crate::linux::Line::new().s("dcn20_set_shaper_3dlut: 3D LUT not ported").say();
        false
    } else {
        (dpp_base.funcs().dpp_program_3dlut.unwrap())(adev, dpp_base, None)
    }
}

/// `dcn20_set_blend_lut` (dcn20_hwseq.c). A blend curve joins with colour
/// management; without one the blend gamma is bypassed.
pub fn dcn20_set_blend_lut(adev: &amdgpu_device, dc: &dc, pipe_ctx: &crate::dc_state::pipe_ctx, plane: usize) -> bool {
    let plane_state = dc.planes[plane].as_deref().unwrap();
    let dpp_base = dc.res_pool.as_deref().unwrap().dpps[pipe_ctx.plane_res.dpp.unwrap()].as_deref().unwrap();

    if plane_state.blend_tf.r#type == TF_TYPE_HWPWL || plane_state.blend_tf.r#type == TF_TYPE_DISTRIBUTED_POINTS {
        crate::linux::Line::new().s("dcn20_set_blend_lut: blend curve not ported").say();
        return false;
    }
    (dpp_base.funcs().dpp_program_blnd_lut.unwrap())(adev, dpp_base, None)
}

/// `dcn20_set_input_transfer_func` (dcn20_hwseq.c): shaper, 3D LUT and
/// blend LUT, then the degamma — the ROM for the predefined curves; a RAM
/// curve (HWPWL, distributed points, PQ) joins with colour management.
pub fn dcn20_set_input_transfer_func(adev: &amdgpu_device, dc: &mut dc, context: &crate::dc_state::dc_state, p: usize,
                                     plane: usize) -> bool {
    let pipe_ctx = context.res_ctx.pipe_ctx[p];
    let mut result = true;

    let Some(dpp) = pipe_ctx.plane_res.dpp else { return false };

    let funcs = dc.hwseq.as_deref().unwrap().funcs;
    (funcs.set_shaper_3dlut.unwrap())(adev, dc, &pipe_ctx, plane);
    (funcs.set_blend_lut.unwrap())(adev, dc, &pipe_ctx, plane);

    let tf = dc.planes[plane].as_deref().unwrap().in_transfer_func;
    let debug = dc.debug;
    let dpp_base = dc.res_pool.as_deref().unwrap().dpps[dpp].as_deref().unwrap();
    let set_degamma = dpp_base.funcs().dpp_set_degamma.unwrap();

    if tf.r#type == TF_TYPE_HWPWL || tf.r#type == TF_TYPE_DISTRIBUTED_POINTS {
        crate::linux::Line::new().s("dcn20_set_input_transfer_func: degamma RAM not ported").say();
        return true;
    }
    // handle here the optimized cases when de-gamma ROM could be used.
    if tf.r#type == TF_TYPE_PREDEFINED {
        match tf.tf {
            TRANSFER_FUNCTION_SRGB => set_degamma(adev, dpp_base, &debug, IPP_DEGAMMA_MODE_HW_sRGB),
            TRANSFER_FUNCTION_BT709 => set_degamma(adev, dpp_base, &debug, IPP_DEGAMMA_MODE_HW_xvYCC),
            TRANSFER_FUNCTION_LINEAR => set_degamma(adev, dpp_base, &debug, IPP_DEGAMMA_MODE_BYPASS),
            TRANSFER_FUNCTION_PQ => {
                set_degamma(adev, dpp_base, &debug, IPP_DEGAMMA_MODE_USER_PWL);
                crate::linux::Line::new().s("dcn20_set_input_transfer_func: PQ degamma RAM not ported").say();
                result = true;
            }
            _ => result = false,
        }
    } else {
        // TF_TYPE_BYPASS, or a case not handled, which Linux bypasses too.
        set_degamma(adev, dpp_base, &debug, IPP_DEGAMMA_MODE_BYPASS);
    }

    result
}

/// `dcn20_set_output_transfer_func` (dcn20_hwseq.c): OGAM on the top pipe;
/// there is no ROM, and a RAM curve joins with colour management, so the
/// gamma is bypassed.
pub fn dcn20_set_output_transfer_func(adev: &amdgpu_device, dc: &mut dc, context: &crate::dc_state::dc_state, p: usize,
                                      stream: usize) -> bool {
    let pipe_ctx = &context.res_ctx.pipe_ctx[p];
    let out_tf = dc.streams[stream].as_deref().unwrap().out_transfer_func;
    let debug = dc.debug;
    let pool = dc.res_pool.as_deref().unwrap();
    let mpcc_id = pool.hubps[pipe_ctx.plane_res.hubp.unwrap()].as_deref().unwrap().base.inst as i32;
    let mpc = pool.mpc.as_deref().unwrap();
    let params = None;

    if let Some(power_on_mpc_mem_pwr) = mpc.funcs().power_on_mpc_mem_pwr {
        power_on_mpc_mem_pwr(adev, mpc, mpcc_id, true);
    }
    if pipe_ctx.top_pipe.is_none() && mpc.funcs().set_output_gamma.is_some()
        && (out_tf.r#type == TF_TYPE_HWPWL || out_tf.r#type == TF_TYPE_DISTRIBUTED_POINTS)
    {
        crate::linux::Line::new().s("dcn20_set_output_transfer_func: output gamma curve not ported").say();
        return true;
    }
    // if above if is not executed then 'params' equal to 0 and set in bypass
    if let Some(set_output_gamma) = mpc.funcs().set_output_gamma {
        set_output_gamma(adev, mpc, &debug, mpcc_id, params);
    }

    true
}

/// `dcn20_update_dchubp_dpp` (dcn20_hwseq.c): the HUBP and DPP of a pipe
/// with a plane, by what its update flags say changed. DCN 2.1 has no
/// `hubp_setup2`, unbounded requesting, mcache or SubVP; a cursor joins
/// with the cursor stage.
pub fn dcn20_update_dchubp_dpp(adev: &amdgpu_device, dc: &mut dc, context: &mut crate::dc_state::dc_state, p: usize,
                               context_is_current: bool) {
    use crate::dc_state::*;
    use crate::dc_stream::*;
    let pipe_ctx = context.res_ctx.pipe_ctx[p];
    let plane = pipe_ctx.plane_state.unwrap();
    let plane_state = *dc.planes[plane].as_deref().unwrap();
    let stream = dc.streams[pipe_ctx.stream.unwrap()].as_deref().unwrap();
    let stream_update_flags = stream.update_flags;
    let timing = stream.timing;
    let output_color_space = stream.output_color_space;
    let csc_matrix = stream.csc_color_matrix.matrix;
    let cursor_address = stream.cursor_attributes_address;
    let flags = pipe_ctx.update_flags;
    let pflags = plane_state.update_flags;
    let debug = dc.debug;
    let hubp_idx = pipe_ctx.plane_res.hubp.unwrap();
    let dpp_idx = pipe_ctx.plane_res.dpp.unwrap();
    let mut viewport_changed = false;
    let _ = timing;

    {
        let pool = dc.res_pool.as_deref_mut().unwrap();
        let dpp = pool.dpps[dpp_idx].as_deref().unwrap();
        let dpp_inst = dpp.base.inst;

        if flags & PIPE_UPDATE_DPPCLK != 0 {
            (dpp.funcs().dpp_dppclk_control.unwrap())(adev, dpp, false, true);
        }

        if flags & PIPE_UPDATE_ENABLE != 0 {
            let dccg = pool.dccg.as_deref_mut().unwrap();
            (dccg.funcs().update_dpp_dto.unwrap())(adev, dccg, dpp_inst as usize, pipe_ctx.plane_res.bw_dppclk_khz);
        }

        // The HUBP picks which VTG (1:1 with an OTG) it follows.
        let hubp = pool.hubps[hubp_idx].as_deref().unwrap();
        if flags & PIPE_UPDATE_HUBP_RQ_DLG_TTU != 0 {
            let tg_inst = pool.timing_generators[pipe_ctx.stream_res.tg.unwrap()].as_deref().unwrap().base.inst;
            (hubp.funcs().hubp_vtg_sel.unwrap())(adev, hubp, tg_inst);

            (hubp.funcs().hubp_setup.unwrap())(adev, hubp, &pipe_ctx.dlg_regs, &pipe_ctx.ttu_regs, &pipe_ctx.rq_regs,
                &pipe_ctx.pipe_dlg_param);
        }

        if flags & PIPE_UPDATE_HUBP_INTERDEPENDENT != 0 {
            (hubp.funcs().hubp_setup_interdependent.unwrap())(adev, hubp, &pipe_ctx.dlg_regs, &pipe_ctx.ttu_regs);
        }

        if flags & (PIPE_UPDATE_ENABLE | PIPE_UPDATE_PLANE_CHANGED) != 0
            || pflags & (SURFACE_UPDATE_BPP_CHANGE | SURFACE_UPDATE_INPUT_CSC_CHANGE | SURFACE_UPDATE_COLOR_SPACE_CHANGE
                | SURFACE_UPDATE_COEFF_REDUCTION_CHANGE) != 0
        {
            // program the input csc
            let dpp = pool.dpps[dpp_idx].as_deref().unwrap();
            (dpp.funcs().dpp_setup.unwrap())(adev, dpp, plane_state.format, EXPANSION_MODE_ZERO,
                plane_state.input_csc_color_matrix, plane_state.color_space, None);
            // dcn20 has neither set_cursor_matrix nor dpp_program_bias_and_scale
        }
    }

    if flags & (PIPE_UPDATE_MPCC | PIPE_UPDATE_PLANE_CHANGED) != 0
        || pflags & (SURFACE_UPDATE_GLOBAL_ALPHA_CHANGE | SURFACE_UPDATE_PER_PIXEL_ALPHA_CHANGE) != 0
    {
        // MPCC inst is equal to pipe index in practice
        (dc.hwseq.as_deref().unwrap().funcs.update_mpcc.unwrap())(adev, dc, context, p);
    }

    if flags & PIPE_UPDATE_SCALER != 0
        || pflags & (SURFACE_UPDATE_SCALING_CHANGE | SURFACE_UPDATE_POSITION_CHANGE
            | SURFACE_UPDATE_PER_PIXEL_ALPHA_CHANGE) != 0
        || stream_update_flags & STREAM_UPDATE_SCALING != 0
    {
        context.res_ctx.pipe_ctx[p].plane_res.scl_data.lb_params.alpha_en = plane_state.per_pixel_alpha;
        // scaler configuration
        let scl_data = context.res_ctx.pipe_ctx[p].plane_res.scl_data;
        let dpp = dc.res_pool.as_deref_mut().unwrap().dpps[dpp_idx].as_deref_mut().unwrap();
        (dpp.funcs().dpp_set_scaler.unwrap())(adev, dpp, &debug, &scl_data);
    }

    let scl_data = context.res_ctx.pipe_ctx[p].plane_res.scl_data;
    if flags & PIPE_UPDATE_VIEWPORT != 0
        || (context_is_current && pflags & SURFACE_UPDATE_POSITION_CHANGE != 0)
        || (context_is_current && pflags & SURFACE_UPDATE_SCALING_CHANGE != 0)
        || (context_is_current && stream_update_flags & STREAM_UPDATE_SCALING != 0)
    {
        let hubp = dc.res_pool.as_deref().unwrap().hubps[hubp_idx].as_deref().unwrap();
        (hubp.funcs().mem_program_viewport.unwrap())(adev, hubp, &scl_data.viewport, &scl_data.viewport_c);
        viewport_changed = true;
    }

    // Any updates are handled in dc interface, just need to apply existing
    // for plane enable
    if (flags & (PIPE_UPDATE_ENABLE | PIPE_UPDATE_OPP_CHANGED | PIPE_UPDATE_SCALER) != 0 || viewport_changed)
        && cursor_address != 0
    {
        crate::linux::Line::new().s("dcn20_update_dchubp_dpp: set_cursor_attribute, set_cursor_position not ported").say();
    }

    // Any updates are handled in dc interface, just need to apply existing
    // for plane enable / opp change
    if flags & (PIPE_UPDATE_ENABLE | PIPE_UPDATE_OPP_CHANGED | PIPE_UPDATE_PLANE_CHANGED) != 0
        || stream_update_flags & (STREAM_UPDATE_GAMUT_REMAP | STREAM_UPDATE_OUT_CSC) != 0
        || pflags & SURFACE_UPDATE_GAMUT_REMAP_CHANGE != 0
    {
        // dpp/cm gamut remap
        (dc.hwss.program_gamut_remap.unwrap())(adev, dc, context, p);

        // call the dcn2 method which uses mpc csc
        let opp_id = dc.res_pool.as_deref().unwrap().hubps[hubp_idx].as_deref().unwrap().base.opp_id;
        (dc.hwss.program_output_csc.unwrap())(adev, dc, context, p, output_color_space, &csc_matrix, opp_id);
    }

    if flags & (PIPE_UPDATE_ENABLE | PIPE_UPDATE_PLANE_CHANGED | PIPE_UPDATE_OPP_CHANGED) != 0
        || pflags & (SURFACE_UPDATE_PIXEL_FORMAT_CHANGE | SURFACE_UPDATE_HORIZONTAL_MIRROR_CHANGE
            | SURFACE_UPDATE_ROTATION_CHANGE | SURFACE_UPDATE_SWIZZLE_CHANGE | SURFACE_UPDATE_DCC_CHANGE
            | SURFACE_UPDATE_BPP_CHANGE | SURFACE_UPDATE_SCALING_CHANGE | SURFACE_UPDATE_PLANE_SIZE_CHANGE) != 0
    {
        let mut size = plane_state.plane_size;

        size.surface_size = scl_data.viewport;
        let hubp = dc.res_pool.as_deref_mut().unwrap().hubps[hubp_idx].as_deref_mut().unwrap();
        (hubp.funcs().hubp_program_surface_config.unwrap())(adev, hubp, plane_state.format, &plane_state.tiling_info,
            &size, plane_state.rotation, &plane_state.dcc, plane_state.horizontal_mirror, 0);
        hubp.base.power_gated = false;
    }

    if flags & (PIPE_UPDATE_ENABLE | PIPE_UPDATE_PLANE_CHANGED) != 0 || pflags & SURFACE_UPDATE_ADDR_UPDATE != 0 {
        // A SubVP main pipe would save its surface address first; DCN 2.1
        // has none.
        (dc.hwss.update_plane_addr.unwrap())(adev, dc, context, p);
    }

    if flags & PIPE_UPDATE_ENABLE != 0 {
        let hubp = dc.res_pool.as_deref_mut().unwrap().hubps[hubp_idx].as_deref_mut().unwrap();
        (hubp.funcs().set_blank.unwrap())(adev, hubp, false);
    }
}

/// `dcn20_program_pipe` (dcn20_hwseq.c): one pipe of the front end, by its
/// update flags. A test pattern change joins with test patterns.
pub fn dcn20_program_pipe(adev: &amdgpu_device, dc: &mut dc, context: &mut crate::dc_state::dc_state, p: usize) {
    use crate::dc_resource::{pipe_type, resource_is_pipe_type};
    use crate::dc_state::*;
    use crate::dc_stream::*;
    let pipe_ctx = context.res_ctx.pipe_ctx[p];
    let funcs = dc.hwseq.as_deref().unwrap().funcs;
    let s = pipe_ctx.stream.unwrap();
    let flags = pipe_ctx.update_flags;
    let plane_state = pipe_ctx.plane_state.map(|i| *dc.planes[i].as_deref().unwrap());
    let stream_update_flags = dc.streams[s].as_deref().unwrap().update_flags;
    let context_is_current = false;

    // Only need to unblank on top pipe
    if resource_is_pipe_type(&pipe_ctx, pipe_type::OTG_MASTER)
        && (flags & (PIPE_UPDATE_ENABLE | PIPE_UPDATE_ODM) != 0 || stream_update_flags & STREAM_UPDATE_ABM_LEVEL != 0)
    {
        (funcs.blank_pixel_data.unwrap())(adev, dc, context, p, !plane_state.is_some_and(|ps| ps.visible));
    }

    // Only update TG on top pipe
    if flags & PIPE_UPDATE_GLOBAL_SYNC != 0 && pipe_ctx.top_pipe.is_none() && pipe_ctx.prev_odm_pipe.is_none() {
        dcn20_program_tg(adev, dc, context, p);
    }

    if flags & PIPE_UPDATE_ODM != 0 {
        (funcs.update_odm.unwrap())(adev, dc, context, p);
    }

    if flags & PIPE_UPDATE_ENABLE != 0 {
        // DCN 2.1's hwseq has no enable_plane, nor its hubbub
        // force_wm_propagate_to_pipes.
        dcn20_enable_plane(adev, dc, context, p);
    }

    // hubbub21_funcs has neither program_det_size nor program_det_segments.

    if plane_state.is_some() && (flags != 0 || plane_state.unwrap().update_flags != 0 || stream_update_flags != 0) {
        dcn20_update_dchubp_dpp(adev, dc, context, p, context_is_current);
    }

    let pipe_ctx = context.res_ctx.pipe_ctx[p];
    let plane_state = pipe_ctx.plane_state.map(|i| *dc.planes[i].as_deref().unwrap());
    if let Some(ps) = plane_state
        && (flags & PIPE_UPDATE_ENABLE != 0 || ps.update_flags & SURFACE_UPDATE_HDR_MULT != 0)
    {
        (funcs.set_hdr_multiplier.unwrap())(adev, dc, &pipe_ctx);
    }

    if let Some(ps) = plane_state
        && (ps.update_flags & (SURFACE_UPDATE_IN_TRANSFER_FUNC_CHANGE | SURFACE_UPDATE_GAMMA_CHANGE
            | SURFACE_UPDATE_LUT_3D) != 0 || flags & PIPE_UPDATE_ENABLE != 0)
    {
        (funcs.set_input_transfer_func.unwrap())(adev, dc, context, p, pipe_ctx.plane_state.unwrap());
    }

    // dcn10_translate_regamma_to_hw_format takes 750us to finish only do
    // gamma programming for powering on, internal memcmp to avoid updating
    // on slave planes
    if flags & (PIPE_UPDATE_ENABLE | PIPE_UPDATE_PLANE_CHANGED) != 0 || stream_update_flags & STREAM_UPDATE_OUT_TF != 0 {
        (funcs.set_output_transfer_func.unwrap())(adev, dc, context, p, s);
    }

    // If the pipe has been enabled or has a different opp, we should
    // reprogram the fmt. This deals with cases where interation between mpc
    // and odm combine on different streams causes a different pipe to be
    // chosen to odm combine with.
    if flags & (PIPE_UPDATE_ENABLE | PIPE_UPDATE_OPP_CHANGED) != 0 {
        let stream = dc.streams[s].as_deref().unwrap();
        let (depth, signal, bit_depth_params, clamping) =
            (stream.timing.display_color_depth, stream.signal, stream.bit_depth_params, stream.clamping);
        let debug = dc.debug;
        let opp = dc.res_pool.as_deref().unwrap().opps[pipe_ctx.stream_res.opp.unwrap()].as_deref().unwrap();
        (opp.funcs().opp_set_dyn_expansion.unwrap())(adev, opp, COLOR_SPACE_YCBCR601, depth, signal);

        (opp.funcs().opp_program_fmt.unwrap())(adev, &debug, opp, &bit_depth_params, &clamping);
    }

    // Set ABM pipe after other pipe configurations done
    if plane_state.is_some_and(|ps| ps.visible) && pipe_ctx.stream_res.abm {
        (dc.hwss.set_pipe.unwrap())(adev, dc, &pipe_ctx);
        let abm_level = dc.streams[s].as_deref().unwrap().abm_level;
        let abm = dc.res_pool.as_deref().unwrap().abm.as_deref().unwrap();
        (abm.funcs().set_abm_level.unwrap())(adev, dc, abm_level);
    }

    if flags & PIPE_UPDATE_TEST_PATTERN_CHANGED != 0 {
        crate::linux::Line::new().s("dcn20_program_pipe: test pattern not ported").say();
    }
}
