//! display/dc/hwss/dcn10/dcn10_hwseq.c (Linux 6.18.55): the DCN 1.0 hw
//! sequencer steps DCN 2.1 binds, as far as the hardware init reaches them —
//! `dcn10_init_hw` and the pipe reset it runs. A pipe is named by its index in
//! the current state's `pipe_ctx`, a block by its index in the pool. No
//! stream exists before the first commit, so every seamless-boot branch is
//! taken with `pipe_ctx->stream == NULL`.
#![allow(dead_code)]

use crate::amdgpu_device::amdgpu_device;
use crate::dc::dc;
use crate::dc_helper::{reg_set, reg_update, reg_write};
use crate::dc_types::MAX_PIPES;
use crate::dcn20_dsc::dcn_dsc_state;
use crate::dcn20_hwseq::OPP_ID_INVALID_U32;
use crate::dcn20_mpc::MAX_MPCC;
use crate::dcn21_hubp::OPP_ID_INVALID;
use crate::regs::*;

/// `get_hubp_by_inst` (dcn10_hwseq.c): the pool index of the HUBP with
/// instance `mpcc_inst`.
fn get_hubp_by_inst(pool: &crate::dc_resource::resource_pool, mpcc_inst: usize) -> Option<usize> {
    (0..pool.pipe_count as usize)
        .find(|&i| pool.hubps[i].as_deref().is_some_and(|h| h.base.inst as usize == mpcc_inst))
}

/// `dcn10_verify_allow_pstate_change_high` (dcn10_hwseq.c). hubbub21 has no
/// `verify_allow_pstate_change_high`, so on DCN 2.1 this returns at once.
pub fn dcn10_verify_allow_pstate_change_high(adev: &amdgpu_device, dc: &dc) {
    let hubbub = dc.res_pool.as_deref().and_then(|p| p.hubbub.as_deref()).expect("hubbub");

    let Some(verify) = hubbub.funcs().verify_allow_pstate_change_high else { return };

    if !verify(adev, hubbub) {
        crate::linux::Line::new().s("dcn10_verify_allow_pstate_change_high: dcn10_hw_wa_force_recovery not ported").say();
    }
}

/// `dcn10_wait_for_mpcc_disconnect` (dcn10_hwseq.c): for every MPCC the
/// pipe's OPP still has a disconnect pending on, wait for it to go idle
/// (if the OTG runs) and blank its HUBP.
pub fn dcn10_wait_for_mpcc_disconnect(adev: &amdgpu_device, dc: &mut dc, pipe: &crate::dc_state::pipe_ctx) {
    let verify = dc.hwseq.as_deref().expect("hwseq").funcs.verify_allow_pstate_change_high;

    if dc.debug.sanity_checks {
        (verify.unwrap())(adev, dc);
    }

    let Some(opp_idx) = pipe.stream_res.opp else { return };

    for mpcc_inst in 0..MAX_PIPES {
        let pool = dc.res_pool.as_deref_mut().expect("pool");
        if pool.opps[opp_idx].as_deref().expect("opp").base.mpcc_disconnect_pending[mpcc_inst] {
            let hubp_idx = get_hubp_by_inst(pool, mpcc_inst);

            if let Some(tg_idx) = pipe.stream_res.tg {
                let tg = pool.timing_generators[tg_idx].as_deref().expect("tg");
                if (tg.funcs().is_tg_enabled.unwrap())(adev, tg) {
                    let mpc = pool.mpc.as_deref().expect("mpc");
                    (mpc.funcs().wait_for_idle.unwrap())(adev, mpc, mpcc_inst as i32);
                }
            }
            pool.opps[opp_idx].as_deref_mut().unwrap().base.mpcc_disconnect_pending[mpcc_inst] = false;
            let hubp = pool.hubps[hubp_idx.expect("hubp of mpcc")].as_deref_mut().expect("hubp");
            (hubp.funcs().set_blank.unwrap())(adev, hubp, true);
        }
    }

    if dc.debug.sanity_checks {
        (verify.unwrap())(adev, dc);
    }
}

