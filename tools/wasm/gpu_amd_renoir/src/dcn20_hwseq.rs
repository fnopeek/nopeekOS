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

    (dc.hwss.wait_for_mpcc_disconnect.unwrap())(adev, dc, pipe_idx);

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
