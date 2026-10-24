//! display/dc/clk_mgr/clk_mgr.c + inc/hw/clk_mgr.h + inc/hw/clk_mgr_internal.h
//! + clk_mgr/dce100/dce_clk_mgr.c (`dce_clock_read_ss_info`) (Linux 6.18.55):
//! the clock manager `dc_construct` creates. Ported grow-as-reached: the
//! members the construct sets; `clks` and the update path join with the hw
//! sequencer.
#![allow(dead_code)]

use crate::amdgpu_device::amdgpu_device;
use crate::bios_parser2::{
    as_signal_type, bios_parser_get_spread_spectrum_info, bios_parser_get_ss_entry_number, bp_result,
    spread_spectrum_info,
};
use crate::dc::dc;
use crate::rn_clk_mgr::rn_clk_mgr_construct;
use alloc::boxed::Box;

pub const MAX_NUM_DPM_LVL: usize = 8;
pub const WM_SET_COUNT: usize = 4;
pub const WM_A: u32 = 0;
pub const WM_B: u32 = 1;
pub const WM_C: u32 = 2;
pub const WM_D: u32 = 3;

/// `enum pp_smu_wm_set_range_type`'s values `wm_type` takes (dm_pp_smu.h).
pub const WM_TYPE_PSTATE_CHG: u32 = 0;
pub const WM_TYPE_RETRAINING: u32 = 1;

/// `struct clk_limit_table_entry`.
#[derive(Default, Clone, Copy, Debug, PartialEq)]
pub struct clk_limit_table_entry {
    pub voltage: u32,
    pub dcfclk_mhz: u32,
    pub fclk_mhz: u32,
    pub memclk_mhz: u32,
    pub socclk_mhz: u32,
    pub dtbclk_mhz: u32,
    pub dispclk_mhz: u32,
    pub dppclk_mhz: u32,
    pub phyclk_mhz: u32,
    pub phyclk_d18_mhz: u32,
    pub wck_ratio: u32,
}

/// `struct clk_limit_num_entries`.
#[derive(Default, Clone, Copy, Debug, PartialEq)]
pub struct clk_limit_num_entries {
    pub num_dcfclk_levels: u32,
    pub num_fclk_levels: u32,
    pub num_memclk_levels: u32,
    pub num_socclk_levels: u32,
    pub num_dtbclk_levels: u32,
    pub num_dispclk_levels: u32,
    pub num_dppclk_levels: u32,
    pub num_phyclk_levels: u32,
    pub num_phyclk_d18_levels: u32,
}

/// `struct clk_limit_table`.
#[derive(Default, Clone, Copy, Debug, PartialEq)]
pub struct clk_limit_table {
    pub entries: [clk_limit_table_entry; MAX_NUM_DPM_LVL],
    pub num_entries_per_clk: clk_limit_num_entries,
    pub num_entries: u32,
}

/// `struct wm_range_table_entry`.
#[derive(Default, Clone, Copy, Debug, PartialEq)]
pub struct wm_range_table_entry {
    pub wm_inst: u32,
    pub wm_type: u32,
    pub pstate_latency_us: f64,
    pub sr_exit_time_us: f64,
    pub sr_enter_plus_exit_time_us: f64,
    pub valid: bool,
}

/// `struct wm_table`: the `entries` view of its union (the `nv_entries`
/// view belongs to DCN 2.0).
#[derive(Default, Clone, Copy, Debug, PartialEq)]
pub struct wm_table {
    pub entries: [wm_range_table_entry; WM_SET_COUNT],
}

/// `struct dummy_pstate_entry`.
#[derive(Default, Clone, Copy, Debug, PartialEq)]
pub struct dummy_pstate_entry {
    pub dram_speed_mts: u32,
    pub dummy_pstate_latency_us: u32,
}

/// `struct clk_bw_params`.
#[derive(Default, Clone, Copy, Debug, PartialEq)]
pub struct clk_bw_params {
    pub vram_type: u32,
    pub num_channels: u32,
    pub dram_channel_width_bytes: u32,
    pub dispclk_vco_khz: u32,
    pub dc_mode_softmax_memclk: u32,
    pub max_memclk_mhz: u32,
    pub clk_table: clk_limit_table,
    pub wm_table: wm_table,
    pub dummy_pstate_table: [dummy_pstate_entry; 4],
    pub dc_mode_limit: clk_limit_table_entry,
}

/// `struct clk_state_registers_and_bypass`.
#[derive(Default, Clone, Copy, Debug, PartialEq)]
pub struct clk_state_registers_and_bypass {
    pub dcfclk: u32,
    pub dcf_deep_sleep_divider: u32,
    pub dcf_deep_sleep_allow: u32,
    pub dprefclk: u32,
    pub dispclk: u32,
    pub dppclk: u32,
    pub dtbclk: u32,
    pub fclk: u32,
    pub dppclk_bypass: u32,
    pub dcfclk_bypass: u32,
    pub dprefclk_bypass: u32,
    pub dispclk_bypass: u32,
}

