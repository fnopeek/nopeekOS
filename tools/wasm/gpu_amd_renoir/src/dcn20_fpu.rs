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

    dc.dml = Some(alloc::boxed::Box::new(crate::dml_structs::dml_init_instance(
        &soc,
        &ip,
        crate::dml_structs::dml_project::DML_PROJECT_DCN21,
    )));
}
