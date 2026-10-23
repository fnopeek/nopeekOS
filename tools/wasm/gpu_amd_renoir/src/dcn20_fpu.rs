//! display/dc/dml/dcn20/dcn20_fpu.c (Linux 6.18.55): the Renoir DML
//! bounding-box data. Linux keeps `dcn2_1_ip`/`dcn2_1_soc` as mutable globals
//! the resource construct patches (e.g. `max_num_dpp = num_pipes`); here they
//! are builder functions that return the base struct, which the caller then
//! patches the same way. Values transcribed 1:1 from the designated
//! initializers (unlisted fields stay at their zero default).
#![allow(dead_code)]

use crate::dml_structs::{
    _vcs_dpi_ip_params_st, _vcs_dpi_soc_bounding_box_st, _vcs_dpi_voltage_scaling_st,
};

/// `struct _vcs_dpi_ip_params_st dcn2_1_ip` (dcn20_fpu.c).
pub fn dcn2_1_ip() -> _vcs_dpi_ip_params_st {
    let mut p = _vcs_dpi_ip_params_st::default();
    p.odm_capable = 1;
    p.gpuvm_enable = true;
    p.hostvm_enable = true;
    p.gpuvm_max_page_table_levels = 1;
    p.hostvm_max_page_table_levels = 4;
    p.hostvm_cached_page_table_levels = 2;
    p.num_dsc = 3;
    p.rob_buffer_size_kbytes = 168;
    p.det_buffer_size_kbytes = 164;
    p.dpte_buffer_size_in_pte_reqs_luma = 44;
    p.dpte_buffer_size_in_pte_reqs_chroma = 42; // todo
    p.dpp_output_buffer_pixels = 2560;
    p.opp_output_buffer_lines = 1;
    p.pixel_chunk_size_kbytes = 8;
    p.pte_enable = 1;
    p.max_page_table_levels = 4;
    p.pte_chunk_size_kbytes = 2;
    p.meta_chunk_size_kbytes = 2;
    p.min_meta_chunk_size_bytes = 256;
    p.writeback_chunk_size_kbytes = 2;
    p.line_buffer_size_bits = 789504;
    p.is_line_buffer_bpp_fixed = 0;
    p.line_buffer_fixed_bpp = 0;
    p.dcc_supported = 1; // true
    p.max_line_buffer_lines = 12;
    p.writeback_luma_buffer_size_kbytes = 12;
    p.writeback_chroma_buffer_size_kbytes = 8;
    p.writeback_chroma_line_buffer_width_pixels = 4;
    p.writeback_max_hscl_ratio = 1.0;
    p.writeback_max_vscl_ratio = 1.0;
    p.writeback_min_hscl_ratio = 1.0;
    p.writeback_min_vscl_ratio = 1.0;
    p.writeback_max_hscl_taps = 12;
    p.writeback_max_vscl_taps = 12;
    p.writeback_line_buffer_luma_buffer_size = 0;
    p.writeback_line_buffer_chroma_buffer_size = 14643;
    p.cursor_buffer_size = 8;
    p.cursor_chunk_size = 2;
    p.max_num_otg = 4;
    p.max_num_dpp = 4;
    p.max_num_wb = 1;
    p.max_dchub_pscl_bw_pix_per_clk = 4;
    p.max_pscl_lb_bw_pix_per_clk = 2;
    p.max_lb_vscl_bw_pix_per_clk = 4;
    p.max_vscl_hscl_bw_pix_per_clk = 4;
    p.max_hscl_ratio = 4.0;
    p.max_vscl_ratio = 4.0;
    p.hscl_mults = 4;
    p.vscl_mults = 4;
    p.max_hscl_taps = 8;
    p.max_vscl_taps = 8;
    p.dispclk_ramp_margin_percent = 1.0;
    p.underscan_factor = 1.10;
    p.min_vblank_lines = 32;
    p.dppclk_delay_subtotal = 77;
    p.dppclk_delay_scl_lb_only = 16;
    p.dppclk_delay_scl = 50;
    p.dppclk_delay_cnvc_formatter = 8;
    p.dppclk_delay_cnvc_cursor = 6;
    p.dispclk_delay_subtotal = 87;
    p.dcfclk_cstate_latency = 10.0; // SRExitTime
    p.max_inter_dcn_tile_repeaters = 8;
    p.xfc_supported = 0; // false
    p.xfc_fill_bw_overhead_percent = 10.0;
    p.xfc_fill_constant_bytes = 0;
    p.ptoi_supported = 0;
    p.number_of_cursors = 1;
    p
}

/// One clock-limit state of `dcn2_1_soc` (the fields the data sets; the rest
/// stay zero).
fn cl(state: i32, dcfclk_mhz: f64, fabricclk_mhz: f64, dispclk_mhz: f64,
      dppclk_mhz: f64, phyclk_mhz: f64, socclk_mhz: f64, dscclk_mhz: f64,
      dram_speed_mts: f64) -> _vcs_dpi_voltage_scaling_st {
    _vcs_dpi_voltage_scaling_st {
        state, dcfclk_mhz, fabricclk_mhz, dispclk_mhz, dppclk_mhz, phyclk_mhz,
        socclk_mhz, dscclk_mhz, dram_speed_mts, ..Default::default()
    }
}

/// `struct _vcs_dpi_soc_bounding_box_st dcn2_1_soc` (dcn20_fpu.c).
pub fn dcn2_1_soc() -> _vcs_dpi_soc_bounding_box_st {
    let mut clock_limits =
        core::array::from_fn(|_| _vcs_dpi_voltage_scaling_st::default());
    //        state  dcfclk   fabric   disp     dpp      phy     soc     dsc      dram
    clock_limits[0] = cl(0, 400.00,  400.0,   600.0,   400.00,  600.0,  278.0, 205.67,  1600.0);
    clock_limits[1] = cl(1, 464.52,  800.0,   654.55,  626.09,  600.0,  278.0, 205.67,  1600.0);
    clock_limits[2] = cl(2, 514.29,  933.0,   757.89,  685.71,  600.0,  278.0, 287.67,  1866.0);
    clock_limits[3] = cl(3, 576.00, 1067.0,   847.06,  757.89,  600.0,  715.0, 318.334, 2134.0);
    clock_limits[4] = cl(4, 626.09, 1200.0,   900.00,  847.06,  810.0,  953.0, 300.0,   2400.0);
    clock_limits[5] = cl(5, 685.71, 1333.0,  1028.57,  960.00,  810.0,  278.0, 342.86,  2666.0);
    clock_limits[6] = cl(6, 757.89, 1467.0,  1107.69, 1028.57,  810.0,  715.0, 369.23,  3200.0);
    clock_limits[7] = cl(7, 847.06, 1600.0,  1395.0,  1285.00, 1325.0,  953.0, 489.0,   4266.0);
    // Extra state, no dispclk ramping.
    clock_limits[8] = cl(8, 847.06, 1600.0,  1395.0,  1285.0,  1325.0,  953.0, 489.0,   4266.0);

    _vcs_dpi_soc_bounding_box_st {
        clock_limits,
        sr_exit_time_us: 12.5,
        sr_enter_plus_exit_time_us: 17.0,
        urgent_latency_us: 4.0,
        urgent_latency_pixel_data_only_us: 4.0,
        urgent_latency_pixel_mixed_with_vm_data_us: 4.0,
        urgent_latency_vm_data_only_us: 4.0,
        urgent_out_of_order_return_per_channel_pixel_only_bytes: 4096,
        urgent_out_of_order_return_per_channel_pixel_and_vm_bytes: 4096,
        urgent_out_of_order_return_per_channel_vm_only_bytes: 4096,
        pct_ideal_dram_sdp_bw_after_urgent_pixel_only: 80.0,
        pct_ideal_dram_sdp_bw_after_urgent_pixel_and_vm: 75.0,
        pct_ideal_dram_sdp_bw_after_urgent_vm_only: 40.0,
        max_avg_sdp_bw_use_normal_percent: 60.0,
        max_avg_dram_bw_use_normal_percent: 100.0,
        writeback_latency_us: 12.0,
        max_request_size_bytes: 256,
        dram_channel_width_bytes: 4.0,
        fabric_datapath_to_dcn_data_return_bytes: 32.0,
        dcn_downspread_percent: 0.5,
        downspread_percent: 0.38,
        dram_page_open_time_ns: 50.0,
        dram_rw_turnaround_time_ns: 17.5,
        dram_return_buffer_per_channel_bytes: 8192.0,
        round_trip_ping_latency_dcfclk_cycles: 128,
        urgent_out_of_order_return_per_channel_bytes: 4096,
        channel_interleave_bytes: 256,
        num_banks: 8,
        num_chans: 4,
        vmm_page_size_bytes: 4096,
        dram_clock_change_latency_us: 23.84,
        return_bus_width_bytes: 64,
        dispclk_dppclk_vco_speed_mhz: 3600.0,
        xfc_bus_transport_time_us: 4.0,
        xfc_xbuf_latency_tolerance_us: 4.0,
        use_urgent_burst_bw: 1,
        num_states: 8,
        ..Default::default()
    }
}

