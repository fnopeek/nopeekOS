//! display/dc/clk_mgr/dcn21/rn_clk_mgr.c + rn_clk_mgr_vbios_smu.c, and the
//! Renoir watermark tables of dml/dcn20/dcn20_fpu.c (Linux 6.18.55): Renoir's
//! clock manager, the construct path. The VBIOS SMC mailbox
//! (C2PMSG_67/83/91) is DC's own, beside the driver SMU's 66/82/90.
#![allow(dead_code)]

use crate::amdgpu_device::amdgpu_device;
use crate::clk_mgr::{
    clk_bw_params, clk_limit_table, clk_limit_table_entry, clk_mgr_funcs_id, clk_mgr_internal,
    clk_state_registers_and_bypass, dce_clock_read_ss_info, wm_range_table_entry, wm_table, MAX_NUM_DPM_LVL,
    WM_A, WM_B, WM_C, WM_D, WM_SET_COUNT, WM_TYPE_PSTATE_CHG, WM_TYPE_RETRAINING,
};
use crate::dc::dc;
use crate::clk_mgr::{DCN_PWR_STATE_LOW_POWER, DCN_PWR_STATE_MISSION_MODE};
use crate::dc_helper::{dm_read_reg, dm_write_reg, generic_reg_get};
use crate::dc_types::integrated_info;
use crate::dm_pp_smu::{
    dpm_clocks, pp_rn_get_dpm_clock_table, pp_smu_status, pp_smu_wm_range_sets, PP_SMU_NUM_DCFCLK_DPM_LEVELS,
    PP_SMU_NUM_FCLK_DPM_LEVELS,
    PP_SMU_NUM_SOCCLK_DPM_LEVELS,
};
use crate::fixpt31_32::{dc_fixpt_floor, dc_fixpt_from_int, dc_fixpt_mul_int};
use crate::linux::{msleep, udelay};
use crate::regs::*;
use alloc::boxed::Box;

/// `Ddr4MemType` / `LpDdr4MemType` (atomfirmware.h, enum atom_dmi_t17_mem_type_def).
const Ddr4MemType: u32 = 26;
const LpDdr4MemType: u32 = 30;

/// `LPDDR_MEM_RETRAIN_LATENCY` (dcn20_fpu.c), from the LPDDR4 training
/// counter requirement.
const LPDDR_MEM_RETRAIN_LATENCY: f64 = 4.977;

/// `IS_SMU_TIMEOUT` (dm_helpers.h).
fn is_smu_timeout(result: u32) -> bool {
    result == 0x0
}

/// `rn_smu_wait_for_response` (rn_clk_mgr_vbios_smu.c): the wait ends when
/// the response register is NOT zero.
fn rn_smu_wait_for_response(adev: &amdgpu_device, delay_us: u32, mut max_retries: u32) -> u32 {
    let mut res_val;

    loop {
        res_val = dm_read_reg(adev, RN_VBIOS_MP1_SMN_C2PMSG_91);
        if res_val != RN_VBIOSSMC_Status_BUSY {
            break;
        }

        if delay_us >= 1000 {
            msleep((delay_us / 1000) as u64);
        } else if delay_us > 0 {
            udelay(delay_us as u64);
        }
        if max_retries == 0 {
            break;
        }
        max_retries -= 1;
    }

    res_val
}

/// `rn_vbios_smu_send_msg_with_param` (rn_clk_mgr_vbios_smu.c).
fn rn_vbios_smu_send_msg_with_param(adev: &amdgpu_device, msg_id: u32, param: u32) -> i32 {
    let result = rn_smu_wait_for_response(adev, 10, 200000);

    if result == RN_VBIOSSMC_Status_BUSY {
        return -1;
    }

    // First clear response register
    dm_write_reg(adev, RN_VBIOS_MP1_SMN_C2PMSG_91, RN_VBIOSSMC_Status_BUSY);

    // Set the parameter register for the SMU message, unit is Mhz
    dm_write_reg(adev, RN_VBIOS_MP1_SMN_C2PMSG_83, param);

    // Trigger the message transaction by writing the message ID
    dm_write_reg(adev, RN_VBIOS_MP1_SMN_C2PMSG_67, msg_id);

    let result = rn_smu_wait_for_response(adev, 10, 200000);

    // dm_helpers_smu_timeout is empty in amdgpu_dm.
    let _ = is_smu_timeout(result);

    // Actual dispclk set is returned in the parameter register
    dm_read_reg(adev, RN_VBIOS_MP1_SMN_C2PMSG_83) as i32
}

/// `rn_vbios_smu_get_smu_version`.
fn rn_vbios_smu_get_smu_version(adev: &amdgpu_device) -> i32 {
    rn_vbios_smu_send_msg_with_param(adev, RN_VBIOSSMC_MSG_GetSmuVersion, 0)
}

/// `rn_vbios_smu_enable_48mhz_tmdp_refclk_pwrdwn`.
fn rn_vbios_smu_enable_48mhz_tmdp_refclk_pwrdwn(adev: &amdgpu_device, enable: bool) {
    rn_vbios_smu_send_msg_with_param(adev, RN_VBIOSSMC_MSG_EnableTmdp48MHzRefclkPwrDown, enable as u32);
}

