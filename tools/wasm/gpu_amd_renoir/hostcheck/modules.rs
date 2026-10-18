// The module's sources the host checks compile, as `mod` items of the
// check's crate root (include!d there), so `crate::…` paths resolve as in
// the module.
#[path = "../src/amdgpu_atombios.rs"]
mod amdgpu_atombios;
#[path = "../src/amdgpu_atomfirmware.rs"]
mod amdgpu_atomfirmware;
#[path = "../src/amdgpu_bios.rs"]
mod amdgpu_bios;
#[path = "../src/amdgpu_device.rs"]
mod amdgpu_device;
#[path = "../src/amdgpu_discovery.rs"]
mod amdgpu_discovery;
#[path = "../src/atom.rs"]
mod atom;
#[path = "../src/ip_blocks.rs"]
mod ip_blocks;
#[path = "../src/linux.rs"]
mod linux;
#[path = "../src/nbio_v7_0.rs"]
mod nbio_v7_0;
#[path = "../src/regs.rs"]
#[allow(dead_code, non_upper_case_globals)]
mod regs;
#[path = "../src/soc15.rs"]
mod soc15;
#[path = "../src/tables.rs"]
#[allow(dead_code, non_upper_case_globals)]
mod tables;