/// `construct_low_pstate_lvl` (dcn20_fpu.c): the min-DF-pstate level, from
/// the lowest clock table entry and `soc.clock_limits[high_voltage_lvl]`.
/// It also inserts a copy of entry 0 at entry 1 of the clock table.
fn construct_low_pstate_lvl(soc: &_vcs_dpi_soc_bounding_box_st, clk_table: &mut crate::clk_mgr::clk_limit_table,
                            high_voltage_lvl: usize) -> _vcs_dpi_voltage_scaling_st {
    let mut low = _vcs_dpi_voltage_scaling_st::default();
    let e0 = clk_table.entries[0];
    let hv = &soc.clock_limits[high_voltage_lvl];

    low.state = 1;
    low.dcfclk_mhz = e0.dcfclk_mhz as f64;
    low.fabricclk_mhz = e0.fclk_mhz as f64;
    low.socclk_mhz = e0.socclk_mhz as f64;
    low.dram_speed_mts = (e0.memclk_mhz * 2) as f64;

    low.dispclk_mhz = hv.dispclk_mhz;
    low.dppclk_mhz = hv.dppclk_mhz;
    low.dram_bw_per_chan_gbps = hv.dram_bw_per_chan_gbps;
    low.dscclk_mhz = hv.dscclk_mhz;
    low.dtbclk_mhz = hv.dtbclk_mhz;
    low.phyclk_d18_mhz = hv.phyclk_d18_mhz;
    low.phyclk_mhz = hv.phyclk_mhz;

    let mut i = clk_table.num_entries as usize;
    while i > 1 {
        clk_table.entries[i] = clk_table.entries[i - 1];
        i -= 1;
    }
    clk_table.entries[1] = clk_table.entries[0];
    clk_table.num_entries += 1;

    low
}

/// `dcn21_update_bw_bounding_box` (dcn20_fpu.c): the SoC clock levels from
/// the clock manager's table, then the DML instance again. Linux edits the
/// `dcn2_1_ip`/`dcn2_1_soc` globals in place; the port carries them in
/// `dc.dml`, which holds exactly those inputs since `dml_init_instance`.
/// The `closest_clk_lvl` of the last entry is the one the low-pstate level
/// reads, out of the clock limits already rewritten — as in Linux.
pub fn dcn21_update_bw_bounding_box(dc: &mut crate::dc::dc) {
    let (Some(pool), Some(dml), Some(clk_mgr)) = (dc.res_pool.as_deref(), dc.dml.as_deref(), dc.clk_mgr.as_deref_mut())
    else {
        return;
    };
    let Some(bw_params) = clk_mgr.base.bw_params.as_deref_mut() else { return };
    let mut ip = dml.ip;
    let mut soc = dml.soc;
    let clk_table = &mut bw_params.clk_table;

    ip.max_num_otg = pool.res_cap.map_or(0, |c| c.num_timing_generator as u32);
    ip.max_num_dpp = pool.pipe_count;
    soc.num_chans = bw_params.num_channels;

    // a copy of the clock limits, so null states are not copied over later
    let mut s = soc.clock_limits;
    let mut closest_clk_lvl = 0usize;
    let mut k = 0usize;

    for i in 0..clk_table.num_entries as usize {
        // loop backwards
        closest_clk_lvl = 0;
        let mut j = soc.num_states as i32 - 1;
        while j >= 0 {
            if soc.clock_limits[j as usize].dcfclk_mhz as u32 <= clk_table.entries[i].dcfclk_mhz {
                closest_clk_lvl = j as usize;
                break;
            }
            j -= 1;
        }

        // clk_table[1] is reserved for min DF PState, filled in below
        if i == 1 {
            k += 1;
        }

        let e = clk_table.entries[i];
        let c = soc.clock_limits[closest_clk_lvl];
        s[k].state = k as i32;
        s[k].dcfclk_mhz = e.dcfclk_mhz as f64;
        s[k].fabricclk_mhz = e.fclk_mhz as f64;
        s[k].socclk_mhz = e.socclk_mhz as f64;
        s[k].dram_speed_mts = (e.memclk_mhz * 2) as f64;

        s[k].dispclk_mhz = c.dispclk_mhz;
        s[k].dppclk_mhz = c.dppclk_mhz;
        s[k].dram_bw_per_chan_gbps = c.dram_bw_per_chan_gbps;
        s[k].dscclk_mhz = c.dscclk_mhz;
        s[k].dtbclk_mhz = c.dtbclk_mhz;
        s[k].phyclk_d18_mhz = c.phyclk_d18_mhz;
        s[k].phyclk_mhz = c.phyclk_mhz;

        k += 1;
    }

    soc.clock_limits = s;

    if clk_table.num_entries != 0 {
        soc.num_states = clk_table.num_entries + 1;
        // fill in min DF PState
        soc.clock_limits[1] = construct_low_pstate_lvl(&soc, clk_table, closest_clk_lvl);
        // duplicate last level
        let n = soc.num_states as usize;
        soc.clock_limits[n] = soc.clock_limits[n - 1];
        soc.clock_limits[n].state = n as i32;
    }

    let dml = dc.dml.get_or_insert_with(crate::dml_structs::DmlBox::zero);
    crate::dml_structs::dml_init_instance(dml, &soc, &ip, crate::dml_structs::DML_PROJECT_DCN21);
}

// Bandwidth validation: DC's state handed to DML as pipes, DML's answers
// written back as clocks, watermarks and per-pipe DLG/TTU/RQ programming.

use crate::dc::dc;
use crate::dc_state::dc_state;
use crate::display_mode_structs as dml;
use crate::display_mode_structs::{_vcs_dpi_display_e2e_pipe_params_st, display_mode_lib};
use crate::dml_math;
use crate::regs::*;