/// `rn_vbios_smu_is_periodic_retraining_disabled`: a PMFW without the
/// message answers 1, so retraining counts as disabled unless it says
/// otherwise.
fn rn_vbios_smu_is_periodic_retraining_disabled(adev: &amdgpu_device) -> i32 {
    rn_vbios_smu_send_msg_with_param(adev, RN_VBIOSSMC_MSG_IsPeriodicRetrainingDisabled, 1)
}

/// `get_vco_frequency_from_reg` (rn_clk_mgr.c): FbMult is 8.16 fixed point
/// in the register, times the DFS reference clock.
fn get_vco_frequency_from_reg(adev: &amdgpu_device, clk_mgr: &clk_mgr_internal) -> i32 {
    let (_, fbmult_frac_val) = generic_reg_get(
        adev,
        RN_CLK1_CLK_PLL_REQ,
        RN_CLK1_CLK_PLL_REQ__FbMult_frac__SHIFT as u8,
        RN_CLK1_CLK_PLL_REQ__FbMult_frac_MASK,
    );
    let (_, fbmult_int_val) = generic_reg_get(
        adev,
        RN_CLK1_CLK_PLL_REQ,
        RN_CLK1_CLK_PLL_REQ__FbMult_int__SHIFT as u8,
        RN_CLK1_CLK_PLL_REQ__FbMult_int_MASK,
    );

    let mut pll_req = dc_fixpt_from_int(fbmult_int_val as i32);

    // the 16-bit fraction moves up to the top of the 32-bit fraction
    pll_req.value |= (fbmult_frac_val << 16) as i64;

    // multiply by REFCLK period
    pll_req = dc_fixpt_mul_int(pll_req, clk_mgr.dfs_ref_freq_khz as i32);

    // integer part is now VCO frequency in kHz
    dc_fixpt_floor(pll_req)
}

/// `struct rn_clk_internal` (clk_mgr_internal.h).
#[derive(Default)]
struct rn_clk_internal {
    CLK1_CLK0_CURRENT_CNT: u32,
    CLK1_CLK0_BYPASS_CNTL: u32,
    CLK1_CLK1_CURRENT_CNT: u32,
    CLK1_CLK1_BYPASS_CNTL: u32,
    CLK1_CLK2_CURRENT_CNT: u32,
    CLK1_CLK2_BYPASS_CNTL: u32,
    CLK1_CLK3_CURRENT_CNT: u32,
    CLK1_CLK3_DS_CNTL: u32,
    CLK1_CLK3_ALLOW_DS: u32,
    CLK1_CLK3_BYPASS_CNTL: u32,
}

/// `rn_dump_clk_registers_internal`.
fn rn_dump_clk_registers_internal(adev: &amdgpu_device, internal: &mut rn_clk_internal) {
    internal.CLK1_CLK3_CURRENT_CNT = dm_read_reg(adev, RN_CLK1_CLK3_CURRENT_CNT);
    internal.CLK1_CLK3_BYPASS_CNTL = dm_read_reg(adev, RN_CLK1_CLK3_BYPASS_CNTL);

    internal.CLK1_CLK3_DS_CNTL = dm_read_reg(adev, RN_CLK1_CLK3_DS_CNTL);
    internal.CLK1_CLK3_ALLOW_DS = dm_read_reg(adev, RN_CLK1_CLK3_ALLOW_DS);

    internal.CLK1_CLK1_CURRENT_CNT = dm_read_reg(adev, RN_CLK1_CLK1_CURRENT_CNT);
    internal.CLK1_CLK1_BYPASS_CNTL = dm_read_reg(adev, RN_CLK1_CLK1_BYPASS_CNTL);

    internal.CLK1_CLK2_CURRENT_CNT = dm_read_reg(adev, RN_CLK1_CLK2_CURRENT_CNT);
    internal.CLK1_CLK2_BYPASS_CNTL = dm_read_reg(adev, RN_CLK1_CLK2_BYPASS_CNTL);

    internal.CLK1_CLK0_CURRENT_CNT = dm_read_reg(adev, RN_CLK1_CLK0_CURRENT_CNT);
    internal.CLK1_CLK0_BYPASS_CNTL = dm_read_reg(adev, RN_CLK1_CLK0_BYPASS_CNTL);
}

/// `rn_dump_clk_registers`: the raw clock registers. The CSV log is written
/// only with `log_info.enabled`, which the construct leaves off.
fn rn_dump_clk_registers(adev: &amdgpu_device, regs_and_bypass: &mut clk_state_registers_and_bypass) {
    let mut internal = rn_clk_internal::default();

    rn_dump_clk_registers_internal(adev, &mut internal);

    regs_and_bypass.dcfclk = internal.CLK1_CLK3_CURRENT_CNT / 10;
    regs_and_bypass.dcf_deep_sleep_divider = internal.CLK1_CLK3_DS_CNTL / 10;
    regs_and_bypass.dcf_deep_sleep_allow = internal.CLK1_CLK3_ALLOW_DS;
    regs_and_bypass.dprefclk = internal.CLK1_CLK2_CURRENT_CNT / 10;
    regs_and_bypass.dispclk = internal.CLK1_CLK0_CURRENT_CNT / 10;
    regs_and_bypass.dppclk = internal.CLK1_CLK1_CURRENT_CNT / 10;

    regs_and_bypass.dppclk_bypass = internal.CLK1_CLK1_BYPASS_CNTL & 0x0007;
    if regs_and_bypass.dppclk_bypass > 4 {
        regs_and_bypass.dppclk_bypass = 0;
    }
    regs_and_bypass.dcfclk_bypass = internal.CLK1_CLK3_BYPASS_CNTL & 0x0007;
    if regs_and_bypass.dcfclk_bypass > 4 {
        regs_and_bypass.dcfclk_bypass = 0;
    }
    regs_and_bypass.dispclk_bypass = internal.CLK1_CLK0_BYPASS_CNTL & 0x0007;
    if regs_and_bypass.dispclk_bypass > 4 {
        regs_and_bypass.dispclk_bypass = 0;
    }
    regs_and_bypass.dprefclk_bypass = internal.CLK1_CLK2_BYPASS_CNTL & 0x0007;
    if regs_and_bypass.dprefclk_bypass > 4 {
        regs_and_bypass.dprefclk_bypass = 0;
    }
}

