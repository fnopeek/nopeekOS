//! The port's DC clock manager, printed as oracle/main_clk.c prints Linux's:
//! the VBIOS parser on a VBIOS image, then rn_clk_mgr_construct on a card
//! whose unwritten registers read hash(reg) and whose VBIOS SMC mailbox
//! answers GetSmuVersion and IsPeriodicRetrainingDisabled as the arguments
//! say. "dpm" lets the SMU hand out its clock table, "nodpm" does not.

extern crate alloc;

include!("modules.rs");

use amdgpu_device::amdgpu_device as Adev;
use bios_parser2::{bp_init_data, dal_bios_parser_create};
use dc_types::dce_version;
use regs::*;
use std::cell::Cell;

thread_local! {
    static ANSWERS: Cell<(u32, u32)> = const { Cell::new((0, 0)) };
}

fn hash(reg: u32) -> u32 {
    reg.wrapping_mul(0x9e37_79b1) ^ 0xa5a5_a5a5
}

/// The VBIOS SMC: a message on C2PMSG_67 answers OK on C2PMSG_91 and its
/// value on C2PMSG_83.
fn vbios_smc(reg: u32, v: u32) {
    if reg != RN_VBIOS_MP1_SMN_C2PMSG_67 {
        return;
    }
    let (ver, retrain) = ANSWERS.with(|a| a.get());
    if v == RN_VBIOSSMC_MSG_GetSmuVersion {
        npk_sys::fake_set_reg(RN_VBIOS_MP1_SMN_C2PMSG_83, ver);
    } else if v == RN_VBIOSSMC_MSG_IsPeriodicRetrainingDisabled {
        npk_sys::fake_set_reg(RN_VBIOS_MP1_SMN_C2PMSG_83, retrain);
    }
    npk_sys::fake_set_reg(RN_VBIOS_MP1_SMN_C2PMSG_91, 1);
}

fn parse(s: &str) -> u32 {
    match s.strip_prefix("0x") {
        Some(h) => u32::from_str_radix(h, 16).unwrap(),
        None => s.parse().unwrap(),
    }
}

/// The SMU's DpmClocks_t as main_clk.c's get_dpm_clock_table hands it out.
fn clocks_table() -> Vec<u8> {
    let mut t = vec![0u8; SIZEOF_DpmClocks_t as usize];
    let mut put = |base: u32, i: u32, freq: u32, vol: u32| {
        let at = (base + i * SIZEOF_DpmClock_t) as usize;
        t[at + DPM_CLOCK_FREQ as usize..][..4].copy_from_slice(&freq.to_le_bytes());
        t[at + DPM_CLOCK_VOL as usize..][..4].copy_from_slice(&vol.to_le_bytes());
    };
    for i in 0..8 {
        put(DPM_CLOCKS_DCF_CLOCKS, i, 400 + 100 * i, 3000 + 50 * i);
        put(DPM_CLOCKS_SOC_CLOCKS, i, 200 + 60 * i, 3000 + 50 * i);
        put(DPM_CLOCKS_V_CLOCKS, i, 300 + 10 * i, 3000 + 50 * i);
        put(DPM_CLOCKS_D_CLOCKS, i, 250 + 10 * i, 3000 + 50 * i);
    }
    for i in 0..4 {
        put(DPM_CLOCKS_F_CLOCKS, i, 1600 - 400 * i, 3000 + 50 * (3 - i));
        put(DPM_CLOCKS_MEM_CLOCKS, i, 2133 - 400 * i, 3000 + 50 * (3 - i));
    }
    t
}

