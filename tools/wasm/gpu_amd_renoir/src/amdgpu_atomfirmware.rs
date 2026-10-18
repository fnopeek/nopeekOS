//! amdgpu/amdgpu_atomfirmware.c (Linux 6.18.55): the atomfirmware data
//! tables `amdgpu_atombios_init` and `amdgpu_device_init` read on an
//! `is_atom_fw` image. Renoir is bare metal (`amdgpu_sriov_vf` is 0) and
//! CHIP_RENOIR, below CHIP_NAVI10, so `get_clock_info` takes no `gfx_info`.

use crate::amdgpu_device::{amdgpu_device, amdgpu_pll};
use crate::atom::{amdgpu_atom_parse_data_header, atom_context};
use crate::linux::Line;
use crate::regs::*;
use alloc::vec;

/// `amdgpu_atomfirmware_query_firmware_capability`.
pub fn amdgpu_atomfirmware_query_firmware_capability(ctx: &atom_context) -> u32 {
    let mut fw_cap = 0;
    if let Some(h) = amdgpu_atom_parse_data_header(ctx, IDX_firmwareinfo) {
        // support firmware_info 3.1 +
        if (h.frev == 3 && h.crev >= 1) || h.frev > 3 {
            fw_cap = ctx.cu32(h.data_start as u32 + FWI31_FIRMWARE_CAPABILITY);
        }
    }
    fw_cap
}

/// `amdgpu_atomfirmware_scratch_regs_init`.
pub fn amdgpu_atomfirmware_scratch_regs_init(adev: &mut amdgpu_device) {
    let Some(ctx) = adev.mode_info.atom_context.as_ref() else { return };
    if let Some(h) = amdgpu_atom_parse_data_header(ctx, IDX_firmwareinfo) {
        adev.bios_scratch_reg_offset = ctx.cu32(h.data_start as u32 + FWI31_BIOS_SCRATCH_REG_STARTADDR);
    }
}

/// `amdgpu_atomfirmware_allocate_fb_v2_1`: the bytes the driver gets.
fn amdgpu_atomfirmware_allocate_fb_v2_1(adev: &mut amdgpu_device, ctx: &atom_context, at: u32) -> i32 {
    let start_addr = ctx.cu32(at + VUF21_START_ADDRESS_IN_KB);
    let fw_size = ctx.cu16(at + VUF21_USED_BY_FIRMWARE_IN_KB) as u32;
    let drv_size = ctx.cu16(at + VUF21_USED_BY_DRIVER_IN_KB) as u32;

    Line::new().s("atom firmware v2_1 requested ").hex(start_addr).s(" ").dec(fw_size as u64).s("kb fw ")
        .dec(drv_size as u64).s("kb drv").dbg();

    if (start_addr & ATOM_VRAM_OPERATION_FLAGS_MASK)
        == ATOM_VRAM_BLOCK_SRIOV_MSG_SHARE_RESERVATION << ATOM_VRAM_OPERATION_FLAGS_SHIFT
    {
        // Firmware request VRAM reservation for SR-IOV
        adev.mman.fw_vram_usage_start_offset = ((start_addr & !ATOM_VRAM_OPERATION_FLAGS_MASK) as u64) << 10;
        adev.mman.fw_vram_usage_size = (fw_size as u64) << 10;
        // Use the default scratch size
        0
    } else {
        (drv_size << 10) as i32
    }
}

/// `amdgpu_atomfirmware_allocate_fb_v2_2`: the bytes the driver gets.
fn amdgpu_atomfirmware_allocate_fb_v2_2(_adev: &mut amdgpu_device, ctx: &atom_context, at: u32) -> i32 {
    let fw_start_addr = ctx.cu32(at + VUF22_FW_REGION_START_ADDRESS_IN_KB);
    let fw_size = ctx.cu16(at + VUF22_USED_BY_FIRMWARE_IN_KB) as u32;
    let drv_start_addr = ctx.cu32(at + VUF22_DRIVER_REGION0_START_ADDRESS_IN_KB);
    let drv_size = ctx.cu32(at + VUF22_USED_BY_DRIVER_REGION0_IN_KB);

    Line::new().s("atom requested fw start at ").hex(fw_start_addr).s(" ").dec(fw_size as u64)
        .s("kb and drv start at ").hex(drv_start_addr).s(" ").dec(drv_size as u64).s("kb").dbg();

    // Both reservations are for SR-IOV only (amdgpu_sriov_vf).
    0
}