const fn wm(wm_inst: u32, pstate_latency_us: f64, sr_exit_time_us: f64, sr_enter_plus_exit_time_us: f64)
    -> wm_range_table_entry {
    wm_range_table_entry {
        wm_inst,
        wm_type: WM_TYPE_PSTATE_CHG,
        pstate_latency_us,
        sr_exit_time_us,
        sr_enter_plus_exit_time_us,
        valid: true,
    }
}

/// `ddr4_wm_table_gs` (dcn20_fpu.c).
pub const ddr4_wm_table_gs: wm_table = wm_table {
    entries: [
        wm(WM_A, 11.72, 7.09, 8.14),
        wm(WM_B, 11.72, 10.12, 11.48),
        wm(WM_C, 11.72, 10.12, 11.48),
        wm(WM_D, 11.72, 10.12, 11.48),
    ],
};

/// `lpddr4_wm_table_gs` (dcn20_fpu.c).
pub const lpddr4_wm_table_gs: wm_table = wm_table {
    entries: [
        wm(WM_A, 11.65333, 5.32, 6.38),
        wm(WM_B, 11.65333, 9.82, 11.196),
        wm(WM_C, 11.65333, 9.89, 11.24),
        wm(WM_D, 11.65333, 9.748, 11.102),
    ],
};

/// `lpddr4_wm_table_with_disabled_ppt` (dcn20_fpu.c).
pub const lpddr4_wm_table_with_disabled_ppt: wm_table = wm_table {
    entries: [
        wm(WM_A, 11.65333, 8.32, 9.38),
        wm(WM_B, 11.65333, 9.82, 11.196),
        wm(WM_C, 11.65333, 9.89, 11.24),
        wm(WM_D, 11.65333, 9.748, 11.102),
    ],
};

/// `ddr4_wm_table_rn` (dcn20_fpu.c).
pub const ddr4_wm_table_rn: wm_table = wm_table {
    entries: [
        wm(WM_A, 11.72, 11.90, 12.80),
        wm(WM_B, 11.72, 13.18, 14.30),
        wm(WM_C, 11.72, 13.18, 14.30),
        wm(WM_D, 11.72, 13.18, 14.30),
    ],
};

/// `ddr4_1R_wm_table_rn` (dcn20_fpu.c).
pub const ddr4_1R_wm_table_rn: wm_table = wm_table {
    entries: [
        wm(WM_A, 11.72, 13.90, 14.80),
        wm(WM_B, 11.72, 13.90, 14.80),
        wm(WM_C, 11.72, 13.90, 14.80),
        wm(WM_D, 11.72, 13.90, 14.80),
    ],
};

/// `lpddr4_wm_table_rn` (dcn20_fpu.c).
pub const lpddr4_wm_table_rn: wm_table = wm_table {
    entries: [
        wm(WM_A, 11.65333, 7.32, 8.38),
        wm(WM_B, 11.65333, 9.82, 11.196),
        wm(WM_C, 11.65333, 9.89, 11.24),
        wm(WM_D, 11.65333, 9.748, 11.102),
    ],
};

const fn clk_entry(dcfclk_mhz: u32, fclk_mhz: u32, memclk_mhz: u32) -> clk_limit_table_entry {
    clk_limit_table_entry {
        voltage: 0,
        dcfclk_mhz,
        fclk_mhz,
        memclk_mhz,
        socclk_mhz: 0,
        dtbclk_mhz: 0,
        dispclk_mhz: 0,
        dppclk_mhz: 0,
        phyclk_mhz: 0,
        phyclk_d18_mhz: 0,
        wck_ratio: 0,
    }
}

const ZERO_CLK: clk_limit_table_entry = clk_entry(0, 0, 0);
const ZERO_WM: wm_range_table_entry = wm_range_table_entry {
    wm_inst: 0,
    wm_type: 0,
    pstate_latency_us: 0.0,
    sr_exit_time_us: 0.0,
    sr_enter_plus_exit_time_us: 0.0,
    valid: false,
};

