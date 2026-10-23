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

/// `struct dc_firmware_info` (include/grph_object_ctrl_defs.h): ported
/// grow-as-reached — the clock-source construct reads only
/// `external_clock_source_frequency_for_dp`, so only that field is here.
#[derive(Default, Clone, Copy)]
pub struct dc_firmware_info {
    /// `struct pll_info pll_info` — the hw-i2c construct reads
    /// `pll_info.crystal_frequency`.
    pub pll_info: pll_info,
    /// In KHz.
    pub external_clock_source_frequency_for_dp: u32,
}

/// `struct pll_info` (the inline member of `dc_firmware_info`): only
/// `crystal_frequency` is reached so far (hw-i2c `reference_frequency`).
#[derive(Default, Clone, Copy)]
pub struct pll_info {
    /// In KHz.
    pub crystal_frequency: u32,
}

/// `struct dc_bios` (dc/dc_bios_types.h): the VBIOS interface a `dc_context`
/// points at. Linux holds it by pointer (owned by the BIOS parser); until
/// bios_parser2 lands the port carries the reached fields by value — the
/// clock-source path reads `fw_info` and `fw_info_valid`.
#[derive(Default)]
pub struct dc_bios {
    pub fw_info: dc_firmware_info,
    pub fw_info_valid: bool,
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
