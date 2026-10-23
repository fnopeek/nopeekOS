//! display/dc/resource/dcn20/dcn20_resource.c (Linux 6.18.55), the pool
//! functions DCN 2.1's `dcn21_res_pool_funcs` takes from DCN 2.0 for adding
//! a stream: mapping it onto the pool and building the pipe's pixel clock,
//! dither and clamping parameters.
#![allow(dead_code)]

use crate::dc::dc;
use crate::dc_resource::{clock_source_of, resource_get_otg_master_for_stream, resource_map_phy_clock_resources,
    resource_map_pool_resources};
use crate::dc_state::dc_state;
use crate::dc_stream::dc_stream_state;
use crate::dc_types::dc_status;
use crate::dce_clock_source::pixel_clk_params;
use crate::regs::*;

/// `get_pixel_clock_parameters`. DCN 2.1's hw sequencer has no
/// `is_dp_dig_pixel_rate_div_policy`.
fn get_pixel_clock_parameters(dc: &dc, context: &dc_state, pipe: usize, stream: &dc_stream_state,
                              pixel_clk_params: &mut pixel_clk_params) {
    let pipe_ctx = &context.res_ctx.pipe_ctx[pipe];
    let pool = dc.res_pool.as_deref().unwrap();
    let link = &dc.links[stream.link];
    let mut opp_cnt = 1;

    let mut odm_pipe = pipe_ctx.next_odm_pipe;
    while let Some(o) = odm_pipe {
        opp_cnt += 1;
        odm_pipe = context.res_ctx.pipe_ctx[o].next_odm_pipe;
    }

    pixel_clk_params.requested_pix_clk_100hz = stream.timing.pix_clk_100hz;

    let mut link_enc = pipe_ctx.link_res.dio_link_enc;
    if !dc.config.unify_link_enc_assignment {
        // link_enc_cfg_get_link_enc: a fixed DIG mapping is the link's own
        link_enc = if link.is_dig_mapping_flexible {
            crate::linux::Line::new().s("link_enc_cfg_get_link_enc: dynamic assignment not ported").say();
            None
        } else {
            link.link_enc.as_ref().map(|_| stream.link as u32)
        };
    }
    if let Some(l) = link_enc {
        pixel_clk_params.encoder_object_id = dc.links[l as usize].link_enc.as_ref().unwrap().base().id;
    }

    let tg = pool.timing_generators[pipe_ctx.stream_res.tg.unwrap()].as_deref().unwrap();
    pixel_clk_params.signal_type = stream.signal;
    pixel_clk_params.controller_id = tg.base.inst + 1;
    // TODO: un-hardcode
    // TODO - DP2.0 HW: calculate requested_sym_clk for UHBR rates
    pixel_clk_params.requested_sym_clk = crate::dc_dp_types::LINK_RATE_LOW * LINK_RATE_REF_FREQ_IN_KHZ;
    pixel_clk_params.flags.ENABLE_SS = false;
    pixel_clk_params.color_depth = stream.timing.display_color_depth;
    pixel_clk_params.flags.DISPLAY_BLANKED = true;
    pixel_clk_params.pixel_encoding = stream.timing.pixel_encoding;

    if stream.timing.pixel_encoding == PIXEL_ENCODING_YCBCR422 {
        pixel_clk_params.color_depth = COLOR_DEPTH_888;
    }

    let two_pixels = crate::dcn20_optc::optc1_is_two_pixels_per_container(&stream.timing);
    if opp_cnt == 4 {
        pixel_clk_params.requested_pix_clk_100hz /= 4;
    } else if two_pixels || opp_cnt == 2 {
        pixel_clk_params.requested_pix_clk_100hz /= 2;
    }

    if stream.timing.timing_3d_format == TIMING_3D_FORMAT_HW_FRAME_PACKING {
        pixel_clk_params.requested_pix_clk_100hz *= 2;
    }

    if two_pixels || opp_cnt > 1 {
        pixel_clk_params.dio_se_pix_per_cycle = 2;
    } else {
        pixel_clk_params.dio_se_pix_per_cycle = 1;
    }
}

