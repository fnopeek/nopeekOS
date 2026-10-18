//! pm/swsmu/amdgpu_smu.c and smu12/renoir_ppt.c (Linux 6.18.55): the SMU
//! block's early_init, which picks renoir_ppt for MP1 12.0.0.

use crate::amdgpu_device::amdgpu_device;
use crate::linux::Line;
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
    /// `smc_fw_state`: SMU_FW_INIT until the first message.
    pub smc_fw_state: u32,
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

/// `smu_cmn_to_asic_specific_index`, CMN2ASIC_MAPPING_MSG: the ASIC's
/// message number from `renoir_message_map`.
fn smu_cmn_to_asic_specific_index(adev: &amdgpu_device, msg: u32) -> i32 {
    if msg >= SMU_MSG_MAX_COUNT || adev.smu.message_map.is_none() {
        return -22;
    }
    let m = &crate::tables::renoir_message_map;
    let at = (msg * CMN2ASIC_MSG_MAPPING_SIZE / 4) as usize;
    let field = |off: u32| m.get(at + (off / 4) as usize).copied().unwrap_or(0);
    if field(CMN2ASIC_MSG_MAPPING_valid_mapping) == 0 {
        return -22;
    }
    // Bare metal: SMU_MSG_VF_FLAG is not asked for.
    field(CMN2ASIC_MSG_MAPPING_map_to)
}

/// `__smu_cmn_poll_stat`: up to `usec_timeout * 20` microseconds for a
/// response.
fn __smu_cmn_poll_stat(adev: &amdgpu_device) -> u32 {
    let mut reg = 0;
    for _ in 0..adev.usec_timeout * 20 {
        reg = adev.rreg32(adev.smu.resp_reg).unwrap_or(0);
        if reg & MP1_C2PMSG_90__CONTENT_MASK != 0 {
            break;
        }
        crate::linux::udelay(1);
    }
    reg
}

/// `__smu_cmn_reg2errno`.
fn __smu_cmn_reg2errno(reg: u32) -> i32 {
    match reg {
        // The SMU is busy--still executing your command.
        SMU_RESP_NONE => -62,
        SMU_RESP_OK => 0,
        // Command completed successfully, but the command status was failure.
        SMU_RESP_CMD_FAIL => -5,
        // Unknown command--ignored by the SMU.
        SMU_RESP_CMD_UNKNOWN => -95,
        // Valid command--bad prerequisites.
        SMU_RESP_CMD_BAD_PREREQ => -22,
        // The SMU is busy with other commands. The client should retry in 10 us.
        SMU_RESP_BUSY_OTHER => -16,
        // Unknown or debug response from the SMU.
        _ => -121,
    }
}

/// `__smu_cmn_reg_print_error`.
fn __smu_cmn_reg_print_error(adev: &amdgpu_device, reg: u32, index: i32, param: u32, message: &[u8]) {
    match reg {
        SMU_RESP_NONE => {
            let msg_idx = adev.rreg32(adev.smu.msg_reg).unwrap_or(0);
            let prm = adev.rreg32(adev.smu.param_reg).unwrap_or(0);
            Line::new().s("SMU: I'm not done with your previous command: SMN_C2PMSG_66:0x").hexw(msg_idx, 8)
                .s(" SMN_C2PMSG_82:0x").hexw(prm, 8).say();
        }
        SMU_RESP_OK | SMU_RESP_CMD_FAIL => {}
        SMU_RESP_CMD_UNKNOWN => {
            Line::new().s("SMU: unknown command: index:").dec(index as u64).s(" param:0x").hexw(param, 8)
                .s(" message:").bytes(message).say();
        }
        SMU_RESP_CMD_BAD_PREREQ => {
            Line::new().s("SMU: valid command, bad prerequisites: index:").dec(index as u64).s(" param:0x")
                .hexw(param, 8).s(" message:").bytes(message).say();
        }
        SMU_RESP_BUSY_OTHER => {
            Line::new().s("SMU: I'm very busy for your command: index:").dec(index as u64).s(" param:0x")
                .hexw(param, 8).s(" message:").bytes(message).say();
        }
        SMU_RESP_DEBUG_END => {
            Line::new().s("SMU: I'm debugging!").say();
        }
        _ => {
            Line::new().s("SMU: response:0x").hexw(reg, 8).s(" for index:").dec(index as u64).s(" param:0x")
                .hexw(param, 8).s(" message:").bytes(message).s("?").say();
        }
    }
}

/// `__smu_cmn_send_msg`.
fn __smu_cmn_send_msg(adev: &amdgpu_device, msg: u16, param: u32) {
    adev.wreg32(adev.smu.resp_reg, 0);
    adev.wreg32(adev.smu.param_reg, param);
    adev.wreg32(adev.smu.msg_reg, msg as u32);
}

/// `smu_cmn_send_smc_msg_with_param`, without a read-back argument. No RAS
/// priority filter (`smc_fw_caps` 0).
pub fn smu_cmn_send_smc_msg_with_param(adev: &mut amdgpu_device, msg: u32, param: u32, name: &[u8]) -> i32 {
    let index = smu_cmn_to_asic_specific_index(adev, msg);
    if index < 0 {
        return if index == -13 { 0 } else { index };
    }
    let mut poll = true;
    if adev.smu.smc_fw_state == SMU_FW_HANG {
        Line::new().s("SMU is in hanged state, failed to send smu message!").say();
        return -121;
    } else if adev.smu.smc_fw_state == SMU_FW_INIT {
        // Ignore initial smu response register value
        poll = false;
        adev.smu.smc_fw_state = SMU_FW_RUNTIME;
    }
    if poll {
        let reg = __smu_cmn_poll_stat(adev);
        let res = __smu_cmn_reg2errno(reg);
        if reg == SMU_RESP_NONE || res == -121 {
            __smu_cmn_reg_print_error(adev, reg, index, param, name);
            return res;
        }
    }
    __smu_cmn_send_msg(adev, index as u16, param);
    let reg = __smu_cmn_poll_stat(adev);
    let res = __smu_cmn_reg2errno(reg);
    if res != 0 {
        if res == -121 {
            adev.smu.smc_fw_state = SMU_FW_HANG;
        }
        __smu_cmn_reg_print_error(adev, reg, index, param, name);
    }
    Line::new().s("smu send message: ").bytes(name).s("(").dec(index as u64).s(") param: 0x").hexw(param, 8)
        .s(", resp: 0x").hexw(reg, 8).dbg();
    res
}

/// `smu_v12_0_mode2_reset` (smu12/smu_v12_0.c).
fn smu_v12_0_mode2_reset(adev: &mut amdgpu_device) -> i32 {
    smu_cmn_send_smc_msg_with_param(adev, SMU_MSG_GfxDeviceDriverReset, SMU_RESET_MODE_2, GfxDeviceDriverReset_NAME)
}

/// `smu_mode2_reset`, swsmu_pm_funcs' `asic_reset_mode_2`.
fn smu_mode2_reset(adev: &mut amdgpu_device) -> i32 {
    if !adev.smu.pm_enabled {
        return -95;
    }
    // renoir_ppt_funcs: .mode2_reset = smu_v12_0_mode2_reset
    let ret = smu_v12_0_mode2_reset(adev);
    if ret != 0 {
        Line::new().s("Mode2 reset failed!").say();
    }
    ret
}

/// `amdgpu_dpm_mode2_reset` (pm/amdgpu_dpm.c).
pub fn amdgpu_dpm_mode2_reset(adev: &mut amdgpu_device) -> i32 {
    if adev.powerplay_pp_funcs.is_none() {
        return -2;
    }
    smu_mode2_reset(adev)
}