/// `dcn10_plane_atomic_disconnect` (dcn10_hwseq.c): take the pipe's DPP out
/// of its OPP's blending tree. A tree has nodes only after a plane was
/// committed; until then there is nothing to remove.
pub fn dcn10_plane_atomic_disconnect(adev: &amdgpu_device, dc: &mut dc, pipe_idx: usize) {
    let state = dc.current_state.as_deref().expect("state");
    let pipe = state.res_ctx.pipe_ctx[pipe_idx];
    let pool = dc.res_pool.as_deref().expect("pool");
    let dpp_id = pool.dpps[pipe.plane_res.dpp.expect("dpp")].as_deref().expect("dpp").base.inst as i32;
    let mpc = pool.mpc.as_deref().expect("mpc");
    let opp = pool.opps[pipe.stream_res.opp.expect("opp")].as_deref().expect("opp");

    let mpc_tree_params = &opp.base.mpc_tree_params;
    let mpcc_to_remove = (mpc.funcs().get_mpcc_for_dpp.unwrap())(mpc, mpc_tree_params, dpp_id);

    // Already reset
    let Some(mpcc_to_remove) = mpcc_to_remove else { return };

    let opp_idx = pipe.stream_res.opp.expect("opp");
    let pool = dc.res_pool.as_deref_mut().expect("pool");
    let mpc = pool.mpc.as_deref_mut().expect("mpc");
    let opp = pool.opps[opp_idx].as_deref_mut().expect("opp");
    (mpc.funcs().remove_mpcc.unwrap())(adev, mpc, &mut opp.base.mpc_tree_params, mpcc_to_remove);
    // Phantom pipes have OTG disabled by default, so MPCC_STATUS will never
    // assert idle; DCN 2.1 has none.
    opp.base.mpcc_disconnect_pending[pipe.plane_res.mpcc_inst as usize] = true;

    dc.optimized_required = true;

    // dcn21_hubp_funcs has no hubp_disconnect.

    if dc.debug.sanity_checks {
        (dc.hwseq.as_deref().unwrap().funcs.verify_allow_pstate_change_high.unwrap())(adev, dc);
    }
}

/// `dcn10_plane_atomic_power_down` (dcn10_hwseq.c): power gate the DPP and
/// HUBP under an IP request.
pub fn dcn10_plane_atomic_power_down(adev: &amdgpu_device, dc: &mut dc, dpp_idx: usize, hubp_idx: usize) {
    let hws = dc.hwseq.as_deref().expect("hwseq");
    let c = hws.rsm();
    let funcs = hws.funcs;
    let (dpp_inst, hubp_inst) = {
        let pool = dc.res_pool.as_deref().expect("pool");
        (pool.dpps[dpp_idx].as_deref().expect("dpp").base.inst, pool.hubps[hubp_idx].as_deref().expect("hubp").base.inst)
    };

    if c.0.DC_IP_REQUEST_CNTL != 0 {
        reg_set!(adev, c, DC_IP_REQUEST_CNTL, 0, IP_REQUEST_EN, 1);

        if let Some(f) = funcs.dpp_pg_control {
            f(adev, dc, dpp_inst, false);
        }

        if let Some(f) = funcs.hubp_pg_control {
            f(adev, dc, hubp_inst, false);
        }

        let pool = dc.res_pool.as_deref_mut().expect("pool");
        let hubp = pool.hubps[hubp_idx].as_deref_mut().unwrap();
        (hubp.funcs().hubp_reset.unwrap())(hubp);
        let dpp = pool.dpps[dpp_idx].as_deref_mut().unwrap();
        (dpp.funcs().dpp_reset.unwrap())(dpp);

        reg_set!(adev, c, DC_IP_REQUEST_CNTL, 0, IP_REQUEST_EN, 0);
    }

    if let Some(f) = funcs.dpp_root_clock_control {
        f(adev, dc, dpp_inst, false);
    }
}

/// `dcn10_bios_golden_init` (dcn10_hwseq.c): the VBIOS display init for the
/// whole DCN and then every pipe, keeping a disallowed self refresh the
/// command table enables.
pub fn dcn10_bios_golden_init(adev: &amdgpu_device, dc: &mut dc) {
    let funcs = dc.hwseq.as_deref().expect("hwseq").funcs;
    let mut allow_self_fresh_force_enable = true;

    if let Some(f) = funcs.s0i3_golden_init_wa
        && f(adev, dc)
    {
        return;
    }

    let hubbub_funcs = dc.res_pool.as_deref().and_then(|p| p.hubbub.as_deref()).expect("hubbub").funcs();
    if let Some(f) = hubbub_funcs.is_allow_self_refresh_enabled {
        allow_self_fresh_force_enable = f(adev, dc.res_pool.as_deref().unwrap().hubbub.as_deref().unwrap());
    }

    // initialize dcn global
    crate::command_table2::bios_parser_enable_disp_power_gating(adev, dc, CONTROLLER_ID_D0, ASIC_PIPE_INIT);

    let pipe_count = dc.res_pool.as_deref().unwrap().pipe_count;
    for i in 0..pipe_count {
        // initialize dcn per pipe
        crate::command_table2::bios_parser_enable_disp_power_gating(adev, dc, CONTROLLER_ID_D0 + i,
            ASIC_PIPE_DISABLE);
    }

    if let Some(allow) = hubbub_funcs.allow_self_refresh_control {
        let hubbub = dc.res_pool.as_deref().unwrap().hubbub.as_deref().unwrap();
        if !allow_self_fresh_force_enable
            && (hubbub_funcs.is_allow_self_refresh_enabled.unwrap())(adev, hubbub)
        {
            allow(adev, hubbub, !dc.debug.disable_stutter);
        }
    }
}