/// `rn_bw_params` (rn_clk_mgr.c): the hardcoded table the SMU's clock table
/// replaces when it answers.
pub const rn_bw_params: clk_bw_params = clk_bw_params {
    vram_type: Ddr4MemType,
    num_channels: 1,
    dram_channel_width_bytes: 0,
    dispclk_vco_khz: 0,
    dc_mode_softmax_memclk: 0,
    max_memclk_mhz: 0,
    clk_table: clk_limit_table {
        entries: [
            clk_entry(400, 400, 800),
            clk_entry(483, 800, 1600),
            clk_entry(602, 1067, 1067),
            clk_entry(738, 1333, 1600),
            ZERO_CLK,
            ZERO_CLK,
            ZERO_CLK,
            ZERO_CLK,
        ],
        num_entries_per_clk: crate::clk_mgr::clk_limit_num_entries {
            num_dcfclk_levels: 0,
            num_fclk_levels: 0,
            num_memclk_levels: 0,
            num_socclk_levels: 0,
            num_dtbclk_levels: 0,
            num_dispclk_levels: 0,
            num_dppclk_levels: 0,
            num_phyclk_levels: 0,
            num_phyclk_d18_levels: 0,
        },
        num_entries: 4,
    },
    wm_table: wm_table { entries: [ZERO_WM; WM_SET_COUNT] },
    dummy_pstate_table: [crate::clk_mgr::dummy_pstate_entry { dram_speed_mts: 0, dummy_pstate_latency_us: 0 }; 4],
    dc_mode_limit: ZERO_CLK,
};

/// `find_socclk_for_voltage`.
fn find_socclk_for_voltage(clock_table: &dpm_clocks, voltage: u32) -> u32 {
    for i in 0..PP_SMU_NUM_SOCCLK_DPM_LEVELS {
        if clock_table.SocClocks[i].Vol == voltage {
            return clock_table.SocClocks[i].Freq;
        }
    }
    0
}

/// `find_dcfclk_for_voltage`.
fn find_dcfclk_for_voltage(clock_table: &dpm_clocks, voltage: u32) -> u32 {
    for i in 0..PP_SMU_NUM_DCFCLK_DPM_LEVELS {
        if clock_table.DcfClocks[i].Vol == voltage {
            return clock_table.DcfClocks[i].Freq;
        }
    }
    0
}

/// `dcn21_clk_mgr_set_bw_params_wm_table` (dcn20_fpu.c): WM set D is
/// re-purposed for memory retraining.
fn dcn21_clk_mgr_set_bw_params_wm_table(bw_params: &mut clk_bw_params) {
    let d = &mut bw_params.wm_table.entries[WM_D as usize];
    d.pstate_latency_us = LPDDR_MEM_RETRAIN_LATENCY;
    d.wm_inst = WM_D;
    d.wm_type = WM_TYPE_RETRAINING;
    d.valid = true;
}

/// `rn_clk_mgr_helper_populate_bw_params`.
fn rn_clk_mgr_helper_populate_bw_params(bw_params: &mut clk_bw_params, clock_table: &dpm_clocks,
                                        bios_info: &integrated_info) {
    const _: () = assert!(PP_SMU_NUM_FCLK_DPM_LEVELS <= MAX_NUM_DPM_LVL);

    // Find lowest DPM, FCLK is filled in reverse order
    let mut j: i32 = -1;
    for i in (0..PP_SMU_NUM_FCLK_DPM_LEVELS).rev() {
        if clock_table.FClocks[i].Freq != 0 && clock_table.FClocks[i].Vol != 0 {
            j = i as i32;
            break;
        }
    }

    if j == -1 {
        // clock table is all 0s, just use our own hardcode
        return;
    }

    bw_params.clk_table.num_entries = (j + 1) as u32;

    let mut i = 0usize;
    while i < bw_params.clk_table.num_entries as usize {
        let ju = j as usize;
        let e = &mut bw_params.clk_table.entries[i];
        e.fclk_mhz = clock_table.FClocks[ju].Freq;
        e.memclk_mhz = clock_table.MemClocks[ju].Freq;
        e.voltage = clock_table.FClocks[ju].Vol;
        e.dcfclk_mhz = find_dcfclk_for_voltage(clock_table, clock_table.FClocks[ju].Vol);
        e.socclk_mhz = find_socclk_for_voltage(clock_table, e.voltage);
        i += 1;
        j -= 1;
    }

    bw_params.vram_type = bios_info.memory_type as u32;
    bw_params.num_channels = bios_info.ma_channel_number as u32;

    for i in 0..WM_SET_COUNT {
        bw_params.wm_table.entries[i].wm_inst = i as u32;

        if i >= bw_params.clk_table.num_entries as usize {
            bw_params.wm_table.entries[i].valid = false;
            continue;
        }

        bw_params.wm_table.entries[i].wm_type = WM_TYPE_PSTATE_CHG;
        bw_params.wm_table.entries[i].valid = true;
    }

    if bw_params.vram_type == LpDdr4MemType {
        dcn21_clk_mgr_set_bw_params_wm_table(bw_params);
    }
}

/// `ASICREV_IS_GREEN_SARDINE` (dal_asic_id.h).
fn asicrev_is_green_sardine(rev: u32) -> bool {
    rev >= GREEN_SARDINE_A0 && rev < 0xFF
}