/// `swizzle_mode_to_macro_tile_size` (dcn_calcs.c).
pub fn swizzle_mode_to_macro_tile_size(sw_mode: u32) -> u32 {
    match sw_mode {
        // for 4/8/16 high tiles
        DC_SW_LINEAR => dml::dm_4k_tile,
        DC_SW_4KB_S | DC_SW_4KB_S_X => dml::dm_4k_tile,
        DC_SW_64KB_S | DC_SW_64KB_S_X | DC_SW_64KB_S_T => dml::dm_64k_tile,
        DC_SW_VAR_S | DC_SW_VAR_S_X => dml::dm_256k_tile,
        // For 64bpp 2 high tiles
        DC_SW_4KB_D | DC_SW_4KB_D_X => dml::dm_4k_tile,
        DC_SW_64KB_D | DC_SW_64KB_D_X | DC_SW_64KB_D_T => dml::dm_64k_tile,
        DC_SW_VAR_D | DC_SW_VAR_D_X => dml::dm_256k_tile,
        DC_SW_4KB_R | DC_SW_4KB_R_X => dml::dm_4k_tile,
        DC_SW_64KB_R | DC_SW_64KB_R_X => dml::dm_64k_tile,
        DC_SW_VAR_R | DC_SW_VAR_R_X => dml::dm_256k_tile,
        // Unsupported swizzle modes for dcn
        _ => 0,
    }
}

/// `swizzle_to_dml_params`.
fn swizzle_to_dml_params(swizzle: u32, sw_mode: &mut i32) {
    let m = match swizzle {
        DC_SW_LINEAR => dml::dm_sw_linear,
        DC_SW_4KB_S => dml::dm_sw_4kb_s,
        DC_SW_4KB_S_X => dml::dm_sw_4kb_s_x,
        DC_SW_4KB_D => dml::dm_sw_4kb_d,
        DC_SW_4KB_D_X => dml::dm_sw_4kb_d_x,
        DC_SW_64KB_S => dml::dm_sw_64kb_s,
        DC_SW_64KB_S_X => dml::dm_sw_64kb_s_x,
        DC_SW_64KB_S_T => dml::dm_sw_64kb_s_t,
        DC_SW_64KB_D => dml::dm_sw_64kb_d,
        DC_SW_64KB_D_X => dml::dm_sw_64kb_d_x,
        DC_SW_64KB_D_T => dml::dm_sw_64kb_d_t,
        DC_SW_64KB_R_X => dml::dm_sw_64kb_r_x,
        DC_SW_VAR_S => dml::dm_sw_var_s,
        DC_SW_VAR_S_X => dml::dm_sw_var_s_x,
        DC_SW_VAR_D => dml::dm_sw_var_d,
        DC_SW_VAR_D_X => dml::dm_sw_var_d_x,
        DC_SW_VAR_R_X => dml::dm_sw_var_r_x,
        // Not supported
        _ => return,
    };
    *sw_mode = m as i32;
}

/// `dcn20_populate_dml_writeback_from_context`.
pub fn dcn20_populate_dml_writeback_from_context(dc: &dc, context: &dc_state,
                                                 pipes: &mut [_vcs_dpi_display_e2e_pipe_params_st]) {
    let mut pipe_cnt = 0;
    for i in 0..dc.res_pool.as_deref().unwrap().pipe_count as usize {
        let Some(s) = context.res_ctx.pipe_ctx[i].stream else { continue };
        let wb_info = &dc.streams[s].as_deref().unwrap().writeback_info[0];
        let p = &mut pipes[pipe_cnt];

        // Set writeback information
        p.dout.wb_enable = if wb_info.wb_enabled { 1 } else { 0 };
        p.dout.num_active_wb += 1;
        p.dout.wb.wb_src_height = wb_info.crop_height as i32;
        p.dout.wb.wb_src_width = wb_info.crop_width as i32;
        p.dout.wb.wb_dst_width = wb_info.dest_width as i32;
        p.dout.wb.wb_dst_height = wb_info.dest_height as i32;
        p.dout.wb.wb_htaps_luma = 1;
        p.dout.wb.wb_vtaps_luma = 1;
        p.dout.wb.wb_htaps_chroma = wb_info.h_taps_c as i32;
        p.dout.wb.wb_vtaps_chroma = wb_info.v_taps_c as i32;
        p.dout.wb.wb_hratio = 1.0;
        p.dout.wb.wb_vratio = 1.0;
        if wb_info.out_format == dwb_scaler_mode_yuv420 {
            if wb_info.output_depth == DWB_OUTPUT_PIXEL_DEPTH_8BPC {
                p.dout.wb.wb_pixel_format = dml::dm_420_8 as i32;
            } else {
                p.dout.wb.wb_pixel_format = dml::dm_420_10 as i32;
            }
        } else {
            p.dout.wb.wb_pixel_format = dml::dm_444_32 as i32;
        }

        pipe_cnt += 1;
    }
}

