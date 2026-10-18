//! Stage 2 against Linux: `amdgpu_device_init` as on the card, then from
//! here on registers never written read SEED(reg) by the rule the fourth
//! argument names (ones, zeros or hash), as in oracle/main_common.c, and
//! the writes of `amdgpu_device_ip_init` through
//! the COMMON block are printed as the oracle prints them.

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
    let ok = stage1::device_init(&mut adev, BDF, (0xfc00_0000, 2 << 20));
    npk_sys::quiet(quiet);
    assert!(ok, "amdgpu_device_init failed");
    let index = nbio_v7_0::nbio_v7_0_get_pcie_index_offset(&adev).unwrap();
    let data = nbio_v7_0::nbio_v7_0_get_pcie_data_offset(&adev).unwrap();
    let rule = match std::env::args().nth(4).as_deref() {
        Some("ones") => ones,
        Some("zeros") => zeros,
        _ => hash,
    };
    npk_sys::fake_card_unset(rule, (index, data));
    npk_sys::fake_card_clear_writes();
    let r = amdgpu_device::amdgpu_device_ip_init_common(&mut adev);
    for (reg, v) in npk_sys::fake_card_writes() {
        println!("write reg {:#x} = {:#x}", reg, v);
    }
    println!("pcie_index {:#x} pcie_data {:#x}", index, data);
    println!("soc15_common_hw_init {}", r);
}