/// Which `clk_mgr_funcs` table is bound.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum clk_mgr_funcs_id {
    dcn21_funcs,
}

/// `struct clk_mgr_funcs` (clk_mgr.h), the members ported.
#[derive(Default, Clone, Copy)]
pub struct clk_mgr_ops {
    pub update_clocks: Option<fn(&amdgpu_device, &mut dc, &mut clk_mgr_internal, &mut crate::dc_state::dc_state, bool)>,
    pub init_clocks: Option<fn(&mut clk_mgr_internal)>,
    pub notify_wm_ranges: Option<fn(&amdgpu_device, &dc, &mut clk_mgr_internal)>,
    pub notify_link_rate_change: Option<fn(&amdgpu_device, &mut dc, &mut clk_mgr_internal, usize)>,
}

impl clk_mgr_funcs_id {
    pub fn ops(self) -> &'static clk_mgr_ops {
        match self {
            clk_mgr_funcs_id::dcn21_funcs => &crate::rn_clk_mgr::DCN21_FUNCS,
        }
    }
}

/// `enum dcn_pwr_state` (dc.h).
pub const DCN_PWR_STATE_UNKNOWN: i32 = -1;
pub const DCN_PWR_STATE_MISSION_MODE: i32 = 0;
pub const DCN_PWR_STATE_LOW_POWER: i32 = 3;

/// `struct dc_clocks` (dc.h). `zstate_support` is `enum
/// dcn_zstate_support_state`, `dtm_level` `enum dtm_pstate`.
#[derive(Default, Clone, Copy, Debug)]
pub struct dc_clocks {
    pub dispclk_khz: i32,
    pub actual_dispclk_khz: i32,
    pub dppclk_khz: i32,
    pub actual_dppclk_khz: i32,
    pub disp_dpp_voltage_level_khz: i32,
    pub dcfclk_khz: i32,
    pub socclk_khz: i32,
    pub dcfclk_deep_sleep_khz: i32,
    pub fclk_khz: i32,
    pub phyclk_khz: i32,
    pub dramclk_khz: i32,
    pub p_state_change_support: bool,
    pub zstate_support: u32,
    pub dtbclk_en: bool,
    pub ref_dtbclk_khz: i32,
    pub fclk_p_state_change_support: bool,
    pub pwr_state: i32,
    pub prev_p_state_change_support: bool,
    pub fclk_prev_p_state_change_support: bool,
    pub num_ways: i32,
    pub fw_based_mclk_switching: bool,
    pub fw_based_mclk_switching_shut_down: bool,
    pub prev_num_ways: i32,
    pub dtm_level: u32,
    pub max_supported_dppclk_khz: i32,
    pub max_supported_dispclk_khz: i32,
    pub bw_dppclk_khz: i32,
    pub bw_dispclk_khz: i32,
    pub idle_dramclk_khz: i32,
    pub idle_fclk_khz: i32,
    pub subvp_prefetch_dramclk_khz: i32,
    pub subvp_prefetch_fclk_khz: i32,
}

/// `struct clk_mgr`. `bw_params` points at a per-ASIC static in Linux
/// (`rn_bw_params`), which the construct then edits; the port owns the copy.
#[derive(Default)]
pub struct clk_mgr {
    pub funcs: Option<clk_mgr_funcs_id>,
    pub psr_allow_active_cache: bool,
    pub force_smu_not_present: bool,
    pub dc_mode_softmax_enabled: bool,
    pub dprefclk_khz: i32,
    pub dp_dto_source_clock_in_khz: i32,
    pub dentist_vco_freq_khz: i32,
    pub clks: dc_clocks,
    pub boot_snapshot: clk_state_registers_and_bypass,
    pub bw_params: Option<Box<clk_bw_params>>,
    pub ranges: crate::dm_pp_smu::pp_smu_wm_range_sets,
}

impl clk_mgr_internal {
    /// `clk_mgr->funcs`.
    pub fn funcs(&self) -> &'static clk_mgr_ops {
        self.base.funcs.expect("clk_mgr funcs").ops()
    }
}

/// `struct clk_mgr_internal`. `pp_smu` and `dccg` are the pool's; the port
/// reaches them through `dc.res_pool` and keeps only whether one was given.
#[derive(Default)]
pub struct clk_mgr_internal {
    pub base: clk_mgr,
    pub smu_ver: i32,
    pub has_pp_smu: bool,
    pub has_dccg: bool,
    pub dfs_bypass_enabled: bool,
    pub dfs_bypass_active: bool,
    pub dfs_ref_freq_khz: u32,
    pub dfs_bypass_disp_clk: i32,
    pub ss_on_dprefclk: bool,
    pub xgmi_enabled: bool,
    pub dprefclk_ss_percentage: i32,
    pub dprefclk_ss_divider: i32,
    pub periodic_retraining_disabled: bool,
    pub smu_present: bool,
    pub dpm_present: bool,
    pub pme_trigger_pending: bool,
    pub cur_phyclk_req_table: [u32; crate::regs::LF_MAX_LINKS as usize],
}