/// `rn_clk_mgr_construct` (rn_clk_mgr.c).
pub fn rn_clk_mgr_construct(adev: &amdgpu_device, dc: &mut dc, clk_mgr: &mut clk_mgr_internal) {
    let mut clock_table = dpm_clocks::default();

    let is_green_sardine = asicrev_is_green_sardine(dc.ctx.asic_id.hw_internal_rev);

    clk_mgr.base.funcs = Some(clk_mgr_funcs_id::dcn21_funcs);

    let pp_smu = dc.res_pool.as_deref().and_then(|p| p.pp_smu);
    clk_mgr.has_pp_smu = pp_smu.is_some();

    clk_mgr.has_dccg = dc.res_pool.as_deref().is_some_and(|p| p.dccg.is_some());
    clk_mgr.dfs_bypass_disp_clk = 0;

    clk_mgr.dprefclk_ss_percentage = 0;
    clk_mgr.dprefclk_ss_divider = 1000;
    clk_mgr.ss_on_dprefclk = false;
    clk_mgr.dfs_ref_freq_khz = 48000;

    clk_mgr.smu_ver = rn_vbios_smu_get_smu_version(adev);

    clk_mgr.periodic_retraining_disabled = rn_vbios_smu_is_periodic_retraining_disabled(adev) != 0;

    // SMU Version 55.51.0 and up no longer have an issue that needs to limit
    // minimum dispclk
    if clk_mgr.smu_ver >= RN_SMU_VER_55_51_0 as i32 {
        dc.debug.min_disp_clk_khz = 0;
    }

    clk_mgr.base.dentist_vco_freq_khz = get_vco_frequency_from_reg(adev, clk_mgr);

    // in case we don't get a value from the register, use default
    if clk_mgr.base.dentist_vco_freq_khz == 0 {
        clk_mgr.base.dentist_vco_freq_khz = 3600000;
    }

    let mut bw_params = Box::new(rn_bw_params);
    let memory_type = dc.ctx.dc_bios.base.integrated_info.as_deref().map_or(0, |i| i.memory_type as u32);
    if memory_type == LpDdr4MemType {
        if clk_mgr.periodic_retraining_disabled {
            bw_params.wm_table = lpddr4_wm_table_with_disabled_ppt;
        } else if is_green_sardine {
            bw_params.wm_table = lpddr4_wm_table_gs;
        } else {
            bw_params.wm_table = lpddr4_wm_table_rn;
        }
    } else if is_green_sardine {
        bw_params.wm_table = ddr4_wm_table_gs;
    } else if dc.config.is_single_rank_dimm {
        bw_params.wm_table = ddr4_1R_wm_table_rn;
    } else {
        bw_params.wm_table = ddr4_wm_table_rn;
    }
    // Saved clocks configured at boot for debug purposes
    rn_dump_clk_registers(adev, &mut clk_mgr.base.boot_snapshot);

    clk_mgr.base.dprefclk_khz = 600000;
    dce_clock_read_ss_info(adev, dc, clk_mgr);

    if let Some(pp) = pp_smu {
        if pp.rn_funcs.get_dpm_clock_table.is_some() {
            let status = pp_rn_get_dpm_clock_table(adev, &mut clock_table);

            if status == pp_smu_status::PP_SMU_RESULT_OK {
                if let Some(ii) = dc.ctx.dc_bios.base.integrated_info.as_deref() {
                    rn_clk_mgr_helper_populate_bw_params(&mut bw_params, &clock_table, ii);
                    // treat memory config as single channel if memory is asymmetrics.
                    if dc.config.is_asymmetric_memory {
                        bw_params.num_channels = 1;
                    }
                }
            }
        }
    }
    clk_mgr.base.bw_params = Some(bw_params);

    // enable powerfeatures when displaycount goes to 0
    if clk_mgr.smu_ver >= 0x00371500 {
        rn_vbios_smu_enable_48mhz_tmdp_refclk_pwrdwn(adev, !dc.debug.disable_48mhz_pwrdwn);
    }
}

/// `dcn21_funcs` (rn_clk_mgr.c), as far as ported.
pub const DCN21_FUNCS: crate::clk_mgr::clk_mgr_ops = crate::clk_mgr::clk_mgr_ops {
    update_clocks: Some(rn_update_clocks),
    init_clocks: Some(rn_init_clocks),
    notify_wm_ranges: Some(rn_notify_wm_ranges),
    notify_link_rate_change: Some(rn_notify_link_rate_change),
};

/// `rn_init_clocks` (rn_clk_mgr.c): forget the clocks; the boot state is
/// assumed to support p-state changes.
fn rn_init_clocks(clk_mgr: &mut clk_mgr_internal) {
    clk_mgr.base.clks = crate::clk_mgr::dc_clocks::default();
    clk_mgr.base.clks.p_state_change_support = true;
    clk_mgr.base.clks.prev_p_state_change_support = true;
    clk_mgr.base.clks.pwr_state = crate::clk_mgr::DCN_PWR_STATE_UNKNOWN;
}

