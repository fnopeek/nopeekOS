//! Stage 4's SMU block against Linux: the port runs to the end of
//! `amdgpu_device_fw_loading` as on the card, then from here on registers
//! never written read SEED(reg) by the rule the fourth argument names, and
//! the block's life runs as oracle/main_smu.c runs it: hw_init, the power
//! gating of SDMA and GFX, late_init, hw_fini. Printed are every register
//! write, the return values and the state the oracle prints. The lines up
//! to `end` are the oracle's stdin: the register bases, what the fake card
//! already holds, the buffers sw_init got and the boot values the port read
//! from the VBIOS.

extern crate alloc;

include!("modules.rs");
#[path = "../src/stage1.rs"]
mod stage1;

use regs::*;

include!("fake_renoir.rs");

fn ones(_reg: u32) -> u32 {
    !0
}

fn zeros(_reg: u32) -> u32 {
    0
}

fn hash(reg: u32) -> u32 {
    reg.wrapping_mul(0x9e37_79b1) ^ 0xa5a5_a5a5
}

fn dump(adev: &amdgpu_device::amdgpu_device) {
    let s = &adev.smu;
    println!("smu fw {:x} if {:x} drv_if {:x} pm_fw {:x} dpm {} feature {:x} workload {:x}",
             s.smc_fw_version, s.smc_fw_if_version, s.smc_driver_if_version, adev.pm.fw_version,
             adev.pm.dpm_enabled as u32, s.feature_supported, s.workload_mask);
    println!("gfx freq {} {} {} {}", s.gfx_default_hard_min_freq, s.gfx_default_soft_max_freq,
             s.gfx_actual_hard_min_freq, s.gfx_actual_soft_max_freq);
    let mut l = format!("gated vcn {} jpeg {} pwr_state", s.vcn_gated[0] as u32, s.jpeg_gated as u32);
    for v in adev.pm.pwr_state.iter() {
        l += &format!(" {}", v);
    }
    println!("{}", l);
}

fn main() {
    let mut adev = fake_renoir();
    let quiet = npk_sys::quiet(true);
    assert!(stage1::up_to_fw_loading(&mut adev, BDF, (0xfc00_0000, 2 << 20)), "the run did not reach fw_loading");

    let index = nbio_v7_0::nbio_v7_0_get_pcie_index_offset(&adev).unwrap();
    let data = nbio_v7_0::nbio_v7_0_get_pcie_data_offset(&adev).unwrap();
    let a = &adev;
    let hw = [MP1_HWIP, SMUIO_HWIP, HDP_HWIP];
    let n: usize = hw.iter().map(|&h| a.reg_offset[h as usize][0].len()).sum();
    let mut l = format!("inputs bases {}", n);
    for h in hw {
        for (seg, v) in a.reg_offset[h as usize][0].iter().enumerate() {
            l += &format!(" {} {} {:x}", h, seg, v);
        }
    }
    l += &format!(" versions {:x} {:x} {:x} flags {:x} apu {:x} cg {:x} pg {:x} pp {:x} vcn {} gen {:x} mlw {:x} usec {} pool {}",
                  a.ip_version(MP1_HWIP, 0), a.ip_version(HDP_HWIP, 0), a.ip_version(GC_HWIP, 0), a.flags,
                  a.apu_flags, a.cg_flags, a.pg_flags, a.pm.pp_feature, a.vcn.num_vcn_inst, a.pm.pcie_gen_mask,
                  a.pm.pcie_mlw_mask, a.usec_timeout, a.pm.smu_prv_buffer_size);
    l += &format!(" pcie {:x} {:x}", index, data);
    println!("{}", l);
    let b = &a.smu.smu_table.boot_values;
    println!("boot {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {}", b.revision, b.gfxclk, b.uclk,
             b.socclk, b.dcefclk, b.eclk, b.vclk, b.dclk, b.vddc, b.vddci, b.mvddc, b.vdd_gfx, b.cooling_id,
             b.pp_table_id, b.format_revision, b.content_revision, b.fclk, b.lclk, b.firmware_caps);
    for blk in a.ip_blocks.iter() {
        println!("block {} {}", blk.version.type_, blk.status.valid as u32);
    }
    // sw_init's buffers in the order smu_init_fb_allocations,
    // smu_alloc_memory_pool and smu_alloc_dummy_read_table ask for them.
    let t = &a.smu.smu_table;
    let pm = &t.tables[SMU_TABLE_PMSTATUSLOG as usize];
    let mut bos = Vec::new();
    if pm.size != 0 {
        bos.push(pm);
    }
    bos.push(&t.driver_table);
    if a.smu.pool_size != 0 {
        bos.push(&t.memory_pool);
    }
    if t.dummy_read_1_table.size != 0 {
        bos.push(&t.dummy_read_1_table);
    }
    for bo in bos {
        println!("bo {} {} {} {:x}", bo.size, bo.align, bo.domain, bo.mc_address);
    }
    for (r, v) in npk_sys::fake_card_regs() {
        println!("preset {:x} {:x}", r, v);
    }
    for (r, v) in npk_sys::fake_card_indirect() {
        println!("pcie {:x} {:x}", r, v);
    }
    println!("end");

    let rule = match std::env::args().nth(4).as_deref() {
        Some("ones") => ones,
        Some("zeros") => zeros,
        _ => hash,
    };
    npk_sys::fake_card_unset(rule, (index, data));
    npk_sys::fake_card_echo(true);

    println!("--- smu hw_init");
    println!("smu_hw_init {}", amdgpu_smu::smu_hw_init(&mut adev));
    dump(&adev);
    println!("--- smu powergating");
    println!("sdma ungate {}", amdgpu_smu::amdgpu_dpm_set_powergating_by_smu(&mut adev, AMD_IP_BLOCK_TYPE_SDMA, false, 0));
    println!("gfx gate {}", amdgpu_smu::amdgpu_dpm_set_powergating_by_smu(&mut adev, AMD_IP_BLOCK_TYPE_GFX, true, 0));
    println!("gfx ungate {}", amdgpu_smu::amdgpu_dpm_set_powergating_by_smu(&mut adev, AMD_IP_BLOCK_TYPE_GFX, false, 0));
    println!("--- smu late_init");
    println!("smu_late_init {}", amdgpu_smu::smu_late_init(&mut adev));
    dump(&adev);
    println!("--- smu hw_fini");
    println!("sdma gate {}", amdgpu_smu::amdgpu_dpm_set_powergating_by_smu(&mut adev, AMD_IP_BLOCK_TYPE_SDMA, true, 0));
    println!("smu_hw_fini {}", amdgpu_smu::smu_hw_fini(&mut adev));
    dump(&adev);
    npk_sys::quiet(quiet);
}
