//! display/dc/resource/dcn21/dcn21_resource.c (Linux 6.18.55): Renoir's
//! resource pool. This unit covers the "Resource + asic cap harcoding" section
//! of `dcn21_resource_construct` — `res_cap_rn` and the `dc->caps` fills. The
//! "Create resources" section (clock sources + the IP-block constructors) is
//! the next unit, each block with its regs.list entries + oracle.
#![allow(dead_code)]

use crate::dc_resource::resource_caps;
use crate::dc_types::dc_caps;

/// `static const struct resource_caps res_cap_rn` (dcn21_resource.c):
/// Renoir has 4 timing generators / OPPs / video planes / audio, 5 stream
/// encoders and PLLs, 1 DWB, 5 DDC, 16 VMIDs, 3 DSC. Fields Linux leaves out
/// of the designated initializer are 0.
pub static RES_CAP_RN: resource_caps = resource_caps {
    num_timing_generator: 4,
    num_opp: 4,
    num_dpp: 0,
    num_video_plane: 4,
    num_audio: 4,
    num_stream_encoder: 5,
    num_pll: 5,
    num_dwb: 1,
    num_ddc: 5,
    num_vmid: 16,
    num_dsc: 3,
    num_dig_link_enc: 0,
    num_usb4_dpia: 0,
    num_hpo_dp_stream_encoder: 0,
    num_hpo_dp_link_encoder: 0,
    num_mpc_3dlut: 0,
};

/// The `dc->caps.*` hardcoding of `dcn21_resource_construct` ("Resource + asic
/// cap harcoding"), extracted so the full construct calls it after it has set
/// up ctx and the pool. The emitted caps values are identical to Linux'.
pub fn dcn21_set_caps(caps: &mut dc_caps) {
    caps.max_downscale_ratio = 200;
    caps.i2c_speed_in_khz = 100;
    caps.i2c_speed_in_khz_hdcp = 5; // 1.4 w/a applied by default
    caps.max_cursor_size = 256;
    caps.min_horizontal_blanking_period = 80;
    caps.dmdata_alloc_size = 2048;

    caps.max_slave_planes = 3;
    caps.max_slave_yuv_planes = 3;
    caps.max_slave_rgb_planes = 3;
    caps.post_blend_color_processing = true;
    caps.force_dp_tps4_for_cp2520 = true;
    caps.extended_aux_timeout_support = true;
    caps.dmcub_support = true;
    caps.is_apu = true;

    // Color pipeline capabilities.
    caps.color.dpp.dcn_arch = true;
    caps.color.dpp.input_lut_shared = false;
    caps.color.dpp.icsc = true;
    caps.color.dpp.dgam_ram = true;
    caps.color.dpp.dgam_rom_caps.srgb = true;
    caps.color.dpp.dgam_rom_caps.bt2020 = true;
    caps.color.dpp.dgam_rom_caps.gamma2_2 = false;
    caps.color.dpp.dgam_rom_caps.pq = false;
    caps.color.dpp.dgam_rom_caps.hlg = false;
    caps.color.dpp.post_csc = false;
    caps.color.dpp.gamma_corr = false;
    caps.color.dpp.dgam_rom_for_yuv = true;

    caps.color.dpp.hw_3d_lut = true;
    caps.color.dpp.ogam_ram = true;
    // no OGAM ROM on DCN2
    caps.color.dpp.ogam_rom_caps.srgb = false;
    caps.color.dpp.ogam_rom_caps.bt2020 = false;
    caps.color.dpp.ogam_rom_caps.gamma2_2 = false;
    caps.color.dpp.ogam_rom_caps.pq = false;
    caps.color.dpp.ogam_rom_caps.hlg = false;
    caps.color.dpp.ocsc = false;

    caps.color.mpc.gamut_remap = false;
    caps.color.mpc.num_3dluts = 0;
    caps.color.mpc.shared_3d_lut = false;
    caps.color.mpc.ogam_ram = true;
    caps.color.mpc.ogam_rom_caps.srgb = false;
    caps.color.mpc.ogam_rom_caps.bt2020 = false;
    caps.color.mpc.ogam_rom_caps.gamma2_2 = false;
    caps.color.mpc.ogam_rom_caps.pq = false;
    caps.color.mpc.ogam_rom_caps.hlg = false;
    caps.color.mpc.ocsc = true;

    caps.dp_hdmi21_pcon_support = true;
}
