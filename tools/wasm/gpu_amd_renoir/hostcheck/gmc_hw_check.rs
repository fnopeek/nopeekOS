//! gmc_v9_0_hw_init against Linux: the run as on the card through GMC's
//! sw_init and the scratch buffer, then the inputs Linux's side needs
//! (the GART table's page directory address and offset, the scratch and
//! dummy page addresses) on one line, then every register write of
//! gmc_v9_0_hw_init as it happens. Registers never written read 0; the
//! invalidation semaphore and acknowledge registers of engine 17 are set
//! so the flushes go through, as in oracle/main_gmc.c.

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
    assert!(stage1::device_init(&mut adev, BDF, (0xfc00_0000, 2 << 20)));
    assert_eq!(amdgpu_device::amdgpu_device_ip_init_common(&mut adev), 0);
    let index = nbio_v7_0::nbio_v7_0_get_pcie_index_offset(&adev).unwrap();
    let data = nbio_v7_0::nbio_v7_0_get_pcie_data_offset(&adev).unwrap();
    npk_sys::fake_card_unset(zeros, (index, data));
    adev.bar0 = (0xd000_0000, 256 << 20);
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
    assert_eq!(gmc_v9_0::gmc_v9_0_sw_init(&mut adev), 0);
    assert_eq!(amdgpu_device::amdgpu_device_mem_scratch_init(&mut adev), 0);
    npk_sys::quiet(quiet);
    let gart = adev.gart.bo.unwrap();
    println!("inputs {:#x} {:#x} {:#x} {:#x} {:#x} {}", amdgpu_gmc::amdgpu_gmc_pd_addr(&adev, gart),
        amdgpu_object::amdgpu_bo_gpu_offset(&adev, gart), adev.mem_scratch_gpu_addr, adev.dummy_page_addr,
        adev.pg_flags, adev.gmc.noretry);
    // Engine 17's semaphore acquired and acknowledge set, on both hubs.
    for h in [AMDGPU_GFXHUB_0, AMDGPU_MMHUB0_0] {
        let hub = adev.vmhub[h as usize];
        adev.wreg32(hub.vm_inv_eng0_sem + hub.eng_distance * 17, 1);
        adev.wreg32(hub.vm_inv_eng0_ack + hub.eng_distance * 17, 0xffff_ffff);
    }
    println!("--- hw_init");
    npk_sys::fake_card_echo(true);
    let r = gmc_v9_0::gmc_v9_0_hw_init(&mut adev);
    npk_sys::fake_card_echo(false);
    println!("gmc_v9_0_hw_init {}", r);
}
