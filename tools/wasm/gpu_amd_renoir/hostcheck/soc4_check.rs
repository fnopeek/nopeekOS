//! The SOC blocks' late life against Linux: the port runs to the end of
//! the blocks' sw_init as on the card, then from here on registers never
//! written read SEED(reg) by the rule the fourth argument names, and the
//! life runs as oracle/main_gmc.c's late mode runs it: IH hw_init, GMC
//! late_init, the clock gating of COMMON, GMC and IH on and off, the
//! hw_fini of IH, GMC and COMMON. Printed are every register write, the
//! return values and the VM invalidation engines. The lines up to `end` are
//! the oracle's stdin: the IH rings, the rings and what the fake card holds.

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

fn main() {
    let mut adev = fake_renoir();
    let quiet = npk_sys::quiet(true);
    let bar2 = (0xfc00_0000, 2 << 20);
    assert!(stage1::device_init(&mut adev, BDF, bar2) && stage1::ip_init_common(&mut adev)
            && stage1::ip_init_gmc(&mut adev) && stage1::ip_init_sw(&mut adev), "the run did not reach sw_init's end");

    let index = nbio_v7_0::nbio_v7_0_get_pcie_index_offset(&adev).unwrap();
    let data = nbio_v7_0::nbio_v7_0_get_pcie_data_offset(&adev).unwrap();
    let a = &adev;
    println!("inputs cg {:x} pg {:x} msi {} dummy {:x}", a.cg_flags, a.pg_flags, a.irq.msi_enabled as u32,
             a.dummy_page_addr);
    // An APU has no ih1 and ih2: their ring_size stays 0.
    for (k, ih) in [(0, &a.irq.ih), (3, &a.irq.ih_soft)] {
        println!("ih {} {} {:x} {:x} {} {} {}", k, ih.ring_size, ih.gpu_addr, ih.wptr_addr, ih.use_bus_addr as u32,
                 ih.use_doorbell as u32, ih.doorbell_index);
    }
    for r in a.rings.iter() {
        println!("ring {} {} {}", r.name, r.vm_hub, r.me);
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

    println!("--- soc ih hw_init");
    println!("vega10_ih_hw_init {}", vega10_ih::vega10_ih_hw_init(&mut adev));
    println!("--- soc gmc late_init");
    println!("gmc_v9_0_late_init {}", gmc_v9_0::gmc_v9_0_late_init(&mut adev));
    for r in adev.rings.iter() {
        println!("inv {} {}", r.name, r.vm_inv_eng);
    }
    println!("--- soc gating");
    soc15::soc15_common_set_clockgating_state(&adev, true);
    gmc_v9_0::gmc_v9_0_set_clockgating_state(&adev, true);
    vega10_ih::vega10_ih_set_clockgating_state(&mut adev, true);
    vega10_ih::vega10_ih_set_clockgating_state(&mut adev, false);
    gmc_v9_0::gmc_v9_0_set_clockgating_state(&adev, false);
    soc15::soc15_common_set_clockgating_state(&adev, false);
    println!("--- soc hw_fini");
    println!("vega10_ih_hw_fini {}", vega10_ih::vega10_ih_hw_fini(&mut adev));
    println!("gmc_v9_0_hw_fini {}", gmc_v9_0::gmc_v9_0_hw_fini(&mut adev));
    println!("soc15_common_hw_fini {}", soc15::soc15_common_hw_fini(&mut adev));
    npk_sys::quiet(quiet);
}
