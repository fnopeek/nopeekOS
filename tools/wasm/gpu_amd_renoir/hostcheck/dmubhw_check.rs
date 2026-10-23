//! The port's DMCUB bring-up, printed as oracle/main_dmubhw.c prints
//! Linux's: the DMUB regions of dm_dmub_sw_init, then dm_dmub_hw_init on a
//! card whose DMCUB is "off", "cold" or "running" (main_dmubhw.c says what
//! each answers), every register write, the DMUB state and a checksum of
//! each window; with "cmd" DC then sends the D0 power state over the
//! inbox ring. Unwritten registers read hash(reg).

extern crate alloc;

include!("modules.rs");

use amdgpu_device::amdgpu_device as Adev;
use dmub_dcn20::dmub_srv_dcn21_regs as R;
use regs::*;

fn hash(reg: u32) -> u32 {
    reg.wrapping_mul(0x9e37_79b1) ^ 0xa5a5_a5a5
}

/// A running DMCUB: it clears the status of a GPINT and answers STOP_FW,
/// reports dal_fw and mailbox_rdy once enabled out of reset, and consumes
/// the inbox ring as soon as its write pointer moves.
fn dmcub(reg: u32, v: u32) {
    if reg == R.offset.DMCUB_INBOX1_WPTR {
        npk_sys::fake_set_reg(R.offset.DMCUB_INBOX1_RPTR, v);
    }
    if reg == R.offset.DMCUB_GPINT_DATAIN1 && v >> 28 != 0 {
        npk_sys::fake_set_reg(reg, v & 0x0fff_ffff);
        if (v >> 16) & 0xfff == DMUB_GPINT__STOP_FW {
            npk_sys::fake_set_reg(R.offset.DMCUB_SCRATCH7, DMUB_GPINT__STOP_FW_RESPONSE);
        }
    }
    if reg == R.offset.DMCUB_CNTL && v & R.mask.DMCUB_CNTL__DMCUB_ENABLE != 0
        && v & R.mask.DMCUB_CNTL__DMCUB_SOFT_RESET == 0
    {
        npk_sys::fake_set_reg(R.offset.DMCUB_SCRATCH0, 3);
    }
}

fn parse(s: &str) -> u64 {
    match s.strip_prefix("0x") {
        Some(h) => u64::from_str_radix(h, 16).unwrap(),
        None => s.parse().unwrap(),
    }
}

fn fnv(p: &[u8]) -> u32 {
    p.iter().fold(0x811c_9dc5u32, |h, &b| (h ^ b as u32).wrapping_mul(0x0100_0193))
}

fn rb(n: &str, r: &dmub_srv::dmub_rb) {
    println!("{} base {:#x} capacity {:#x} rptr {} wrpt {}", n, r.base_address, r.capacity, r.rptr, r.wrpt);
}

