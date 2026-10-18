//! pm/swsmu/amdgpu_smu.c and smu12/renoir_ppt.c (Linux 6.18.55): the SMU
//! block's early_init, which picks renoir_ppt for MP1 12.0.0.

use crate::amdgpu_device::amdgpu_device;
use crate::regs::*;

/// `struct smu_context`, the part early_init fills.
#[derive(Default)]
pub struct smu_context {
    pub pm_enabled: bool,
    pub is_apu: bool,
    pub od_enabled: bool,
    pub smu_baco_state: u32,
    pub smu_baco_platform_support: bool,
    pub smu_baco_maco_support: bool,
    pub user_dpm_profile_fan_mode: i32,
    pub power_profile_mode: u32,
    /// `ppt_funcs` and the message, clock, table and workload maps, by name.
    pub ppt_funcs: Option<&'static str>,
    pub message_map: Option<&'static str>,
    pub clock_map: Option<&'static str>,
    pub table_map: Option<&'static str>,
    pub workload_map: Option<&'static str>,
    pub smc_driver_if_version: u32,
    pub param_reg: u32,
    pub msg_reg: u32,
    pub resp_reg: u32,
}

/// `SOC15_REG_OFFSET(MP1, 0, reg)`.
fn mp1_reg(adev: &amdgpu_device, reg: u32, base_idx: u32) -> u32 {
    adev.reg_offset[MP1_HWIP as usize][0].get(base_idx as usize).copied().unwrap_or(0) + reg
}

/// `renoir_set_ppt_funcs`.
fn renoir_set_ppt_funcs(adev: &mut amdgpu_device) {
    let param_reg = mp1_reg(adev, mmMP1_SMN_C2PMSG_82, mmMP1_SMN_C2PMSG_82_BASE_IDX);
    let msg_reg = mp1_reg(adev, mmMP1_SMN_C2PMSG_66, mmMP1_SMN_C2PMSG_66_BASE_IDX);
    let resp_reg = mp1_reg(adev, mmMP1_SMN_C2PMSG_90, mmMP1_SMN_C2PMSG_90_BASE_IDX);
    let smu = &mut adev.smu;
    smu.ppt_funcs = Some("renoir_ppt_funcs");
    smu.message_map = Some("renoir_message_map");
    smu.clock_map = Some("renoir_clk_map");
    smu.table_map = Some("renoir_table_map");
    smu.workload_map = Some("renoir_workload_map");
    smu.smc_driver_if_version = SMU12_DRIVER_IF_VERSION;
    smu.is_apu = true;
    smu.param_reg = param_reg;
    smu.msg_reg = msg_reg;
    smu.resp_reg = resp_reg;
}

/// `smu_set_funcs`: MP1 12.0.0, so renoir_ppt.
fn smu_set_funcs(adev: &mut amdgpu_device) -> i32 {
    if adev.pm.pp_feature & PP_OVERDRIVE_MASK != 0 {
        adev.smu.od_enabled = true;
    }
    if adev.ip_version(MP1_HWIP, 0) == 12 << 24 {
        renoir_set_ppt_funcs(adev);
        0
    } else {
        -22
    }
}

/// `smu_early_init`. `smu_init_microcode` is 0: renoir_ppt_funcs has no
/// `init_microcode`, the SMU firmware of an APU comes with the system BIOS.
pub fn smu_early_init(adev: &mut amdgpu_device) -> i32 {
    adev.smu = smu_context {
        pm_enabled: crate::tables::amdgpu_dpm[0] != 0,
        is_apu: false,
        smu_baco_state: SMU_BACO_STATE_NONE,
        smu_baco_platform_support: false,
        smu_baco_maco_support: false,
        user_dpm_profile_fan_mode: -1,
        power_profile_mode: PP_SMC_POWER_PROFILE_UNKNOWN as u32,
        ..smu_context::default()
    };
    adev.powerplay_pp_funcs = Some("swsmu_pm_funcs");
    smu_set_funcs(adev)
}
