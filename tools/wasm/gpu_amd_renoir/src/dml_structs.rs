//! display/dc/dml/display_mode_structs.h (Linux 6.18.55): the DML (Display
//! Mode Library) parameter structs — the per-ASIC IP and SoC bounding-box
//! inputs the bandwidth/watermark math reads. `double` → f64, `unsigned int`
//! → u32, `unsigned char` → u8; the field names are Linux' verbatim (so the
//! Renoir data in dcn20_fpu.rs transcribes 1:1).
#![allow(dead_code, non_snake_case)]

/// `struct _vcs_dpi_ip_params_st`: DCN IP parameters.
#[derive(Default, Clone, Copy)]
pub struct _vcs_dpi_ip_params_st {
    pub use_min_dcfclk: bool,
    pub clamp_min_dcfclk: bool,
    pub gpuvm_enable: bool,
    pub hostvm_enable: bool,
    pub dsc422_native_support: bool,
    pub gpuvm_max_page_table_levels: u32,
    pub hostvm_max_page_table_levels: u32,
    pub hostvm_cached_page_table_levels: u32,
    pub pte_group_size_bytes: u32,
    pub max_inter_dcn_tile_repeaters: u32,
    pub num_dsc: u32,
    pub odm_capable: u32,
    pub rob_buffer_size_kbytes: u32,
    pub det_buffer_size_kbytes: u32,
    pub min_comp_buffer_size_kbytes: u32,
    pub dpte_buffer_size_in_pte_reqs_luma: u32,
    pub dpte_buffer_size_in_pte_reqs_chroma: u32,
    pub pde_proc_buffer_size_64k_reqs: u32,
    pub dpp_output_buffer_pixels: u32,
    pub opp_output_buffer_lines: u32,
    pub pixel_chunk_size_kbytes: u32,
    pub alpha_pixel_chunk_size_kbytes: u32,
    pub min_pixel_chunk_size_bytes: u32,
    pub dcc_meta_buffer_size_bytes: u32,
    pub pte_enable: u8,
    pub pte_chunk_size_kbytes: u32,
    pub meta_chunk_size_kbytes: u32,
    pub min_meta_chunk_size_bytes: u32,
    pub writeback_chunk_size_kbytes: u32,
    pub line_buffer_size_bits: u32,
    pub max_line_buffer_lines: u32,
    pub writeback_luma_buffer_size_kbytes: u32,
    pub writeback_chroma_buffer_size_kbytes: u32,
    pub writeback_chroma_line_buffer_width_pixels: u32,
    pub writeback_interface_buffer_size_kbytes: u32,
    pub writeback_line_buffer_buffer_size: u32,
    pub writeback_10bpc420_supported: u32,
    pub writeback_max_hscl_ratio: f64,
    pub writeback_max_vscl_ratio: f64,
    pub writeback_min_hscl_ratio: f64,
    pub writeback_min_vscl_ratio: f64,
    pub maximum_dsc_bits_per_component: u32,
    pub maximum_pixels_per_line_per_dsc_unit: u32,
    pub writeback_max_hscl_taps: u32,
    pub writeback_max_vscl_taps: u32,
    pub writeback_line_buffer_luma_buffer_size: u32,
    pub writeback_line_buffer_chroma_buffer_size: u32,
    pub max_page_table_levels: u32,
    pub max_num_dpp: u32,
    pub max_num_otg: u32,
    pub cursor_chunk_size: u32,
    pub cursor_buffer_size: u32,
    pub max_num_wb: u32,
    pub max_dchub_pscl_bw_pix_per_clk: u32,
    pub max_pscl_lb_bw_pix_per_clk: u32,
    pub max_lb_vscl_bw_pix_per_clk: u32,
    pub max_vscl_hscl_bw_pix_per_clk: u32,
    pub max_hscl_ratio: f64,
    pub max_vscl_ratio: f64,
    pub hscl_mults: u32,
    pub vscl_mults: u32,
    pub max_hscl_taps: u32,
    pub max_vscl_taps: u32,
    pub xfc_supported: u32,
    pub ptoi_supported: u32,
    pub gfx7_compat_tiling_supported: u32,
    pub odm_combine_4to1_supported: bool,
    pub dynamic_metadata_vm_enabled: bool,
    pub max_num_hdmi_frl_outputs: u32,
    pub xfc_fill_constant_bytes: u32,
    pub dispclk_ramp_margin_percent: f64,
    pub xfc_fill_bw_overhead_percent: f64,
    pub underscan_factor: f64,
    pub min_vblank_lines: u32,
    pub dppclk_delay_subtotal: u32,
    pub dispclk_delay_subtotal: u32,
    pub dcfclk_cstate_latency: f64,
    pub dppclk_delay_scl: u32,
    pub dppclk_delay_scl_lb_only: u32,
    pub dppclk_delay_cnvc_formatter: u32,
    pub dppclk_delay_cnvc_cursor: u32,
    pub is_line_buffer_bpp_fixed: u32,
    pub line_buffer_fixed_bpp: u32,
    pub dcc_supported: u32,
    pub config_return_buffer_size_in_kbytes: u32,
    pub compressed_buffer_segment_size_in_kbytes: u32,
    pub meta_fifo_size_in_kentries: u32,
    pub zero_size_buffer_entries: u32,
    pub compbuf_reserved_space_64b: u32,
    pub compbuf_reserved_space_zs: u32,
    pub IsLineBufferBppFixed: u32,
    pub LineBufferFixedBpp: u32,
    pub can_vstartup_lines_exceed_vsync_plus_back_porch_lines_minus_one: u32,
    pub bug_forcing_LC_req_same_size_fixed: u32,
    pub number_of_cursors: u32,
    pub max_num_dp2p0_outputs: u32,
    pub max_num_dp2p0_streams: u32,
    pub VBlankNomDefaultUS: u32,
    /// DM workaround (TODO upstream: remove after root-cause fix).
    pub dsc_delay_factor_wa: f64,
    pub min_prefetch_in_strobe_us: f64,
}