/// `dcn20_populate_dml_pipes_from_context`. A dynamic-metadata stream and
/// a 128b/132b DP output (`dp_is_128b_132b_signal`: DCN 2.1 has no HPO
/// encoder) are read as Linux reads them.
pub fn dcn20_populate_dml_pipes_from_context(dc: &dc, context: &dc_state,
                                             pipes: &mut [_vcs_dpi_display_e2e_pipe_params_st],
                                             _validate_mode: u32) -> i32 {
    use crate::dc_resource::{resource_are_streams_timing_synchronizable, resource_are_vblanks_synchronizable,
        resource_get_odm_slice_count};
    let pool = dc.res_pool.as_deref().unwrap();
    let res_ctx = &context.res_ctx;
    let stream_of = |i: usize| dc.streams[res_ctx.pipe_ctx[i].stream.unwrap()].as_deref().unwrap();
    let mut synchronized_vblank = true;

    let mut pipe_cnt: i32 = -1;
    for i in 0..pool.pipe_count as usize {
        if res_ctx.pipe_ctx[i].stream.is_none() {
            continue;
        }

        if pipe_cnt < 0 {
            pipe_cnt = i as i32;
            continue;
        }

        if res_ctx.pipe_ctx[pipe_cnt as usize].stream == res_ctx.pipe_ctx[i].stream {
            continue;
        }

        if dc.debug.disable_timing_sync
            || (!resource_are_streams_timing_synchronizable(dc, stream_of(pipe_cnt as usize), stream_of(i))
                && !resource_are_vblanks_synchronizable(dc, stream_of(pipe_cnt as usize), stream_of(i)))
        {
            synchronized_vblank = false;
            break;
        }
    }

    let mut pipe_cnt = 0usize;
    for i in 0..pool.pipe_count as usize {
        let pc = &res_ctx.pipe_ctx[i];
        if pc.stream.is_none() {
            continue;
        }
        let stream = stream_of(i);
        let timing = &stream.timing;

        let v_total = timing.v_total;
        let front_porch = timing.v_front_porch;
        let output_bpc: i32;
        let mut aud_check = crate::dc_resource::audio_check::default();
        let p = &mut pipes[pipe_cnt];

        p.clks_cfg.refclk_mhz = pool.ref_clocks.dchub_ref_clock_inKhz as f64 / 1000.0;

        p.pipe.dest.use_maximum_vstartup = (dc.ctx.dce_version == crate::dc_types::dce_version::DCN_VERSION_2_01) as u8;

        p.dout.dsc_enable = stream.timing.flags.DSC as i32;
        // todo: rotation?
        p.dout.dsc_slices = stream.timing.dsc_cfg.num_slices_h as i32;
        if stream.use_dynamic_meta {
            p.pipe.src.dynamic_metadata_enable = 1;
            // 1/2 vblank
            p.pipe.src.dynamic_metadata_lines_before_active =
                (v_total - timing.v_addressable - timing.v_border_top - timing.v_border_bottom) / 2;
            // 36 bytes dp, 32 hdmi
            p.pipe.src.dynamic_metadata_xmit_bytes =
                if crate::link_detection::dc_is_dp_signal(stream.signal) { 36 } else { 32 };
        }
        p.pipe.src.dcc = 0;
        p.pipe.src.dcc_rate = 1;
        p.pipe.dest.synchronized_vblank_all_planes = synchronized_vblank as u8;
        p.pipe.dest.synchronize_timings = synchronized_vblank;
        p.pipe.dest.hblank_start = timing.h_total.wrapping_sub(timing.h_front_porch);
        p.pipe.dest.hblank_end = p.pipe.dest.hblank_start
            .wrapping_sub(timing.h_addressable)
            .wrapping_sub(timing.h_border_left)
            .wrapping_sub(timing.h_border_right);
        p.pipe.dest.vblank_start = v_total.wrapping_sub(front_porch);
        p.pipe.dest.vblank_end = p.pipe.dest.vblank_start
            .wrapping_sub(timing.v_addressable)
            .wrapping_sub(timing.v_border_top)
            .wrapping_sub(timing.v_border_bottom);
        p.pipe.dest.htotal = timing.h_total;
        p.pipe.dest.vtotal = v_total;
        p.pipe.dest.hactive = timing.h_addressable + timing.h_border_left + timing.h_border_right;
        p.pipe.dest.vactive = timing.v_addressable + timing.v_border_top + timing.v_border_bottom;
        p.pipe.dest.interlaced = timing.flags.INTERLACE as u8;
        p.pipe.dest.pixel_rate_mhz = timing.pix_clk_100hz as f64 / 10000.0;
        if timing.timing_3d_format == TIMING_3D_FORMAT_HW_FRAME_PACKING {
            p.pipe.dest.pixel_rate_mhz *= 2.0;
        }
        p.pipe.dest.otg_inst = pool.timing_generators[pc.stream_res.tg.unwrap()].as_deref().unwrap().base.inst as u8;
        p.dout.dp_lanes = 4;
        p.dout.dp_rate = dml::dm_dp_rate_na as i32;
        p.dout.is_virtual = 0;
        p.pipe.dest.vtotal_min = stream.adjust.v_total_min;
        p.pipe.dest.vtotal_max = stream.adjust.v_total_max;
        p.pipe.dest.odm_combine = match resource_get_odm_slice_count(res_ctx, i) {
            2 => dml::dm_odm_combine_mode_2to1,
            4 => dml::dm_odm_combine_mode_4to1,
            _ => dml::dm_odm_combine_mode_disabled,
        };
        p.pipe.src.hsplit_grp = pc.pipe_idx as u32;
        if let Some(top) = pc.top_pipe.filter(|&t| res_ctx.pipe_ctx[t].plane_state == pc.plane_state) {
            let mut first_pipe = top;
            let mut split_idx = 0;

            while let Some(t) = res_ctx.pipe_ctx[first_pipe].top_pipe {
                if res_ctx.pipe_ctx[t].plane_state != pc.plane_state {
                    break;
                }
                first_pipe = t;
                split_idx += 1;
            }
            // Treat 4to1 mpc combine as an mpo of 2 2-to-1 combines
            if split_idx == 0 {
                p.pipe.src.hsplit_grp = res_ctx.pipe_ctx[first_pipe].pipe_idx as u32;
            } else if split_idx == 1 {
                p.pipe.src.hsplit_grp = pc.pipe_idx as u32;
            } else if split_idx == 2 {
                p.pipe.src.hsplit_grp = res_ctx.pipe_ctx[top].pipe_idx as u32;
            }
        } else if let Some(prev) = pc.prev_odm_pipe {
            let mut first_pipe = prev;

            while let Some(pp) = res_ctx.pipe_ctx[first_pipe].prev_odm_pipe {
                first_pipe = pp;
            }
            p.pipe.src.hsplit_grp = res_ctx.pipe_ctx[first_pipe].pipe_idx as u32;
        }

        match stream.signal {
            LF_SIGNAL_TYPE_DISPLAY_PORT_MST | LF_SIGNAL_TYPE_DISPLAY_PORT => {
                p.dout.output_type = dml::dm_dp as i32;
                if crate::link_dp_capability::dp_is_128b_132b_signal(dc, context, i) {
                    p.dout.output_type = dml::dm_dp2p0 as i32;
                }
            }
            LF_SIGNAL_TYPE_EDP => p.dout.output_type = dml::dm_edp as i32,
            LF_SIGNAL_TYPE_HDMI_TYPE_A | LF_SIGNAL_TYPE_DVI_SINGLE_LINK | LF_SIGNAL_TYPE_DVI_DUAL_LINK => {
                p.dout.output_type = dml::dm_hdmi as i32;
            }
            _ => {
                // In case there is no signal, set dp with 4 lanes to allow max config
                p.dout.is_virtual = 1;
                p.dout.output_type = dml::dm_dp as i32;
                p.dout.dp_lanes = 4;
            }
        }

        output_bpc = match stream.timing.display_color_depth {
            COLOR_DEPTH_666 => 6,
            COLOR_DEPTH_888 => 8,
            COLOR_DEPTH_101010 => 10,
            COLOR_DEPTH_121212 => 12,
            COLOR_DEPTH_141414 => 14,
            COLOR_DEPTH_161616 => 16,
            COLOR_DEPTH_999 => 9,
            COLOR_DEPTH_111111 => 11,
            _ => 8,
        };

        match stream.timing.pixel_encoding {
            PIXEL_ENCODING_RGB | PIXEL_ENCODING_YCBCR444 => {
                p.dout.output_format = dml::dm_444 as i32;
                p.dout.output_bpp = (output_bpc * 3) as f64;
            }
            PIXEL_ENCODING_YCBCR420 => {
                p.dout.output_format = dml::dm_420 as i32;
                p.dout.output_bpp = (output_bpc as f64 * 3.0) / 2.0;
            }
            PIXEL_ENCODING_YCBCR422 => {
                if stream.timing.flags.DSC && !stream.timing.dsc_cfg.ycbcr422_simple {
                    p.dout.output_format = dml::dm_n422 as i32;
                } else {
                    p.dout.output_format = dml::dm_s422 as i32;
                }
                p.dout.output_bpp = (output_bpc * 2) as f64;
            }
            _ => {
                p.dout.output_format = dml::dm_444 as i32;
                p.dout.output_bpp = (output_bpc * 3) as f64;
            }
        }

        if stream.timing.flags.DSC {
            p.dout.output_bpp = stream.timing.dsc_cfg.bits_per_pixel as f64 / 16.0;
        }

        // todo: default max for now, until there is logic reflecting this in dc
        p.dout.dsc_input_bpc = 12;
        // fill up the audio sample rate (unit in kHz)
        crate::dc_resource::get_audio_check(&stream.audio_info, &mut aud_check);
        p.dout.max_audio_sample_rate = (aud_check.max_audiosample_rate / 1000) as i32;
        // For graphic plane, cursor number is 1, nv12 is 0
        // bw calculations due to cursor on/off
        let plane = pc.plane_state.map(|pl| dc.planes[pl].as_deref().unwrap());
        if plane.is_some_and(|pln| pln.address.r#type == PLN_ADDR_TYPE_VIDEO_PROGRESSIVE)
            || (plane.is_some() && crate::dc_state::dc_state_get_pipe_subvp_type(context, pc) == SUBVP_PHANTOM)
        {
            p.pipe.src.num_cursors = 0;
        } else {
            p.pipe.src.num_cursors = dc.dml.as_deref().unwrap().ip.number_of_cursors as i32;
        }

        p.pipe.src.cur0_src_width = 256;
        p.pipe.src.cur0_bpp = dml::dm_cur_32bit as i32;

        match plane {
            None => {
                p.pipe.src.is_hsplit = (p.pipe.dest.odm_combine != dml::dm_odm_combine_mode_disabled) as u8;
                p.pipe.src.source_scan = dml::dm_horz as i32;
                p.pipe.src.source_rotation = dml::dm_rotation_0 as i32;
                p.pipe.src.sw_mode = dml::dm_sw_4kb_s as i32;
                p.pipe.src.macro_tile_size = dml::dm_64k_tile as i32;
                p.pipe.src.viewport_width = timing.h_addressable;
                if p.pipe.src.viewport_width > 1920 {
                    p.pipe.src.viewport_width = 1920;
                }
                p.pipe.src.viewport_height = timing.v_addressable;
                if p.pipe.src.viewport_height > 1080 {
                    p.pipe.src.viewport_height = 1080;
                }
                p.pipe.src.surface_height_y = p.pipe.src.viewport_height;
                p.pipe.src.surface_width_y = p.pipe.src.viewport_width;
                p.pipe.src.surface_height_c = p.pipe.src.viewport_height;
                p.pipe.src.surface_width_c = p.pipe.src.viewport_width;
                p.pipe.src.data_pitch = ((p.pipe.src.viewport_width + 255) / 256) * 256;
                p.pipe.src.source_format = dml::dm_444_32 as i32;
                p.pipe.src.cur0_src_width = 0;
                p.pipe.src.cur1_src_width = 0;
                p.pipe.dest.recout_width = p.pipe.src.viewport_width; // vp_width/hratio
                p.pipe.dest.recout_height = p.pipe.src.viewport_height; // vp_height/vratio
                p.pipe.dest.full_recout_width = p.pipe.dest.recout_width; // when is_hsplit != 1
                p.pipe.dest.full_recout_height = p.pipe.dest.recout_height; // when is_hsplit != 1
                p.pipe.scale_ratio_depth.lb_depth = dml::dm_lb_16 as i32;
                p.pipe.scale_ratio_depth.hscl_ratio = 1.0;
                p.pipe.scale_ratio_depth.vscl_ratio = 1.0;
                p.pipe.scale_ratio_depth.scl_enable = 0; // Lb only or Full scl
                p.pipe.scale_taps.htaps = 1;
                p.pipe.scale_taps.vtaps = 1;
                p.pipe.dest.vtotal_min = v_total;
                p.pipe.dest.vtotal_max = v_total;

                if p.pipe.dest.odm_combine == dml::dm_odm_combine_mode_2to1 {
                    p.pipe.src.viewport_width /= 2;
                    p.pipe.dest.recout_width /= 2;
                } else if p.pipe.dest.odm_combine == dml::dm_odm_combine_mode_4to1 {
                    p.pipe.src.viewport_width /= 4;
                    p.pipe.dest.recout_width /= 4;
                }
            }
            Some(pln) => {
                let scl = &pc.plane_res.scl_data;
                let pl = pc.plane_state;

                p.pipe.src.immediate_flip = pln.flip_immediate as u8;
                p.pipe.src.is_hsplit = (pc.bottom_pipe.is_some_and(|b| res_ctx.pipe_ctx[b].plane_state == pl)
                    || pc.top_pipe.is_some_and(|t| res_ctx.pipe_ctx[t].plane_state == pl)
                    || p.pipe.dest.odm_combine != dml::dm_odm_combine_mode_disabled) as u8;

                // stereo is not split
                if pln.stereo_format == PLANE_STEREO_FORMAT_SIDE_BY_SIDE
                    || pln.stereo_format == PLANE_STEREO_FORMAT_TOP_AND_BOTTOM
                {
                    p.pipe.src.is_hsplit = 0;
                    p.pipe.src.hsplit_grp = pc.pipe_idx as u32;
                }

                p.pipe.src.source_scan = if pln.rotation == ROTATION_ANGLE_90 || pln.rotation == ROTATION_ANGLE_270 {
                    dml::dm_vert as i32
                } else {
                    dml::dm_horz as i32
                };
                match pln.rotation {
                    ROTATION_ANGLE_0 => p.pipe.src.source_rotation = dml::dm_rotation_0 as i32,
                    ROTATION_ANGLE_90 => p.pipe.src.source_rotation = dml::dm_rotation_90 as i32,
                    ROTATION_ANGLE_180 => p.pipe.src.source_rotation = dml::dm_rotation_180 as i32,
                    ROTATION_ANGLE_270 => p.pipe.src.source_rotation = dml::dm_rotation_270 as i32,
                    _ => {}
                }

                p.pipe.src.viewport_y_y = scl.viewport.y as u32;
                p.pipe.src.viewport_y_c = scl.viewport_c.y as u32;
                p.pipe.src.viewport_x_y = scl.viewport.x as u32;
                p.pipe.src.viewport_x_c = scl.viewport_c.x as u32;
                p.pipe.src.viewport_width = scl.viewport.width as u32;
                p.pipe.src.viewport_width_c = scl.viewport_c.width as u32;
                p.pipe.src.viewport_height = scl.viewport.height as u32;
                p.pipe.src.viewport_height_c = scl.viewport_c.height as u32;
                p.pipe.src.viewport_width_max = pln.src_rect.width as u32;
                p.pipe.src.viewport_height_max = pln.src_rect.height as u32;
                p.pipe.src.surface_width_y = pln.plane_size.surface_size.width as u32;
                p.pipe.src.surface_height_y = pln.plane_size.surface_size.height as u32;
                p.pipe.src.surface_width_c = pln.plane_size.chroma_size.width as u32;
                p.pipe.src.surface_height_c = pln.plane_size.chroma_size.height as u32;
                if pln.format == SURFACE_PIXEL_FORMAT_GRPH_RGBE_ALPHA || pln.format >= SURFACE_PIXEL_FORMAT_VIDEO_BEGIN {
                    p.pipe.src.data_pitch = pln.plane_size.surface_pitch as u32;
                    p.pipe.src.data_pitch_c = pln.plane_size.chroma_pitch as u32;
                    p.pipe.src.meta_pitch = pln.dcc.meta_pitch as u32;
                    p.pipe.src.meta_pitch_c = pln.dcc.meta_pitch_c as u32;
                } else {
                    p.pipe.src.data_pitch = pln.plane_size.surface_pitch as u32;
                    p.pipe.src.meta_pitch = pln.dcc.meta_pitch as u32;
                }
                p.pipe.src.dcc = pln.dcc.enable as u8;
                p.pipe.dest.recout_width = scl.recout.width as u32;
                p.pipe.dest.recout_height = scl.recout.height as u32;
                p.pipe.dest.full_recout_height = scl.recout.height as u32;
                p.pipe.dest.full_recout_width = scl.recout.width as u32;
                if p.pipe.dest.odm_combine == dml::dm_odm_combine_mode_2to1 {
                    p.pipe.dest.full_recout_width *= 2;
                } else if p.pipe.dest.odm_combine == dml::dm_odm_combine_mode_4to1 {
                    p.pipe.dest.full_recout_width *= 4;
                } else {
                    let mut split_pipe = pc.bottom_pipe;
                    while let Some(sp) = split_pipe.filter(|&s| res_ctx.pipe_ctx[s].plane_state == pl) {
                        p.pipe.dest.full_recout_width = p.pipe.dest.full_recout_width
                            .wrapping_add(res_ctx.pipe_ctx[sp].plane_res.scl_data.recout.width as u32);
                        split_pipe = res_ctx.pipe_ctx[sp].bottom_pipe;
                    }
                    split_pipe = pc.top_pipe;
                    while let Some(sp) = split_pipe.filter(|&s| res_ctx.pipe_ctx[s].plane_state == pl) {
                        p.pipe.dest.full_recout_width = p.pipe.dest.full_recout_width
                            .wrapping_add(res_ctx.pipe_ctx[sp].plane_res.scl_data.recout.width as u32);
                        split_pipe = res_ctx.pipe_ctx[sp].top_pipe;
                    }
                }

                let one = crate::fixpt31_32::dc_fixpt_one.value;
                p.pipe.scale_ratio_depth.lb_depth = dml::dm_lb_16 as i32;
                p.pipe.scale_ratio_depth.hscl_ratio = scl.ratios.horz.value as f64 / (1u64 << 32) as f64;
                p.pipe.scale_ratio_depth.hscl_ratio_c = scl.ratios.horz_c.value as f64 / (1u64 << 32) as f64;
                p.pipe.scale_ratio_depth.vscl_ratio = scl.ratios.vert.value as f64 / (1u64 << 32) as f64;
                p.pipe.scale_ratio_depth.vscl_ratio_c = scl.ratios.vert_c.value as f64 / (1u64 << 32) as f64;
                p.pipe.scale_ratio_depth.scl_enable = (scl.ratios.vert.value != one
                    || scl.ratios.horz.value != one
                    || scl.ratios.vert_c.value != one
                    || scl.ratios.horz_c.value != one // Lb only or Full scl
                    || dc.debug.always_scale) as i32; // support always scale
                p.pipe.scale_taps.htaps = scl.taps.h_taps;
                p.pipe.scale_taps.htaps_c = scl.taps.h_taps_c;
                p.pipe.scale_taps.vtaps = scl.taps.v_taps;
                p.pipe.scale_taps.vtaps_c = scl.taps.v_taps_c;

                p.pipe.src.macro_tile_size = swizzle_mode_to_macro_tile_size(pln.tiling_info.swizzle) as i32;
                swizzle_to_dml_params(pln.tiling_info.swizzle, &mut p.pipe.src.sw_mode);

                p.pipe.src.source_format = match pln.format {
                    SURFACE_PIXEL_FORMAT_VIDEO_420_YCbCr | SURFACE_PIXEL_FORMAT_VIDEO_420_YCrCb => dml::dm_420_8,
                    SURFACE_PIXEL_FORMAT_VIDEO_420_10bpc_YCbCr | SURFACE_PIXEL_FORMAT_VIDEO_420_10bpc_YCrCb => {
                        dml::dm_420_10
                    }
                    SURFACE_PIXEL_FORMAT_GRPH_ARGB16161616
                    | SURFACE_PIXEL_FORMAT_GRPH_ABGR16161616
                    | SURFACE_PIXEL_FORMAT_GRPH_ARGB16161616F
                    | SURFACE_PIXEL_FORMAT_GRPH_ABGR16161616F => dml::dm_444_64,
                    SURFACE_PIXEL_FORMAT_GRPH_ARGB1555 | SURFACE_PIXEL_FORMAT_GRPH_RGB565 => dml::dm_444_16,
                    SURFACE_PIXEL_FORMAT_GRPH_PALETA_256_COLORS => dml::dm_444_8,
                    SURFACE_PIXEL_FORMAT_GRPH_RGBE_ALPHA => dml::dm_rgbe_alpha,
                    _ => dml::dm_444_32,
                } as i32;
            }
        }

        pipe_cnt += 1;
    }

    // populate writeback information
    dcn20_populate_dml_writeback_from_context(dc, context, pipes);

    pipe_cnt as i32
}

/// `dcn21_populate_dml_pipes_from_context`, DCN 2.1's `populate_dml_pipes`.
pub fn dcn21_populate_dml_pipes_from_context(dc: &dc, context: &dc_state,
                                             pipes: &mut [_vcs_dpi_display_e2e_pipe_params_st],
                                             validate_mode: u32) -> i32 {
    let pipe_cnt = dcn20_populate_dml_pipes_from_context(dc, context, pipes, validate_mode);

    let riommu_active = dc.res_pool.as_deref().unwrap().hubbub.as_deref().unwrap().base.riommu_active;
    for p in pipes.iter_mut().take(pipe_cnt as usize) {
        p.pipe.src.hostvm = riommu_active;
        p.pipe.src.gpuvm = true;
    }

    pipe_cnt
}

/// `dcn20_fpu_adjust_dppclk`.
pub fn dcn20_fpu_adjust_dppclk(v: &mut dml::vba_vars_st, vlevel: i32, max_mpc_comb: i32, pipe_idx: i32,
                               is_validating_bw: bool) {
    let c = &mut v.RequiredDPPCLK[vlevel as usize][max_mpc_comb as usize][pipe_idx as usize];
    if is_validating_bw {
        *c *= 2.0;
    } else {
        *c /= 2.0;
    }
}

/// `patch_bounding_box`.
fn patch_bounding_box(dc: &mut dc, bb: &mut dml::_vcs_dpi_soc_bounding_box_st) {
    let o = dc.bb_overrides;
    let bw = dc.clk_mgr.as_deref_mut().unwrap().base.bw_params.as_deref_mut().unwrap();
    if o.sr_exit_time_ns != 0 {
        for e in bw.wm_table.entries.iter_mut() {
            e.sr_exit_time_us = o.sr_exit_time_ns as f64 / 1000.0;
        }
    }

    if o.sr_enter_plus_exit_time_ns != 0 {
        for e in bw.wm_table.entries.iter_mut() {
            e.sr_enter_plus_exit_time_us = o.sr_enter_plus_exit_time_ns as f64 / 1000.0;
        }
    }

    if o.urgent_latency_ns != 0 {
        bb.urgent_latency_us = o.urgent_latency_ns as f64 / 1000.0;
    }

    if o.dram_clock_change_latency_ns != 0 {
        for e in bw.wm_table.entries.iter_mut() {
            e.pstate_latency_us = o.dram_clock_change_latency_ns as f64 / 1000.0;
        }
    }
}

/// `calculate_wm_set_for_vlevel` (dcn301_fpu.c).
fn calculate_wm_set_for_vlevel(vlevel: i32, table_entry: &crate::clk_mgr::wm_range_table_entry,
                               wm_set: &mut crate::dc_state::dcn_watermarks, dml: &mut display_mode_lib,
                               pipes: &mut [_vcs_dpi_display_e2e_pipe_params_st], pipe_cnt: i32) {
    use crate::display_mode_vba::*;
    let dram_clock_change_latency_cached = dml.soc.dram_clock_change_latency_us;
    let n = pipe_cnt as u32;

    // only pipe 0 is read for voltage and dcf/soc clocks
    pipes[0].clks_cfg.voltage = vlevel;
    pipes[0].clks_cfg.dcfclk_mhz = dml.soc.clock_limits[vlevel as usize].dcfclk_mhz;
    pipes[0].clks_cfg.socclk_mhz = dml.soc.clock_limits[vlevel as usize].socclk_mhz;

    dml.soc.dram_clock_change_latency_us = table_entry.pstate_latency_us;
    dml.soc.sr_exit_time_us = table_entry.sr_exit_time_us;
    dml.soc.sr_enter_plus_exit_time_us = table_entry.sr_enter_plus_exit_time_us;

    wm_set.urgent_ns = dml_math::f2u32(get_wm_urgent(dml, pipes, n) * 1000.0);
    wm_set.cstate_pstate.cstate_enter_plus_exit_ns = dml_math::f2u32(get_wm_stutter_enter_exit(dml, pipes, n) * 1000.0);
    wm_set.cstate_pstate.cstate_exit_ns = dml_math::f2u32(get_wm_stutter_exit(dml, pipes, n) * 1000.0);
    wm_set.cstate_pstate.pstate_change_ns = dml_math::f2u32(get_wm_dram_clock_change(dml, pipes, n) * 1000.0);
    wm_set.pte_meta_urgent_ns = dml_math::f2u32(get_wm_memory_trip(dml, pipes, n) * 1000.0);
    wm_set.frac_urg_bw_nom = dml_math::f2u32(get_fraction_of_urgent_bandwidth(dml, pipes, n) * 1000.0);
    wm_set.frac_urg_bw_flip = dml_math::f2u32(get_fraction_of_urgent_bandwidth_imm_flip(dml, pipes, n) * 1000.0);
    wm_set.urgent_latency_ns = dml_math::f2u32(get_urgent_latency(dml, pipes, n) * 1000.0);
    dml.soc.dram_clock_change_latency_us = dram_clock_change_latency_cached;
}

/// `dcn21_calculate_wm`.
fn dcn21_calculate_wm(dc: &mut dc, context: &mut dc_state, pipes: &mut [_vcs_dpi_display_e2e_pipe_params_st],
                      out_pipe_cnt: &mut i32, pipe_split_from: &[i32], vlevel_req: i32, validate_mode: u32) {
    let mut dml_box = context.dml.take().unwrap();
    patch_bounding_box(dc, &mut dml_box.soc);
    context.dml = Some(dml_box);

    let pool_pipes = dc.res_pool.as_deref().unwrap().pipe_count as usize;
    let refclk = dc.res_pool.as_deref().unwrap().ref_clocks.dchub_ref_clock_inKhz;
    let (mut pipe_cnt, mut pipe_idx) = (0usize, 0usize);
    {
        let v = &context.dml.as_deref().unwrap().vba;
        for i in 0..pool_pipes {
            if context.res_ctx.pipe_ctx[i].stream.is_none() {
                continue;
            }

            let p = &mut pipes[pipe_cnt];
            let mpc = v.maxMpcComb as usize;
            let vl = vlevel_req as usize;
            p.clks_cfg.refclk_mhz = refclk as f64 / 1000.0;
            p.clks_cfg.dispclk_mhz = v.RequiredDISPCLK[vl][mpc];

            if pipe_split_from[i] < 0 {
                p.clks_cfg.dppclk_mhz = v.RequiredDPPCLK[vl][mpc][pipe_idx];
                if v.BlendingAndTiming[pipe_idx] == pipe_idx as u32 {
                    p.pipe.dest.odm_combine = v.ODMCombineEnablePerState[vl][pipe_idx];
                } else {
                    p.pipe.dest.odm_combine = 0;
                }
                pipe_idx += 1;
            } else {
                let f = pipe_split_from[i] as usize;
                p.clks_cfg.dppclk_mhz = v.RequiredDPPCLK[vl][mpc][f];
                if v.BlendingAndTiming[f] == f as u32 {
                    p.pipe.dest.odm_combine = v.ODMCombineEnablePerState[vl][f];
                } else {
                    p.pipe.dest.odm_combine = 0;
                }
            }
            pipe_cnt += 1;
        }
    }

    let mut pipe_cnt = pipe_cnt as i32;
    if pipe_cnt != pipe_idx as i32 {
        pipe_cnt = dcn21_populate_dml_pipes_from_context(dc, context, pipes, validate_mode);
    }

    *out_pipe_cnt = pipe_cnt;

    let bw_params = *dc.clk_mgr.as_deref().unwrap().base.bw_params.as_deref().unwrap();
    let vlevel_max = bw_params.clk_table.num_entries as i32 - 1;
    let dml = context.dml.as_deref_mut().unwrap();
    let wms = &mut context.bw_ctx.dcn.watermarks;

    // WM Set D
    let table_entry = &bw_params.wm_table.entries[crate::clk_mgr::WM_D as usize];
    let vlevel = if table_entry.wm_type == crate::clk_mgr::WM_TYPE_RETRAINING { 0 } else { vlevel_max };
    calculate_wm_set_for_vlevel(vlevel, table_entry, &mut wms.d, dml, pipes, pipe_cnt);
    // WM Set C
    let table_entry = &bw_params.wm_table.entries[crate::clk_mgr::WM_C as usize];
    let vlevel = vlevel_req.max(3).min(vlevel_max);
    calculate_wm_set_for_vlevel(vlevel, table_entry, &mut wms.c, dml, pipes, pipe_cnt);
    // WM Set B
    let table_entry = &bw_params.wm_table.entries[crate::clk_mgr::WM_B as usize];
    let vlevel = vlevel_req.max(2).min(vlevel_max);
    calculate_wm_set_for_vlevel(vlevel, table_entry, &mut wms.b, dml, pipes, pipe_cnt);

    // WM Set A
    let table_entry = &bw_params.wm_table.entries[crate::clk_mgr::WM_A as usize];
    let vlevel = vlevel_req.min(vlevel_max);
    calculate_wm_set_for_vlevel(vlevel, table_entry, &mut wms.a, dml, pipes, pipe_cnt);
}

/// `is_dtbclk_required` (dcn32_fpu.c).
fn is_dtbclk_required(dc: &dc, context: &dc_state) -> bool {
    for i in 0..dc.res_pool.as_deref().unwrap().pipe_count as usize {
        if context.res_ctx.pipe_ctx[i].stream.is_none() {
            continue;
        }
        if crate::link_dp_capability::dp_is_128b_132b_signal(dc, context, i) {
            return true;
        }
    }
    false
}

/// `decide_zstate_support`.
fn decide_zstate_support(dc: &dc, context: &dc_state) -> u32 {
    let mut plane_count = 0;
    for i in 0..dc.res_pool.as_deref().unwrap().pipe_count as usize {
        if context.res_ctx.pipe_ctx[i].plane_state.is_some() {
            plane_count += 1;
        }
    }

    // Z9 and Z10 allowed cases:
    //	1. 0 Planes enabled
    //	2. single eDP, on link 0, 1 plane and stutter period > 5ms
    // Z10 only cases:
    //	1. single eDP, on link 0, 1 plane and stutter period >= 5ms
    // Z8 cases:
    //	1. stutter period sufficient
    // Zstate not allowed cases:
    //	1. Everything else
    if plane_count == 0 {
        return DCN_ZSTATE_SUPPORT_ALLOW;
    }
    let stream0 = context.streams[0].map(|s| dc.streams[s].as_deref().unwrap());
    if context.stream_count == 1 && stream0.unwrap().signal == LF_SIGNAL_TYPE_EDP {
        let link = &dc.links[stream0.unwrap().link];
        let stream_status = &context.stream_status[0];
        let vba = &context.dml.as_deref().unwrap().vba;
        let minmum_z8_residency =
            if dc.debug.minimum_z8_residency_time > 0 { dc.debug.minimum_z8_residency_time as i32 } else { 1000 };
        let allow_z8 = vba.StutterPeriod > minmum_z8_residency as f64;
        let is_pwrseq0 = link.link_index == 0;
        let is_psr = (link.psr_settings.psr_version == DC_PSR_VERSION_1
            || link.psr_settings.psr_version == DC_PSR_VERSION_SU_1)
            && !link.panel_config.psr_disable_psr;
        let is_replay = false; // replay_settings.replay_feature_enabled: no Replay on DCN 2.1

        // Don't support multi-plane configurations
        if stream_status.plane_count > 1 {
            return DCN_ZSTATE_SUPPORT_DISALLOW;
        }

        if is_pwrseq0 && vba.StutterPeriod > 5000.0 {
            DCN_ZSTATE_SUPPORT_ALLOW
        } else if is_pwrseq0 && (is_psr || is_replay) {
            DCN_ZSTATE_SUPPORT_ALLOW_Z8_Z10_ONLY
        } else if allow_z8 {
            DCN_ZSTATE_SUPPORT_ALLOW_Z8_ONLY
        } else {
            DCN_ZSTATE_SUPPORT_DISALLOW
        }
    } else {
        DCN_ZSTATE_SUPPORT_DISALLOW
    }
}

/// `dcn20_calculate_dlg_params`. A FreeSync stream would move vstartup
/// (`dcn20_adjust_freesync_v_startup`), named, not ported.
pub fn dcn20_calculate_dlg_params(dc: &dc, context: &mut dc_state, pipes: &mut [_vcs_dpi_display_e2e_pipe_params_st],
                                  pipe_cnt: i32, vlevel: i32) {
    use crate::display_mode_vba::{get_vready_offset, get_vstartup, get_vupdate_offset, get_vupdate_width};
    let pool_pipes = dc.res_pool.as_deref().unwrap().pipe_count as usize;
    let mut active_hubp_count = 0;
    let n = pipe_cnt as u32;

    // Writeback MCIF_WB arbitration parameters
    crate::dcn20_resource::dcn20_set_mcif_arb_params(dc, context, pipes, pipe_cnt);

    let is_dtb = is_dtbclk_required(dc, context);
    {
        let vba = &context.dml.as_deref().unwrap().vba;
        let clk = &mut context.bw_ctx.dcn.clk;
        clk.dispclk_khz = dml_math::f2i32(vba.DISPCLK * 1000.0);
        clk.dcfclk_khz = dml_math::f2i32(vba.DCFCLK * 1000.0);
        clk.socclk_khz = dml_math::f2i32(vba.SOCCLK * 1000.0);
        clk.dramclk_khz = dml_math::f2i32(vba.DRAMSpeed * 1000.0 / 16.0);

        if dc.debug.min_dram_clk_khz > clk.dramclk_khz {
            clk.dramclk_khz = dc.debug.min_dram_clk_khz;
        }

        clk.dcfclk_deep_sleep_khz = dml_math::f2i32(vba.DCFCLKDeepSleep * 1000.0);
        clk.fclk_khz = dml_math::f2i32(vba.FabricClock * 1000.0);
        clk.p_state_change_support = vba.DRAMClockChangeSupport[vlevel as usize][vba.maxMpcComb as usize]
            != dml::dm_dram_clock_change_unsupported;

        // Pstate change might not be supported by hardware, but it might be
        // possible with firmware driven vertical blank stretching.
        clk.p_state_change_support |= clk.fw_based_mclk_switching;

        clk.dppclk_khz = 0;

        clk.dtbclk_en = is_dtb;

        if clk.dispclk_khz < dc.debug.min_disp_clk_khz {
            clk.dispclk_khz = dc.debug.min_disp_clk_khz;
        }
    }

    let mut pipe_idx = 0usize;
    for i in 0..pool_pipes {
        if context.res_ctx.pipe_ctx[i].stream.is_none() {
            continue;
        }
        if context.res_ctx.pipe_ctx[i].plane_state.is_some() {
            active_hubp_count += 1;
        }
        let dml = context.dml.as_deref_mut().unwrap();
        let w = pipe_idx as u32;
        pipes[pipe_idx].pipe.dest.vstartup_start = dml_math::f2u32(get_vstartup(dml, pipes, n, w));
        pipes[pipe_idx].pipe.dest.vupdate_offset = dml_math::f2u32(get_vupdate_offset(dml, pipes, n, w));
        pipes[pipe_idx].pipe.dest.vupdate_width = dml_math::f2u32(get_vupdate_width(dml, pipes, n, w));
        pipes[pipe_idx].pipe.dest.vready_offset = dml_math::f2u32(get_vready_offset(dml, pipes, n, w));

        let det = dml.ip.det_buffer_size_kbytes;
        let subvp = crate::dc_state::dc_state_get_pipe_subvp_type(context, &context.res_ctx.pipe_ctx[i]);
        let pc = &mut context.res_ctx.pipe_ctx[i];
        if subvp == SUBVP_PHANTOM {
            // Phantom pipe requires that DET_SIZE = 0 and no unbounded requests
            pc.det_buffer_size_kb = 0;
            pc.unbounded_req = false;
        } else {
            pc.det_buffer_size_kb = det as i32;
            pc.unbounded_req = pipes[pipe_idx].pipe.src.unbounded_req_mode;
        }

        let clk = &mut context.bw_ctx.dcn.clk;
        if (clk.dppclk_khz as f64) < pipes[pipe_idx].clks_cfg.dppclk_mhz * 1000.0 {
            clk.dppclk_khz = dml_math::f2i32(pipes[pipe_idx].clks_cfg.dppclk_mhz * 1000.0);
        }
        pc.plane_res.bw_dppclk_khz = dml_math::f2i32(pipes[pipe_idx].clks_cfg.dppclk_mhz * 1000.0);
        pc.pipe_dlg_param = pipes[pipe_idx].pipe.dest;
        if dc.streams[pc.stream.unwrap()].as_deref().unwrap().adaptive_sync_infopacket_valid {
            crate::linux::Line::new().s("dcn20_adjust_freesync_v_startup not ported").say();
        }

        pipe_idx += 1;
    }
    let dml = context.dml.as_deref_mut().unwrap();
    let clk = &mut context.bw_ctx.dcn.clk;
    // If DCN isn't making memory requests we can allow pstate change
    if active_hubp_count == 0 {
        clk.p_state_change_support = true;
    }
    // save a original dppclock copy
    clk.bw_dppclk_khz = clk.dppclk_khz;
    clk.bw_dispclk_khz = clk.dispclk_khz;
    clk.max_supported_dppclk_khz = dml_math::f2i32(dml.soc.clock_limits[vlevel as usize].dppclk_mhz * 1000.0);
    clk.max_supported_dispclk_khz = dml_math::f2i32(dml.soc.clock_limits[vlevel as usize].dispclk_mhz * 1000.0);

    context.bw_ctx.dcn.compbuf_size_kb = (dml.ip.config_return_buffer_size_in_kbytes as i32)
        .wrapping_sub((dml.ip.det_buffer_size_kbytes as i32).wrapping_mul(pipe_idx as i32));

    let p_state = clk.p_state_change_support;
    let mut pipe_idx = 0u32;
    for i in 0..pool_pipes {
        let mut cstate_en =
            dml.vba.PrefetchMode[vlevel as usize][dml.vba.maxMpcComb as usize] != 2;

        if context.res_ctx.pipe_ctx[i].stream.is_none() {
            continue;
        }

        // cstate disabled on 201
        if dc.ctx.dce_version == crate::dc_types::dce_version::DCN_VERSION_2_01 {
            cstate_en = false;
        }

        let pc = &mut context.res_ctx.pipe_ctx[i];
        let get_dlg = dml.funcs.rq_dlg_get_dlg_reg.unwrap();
        get_dlg(dml, &mut pc.dlg_regs, &mut pc.ttu_regs, pipes, n, pipe_idx, cstate_en, p_state, false, false, true);

        let get_rq = dml.funcs.rq_dlg_get_rq_reg.unwrap();
        get_rq(dml, &mut pc.rq_regs, &pipes[pipe_idx as usize].pipe);
        pipe_idx += 1;
    }
    context.bw_ctx.dcn.clk.zstate_support = decide_zstate_support(dc, context);
}

/// `dcn21_validate_bandwidth_fp`.
pub fn dcn21_validate_bandwidth_fp(dc: &mut dc, context: &mut dc_state, validate_mode: u32,
                                   pipes: &mut [_vcs_dpi_display_e2e_pipe_params_st]) -> bool {
    let mut vlevel = 0;
    let mut pipe_split_from = [0i32; crate::dc_types::MAX_PIPES];
    let mut pipe_cnt = 0;

    let mut out = crate::dcn21_resource::dcn21_fast_validate_bw(dc, context, pipes, &mut pipe_cnt,
        &mut pipe_split_from, &mut vlevel, validate_mode);

    if pipe_cnt == 0 {
        return out;
    }

    if !out {
        crate::linux::Line::new().s("Mode Validation Warning: failed validation.").say();
        out = false;
        return out;
    }

    if validate_mode != DC_VALIDATE_MODE_AND_PROGRAMMING {
        return out;
    }

    dcn21_calculate_wm(dc, context, pipes, &mut pipe_cnt, &pipe_split_from, vlevel, validate_mode);
    dcn20_calculate_dlg_params(dc, context, pipes, pipe_cnt, vlevel);

    out
}
