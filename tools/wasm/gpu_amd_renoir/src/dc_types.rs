//! display/dc/dc_types.h, dc/dc.h, include/dal_types.h (Linux 6.18.55): the
//! self-contained core types of Display Core — the ones `dc_create` touches
//! that do not pull in a sub-block (bios, link, dml). The nested caps structs
//! (dc_caps → dc_plane_cap / dc_color_caps / …), dc_context and struct dc come
//! with the object graph (docs/plan/GPU_AMD_DC.md stage A).
#![allow(dead_code)]

/// `enum dce_environment` (dc_types.h). Renoir boots `DCE_ENV_PRODUCTION_DRV`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u32)]
pub enum dce_environment {
    DCE_ENV_PRODUCTION_DRV = 0,
    /// Emulation on FPGA, "Maximus" — only DC registers may be touched.
    DCE_ENV_FPGA_MAXIMUS,
    /// Emulation on real HW or FPGA, for Diagnostics.
    DCE_ENV_DIAG,
    /// Guest VM: DC HW may exist but is not virtualized; SW support for VDI.
    DCE_ENV_VIRTUAL_HW,
}

/// `enum dce_version` (include/dal_types.h). `resource_parse_asic_id` maps
/// Renoir/Lucienne to `DCN_VERSION_2_1`. The whole ladder is kept so other
/// code can match on it; the discriminants follow the C order (UNKNOWN = -1).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(i32)]
pub enum dce_version {
    DCE_VERSION_UNKNOWN = -1,
    DCE_VERSION_6_0,
    DCE_VERSION_6_1,
    DCE_VERSION_6_4,
    DCE_VERSION_8_0,
    DCE_VERSION_8_1,
    DCE_VERSION_8_3,
    DCE_VERSION_10_0,
    DCE_VERSION_11_0,
    DCE_VERSION_11_2,
    DCE_VERSION_11_22,
    DCE_VERSION_12_0,
    DCE_VERSION_12_1,
    DCE_VERSION_MAX,
    DCN_VERSION_1_0,
    DCN_VERSION_1_01,
    DCN_VERSION_2_0,
    DCN_VERSION_2_01,
    DCN_VERSION_2_1,
    DCN_VERSION_3_0,
    DCN_VERSION_3_01,
    DCN_VERSION_3_02,
    DCN_VERSION_3_03,
    DCN_VERSION_3_1,
    DCN_VERSION_3_14,
    DCN_VERSION_3_15,
    DCN_VERSION_3_16,
    DCN_VERSION_3_2,
    DCN_VERSION_3_21,
    DCN_VERSION_3_5,
    DCN_VERSION_3_51,
    DCN_VERSION_3_6,
    DCN_VERSION_4_01,
    DCN_VERSION_MAX,
}

/// `enum dp_protocol_version` (dc.h). `dc_create` sets `max_dp_protocol_version
/// = DP_VERSION_1_4`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u32)]
pub enum dp_protocol_version {
    DP_VERSION_1_4 = 0,
    DP_VERSION_2_1,
    DP_VERSION_UNKNOWN,
}

/// `struct dmcu_version` (dc.h): DCN 2.1 has no DMCU, so this stays zero.
#[derive(Default, Clone, Copy)]
pub struct dmcu_version {
    pub interface_version: u32,
    pub abm_version: u32,
    pub psr_version: u32,
    pub build_version: u32,
}

/// `struct dc_versions` (dc.h).
#[derive(Default, Clone, Copy)]
pub struct dc_versions {
    /// `dc_ver` = `DC_VER`.
    pub dc_ver: &'static str,
    pub dmcu_version: dmcu_version,
}

/// `struct hw_asic_id` (dc_types.h): what `dc_init_data` / `dc_context` carry
/// about the chip. `atombios_base_address` is a pointer in Linux; here the
/// address as u64 (0 = none).
#[derive(Default, Clone, Copy)]
pub struct hw_asic_id {
    pub chip_id: u32,
    pub chip_family: u32,
    pub pci_revision_id: u32,
    pub hw_internal_rev: u32,
    pub vram_type: u32,
    pub vram_width: u32,
    pub feature_flags: u32,
    pub fake_paths_num: u32,
    pub atombios_base_address: u64,
}