/// `dcn10_init_pipes` (dcn10_hwseq.c): blank every running OTG, disconnect
/// and power down every plane, re-init every OTG and power gate the DSCs a
/// running OTG does not use. `context` is the state whose streams are kept
/// on a seamless boot, dc's current state when None; the pipes reset are
/// the current state's either way.
pub fn dcn10_init_pipes(adev: &amdgpu_device, dc: &mut dc, context: Option<&crate::dc_state::dc_state>) {
    let funcs = dc.hwseq.as_deref().expect("hwseq").funcs;
    let mut tg_enabled = [false; MAX_PIPES];
    let pipe_count = dc.res_pool.as_deref().expect("pool").pipe_count as usize;
    let res_cap = dc.res_pool.as_deref().unwrap().res_cap.expect("res_cap");

    let (ctx_pipes, can_apply_seamless_boot) = {
        let context = context.unwrap_or_else(|| dc.current_state.as_deref().unwrap());
        let pipes = context.res_ctx.pipe_ctx;
        let seamless = (0..context.stream_count as usize)
            .any(|i| dc.streams[context.streams[i].unwrap()].as_deref().unwrap().apply_seamless_boot_optimization);
        (pipes, seamless)
    };

    let stream_of = |_dc: &dc, i: usize| ctx_pipes[i].stream;

    for i in 0..pipe_count {
        // A pipe with a stream is kept when booting seamlessly.
        if stream_of(dc, i).is_some() && can_apply_seamless_boot {
            continue;
        }

        let pool = dc.res_pool.as_deref().unwrap();
        let tg = pool.timing_generators[i].as_deref().expect("tg");

        // Blank controller using driver code instead of command table.
        if (tg.funcs().is_tg_enabled.unwrap())(adev, tg) {
            if let Some(init_blank) = funcs.init_blank {
                init_blank(adev, dc, i);
                (tg.funcs().lock.unwrap())(adev, tg);
            } else {
                crate::linux::Line::new().s("dcn10_init_pipes: set_blank path not ported").say();
            }
        }
    }

    // Reset det size
    for i in 0..pipe_count {
        // Do not need to reset for seamless boot
        if stream_of(dc, i).is_some() && can_apply_seamless_boot {
            continue;
        }

        let pool = dc.res_pool.as_deref().unwrap();
        if let (Some(hubbub), Some(hubp)) = (pool.hubbub.as_deref(), pool.hubps[i].as_deref()) {
            if let Some(f) = hubbub.funcs().program_det_size {
                f(adev, hubbub, hubp.base.inst, 0);
            }
            if let Some(f) = hubbub.funcs().program_det_segments {
                f(adev, hubbub, hubp.base.inst, 0);
            }
        }
    }

    // num_opp will be equal to number of mpcc
    for i in 0..res_cap.num_opp as usize {
        // Cannot reset the MPC mux if seamless boot
        if stream_of(dc, i).is_some() && can_apply_seamless_boot {
            continue;
        }

        let mpc = dc.res_pool.as_deref_mut().unwrap().mpc.as_deref_mut().expect("mpc");
        (mpc.funcs().mpc_init_single_inst.unwrap())(adev, mpc, i as u32);
    }

    for i in 0..pipe_count {
        // There is assumption that pipe_ctx is not mapping irregularly to
        // non-preferred front end. If pipe_ctx->stream is not NULL, we will
        // use the pipe, so don't disable
        if can_apply_seamless_boot && stream_of(dc, i).is_some() {
            let pool = dc.res_pool.as_deref_mut().unwrap();
            let ctg = pool.timing_generators[ctx_pipes[i].stream_res.tg.unwrap()].as_deref().unwrap();
            if (ctg.funcs().is_tg_enabled.unwrap())(adev, ctg) {
                // Enable double buffering for OTG_BLANK no matter if seamless
                // boot is enabled or not to suppress global sync signals when
                // OTG blanked. This is to prevent pipe from requesting data
                // while in PSR.
                let tg = pool.timing_generators[i].as_deref().unwrap();
                (tg.funcs().tg_init.unwrap())(adev, tg);
                pool.hubps[i].as_deref_mut().unwrap().base.power_gated = true;
                tg_enabled[i] = true;
                continue;
            }
        }

        // Disable on the current state so the new one isn't cleared.
        {
            let pool = dc.res_pool.as_deref_mut().unwrap();
            let pipe = &mut dc.current_state.as_deref_mut().unwrap().res_ctx.pipe_ctx[i];

            let hubp = pool.hubps[i].as_deref_mut().expect("hubp");
            (hubp.funcs().hubp_reset.unwrap())(hubp);
            let dpp = pool.dpps[i].as_deref_mut().expect("dpp");
            (dpp.funcs().dpp_reset.unwrap())(dpp);
            let dpp_inst = dpp.base.inst;

            pipe.stream_res.tg = Some(i);
            pipe.pipe_idx = i as u8;

            pipe.plane_res.hubp = Some(i);
            pipe.plane_res.dpp = Some(i);
            pipe.plane_res.mpcc_inst = dpp_inst as u8;
            let hubp = pool.hubps[i].as_deref_mut().unwrap();
            hubp.base.mpcc_id = dpp_inst as i32;
            hubp.base.opp_id = OPP_ID_INVALID;
            hubp.base.power_gated = false;

            let opp = pool.opps[i].as_deref_mut().expect("opp");
            opp.base.mpc_tree_params.opp_id = opp.base.inst as i32;
            opp.base.mpc_tree_params.opp_list = None;
            opp.base.mpcc_disconnect_pending[pipe.plane_res.mpcc_inst as usize] = true;
            pipe.stream_res.opp = Some(i);
        }

        (funcs.plane_atomic_disconnect.unwrap())(adev, dc, i);

        {
            let tg = dc.res_pool.as_deref().unwrap().timing_generators[i].as_deref().unwrap();
            if (tg.funcs().is_tg_enabled.unwrap())(adev, tg) {
                (tg.funcs().unlock.unwrap())(adev, tg);
            }
        }

        (dc.hwss.disable_plane.unwrap())(adev, dc, i);

        let pipe = &mut dc.current_state.as_deref_mut().unwrap().res_ctx.pipe_ctx[i];
        pipe.stream_res.tg = None;
        pipe.plane_res.hubp = None;

        let tg = dc.res_pool.as_deref().unwrap().timing_generators[i].as_deref().unwrap();
        if (tg.funcs().is_tg_enabled.unwrap())(adev, tg) {
            // init_odm is not a member of dcn20_tg_funcs.
        }

        (tg.funcs().tg_init.unwrap())(adev, tg);
    }

    // Clean up MPC tree
    for i in 0..pipe_count {
        if tg_enabled[i] {
            let pool = dc.res_pool.as_deref_mut().unwrap();
            let mpc_array = pool.mpc.as_deref().expect("mpc").base.mpcc_array;
            let opp = pool.opps[i].as_deref_mut().unwrap();
            if let Some(top) = opp.base.mpc_tree_params.opp_list
                && let Some(bot) = mpc_array[top].mpcc_bot
            {
                let bot_id = mpc_array[bot].mpcc_id as usize;

                if bot_id < MAX_MPCC && bot_id < MAX_PIPES && !tg_enabled[bot_id] {
                    opp.base.mpc_tree_params.opp_list = None;
                }
            }
        }
    }

    // Power gate DSCs
    if let Some(dsc_pg_control) = funcs.dsc_pg_control {
        let mut num_opps = 0u32;
        let mut opp_id_src0 = OPP_ID_INVALID_U32;
        let mut opp_id_src1 = OPP_ID_INVALID_U32;
        let pool = dc.res_pool.as_deref().unwrap();

        // Step 1: find the OPTC that runs with DSC on. The pool's count is the
        // chip's (fused pipes taken out), not the built-in default.
        for i in 0..pool.timing_generator_count as usize {
            let mut optc_dsc_state = 0u32;
            let tg = pool.timing_generators[i].as_deref().unwrap();

            if (tg.funcs().is_tg_enabled.unwrap())(adev, tg) {
                if let Some(f) = tg.funcs().get_dsc_status {
                    f(adev, tg, &mut optc_dsc_state);
                }
                // Only one OPTC runs DSC; non-zero is DSC enabled.
                if optc_dsc_state != 0 {
                    (tg.funcs().get_optc_source.unwrap())(adev, tg, &mut num_opps, &mut opp_id_src0,
                        &mut opp_id_src1);
                    break;
                }
            }
        }

        // Step 2: power down every DSC but the running OPTC's.
        for i in 0..res_cap.num_dsc as usize {
            let mut s = dcn_dsc_state::default();
            let dsc = pool.dscs[i].as_deref().expect("dsc");

            (dsc.funcs().dsc_read_state.unwrap())(adev, dsc, &mut s);

            if (s.dsc_opp_source == opp_id_src0 || s.dsc_opp_source == opp_id_src1)
                && s.dsc_clock_en != 0
                && s.dsc_fw_en != 0
            {
                continue;
            }

            dsc_pg_control(adev, dc, dsc.base.inst, false);
        }
    }
}