/// `build_clamping_params`.
fn build_clamping_params(stream: &mut dc_stream_state) {
    stream.clamping.clamping_level = CLAMPING_FULL_RANGE;
    stream.clamping.c_depth = stream.timing.display_color_depth;
    stream.clamping.pixel_encoding = stream.timing.pixel_encoding;
}

/// `dcn20_build_pipe_pix_clk_params`.
pub fn dcn20_build_pipe_pix_clk_params(dc: &dc, context: &mut dc_state, pipe: usize) {
    let stream = dc.streams[context.res_ctx.pipe_ctx[pipe].stream.unwrap()].as_deref().unwrap();
    let mut params = context.res_ctx.pipe_ctx[pipe].stream_res.pix_clk_params;
    get_pixel_clock_parameters(dc, context, pipe, stream, &mut params);

    let pipe_ctx = &mut context.res_ctx.pipe_ctx[pipe];
    pipe_ctx.stream_res.pix_clk_params = params;
    let cs = clock_source_of(dc.res_pool.as_deref().unwrap(), pipe_ctx.clock_source.unwrap());
    crate::dce_clock_source::dce112_get_pix_clk_dividers(cs, &pipe_ctx.stream_res.pix_clk_params,
        &mut pipe_ctx.pll_settings);
}

/// `build_pipe_hw_param`: DCN 2.1's pool has no `build_pipe_pix_clk_params`.
fn build_pipe_hw_param(dc: &mut dc, context: &mut dc_state, pipe: usize) -> dc_status {
    dcn20_build_pipe_pix_clk_params(dc, context, pipe);

    let stream = dc.streams[context.res_ctx.pipe_ctx[pipe].stream.unwrap()].as_deref_mut().unwrap();
    stream.clamping.pixel_encoding = stream.timing.pixel_encoding;

    let mut bit_depth_params = stream.bit_depth_params;
    crate::dc_resource::resource_build_bit_depth_reduction_params(stream, &mut bit_depth_params);
    stream.bit_depth_params = bit_depth_params;
    build_clamping_params(stream);

    dc_status::DC_OK
}

/// `dcn20_build_mapped_resource`.
pub fn dcn20_build_mapped_resource(dc: &mut dc, context: &mut dc_state, stream: usize) -> dc_status {
    let Some(pipe) = resource_get_otg_master_for_stream(&context.res_ctx, stream) else {
        return dc_status::DC_ERROR_UNEXPECTED;
    };

    build_pipe_hw_param(dc, context, pipe)
}

/// `dcn20_add_stream_to_ctx`. A DSC stream takes a DSC block
/// (`dcn20_add_dsc_to_stream_resource`), named, not ported.
pub fn dcn20_add_stream_to_ctx(dc: &mut dc, new_ctx: &mut dc_state, dc_stream: usize) -> dc_status {
    let mut result = resource_map_pool_resources(dc, new_ctx, dc_stream);

    if result == dc_status::DC_OK {
        result = resource_map_phy_clock_resources(dc, new_ctx, dc_stream);
    }

    // Get a DSC if required and available
    if result == dc_status::DC_OK && dc.streams[dc_stream].as_deref().unwrap().timing.flags.DSC {
        crate::linux::Line::new().s("dcn20_add_dsc_to_stream_resource not ported").say();
        result = dc_status::DC_NO_DSC_RESOURCE;
    }

    if result == dc_status::DC_OK {
        result = dcn20_build_mapped_resource(dc, new_ctx, dc_stream);
    }

    result
}

