//! The port's amdgpu_dm_init up to dc_hardware_init, printed as
//! oracle/main_dminit.c prints Linux's: the dc_init_data that reaches
//! dc_create (read back from the dc it built), then the debug members
//! amdgpu_dm_init writes. The port runs the real dc_create on the VBIOS
//! with the VBIOS SMC answering; there is no DMUB service, so
//! dm_dmub_hw_init returns at once.

extern crate alloc;

include!("modules.rs");

use amdgpu_device::amdgpu_device as Adev;
use regs::*;

fn hash(reg: u32) -> u32 {
    reg.wrapping_mul(0x9e37_79b1) ^ 0xa5a5_a5a5
}

/// The VBIOS SMC answers every message OK.
fn vbios_smc(reg: u32, _v: u32) {
    if reg == RN_VBIOS_MP1_SMN_C2PMSG_67 {
        npk_sys::fake_set_reg(RN_VBIOS_MP1_SMN_C2PMSG_83, 0x373700);
        npk_sys::fake_set_reg(RN_VBIOS_MP1_SMN_C2PMSG_91, 1);
    }
}

fn parse(s: &str) -> u32 {
    match s.strip_prefix("0x") {
        Some(h) => u32::from_str_radix(h, 16).unwrap(),
        None => s.parse().unwrap(),
    }
}

fn offs(n: &str, o: &[u32]) {
    print!("{}", n);
    for i in 0..5 {
        print!(" {:#x}", o.get(i).copied().unwrap_or(0));
    }
    println!();
}

fn main() {
    let mut args = std::env::args().skip(1);
    let bios = std::fs::read(args.next().expect("vbios.bin")).expect("read");
    let mut a = || parse(&args.next().expect("argument"));
    let (asic_type, flags, apu_flags, dmcub, dce_ip) = (a(), a(), a(), a(), a());
    let (device, revision, sub_vendor, sub_device) = (a(), a(), a(), a());

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
    npk_sys::fake_write_hook(vbios_smc);
    npk_sys::quiet(true);

    let mut adev = Adev::new(device as u16, revision as u8, npk_sys::Mmio(0), 0x100000 * 4);
    adev.subsystem_vendor = sub_vendor as u16;
    adev.subsystem_device = sub_device as u16;
    adev.asic_type = asic_type;
    adev.flags = flags;
    adev.apu_flags = apu_flags;
    adev.dm.dmcub_fw_version = dmcub;
    adev.ip_versions[DCE_HWIP as usize][0] = dce_ip;
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

    let r = amdgpu_dm::amdgpu_dm_init(&mut adev);

    let dc = adev.dm.dc.as_deref().expect("dc");
    let id = &dc.ctx.asic_id;
    let f = &dc.config;
    println!(
        "asic_id family {} rev {} internal {} chip {:#x} vram_width {}",
        id.chip_family, id.pci_revision_id, id.hw_internal_rev, id.chip_id, id.vram_width
    );
    let virtual_links = dc.links.iter().filter(|l| l.connector_signal == LF_SIGNAL_TYPE_VIRTUAL).count();
    println!(
        "env {} cgs {} virtual_links {} bb_from_dmub {}",
        dc.ctx.dce_environment as u32, dc.ctx.cgs_device as u32, virtual_links, adev.dm.bb_from_dmub as u32
    );
    println!(
        "flags disable_dmcu {} gpu_vm {} fbc {} multi_mon {} frac_pwm {} edp_no_pwr {} lttpr {} {}",
        f.disable_dmcu as u32, f.gpu_vm_support as u32, f.fbc_support as u32, f.multi_mon_pp_mclk_switch as u32,
        f.disable_fractional_pwm as u32, f.edp_no_power_sequencing as u32,
        f.allow_lttpr_non_transparent_mode_dp1_4a as u32, f.allow_lttpr_non_transparent_mode_dp2_0 as u32
    );
    println!(
        "flags seamless {} {} mipi {} ips {} ips_vpb {} dtb0 {} edp0_dp1 {}",
        f.seamless_boot_edp_requested as u32, f.allow_seamless_boot_optimization as u32,
        f.enable_mipi_converter_optimization as u32, f.disable_ips, f.disable_ips_in_vpb, f.allow_0_dtb_clk as u32,
        f.support_edp0_on_dp1 as u32
    );
    offs("dcn", &dc.ctx.dcn_reg_offsets);
    offs("nbio", &dc.ctx.nbio_reg_offsets);
    offs("clk", &dc.ctx.clk_reg_offsets);
    let g = &dc.debug;
    println!(
        "amdgpu_dm_init {} gpu_vm_support {} quirks {} {}",
        r, adev.mode_info.gpu_vm_support as u32, adev.dm.aux_hpd_discon_quirk as u32, adev.dm.edp0_on_dp1_quirk as u32
    );
    println!(
        "debug split {} policy {} stutter {} dsc {} clock_gate {} subvp_mclk {} subvp_off {} dml2 {} {}",
        g.force_single_disp_pipe_split as u32, g.pipe_split_policy as u32, g.disable_stutter as u32,
        g.disable_dsc as u32, g.disable_clock_gate as u32, g.force_subvp_mclk_switch as u32,
        g.force_disable_subvp as u32, g.using_dml2 as u32, g.using_dml21 as u32
    );
    println!(
        "debug hdcp {} {} skip_lt {} visual_confirm {} ignore_cable_id {}",
        g.hdcp_lc_force_fw_enable as u32, g.hdcp_lc_enable_sw_fallback as u32, g.skip_detection_link_training as u32,
        g.visual_confirm, g.ignore_cable_id as u32
    );
}