/// `build_watermark_ranges` (rn_clk_mgr.c): one reader range per valid
/// watermark set, selected by DCFCLK and unconstrained in FCLK; the
/// writeback range is unconstrained.
fn build_watermark_ranges(bw_params: &clk_bw_params, ranges: &mut pp_smu_wm_range_sets) {
    use crate::dm_pp_smu::{PP_SMU_WM_SET_RANGE_CLK_UNCONSTRAINED_MAX as UMAX,
                           PP_SMU_WM_SET_RANGE_CLK_UNCONSTRAINED_MIN as UMIN};
    let mut num_valid_sets = 0usize;

    for i in 0..WM_SET_COUNT {
        // skip empty entries, the smu array has no holes
        if !bw_params.wm_table.entries[i].valid {
            continue;
        }

        let r = &mut ranges.reader_wm_sets[num_valid_sets];
        r.wm_inst = bw_params.wm_table.entries[i].wm_inst as u8;
        r.wm_type = bw_params.wm_table.entries[i].wm_type as u8;
        // We will not select WM based on fclk, so leave it as unconstrained
        r.min_fill_clk_mhz = UMIN;
        r.max_fill_clk_mhz = UMAX;
        // dcfclk wil be used to select WM

        if r.wm_type as u32 == WM_TYPE_PSTATE_CHG {
            if i == 0 {
                r.min_drain_clk_mhz = 0;
            } else {
                // add 1 to make it non-overlapping with next lvl
                r.min_drain_clk_mhz = (bw_params.clk_table.entries[i - 1].dcfclk_mhz + 1) as u16;
            }
            r.max_drain_clk_mhz = bw_params.clk_table.entries[i].dcfclk_mhz as u16;
        } else {
            // unconstrained for memory retraining
            r.min_fill_clk_mhz = UMIN;
            r.max_fill_clk_mhz = UMAX;

            // Modify previous watermark range to cover up to max
            if num_valid_sets > 0 {
                ranges.reader_wm_sets[num_valid_sets - 1].max_fill_clk_mhz = UMAX;
            }
        }
        num_valid_sets += 1;
    }

    // Must have at least one set of valid watermarks
    ranges.num_reader_wm_sets = num_valid_sets as u32;

    // modify the min and max to make sure we cover the whole range
    ranges.reader_wm_sets[0].min_drain_clk_mhz = UMIN;
    ranges.reader_wm_sets[0].min_fill_clk_mhz = UMIN;
    let last = (ranges.num_reader_wm_sets as usize).wrapping_sub(1);
    ranges.reader_wm_sets[last].max_drain_clk_mhz = UMAX;
    ranges.reader_wm_sets[last].max_fill_clk_mhz = UMAX;

    // This is for writeback only, does not matter currently as no writeback support
    ranges.num_writer_wm_sets = 1;
    ranges.writer_wm_sets[0].wm_inst = WM_A as u8;
    ranges.writer_wm_sets[0].min_fill_clk_mhz = UMIN;
    ranges.writer_wm_sets[0].max_fill_clk_mhz = UMAX;
    ranges.writer_wm_sets[0].min_drain_clk_mhz = UMIN;
    ranges.writer_wm_sets[0].max_drain_clk_mhz = UMAX;
}

/// `rn_notify_wm_ranges` (rn_clk_mgr.c): tell the SMU which watermark set to
/// use for which clock range.
fn rn_notify_wm_ranges(adev: &amdgpu_device, dc: &dc, clk_mgr: &mut clk_mgr_internal) {
    let pp_smu = dc.res_pool.as_deref().and_then(|p| p.pp_smu);

    if !dc.debug.disable_pplib_wm_range {
        let base = &mut clk_mgr.base;
        build_watermark_ranges(base.bw_params.as_deref().expect("bw_params"), &mut base.ranges);

        // Notify PP Lib/SMU which Watermarks to use for which clock ranges
        if let Some(pp) = pp_smu
            && pp.rn_funcs.set_wm_ranges.is_some()
        {
            crate::dm_pp_smu::pp_rn_set_wm_ranges(adev, &base.ranges);
        }
    }
}

/// `khz_to_mhz_ceil` (clk_mgr_internal.h).
fn khz_to_mhz_ceil(khz: i32) -> i32 {
    (khz + 999) / 1000
}

/// `should_set_clock` (clk_mgr_internal.h).
fn should_set_clock(safe_to_lower: bool, calc_clk: i32, cur_clk: i32) -> bool {
    (safe_to_lower && calc_clk < cur_clk) || calc_clk > cur_clk
}

/// `rn_vbios_smu_set_dispclk` (rn_clk_mgr_vbios_smu.c). DCN 2.1 has no
/// DMCU to tell the new wait loop.
fn rn_vbios_smu_set_dispclk(adev: &amdgpu_device, requested_dispclk_khz: i32) -> i32 {
    // Unit of SMU msg parameter is Mhz
    let actual_dispclk_set_mhz = rn_vbios_smu_send_msg_with_param(adev, RN_VBIOSSMC_MSG_SetDispclkFreq,
        khz_to_mhz_ceil(requested_dispclk_khz) as u32);

    actual_dispclk_set_mhz * 1000
}

/// `rn_vbios_smu_set_hard_min_dcfclk` (rn_clk_mgr_vbios_smu.c).
fn rn_vbios_smu_set_hard_min_dcfclk(adev: &amdgpu_device, clk_mgr: &clk_mgr_internal, requested_dcfclk_khz: i32) -> i32 {
    let mut actual_dcfclk_set_mhz = -1;

    if clk_mgr.smu_ver < 0x370c00 {
        return actual_dcfclk_set_mhz;
    }

    actual_dcfclk_set_mhz = rn_vbios_smu_send_msg_with_param(adev, RN_VBIOSSMC_MSG_SetHardMinDcfclkByFreq,
        khz_to_mhz_ceil(requested_dcfclk_khz) as u32);

    actual_dcfclk_set_mhz * 1000
}

/// `rn_vbios_smu_set_min_deep_sleep_dcfclk` (rn_clk_mgr_vbios_smu.c).
fn rn_vbios_smu_set_min_deep_sleep_dcfclk(adev: &amdgpu_device, clk_mgr: &clk_mgr_internal,
                                          requested_min_ds_dcfclk_khz: i32) -> i32 {
    let mut actual_min_ds_dcfclk_mhz = -1;

    if clk_mgr.smu_ver < 0x370c00 {
        return actual_min_ds_dcfclk_mhz;
    }

    actual_min_ds_dcfclk_mhz = rn_vbios_smu_send_msg_with_param(adev, RN_VBIOSSMC_MSG_SetMinDeepSleepDcfclk,
        khz_to_mhz_ceil(requested_min_ds_dcfclk_khz) as u32);

    actual_min_ds_dcfclk_mhz * 1000
}

