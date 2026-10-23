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