/// `amdgpu_atomfirmware_allocate_fb_scratch`.
pub fn amdgpu_atomfirmware_allocate_fb_scratch(adev: &mut amdgpu_device) -> i32 {
    let Some(mut ctx) = adev.mode_info.atom_context.take() else { return -12 };
    let mut usage_bytes = 0;

    if let Some(h) = amdgpu_atom_parse_data_header(&ctx, IDX_vram_usagebyfirmware) {
        if h.frev == 2 && h.crev == 1 {
            usage_bytes = amdgpu_atomfirmware_allocate_fb_v2_1(adev, &ctx, h.data_start as u32);
        } else if h.frev >= 2 && h.crev >= 2 {
            usage_bytes = amdgpu_atomfirmware_allocate_fb_v2_2(adev, &ctx, h.data_start as u32);
        }
    }

    ctx.scratch_size_bytes = 0;
    if usage_bytes == 0 {
        usage_bytes = 20 * 1024;
    }
    // allocate some scratch memory
    ctx.scratch = vec![0u32; (usage_bytes as usize).div_ceil(4)];
    ctx.scratch_size_bytes = usage_bytes;
    adev.mode_info.atom_context = Some(ctx);
    0
}

/// The reference and divider limits `get_clock_info` gives a PLL.
fn pll_limits(pll: &mut amdgpu_pll) {
    pll.reference_div = 0;
    pll.min_post_div = 1;
    pll.max_post_div = 1;
    pll.min_ref_div = 2;
    pll.max_ref_div = 0xff;
    pll.min_feedback_div = 4;
    pll.max_feedback_div = 0xff;
    pll.best_vco = 0;
}

/// `amdgpu_atomfirmware_get_clock_info`.
pub fn amdgpu_atomfirmware_get_clock_info(adev: &mut amdgpu_device) -> i32 {
    let Some(ctx) = adev.mode_info.atom_context.as_ref() else { return -22 };
    let mut ret = -22;

    if let Some(h) = amdgpu_atom_parse_data_header(ctx, IDX_firmwareinfo) {
        let at = h.data_start as u32;
        adev.clock.default_sclk = ctx.cu32(at + FWI31_BOOTUP_SCLK_IN10KHZ);
        adev.clock.default_mclk = ctx.cu32(at + FWI31_BOOTUP_MCLK_IN10KHZ);
        adev.pm.current_sclk = adev.clock.default_sclk;
        adev.pm.current_mclk = adev.clock.default_mclk;
        ret = 0;
    }

    if let Some(h) = amdgpu_atom_parse_data_header(ctx, IDX_smu_info) {
        let at = h.data_start as u32;
        // system clock
        if h.frev == 3 {
            adev.clock.spll.reference_freq = ctx.cu32(at + SMU31_CORE_REFCLK_10KHZ);
        } else if h.frev == 4 {
            adev.clock.spll.reference_freq = ctx.cu32(at + SMU40_CORE_REFCLK_10KHZ);
        }
        pll_limits(&mut adev.clock.spll);
        ret = 0;
    }

    if let Some(h) = amdgpu_atom_parse_data_header(ctx, IDX_umc_info) {
        // memory clock
        adev.clock.mpll.reference_freq = ctx.cu32(h.data_start as u32 + UMC31_MEM_REFCLK_10KHZ);
        pll_limits(&mut adev.clock.mpll);
        ret = 0;
    }

    ret
}