/// `rn_vbios_smu_set_phyclk` (rn_clk_mgr_vbios_smu.c).
pub fn rn_vbios_smu_set_phyclk(adev: &amdgpu_device, requested_phyclk_khz: i32) {
    rn_vbios_smu_send_msg_with_param(adev, RN_VBIOSSMC_MSG_SetPhyclkVoltageByFreq,
        khz_to_mhz_ceil(requested_phyclk_khz) as u32);
}

/// `rn_vbios_smu_set_dppclk` (rn_clk_mgr_vbios_smu.c).
fn rn_vbios_smu_set_dppclk(adev: &amdgpu_device, requested_dpp_khz: i32) -> i32 {
    let actual_dppclk_set_mhz = rn_vbios_smu_send_msg_with_param(adev, RN_VBIOSSMC_MSG_SetDppclkFreq,
        khz_to_mhz_ceil(requested_dpp_khz) as u32);

    actual_dppclk_set_mhz * 1000
}

/// `rn_vbios_smu_set_dcn_low_power_state` (rn_clk_mgr_vbios_smu.c).
fn rn_vbios_smu_set_dcn_low_power_state(adev: &amdgpu_device, state: i32) {
    let disp_count = if state == DCN_PWR_STATE_LOW_POWER { 0 } else { 1 };

    rn_vbios_smu_send_msg_with_param(adev, RN_VBIOSSMC_MSG_SetDisplayCount, disp_count);
}

/// `rn_get_active_display_cnt_wa` (rn_clk_mgr.c).
fn rn_get_active_display_cnt_wa(adev: &amdgpu_device, dc: &dc, context: &crate::dc_state::dc_state) -> i32 {
    let mut display_count = 0;
    let mut tmds_present = false;

    for i in 0..context.stream_count as usize {
        let stream = dc.streams[context.streams[i].unwrap()].as_deref().unwrap();

        if stream.signal == LF_SIGNAL_TYPE_HDMI_TYPE_A
            || stream.signal == LF_SIGNAL_TYPE_DVI_SINGLE_LINK
            || stream.signal == LF_SIGNAL_TYPE_DVI_DUAL_LINK
        {
            tmds_present = true;
        }
    }

    for i in 0..dc.link_count as usize {
        let enc = dc.links[i].link_enc.as_ref().unwrap();

        // abusing the fact that the dig and phy are coupled to see if the
        // phy is enabled
        if enc.funcs().is_dig_enabled.is_some_and(|f| f(adev, enc)) {
            display_count += 1;
        }
    }

    // WA for hang on HDMI after display off back back on
    if display_count == 0 && tmds_present {
        display_count = 1;
    }

    display_count
}

/// `rn_update_clocks_update_dpp_dto` (rn_clk_mgr.c).
fn rn_update_clocks_update_dpp_dto(adev: &amdgpu_device, dc: &mut dc, context: &crate::dc_state::dc_state,
                                   ref_dpp_clk: i32, safe_to_lower: bool) {
    let pool = dc.res_pool.as_deref_mut().unwrap();
    let dccg = pool.dccg.as_deref_mut().unwrap();
    dccg.base.ref_dppclk = ref_dpp_clk;

    for i in 0..pool.pipe_count as usize {
        // Loop index may not match dpp->inst if some pipes disabled, so
        // select correct inst from res_pool
        let dpp_inst = pool.dpps[i].as_deref().unwrap().base.inst as usize;
        let dppclk_khz = context.res_ctx.pipe_ctx[i].plane_res.bw_dppclk_khz;

        let dccg = pool.dccg.as_deref_mut().unwrap();
        let prev_dppclk_khz = dccg.base.pipe_dppclk_khz[dpp_inst];

        if safe_to_lower || prev_dppclk_khz < dppclk_khz {
            (dccg.funcs().update_dpp_dto.unwrap())(adev, dccg, dpp_inst, dppclk_khz);
        }
    }
}