/// `dcn10_init_hw` (dcn10_hwseq.c): take the display hardware over from the
/// VBIOS. `is_optimized_init_done` is false in Linux too; the DMCUB's
/// optimized init only skips the golden init.
pub fn dcn10_init_hw(adev: &amdgpu_device, dc: &mut dc) {
    let funcs = dc.hwseq.as_deref().expect("hwseq").funcs;
    let mut backlight = MAX_BACKLIGHT_LEVEL;
    let mut user_level = MAX_BACKLIGHT_LEVEL;
    let is_optimized_init_done = false;

    if let Some(clk_mgr) = dc.clk_mgr.as_deref_mut()
        && let Some(f) = clk_mgr.funcs().init_clocks
    {
        f(clk_mgr);
    }

    // Align bw context with hw config when system resume.
    if let Some(clk_mgr) = dc.clk_mgr.as_deref()
        && clk_mgr.base.clks.dispclk_khz != 0
        && clk_mgr.base.clks.dppclk_khz != 0
    {
        let clks = clk_mgr.base.clks;
        let state = dc.current_state.as_deref_mut().expect("state");
        state.bw_ctx.dcn.clk.dispclk_khz = clks.dispclk_khz;
        state.bw_ctx.dcn.clk.dppclk_khz = clks.dppclk_khz;
    }

    // Initialize the dccg
    if let Some(dccg) = dc.res_pool.as_deref().and_then(|p| p.dccg.as_deref())
        && let Some(f) = dccg.funcs().dccg_init
    {
        f(dccg);
    }

    if !crate::bios_parser2::bios_parser_is_accelerated_mode(adev, &dc.ctx.dc_bios.base) {
        (funcs.disable_vga.unwrap())(adev, dc.hwseq.as_deref().unwrap());
    }

    if !crate::dc_dmub_srv::dc_dmub_srv_optimized_init_done(adev, dc.ctx.dmub_srv.as_deref()) {
        (funcs.bios_golden_init.unwrap())(adev, dc);
    }

    if dc.ctx.dc_bios.base.fw_info_valid {
        let crystal = dc.ctx.dc_bios.base.fw_info.pll_info.crystal_frequency;
        let pool = dc.res_pool.as_deref_mut().expect("pool");
        pool.ref_clocks.xtalin_clock_inKhz = crystal;

        if let (Some(dccg), Some(hubbub)) = (pool.dccg.as_deref(), pool.hubbub.as_deref()) {
            let mut dccg_ref = 0;
            (dccg.funcs().get_dccg_ref_freq.unwrap())(adev, dccg, crystal, &mut dccg_ref);
            pool.ref_clocks.dccg_ref_clock_inKhz = dccg_ref;

            let mut dchub_ref = 0;
            (hubbub.funcs().get_dchub_ref_freq.unwrap())(adev, hubbub, dccg_ref, &mut dchub_ref);
            pool.ref_clocks.dchub_ref_clock_inKhz = dchub_ref;
        } else {
            // Not all ASICs have DCCG sw component
            pool.ref_clocks.dccg_ref_clock_inKhz = pool.ref_clocks.xtalin_clock_inKhz;
            pool.ref_clocks.dchub_ref_clock_inKhz = pool.ref_clocks.xtalin_clock_inKhz;
        }
    }

    for i in 0..dc.link_count as usize {
        // Power up AND update implementation according to the required signal
        // (which may be different from the default signal on connector).
        let link = &mut dc.links[i];
        let enc = link.link_enc.as_ref().expect("link encoder");

        if !is_optimized_init_done {
            (enc.funcs().hw_init.unwrap())(adev, enc);
        }

        // Check for enabled DIG to identify enabled display
        if let Some(is_dig_enabled) = enc.funcs().is_dig_enabled
            && is_dig_enabled(adev, enc)
        {
            let fec_active = enc.funcs().fec_is_active.is_some_and(|f| f(adev, enc));
            link.link_status.link_active = true;
            if fec_active {
                link.fec_state = dc_link_fec_enabled;
            }
        }
    }

    // we want to turn off all dp displays before doing detection
    crate::link_dpms::link_blank_all_dp_displays(adev, dc);

    if let Some(f) = funcs.enable_power_gating_plane {
        f(adev, dc.hwseq.as_deref().unwrap(), true);
    }

    // When taking control over from the VBIOS for a seamless first mode set,
    // the pipes stay up until it is known which are wanted; otherwise
    // everything is powered down.
    if crate::bios_parser2::bios_parser_is_accelerated_mode(adev, &dc.ctx.dc_bios.base)
        || !dc.config.seamless_boot_edp_requested
    {
        if !is_optimized_init_done {
            (funcs.init_pipes.unwrap())(adev, dc, None);
            let hubbub = dc.res_pool.as_deref().unwrap().hubbub.as_deref().expect("hubbub");
            if let Some(f) = hubbub.funcs().allow_self_refresh_control {
                f(adev, hubbub, !dc.debug.disable_stutter);
            }
        }
    }

    if !is_optimized_init_done {
        let pool = dc.res_pool.as_deref().unwrap();
        for i in 0..pool.audio_count as usize {
            let audio = pool.audios[i].as_deref().expect("audio");
            (audio.funcs().hw_init.unwrap())(adev, audio);
        }

        for i in 0..dc.link_count as usize {
            if let Some(panel_cntl) = dc.links[i].panel_cntl.as_deref_mut() {
                backlight = (panel_cntl.funcs().hw_init.unwrap())(adev, panel_cntl);
                user_level = panel_cntl.stored_backlight_registers.USER_LEVEL;
            }
        }

        if let Some(abm) = dc.res_pool.as_deref().unwrap().abm.as_deref() {
            let f = abm.funcs().abm_init.unwrap();
            f(adev, dc, backlight, user_level);
        }

        // res_pool->dmcu is NULL with a DMCUB (config.disable_dmcu).
    }

    // abm->dmcu_is_running needs a dmcu.

    // power AFMT HDMI memory
    let c = dc.hwseq.as_deref().unwrap().rsm();
    if !is_optimized_init_done {
        reg_write!(adev, c.0, DIO_MEM_PWR_CTRL, 0);
    }

    if !dc.debug.disable_clock_gate {
        // enable all DCN clock gating
        reg_write!(adev, c.0, DCCG_GATE_DISABLE_CNTL, 0);

        reg_write!(adev, c.0, DCCG_GATE_DISABLE_CNTL2, 0);

        reg_update!(adev, c, DCFCLK_CNTL, DCFCLK_GATE_DIS, 0);
    }

    if dc.clk_mgr.as_deref().is_some_and(|m| m.funcs().notify_wm_ranges.is_some()) {
        let mut clk_mgr = dc.clk_mgr.take().unwrap();
        (clk_mgr.funcs().notify_wm_ranges.unwrap())(adev, dc, &mut clk_mgr);
        dc.clk_mgr = Some(clk_mgr);
    }
}