/// `dce_clock_read_ss_info` (dce_clk_mgr.c).
pub fn dce_clock_read_ss_info(adev: &amdgpu_device, dc: &dc, clk_mgr_dce: &mut clk_mgr_internal) {
    let bp = &dc.ctx.dc_bios;
    let bios = &adev.bios[..];
    let ss_info_num = bios_parser_get_ss_entry_number(bp, as_signal_type::AS_SIGNAL_TYPE_GPU_PLL);

    if ss_info_num != 0 {
        let mut info = spread_spectrum_info::default();
        let result =
            bios_parser_get_spread_spectrum_info(bp, bios, as_signal_type::AS_SIGNAL_TYPE_GPU_PLL, 0, &mut info);

        // VBIOS keeps the GPU PLL entry even with SS off; a non-zero
        // percentage is the sign that it is on.
        if result == bp_result::BP_RESULT_OK && info.spread_spectrum_percentage != 0 {
            clk_mgr_dce.ss_on_dprefclk = true;
            clk_mgr_dce.dprefclk_ss_divider = info.spread_percentage_divider as i32;

            if !info.r#type.CENTER_MODE {
                clk_mgr_dce.dprefclk_ss_percentage = info.spread_spectrum_percentage as i32;
            }

            return;
        }

        let result =
            bios_parser_get_spread_spectrum_info(bp, bios, as_signal_type::AS_SIGNAL_TYPE_DISPLAY_PORT, 0, &mut info);

        // Likewise for the DPREFCLK entry.
        if result == bp_result::BP_RESULT_OK && info.spread_spectrum_percentage != 0 {
            clk_mgr_dce.ss_on_dprefclk = true;
            clk_mgr_dce.dprefclk_ss_divider = info.spread_percentage_divider as i32;

            if !info.r#type.CENTER_MODE {
                clk_mgr_dce.dprefclk_ss_percentage = info.spread_spectrum_percentage as i32;
            }
            if dc.config.ignore_dpref_ss {
                clk_mgr_dce.dprefclk_ss_percentage = 0;
            }
        }
    }
}

/// `dc_clk_mgr_create` (clk_mgr.c), the FAMILY_RV / Renoir case.
pub fn dc_clk_mgr_create(adev: &amdgpu_device, dc: &mut dc) -> Option<Box<clk_mgr_internal>> {
    let mut clk_mgr = Box::new(clk_mgr_internal::default());
    rn_clk_mgr_construct(adev, dc, &mut clk_mgr);
    Some(clk_mgr)
}

/// Run `update_clocks` with dc's clock manager taken out of `dc` for the
/// call (Linux passes `dc->clk_mgr` beside `dc`).
pub fn clk_mgr_update_clocks(adev: &amdgpu_device, dc: &mut dc, context: &mut crate::dc_state::dc_state,
                             safe_to_lower: bool) {
    let mut clk_mgr = dc.clk_mgr.take().unwrap();
    (clk_mgr.funcs().update_clocks.unwrap())(adev, dc, &mut clk_mgr, context, safe_to_lower);
    dc.clk_mgr = Some(clk_mgr);
}

/// Run a hw sequencer step on dc's current state, taken out of `dc` for
/// the call.
fn on_current_state(adev: &amdgpu_device, dc: &mut dc,
                    f: fn(&amdgpu_device, &mut dc, &mut crate::dc_state::dc_state)) {
    let mut state = dc.current_state.take().unwrap();
    f(adev, dc, &mut state);
    dc.current_state = Some(state);
}

/// `clk_mgr_exit_optimized_pwr_state` (clk_mgr.c). The eDP links' PSR and
/// Replay are later stages; `psr_feature_enabled` is never set before.
pub fn clk_mgr_exit_optimized_pwr_state(adev: &amdgpu_device, dc: &mut dc) {
    let edp_links = crate::dc::dc_get_edp_links(dc);
    if let Some(f) = dc.hwss.exit_optimized_pwr_state {
        on_current_state(adev, dc, f);
    }

    for li in edp_links {
        if !dc.links[li].psr_settings.psr_feature_enabled {
            continue;
        }
        crate::linux::Line::new().s("clk_mgr_exit_optimized_pwr_state: edp_set_psr_allow_active not ported").say();
    }
}

/// `clk_mgr_optimize_pwr_state` (clk_mgr.c).
pub fn clk_mgr_optimize_pwr_state(adev: &amdgpu_device, dc: &mut dc) {
    let edp_links = crate::dc::dc_get_edp_links(dc);
    for li in edp_links {
        if !dc.links[li].psr_settings.psr_feature_enabled {
            continue;
        }
        crate::linux::Line::new().s("clk_mgr_optimize_pwr_state: edp_set_psr_allow_active not ported").say();
    }

    if let Some(f) = dc.hwss.optimize_pwr_state {
        on_current_state(adev, dc, f);
    }
}
