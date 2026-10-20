//! Stage 4's SDMA block against Linux: the port runs to the end of
//! `amdgpu_device_fw_loading` as on the card, the SMU comes up as phase 2
//! has it, then from here on registers never written read SEED(reg) by the
//! rule the fourth argument names, and the block's life runs as
//! oracle/main_sdma.c runs it: hw_init, late_init, gating on and off,
//! hw_fini. Printed are every register write and every doorbell with the
//! ring dwords it hands over, then the state the oracle prints. The lines
//! up to the last `ring` are the oracle's stdin.

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

/// A doorbell: the wptr in bytes, so the dwords from the last one on this
/// index to `value / 4`, then the fake GPU runs them.
fn print_doorbell(index: u32, value: u64) {
    let from = RUNG.with(|r| r.borrow().iter().find(|e| e.0 == index).map_or(0, |e| e.1));
    match queue_for(index) {
        Some(q) => {
            let mut l = format!("doorbell {:#x} = {:#x}:", index, value);
            let mut at = from;
            while at != value / 4 {
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
        r.push((index, value / 4));
    });
    doorbell(index, value);
}

fn main() {
    let mut adev = fake_renoir();
    fake_gpu();
    npk_sys::fake_doorbell_hook(print_doorbell);
    let quiet = npk_sys::quiet(true);
    assert!(stage1::up_to_fw_loading(&mut adev, BDF, (0xfc00_0000, 2 << 20)), "the run did not reach fw_loading");
    assert_eq!(amdgpu_smu::smu_hw_init(&mut adev), 0, "smu_hw_init");

    let a = &adev;
    // GC's bases too: soc15_program_register_sequence compares against
    // four GC registers.
    let hw = [GC_HWIP, SDMA0_HWIP];
    let n: usize = hw.iter().map(|&h| a.reg_offset[h as usize][0].len()).sum();
    let mut l = format!("inputs bases {}", n);
    for h in hw {
        for (seg, v) in a.reg_offset[h as usize][0].iter().enumerate() {
            l += &format!(" {} {} {:x}", h, seg, v);
        }
    }
    l += &format!(" versions {:x} flags {:x} apu {:x} cg {:x} pg {:x} device {:x} usec {} instances {} page {}",
                  a.ip_version(SDMA0_HWIP, 0), a.flags, a.apu_flags, a.cg_flags as u32, a.pg_flags, a.device,
                  a.usec_timeout, a.sdma.num_instances, a.sdma.has_page_queue as u32);
    l += &format!(" wb {:#x} {} used", a.wb.gpu_addr, a.wb.num_wb);
    for u in a.wb.used.iter() {
        l += &format!(" {:x}", u);
    }
    println!("{}", l);
    let mut order = Vec::new();
    for i in 0..a.sdma.num_instances as usize {
        let s = &a.sdma.instance[i];
        let r = s.ring.unwrap();
        let g = &a.rings[r];
        println!("ring {} {} {} {} {} {} {} {:#x} {:#x} {:#x}", g.name, s.fw_version, s.burst_nop as u32, g.me,
                 g.doorbell_index, g.ring_size, g.max_dw, g.gpu_addr, g.rptr_gpu_addr, g.wptr_gpu_addr);
        order.push(r);
    }

    let index = nbio_v7_0::nbio_v7_0_get_pcie_index_offset(&adev).unwrap();
    let data = nbio_v7_0::nbio_v7_0_get_pcie_data_offset(&adev).unwrap();
    let rule = match std::env::args().nth(4).as_deref() {
        Some("ones") => ones,
        Some("zeros") => zeros,
        _ => hash,
    };
    npk_sys::fake_card_unset(rule, (index, data));
    // The SMU mailbox: SDMA's power-up and power-down are the SMU oracle's.
    let mp1 = |idx: u32, reg: u32| adev.reg_offset[MP1_HWIP as usize][0][idx as usize] + reg;
    npk_sys::fake_card_echo_skip(&[mp1(mmMP1_SMN_C2PMSG_66_BASE_IDX, mmMP1_SMN_C2PMSG_66),
                                   mp1(mmMP1_SMN_C2PMSG_82_BASE_IDX, mmMP1_SMN_C2PMSG_82),
                                   mp1(mmMP1_SMN_C2PMSG_90_BASE_IDX, mmMP1_SMN_C2PMSG_90)]);
    npk_sys::fake_card_echo(true);

    println!("--- sdma hw_init");
    println!("sdma_v4_0_hw_init {}", sdma_v4_0::sdma_v4_0_hw_init(&mut adev));
    for &r in order.iter() {
        let ring = &adev.rings[r];
        println!("ready {} {} wptr {:#x}", ring.name, ring.sched.ready as u32,
                 amdgpu_ring::ring_wb_read64(&adev, ring.wptr_offs));
    }
    let mut l = String::from("wb used");
    for u in adev.wb.used.iter() {
        l += &format!(" {:x}", u);
    }
    println!("{}", l);
    println!("--- sdma late_init");
    println!("sdma_v4_0_late_init {}", sdma_v4_0::sdma_v4_0_late_init(&mut adev));
    println!("--- sdma gating");
    sdma_v4_0::sdma_v4_0_set_clockgating_state(&adev, true);
    sdma_v4_0::sdma_v4_0_set_powergating_state(&adev, true);
    sdma_v4_0::sdma_v4_0_set_powergating_state(&adev, false);
    sdma_v4_0::sdma_v4_0_set_clockgating_state(&adev, false);
    println!("--- sdma hw_fini");
    println!("sdma_v4_0_hw_fini {}", sdma_v4_0::sdma_v4_0_hw_fini(&mut adev));
    npk_sys::quiet(quiet);
}