/// `apply_front_porch_workaround` (dcn10_hwseq.c).
fn apply_front_porch_workaround(timing: &mut crate::dc_hw_types::dc_crtc_timing) {
    if timing.flags.INTERLACE {
        if timing.v_front_porch < 2 {
            timing.v_front_porch = 2;
        }
    } else if timing.v_front_porch < 1 {
        timing.v_front_porch = 1;
    }
}

/// `dcn10_get_vupdate_offset_from_vsync` (dcn10_hwseq.c).
pub fn dcn10_get_vupdate_offset_from_vsync(dc: &dc, context: &crate::dc_state::dc_state, p: usize) -> i32 {
    let pipe_ctx = &context.res_ctx.pipe_ctx[p];
    let mut patched_crtc_timing = dc.streams[pipe_ctx.stream.unwrap()].as_deref().unwrap().timing;
    apply_front_porch_workaround(&mut patched_crtc_timing);

    let interlace_factor = if patched_crtc_timing.flags.INTERLACE { 2 } else { 1 };

    let vesa_sync_start = (patched_crtc_timing.v_addressable + patched_crtc_timing.v_border_bottom
        + patched_crtc_timing.v_front_porch) as i32;

    let asic_blank_end = (patched_crtc_timing.v_total as i32 - vesa_sync_start
        - patched_crtc_timing.v_border_top as i32) * interlace_factor;

    asic_blank_end - pipe_ctx.pipe_dlg_param.vstartup_start as i32 + 1
}