/// `#define DC_VER` (dc.h).
pub const DC_VER: &str = "3.2.351";

/// `#define MAX_PLANES` / `MAX_SURFACES` (dc.h).
pub const MAX_PLANES: usize = 6;
pub const MAX_SURFACES: usize = 4;

/// `#define MAX_PIPES` (inc/hw/hw_shared.h): the DCN pipe arrays' bound.
pub const MAX_PIPES: usize = 6;

/// `struct dc_firmware_info` (include/grph_object_ctrl_defs.h).
#[derive(Default, Clone, Copy, Debug)]
pub struct dc_firmware_info {
    pub pll_info: pll_info,
    pub feature: firmware_feature,
    /// In KHz.
    pub default_display_engine_pll_frequency: u32,
    /// In KHz.
    pub external_clock_source_frequency_for_dp: u32,
    /// In KHz.
    pub smu_gpu_pll_output_freq: u32,
    pub min_allowed_bl_level: u8,
    pub remote_display_config: u8,
    /// In KHz.
    pub default_memory_clk: u32,
    /// In KHz.
    pub default_engine_clk: u32,
    /// In KHz - DCE12 only.
    pub dp_phy_ref_clk: u32,
    /// In KHz - DCE12 only.
    pub i2c_engine_ref_clk: u32,
    pub oem_i2c_present: bool,
    pub oem_i2c_obj_id: u8,
}

/// `struct pll_info` (the inline member of `dc_firmware_info`), in KHz.
#[derive(Default, Clone, Copy, Debug)]
pub struct pll_info {
    pub crystal_frequency: u32,
    pub min_input_pxl_clk_pll_frequency: u32,
    pub max_input_pxl_clk_pll_frequency: u32,
    pub min_output_pxl_clk_pll_frequency: u32,
    pub max_output_pxl_clk_pll_frequency: u32,
}

/// `struct firmware_feature` (the inline member of `dc_firmware_info`).
#[derive(Default, Clone, Copy, Debug)]
pub struct firmware_feature {
    pub memory_clk_ss_percentage: u32,
    pub engine_clk_ss_percentage: u32,
}

/// `struct dc_vram_info` (include/grph_object_ctrl_defs.h).
#[derive(Default, Clone, Copy, Debug)]
pub struct dc_vram_info {
    pub num_chans: u32,
    pub dram_channel_width_bytes: u32,
}

/// `struct bios_registers` (dc_bios_types.h).
#[derive(Default, Clone, Copy)]
pub struct bios_registers {
    pub BIOS_SCRATCH_3: u32,
    pub BIOS_SCRATCH_6: u32,
}

/// `struct dc_bios` (dc/dc_bios_types.h): the base of `bios_parser`. The
/// image pointer is not stored (see bios_parser2.rs); `bb_info` joins with a
/// DCN 3.x VBIOS.
#[derive(Default)]
pub struct dc_bios {
    pub bios_size: u32,
    pub regs: Option<&'static bios_registers>,
    pub integrated_info: Option<alloc::boxed::Box<integrated_info>>,
    pub fw_info: dc_firmware_info,
    pub fw_info_valid: bool,
    pub vram_info: dc_vram_info,
}

/// `enum object_type` (include/grph_object_id.h).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum object_type {
    #[default]
    OBJECT_TYPE_UNKNOWN = 0,
    OBJECT_TYPE_GPU,
    OBJECT_TYPE_ENCODER,
    OBJECT_TYPE_CONNECTOR,
    OBJECT_TYPE_ROUTER,
    OBJECT_TYPE_GENERIC,
    OBJECT_TYPE_AUDIO,
    OBJECT_TYPE_CONTROLLER,
    OBJECT_TYPE_CLOCK_SOURCE,
    OBJECT_TYPE_ENGINE,
}

/// `enum object_enum_id` (include/grph_object_id.h).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum object_enum_id {
    #[default]
    ENUM_ID_UNKNOWN = 0,
    ENUM_ID_1,
    ENUM_ID_2,
    ENUM_ID_3,
    ENUM_ID_4,
    ENUM_ID_5,
    ENUM_ID_6,
    ENUM_ID_7,
}