/// `rn_update_clocks` (rn_clk_mgr.c): raise (or, when safe, lower) the
/// clocks to what `context` asks for. DCN 2.1 has no DMCU to tell a new
/// dispclk.
fn rn_update_clocks(adev: &amdgpu_device, dc: &mut dc, clk_mgr: &mut clk_mgr_internal,
                    context: &mut crate::dc_state::dc_state, safe_to_lower: bool) {
    let mut update_dppclk = false;
    let mut update_dispclk = false;
    let mut dpp_clock_lowered = false;

    if dc.work_arounds.skip_clock_update {
        return;
    }

    // if it is safe to lower, but we are already in the lower state, we
    // don't have to do anything also if safe to lower is false, we just go
    // in the higher state
    if safe_to_lower && !dc.debug.disable_48mhz_pwrdwn {
        // check that we're not already in lower
        if clk_mgr.base.clks.pwr_state != DCN_PWR_STATE_LOW_POWER {
            let display_count = rn_get_active_display_cnt_wa(adev, dc, context);

            // if we can go lower, go lower
            if display_count == 0 {
                rn_vbios_smu_set_dcn_low_power_state(adev, DCN_PWR_STATE_LOW_POWER);
                // update power state
                clk_mgr.base.clks.pwr_state = DCN_PWR_STATE_LOW_POWER;
            }
        }
    } else {
        // check that we're not already in D0
        if clk_mgr.base.clks.pwr_state != DCN_PWR_STATE_MISSION_MODE {
            rn_vbios_smu_set_dcn_low_power_state(adev, DCN_PWR_STATE_MISSION_MODE);
            // update power state
            clk_mgr.base.clks.pwr_state = DCN_PWR_STATE_MISSION_MODE;
        }
    }

    let new_clocks = &mut context.bw_ctx.dcn.clk;

    if should_set_clock(safe_to_lower, new_clocks.dcfclk_khz, clk_mgr.base.clks.dcfclk_khz) {
        clk_mgr.base.clks.dcfclk_khz = new_clocks.dcfclk_khz;
        rn_vbios_smu_set_hard_min_dcfclk(adev, clk_mgr, clk_mgr.base.clks.dcfclk_khz);
    }

    let new_clocks = &mut context.bw_ctx.dcn.clk;
    if should_set_clock(safe_to_lower, new_clocks.dcfclk_deep_sleep_khz, clk_mgr.base.clks.dcfclk_deep_sleep_khz) {
        clk_mgr.base.clks.dcfclk_deep_sleep_khz = new_clocks.dcfclk_deep_sleep_khz;
        rn_vbios_smu_set_min_deep_sleep_dcfclk(adev, clk_mgr, clk_mgr.base.clks.dcfclk_deep_sleep_khz);
    }

    // workaround: Limit dppclk to 100Mhz to avoid lower eDP panel switch to
    // plus 4K monitor underflow. Do not adjust dppclk if dppclk is 0 to
    // avoid unexpected result
    if new_clocks.dppclk_khz < 100000 && new_clocks.dppclk_khz > 0 {
        new_clocks.dppclk_khz = 100000;
    }

    // Temporally ignore thew 0 cases for disp and dpp clks. We may have a
    // new feature that requires 0 clks in the future.
    if new_clocks.dppclk_khz == 0 || new_clocks.dispclk_khz == 0 {
        new_clocks.dppclk_khz = clk_mgr.base.clks.dppclk_khz;
        new_clocks.dispclk_khz = clk_mgr.base.clks.dispclk_khz;
    }

    if should_set_clock(safe_to_lower, new_clocks.dppclk_khz, clk_mgr.base.clks.dppclk_khz) {
        if clk_mgr.base.clks.dppclk_khz > new_clocks.dppclk_khz {
            dpp_clock_lowered = true;
        }
        clk_mgr.base.clks.dppclk_khz = new_clocks.dppclk_khz;
        update_dppclk = true;
    }

    if should_set_clock(safe_to_lower, new_clocks.dispclk_khz, clk_mgr.base.clks.dispclk_khz) {
        clk_mgr.base.clks.dispclk_khz = new_clocks.dispclk_khz;
        clk_mgr.base.clks.actual_dispclk_khz = rn_vbios_smu_set_dispclk(adev, clk_mgr.base.clks.dispclk_khz);

        update_dispclk = true;
    }

    if dpp_clock_lowered {
        // increase per DPP DTO before lowering global dppclk with requested
        // dppclk
        rn_update_clocks_update_dpp_dto(adev, dc, context, clk_mgr.base.clks.dppclk_khz, safe_to_lower);

        clk_mgr.base.clks.actual_dppclk_khz = rn_vbios_smu_set_dppclk(adev, clk_mgr.base.clks.dppclk_khz);

        // update dpp dto with actual dpp clk.
        rn_update_clocks_update_dpp_dto(adev, dc, context, clk_mgr.base.clks.actual_dppclk_khz, safe_to_lower);
    } else {
        // increase global DPPCLK before lowering per DPP DTO
        if update_dppclk || update_dispclk {
            clk_mgr.base.clks.actual_dppclk_khz = rn_vbios_smu_set_dppclk(adev, clk_mgr.base.clks.dppclk_khz);
        }

        // always update dtos unless clock is lowered and not safe to lower
        rn_update_clocks_update_dpp_dto(adev, dc, context, clk_mgr.base.clks.actual_dppclk_khz, safe_to_lower);
    }

    // res_pool->dmcu is NULL on DCN 2.1.
    let _ = update_dispclk;
}

/// `rn_notify_link_rate_change` (rn_clk_mgr.c): the PHY clock follows the
/// fastest enabled link.
fn rn_notify_link_rate_change(adev: &amdgpu_device, dc: &mut dc, clk_mgr: &mut clk_mgr_internal, li: usize) {
    let mut max_phyclk_req = 0;

    clk_mgr.cur_phyclk_req_table[dc.links[li].link_index as usize] =
        dc.links[li].cur_link_settings.link_rate * LINK_RATE_REF_FREQ_IN_KHZ;

    for i in 0..LF_MAX_LINKS as usize {
        if clk_mgr.cur_phyclk_req_table[i] > max_phyclk_req {
            max_phyclk_req = clk_mgr.cur_phyclk_req_table[i];
        }
    }

    if max_phyclk_req != clk_mgr.base.clks.phyclk_khz as u32 {
        clk_mgr.base.clks.phyclk_khz = max_phyclk_req as i32;
        rn_vbios_smu_set_phyclk(adev, max_phyclk_req as i32);
    }
}