fn main() {
    let mut args = std::env::args().skip(1);
    let bios = std::fs::read(args.next().expect("vbios.bin")).expect("read");
    let ver = parse(&args.next().expect("smu version"));
    let retrain = parse(&args.next().expect("retraining answer"));
    let dpm = args.next().as_deref() == Some("dpm");
    ANSWERS.with(|a| a.set((ver, retrain)));

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

    let mut adev = Adev::new(0x164c, 0xc1, npk_sys::Mmio(0), 0x100000 * 4);
    adev.bios = bios;
    adev.mode_info.atom_context = atom::amdgpu_atom_parse(adev.bios.clone());
    adev.smu.pm_enabled = true;
    adev.pm.dpm_enabled = dpm;
    adev.smu.smu_table.clocks_table = clocks_table();

    let mut dc = Box::new(dc::dc::default());
    let init = bp_init_data { bios: &adev.bios };
    dc.ctx.dc_bios = dal_bios_parser_create(&adev, &init, dce_version::DCN_VERSION_2_1).expect("bios parser");
    dc.ctx.dce_version = dce_version::DCN_VERSION_2_1;
    dc.debug.min_disp_clk_khz = 100000;
    dc.debug.disable_48mhz_pwrdwn = false;
    let mut pool = Box::new(dc_resource::resource_pool::default());
    let mut pp = dm_pp_smu::pp_smu_funcs::default();
    dm_pp_smu::dm_pp_get_funcs(dce_version::DCN_VERSION_2_1, &mut pp);
    pool.pp_smu = Some(pp);
    dc.res_pool = Some(pool);

    let mut clk_mgr = Box::new(clk_mgr::clk_mgr_internal::default());
    rn_clk_mgr::rn_clk_mgr_construct(&adev, &mut dc, &mut clk_mgr);
    let clk_mgr = &*dc.clk_mgr.insert(clk_mgr);

    for (reg, v) in npk_sys::fake_card_writes() {
        println!("write reg {:#x} = {:#x}", reg, v);
    }
    println!(
        "smu_ver {:#x} retraining_disabled {} min_disp_clk_khz {}",
        clk_mgr.smu_ver, clk_mgr.periodic_retraining_disabled as u32, dc.debug.min_disp_clk_khz
    );
    println!(
        "dfs_ref_freq_khz {} dfs_bypass_disp_clk {} dentist_vco_freq_khz {} dprefclk_khz {}",
        clk_mgr.dfs_ref_freq_khz, clk_mgr.dfs_bypass_disp_clk, clk_mgr.base.dentist_vco_freq_khz,
        clk_mgr.base.dprefclk_khz
    );
    println!(
        "ss_on_dprefclk {} dprefclk_ss_percentage {} dprefclk_ss_divider {}",
        clk_mgr.ss_on_dprefclk as u32, clk_mgr.dprefclk_ss_percentage, clk_mgr.dprefclk_ss_divider
    );
    let s = &clk_mgr.base.boot_snapshot;
    println!(
        "boot dcfclk {} ds {} {} dprefclk {} dispclk {} dppclk {} bypass {} {} {} {}",
        s.dcfclk, s.dcf_deep_sleep_divider, s.dcf_deep_sleep_allow, s.dprefclk, s.dispclk, s.dppclk,
        s.dppclk_bypass, s.dcfclk_bypass, s.dprefclk_bypass, s.dispclk_bypass
    );
    let b = clk_mgr.base.bw_params.as_deref().unwrap();
    println!("bw vram_type {} num_channels {} num_entries {}", b.vram_type, b.num_channels, b.clk_table.num_entries);
    for (i, e) in b.clk_table.entries.iter().enumerate() {
        println!(
            "clk {} voltage {} dcfclk {} fclk {} memclk {} socclk {}",
            i, e.voltage, e.dcfclk_mhz, e.fclk_mhz, e.memclk_mhz, e.socclk_mhz
        );
    }
    for (i, w) in b.wm_table.entries.iter().enumerate() {
        println!(
            "wm {} inst {} type {} pstate {:.6} sr_exit {:.6} sr_enter_exit {:.6} valid {}",
            i, w.wm_inst, w.wm_type, w.pstate_latency_us, w.sr_exit_time_us, w.sr_enter_plus_exit_time_us,
            w.valid as u32
        );
    }

    // dc_construct then runs update_bw_bounding_box on this table; the
    // resource construct capped the IP at the live pipe count.
    {
        let pool = dc.res_pool.as_deref_mut().unwrap();
        pool.pipe_count = 4;
        pool.res_cap = Some(&dcn21_resource::RES_CAP_RN);
    }
    let mut ip = dcn20_fpu::dcn2_1_ip();
    ip.max_num_dpp = 4;
    ip.max_num_otg = 4;
    let dml = dc.dml.get_or_insert_with(dml_structs::DmlBox::zero);
    dml_structs::dml_init_instance(dml, &dcn20_fpu::dcn2_1_soc(), &ip, dml_structs::DML_PROJECT_DCN21);
    dcn20_fpu::dcn21_update_bw_bounding_box(&mut dc);
    let dml = dc.dml.as_deref().unwrap();
    let b = dc.clk_mgr.as_deref().unwrap().base.bw_params.as_deref().unwrap();
    println!(
        "bb dml_inits 1 max_num_otg {} max_num_dpp {} num_chans {} num_states {} clk_entries {}",
        dml.ip.max_num_otg, dml.ip.max_num_dpp, dml.soc.num_chans, dml.soc.num_states, b.clk_table.num_entries
    );
    for (i, e) in b.clk_table.entries.iter().enumerate() {
        println!(
            "bb clk {} voltage {} dcfclk {} fclk {} memclk {} socclk {}",
            i, e.voltage, e.dcfclk_mhz, e.fclk_mhz, e.memclk_mhz, e.socclk_mhz
        );
    }
    for i in 0..=dml.soc.num_states as usize {
        let l = &dml.soc.clock_limits[i];
        println!(
            "bb state {}: {} dcf {:.3} fab {:.3} soc {:.3} dram {:.3} disp {:.3} dpp {:.3} bw {:.3} dsc {:.3} dtb {:.3} \
             phy18 {:.3} phy {:.3}",
            i, l.state, l.dcfclk_mhz, l.fabricclk_mhz, l.socclk_mhz, l.dram_speed_mts, l.dispclk_mhz, l.dppclk_mhz,
            l.dram_bw_per_chan_gbps, l.dscclk_mhz, l.dtbclk_mhz, l.phyclk_d18_mhz, l.phyclk_mhz
        );
    }
}
