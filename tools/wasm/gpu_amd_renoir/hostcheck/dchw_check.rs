//! The port's DC bring-up, printed as oracle/main_dchw.c prints Linux's:
//! amdgpu_dm_init up to dc_hardware_init on the VBIOS, then a DMUB service
//! attached and dc_hardware_init. Unwritten registers read hash / ones /
//! zeros / lit / boot; the VBIOS SMC answers every message. For boot the
//! third argument is the oracle's output, whose `preset` lines are the boot
//! registers Linux's tables name. Then, as amdgpu_dm_init goes on,
//! mmhub_read_system_context and dc_setup_system_context on the oracle's GMC
//! layout, with the GART table a VRAM buffer. Every register write is
//! printed; the DMUB commands are read back off the inbox ring, then the
//! state init_hw leaves.

extern crate alloc;

include!("modules.rs");

use amdgpu_device::amdgpu_device as Adev;
use regs::*;

fn hash(reg: u32) -> u32 {
    reg.wrapping_mul(0x9e37_79b1) ^ 0xa5a5_a5a5
}

fn ones(_reg: u32) -> u32 {
    !0
}

fn zeros(_reg: u32) -> u32 {
    0
}

/// All ones, but no pipe fused off: every block reads as running.
fn lit(reg: u32) -> u32 {
    if reg == CC_DC_PIPE_DIS { 0 } else { !0 }
}

/// The VBIOS SMC answers every message OK with the SMU version; the DMCUB
/// consumes the inbox ring as soon as its write pointer moves.
fn device(reg: u32, v: u32) {
    if reg == RN_VBIOS_MP1_SMN_C2PMSG_67 {
        npk_sys::fake_set_reg(RN_VBIOS_MP1_SMN_C2PMSG_83, 0x373700);
        npk_sys::fake_set_reg(RN_VBIOS_MP1_SMN_C2PMSG_91, 1);
    }
    if reg == DMUB_DMCUB_INBOX1_WPTR {
        npk_sys::fake_set_reg(DMUB_DMCUB_INBOX1_RPTR, v);
    }
}

fn print_range(n: &str, i: u32, r: &dm_pp_smu::pp_smu_wm_set_range) {
    println!(
        "  {} {} inst {} type {} fill {} {} drain {} {}",
        n, i, r.wm_inst, r.wm_type, r.min_fill_clk_mhz, r.max_fill_clk_mhz, r.min_drain_clk_mhz, r.max_drain_clk_mhz
    );
}

