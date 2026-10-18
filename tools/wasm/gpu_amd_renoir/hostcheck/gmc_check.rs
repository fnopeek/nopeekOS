//! gmc_v9_0_sw_init up to amdgpu_bo_init against Linux: `amdgpu_device_init`
//! and COMMON as on the card, then the presets oracle/main_gmc.c sets
//! (registers never written read 0 from here on), then the layout part of
//! the GMC sw_init, printed as the oracle prints it.

extern crate alloc;

include!("modules.rs");
#[path = "../src/stage1.rs"]
mod stage1;

use regs::*;

include!("fake_renoir.rs");

fn zeros(_reg: u32) -> u32 {
    0
}

fn soc15(adev: &amdgpu_device::amdgpu_device, hwip: u32, idx: u32, reg: u32) -> u32 {
    adev.reg_offset[hwip as usize][0][idx as usize] + reg
}

fn main() {
    let mut adev = fake_renoir();
    let quiet = npk_sys::quiet(true);
    let ok = stage1::device_init(&mut adev, BDF, (0xfc00_0000, 2 << 20));
    assert!(ok, "amdgpu_device_init failed");
    assert_eq!(amdgpu_device::amdgpu_device_ip_init_common(&mut adev), 0);
    npk_sys::quiet(quiet);
    let index = nbio_v7_0::nbio_v7_0_get_pcie_index_offset(&adev).unwrap();
    let data = nbio_v7_0::nbio_v7_0_get_pcie_data_offset(&adev).unwrap();
    npk_sys::fake_card_unset(zeros, (index, data));
    npk_sys::fake_card_clear_writes();
    adev.bar0 = (0xd000_0000, 256 << 20);
    // The presets main_gmc.c sets alike.
    let presets = [
        (soc15(&adev, NBIO_HWIP, NBIO_mmRCC_CONFIG_MEMSIZE_BASE_IDX, NBIO_mmRCC_CONFIG_MEMSIZE), 512),
        (soc15(&adev, MMHUB_HWIP, MM_mmMC_VM_FB_LOCATION_BASE_BASE_IDX, MM_mmMC_VM_FB_LOCATION_BASE), 0xf400),
        (soc15(&adev, MMHUB_HWIP, MM_mmMC_VM_FB_LOCATION_TOP_BASE_IDX, MM_mmMC_VM_FB_LOCATION_TOP), 0xf41f),
        (soc15(&adev, GC_HWIP, GC_mmMC_VM_FB_OFFSET_BASE_IDX, GC_mmMC_VM_FB_OFFSET), 0x3e0),
        (soc15(&adev, DCE_HWIP, mmHUBP0_DCSURF_PRI_VIEWPORT_DIMENSION_DCN2_BASE_IDX,
            mmHUBP0_DCSURF_PRI_VIEWPORT_DIMENSION_DCN2), 1080 << 16 | 1920),
    ];
    for (reg, v) in presets {
        adev.wreg32(reg, v);
    }
    for (reg, v) in npk_sys::fake_card_writes() {
        println!("write reg {:#x} = {:#x}", reg, v);
    }
    println!("--- presets set");
    let r = gmc_v9_0::gmc_v9_0_sw_init_layout(&mut adev);
    println!("gmc_v9_0_sw_init up to amdgpu_bo_init: {}", r);
    let mc = &adev.gmc;
    println!("mc_mask {:#x} mc_vram_size {:#x} real {:#x} visible {:#x} aper {:#x}+{:#x}",
        mc.mc_mask, mc.mc_vram_size, mc.real_vram_size, mc.visible_vram_size, mc.aper_base, mc.aper_size);
    println!("fb {:#x}-{:#x} vram {:#x}-{:#x} gart {:#x} {:#x}-{:#x} agp {:#x}-{:#x}+{:#x}",
        mc.fb_start, mc.fb_end, mc.vram_start, mc.vram_end, mc.gart_size, mc.gart_start, mc.gart_end,
        mc.agp_start, mc.agp_end, mc.agp_size);
    let vm = &adev.vm_manager;
    println!("vm max_pfn {:#x} num_level {} root_level {} block_size {} fragment_size {} vram_base_offset {:#x}",
        vm.max_pfn, vm.num_level, vm.root_level, vm.block_size, vm.fragment_size, vm.vram_base_offset);
    let m = &adev.mman;
    println!("stolen vga {:#x} extended {:#x} reserved {:#x}+{:#x} keep {} vmhubs_mask {:#x}",
        m.stolen_vga_size, m.stolen_extended_size, m.stolen_reserved_offset, m.stolen_reserved_size,
        m.keep_stolen_vga_memory as u32, adev.vmhubs_mask);
    for (h, v) in adev.vmhub.iter().enumerate() {
        if v.ctx0_ptb_addr_lo32 == 0 {
            continue;
        }
        println!("vmhub[{}] ptb {:#x} {:#x} inv sem {:#x} req {:#x} ack {:#x} ctx0_cntl {:#x} fault status {:#x} cntl {:#x} dist {} {} {} {}",
            h, v.ctx0_ptb_addr_lo32, v.ctx0_ptb_addr_hi32, v.vm_inv_eng0_sem, v.vm_inv_eng0_req, v.vm_inv_eng0_ack,
            v.vm_context0_cntl, v.vm_l2_pro_fault_status, v.vm_l2_pro_fault_cntl, v.ctx_distance,
            v.ctx_addr_distance, v.eng_distance, v.eng_addr_distance);
    }
    for (c, client) in adev.irq.client.iter().enumerate() {
        for (i, s) in client.sources.iter().flatten().enumerate() {
            if let Some(name) = s {
                println!("irq client {} src {} {}", c, i, name);
            }
        }
    }
}