/// `dcn20_merge_pipes_for_validate`: undo earlier ODM and MPC splits, so
/// mode support decides them afresh. A DSC block of an ODM pipe would be
/// released (`dcn20_release_dsc`); DCN 2.1 streams here carry none.
pub fn dcn20_merge_pipes_for_validate(dc: &mut dc, context: &mut dc_state) {
    let pipe_count = dc.res_pool.as_deref().unwrap().pipe_count as usize;
    let res = &mut context.res_ctx;

    // merge previously split odm pipes since mode support needs to make the decision
    for i in 0..pipe_count {
        let mut odm_pipe = res.pipe_ctx[i].next_odm_pipe;

        if res.pipe_ctx[i].prev_odm_pipe.is_some() {
            continue;
        }

        res.pipe_ctx[i].next_odm_pipe = None;
        while let Some(o) = odm_pipe {
            let next_odm_pipe = res.pipe_ctx[o].next_odm_pipe;
            let p = &mut res.pipe_ctx[o];

            p.plane_state = None;
            p.stream = None;
            p.top_pipe = None;
            p.bottom_pipe = None;
            p.prev_odm_pipe = None;
            p.next_odm_pipe = None;
            // Clear plane_res and stream_res
            p.plane_res = Default::default();
            p.stream_res = Default::default();
            odm_pipe = next_odm_pipe;
        }
        if res.pipe_ctx[i].plane_state.is_some() {
            crate::dc_resource::resource_build_scaling_params(dc, res, i);
        }
    }

    // merge previously mpc split pipes since mode support needs to make the decision
    for i in 0..pipe_count {
        let Some(h) = res.pipe_ctx[i].bottom_pipe else { continue };
        if res.pipe_ctx[h].plane_state != res.pipe_ctx[i].plane_state {
            continue;
        }

        res.pipe_ctx[i].bottom_pipe = res.pipe_ctx[h].bottom_pipe;
        if let Some(b) = res.pipe_ctx[h].bottom_pipe {
            res.pipe_ctx[b].top_pipe = Some(i);
        }
        let p = &mut res.pipe_ctx[h];
        p.plane_state = None;
        p.stream = None;
        p.top_pipe = None;
        p.bottom_pipe = None;

        // Clear plane_res and stream_res
        p.plane_res = Default::default();
        p.stream_res = Default::default();
        if res.pipe_ctx[i].plane_state.is_some() {
            crate::dc_resource::resource_build_scaling_params(dc, res, i);
        }
    }
}

