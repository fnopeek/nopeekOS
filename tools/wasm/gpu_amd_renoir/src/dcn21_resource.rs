//! display/dc/resource/dcn21/dcn21_resource.c (Linux 6.18.55): Renoir's
//! resource pool. This unit covers the "Resource + asic cap harcoding" section
//! of `dcn21_resource_construct` — `res_cap_rn` and the `dc->caps` fills. The
//! "Create resources" section (clock sources + the IP-block constructors) is
//! the next unit, each block with its regs.list entries + oracle.
#![allow(dead_code)]

use crate::amdgpu_device::amdgpu_device;
use crate::dc::{cap_func_id, dc, dc_debug_options, dc_init_data, dcc_option, pipe_split_policy};
use crate::dc_types::dce_environment;
use crate::dm_pp_smu::{dm_pp_get_funcs, pp_smu_funcs, pp_smu_ver};
use crate::dc_helper::{dm_read_reg, generic_reg_get};
use crate::dc_resource::{
    resource_caps, resource_pool, resource_straps, stream_enc_slot, update_num_audio,
    NO_UNDERLAY_PIPE,
};
use crate::dc_types::{
    dc_caps, dc_plane_cap, dc_plane_pixel_format_support, dc_plane_scale_factor, dc_plane_type,
    engine_id,
};
use crate::dce_audio::{dce_aud_endpoint_valid, dcn21_create_audio};
use crate::dce_hwseq::dcn21_hwseq_create;
use crate::dcn20_fpu::{dcn2_1_ip, dcn2_1_soc};
use crate::dcn20_stream_encoder::dcn21_stream_encoder_create;
use crate::dml_structs::{dml_init_instance, dml_project};
use crate::virtual_stream_encoder::virtual_stream_encoder_create;
use alloc::boxed::Box;
use crate::regs::{
    CC_DC_PIPE_DIS, DC_PINSTRAPS, DC_PINSTRAPS__DC_PINSTRAPS_AUDIO_MASK,
    DC_PINSTRAPS__DC_PINSTRAPS_AUDIO__SHIFT,
};
use crate::dccg::{dccg21_create, DCCG_MASK, DCCG_REGS, DCCG_SHIFT};
use crate::dcn10_ipp::dcn21_ipp_create;
use crate::dcn20_dpp::dcn21_dpp_create;
use crate::dcn20_dsc::dcn21_dsc_create;
use crate::dcn20_dwb::dcn20_dwbc_create;
use crate::dcn20_hubbub::dcn21_hubbub_create;
use crate::dcn20_mmhubbub::dcn20_mmhubbub_create;
use crate::dcn20_mpc::dcn21_mpc_create;
use crate::dcn20_opp::dcn21_opp_create;
use crate::dcn20_optc::dcn21_timing_generator_create;
use crate::dcn21_hubp::dcn21_hubp_create;
use crate::dce_aux::dcn21_aux_engine_create;
use crate::dce_clock_source::{clock_source_id, dcn21_clock_source_create, CLK_SRC_REGS};
use crate::dce_i2c_hw::dcn21_i2c_hw_create;

/// `enum dcn20_clk_src_array_id` (dcn21_resource.c): the clock-source slots.
const DCN20_CLK_SRC_PLL0: usize = 0;
const DCN20_CLK_SRC_PLL1: usize = 1;
const DCN20_CLK_SRC_PLL2: usize = 2;
const DCN20_CLK_SRC_PLL3: usize = 3;
const DCN20_CLK_SRC_PLL4: usize = 4;
const DCN20_CLK_SRC_TOTAL_DCN21: u32 = 5;

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