/// `#define DC__VOLTAGE_STATES` (dc_features.h).
pub const DC__VOLTAGE_STATES: usize = 40;

/// `enum dm_prefetch_modes` (display_mode_enums.h).
#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u32)]
pub enum dm_prefetch_modes {
    #[default]
    dm_prefetch_support_uclk_fclk_and_stutter_if_possible,
    dm_prefetch_support_uclk_fclk_and_stutter,
    dm_prefetch_support_fclk_and_stutter,
    dm_prefetch_support_stutter,
    dm_prefetch_support_none,
}

/// `enum self_refresh_affinity` (display_mode_enums.h).
#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u32)]
pub enum self_refresh_affinity {
    #[default]
    dm_try_to_allow_self_refresh_and_mclk_switch,
    dm_allow_self_refresh_and_mclk_switch,
    dm_allow_self_refresh,
    dm_neither_self_refresh_nor_mclk_switch,
}

/// `struct _vcs_dpi_voltage_scaling_st`: one DPM clock-limit state.
#[derive(Default, Clone, Copy)]
pub struct _vcs_dpi_voltage_scaling_st {
    pub state: i32,
    pub dscclk_mhz: f64,
    pub dcfclk_mhz: f64,
    pub socclk_mhz: f64,
    pub phyclk_d18_mhz: f64,
    pub phyclk_d32_mhz: f64,
    pub dram_speed_mts: f64,
    pub fabricclk_mhz: f64,
    pub dispclk_mhz: f64,
    pub dram_bw_per_chan_gbps: f64,
    pub phyclk_mhz: f64,
    pub dppclk_mhz: f64,
    pub dtbclk_mhz: f64,
    pub net_bw_in_kbytes_sec: f32,
}