fn main() {
    let mut args = std::env::args().skip(1);
    let bios = std::fs::read(args.next().expect("vbios.bin")).expect("read");
    let mode = args.next().unwrap_or_default();
    let rule: fn(u32) -> u32 = match mode.as_str() {
        "ones" => ones,
        "zeros" => zeros,
        "lit" | "boot" => lit,
        _ => hash,
    };
    let presets: Vec<(u32, u32)> = if mode == "boot" {
        let text = std::fs::read_to_string(args.next().expect("oracle output")).expect("read");
        text.lines()
            .filter_map(|l| l.strip_prefix("preset reg 0x"))
            .map(|l| {
                let (r, v) = l.split_once(" = 0x").expect("preset line");
                (u32::from_str_radix(r, 16).unwrap(), u32::from_str_radix(v, 16).unwrap())
            })
            .collect()
    } else {
        Vec::new()
    };

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
    npk_sys::fake_card_unset(rule, (!0 - 1, !0 - 2));
    npk_sys::fake_write_hook(device);
    npk_sys::fake_card_echo_skip(&[DMUB_DMCUB_INBOX1_WPTR]);
    npk_sys::quiet(true);

    let mut adev = Adev::new(0x164c, 0xc1, npk_sys::Mmio(0), 0x100000 * 4);
    adev.subsystem_vendor = 0x17aa;
    adev.subsystem_device = 0x3f1a;
    adev.asic_type = 24;
    adev.flags = 0x20000;
    adev.dm.dmcub_fw_version = 0x0101002b;
    adev.ip_versions[DCE_HWIP as usize][0] = 0x02010000;
    adev.family = 142;
    adev.external_rev_id = 0x91;
    adev.gmc.vram_width = 64;
    adev.dm.cgs_device = true;
    adev.reg_offset[DCE_HWIP as usize][0] = vec![0x12, 0xc0, 0x34c0, 0x9000, 0x2403c00];
    adev.reg_offset[NBIO_HWIP as usize][0] = vec![0, 0x14, 0xd20, 0x10400, 0x241b000];
    adev.reg_offset[CLK_HWIP as usize][0] = vec![0x16c00, 0x16e00, 0x17000, 0x17200, 0x1b000];
    adev.bios = bios;
    adev.mode_info.atom_context = atom::amdgpu_atom_parse(adev.bios.clone());
    adev.smu.pm_enabled = true;
    adev.apu_flags = AMD_APU_IS_RENOIR;
    adev.gmc.vram_start = 0xf4_0000_0000;
    adev.gmc.fb_start = 0xf4_0000_0000;
    adev.gmc.fb_end = 0xf4_0000_0000 + 0x2000_0000 - 1;
    adev.gmc.gart_start = 0;
    adev.gmc.gart_end = 0x4000_0000 - 1;
    adev.gmc.agp_start = 0xf8_0000_0000;
    adev.gmc.agp_end = 0xffff_ffff_ffff;
    adev.vm_manager.vram_base_offset = 0x6000_0000;
    adev.mman.bos.push(Some(amdgpu_object::amdgpu_bo {
        tbo: ttm::ttm_buffer_object {
            resource: Some(ttm::ttm_resource { start: 0x1ff00, size: 0x100000, mem_type: TTM_PL_VRAM,
                ..Default::default() }),
            ..Default::default()
        },
        flags: 0,
        preferred_domains: 0,
        allowed_domains: 0,
        placements: Vec::new(),
        xcp_id: 0,
        kptr: None,
        ref_count: 1,
        parent: None,
        vm_bo: None,
        _resv: Default::default(),
        resv: None,
        deleted: false,
        entries: Vec::new(),
        client: None,
        pool: None,
        tiling_flags: 0,
        metadata_flags: 0,
        metadata: Vec::new(),
    }));
    adev.gart.bo = Some(adev.mman.bos.len() - 1);

    npk_sys::fake_card_echo(true);
    println!("amdgpu_dm_init {}", amdgpu_dm::amdgpu_dm_init(&mut adev));
    let mut dc = adev.dm.dc.take().expect("dc");
    for &(r, v) in &presets {
        println!("preset reg {:#x} = {:#x}", r, v);
        npk_sys::fake_set_reg(r, v);
    }

    let ring = npk_sys::Dma::alloc((DMUB_RB_SIZE / 4096) as i32).expect("dma");
    let mut dmub = Box::new(dmub_srv::dmub_srv::default());
    dmub_srv::dmub_srv_create(&mut dmub, DMUB_ASIC_DCN21, 0);
    dmub.hw_init = true;
    dmub.power_state = DMUB_POWER_STATE_D0;
    dmub.cpu = Some(amdgpu_object::amdgpu_kptr::gtt(ring.handle));
    dmub.inbox1.rb.capacity = DMUB_RB_SIZE;
    dc.ctx.dmub_srv = dc_dmub_srv::dc_dmub_srv_create(dmub);

    println!("dc_hardware_init");
    dc::dc_hardware_init(&adev, &mut dc);

    println!("dc_setup_system_context");
    if adev.flags & AMD_IS_APU != 0 && adev.asic_type >= CHIP_CARRIZO {
        let pa_config = amdgpu_dm::mmhub_read_system_context(&adev);
        let a = &pa_config.system_aperture;
        println!(
            "pa_config aperture {:#x} {:#x} fb {:#x} {:#x} {:#x} agp {:#x} {:#x} {:#x}",
            a.start_addr, a.end_addr, a.fb_base, a.fb_top, a.fb_offset, a.agp_base, a.agp_bot, a.agp_top
        );
        let g = &pa_config.gart_config;
        println!(
            "pa_config gart {:#x} {:#x} {:#x} hvm {}",
            g.page_table_start_addr, g.page_table_end_addr, g.page_table_base_addr, pa_config.is_hvm_enabled as u32
        );

        println!("num_vmids {}", dc::dc_setup_system_context(&adev, &mut dc, &pa_config));
        println!(
            "riommu_active {} vm_pa_config valid {}",
            dc.res_pool.as_deref().unwrap().hubbub.as_deref().unwrap().base.riommu_active as u32,
            dc.vm_pa_config.valid as u32
        );
    }
    npk_sys::fake_card_echo(false);

    let wrpt = dc.ctx.dmub_srv.as_deref().unwrap().dmub.as_deref().unwrap().inbox1.rb.wrpt;
    let mem = npk_sys::window_bytes(ring.handle);
    for slot in mem[..wrpt as usize].chunks(DMUB_RB_CMD_SIZE as usize) {
        print!("dmub cmd");
        for b in slot {
            print!(" {:02x}", b);
        }
        println!();
    }

    let ranges = &dc.clk_mgr.as_deref().unwrap().base.ranges;
    println!("wm_ranges reader {} writer {}", ranges.num_reader_wm_sets, ranges.num_writer_wm_sets);
    for i in 0..ranges.num_reader_wm_sets {
        print_range("reader", i, &ranges.reader_wm_sets[i as usize]);
    }
    for i in 0..ranges.num_writer_wm_sets {
        print_range("writer", i, &ranges.writer_wm_sets[i as usize]);
    }
    let pool = dc.res_pool.as_deref().unwrap();
    println!(
        "ref_clocks xtalin {} dccg {} dchub {}",
        pool.ref_clocks.xtalin_clock_inKhz, pool.ref_clocks.dccg_ref_clock_inKhz, pool.ref_clocks.dchub_ref_clock_inKhz
    );
    for (l, k) in dc.links.iter().enumerate() {
        println!(
            "link {} sink_present {} active {} fec {}",
            l, k.edp_sink_present as u32, k.link_status.link_active as u32, k.fec_state
        );
    }
}