/// `struct graphics_object_id` (include/grph_object_id.h). Linux packs it
/// into a u32 bitfield (`id:8`, `enum_id:4`, `type:4`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct graphics_object_id {
    pub id: u32,
    pub enum_id: object_enum_id,
    pub r#type: object_type,
}

impl graphics_object_id {
    /// `dal_graphics_object_id_get_connector_id`: the id of a connector
    /// object, CONNECTOR_ID_UNKNOWN (0) for any other object.
    pub fn get_connector_id(&self) -> u32 {
        if self.r#type == object_type::OBJECT_TYPE_CONNECTOR { self.id } else { 0 }
    }

    /// `dal_graphics_object_id_get_encoder_id`: the id of an encoder object,
    /// ENCODER_ID_UNKNOWN (0) for any other object.
    pub fn get_encoder_id(&self) -> u32 {
        if self.r#type == object_type::OBJECT_TYPE_ENCODER { self.id } else { 0 }
    }

    /// `dal_graphics_object_id_init`.
    pub fn init(id: u32, enum_id: object_enum_id, r#type: object_type) -> Self {
        graphics_object_id { id: id & 0xff, enum_id, r#type }
    }
}

/// Sizes of `integrated_info` (include/grph_object_ctrl_defs.h).
pub const NUMBER_OF_UCHAR_FOR_GUID: usize = 16;
pub const MAX_NUMBER_OF_EXT_DISPLAY_PATH: usize = 7;
pub const NUMBER_OF_CSR_M3_ARB: usize = 10;
pub const NUMBER_OF_DISP_CLK_VOLTAGE: usize = 4;
pub const NUMBER_OF_AVAILABLE_SCLK: usize = 5;

/// `struct i2c_reg_info`.
#[derive(Default, Clone, Copy, Debug)]
pub struct i2c_reg_info {
    pub i2c_reg_index: u8,
    pub i2c_reg_val: u8,
}

/// `struct edp_info`.
#[derive(Default, Clone, Copy, Debug)]
pub struct edp_info {
    pub edp_backlight_pwm_hz: u16,
    pub edp_ss_percentage: u16,
    pub edp_ss_rate_10hz: u16,
    pub edp_pwr_on_off_delay: u8,
    pub edp_pwr_on_vary_bl_to_blon: u8,
    pub edp_pwr_down_bloff_to_vary_bloff: u8,
    pub edp_panel_bpc: u8,
    pub edp_bootup_bl_level: u8,
}

/// `struct clock_voltage_caps` (in `integrated_info`).
#[derive(Default, Clone, Copy, Debug)]
pub struct clock_voltage_caps {
    pub voltage_index: u32,
    /// In KHz.
    pub max_supported_clk: u32,
}

/// `struct external_display_path` (in `integrated_info`). `channel_mapping`
/// is `union ddi_channel_mapping` as its raw byte.
#[derive(Default, Clone, Copy, Debug)]
pub struct external_display_path {
    pub device_tag: u32,
    pub device_acpi_enum: u32,
    pub device_connector_id: graphics_object_id,
    pub ext_aux_ddc_lut_index: u8,
    pub ext_hpd_pin_lut_index: u8,
    pub ext_encoder_obj_id: graphics_object_id,
    pub channel_mapping: u8,
    pub caps: u16,
}

/// `struct display_connection_info` (in `integrated_info`).
#[derive(Default, Clone, Copy, Debug)]
pub struct display_connection_info {
    pub path: [external_display_path; MAX_NUMBER_OF_EXT_DISPLAY_PATH],
    pub gu_id: [u8; NUMBER_OF_UCHAR_FOR_GUID],
    pub checksum: u8,
    pub fixdpvoltageswing: u8,
}

/// `struct available_s_clk_list` (in `integrated_info`).
#[derive(Default, Clone, Copy, Debug)]
pub struct available_s_clk_list {
    /// In KHz.
    pub supported_s_clk: u32,
    pub voltage_index: u32,
    pub voltage_id: u32,
}

/// `struct integrated_info` (include/grph_object_ctrl_defs.h).
#[derive(Default, Clone, Debug)]
pub struct integrated_info {
    pub disp_clk_voltage: [clock_voltage_caps; NUMBER_OF_DISP_CLK_VOLTAGE],
    pub ext_disp_conn_info: display_connection_info,
    pub avail_s_clk: [available_s_clk_list; NUMBER_OF_AVAILABLE_SCLK],
    pub memory_type: u8,
    pub ma_channel_number: u8,
    pub boot_up_engine_clock: u32,
    pub dentist_vco_freq: u32,
    pub boot_up_uma_clock: u32,
    pub boot_up_req_display_vector: u32,
    pub other_display_misc: u32,
    pub gpu_cap_info: u32,
    pub sb_mmio_base_addr: u32,
    pub system_config: u32,
    pub cpu_cap_info: u32,
    pub max_nb_voltage: u32,
    pub min_nb_voltage: u32,
    pub boot_up_nb_voltage: u32,
    pub ext_disp_conn_info_offset: u32,
    pub csr_m3_arb_cntl_default: [u32; NUMBER_OF_CSR_M3_ARB],
    pub csr_m3_arb_cntl_uvd: [u32; NUMBER_OF_CSR_M3_ARB],
    pub csr_m3_arb_cntl_fs3d: [u32; NUMBER_OF_CSR_M3_ARB],
    pub gmc_restore_reset_time: u32,
    pub minimum_n_clk: u32,
    pub idle_n_clk: u32,
    pub ddr_dll_power_up_time: u32,
    pub ddr_pll_power_up_time: u32,
    pub pcie_clk_ss_type: u32,
    pub lvds_ss_percentage: u32,
    pub lvds_sspread_rate_in_10hz: u32,
    pub hdmi_ss_percentage: u32,
    pub hdmi_sspread_rate_in_10hz: u32,
    pub dvi_ss_percentage: u32,
    pub dvi_sspread_rate_in_10_hz: u32,
    pub sclk_dpm_boost_margin: u32,
    pub sclk_dpm_throttle_margin: u32,
    pub sclk_dpm_tdp_limit_pg: u32,
    pub sclk_dpm_tdp_limit_boost: u32,
    pub boost_engine_clock: u32,
    pub boost_vid_2bit: u32,
    pub enable_boost: u32,
    pub gnb_tdp_limit: u32,
    pub max_lvds_pclk_freq_in_single_link: u32,
    pub lvds_misc: u32,
    pub lvds_pwr_on_seq_dig_on_to_de_in_4ms: u32,
    pub lvds_pwr_on_seq_de_to_vary_bl_in_4ms: u32,
    pub lvds_pwr_off_seq_vary_bl_to_de_in4ms: u32,
    pub lvds_pwr_off_seq_de_to_dig_on_in4ms: u32,
    pub lvds_off_to_on_delay_in_4ms: u32,
    pub lvds_pwr_on_seq_vary_bl_to_blon_in_4ms: u32,
    pub lvds_pwr_off_seq_blon_to_vary_bl_in_4ms: u32,
    pub lvds_reserved1: u32,
    pub lvds_bit_depth_control_val: u32,
    pub dp0_ext_hdmi_slv_addr: u8,
    pub dp0_ext_hdmi_reg_num: u8,
    pub dp0_ext_hdmi_reg_settings: [i2c_reg_info; 9],
    pub dp0_ext_hdmi_6g_reg_num: u8,
    pub dp0_ext_hdmi_6g_reg_settings: [i2c_reg_info; 3],
    pub dp1_ext_hdmi_slv_addr: u8,
    pub dp1_ext_hdmi_reg_num: u8,
    pub dp1_ext_hdmi_reg_settings: [i2c_reg_info; 9],
    pub dp1_ext_hdmi_6g_reg_num: u8,
    pub dp1_ext_hdmi_6g_reg_settings: [i2c_reg_info; 3],
    pub dp2_ext_hdmi_slv_addr: u8,
    pub dp2_ext_hdmi_reg_num: u8,
    pub dp2_ext_hdmi_reg_settings: [i2c_reg_info; 9],
    pub dp2_ext_hdmi_6g_reg_num: u8,
    pub dp2_ext_hdmi_6g_reg_settings: [i2c_reg_info; 3],
    pub dp3_ext_hdmi_slv_addr: u8,
    pub dp3_ext_hdmi_reg_num: u8,
    pub dp3_ext_hdmi_reg_settings: [i2c_reg_info; 9],
    pub dp3_ext_hdmi_6g_reg_num: u8,
    pub dp3_ext_hdmi_6g_reg_settings: [i2c_reg_info; 3],
    pub dp_ss_control: u32,
    pub edp1_info: edp_info,
    pub edp2_info: edp_info,
    pub gpuclk_ss_percentage: u32,
    pub gpuclk_ss_type: u32,
}

/// `enum engine_id` (include/grph_object_id.h): the DIG/DAC/HPO encoder
/// engines. DIGA is 0, so a stream encoder's `stream_enc_inst` is
/// `eng_id - ENGINE_ID_DIGA`. ENGINE_ID_UNKNOWN (= -1) is omitted
/// (grow-as-reached — None stands in where it is needed).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[repr(i32)]
pub enum engine_id {
    #[default]
    ENGINE_ID_DIGA = 0,
    ENGINE_ID_DIGB,
    ENGINE_ID_DIGC,
    ENGINE_ID_DIGD,
    ENGINE_ID_DIGE,
    ENGINE_ID_DIGF,
    ENGINE_ID_DIGG,
    ENGINE_ID_DACA,
    ENGINE_ID_DACB,
    ENGINE_ID_VCE,
    ENGINE_ID_HPO_0,
    ENGINE_ID_HPO_1,
    ENGINE_ID_HPO_DP_0,
    ENGINE_ID_HPO_DP_1,
    ENGINE_ID_HPO_DP_2,
    ENGINE_ID_HPO_DP_3,
    ENGINE_ID_VIRTUAL,
    ENGINE_ID_COUNT,
}

// ── dc_caps and its tree (dc.h) ───────────────────────────────────────────
// Software capability structs the resource pool fills; the C uns:1 bitfields
// become bool here (these are not MMIO layouts — only the values matter).

/// `enum dc_plane_type` (dc_hw_types.h).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[repr(u32)]
pub enum dc_plane_type {
    #[default]
    DC_PLANE_TYPE_INVALID,
    DC_PLANE_TYPE_DCE_RGB,
    DC_PLANE_TYPE_DCE_UNDERLAY,
    DC_PLANE_TYPE_DCN_UNIVERSAL,
}

/// `struct rom_curve_caps` (dc.h).
#[derive(Default, Clone, Copy)]
pub struct rom_curve_caps {
    pub srgb: bool,
    pub bt2020: bool,
    pub gamma2_2: bool,
    pub pq: bool,
    pub hlg: bool,
}

/// `struct lut3d_caps` (dc.h); the anonymous groups become named structs.
#[derive(Default, Clone, Copy)]
pub struct lut3d_mem_layout_support {
    pub swizzle_3d_rgb: bool,
    pub swizzle_3d_bgr: bool,
    pub linear_1d: bool,
}
#[derive(Default, Clone, Copy)]
pub struct lut3d_mem_format_support {
    pub unorm_12msb: bool,
    pub unorm_12lsb: bool,
    pub float_fp1_5_10: bool,
}
#[derive(Default, Clone, Copy)]
pub struct lut3d_mem_pixel_order_support {
    pub order_rgba: bool,
    pub order_bgra: bool,
}
#[derive(Default, Clone, Copy)]
pub struct lut3d_dim_caps {
    pub dim_9: bool,
    pub dim_17: bool,
    pub dim_33: bool,
    pub dim_45: bool,
    pub dim_65: bool,
}
#[derive(Default, Clone, Copy)]
pub struct lut3d_caps {
    pub dma_3d_lut: bool,
    pub mem_layout_support: lut3d_mem_layout_support,
    pub mem_format_support: lut3d_mem_format_support,
    pub mem_pixel_order_support: lut3d_mem_pixel_order_support,
    pub lut_dim_caps: lut3d_dim_caps,
}

/// `struct dpp_color_caps` (dc.h).
#[derive(Default, Clone, Copy)]
pub struct dpp_color_caps {
    pub dcn_arch: bool,
    pub input_lut_shared: bool,
    pub icsc: bool,
    pub dgam_ram: bool,
    pub post_csc: bool,
    pub gamma_corr: bool,
    pub hw_3d_lut: bool,
    pub ogam_ram: bool,
    pub ocsc: bool,
    pub dgam_rom_for_yuv: bool,
    pub dgam_rom_caps: rom_curve_caps,
    pub ogam_rom_caps: rom_curve_caps,
}

/// `struct mpc_color_caps` (dc.h).
#[derive(Default, Clone, Copy)]
pub struct mpc_color_caps {
    pub gamut_remap: bool,
    pub ogam_ram: bool,
    pub ocsc: bool,
    pub num_3dluts: u8,
    pub num_rmcm_3dluts: u8,
    pub shared_3d_lut: bool,
    pub ogam_rom_caps: rom_curve_caps,
    pub mcm_3d_lut_caps: lut3d_caps,
    pub rmcm_3d_lut_caps: lut3d_caps,
    pub preblend: bool,
}

/// `struct dc_color_caps` (dc.h).
#[derive(Default, Clone, Copy)]
pub struct dc_color_caps {
    pub dpp: dpp_color_caps,
    pub mpc: mpc_color_caps,
}

/// `struct dc_dmub_caps` (dc.h).
#[derive(Default, Clone, Copy)]
pub struct dc_dmub_caps {
    pub psr: bool,
    pub mclk_sw: bool,
    pub subvp_psr: bool,
    pub gecc_enable: bool,
    pub fams_ver: u8,
    pub aux_backlight_support: bool,
}

/// `struct dc_scl_caps` (dc.h).
#[derive(Default, Clone, Copy)]
pub struct dc_scl_caps {
    pub sharpener_support: bool,
}

/// The two anonymous scale-factor groups of `dc_plane_cap` (x1000).
#[derive(Default, Clone, Copy)]
pub struct dc_plane_scale_factor {
    pub argb8888: u32,
    pub nv12: u32,
    pub fp16: u32,
}
/// `dc_plane_cap.pixel_format_support`.
#[derive(Default, Clone, Copy)]
pub struct dc_plane_pixel_format_support {
    pub argb8888: bool,
    pub nv12: bool,
    pub fp16: bool,
    pub p010: bool,
    pub ayuv: bool,
}

/// `struct dc_plane_cap` (dc.h).
#[derive(Default, Clone, Copy)]
pub struct dc_plane_cap {
    pub r#type: dc_plane_type,
    pub per_pixel_alpha: bool,
    pub pixel_format_support: dc_plane_pixel_format_support,
    pub max_upscale_factor: dc_plane_scale_factor,
    pub max_downscale_factor: dc_plane_scale_factor,
    pub min_width: u32,
    pub min_height: u32,
}

/// `struct dc_caps` (dc.h): what the resource pool reports and `dc_create`
/// fills (max_streams/links/audios/otg_num …).
#[derive(Default, Clone, Copy)]
pub struct dc_caps {
    pub max_streams: u32,
    pub max_links: u32,
    pub max_audios: u32,
    pub max_slave_planes: u32,
    pub max_slave_yuv_planes: u32,
    pub max_slave_rgb_planes: u32,
    pub max_planes: u32,
    pub max_downscale_ratio: u32,
    pub i2c_speed_in_khz: u32,
    pub i2c_speed_in_khz_hdcp: u32,
    pub dmdata_alloc_size: u32,
    pub max_cursor_size: u32,
    pub max_buffered_cursor_size: u32,
    pub max_video_width: u32,
    pub max_optimizable_video_width: u32,
    pub min_horizontal_blanking_period: u32,
    pub linear_pitch_alignment: i32,
    pub dcc_const_color: bool,
    pub dynamic_audio: bool,
    pub is_apu: bool,
    pub dual_link_dvi: bool,
    pub post_blend_color_processing: bool,
    pub force_dp_tps4_for_cp2520: bool,
    pub disable_dp_clk_share: bool,
    pub psp_setup_panel_mode: bool,
    pub extended_aux_timeout_support: bool,
    pub dmcub_support: bool,
    pub zstate_support: bool,
    pub ips_support: bool,
    pub ips_v2_support: bool,
    pub num_of_internal_disp: u32,
    pub max_dp_protocol_version: dp_protocol_version,
    pub mall_size_per_mem_channel: u32,
    pub mall_size_total: u32,
    pub cursor_cache_size: u32,
    pub planes: [dc_plane_cap; MAX_PLANES],
    pub color: dc_color_caps,
    pub dmub_caps: dc_dmub_caps,
    pub dp_hpo: bool,
    pub dp_hdmi21_pcon_support: bool,
    pub edp_dsc_support: bool,
    pub vbios_lttpr_aware: bool,
    pub vbios_lttpr_enable: bool,
    pub fused_io_supported: bool,
    pub max_otg_num: u32,
    pub max_cab_allocation_bytes: u32,
    pub cache_line_size: u32,
    pub cache_num_ways: u32,
    pub subvp_fw_processing_delay_us: u16,
    pub subvp_drr_max_vblank_margin_us: u8,
    pub subvp_prefetch_end_to_mall_start_us: u16,
    pub subvp_swath_height_margin_lines: u8,
    pub subvp_pstate_allow_width_us: u16,
    pub subvp_vertical_int_margin_us: u16,
    pub seamless_odm: bool,
    pub max_v_total: u32,
    pub vtotal_limited_by_fp2: bool,
    pub max_disp_clock_khz_at_vmin: u32,
    pub subvp_drr_vblank_start_margin_us: u8,
    pub cursor_not_scaled: bool,
    pub dcmode_power_limits_present: bool,
    pub sequential_ono: bool,
    pub dcc_plane_width_limit: u32,
    pub scl_caps: dc_scl_caps,
    pub num_of_host_routers: u8,
    pub num_of_dpias_per_host_router: u8,
    pub max_odm_combine_factor: u8,
}

impl Default for dp_protocol_version {
    // Zero-init matches DP_VERSION_1_4 (= 0), as a kzalloc'd dc_caps starts.
    fn default() -> Self { dp_protocol_version::DP_VERSION_1_4 }
}

/// `enum dc_status` (core_status.h).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum dc_status {
    DC_OK = 1,
    DC_NO_CONTROLLER_RESOURCE = 2,
    DC_NO_STREAM_ENC_RESOURCE = 3,
    DC_NO_CLOCK_SOURCE_RESOURCE = 4,
    DC_FAIL_CONTROLLER_VALIDATE = 5,
    DC_FAIL_ENC_VALIDATE = 6,
    DC_FAIL_ATTACH_SURFACES = 7,
    DC_FAIL_DETACH_SURFACES = 8,
    DC_FAIL_SURFACE_VALIDATE = 9,
    DC_NO_DP_LINK_BANDWIDTH = 10,
    DC_EXCEED_DONGLE_CAP = 11,
    DC_SURFACE_PIXEL_FORMAT_UNSUPPORTED = 12,
    DC_FAIL_BANDWIDTH_VALIDATE = 13,
    DC_FAIL_SCALING = 14,
    DC_FAIL_DP_LINK_TRAINING = 15,
    DC_FAIL_DSC_VALIDATE = 16,
    DC_NO_DSC_RESOURCE = 17,
    DC_FAIL_UNSUPPORTED_1 = 18,
    DC_FAIL_CLK_EXCEED_MAX = 21,
    DC_FAIL_CLK_BELOW_MIN = 22,
    DC_FAIL_CLK_BELOW_CFG_REQUIRED = 23,
    DC_NOT_SUPPORTED = 24,
    DC_UNSUPPORTED_VALUE = 25,
    DC_NO_LINK_ENC_RESOURCE = 26,
    DC_FAIL_DP_PAYLOAD_ALLOCATION = 27,
    DC_FAIL_DP_LINK_BANDWIDTH = 28,
    DC_FAIL_HW_CURSOR_SUPPORT = 29,
    DC_FAIL_DP_TUNNEL_BW_VALIDATE = 30,
    DC_ERROR_UNEXPECTED = -1,
}

/// `enum dc_edid_status` (dc_types.h).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum dc_edid_status {
    EDID_OK,
    EDID_BAD_INPUT,
    EDID_NO_RESPONSE,
    EDID_BAD_CHECKSUM,
    EDID_THE_SAME,
    EDID_FALL_BACK,
    EDID_PARTIAL_VALID,
}

/// `struct dc_static_screen_params` (dc_hw_types.h).
#[derive(Default, Clone, Copy, Debug)]
pub struct dc_static_screen_params {
    pub force_trigger: bool,
    pub cursor_update: bool,
    pub surface_update: bool,
    pub overlay_update: bool,
    pub num_frames: u32,
}
