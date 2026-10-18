//! The port's ATOM path on a VBIOS image, printed line for line as
//! oracle/main.c prints Linux's. check.sh diffs the two.

extern crate alloc;

include!("modules.rs");

use amdgpu_device::{amdgpu_device as Adev, amdgpu_pll};
use atom::{amdgpu_atom_parse_cmd_header, amdgpu_atom_parse_data_header, cstr};

fn pll(n: &str, p: &amdgpu_pll) {
    println!(
        "{} reference_freq {} reference_div {} post_div {}-{} ref_div {}-{} feedback_div {}-{} best_vco {}",
        n, p.reference_freq, p.reference_div, p.min_post_div, p.max_post_div, p.min_ref_div, p.max_ref_div,
        p.min_feedback_div, p.max_feedback_div, p.best_vco
    );
}

fn field(n: &str, s: &[u8]) {
    println!("{} '{}'", n, String::from_utf8_lossy(cstr(s)));
}

fn main() {
    let path = std::env::args().nth(1).expect("vbios.bin");
    let bios = std::fs::read(path).expect("read");
    let n = bios.len();
    let mut adev = Adev::new(0x164c, 0xc1, npk_sys::Mmio(0), 0);
    adev.bios = bios;
    // amdgpu_get_bios: asic_type CHIP_RENOIR >= CHIP_VEGA10
    adev.is_atom_fw = true;

    // amdgpu_atombios_init, with the fields printed between its steps as
    // the oracle does.
    adev.mode_info.atom_context = atom::amdgpu_atom_parse(adev.bios.clone());
    let Some(ctx) = adev.mode_info.atom_context.as_ref() else {
        println!("parse failed");
        std::process::exit(1);
    };
    println!("size {}", n);
    println!("cmd_table {:#x} data_table {:#x} version 0x{:08x}", ctx.cmd_table, ctx.data_table, ctx.version);
    field("name", &ctx.name);
    field("vbios_pn", &ctx.vbios_pn);
    field("date", &ctx.date);
    field("vbios_ver_str", &ctx.vbios_ver_str);
    field("build_num", &ctx.build_num);
    for (i, &v) in ctx.iio.iter().enumerate() {
        if v != 0 {
            println!("iio[{}] {:#x}", i, v);
        }
    }
    let nd = (ctx.cu16(ctx.data_table) as u32).wrapping_sub(4) / 2;
    for i in 0..nd {
        if let Some(h) = amdgpu_atom_parse_data_header(ctx, i) {
            println!("data[{}] at {:#x} size {} rev {}.{}", i, h.data_start, h.size, h.frev, h.crev);
        }
    }
    let nc = (ctx.cu16(ctx.cmd_table) as u32).wrapping_sub(4) / 2;
    for i in 0..nc {
        if let Some((frev, crev)) = amdgpu_atom_parse_cmd_header(ctx, i) {
            println!("cmd[{}] rev {}.{}", i, frev, crev);
        }
    }

    amdgpu_atomfirmware::amdgpu_atomfirmware_scratch_regs_init(&mut adev);
    let r = amdgpu_atomfirmware::amdgpu_atomfirmware_allocate_fb_scratch(&mut adev);
    adev.mode_info.firmware_flags = adev
        .mode_info
        .atom_context
        .as_ref()
        .map_or(0, amdgpu_atomfirmware::amdgpu_atomfirmware_query_firmware_capability);
    let ctx = adev.mode_info.atom_context.as_ref().unwrap();
    println!("bios_scratch_reg_offset {:#x}", adev.bios_scratch_reg_offset);
    println!("allocate_fb_scratch {} scratch_size_bytes {}", r, ctx.scratch_size_bytes);
    println!(
        "fw_vram_usage start {:#x} size {:#x} drv_vram_usage start {:#x} size {:#x}",
        adev.mman.fw_vram_usage_start_offset, adev.mman.fw_vram_usage_size,
        adev.mman.drv_vram_usage_start_offset, adev.mman.drv_vram_usage_size
    );
    println!("firmware_flags 0x{:08x}", adev.mode_info.firmware_flags);

    let r = amdgpu_atomfirmware::amdgpu_atomfirmware_get_clock_info(&mut adev);
    println!(
        "get_clock_info {} default_sclk {} default_mclk {} current_sclk {} current_mclk {}",
        r, adev.clock.default_sclk, adev.clock.default_mclk, adev.pm.current_sclk, adev.pm.current_mclk
    );
    pll("spll", &adev.clock.spll);
    pll("mpll", &adev.clock.mpll);

    adev.flags = regs::CHIP_RENOIR | regs::AMD_IS_APU;
    let (w, t, v, r) = match amdgpu_atomfirmware::amdgpu_atomfirmware_get_vram_info(&adev) {
        Ok((w, t, v)) => (w, t, v, 0),
        Err(e) => (0, 0, 0, e),
    };
    println!("get_vram_info {} width {} type {} vendor {}", r, w, t, v);

    // amdgpu_atombios_init as the driver runs it gives the same context.
    let mut again = Adev::new(0x164c, 0xc1, npk_sys::Mmio(0), 0);
    again.bios = adev.bios.clone();
    again.is_atom_fw = true;
    assert_eq!(amdgpu_atombios::amdgpu_atombios_init(&mut again), 0);
    assert_eq!(again.bios_scratch_reg_offset, adev.bios_scratch_reg_offset);
    assert_eq!(again.mode_info.firmware_flags, adev.mode_info.firmware_flags);
}