/// `dcn20_validate_apply_pipe_split_flags`: which pipes DML wants split
/// (MPC or ODM), and the voltage level that allows it.
pub fn dcn20_validate_apply_pipe_split_flags(dc: &dc, context: &mut dc_state, mut vlevel: i32, split: &mut [i32],
                                             merge: &mut [bool]) -> i32 {
    use crate::dc::pipe_split_policy::*;
    use crate::dc_resource::{resource_get_mpc_slice_count, resource_get_odm_slice_count};
    use crate::display_mode_structs as dml;
    let pool = dc.res_pool.as_deref().unwrap();
    let pipe_count = pool.pipe_count as usize;
    let res = &context.res_ctx;
    let mut plane_count = 0;
    let mut force_split = false;
    let mut avoid_split = dc.debug.pipe_split_policy == MPC_SPLIT_AVOID;
    let num_states = context.dml.as_deref().unwrap().soc.num_states as i32;
    let v = &mut context.dml.as_deref_mut().unwrap().vba;
    let mut max_mpc_comb = v.maxMpcComb;

    if context.stream_count > 1 {
        if dc.debug.pipe_split_policy == MPC_SPLIT_AVOID_MULT_DISP {
            avoid_split = true;
        }
    } else if dc.debug.force_single_disp_pipe_split {
        force_split = true;
    }

    for i in 0..pipe_count {
        let pipe = &res.pipe_ctx[i];

        // Workaround for avoiding pipe-split in cases where we'd split
        // planes that are too small, resulting in splits that aren't
        // valid for the scaler.
        if let Some(pl) = pipe.plane_state.map(|p| dc.planes[p].as_deref().unwrap()) {
            if pl.dst_rect.width <= 16 || pl.dst_rect.height <= 16 || pl.src_rect.width <= 16
                || pl.src_rect.height <= 16
            {
                avoid_split = true;
            }
        }

        // TODO: fix dc bugs and remove this split threshold thing
        if pipe.stream.is_some()
            && pipe.prev_odm_pipe.is_none()
            && pipe.top_pipe.is_none_or(|t| res.pipe_ctx[t].plane_state != pipe.plane_state)
        {
            plane_count += 1;
        }
    }
    if plane_count > pipe_count / 2 {
        avoid_split = true;
    }

    // W/A: Mode timing with borders may not work well with pipe split, avoid for this corner case
    for i in 0..pipe_count {
        let Some(s) = res.pipe_ctx[i].stream else { continue };
        let timing = dc.streams[s].as_deref().unwrap().timing;
        if timing.h_border_left + timing.h_border_right + timing.v_border_top + timing.v_border_bottom > 0 {
            avoid_split = true;
            break;
        }
    }

    // Avoid split loop looks for lowest voltage level that allows most unsplit pipes possible
    if avoid_split {
        let mut pipe_idx = 0;
        for i in 0..pipe_count {
            if res.pipe_ctx[i].stream.is_none() {
                continue;
            }

            let vlevel_split = vlevel;
            while vlevel <= num_states {
                if v.NoOfDPP[vlevel as usize][0][pipe_idx] == 1 && v.ModeSupport[vlevel as usize][0] {
                    break;
                }
                vlevel += 1;
            }
            // Impossible to not split this pipe
            if vlevel > num_states {
                vlevel = vlevel_split;
            } else {
                max_mpc_comb = 0;
            }
            pipe_idx += 1;
        }
        v.maxMpcComb = max_mpc_comb;
    }

    // Split loop sets which pipe should be split based on dml outputs and dc flags
    let mut pipe_idx = 0usize;
    for i in 0..pipe_count {
        let pipe = &res.pipe_ctx[i];
        let pipe_plane = v.pipe_plane[pipe_idx] as usize;
        let split4mpc = context.stream_count == 1 && plane_count == 1 && dc.config.enable_4to1MPC && pipe_count >= 4;

        let Some(s) = pipe.stream else { continue };
        let stream = dc.streams[s].as_deref().unwrap();
        let (vl, mm) = (vlevel as usize, max_mpc_comb as usize);

        if split4mpc || v.NoOfDPP[vl][mm][pipe_plane] == 4 {
            split[i] = 4;
        } else if force_split || v.NoOfDPP[vl][mm][pipe_plane] == 2 {
            split[i] = 2;
        }

        if (stream.view_format == VIEW_3D_FORMAT_SIDE_BY_SIDE || stream.view_format == VIEW_3D_FORMAT_TOP_AND_BOTTOM)
            && (stream.timing.timing_3d_format == TIMING_3D_FORMAT_TOP_AND_BOTTOM
                || stream.timing.timing_3d_format == TIMING_3D_FORMAT_SIDE_BY_SIDE)
        {
            split[i] = 2;
        }
        let tg_inst = pool.timing_generators[pipe.stream_res.tg.unwrap()].as_deref().unwrap().base.inst;
        if dc.debug.force_odm_combine & (1 << tg_inst) != 0 {
            split[i] = 2;
            v.ODMCombineEnablePerState[vl][pipe_plane] = dml::dm_odm_combine_mode_2to1;
        }
        if dc.debug.force_odm_combine_4to1 & (1 << tg_inst) != 0 {
            split[i] = 4;
            v.ODMCombineEnablePerState[vl][pipe_plane] = dml::dm_odm_combine_mode_4to1;
        }
        // 420 format workaround
        if stream.timing.h_addressable > 7680 && stream.timing.pixel_encoding == PIXEL_ENCODING_YCBCR420 {
            split[i] = 4;
        }
        v.ODMCombineEnabled[pipe_plane] = v.ODMCombineEnablePerState[vl][pipe_plane];

        let top_same = pipe.top_pipe.is_some_and(|t| res.pipe_ctx[t].plane_state == pipe.plane_state);
        if v.ODMCombineEnabled[pipe_plane] == dml::dm_odm_combine_mode_disabled {
            if resource_get_mpc_slice_count(res, i) == 2 {
                // If need split for mpc but 2 way split already
                if split[i] == 4 {
                    split[i] = 2; // 2 -> 4 MPC
                } else if split[i] == 2 {
                    split[i] = 0; // 2 -> 2 MPC
                } else if top_same {
                    merge[i] = true; // 2 -> 1 MPC
                }
            } else if resource_get_mpc_slice_count(res, i) == 4 {
                // If need split for mpc but 4 way split already
                if split[i] == 2
                    && (pipe.top_pipe.is_some_and(|t| res.pipe_ctx[t].top_pipe.is_none()) || pipe.bottom_pipe.is_none())
                {
                    merge[i] = true; // 4 -> 2 MPC
                } else if split[i] == 0 && top_same {
                    merge[i] = true; // 4 -> 1 MPC
                }
                split[i] = 0;
            } else if resource_get_odm_slice_count(res, i) > 1 {
                // ODM -> MPC transition
                if pipe.prev_odm_pipe.is_some() {
                    split[i] = 0;
                    merge[i] = true;
                }
            }
        } else if resource_get_odm_slice_count(res, i) == 2 {
            // If need split for odm but 2 way split already
            if split[i] == 4 {
                split[i] = 2; // 2 -> 4 ODM
            } else if split[i] == 2 {
                split[i] = 0; // 2 -> 2 ODM
            } else if pipe.prev_odm_pipe.is_some() {
                merge[i] = true; // exit ODM
            }
        } else if resource_get_odm_slice_count(res, i) == 4 {
            // If need split for odm but 4 way split already
            if split[i] == 2
                && (pipe.prev_odm_pipe.is_some_and(|p| res.pipe_ctx[p].prev_odm_pipe.is_none())
                    || pipe.next_odm_pipe.is_none())
            {
                merge[i] = true; // 4 -> 2 ODM
            } else if split[i] == 0 && pipe.prev_odm_pipe.is_some() {
                merge[i] = true; // exit ODM
            }
            split[i] = 0;
        } else if resource_get_mpc_slice_count(res, i) > 1 {
            // MPC -> ODM transition
            if top_same {
                split[i] = 0;
                merge[i] = true;
            }
        }

        // Adjust dppclk when split is forced, do not bother with dispclk
        if split[i] != 0 && v.NoOfDPP[vl][mm][pipe_idx] == 1 {
            crate::dcn20_fpu::dcn20_fpu_adjust_dppclk(v, vlevel, max_mpc_comb, pipe_idx as i32, false);
        }
        pipe_idx += 1;
    }

    vlevel
}

