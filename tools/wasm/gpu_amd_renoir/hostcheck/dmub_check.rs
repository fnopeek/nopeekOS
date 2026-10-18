//! The port's DMUB region layout on the DMCUB image read off the card, printed as
//! oracle/main_dmub.c prints Linux's.

extern crate alloc;

include!("modules.rs");

use crate::dmub_srv::{dmub_srv_calc_mem_info, dmub_srv_fb_info, dmub_srv_region_info};

fn main() {
    let mut args = std::env::args().skip(1);
    let data = std::fs::read(args.next().expect("dmcub.bin")).expect("read");
    let vbios_size = parse(&args.next().expect("vbios size")) as u32;
    let gpu = parse(&args.next().expect("gpu address"));
    let mut dmub = crate::dmub_srv::dmub_srv::default();
    let mut region_info = dmub_srv_region_info::default();
    let r = amdgpu_dm::dm_dmub_regions(&data, vbios_size, regs::DMUB_ASIC_DCN21, &mut dmub, &mut region_info);
    println!(
        "region_info {} fb_size {:#x} gart_size {:#x} num_regions {} fw_version 0x{:08x}",
        if r == 0 { 0 } else { regs::DMUB_STATUS_INVALID }, region_info.fb_size, region_info.gart_size,
        region_info.num_regions, dmub.fw_version
    );
    let m = &dmub.meta_info;
    println!(
        "meta magic 0x{:08x} fw_region_size {:#x} trace_buffer_size {:#x} shared_state_size {:#x}",
        m.magic_value, m.fw_region_size, m.trace_buffer_size, m.shared_state_size
    );
    for (i, g) in region_info.regions.iter().enumerate() {
        println!("region {} {:#x}-{:#x}", i, g.base, g.top);
    }
    let mut fb_info = dmub_srv_fb_info::default();
    let status = dmub_srv_calc_mem_info(&dmub, &region_info, &amdgpu_dm::WINDOW_MEMORY_TYPE, (0, gpu), (0, 0), &mut fb_info);
    println!("mem_info {} num_fb {}", status, fb_info.num_fb);
    for (i, f) in fb_info.fb.iter().enumerate() {
        println!("fb {} cpu {:#x} gpu {:#x} size {:#x}", i, f.cpu_off, f.gpu_addr, f.size);
    }
}

fn parse(s: &str) -> u64 {
    match s.strip_prefix("0x") {
        Some(h) => u64::from_str_radix(h, 16).unwrap(),
        None => s.parse().unwrap(),
    }
}