/// `dcn10_config_stereo_parameters` (dcn10_hwseq.c).
fn dcn10_config_stereo_parameters(dc: &dc, stream: &crate::dc_stream::dc_stream_state,
                                  flags: &mut crate::dcn20_optc::crtc_stereo_flags) {
    let view_format = stream.view_format;
    let timing_3d_format = stream.timing.timing_3d_format;
    let non_stereo_timing = timing_3d_format == TIMING_3D_FORMAT_NONE
        || timing_3d_format == TIMING_3D_FORMAT_SIDE_BY_SIDE
        || timing_3d_format == TIMING_3D_FORMAT_TOP_AND_BOTTOM;

    if !non_stereo_timing && view_format == VIEW_3D_FORMAT_FRAME_SEQUENTIAL {
        flags.PROGRAM_STEREO = true;
        flags.PROGRAM_POLARITY = true;
        if timing_3d_format == TIMING_3D_FORMAT_FRAME_ALTERNATE
            || timing_3d_format == TIMING_3D_FORMAT_INBAND_FA
            || timing_3d_format == TIMING_3D_FORMAT_DP_HDMI_INBAND_FA
            || timing_3d_format == TIMING_3D_FORMAT_SIDEBAND_FA
        {
            if let Some(ddc) = dc.links[stream.link].ddc.as_deref() {
                let dongle = ddc.dongle_type;

                if dongle == crate::dc_ddc_types::DISPLAY_DONGLE_DP_VGA_CONVERTER
                    || dongle == crate::dc_ddc_types::DISPLAY_DONGLE_DP_DVI_CONVERTER
                    || dongle == crate::dc_ddc_types::DISPLAY_DONGLE_DP_HDMI_CONVERTER
                {
                    flags.DISABLE_STEREO_DP_SYNC = true;
                }
            }
        }
        flags.RIGHT_EYE_POLARITY = stream.timing.flags.RIGHT_EYE_3D_POLARITY;
        if timing_3d_format == TIMING_3D_FORMAT_HW_FRAME_PACKING {
            flags.FRAME_PACKED = true;
        }
    }
}