/// `dcn20_find_secondary_pipe`: a free pipe for the second half of a split,
/// preferring the one the current state used.
pub fn dcn20_find_secondary_pipe(dc: &dc, res_ctx: &mut crate::dc_state::resource_context, primary_pipe: usize)
                                 -> Option<usize> {
    let cur = &dc.current_state.as_deref().unwrap().res_ctx;
    let pipe_count = dc.res_pool.as_deref().unwrap().pipe_count as i32;
    let pidx = res_ctx.pipe_ctx[primary_pipe].pipe_idx as usize;
    let mut secondary_pipe = None;

    // first check the prev dc state:
    // if this primary pipe has a bottom pipe in prev. state
    // and if the bottom pipe is still available (which it should be),
    // pick that pipe as secondary
    // Same logic applies for ODM pipes
    if let Some(n) = cur.pipe_ctx[pidx].next_odm_pipe {
        let preferred_pipe_idx = cur.pipe_ctx[n].pipe_idx as usize;
        if res_ctx.pipe_ctx[preferred_pipe_idx].stream.is_none() {
            secondary_pipe = Some(preferred_pipe_idx);
            res_ctx.pipe_ctx[preferred_pipe_idx].pipe_idx = preferred_pipe_idx as u8;
        }
    }
    if secondary_pipe.is_none() {
        if let Some(b) = cur.pipe_ctx[pidx].bottom_pipe {
            let preferred_pipe_idx = cur.pipe_ctx[b].pipe_idx as usize;
            if res_ctx.pipe_ctx[preferred_pipe_idx].stream.is_none() {
                secondary_pipe = Some(preferred_pipe_idx);
                res_ctx.pipe_ctx[preferred_pipe_idx].pipe_idx = preferred_pipe_idx as u8;
            }
        }
    }

    // if this primary pipe does not have a bottom pipe in prev. state
    // start backward and find a pipe that did not used to be a bottom pipe in
    // prev. dc state. This way we make sure we keep the same assignment as
    // last state and will not have to reprogram every pipe
    if secondary_pipe.is_none() {
        let mut j = pipe_count - 1;
        while j >= 0 {
            let ju = j as usize;
            if cur.pipe_ctx[ju].top_pipe.is_none() && cur.pipe_ctx[ju].prev_odm_pipe.is_none()
                && res_ctx.pipe_ctx[ju].stream.is_none()
            {
                secondary_pipe = Some(ju);
                res_ctx.pipe_ctx[ju].pipe_idx = ju as u8;
                break;
            }
            j -= 1;
        }
    }
    // search backwards for the second pipe to keep pipe
    // assignment more consistent
    if secondary_pipe.is_none() {
        let mut j = pipe_count - 1;
        while j >= 0 {
            let ju = j as usize;
            if res_ctx.pipe_ctx[ju].stream.is_none() {
                secondary_pipe = Some(ju);
                res_ctx.pipe_ctx[ju].pipe_idx = ju as u8;
                break;
            }
            j -= 1;
        }
    }

    secondary_pipe
}

