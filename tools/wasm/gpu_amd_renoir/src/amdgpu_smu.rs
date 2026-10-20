//! pm/swsmu/amdgpu_smu.c, smu12/renoir_ppt.c and smu12/smu_v12_0.c (Linux
//! 6.18.55): the SMU block's early_init, which picks renoir_ppt for MP1
//! 12.0.0, and its sw_init.

use crate::amdgpu_device::amdgpu_device;
use crate::linux::Line;
use crate::regs::*;
use alloc::vec;
use alloc::vec::Vec;

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
    pub pool_size: u64,
    pub feature_num: u32,
    pub watermarks_bitmap: u32,
    pub workload_refcount: [u32; PP_SMC_POWER_PROFILE_COUNT as usize],
    pub dpm_level: u32,
    pub requested_dpm_level: u32,
    pub smu_table: smu_table_context,
    /// `smu_feature.supported` / `.allowed`: SMU_FEATURE_MAX is 64.
    pub feature_supported: u64,
    pub feature_allowed: u64,
    /// `smu_power.power_gate`.
    pub vcn_gated: [bool; AMDGPU_MAX_VCN_INSTANCES as usize],
    pub jpeg_gated: bool,
    pub vpe_gated: bool,
    pub isp_gated: bool,
    pub umsch_mm_gated: bool,
    pub smc_fw_if_version: u32,
    pub smc_fw_version: u32,
    pub wbrf_supported: bool,
    pub workload_mask: u32,
    pub gfx_default_hard_min_freq: u32,
    pub gfx_default_soft_max_freq: u32,
    pub gfx_actual_hard_min_freq: u32,
    pub gfx_actual_soft_max_freq: u32,
}

/// `struct smu_table`.
#[derive(Default, Clone, Copy)]
pub struct smu_table {
    pub size: u64,
    pub align: u32,
    pub domain: u32,
    pub bo: Option<usize>,
    pub mc_address: u64,
    pub cpu_addr: Option<crate::amdgpu_object::amdgpu_kptr>,
}

/// `struct smu_bios_boot_up_values`.
#[derive(Default)]
pub struct smu_bios_boot_up_values {
    pub revision: u32,
    pub gfxclk: u32,
    pub uclk: u32,
    pub socclk: u32,
    pub dcefclk: u32,
    pub eclk: u32,
    pub vclk: u32,
    pub dclk: u32,
    pub vddc: u16,
    pub vddci: u16,
    pub mvddc: u16,
    pub vdd_gfx: u16,
    pub cooling_id: u8,
    pub pp_table_id: u32,
    pub format_revision: u32,
    pub content_revision: u32,
    pub fclk: u32,
    pub lclk: u32,
    pub firmware_caps: u32,
}

