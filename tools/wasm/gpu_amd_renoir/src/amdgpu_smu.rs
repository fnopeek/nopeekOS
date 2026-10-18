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
    smu.watermarks_bitmap = 0;
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