/// `struct _vcs_dpi_soc_bounding_box_st`: DCN SoC parameters. `clock_limits`
/// is `[_; DC__VOLTAGE_STATES]` (N=40 > 32), so Default is written by hand with
/// `core::array::from_fn`.
#[derive(Clone, Copy)]
pub struct _vcs_dpi_soc_bounding_box_st {
    pub clock_limits: [_vcs_dpi_voltage_scaling_st; DC__VOLTAGE_STATES],
    pub num_states: u32,
    pub sr_exit_time_us: f64,
    pub sr_enter_plus_exit_time_us: f64,
    pub sr_exit_z8_time_us: f64,
    pub sr_enter_plus_exit_z8_time_us: f64,
    pub urgent_latency_us: f64,
    pub urgent_latency_pixel_data_only_us: f64,
    pub urgent_latency_pixel_mixed_with_vm_data_us: f64,
    pub urgent_latency_vm_data_only_us: f64,
    pub usr_retraining_latency_us: f64,
    pub smn_latency_us: f64,
    pub fclk_change_latency_us: f64,
    pub mall_allocated_for_dcn_mbytes: f64,
    pub pct_ideal_fabric_bw_after_urgent: f64,
    pub pct_ideal_dram_bw_after_urgent_strobe: f64,
    pub max_avg_fabric_bw_use_normal_percent: f64,
    pub max_avg_dram_bw_use_normal_strobe_percent: f64,
    pub allow_for_pstate_or_stutter_in_vblank_final: dm_prefetch_modes,
    pub dram_clock_change_requirement_final: bool,
    pub writeback_latency_us: f64,
    pub ideal_dram_bw_after_urgent_percent: f64,
    pub pct_ideal_dram_sdp_bw_after_urgent_pixel_only: f64,
    pub pct_ideal_dram_sdp_bw_after_urgent_pixel_and_vm: f64,
    pub pct_ideal_dram_sdp_bw_after_urgent_vm_only: f64,
    pub pct_ideal_sdp_bw_after_urgent: f64,
    pub max_avg_sdp_bw_use_normal_percent: f64,
    pub max_avg_dram_bw_use_normal_percent: f64,
    pub max_request_size_bytes: u32,
    pub downspread_percent: f64,
    pub dram_page_open_time_ns: f64,
    pub dram_rw_turnaround_time_ns: f64,
    pub dram_return_buffer_per_channel_bytes: f64,
    pub dram_channel_width_bytes: f64,
    pub fabric_datapath_to_dcn_data_return_bytes: f64,
    pub dcn_downspread_percent: f64,
    pub dispclk_dppclk_vco_speed_mhz: f64,
    pub dfs_vco_period_ps: f64,
    pub urgent_out_of_order_return_per_channel_pixel_only_bytes: u32,
    pub urgent_out_of_order_return_per_channel_pixel_and_vm_bytes: u32,
    pub urgent_out_of_order_return_per_channel_vm_only_bytes: u32,
    pub round_trip_ping_latency_dcfclk_cycles: u32,
    pub urgent_out_of_order_return_per_channel_bytes: u32,
    pub channel_interleave_bytes: u32,
    pub num_banks: u32,
    pub num_chans: u32,
    pub vmm_page_size_bytes: u32,
    pub hostvm_min_page_size_bytes: u32,
    pub gpuvm_min_page_size_bytes: u32,
    pub dram_clock_change_latency_us: f64,
    pub dummy_pstate_latency_us: f64,
    pub writeback_dram_clock_change_latency_us: f64,
    pub return_bus_width_bytes: u32,
    pub voltage_override: u32,
    pub xfc_bus_transport_time_us: f64,
    pub xfc_xbuf_latency_tolerance_us: f64,
    pub use_urgent_burst_bw: i32,
    pub min_dcfclk: f64,
    pub do_urgent_latency_adjustment: bool,
    pub urgent_latency_adjustment_fabric_clock_component_us: f64,
    pub urgent_latency_adjustment_fabric_clock_reference_mhz: f64,
    pub disable_dram_clock_change_vactive_support: bool,
    pub allow_dram_clock_one_display_vactive: bool,
    pub allow_dram_self_refresh_or_dram_clock_change_in_vblank: self_refresh_affinity,
    pub max_vratio_pre: f64,
}

