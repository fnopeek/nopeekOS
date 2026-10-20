//! Stage 4's GFX block against Linux: the port runs to the end of
//! `amdgpu_device_fw_loading` as on the card, then from here on registers
//! never written read SEED(reg) by the rule the fourth argument names, and
//! the block's life runs as oracle/main_gfx.c runs it: hw_init, late_init,
//! gating on and off, hw_fini. Printed are every register write and every
//! doorbell with the ring dwords it hands over, then the state the oracle
//! prints. The first line, `inputs …`, is the oracle's stdin.

extern crate alloc;

include!("modules.rs");
#[path = "../src/stage1.rs"]
mod stage1;

use regs::*;

include!("fake_renoir.rs");
include!("fake_gpu.rs");

fn ones(_reg: u32) -> u32 {
    !0
}

fn zeros(_reg: u32) -> u32 {
    0
}

fn hash(reg: u32) -> u32 {
    reg.wrapping_mul(0x9e37_79b1) ^ 0xa5a5_a5a5
}

thread_local! {
    static RUNG: RefCell<Vec<(u32, u64)>> = const { RefCell::new(Vec::new()) };
}

/// A doorbell: the dwords from the last one on this index to `value`,
/// then the fake GPU runs them.
fn print_doorbell(index: u32, value: u64) {
    let from = RUNG.with(|r| r.borrow().iter().find(|e| e.0 == index).map_or(0, |e| e.1));
    match queue_for(index) {
        Some(q) => {
            let mut l = format!("doorbell {:#x} = {:#x}:", index, value);
            let mut at = from;
            while at != value {
                l += &format!(" {:08x}", gpu_r32(q.base + (at % q.size_dw as u64) * 4));
                at += 1;
            }
            println!("{}", l);
        }
        None => println!("doorbell {:#x} = {:#x}: no ring", index, value),
    }
    RUNG.with(|r| {
        let mut r = r.borrow_mut();
        r.retain(|e| e.0 != index);
        r.push((index, value));
    });
    doorbell(index, value);
}

fn ring_line(adev: &amdgpu_device::amdgpu_device, kind: &str, r: usize) -> String {
    let g = &adev.rings[r];
    format!("ring {} {} {} {} {} {} {} {} {:#x} {:#x} {:#x} {:#x} {:#x}", g.name, kind, g.me, g.pipe, g.queue,
            g.doorbell_index, g.ring_size, g.max_dw, g.gpu_addr, g.rptr_gpu_addr, g.wptr_gpu_addr, g.eop_gpu_addr,
            g.mqd_gpu_addr)
}

