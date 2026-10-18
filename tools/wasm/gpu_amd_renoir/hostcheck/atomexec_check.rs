//! The port's interpreter on the tables smu_v12_0_get_vbios_bootup_values
//! runs, printed as oracle/main_atomexec.c prints Linux's; with a third
//! argument, that one command table on 256 zero parameters.

extern crate alloc;

include!("modules.rs");

use amdgpu_device::amdgpu_device as Adev;
use atom_exec::{amdgpu_atom_execute_table, atom_card};
use regs::*;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

const MAX_READS: u32 = 200000;

struct Card {
    rule: u32,
    regs: RefCell<HashMap<u32, u32>>,
    reads: Cell<u32>,
}

impl atom_card for Card {
    fn reg_read(&self, reg: u32) -> u32 {
        let v = self.regs.borrow().get(&reg).copied().unwrap_or(match self.rule {
            0 => 0xffff_ffff,
            1 => 0,
            _ => reg.wrapping_mul(0x9e37_79b1) ^ (reg >> 7).wrapping_mul(0x85eb_ca6b),
        });
        println!("R {:05x} {:08x}", reg, v);
        self.reads.set(self.reads.get() + 1);
        if self.reads.get() >= MAX_READS {
            println!("read cap");
            std::process::exit(0);
        }
        v
    }

    fn reg_write(&self, reg: u32, val: u32) {
        println!("W {:05x} {:08x}", reg, val);
        self.regs.borrow_mut().insert(reg, val);
    }
}

fn clockinfo(ctx: &mut atom::atom_context, card: &Card, n: &str, clk_id: u32, syspll_id: u32) {
    // struct atom_get_smu_clock_info_parameters_v3_1
    let input = [syspll_id as u8, clk_id as u8, GET_SMU_CLOCK_INFO_V3_1_GET_CLOCK_FREQ as u8, 0];
    let mut ps = [u32::from_le_bytes(input)];
    println!("--- {}", n);
    let ret = amdgpu_atom_execute_table(ctx, card, IDX_getsmuclockinfo, &mut ps, 4);
    println!(
        "{} ret {} freq {} divmul {:08x} {:08x}",
        n, ret, if ret != 0 { 0 } else { ps[0] / 10000 }, ctx.divmul[0], ctx.divmul[1]
    );
}

fn main() {
    let mut args = std::env::args().skip(1);
    let bios = std::fs::read(args.next().expect("vbios.bin")).expect("read");
    let rule = match args.next().as_deref() {
        Some("ones") => 0,
        Some("zeros") => 1,
        _ => 2,
    };
    let mut adev = Adev::new(0x164c, 0xc1, npk_sys::Mmio(0), 0);
    adev.bios = bios;
    adev.is_atom_fw = true;
    adev.mode_info.atom_context = atom::amdgpu_atom_parse(adev.bios.clone());
    amdgpu_atomfirmware::amdgpu_atomfirmware_scratch_regs_init(&mut adev);
    amdgpu_atomfirmware::amdgpu_atomfirmware_allocate_fb_scratch(&mut adev);
    let card = Card { rule, regs: RefCell::new(HashMap::new()), reads: Cell::new(0) };
    let ctx = adev.mode_info.atom_context.as_mut().unwrap();
    if let Some(t) = args.next() {
        let mut ps = [0u32; 256];
        let r = amdgpu_atom_execute_table(ctx, &card, t.parse().unwrap(), &mut ps, 256);
        println!(
            "table {} ret {} divmul {:08x} {:08x} cs {} {}",
            t, r, ctx.divmul[0], ctx.divmul[1], ctx.cs_equal as i32, ctx.cs_above as i32
        );
        for (i, v) in ps.iter().enumerate() {
            if *v != 0 {
                println!("ps[{}] {:08x}", i, v);
            }
        }
        return;
    }
    clockinfo(ctx, &card, "socclk", SMU12_SYSPLL0_SOCCLK_ID, SMU12_SYSPLL0_ID);
    clockinfo(ctx, &card, "dcefclk", SMU12_SYSPLL1_DCFCLK_ID, SMU12_SYSPLL1_ID);
    clockinfo(ctx, &card, "vclk", SMU12_SYSPLL0_VCLK_ID, SMU12_SYSPLL0_ID);
    clockinfo(ctx, &card, "dclk", SMU12_SYSPLL0_DCLK_ID, SMU12_SYSPLL0_ID);
    clockinfo(ctx, &card, "fclk", SMU12_SYSPLL3_0_FCLK_ID, SMU12_SYSPLL3_0_ID);
}