/// `static const struct dc_plane_cap plane_cap` (dcn21_resource.c): every plane
/// of Renoir is a universal DCN plane. Written into `dc.caps.planes[]` once the
/// live pipe count is known.
pub static PLANE_CAP: dc_plane_cap = dc_plane_cap {
    r#type: dc_plane_type::DC_PLANE_TYPE_DCN_UNIVERSAL,
    per_pixel_alpha: true,
    pixel_format_support: dc_plane_pixel_format_support {
        argb8888: true,
        nv12: true,
        fp16: true,
        p010: true,
        ayuv: false,
    },
    max_upscale_factor: dc_plane_scale_factor { argb8888: 16000, nv12: 16000, fp16: 16000 },
    max_downscale_factor: dc_plane_scale_factor { argb8888: 250, nv12: 250, fp16: 250 },
    min_width: 64,
    min_height: 64,
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

/// `read_pipe_fuses` (dcn21_resource.c): the pipe-disable fuse bitmap. Bit `i`
/// set means hw pipe `i` is fused off. RV1/Renoir has at most 4 pipes, so the
/// value is masked to its low 4 bits. This is the first hardware register read
/// of the DC port — every other constructor only stores pointers.
fn read_pipe_fuses(adev: &amdgpu_device) -> u32 {
    // REG(CC_DC_PIPE_DIS) is DCN_BASE seg2 + mmCC_DC_PIPE_DIS, resolved in regs.rs.
    dm_read_reg(adev, CC_DC_PIPE_DIS) & 0xf
}

/// `read_dce_straps` (dcn21_resource.c): read the board straps. dcn21 reads
/// only the audio pin-straps field of `DC_PINSTRAPS`; the other straps stay 0.
pub fn read_dce_straps(adev: &amdgpu_device, straps: &mut resource_straps) {
    let (_, audio) = generic_reg_get(
        adev,
        DC_PINSTRAPS,
        DC_PINSTRAPS__DC_PINSTRAPS_AUDIO__SHIFT as u8,
        DC_PINSTRAPS__DC_PINSTRAPS_AUDIO_MASK,
    );
    straps.dc_pinstraps_audio = audio;
}

/// The DIG engine ids the stream-encoder loop walks (Linux passes the loop
/// index `i` where an `engine_id` is expected; DIGA is 0).
const DIG_ENGINE_IDS: [engine_id; 5] = [
    engine_id::ENGINE_ID_DIGA,
    engine_id::ENGINE_ID_DIGB,
    engine_id::ENGINE_ID_DIGC,
    engine_id::ENGINE_ID_DIGD,
    engine_id::ENGINE_ID_DIGE,
];

/// `resource_construct` (core/dc_resource.c), specialised to dcn21's
/// `res_create_funcs` (read_dce_straps / dcn21_create_audio /
/// dcn21_stream_encoder_create / dcn21_hwseq_create): create the audio
/// endpoints, the DIG + virtual stream encoders and the hw sequencer. The
/// funcs-vtable dispatch (endpoint_valid, the create_* table) is modelled by
/// calling the concrete dcn21 functions directly.
pub fn resource_construct(
    adev: &amdgpu_device,
    num_virtual_links: u32,
    dc: &mut dc,
    pool: &mut resource_pool,
) -> bool {
    let caps = pool.res_cap.unwrap();
    let mut num_audio = caps.num_audio as u32;
    let mut straps = resource_straps::default();

    read_dce_straps(adev, &mut straps);

    pool.audio_count = 0;
    update_num_audio(&straps, &mut num_audio, &mut pool.audio_support);
    let _ = num_audio; // Linux derives it here; this ASIC's loop uses the cap.
    for i in 0..caps.num_audio as usize {
        let aud = dcn21_create_audio(i as u32);
        // aud->funcs->endpoint_valid(aud): stop at the first endpoint with no port.
        if !dce_aud_endpoint_valid(&aud, adev) {
            break;
        }
        pool.audios[i] = Some(aud);
        pool.audio_count += 1;
    }

    pool.stream_enc_count = 0;
    for i in 0..caps.num_stream_encoder as usize {
        pool.stream_enc[i] =
            Some(stream_enc_slot::Dcn10(dcn21_stream_encoder_create(DIG_ENGINE_IDS[i])));
        pool.stream_enc_count += 1;
    }

    // hpo_dp_stream_enc / hpo_dp_link_enc / mpc_3dlut are 0 on Renoir.

    dc.caps.dynamic_audio = pool.audio_count < pool.stream_enc_count;

    for _ in 0..num_virtual_links {
        let idx = pool.stream_enc_count as usize;
        pool.stream_enc[idx] = Some(stream_enc_slot::Virtual(virtual_stream_encoder_create()));
        pool.stream_enc_count += 1;
    }

    dc.hwseq = Some(dcn21_hwseq_create());
    true
}

/// `dcn21_resource_construct` (dcn21_resource.c), as far as it is ported: the
/// pool's resource caps, the `dc->caps` hardcoding, and the first "Create
/// resources" block — the five combo-PHY clock sources plus the DP-DTO clock
/// source. The remaining constructors (dccg, hubbub, per-pipe hubp/dpp/opp/optc,
/// mpc, encoders, `dml_init_instance`) grow this as each block lands. The pipe
/// loop honours `read_pipe_fuses`; `ctx->dc_bios->regs = &bios_regs` waits for
/// bios_parser, and the NULL-checks / `goto create_fail` return come with the
/// full construct.
pub fn dcn21_resource_construct(
    adev: &amdgpu_device,
    num_virtual_links: u32,
    dc: &mut dc,
    pool: &mut resource_pool,
) {
    let pipe_fuses = read_pipe_fuses(adev);

    pool.res_cap = Some(&RES_CAP_RN);

    // Resource + asic cap hardcoding.
    pool.underlay_pipe_index = NO_UNDERLAY_PIPE;
    // max pipe num for ASIC before check pipe fuses
    pool.pipe_count = RES_CAP_RN.num_timing_generator as u32;

    dcn21_set_caps(&mut dc.caps);

    if dc.ctx.dce_environment == dce_environment::DCE_ENV_PRODUCTION_DRV {
        dc.debug = DEBUG_DEFAULTS_DRV;
    }

    // The DML instance. `num_pipes` is the live DPP count (the IP max minus the
    // fused-off pipes); it caps the IP's max_num_dpp/otg before dml_init copies
    // the SoC/IP inputs into dc.dml. (Linux does this just before the pipe loop.)
    let mut ip = dcn2_1_ip();
    let mut num_pipes = ip.max_num_dpp;
    for i in 0..ip.max_num_dpp {
        if pipe_fuses & (1 << i) != 0 {
            num_pipes -= 1;
        }
    }
    ip.max_num_dpp = num_pipes;
    ip.max_num_otg = num_pipes;
    dc.dml = Some(Box::new(dml_init_instance(&dcn2_1_soc(), &ip, dml_project::DML_PROJECT_DCN21)));

    // Create resources: the clock sources. CLOCK_SOURCE_COMBO_PHY_PLL0..4 use
    // clk_src_regs[0..4]; the DP-DTO source reuses clk_src_regs[0] (Linux'
    // "todo: not reuse phy_pll registers").
    let bios = &dc.ctx.dc_bios.base;
    pool.clock_sources[DCN20_CLK_SRC_PLL0] = dcn21_clock_source_create(
        bios, clock_source_id::CLOCK_SOURCE_COMBO_PHY_PLL0, &CLK_SRC_REGS[0], false);
    pool.clock_sources[DCN20_CLK_SRC_PLL1] = dcn21_clock_source_create(
        bios, clock_source_id::CLOCK_SOURCE_COMBO_PHY_PLL1, &CLK_SRC_REGS[1], false);
    pool.clock_sources[DCN20_CLK_SRC_PLL2] = dcn21_clock_source_create(
        bios, clock_source_id::CLOCK_SOURCE_COMBO_PHY_PLL2, &CLK_SRC_REGS[2], false);
    pool.clock_sources[DCN20_CLK_SRC_PLL3] = dcn21_clock_source_create(
        bios, clock_source_id::CLOCK_SOURCE_COMBO_PHY_PLL3, &CLK_SRC_REGS[3], false);
    pool.clock_sources[DCN20_CLK_SRC_PLL4] = dcn21_clock_source_create(
        bios, clock_source_id::CLOCK_SOURCE_COMBO_PHY_PLL4, &CLK_SRC_REGS[4], false);

    pool.clk_src_count = DCN20_CLK_SRC_TOTAL_DCN21;

    pool.dp_clock_source = dcn21_clock_source_create(
        bios, clock_source_id::CLOCK_SOURCE_ID_DP_DTO, &CLK_SRC_REGS[0], true);

    // The DCCG.
    pool.dccg = Some(dccg21_create(&DCCG_REGS, &DCCG_SHIFT, &DCCG_MASK));

    // dmcu (with !config.disable_dmcu; it clears debug.dmub_command_table),
    // the DMUB psr and the DMUB/DCE abm join with the DMUB command path.

    pool.pp_smu = dcn21_pp_smu_create(dc.ctx.dce_version);

    // Per pipe: mem input (hubp) -> ipp -> dpp -> opp -> TG. A pipe fused off
    // (its bit set in pipe_fuses) is skipped: the hw instance stays `i`, but the
    // live blocks compact into slot `j`, so the pool has no gaps. On Renoir with
    // no pipes fused, j == pipe_count.
    let mut j = 0usize;
    for i in 0..pool.pipe_count as usize {
        if pipe_fuses & (1 << i) != 0 {
            continue;
        }
        pool.hubps[j] = dcn21_hubp_create(i as u32);
        pool.ipps[j] = Some(dcn21_ipp_create(i as u32));
        pool.dpps[j] = dcn21_dpp_create(i as u32);
        pool.opps[j] = Some(dcn21_opp_create(i as u32));
        pool.timing_generators[j] = Some(dcn21_timing_generator_create(i as u32));
        j += 1;
    }

    // Per ddc: aux engine (engines[i]) + hw i2c (hw_i2cs[i]); sw_i2cs stays None.
    let ext_aux = dc.caps.extended_aux_timeout_support;
    for i in 0..RES_CAP_RN.num_ddc as usize {
        pool.engines[i] = Some(dcn21_aux_engine_create(i as u32, ext_aux));
        pool.hw_i2cs[i] = Some(dcn21_i2c_hw_create(&dc.ctx.dc_bios.base, i as u32));
    }

    // Linux then sets the live-pipe counts to j, before mpc and hubbub.
    pool.timing_generator_count = j as u32;
    pool.pipe_count = j as u32;
    pool.mpcc_count = j as u32;

    pool.mpc = Some(dcn21_mpc_create());
    pool.hubbub = Some(dcn21_hubbub_create());

    for i in 0..RES_CAP_RN.num_dsc as usize {
        pool.dscs[i] = Some(dcn21_dsc_create(i as u32));
    }

    // The writeback path: one DWBC + one MCIF-WB per `num_dwb`.
    dcn20_dwbc_create(pool);
    dcn20_mmhubbub_create(pool);

    // Audio + stream encoders (DIG + virtual) + the hw sequencer. (Linux routes
    // these through res_create_funcs; the dcn21 binding is inlined.)
    resource_construct(adev, num_virtual_links, dc, pool);

    crate::hw_sequencer::dcn21_hw_sequencer_construct(dc);

    // The plane/ODM caps, now that the live pipe count is known.
    dc.caps.max_planes = pool.pipe_count;
    for i in 0..dc.caps.max_planes as usize {
        dc.caps.planes[i] = PLANE_CAP;
    }
    dc.caps.max_odm_combine_factor = 2;
    dc.cap_funcs.get_dcc_compression_cap = Some(cap_func_id::dcn20_get_dcc_compression_cap);

    // With this dcn21_resource_construct is complete bar the irq service
    // (dal_irq_service_dcn21_create) and `pool.funcs = &dcn21_res_pool_funcs`
    // (the validate/map vtable — stage E).
}

/// `debug_defaults_drv` (dcn21_resource.c).
const DEBUG_DEFAULTS_DRV: dc_debug_options = dc_debug_options {
    disable_dmcu: false,
    force_abm_enable: false,
    clock_trace: true,
    disable_pplib_clock_request: true,
    min_disp_clk_khz: 100000,
    pipe_split_policy: pipe_split_policy::MPC_SPLIT_DYNAMIC,
    force_single_disp_pipe_split: false,
    disable_dcc: dcc_option::DCC_ENABLE,
    vsr_support: true,
    performance_trace: false,
    max_downscale_src_width: 4096,
    disable_pplib_wm_range: false,
    scl_reset_length10: true,
    sanity_checks: true,
    disable_48mhz_pwrdwn: false,
    usbc_combo_phy_reset_wa: true,
    dmub_command_table: true,
    use_max_lb: true,
    enable_legacy_fast_update: true,
    using_dml2: false,
    hdmi20_disable: false,
    disable_stutter: false,
    disable_dsc: false,
    disable_clock_gate: false,
    force_subvp_mclk_switch: false,
    force_disable_subvp: false,
    fams2_config_enable: false,
    using_dml21: false,
    hdcp_lc_force_fw_enable: false,
    hdcp_lc_enable_sw_fallback: false,
    skip_detection_link_training: false,
    visual_confirm: 0,
    ignore_cable_id: false,
    enable_dmub_aux_for_legacy_ddc: false,
    dpia_debug_disable_dpia: false,
    disable_timeout: false,
};

/// `dcn21_pp_smu_create` (dcn21_resource.c): a table that is not Renoir's is
/// zeroed, which the port says with None.
fn dcn21_pp_smu_create(dce_version: crate::dc_types::dce_version) -> Option<pp_smu_funcs> {
    let mut pp_smu = pp_smu_funcs::default();

    dm_pp_get_funcs(dce_version, &mut pp_smu);

    if pp_smu.ver != pp_smu_ver::PP_SMU_VER_RN {
        return None;
    }

    Some(pp_smu)
}

/// `dcn21_create_resource_pool` (dcn21_resource.c). The port's pool is the
/// `resource_pool` itself (Linux wraps it as `dcn21_resource_pool.base`).
pub fn dcn21_create_resource_pool(
    adev: &amdgpu_device,
    init_data: &dc_init_data,
    dc: &mut dc,
) -> Option<Box<resource_pool>> {
    let mut pool = Box::new(resource_pool::default());
    dcn21_resource_construct(adev, init_data.num_virtual_links as u32, dc, &mut pool);
    Some(pool)
}
