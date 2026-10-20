//! The run end to end on the host: stage1::run as _start calls it, on a
//! fake card with the device's IP discovery binary at the top of VRAM and
//! an ACPI VFCT holding the device's VBIOS, built to the layouts
//! amdgpu_acpi_vfct_bios reads, and the staged firmware as sys/firmware/.
//! Prints the log the card run prints.

extern crate alloc;

include!("modules.rs");
#[path = "../src/stage1.rs"]
mod stage1;

use regs::*;

include!("fake_renoir.rs");
include!("fake_gpu.rs");

fn main() {
    let mut adev = fake_renoir();
    fake_gpu();
    stage1::run(&mut adev, BDF, (0xfc00_0000, 2 << 20));
    PACKETS.with(|p| for l in p.borrow().iter() { println!("gpu {}", l); });
    // Every write but the MM_INDEX window VRAM is read through.
    for (reg, v) in npk_sys::fake_card_writes() {
        if reg != mmMM_INDEX && reg != mmMM_INDEX_HI {
            println!("write reg {:#x} = {:#x}", reg, v);
        }
    }
}