/// `struct smu_table_context`, the part sw_init fills.
#[derive(Default)]
pub struct smu_table_context {
    pub tables: [smu_table; SMU_TABLE_COUNT as usize],
    pub driver_table: smu_table,
    pub memory_pool: smu_table,
    pub dummy_read_1_table: smu_table,
    pub clocks_table: Vec<u8>,
    pub metrics_table: Vec<u8>,
    pub metrics_time: u64,
    pub watermarks_table: Vec<u8>,
    pub gpu_metrics_table_size: u32,
    pub gpu_metrics_table: Vec<u8>,
    pub boot_values: smu_bios_boot_up_values,
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

/// `smu_cmn_send_smc_msg_with_param` without a read-back argument.
pub fn smu_cmn_send_smc_msg_with_param(adev: &mut amdgpu_device, msg: u32, param: u32, name: &[u8]) -> i32 {
    smu_cmn_send_smc_msg_with_param_read(adev, msg, param, name, None)
}

/// `smu_cmn_send_smc_msg`.
fn smu_cmn_send_smc_msg(adev: &mut amdgpu_device, msg: u32, name: &[u8], read_arg: Option<&mut u32>) -> i32 {
    smu_cmn_send_smc_msg_with_param_read(adev, msg, 0, name, read_arg)
}

/// `smu_cmn_send_smc_msg_with_param`. No RAS priority filter
/// (`smc_fw_caps` 0).
pub fn smu_cmn_send_smc_msg_with_param_read(adev: &mut amdgpu_device, msg: u32, param: u32, name: &[u8],
                                            read_arg: Option<&mut u32>) -> i32 {
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
    if let Some(arg) = read_arg {
        // smu_cmn_read_arg
        *arg = adev.rreg32(adev.smu.param_reg).unwrap_or(0);
        Line::new().s("smu send message: ").bytes(name).s("(").dec(index as u64).s(") param: 0x").hexw(param, 8)
            .s(", resp: 0x").hexw(reg, 8).s(", readval: 0x").hexw(*arg, 8).dbg();
    } else {
        Line::new().s("smu send message: ").bytes(name).s("(").dec(index as u64).s(") param: 0x").hexw(param, 8)
            .s(", resp: 0x").hexw(reg, 8).dbg();
    }
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

/// `SMU_TABLE_INIT`.
fn smu_table_init(tables: &mut [smu_table], id: u32, size: u32, align: u64, domain: u32) {
    let t = &mut tables[id as usize];
    t.size = size as u64;
    t.align = align as u32;
    t.domain = domain;
}

/// `renoir_init_smc_tables`.
fn renoir_init_smc_tables(adev: &mut amdgpu_device) -> i32 {
    let smu_table = &mut adev.smu.smu_table;
    let page = crate::amdgpu_object::PAGE_SIZE;
    smu_table_init(&mut smu_table.tables, SMU_TABLE_WATERMARKS, SIZEOF_Watermarks_t, page, AMDGPU_GEM_DOMAIN_VRAM);
    smu_table_init(&mut smu_table.tables, SMU_TABLE_DPMCLOCKS, SIZEOF_DpmClocks_t, page, AMDGPU_GEM_DOMAIN_VRAM);
    smu_table_init(&mut smu_table.tables, SMU_TABLE_SMU_METRICS, SIZEOF_SmuMetrics_t, page, AMDGPU_GEM_DOMAIN_VRAM);
    smu_table.clocks_table = vec![0; SIZEOF_DpmClocks_t as usize];
    smu_table.metrics_table = vec![0; SIZEOF_SmuMetrics_t as usize];
    smu_table.metrics_time = 0;
    smu_table.watermarks_table = vec![0; SIZEOF_Watermarks_t as usize];
    smu_table.gpu_metrics_table_size = SIZEOF_gpu_metrics_v2_2;
    smu_table.gpu_metrics_table = vec![0; SIZEOF_gpu_metrics_v2_2 as usize];
    0
}

/// One `amdgpu_bo_create_kernel` into a `struct smu_table`.
fn smu_table_create(adev: &mut amdgpu_device, t: smu_table) -> Result<smu_table, i32> {
    let mut t = t;
    let Some(h) = crate::amdgpu_object::amdgpu_bo_create_kernel(adev, t.size, t.align as u64, t.domain, true)? else {
        return Ok(t);
    };
    t.bo = Some(h);
    t.mc_address = crate::amdgpu_object::amdgpu_bo_gpu_offset(adev, h);
    t.cpu_addr = crate::amdgpu_object::bo(adev, h).kptr;
    Ok(t)
}

/// `smu_init_fb_allocations`.
fn smu_init_fb_allocations(adev: &mut amdgpu_device) -> i32 {
    let log = adev.smu.smu_table.tables[SMU_TABLE_PMSTATUSLOG as usize];
    // VRAM allocation for tool table
    if log.size != 0 {
        match smu_table_create(adev, log) {
            Ok(t) => adev.smu.smu_table.tables[SMU_TABLE_PMSTATUSLOG as usize] = t,
            Err(r) => {
                Line::new().s("VRAM allocation for tool table failed!").say();
                return r;
            }
        }
    }
    let smu_table = &mut adev.smu.smu_table;
    smu_table.driver_table.domain = AMDGPU_GEM_DOMAIN_VRAM | AMDGPU_GEM_DOMAIN_GTT;
    let mut max_table_size = 0;
    // VRAM allocation for driver table
    for i in 0..SMU_TABLE_COUNT as usize {
        let t = smu_table.tables[i];
        if t.size == 0 {
            continue;
        }
        // If one of the tables has VRAM domain restriction, keep it in VRAM
        if t.domain & (AMDGPU_GEM_DOMAIN_VRAM | AMDGPU_GEM_DOMAIN_GTT) == AMDGPU_GEM_DOMAIN_VRAM {
            smu_table.driver_table.domain = AMDGPU_GEM_DOMAIN_VRAM;
        }
        if i == SMU_TABLE_PMSTATUSLOG as usize {
            continue;
        }
        max_table_size = max_table_size.max(t.size);
    }
    smu_table.driver_table.size = max_table_size;
    smu_table.driver_table.align = crate::amdgpu_object::PAGE_SIZE as u32;
    let driver_table = smu_table.driver_table;
    match smu_table_create(adev, driver_table) {
        Ok(t) => {
            adev.smu.smu_table.driver_table = t;
            0
        }
        Err(r) => {
            Line::new().s("VRAM allocation for driver table failed!").say();
            let log = &mut adev.smu.smu_table.tables[SMU_TABLE_PMSTATUSLOG as usize];
            if log.mc_address != 0 {
                let mut bo = log.bo.take();
                log.mc_address = 0;
                log.cpu_addr = None;
                crate::amdgpu_object::amdgpu_bo_free_kernel(adev, &mut bo);
            }
            r
        }
    }
}

/// `smu_alloc_memory_pool`: `amdgpu_smu_memory_pool_size` is 0, which
/// `amdgpu_device_check_smu_prv_buffer_size` keeps.
fn smu_alloc_memory_pool(adev: &mut amdgpu_device) -> i32 {
    if adev.smu.pool_size == SMU_MEMORY_POOL_SIZE_ZERO as u64 {
        return 0;
    }
    Line::new().s("smu memory pool: not ported for a non-zero pool size").say();
    -22
}

/// `smu_alloc_dummy_read_table`: renoir_ppt sets no dummy read table.
fn smu_alloc_dummy_read_table(adev: &mut amdgpu_device) -> i32 {
    let t = adev.smu.smu_table.dummy_read_1_table;
    if t.size == 0 {
        return 0;
    }
    match smu_table_create(adev, t) {
        Ok(t) => {
            adev.smu.smu_table.dummy_read_1_table = t;
            0
        }
        Err(r) => {
            Line::new().s("VRAM allocation for dummy read table failed!").say();
            r
        }
    }
}

/// `smu_smc_table_sw_init`: renoir_ppt has no `init_power` and no
/// `i2c_init`.
fn smu_smc_table_sw_init(adev: &mut amdgpu_device) -> i32 {
    let r = renoir_init_smc_tables(adev);
    if r != 0 {
        Line::new().s("Failed to init smc tables!").say();
        return r;
    }
    let r = smu_init_fb_allocations(adev);
    if r != 0 {
        return r;
    }
    let r = smu_alloc_memory_pool(adev);
    if r != 0 {
        return r;
    }
    smu_alloc_dummy_read_table(adev)
}

/// The register accessors of `struct card_info` (amdgpu_atombios.c
/// `cail_reg_read`/`cail_reg_write`).
impl crate::atom_exec::atom_card for amdgpu_device {
    fn reg_read(&self, reg: u32) -> u32 {
        self.rreg32(reg).unwrap_or(0)
    }

    fn reg_write(&self, reg: u32, val: u32) {
        self.wreg32(reg, val);
    }
}

/// `amdgpu_atom_execute_table` on `adev->mode_info.atom_context`.
pub fn amdgpu_atom_execute_table(adev: &mut amdgpu_device, index: u32, params: &mut [u32], params_size: usize) -> i32 {
    let Some(mut ctx) = adev.mode_info.atom_context.take() else { return -22 };
    let r = crate::atom_exec::amdgpu_atom_execute_table(&mut ctx, adev, index, params, params_size);
    adev.mode_info.atom_context = Some(ctx);
    r
}

/// `smu_v12_0_atom_get_smu_clockinfo`.
fn smu_v12_0_atom_get_smu_clockinfo(adev: &mut amdgpu_device, clk_id: u32, syspll_id: u32, clk_freq: &mut u32) -> i32 {
    // struct atom_get_smu_clock_info_parameters_v3_1
    let input = [syspll_id as u8, clk_id as u8, GET_SMU_CLOCK_INFO_V3_1_GET_CLOCK_FREQ as u8, 0];
    let mut ps = [u32::from_le_bytes(input)];
    if amdgpu_atom_execute_table(adev, IDX_getsmuclockinfo, &mut ps, 4) != 0 {
        return -22;
    }
    // atom_get_smu_clock_info_output_parameters_v3_1
    *clk_freq = ps[0] / 10000;
    0
}

/// `smu_v12_0_get_vbios_bootup_values`.
fn smu_v12_0_get_vbios_bootup_values(adev: &mut amdgpu_device) -> i32 {
    let Some(ctx) = adev.mode_info.atom_context.as_ref() else { return -22 };
    let Some(h) = crate::atom::amdgpu_atom_parse_data_header(ctx, IDX_firmwareinfo_v2_1) else { return -22 };
    let b = h.data_start as u32;
    let format_revision = ctx.cu8(b + ATH_FORMAT_REVISION) as u32;
    let content_revision = ctx.cu8(b + ATH_CONTENT_REVISION) as u32;
    if format_revision != 3 {
        Line::new().s("unknown atom_firmware_info version! for smu12").say();
        return -22;
    }
    let v = &mut adev.smu.smu_table.boot_values;
    match content_revision {
        0..=2 => {
            v.revision = ctx.cu32(b + FWI31_SMU_FIRMWARE_REVISION);
            v.gfxclk = ctx.cu32(b + FWI31_SMU_BOOTUP_SCLK_IN10KHZ);
            v.uclk = ctx.cu32(b + FWI31_SMU_BOOTUP_MCLK_IN10KHZ);
            v.socclk = 0;
            v.dcefclk = 0;
            v.vddc = ctx.cu16(b + FWI31_SMU_BOOTUP_VDDC_MV);
            v.vddci = ctx.cu16(b + FWI31_SMU_BOOTUP_VDDCI_MV);
            v.mvddc = ctx.cu16(b + FWI31_SMU_BOOTUP_MVDDC_MV);
            v.vdd_gfx = ctx.cu16(b + FWI31_SMU_BOOTUP_VDDGFX_MV);
            v.cooling_id = ctx.cu8(b + FWI31_SMU_COOLINGSOLUTION_ID);
            v.pp_table_id = 0;
            v.firmware_caps = ctx.cu32(b + FWI31_SMU_FIRMWARE_CAPABILITY);
        }
        _ => {
            v.revision = ctx.cu32(b + FWI33_SMU_FIRMWARE_REVISION);
            v.gfxclk = ctx.cu32(b + FWI33_SMU_BOOTUP_SCLK_IN10KHZ);
            v.uclk = ctx.cu32(b + FWI33_SMU_BOOTUP_MCLK_IN10KHZ);
            v.socclk = 0;
            v.dcefclk = 0;
            v.vddc = ctx.cu16(b + FWI33_SMU_BOOTUP_VDDC_MV);
            v.vddci = ctx.cu16(b + FWI33_SMU_BOOTUP_VDDCI_MV);
            v.mvddc = ctx.cu16(b + FWI33_SMU_BOOTUP_MVDDC_MV);
            v.vdd_gfx = ctx.cu16(b + FWI33_SMU_BOOTUP_VDDGFX_MV);
            v.cooling_id = ctx.cu8(b + FWI33_SMU_COOLINGSOLUTION_ID);
            v.pp_table_id = ctx.cu32(b + FWI33_SMU_PPLIB_PPTABLE_ID);
            v.firmware_caps = ctx.cu32(b + FWI33_SMU_FIRMWARE_CAPABILITY);
        }
    }
    v.format_revision = format_revision;
    v.content_revision = content_revision;

    let mut f = 0;
    smu_v12_0_atom_get_smu_clockinfo(adev, SMU12_SYSPLL0_SOCCLK_ID, SMU12_SYSPLL0_ID, &mut f);
    adev.smu.smu_table.boot_values.socclk = f;
    let mut f = 0;
    smu_v12_0_atom_get_smu_clockinfo(adev, SMU12_SYSPLL1_DCFCLK_ID, SMU12_SYSPLL1_ID, &mut f);
    adev.smu.smu_table.boot_values.dcefclk = f;
    let mut f = 0;
    smu_v12_0_atom_get_smu_clockinfo(adev, SMU12_SYSPLL0_VCLK_ID, SMU12_SYSPLL0_ID, &mut f);
    adev.smu.smu_table.boot_values.vclk = f;
    let mut f = 0;
    smu_v12_0_atom_get_smu_clockinfo(adev, SMU12_SYSPLL0_DCLK_ID, SMU12_SYSPLL0_ID, &mut f);
    adev.smu.smu_table.boot_values.dclk = f;
    let v = &adev.smu.smu_table.boot_values;
    if v.format_revision == 3 && v.content_revision >= 2 {
        let mut f = 0;
        smu_v12_0_atom_get_smu_clockinfo(adev, SMU12_SYSPLL3_0_FCLK_ID, SMU12_SYSPLL3_0_ID, &mut f);
        adev.smu.smu_table.boot_values.fclk = f;
    }
    0
}

/// `smu_sw_init`. renoir_ppt has no `init_pptable_microcode`, no
/// `register_irq_handler` and no `get_fan_control_mode`.
pub fn smu_sw_init(adev: &mut amdgpu_device) -> i32 {
    let smu = &mut adev.smu;
    smu.pool_size = adev.pm.smu_prv_buffer_size as u64;
    smu.feature_num = SMU_FEATURE_MAX;
    smu.feature_supported = 0;
    smu.feature_allowed = 0;
    smu.watermarks_bitmap = 0;
    for i in 0..adev.vcn.num_vcn_inst as usize {
        smu.vcn_gated[i] = true;
    }
    smu.jpeg_gated = true;
    smu.vpe_gated = true;
    smu.isp_gated = true;
    smu.umsch_mm_gated = true;
    // smu_init_power_profile
    if smu.power_profile_mode == PP_SMC_POWER_PROFILE_UNKNOWN as u32 {
        smu.power_profile_mode = PP_SMC_POWER_PROFILE_BOOTUP_DEFAULT;
    }
    smu.workload_refcount[smu.power_profile_mode as usize] += 1;
    smu.dpm_level = AMD_DPM_FORCED_LEVEL_AUTO;
    smu.requested_dpm_level = AMD_DPM_FORCED_LEVEL_AUTO;

    let r = smu_smc_table_sw_init(adev);
    if r != 0 {
        Line::new().s("Failed to sw init smc table!").say();
        return r;
    }
    // get boot_values from vbios to set revision, gfxclk, and etc.
    let r = smu_v12_0_get_vbios_bootup_values(adev);
    if r != 0 {
        Line::new().s("Failed to get VBIOS boot clock values!").say();
        return r;
    }
    adev.pm.no_fan = true;
    0
}

/// `smu_cmn_to_asic_specific_index`, CMN2ASIC_MAPPING_TABLE: the ASIC's
/// table number from `renoir_table_map`.
fn smu_cmn_to_asic_specific_table(adev: &amdgpu_device, index: u32) -> i32 {
    if index >= SMU_TABLE_COUNT || adev.smu.table_map.is_none() {
        return -22;
    }
    let m = &crate::tables::renoir_table_map;
    let at = (index * CMN2ASIC_MAPPING_SIZE / 4) as usize;
    let field = |off: u32| m.get(at + (off / 4) as usize).copied().unwrap_or(0);
    if field(CMN2ASIC_MAPPING_valid_mapping) == 0 {
        return -22;
    }
    field(CMN2ASIC_MAPPING_map_to)
}

/// `renoir_get_enabled_mask`: the PMFW exports no feature mask to an APU,
/// so every feature counts as enabled.
fn renoir_get_enabled_mask(feature_mask: &mut u64) -> i32 {
    *feature_mask = u64::MAX;
    0
}

/// `smu_cmn_feature_is_enabled`: renoir has no feature map, and its mask
/// is all ones.
fn smu_cmn_feature_is_enabled(_adev: &amdgpu_device, _mask: u32) -> bool {
    let mut enabled_features = 0;
    // __smu_get_enabled_features
    if renoir_get_enabled_mask(&mut enabled_features) != 0 {
        Line::new().s("Failed to retrieve enabled ppfeatures!").say();
        return false;
    }
    // For Renoir and Cyan Skillfish, they are assumed to have all features
    // enabled. Also considering they have no feature_map available, the
    // check here can avoid unwanted feature_map check below.
    if enabled_features == u64::MAX {
        return true;
    }
    // smu_cmn_to_asic_specific_index(CMN2ASIC_MAPPING_FEATURE): no
    // feature_map on renoir.
    false
}

/// `smu_v12_0_check_fw_status`.
fn smu_v12_0_check_fw_status(adev: &amdgpu_device) -> i32 {
    let mp1_fw_flags =
        crate::amdgpu_device::amdgpu_device_indirect_rreg(adev, MP1_Public | (smnMP1_FIRMWARE_FLAGS & 0xffffffff))
            .unwrap_or(0);
    if (mp1_fw_flags & MP1_FIRMWARE_FLAGS__INTERRUPTS_ENABLED_MASK) >> MP1_FIRMWARE_FLAGS__INTERRUPTS_ENABLED__SHIFT
        != 0
    {
        return 0;
    }
    -5
}

/// `smu_cmn_get_smc_version`.
fn smu_cmn_get_smc_version(adev: &mut amdgpu_device, if_version: &mut u32, smu_version: &mut u32) -> i32 {
    if adev.smu.smc_fw_if_version != 0 && adev.smu.smc_fw_version != 0 {
        *if_version = adev.smu.smc_fw_if_version;
        *smu_version = adev.smu.smc_fw_version;
        return 0;
    }
    let r = smu_cmn_send_smc_msg(adev, SMU_MSG_GetDriverIfVersion, GetDriverIfVersion_NAME, Some(if_version));
    if r != 0 {
        return r;
    }
    adev.smu.smc_fw_if_version = *if_version;
    let r = smu_cmn_send_smc_msg(adev, SMU_MSG_GetSmuVersion, GetSmuVersion_NAME, Some(smu_version));
    if r != 0 {
        return r;
    }
    adev.smu.smc_fw_version = *smu_version;
    0
}

/// `smu_v12_0_check_fw_version`.
fn smu_v12_0_check_fw_version(adev: &mut amdgpu_device) -> i32 {
    let mut if_version = 0xff;
    let mut smu_version = 0xff;
    let r = smu_cmn_get_smc_version(adev, &mut if_version, &mut smu_version);
    if r != 0 {
        return r;
    }
    let smu_program = (smu_version >> 24) & 0xff;
    let smu_major = (smu_version >> 16) & 0xff;
    let smu_minor = (smu_version >> 8) & 0xff;
    let smu_debug = smu_version & 0xff;
    if adev.smu.is_apu {
        adev.pm.fw_version = smu_version;
    }
    // 1. if_version mismatch is not critical as our fw is designed to be
    // backward compatible.
    // 2. New fw usually brings some optimizations. But that's visible only
    // on the paired driver.
    // Considering above, we just leave user a verbal message instead of
    // halt driver loading.
    if if_version != adev.smu.smc_driver_if_version {
        Line::new().s("smu driver if version = 0x").hexw(adev.smu.smc_driver_if_version, 8)
            .s(", smu fw if version = 0x").hexw(if_version, 8).s(", smu fw program = ").dec(smu_program as u64)
            .s(", smu fw version = 0x").hexw(smu_version, 8).s(" (").dec(smu_major as u64).s(".")
            .dec(smu_minor as u64).s(".").dec(smu_debug as u64).s(")").say();
    }
    0
}

/// `smu_start_smc_engine`: the firmware is loaded by the PSP, and renoir
/// has `check_fw_status` and `check_fw_version`.
fn smu_start_smc_engine(adev: &mut amdgpu_device) -> i32 {
    adev.smu.smc_fw_state = SMU_FW_INIT;
    if smu_v12_0_check_fw_status(adev) != 0 {
        Line::new().s("SMC is not ready").say();
        return -5;
    }
    // Send msg GetDriverIfVersion to check if the return value is equal
    // with DRIVER_IF_VERSION of smc header.
    smu_v12_0_check_fw_version(adev)
}

/// `is_vcn_enabled`.
fn is_vcn_enabled(adev: &amdgpu_device) -> bool {
    !adev.ip_blocks.iter().any(|b| {
        (b.version.type_ == AMD_IP_BLOCK_TYPE_VCN || b.version.type_ == AMD_IP_BLOCK_TYPE_JPEG) && !b.status.valid
    })
}

/// `renoir_dpm_set_vcn_enable`.
fn renoir_dpm_set_vcn_enable(adev: &mut amdgpu_device, enable: bool) -> i32 {
    if enable {
        // vcn dpm on is a prerequisite for vcn power gate messages
        if smu_cmn_feature_is_enabled(adev, SMU_FEATURE_VCN_PG_BIT) {
            return smu_cmn_send_smc_msg_with_param(adev, SMU_MSG_PowerUpVcn, 0, PowerUpVcn_NAME);
        }
    } else if smu_cmn_feature_is_enabled(adev, SMU_FEATURE_VCN_PG_BIT) {
        return smu_cmn_send_smc_msg(adev, SMU_MSG_PowerDownVcn, PowerDownVcn_NAME, None);
    }
    0
}

/// `renoir_dpm_set_jpeg_enable`.
fn renoir_dpm_set_jpeg_enable(adev: &mut amdgpu_device, enable: bool) -> i32 {
    if enable {
        if smu_cmn_feature_is_enabled(adev, SMU_FEATURE_JPEG_PG_BIT) {
            return smu_cmn_send_smc_msg_with_param(adev, SMU_MSG_PowerUpJpeg, 0, PowerUpJpeg_NAME);
        }
    } else if smu_cmn_feature_is_enabled(adev, SMU_FEATURE_JPEG_PG_BIT) {
        return smu_cmn_send_smc_msg_with_param(adev, SMU_MSG_PowerDownJpeg, 0, PowerDownJpeg_NAME);
    }
    0
}

/// `smu_dpm_set_vcn_enable`.
fn smu_dpm_set_vcn_enable(adev: &mut amdgpu_device, enable: bool, inst: usize) -> i32 {
    // don't poweron vcn/jpeg when they are skipped.
    if !is_vcn_enabled(adev) {
        return 0;
    }
    if adev.smu.vcn_gated[inst] ^ enable {
        return 0;
    }
    let r = renoir_dpm_set_vcn_enable(adev, enable);
    if r == 0 {
        adev.smu.vcn_gated[inst] = !enable;
    }
    r
}

/// `smu_dpm_set_jpeg_enable`.
fn smu_dpm_set_jpeg_enable(adev: &mut amdgpu_device, enable: bool) -> i32 {
    if !is_vcn_enabled(adev) {
        return 0;
    }
    if adev.smu.jpeg_gated ^ enable {
        return 0;
    }
    let r = renoir_dpm_set_jpeg_enable(adev, enable);
    if r == 0 {
        adev.smu.jpeg_gated = !enable;
    }
    r
}

/// `smu_v12_0_set_gfx_cgpg`.
fn smu_v12_0_set_gfx_cgpg(adev: &mut amdgpu_device, enable: bool) -> i32 {
    // Until now the SMU12 only implemented for Renoir series so here neen't
    // do APU check.
    if adev.pg_flags & AMD_PG_SUPPORT_GFX_PG == 0 || adev.in_s0ix {
        return 0;
    }
    smu_cmn_send_smc_msg_with_param(adev, SMU_MSG_SetGfxCGPG, enable as u32, SetGfxCGPG_NAME)
}

/// `smu_v12_0_set_driver_table_location`.
fn smu_v12_0_set_driver_table_location(adev: &mut amdgpu_device) -> i32 {
    let mc_address = adev.smu.smu_table.driver_table.mc_address;
    let mut r = 0;
    if mc_address != 0 {
        r = smu_cmn_send_smc_msg_with_param(adev, SMU_MSG_SetDriverDramAddrHigh, (mc_address >> 32) as u32,
                                            SetDriverDramAddrHigh_NAME);
        if r == 0 {
            r = smu_cmn_send_smc_msg_with_param(adev, SMU_MSG_SetDriverDramAddrLow, mc_address as u32,
                                                SetDriverDramAddrLow_NAME);
        }
    }
    r
}

/// `hdp_v4_0_invalidate_hdp` without a ring.
pub fn hdp_v4_0_invalidate_hdp(adev: &amdgpu_device) {
    let v = adev.ip_version(HDP_HWIP, 0);
    if v == 4 << 24 | 4 << 16 || v == 4 << 24 | 4 << 16 | 2 << 8 || v == 4 << 24 | 4 << 16 | 5 << 8 {
        return;
    }
    crate::nbio_v7_0::wreg32_soc15(adev, HDP_HWIP, 0, HDP_mmHDP_READ_CACHE_INVALIDATE_BASE_IDX,
                                   HDP_mmHDP_READ_CACHE_INVALIDATE, 1);
    crate::nbio_v7_0::rreg32_soc15(adev, HDP_HWIP, 0, HDP_mmHDP_READ_CACHE_INVALIDATE_BASE_IDX,
                                   HDP_mmHDP_READ_CACHE_INVALIDATE);
}

/// `smu_cmn_update_table` from the SMU to the driver (`drv2smu` false):
/// soc15_asic_funcs has no `invalidate_hdp`, so it is the HDP block's.
fn smu_cmn_update_table_smu2drv(adev: &mut amdgpu_device, table_index: u32, argument: u32) -> Result<Vec<u8>, i32> {
    let table_id = smu_cmn_to_asic_specific_table(adev, table_index);
    if table_index >= SMU_TABLE_COUNT || table_id < 0 {
        return Err(-22);
    }
    let table_size = adev.smu.smu_table.tables[table_index as usize].size as usize;
    let r = smu_cmn_send_smc_msg_with_param(adev, SMU_MSG_TransferTableSmu2Dram,
                                            table_id as u32 | ((argument & 0xFFFF) << 16),
                                            TransferTableSmu2Dram_NAME);
    if r != 0 {
        return Err(r);
    }
    hdp_v4_0_invalidate_hdp(adev);
    let Some(k) = adev.smu.smu_table.driver_table.cpu_addr else { return Err(-22) };
    let mut data = vec![0u8; table_size];
    crate::amdgpu_gart::kptr_read(adev, k, 0, &mut data);
    Ok(data)
}

/// `smu_v12_0_set_default_dpm_tables`: SMU_TABLE_DPMCLOCKS into
/// `clocks_table`.
fn smu_v12_0_set_default_dpm_tables(adev: &mut amdgpu_device) -> i32 {
    match smu_cmn_update_table_smu2drv(adev, SMU_TABLE_DPMCLOCKS, 0) {
        Ok(data) => {
            let n = data.len().min(adev.smu.smu_table.clocks_table.len());
            adev.smu.smu_table.clocks_table[..n].copy_from_slice(&data[..n]);
            0
        }
        Err(r) => r,
    }
}

/// `smu_set_default_dpm_table`.
fn smu_set_default_dpm_table(adev: &mut amdgpu_device) -> i32 {
    let n = adev.vcn.num_vcn_inst as usize;
    let mut vcn_gate = [false; AMDGPU_MAX_VCN_INSTANCES as usize];
    let mut jpeg_gate = false;
    if adev.pg_flags & AMD_PG_SUPPORT_VCN != 0 {
        vcn_gate[..n].copy_from_slice(&adev.smu.vcn_gated[..n]);
    }
    if adev.pg_flags & AMD_PG_SUPPORT_JPEG != 0 {
        jpeg_gate = adev.smu.jpeg_gated;
    }
    if adev.pg_flags & AMD_PG_SUPPORT_VCN != 0 {
        for i in 0..n {
            let r = smu_dpm_set_vcn_enable(adev, true, i);
            if r != 0 {
                return r;
            }
        }
    }
    let mut r;
    'err_out: {
        if adev.pg_flags & AMD_PG_SUPPORT_JPEG != 0 {
            r = smu_dpm_set_jpeg_enable(adev, true);
            if r != 0 {
                break 'err_out;
            }
        }
        r = smu_v12_0_set_default_dpm_tables(adev);
        if r != 0 {
            Line::new().s("Failed to setup default dpm clock tables!").say();
        }
        if adev.pg_flags & AMD_PG_SUPPORT_JPEG != 0 {
            smu_dpm_set_jpeg_enable(adev, !jpeg_gate);
        }
    }
    if adev.pg_flags & AMD_PG_SUPPORT_VCN != 0 {
        for i in 0..n {
            smu_dpm_set_vcn_enable(adev, !vcn_gate[i], i);
        }
    }
    r
}

/// `smu_smc_hw_setup` for renoir: of the ppt hooks it calls, renoir has
/// `set_driver_table_location`, `get_enabled_mask`, `is_dpm_running` and
/// `set_default_dpm_table`; the others return 0. SCPM is off, there is no
/// wbrf, and MP1 12.0.0 has no XGMI PLPD policy.
fn smu_smc_hw_setup(adev: &mut amdgpu_device) -> i32 {
    let r = smu_v12_0_set_driver_table_location(adev);
    if r != 0 {
        Line::new().s("Failed to SetDriverDramAddr!").say();
        return r;
    }
    // smu_update_pcie_parameters: renoir has no update_pcie_parameters, so
    // the gen and width it would be given are not needed.

    // smu_init_xgmi_plpd_mode: no policies on renoir.

    let mut features_supported = 0;
    let r = renoir_get_enabled_mask(&mut features_supported);
    if r != 0 {
        Line::new().s("Failed to retrieve supported dpm features!").say();
        return r;
    }
    adev.smu.feature_supported = features_supported;

    // renoir_is_dpm_running: true outside suspend.
    if adev.in_suspend {
        Line::new().s("dpm has been disabled").say();
    }

    // Set initialized values (get from vbios) to dpm tables context such as
    // gfxclk, memclk, dcefclk, and etc. And enable the DPM feature for each
    // type of clks.
    let r = smu_set_default_dpm_table(adev);
    if r != 0 {
        Line::new().s("Failed to setup default dpm clock tables!").say();
        return r;
    }
    // smu_get_thermal_temperature_range, smu_enable_thermal_alert,
    // smu_notify_display_change, smu_set_min_dcef_deep_sleep: renoir has
    // none of them; smu_wbrf_init returns 0 without wbrf.
    0
}

/// `smu_hw_init`: bare metal, no SR-IOV.
pub fn smu_hw_init(adev: &mut amdgpu_device) -> i32 {
    let r = smu_start_smc_engine(adev);
    if r != 0 {
        Line::new().s("SMC engine is not correctly up!").say();
        return r;
    }
    // smu_wbrf_support_check: renoir has no is_asic_wbrf_supported.
    adev.smu.wbrf_supported = false;

    if adev.smu.is_apu {
        // smu_set_gfx_imu_enable: renoir has no set_gfx_power_up_by_imu.
        for i in 0..adev.vcn.num_vcn_inst as usize {
            smu_dpm_set_vcn_enable(adev, true, i);
        }
        smu_dpm_set_jpeg_enable(adev, true);
        // smu_dpm_set_umsch_mm_enable: enable_umsch_mm is off;
        // smu_set_mall_enable: renoir has no set_mall_enable.
        smu_v12_0_set_gfx_cgpg(adev, true);
    }

    if !adev.smu.pm_enabled {
        return 0;
    }

    // smu_get_driver_allowed_feature_mask: SCPM is off, renoir has no
    // get_allowed_feature_mask.
    adev.smu.feature_allowed = 0;

    let r = smu_smc_hw_setup(adev);
    if r != 0 {
        Line::new().s("Failed to setup smc hw!").say();
        return r;
    }
    // smu_init_max_sustainable_clocks: renoir has none.
    adev.pm.dpm_enabled = true;
    Line::new().s("SMU is initialized successfully!").say();
    0
}

/// `smu_v12_0_powergate_sdma`.
fn smu_v12_0_powergate_sdma(adev: &mut amdgpu_device, gate: bool) -> i32 {
    if !adev.smu.is_apu {
        return 0;
    }
    if gate {
        smu_cmn_send_smc_msg(adev, SMU_MSG_PowerDownSdma, PowerDownSdma_NAME, None)
    } else {
        smu_cmn_send_smc_msg(adev, SMU_MSG_PowerUpSdma, PowerUpSdma_NAME, None)
    }
}

/// `smu_dpm_set_power_gate` (swsmu's `set_powergating_by_smu`), for the
/// blocks Renoir's hw_init gates.
fn smu_dpm_set_power_gate(adev: &mut amdgpu_device, block_type: u32, gate: bool, inst: usize) -> i32 {
    if !adev.smu.pm_enabled || !adev.pm.dpm_enabled {
        Line::new().s("SMU uninitialized but power ").s(if gate { "gate" } else { "ungate" })
            .s(" requested for ").dec(block_type as u64).s("!").say();
        return -95;
    }
    let what = if gate { "gate" } else { "ungate" };
    match block_type {
        // Some legacy code of amdgpu_vcn.c and vcn_v2*.c still uses
        // AMD_IP_BLOCK_TYPE_UVD for VCN. So, here both of them are kept.
        AMD_IP_BLOCK_TYPE_UVD | AMD_IP_BLOCK_TYPE_VCN => {
            let r = smu_dpm_set_vcn_enable(adev, !gate, inst);
            if r != 0 {
                Line::new().s("Failed to power ").s(what).s(" VCN instance ").dec(inst as u64).s("!").say();
            }
            r
        }
        AMD_IP_BLOCK_TYPE_SDMA => {
            let r = smu_v12_0_powergate_sdma(adev, gate);
            if r != 0 {
                Line::new().s("Failed to power ").s(what).s(" SDMA!").say();
            }
            r
        }
        AMD_IP_BLOCK_TYPE_GFX => {
            let r = smu_v12_0_gfx_off_control(adev, gate);
            if r != 0 {
                Line::new().s("Failed to ").s(if gate { "enable" } else { "disable" }).s(" gfxoff!").say();
            }
            r
        }
        AMD_IP_BLOCK_TYPE_JPEG => {
            let r = smu_dpm_set_jpeg_enable(adev, !gate);
            if r != 0 {
                Line::new().s("Failed to power ").s(what).s(" JPEG!").say();
            }
            r
        }
        // smu_dpm_set_vpe_enable, smu_dpm_set_isp_enable: renoir has
        // neither hook and returns 0.
        AMD_IP_BLOCK_TYPE_VPE | AMD_IP_BLOCK_TYPE_ISP => 0,
        _ => {
            Line::new().s("Unsupported block type!").say();
            -22
        }
    }
}

/// `amdgpu_dpm_set_powergating_by_smu`.
pub fn amdgpu_dpm_set_powergating_by_smu(adev: &mut amdgpu_device, block_type: u32, gate: bool, inst: usize) -> i32 {
    let pwr_state = if gate { POWER_STATE_OFF } else { POWER_STATE_ON };
    let is_vcn = block_type == AMD_IP_BLOCK_TYPE_VCN;
    if adev.pm.pwr_state[block_type as usize] == pwr_state && (!is_vcn || adev.vcn.num_vcn_inst == 1) {
        Line::new().s("IP block").dec(block_type as u64).s(" already in the target ")
            .s(if gate { "gate" } else { "ungate" }).s(" state!").dbg();
        return 0;
    }
    let r = match block_type {
        AMD_IP_BLOCK_TYPE_VCN => smu_dpm_set_power_gate(adev, block_type, gate, inst),
        AMD_IP_BLOCK_TYPE_UVD | AMD_IP_BLOCK_TYPE_VCE | AMD_IP_BLOCK_TYPE_GFX | AMD_IP_BLOCK_TYPE_SDMA
        | AMD_IP_BLOCK_TYPE_JPEG | AMD_IP_BLOCK_TYPE_GMC | AMD_IP_BLOCK_TYPE_ACP | AMD_IP_BLOCK_TYPE_VPE
        | AMD_IP_BLOCK_TYPE_ISP => smu_dpm_set_power_gate(adev, block_type, gate, 0),
        _ => 0,
    };
    if r == 0 {
        adev.pm.pwr_state[block_type as usize] = pwr_state;
    }
    r
}

/// `smu_v12_0_get_gfxoff_status`.
fn smu_v12_0_get_gfxoff_status(adev: &amdgpu_device) -> u32 {
    let reg = crate::nbio_v7_0::rreg32_soc15(adev, SMUIO_HWIP, 0, SMUIO_mmSMUIO_GFX_MISC_CNTL_BASE_IDX,
                                             SMUIO_mmSMUIO_GFX_MISC_CNTL).unwrap_or(0);
    (reg & SMUIO_SMUIO_GFX_MISC_CNTL__PWR_GFXOFF_STATUS_MASK) >> SMUIO_SMUIO_GFX_MISC_CNTL__PWR_GFXOFF_STATUS__SHIFT
}

/// `smu_v12_0_gfx_off_control` (`smu_gfx_off_control`).
fn smu_v12_0_gfx_off_control(adev: &mut amdgpu_device, enable: bool) -> i32 {
    if enable {
        return smu_cmn_send_smc_msg(adev, SMU_MSG_AllowGfxOff, AllowGfxOff_NAME, None);
    }
    let r = smu_cmn_send_smc_msg(adev, SMU_MSG_DisallowGfxOff, DisallowGfxOff_NAME, None);
    // confirm gfx is back to "on" state, timeout is 0.5 second
    let mut timeout = 500;
    while smu_v12_0_get_gfxoff_status(adev) != 2 {
        crate::linux::mdelay(1);
        timeout -= 1;
        if timeout == 0 {
            Line::new().s("disable gfxoff timeout and failed!").say();
            break;
        }
    }
    r
}

/// `smu_cmn_to_asic_specific_index`, CMN2ASIC_MAPPING_WORKLOAD:
/// `renoir_workload_map`.
fn smu_cmn_to_asic_specific_workload(adev: &amdgpu_device, index: u32) -> i32 {
    if index >= PP_SMC_POWER_PROFILE_COUNT || adev.smu.workload_map.is_none() {
        return -22;
    }
    let m = &crate::tables::renoir_workload_map;
    let at = (index * CMN2ASIC_MAPPING_SIZE / 4) as usize;
    let field = |off: u32| m.get(at + (off / 4) as usize).copied().unwrap_or(0);
    if field(CMN2ASIC_MAPPING_valid_mapping) == 0 {
        return -524;
    }
    field(CMN2ASIC_MAPPING_map_to)
}

/// `renoir_set_power_profile_mode` with
/// `smu_cmn_get_backend_workload_mask`.
fn renoir_set_power_profile_mode(adev: &mut amdgpu_device, workload_mask: u32) -> i32 {
    let mut backend_workload_mask = 0u32;
    for profile_mode in 0..PP_SMC_POWER_PROFILE_COUNT {
        if workload_mask & (1 << profile_mode) == 0 {
            continue;
        }
        // conv PP_SMC_POWER_PROFILE* to WORKLOAD_PPLIB_*_BIT
        let workload_type = smu_cmn_to_asic_specific_workload(adev, profile_mode);
        if workload_type < 0 {
            continue;
        }
        backend_workload_mask |= 1 << workload_type;
    }
    let r = smu_cmn_send_smc_msg_with_param(adev, SMU_MSG_ActiveProcessNotify, backend_workload_mask,
                                            ActiveProcessNotify_NAME);
    if r != 0 {
        Line::new().s("Failed to set workload mask 0x08").hexu(workload_mask).say();
    }
    r
}

/// `smu_bump_power_profile_mode` without custom parameters.
fn smu_bump_power_profile_mode(adev: &mut amdgpu_device) -> i32 {
    let mut workload_mask = 0u32;
    for i in 0..PP_SMC_POWER_PROFILE_COUNT as usize {
        if adev.smu.workload_refcount[i] != 0 {
            workload_mask |= 1 << i;
        }
    }
    if adev.smu.workload_mask == workload_mask {
        return 0;
    }
    let r = renoir_set_power_profile_mode(adev, workload_mask);
    if r == 0 {
        adev.smu.workload_mask = workload_mask;
    }
    r
}

/// `renoir_set_fine_grain_gfx_freq_parameters`.
fn renoir_set_fine_grain_gfx_freq_parameters(adev: &mut amdgpu_device) -> i32 {
    let (mut min, mut max) = (0, 0);
    let r = smu_cmn_send_smc_msg_with_param_read(adev, SMU_MSG_GetMinGfxclkFrequency, 0, GetMinGfxclkFrequency_NAME,
                                                 Some(&mut min));
    if r != 0 {
        return r;
    }
    let r = smu_cmn_send_smc_msg_with_param_read(adev, SMU_MSG_GetMaxGfxclkFrequency, 0, GetMaxGfxclkFrequency_NAME,
                                                 Some(&mut max));
    if r != 0 {
        return r;
    }
    let smu = &mut adev.smu;
    smu.gfx_default_hard_min_freq = min;
    smu.gfx_default_soft_max_freq = max;
    smu.gfx_actual_hard_min_freq = 0;
    smu.gfx_actual_soft_max_freq = 0;
    0
}

/// `smu_late_init` for renoir: of the hooks it calls, renoir has
/// `set_fine_grain_gfx_freq_parameters` and `set_power_profile_mode`;
/// `smu_set_ac_dc` finds no `set_power_source`, and the restore of a user
/// profile is for resume only.
pub fn smu_late_init(adev: &mut amdgpu_device) -> i32 {
    renoir_set_fine_grain_gfx_freq_parameters(adev);
    if !adev.smu.pm_enabled {
        return 0;
    }
    // smu_handle_task(dpm_level, AMD_PP_TASK_COMPLETE_INIT) ->
    // smu_adjust_power_state_dynamic(dpm_level, skip display settings):
    // renoir has no apply_clocks_adjust_rules, and the level asked for is
    // the current one.
    if !adev.pm.dpm_enabled {
        return 0;
    }
    let level = adev.smu.dpm_level;
    if level != AMD_DPM_FORCED_LEVEL_MANUAL && level != AMD_DPM_FORCED_LEVEL_PERF_DETERMINISM {
        smu_bump_power_profile_mode(adev);
    }
    0
}

/// `smu_hw_fini`: no SR-IOV. `smu_smc_hw_cleanup` finds nothing renoir
/// implements (no thermal alert, no `system_features_control`, no RLC
/// notify), and `smu_reset_mp1_state` is for MP1 13.0.10.
pub fn smu_hw_fini(adev: &mut amdgpu_device) -> i32 {
    for i in 0..adev.vcn.num_vcn_inst as usize {
        smu_dpm_set_vcn_enable(adev, false, i);
        adev.vcn.inst[i].cur_state = AMD_PG_STATE_GATE;
    }
    smu_dpm_set_jpeg_enable(adev, false);
    adev.jpeg.cur_state = AMD_PG_STATE_GATE;
    if !adev.smu.pm_enabled {
        return 0;
    }
    adev.pm.dpm_enabled = false;
    0
}