fn main() {
    let mut adev = fake_renoir();
    fake_gpu();
    npk_sys::fake_doorbell_hook(print_doorbell);
    let quiet = npk_sys::quiet(true);
    assert!(stage1::up_to_fw_loading(&mut adev, BDF, (0xfc00_0000, 2 << 20)), "the run did not reach fw_loading");
    // Phase 2 runs the SMU before the GFX block; GFXOFF needs it up.
    assert_eq!(amdgpu_smu::smu_hw_init(&mut adev), 0, "smu_hw_init");

    let a = &adev;
    let gc = &a.reg_offset[GC_HWIP as usize][0];
    let mut l = format!("inputs bases {}", gc.len());
    for (seg, v) in gc.iter().enumerate() {
        l += &format!(" {} {} {:x}", GC_HWIP, seg, v);
    }
    let g = &a.gfx;
    l += &format!(" versions {:x} {:x} flags {:x} apu {:x} cg {:x} pg {:x} pp {:x}", a.ip_version(GC_HWIP, 0),
                  a.ip_version(SDMA0_HWIP, 0), a.flags, a.apu_flags, a.cg_flags as u32, a.pg_flags, a.pm.pp_feature);
    let c = &g.config;
    l += &format!(" config {} {} {} {} {} mec {} {} {} compute {} gfx {} me_fw {:x} {:x}", c.max_shader_engines,
                  c.max_sh_per_se, c.max_backends_per_se, c.max_cu_per_sh, c.max_hw_contexts, g.mec.num_mec,
                  g.mec.num_pipe_per_mec, g.mec.num_queue_per_pipe, g.num_compute_rings, g.num_gfx_rings,
                  g.me_fw_version, g.me_feature_version);
    l += &format!(" rlc {} {:#x} {} {:#x} doorbell {} {} {} vm {} {} gmc {} {:#x} {:#x} gds {} {} qmask {:#x} cleaner {} {:#x}",
                  g.rlc.is_rlc_v2_1 as u32, g.rlc.clear_state_gpu_addr, g.rlc.clear_state_size, g.rlc.cp_table_gpu_addr,
                  a.doorbell_index.kiq, a.doorbell_index.userqueue_end, a.doorbell.size, a.vm_manager.first_kfd_vmid,
                  a.vm_manager.id_mgr[AMDGPU_GFXHUB_0 as usize].num_ids, a.gmc.noretry, a.gmc.private_aperture_start,
                  a.gmc.shared_aperture_start, a.gds.gds_size, a.gds.gds_compute_max_wave_id, g.mec_queue_bitmap[0],
                  g.enable_cleaner_shader as u32, g.cleaner_shader_gpu_addr);
    println!("{}", l);
    println!("{}", ring_line(a, "gfx", g.gfx_ring[0]));
    for &r in g.compute_ring.iter() {
        println!("{}", ring_line(a, "compute", r));
    }
    println!("{}", ring_line(a, "kiq", g.kiq.ring.unwrap()));

    let index = nbio_v7_0::nbio_v7_0_get_pcie_index_offset(&adev).unwrap();
    let data = nbio_v7_0::nbio_v7_0_get_pcie_data_offset(&adev).unwrap();
    let rule = match std::env::args().nth(4).as_deref() {
        Some("ones") => ones,
        Some("zeros") => zeros,
        _ => hash,
    };
    npk_sys::fake_card_unset(rule, (index, data));
    // The SMU mailbox: GFXOFF's messages are the SMU oracle's.
    let mp1 = |idx: u32, reg: u32| adev.reg_offset[MP1_HWIP as usize][0][idx as usize] + reg;
    npk_sys::fake_card_echo_skip(&[mp1(mmMP1_SMN_C2PMSG_66_BASE_IDX, mmMP1_SMN_C2PMSG_66),
                                   mp1(mmMP1_SMN_C2PMSG_82_BASE_IDX, mmMP1_SMN_C2PMSG_82),
                                   mp1(mmMP1_SMN_C2PMSG_90_BASE_IDX, mmMP1_SMN_C2PMSG_90)]);
    npk_sys::fake_card_echo(true);

    println!("--- gfx hw_init");
    println!("gfx_v9_0_hw_init {}", gfx_v9_0::gfx_v9_0_hw_init(&mut adev));
    let c = &adev.gfx.config;
    println!("rbs {} mask {:#x} cu {} ao {:#x} db_debug2 {:#x}", c.num_rbs, c.backend_enable_mask,
             adev.gfx.cu_info.number, adev.gfx.cu_info.ao_cu_mask, c.db_debug2);
    let mut l = String::from("cleaner:");
    if let Some(k) = adev.gfx.cleaner_shader_cpu_ptr {
        for i in 0..adev.gfx.cleaner_shader_size / 4 {
            l += &format!(" {:08x}", amdgpu_gart::kptr_r32(&adev, k, i as u64 * 4));
        }
    }
    println!("{}", l);
    let mut l = String::from("csb:");
    let k = adev.gfx.rlc.cs_ptr.unwrap();
    for i in 0..adev.gfx.rlc.clear_state_size {
        l += &format!(" {:08x}", amdgpu_gart::kptr_r32(&adev, k, i as u64 * 4));
    }
    println!("{}", l);
    let mut order: Vec<usize> = vec![adev.gfx.gfx_ring[0]];
    order.extend(adev.gfx.compute_ring.iter());
    order.push(adev.gfx.kiq.ring.unwrap());
    for &r in order.iter() {
        let ring = &adev.rings[r];
        let mut l = format!("mqd {}:", ring.name);
        if let Some(k) = ring.mqd_ptr {
            for i in 0..SIZEOF_v9_mqd_allocation / 4 {
                let v = amdgpu_gart::kptr_r32(&adev, k, i as u64 * 4);
                if v != 0 {
                    l += &format!(" {:x}={:08x}", i, v);
                }
            }
        }
        println!("{}", l);
        println!("ready {} {}", ring.name, ring.sched.ready as u32);
    }
    println!("--- gfx late_init");
    println!("gfx_v9_0_late_init {}", gfx_v9_0::gfx_v9_0_late_init(&mut adev));
    println!("--- gfx gating");
    gfx_v9_0::gfx_v9_0_set_clockgating_state(&mut adev, true);
    gfx_v9_0::gfx_v9_0_set_powergating_state(&mut adev, true);
    println!("gfxoff {} count {}", adev.gfx.gfx_off_state as u32, adev.gfx.gfx_off_req_count);
    gfx_v9_0::gfx_v9_0_set_powergating_state(&mut adev, false);
    gfx_v9_0::gfx_v9_0_set_clockgating_state(&mut adev, false);
    println!("gfxoff {} count {}", adev.gfx.gfx_off_state as u32, adev.gfx.gfx_off_req_count);
    println!("--- gfx hw_fini");
    println!("gfx_v9_0_hw_fini {}", gfx_v9_0::gfx_v9_0_hw_fini(&mut adev));
    npk_sys::quiet(quiet);
}