/// `dcn10_setup_stereo` (dcn10_hwseq.c): the stereo sync GPIO for a
/// sideband frame-alternate timing, then the OPP's and OTG's stereo.
pub fn dcn10_setup_stereo(adev: &amdgpu_device, dc: &mut dc, context: &mut crate::dc_state::dc_state, p: usize) {
    let mut flags = crate::dcn20_optc::crtc_stereo_flags::default();
    let pipe_ctx = context.res_ctx.pipe_ctx[p];
    let stream = dc.streams[pipe_ctx.stream.unwrap()].as_deref().unwrap();

    dcn10_config_stereo_parameters(dc, stream, &mut flags);

    if stream.timing.timing_3d_format == TIMING_3D_FORMAT_SIDEBAND_FA {
        if !crate::dc::dc_set_generic_gpio_for_stereo(adev, dc, true) {
            crate::dc::dc_set_generic_gpio_for_stereo(adev, dc, false);
        }
    } else {
        crate::dc::dc_set_generic_gpio_for_stereo(adev, dc, false);
    }

    let timing = dc.streams[pipe_ctx.stream.unwrap()].as_deref().unwrap().timing;
    let pool = dc.res_pool.as_deref().unwrap();
    let opp = pool.opps[pipe_ctx.stream_res.opp.unwrap()].as_deref().unwrap();
    (opp.funcs().opp_program_stereo.unwrap())(adev, opp, flags.PROGRAM_STEREO, &timing);

    let tg = pool.timing_generators[pipe_ctx.stream_res.tg.unwrap()].as_deref().unwrap();
    (tg.funcs().program_stereo.unwrap())(adev, tg, &timing, &flags);
}

/// `dcn10_lock_all_pipes` (dcn10_hwseq.c): (un)lock the top pipe's TG of
/// every enabled pipe that has or had a plane. `context` None is the
/// current state. DCN 2.1 has no SubVP phantoms.
pub fn dcn10_lock_all_pipes(adev: &amdgpu_device, dc: &mut dc, context: Option<&crate::dc_state::dc_state>, lock: bool) {
    let pipe_count = dc.res_pool.as_deref().unwrap().pipe_count as usize;
    let cur_res = dc.current_state.as_deref().unwrap().res_ctx;
    let res = context.map_or(cur_res, |c| c.res_ctx);

    for i in 0..pipe_count {
        let old_pipe_ctx = &cur_res.pipe_ctx[i];
        let pipe_ctx = &res.pipe_ctx[i];

        // Only lock the top pipe's tg to prevent redundant (un)locking. Also
        // skip if pipe is disabled.
        if pipe_ctx.top_pipe.is_some() || pipe_ctx.stream.is_none()
            || (pipe_ctx.plane_state.is_none() && old_pipe_ctx.plane_state.is_none())
        {
            continue;
        }
        let tg = dc.res_pool.as_deref().unwrap().timing_generators[pipe_ctx.stream_res.tg.unwrap()].as_deref().unwrap();
        if !(tg.funcs().is_tg_enabled.unwrap())(adev, tg) {
            continue;
        }

        (dc.hwss.pipe_control_lock.unwrap())(adev, dc, &res, i, lock);
    }
}