/// `dcn20_split_stream_for_mpc`: the secondary pipe becomes a copy of the
/// primary on its own HUBP/DPP, blended below it.
pub fn dcn20_split_stream_for_mpc(res_ctx: &mut crate::dc_state::resource_context,
                                  pool: &crate::dc_resource::resource_pool, primary_pipe: usize,
                                  secondary_pipe: usize) {
    let pipe_idx = res_ctx.pipe_ctx[secondary_pipe].pipe_idx as usize;
    let sec_bot_pipe = res_ctx.pipe_ctx[secondary_pipe].bottom_pipe;

    res_ctx.pipe_ctx[secondary_pipe] = res_ctx.pipe_ctx[primary_pipe];
    let s = &mut res_ctx.pipe_ctx[secondary_pipe];
    s.bottom_pipe = sec_bot_pipe;

    s.pipe_idx = pipe_idx as u8;
    s.plane_res.hubp = pool.hubps[pipe_idx].as_ref().map(|_| pipe_idx);
    s.plane_res.ipp = pool.ipps[pipe_idx].as_ref().map(|_| pipe_idx);
    s.plane_res.dpp = pool.dpps[pipe_idx].as_ref().map(|_| pipe_idx);
    s.plane_res.mpcc_inst = pool.dpps[pipe_idx].as_deref().unwrap().base.inst as u8;
    // stream_res.dsc = NULL: DCN 2.1 pipes here hold no DSC
    if let Some(pb) = res_ctx.pipe_ctx[primary_pipe].bottom_pipe.filter(|&b| b != secondary_pipe) {
        res_ctx.pipe_ctx[secondary_pipe].bottom_pipe = Some(pb);
        res_ctx.pipe_ctx[pb].top_pipe = Some(secondary_pipe);
    }
    res_ctx.pipe_ctx[primary_pipe].bottom_pipe = Some(secondary_pipe);
    res_ctx.pipe_ctx[secondary_pipe].top_pipe = Some(primary_pipe);
}

/// `dcn20_validate_dsc`: a DSC stream's configuration through its DSC
/// block (`dsc_validate_stream`), named, not ported.
pub fn dcn20_validate_dsc(dc: &dc, new_ctx: &dc_state) -> bool {
    for i in 0..dc.res_pool.as_deref().unwrap().pipe_count as usize {
        let pipe_ctx = &new_ctx.res_ctx.pipe_ctx[i];

        // Only need to validate top pipe
        let Some(s) = pipe_ctx.stream else { continue };
        if pipe_ctx.top_pipe.is_some() || pipe_ctx.prev_odm_pipe.is_some()
            || !dc.streams[s].as_deref().unwrap().timing.flags.DSC
        {
            continue;
        }

        crate::linux::Line::new().s("dcn20_validate_dsc: dsc_validate_stream not ported").say();
        return false;
    }
    true
}

/// `dcn20_set_mcif_arb_params`: the writeback arbitration of each enabled
/// writeback (`dcn20_fpu_set_wb_arb_params`), named, not ported.
pub fn dcn20_set_mcif_arb_params(dc: &dc, context: &dc_state,
                                 _pipes: &mut [crate::display_mode_structs::_vcs_dpi_display_e2e_pipe_params_st],
                                 _pipe_cnt: i32) {
    for i in 0..dc.res_pool.as_deref().unwrap().pipe_count as usize {
        let Some(s) = context.res_ctx.pipe_ctx[i].stream else { continue };
        for wb in dc.streams[s].as_deref().unwrap().writeback_info.iter() {
            if !wb.wb_enabled {
                continue;
            }
            crate::linux::Line::new().s("dcn20_set_mcif_arb_params: writeback not ported").say();
            return;
        }
    }
}
