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
        crate::linux::Line::new().s("dcn20_blank_pixel_data: dcn21_set_pipe, set_abm_level not ported").say();
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
/// phantoms. Comparing a plane's scaler data and DLG/TTU/RQ registers
/// against an old plane joins with plane updates.
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

    crate::linux::Line::new().s("dcn20_detect_pipe_changes: scaler, viewport and dlg/ttu/rq comparison not ported").say();

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
                    crate::linux::Line::new().s("dcn20_program_front_end_for_ctx: dcn20_program_pipe not ported").say();
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