fn main() {
    let mut args = std::env::args().skip(1);
    let fw = std::fs::read(args.next().expect("dmcub.bin")).expect("read");
    let bios = std::fs::read(args.next().expect("vbios.bin")).expect("read");
    let model = args.next().expect("off|cold|running");
    let psp = args.next().expect("psp|direct") == "psp";
    let fb_start = parse(&args.next().expect("fb_start"));
    let vram_base_offset = parse(&args.next().expect("vram_base_offset"));
    let gpu = parse(&args.next().expect("buffer gpu address"));
    let cmd = args.next().as_deref() == Some("cmd");

    npk_sys::fake_card(npk_sys::FakeCard {
        regs: Default::default(),
        mm_index: !0,
        mm_index_hi: !0,
        mm_data: !0,
        vram_base: 0,
        vram: Vec::new(),
        on_write: Vec::new(),
        writes: Vec::new(),
        psp_wptr: None,
    });
    npk_sys::fake_card_unset(hash, (!0 - 1, !0 - 2));
    npk_sys::quiet(true);
    if model == "off" {
        npk_sys::fake_set_reg(R.offset.CC_DC_PIPE_DIS, 0);
    } else {
        npk_sys::fake_set_reg(R.offset.CC_DC_PIPE_DIS, R.mask.CC_DC_PIPE_DIS__DC_DMCUB_ENABLE);
        if model == "running" {
            npk_sys::fake_set_reg(R.offset.DMCUB_CNTL, R.mask.DMCUB_CNTL__DMCUB_ENABLE);
            npk_sys::fake_write_hook(dmcub);
        } else {
            npk_sys::fake_set_reg(R.offset.DMCUB_CNTL, R.mask.DMCUB_CNTL__DMCUB_SOFT_RESET);
        }
    }

    let mut adev = Adev::new(0x1636, 0xd1, npk_sys::Mmio(0), 0x100000 * 4);
    adev.bios = bios;
    adev.firmware.load_type = if psp { AMDGPU_FW_LOAD_PSP } else { AMDGPU_FW_LOAD_DIRECT };
    adev.gmc.fb_start = fb_start;
    adev.vm_manager.vram_base_offset = vram_base_offset;

    // dm_dmub_sw_init: the regions, rebased on a buffer at `gpu`.
    let le32 = |at: u32| u32::from_le_bytes(fw[at as usize..at as usize + 4].try_into().unwrap());
    adev.dm.dmcub_fw_version = le32(UCODE_HDR_UCODE_VERSION);
    let mut dmub = dmub_srv::dmub_srv::default();
    let mut region_info = dmub_srv::dmub_srv_region_info::default();
    let r = amdgpu_dm::dm_dmub_regions(&fw, adev.bios.len() as u32, DMUB_ASIC_DCN21, &mut dmub, &mut region_info);
    let mut fb_info = dmub_srv::dmub_srv_fb_info::default();
    dmub_srv::dmub_srv_calc_mem_info(&dmub, &region_info, &amdgpu_dm::WINDOW_MEMORY_TYPE, (0, gpu), (0, 0), &mut fb_info);
    println!("sw_init {} dmcub_fw_version 0x{:08x} fw_version 0x{:08x}", r, adev.dm.dmcub_fw_version, dmub.fw_version);
    let pages = region_info.fb_size.div_ceil(4096) as i32;
    let buf = npk_sys::Dma::alloc(pages).expect("dma");
    adev.dm.dmub_bo_cpu_addr = Some(amdgpu_object::amdgpu_kptr::gtt(buf.handle));
    adev.dm.dmub_srv = Some(Box::new(dmub));
    adev.dm.dmub_fb_info = Some(fb_info);
    adev.dm_dmub_fw = Some(linux::firmware { data: fw.clone() });

    adev.dm.dc = Some(Box::new(dc::dc::default()));
    npk_sys::fake_card_echo(true);
    let r = amdgpu_dm::dm_dmub_hw_init(&mut adev);
    npk_sys::fake_card_echo(false);

    let d = amdgpu_dm::dm_dmub_srv(&adev).unwrap();
    println!(
        "hw_init {} hw_init {} dc_dmub_srv {}",
        r, d.hw_init as u32, adev.dm.dc.as_ref().unwrap().ctx.dmub_srv.is_some() as u32
    );
    println!(
        "fb_base {:#x} fb_offset {:#x} psp_version {} power_state {} inbox1_last_wptr {}",
        d.fb_base, d.fb_offset, d.psp_version, d.power_state, d.inbox1_last_wptr
    );
    rb("inbox1", &d.inbox1.rb);
    rb("outbox1", &d.outbox1_rb);
    rb("outbox0", &d.outbox0_rb);
    println!("fw_state {:#x} shared_state {:#x}", d.fw_state.unwrap_or(0), d.shared_state.unwrap_or(0));
    println!(
        "scratch_mem gpu {:#x} size {:#x} ib_mem gpu {:#x} size {:#x}",
        d.scratch_mem_fb.gpu_addr, d.scratch_mem_fb.size, d.ib_mem_gart.gpu_addr, d.ib_mem_gart.size
    );
    if cmd {
        let mut dc = adev.dm.dc.take().unwrap();
        npk_sys::fake_card_echo(true);
        dc_dmub_srv::dc_dmub_srv_notify_fw_dc_power_state(&adev, &mut dc, DC_ACPI_CM_POWER_STATE_D0);
        npk_sys::fake_card_echo(false);
        let dc_srv = dc.ctx.dmub_srv.as_deref().unwrap();
        println!("optimized_init_done {}", dc_dmub_srv::dc_dmub_srv_optimized_init_done(&adev, Some(dc_srv)) as u32);
        let d = dc_srv.dmub.as_deref().unwrap();
        println!(
            "inbox1 rptr {} wrpt {} last_wptr {} submitted {} reported {}",
            d.inbox1.rb.rptr, d.inbox1.rb.wrpt, d.inbox1_last_wptr, d.inbox1.num_submitted, d.inbox1.num_reported
        );
        let mem = npk_sys::window_bytes(buf.handle);
        print!("slot0");
        for b in &mem[d.inbox1.rb.base_address as usize..][..DMUB_RB_CMD_SIZE as usize] {
            print!(" {:02x}", b);
        }
        println!();
        let g = &d.debug;
        println!(
            "timeout {} cmd {:02x} {:02x} version 0x{:08x}",
            g.timeout_info.timeout_occured as u32, g.timeout_info.timeout_cmd.0[0], g.timeout_info.timeout_cmd.0[1],
            g.dmcub_version
        );
        print!("scratch");
        for s in &g.scratch[..16] {
            print!(" {:08x}", s);
        }
        println!(
            "\nfaults {:08x} {:08x} {:08x} inbox1 {:08x} {:08x} {:08x} inbox0 {:08x} {:08x} {:08x}",
            g.undefined_address_fault_addr, g.inst_fetch_fault_addr, g.data_write_fault_addr, g.inbox1_rptr,
            g.inbox1_wptr, g.inbox1_size, g.inbox0_rptr, g.inbox0_wptr, g.inbox0_size
        );
        println!(
            "flags {} {} {} {} {} {}",
            g.is_dmcub_enabled as u32, g.is_dmcub_soft_reset as u32, g.is_dmcub_secure_reset as u32,
            g.is_traceport_en as u32, g.is_cw0_enabled as u32, g.is_cw6_enabled as u32
        );
        adev.dm.dc = Some(dc);
    }
    let mem = npk_sys::window_bytes(buf.handle);
    let fb = adev.dm.dmub_fb_info.as_ref().unwrap();
    for (i, f) in fb.fb.iter().enumerate() {
        let w = &mem[f.cpu_off as usize..f.cpu_off as usize + f.size as usize];
        println!("window {} gpu {:#x} size {:#x} sum 0x{:08x}", i, f.gpu_addr, f.size, fnv(w));
    }
}