impl Default for _vcs_dpi_soc_bounding_box_st {
    fn default() -> Self {
        Self {
            clock_limits: core::array::from_fn(|_| _vcs_dpi_voltage_scaling_st::default()),
            num_states: 0,
            sr_exit_time_us: 0.0,
            sr_enter_plus_exit_time_us: 0.0,
            sr_exit_z8_time_us: 0.0,
            sr_enter_plus_exit_z8_time_us: 0.0,
            urgent_latency_us: 0.0,
            urgent_latency_pixel_data_only_us: 0.0,
            urgent_latency_pixel_mixed_with_vm_data_us: 0.0,
            urgent_latency_vm_data_only_us: 0.0,
            usr_retraining_latency_us: 0.0,
            smn_latency_us: 0.0,
            fclk_change_latency_us: 0.0,
            mall_allocated_for_dcn_mbytes: 0.0,
            pct_ideal_fabric_bw_after_urgent: 0.0,
            pct_ideal_dram_bw_after_urgent_strobe: 0.0,
            max_avg_fabric_bw_use_normal_percent: 0.0,
            max_avg_dram_bw_use_normal_strobe_percent: 0.0,
            allow_for_pstate_or_stutter_in_vblank_final: dm_prefetch_modes::default(),
            dram_clock_change_requirement_final: false,
            writeback_latency_us: 0.0,
            ideal_dram_bw_after_urgent_percent: 0.0,
            pct_ideal_dram_sdp_bw_after_urgent_pixel_only: 0.0,
            pct_ideal_dram_sdp_bw_after_urgent_pixel_and_vm: 0.0,
            pct_ideal_dram_sdp_bw_after_urgent_vm_only: 0.0,
            pct_ideal_sdp_bw_after_urgent: 0.0,
            max_avg_sdp_bw_use_normal_percent: 0.0,
            max_avg_dram_bw_use_normal_percent: 0.0,
            max_request_size_bytes: 0,
            downspread_percent: 0.0,
            dram_page_open_time_ns: 0.0,
            dram_rw_turnaround_time_ns: 0.0,
            dram_return_buffer_per_channel_bytes: 0.0,
            dram_channel_width_bytes: 0.0,
            fabric_datapath_to_dcn_data_return_bytes: 0.0,
            dcn_downspread_percent: 0.0,
            dispclk_dppclk_vco_speed_mhz: 0.0,
            dfs_vco_period_ps: 0.0,
            urgent_out_of_order_return_per_channel_pixel_only_bytes: 0,
            urgent_out_of_order_return_per_channel_pixel_and_vm_bytes: 0,
            urgent_out_of_order_return_per_channel_vm_only_bytes: 0,
            round_trip_ping_latency_dcfclk_cycles: 0,
            urgent_out_of_order_return_per_channel_bytes: 0,
            channel_interleave_bytes: 0,
            num_banks: 0,
            num_chans: 0,
            vmm_page_size_bytes: 0,
            hostvm_min_page_size_bytes: 0,
            gpuvm_min_page_size_bytes: 0,
            dram_clock_change_latency_us: 0.0,
            dummy_pstate_latency_us: 0.0,
            writeback_dram_clock_change_latency_us: 0.0,
            return_bus_width_bytes: 0,
            voltage_override: 0,
            xfc_bus_transport_time_us: 0.0,
            xfc_xbuf_latency_tolerance_us: 0.0,
            use_urgent_burst_bw: 0,
            min_dcfclk: 0.0,
            do_urgent_latency_adjustment: false,
            urgent_latency_adjustment_fabric_clock_component_us: 0.0,
            urgent_latency_adjustment_fabric_clock_reference_mhz: 0.0,
            disable_dram_clock_change_vactive_support: false,
            allow_dram_clock_one_display_vactive: false,
            allow_dram_self_refresh_or_dram_clock_change_in_vblank: self_refresh_affinity::default(),
            max_vratio_pre: 0.0,
        }
    }
}

// ── the DML instance (display_mode_lib.{h,c}) ──────────────────────────────

/// `enum dml_project` (display_mode_lib.h): which ASIC's DML the instance runs.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum dml_project {
    #[default]
    DML_PROJECT_UNDEFINED,
    DML_PROJECT_RAVEN1,
    DML_PROJECT_NAVI10,
    DML_PROJECT_NAVI10v2,
    DML_PROJECT_DCN201,
    DML_PROJECT_DCN21,
    DML_PROJECT_DCN30,
    DML_PROJECT_DCN31,
    DML_PROJECT_DCN315,
    DML_PROJECT_DCN314,
    DML_PROJECT_DCN32,
}

/// Which `dml_funcs` vtable the instance points at. Renoir uses `dml21_funcs`;
/// the vtable bodies (the rq/dlg/ttu + mode-support math) are stage C.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum dml_funcs {
    dml21_funcs,
}

/// `struct display_mode_lib` (display_mode_lib.h): the DML instance. Ported
/// grow-as-reached — the inputs (`ip`/`soc`), the project and the funcs
/// identity; the `vba` working state and `dml_pipe_state` are the stage-C math.
#[derive(Clone, Copy)]
pub struct display_mode_lib {
    pub ip: _vcs_dpi_ip_params_st,
    pub soc: _vcs_dpi_soc_bounding_box_st,
    pub project: dml_project,
    pub funcs: Option<dml_funcs>,
}

/// `dml_init_instance` (display_mode_lib.c): copy the SoC/IP inputs into the
/// instance and bind the per-project DML vtable. Renoir → `dml21_funcs`.
pub fn dml_init_instance(
    soc_bb: &_vcs_dpi_soc_bounding_box_st,
    ip_params: &_vcs_dpi_ip_params_st,
    project: dml_project,
) -> display_mode_lib {
    let funcs = match project {
        dml_project::DML_PROJECT_DCN21 => Some(dml_funcs::dml21_funcs),
        // Other projects bind their own dmlNN_funcs; not built (Renoir = DCN21).
        _ => None,
    };
    display_mode_lib { ip: *ip_params, soc: *soc_bb, project, funcs }
}
