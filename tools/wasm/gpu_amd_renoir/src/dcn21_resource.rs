//! display/dc/resource/dcn21/dcn21_resource.c (Linux 6.18.55): Renoir's
//! resource pool. This unit covers the "Resource + asic cap harcoding" section
//! of `dcn21_resource_construct` — `res_cap_rn` and the `dc->caps` fills. The
//! "Create resources" section (clock sources + the IP-block constructors) is
//! the next unit, each block with its regs.list entries + oracle.
#![allow(dead_code)]

use crate::amdgpu_device::amdgpu_device;
use crate::dc::dc;
use crate::dc_helper::dm_read_reg;
use crate::dc_resource::{resource_caps, resource_pool, NO_UNDERLAY_PIPE};
use crate::dc_types::dc_caps;
use crate::regs::CC_DC_PIPE_DIS;
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

/// `dcn21_resource_construct` (dcn21_resource.c), as far as it is ported: the
/// pool's resource caps, the `dc->caps` hardcoding, and the first "Create
/// resources" block — the five combo-PHY clock sources plus the DP-DTO clock
/// source. The remaining constructors (dccg, hubbub, per-pipe hubp/dpp/opp/optc,
/// mpc, encoders, `dml_init_instance`) grow this as each block lands. The pipe
/// loop honours `read_pipe_fuses`; `ctx->dc_bios->regs = &bios_regs` waits for
/// bios_parser, and the NULL-checks / `goto create_fail` return come with the
/// full construct.
pub fn dcn21_resource_construct(adev: &amdgpu_device, dc: &mut dc, pool: &mut resource_pool) {
    let pipe_fuses = read_pipe_fuses(adev);

    pool.res_cap = Some(&RES_CAP_RN);

    // Resource + asic cap hardcoding.
    pool.underlay_pipe_index = NO_UNDERLAY_PIPE;
    // max pipe num for ASIC before check pipe fuses
    pool.pipe_count = RES_CAP_RN.num_timing_generator as u32;

    dcn21_set_caps(&mut dc.caps);

    // Create resources: the clock sources. CLOCK_SOURCE_COMBO_PHY_PLL0..4 use
    // clk_src_regs[0..4]; the DP-DTO source reuses clk_src_regs[0] (Linux'
    // "todo: not reuse phy_pll registers").
    let bios = &dc.ctx.dc_bios;
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

    // The DCCG. (dmcu/psr/abm are gated on dc->config and wait; pp_smu waits.)
    pool.dccg = Some(dccg21_create(&DCCG_REGS, &DCCG_SHIFT, &DCCG_MASK));

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
        pool.hw_i2cs[i] = Some(dcn21_i2c_hw_create(&dc.ctx.dc_bios, i as u32));
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
}
