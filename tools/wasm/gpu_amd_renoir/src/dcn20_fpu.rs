//! display/dc/dml/dcn20/dcn20_fpu.c (Linux 6.18.55): the Renoir DML
//! bounding-box data. Linux keeps `dcn2_1_ip`/`dcn2_1_soc` as mutable globals
//! the resource construct patches (e.g. `max_num_dpp = num_pipes`); here they
//! are builder functions that return the base struct, which the caller then
//! patches the same way. Values transcribed 1:1 from the designated
//! initializers (unlisted fields stay at their zero default).
#![allow(dead_code)]

use crate::dml_structs::_vcs_dpi_ip_params_st;

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