/// `dcn10_program_gamut_remap` (dcn10_hwseq.c): the stream's remap, else
/// the plane's, else bypass.
pub fn dcn10_program_gamut_remap(adev: &amdgpu_device, dc: &mut dc, context: &crate::dc_state::dc_state, p: usize) {
    use crate::dcn20_dpp_cm::dpp_grph_csc_adjustment;
    let pipe_ctx = &context.res_ctx.pipe_ctx[p];
    let mut adjust = dpp_grph_csc_adjustment { gamut_adjust_type: GRAPHICS_GAMUT_ADJUST_TYPE_BYPASS, ..Default::default() };
    let stream = dc.streams[pipe_ctx.stream.unwrap()].as_deref().unwrap();

    if stream.gamut_remap_matrix.enable_remap {
        adjust.gamut_adjust_type = GRAPHICS_GAMUT_ADJUST_TYPE_SW;
        adjust.temperature_matrix = stream.gamut_remap_matrix.matrix;
    } else if let Some(ps) = pipe_ctx.plane_state.map(|i| dc.planes[i].as_deref().unwrap())
        && ps.gamut_remap_matrix.enable_remap
    {
        adjust.gamut_adjust_type = GRAPHICS_GAMUT_ADJUST_TYPE_SW;
        adjust.temperature_matrix = ps.gamut_remap_matrix.matrix;
    }

    let dpp = dc.res_pool.as_deref().unwrap().dpps[pipe_ctx.plane_res.dpp.unwrap()].as_deref().unwrap();
    (dpp.funcs().dpp_set_gamut_remap.unwrap())(adev, dpp, &adjust);
}

/// `dcn10_update_visual_confirm_color` (dcn10_hwseq.c).
pub fn dcn10_update_visual_confirm_color(adev: &amdgpu_device, dc: &mut dc, pipe_ctx: &crate::dc_state::pipe_ctx,
                                         mpcc_id: i32) {
    let mpc = dc.res_pool.as_deref_mut().unwrap().mpc.as_deref_mut().unwrap();

    if let Some(set_bg_color) = mpc.funcs().set_bg_color {
        set_bg_color(adev, mpc, &pipe_ctx.visual_confirm_color, mpcc_id);
    }
}

/// `dcn10_set_hdr_multiplier` (dcn10_hwseq.c): the plane's multiplier as a
/// custom float (6 exponent, 12 mantissa bits, signed), 1.0 for none.
pub fn dcn10_set_hdr_multiplier(adev: &amdgpu_device, dc: &dc, pipe_ctx: &crate::dc_state::pipe_ctx) {
    use crate::fixpt31_32::{dc_fixpt_eq, dc_fixpt_from_int};
    let multiplier = dc.planes[pipe_ctx.plane_state.unwrap()].as_deref().unwrap().hdr_mult;
    let mut hw_mult = 0x1f000; // 1.0 default multiplier
    let fmt = crate::conversion::custom_float_format { exponenta_bits: 6, mantissa_bits: 12, sign: true };

    if !dc_fixpt_eq(multiplier, dc_fixpt_from_int(0)) {
        // check != 0
        crate::conversion::convert_to_custom_float_format(multiplier, &fmt, &mut hw_mult);
    }

    let dpp = dc.res_pool.as_deref().unwrap().dpps[pipe_ctx.plane_res.dpp.unwrap()].as_deref().unwrap();
    (dpp.funcs().dpp_set_hdr_multiplier.unwrap())(adev, dpp, hw_mult);
}

/// `dcn10_update_pending_status` (dcn10_hwseq.c): whether the plane's flip
/// is still pending, and the address it shows once not. DCN 2.1's TG has
/// no `is_stereo_left_eye`.
pub fn dcn10_update_pending_status(adev: &amdgpu_device, dc: &mut dc, pipe_ctx: &crate::dc_state::pipe_ctx) {
    let Some(plane) = pipe_ctx.plane_state else { return };

    let hubp = dc.res_pool.as_deref().unwrap().hubps[pipe_ctx.plane_res.hubp.unwrap()].as_deref().unwrap();
    let flip_pending = (hubp.funcs().hubp_is_flip_pending.unwrap())(adev, hubp);

    let plane_state = dc.planes[plane].as_deref_mut().unwrap();
    plane_state.status.is_flip_pending = plane_state.status.is_flip_pending || flip_pending;

    if !flip_pending {
        plane_state.status.current_address = plane_state.status.requested_address;
    }

    // The multi-plane self refresh workaround is never applied: its arming
    // in dcn20_post_unlock_program_front_end is named, not ported.
}
