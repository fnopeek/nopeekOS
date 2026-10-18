//! The port's microcode requests on a directory of firmware files, printed
//! line for line as oracle/main_ucode.c prints Linux's: the same functions
//! in early_init's block order, from the same state.

extern crate alloc;

include!("modules.rs");

use amdgpu_device::amdgpu_device as Adev;
use psp_image_names::*;
use regs::*;

mod psp_image_names {
    pub use crate::amdgpu_psp::psp_bin_desc;
}

fn d(name: &str, c: &psp_bin_desc) {
    let at = c.start_addr.map_or(-1, |(_, o)| o as i64);
    println!("{} fw_version {:#x} feature {:#x} size {} at {}", name, c.fw_version, c.feature_version, c.size_bytes, at);
}

fn main() {
    npk_sys::fake_firmware_dir(&std::env::args().nth(1).expect("firmware dir"));
    let mut a = Adev::new(0x164c, 0xc1, npk_sys::Mmio(0), 0);
    a.subsystem_vendor = 0x17aa;
    a.subsystem_device = 0x3806;
    a.apu_flags = AMD_APU_IS_RENOIR;
    a.flags = AMD_IS_APU;
    a.external_rev_id = 0x91;
    a.firmware.load_type = AMDGPU_FW_LOAD_PSP;
    a.pm.pp_feature = 0xfff7bfff;
    a.pg_flags = 0x0002c100;
    a.ip_versions[GC_HWIP as usize][0] = 9 << 24 | 3 << 16;
    a.ip_versions[SDMA0_HWIP as usize][0] = 4 << 24 | 1 << 16 | 2 << 8;
    a.ip_versions[MP0_HWIP as usize][0] = 11 << 24 | 3 << 8;
    a.ip_versions[UVD_HWIP as usize][0] = 2 << 24 | 2 << 16;
    a.ip_versions[DCE_HWIP as usize][0] = 2 << 24 | 1 << 16;
    a.sdma.num_instances = 1;
    a.gfx.num_gfx_rings = 1;

    println!("psp_v12_0_init_microcode {}", amdgpu_psp::psp_v12_0_init_microcode(&mut a));
    println!("dm_init_microcode {}", amdgpu_dm::dm_init_microcode(&mut a));
    println!("gfx_v9_0_init_microcode {}", gfx_v9_0::gfx_v9_0_init_microcode(&mut a));
    println!("sdma_v4_0_init_microcode {}", sdma_v4_0::sdma_v4_0_early_init(&mut a));
    println!("amdgpu_vcn_early_init {}", vcn_v2_0::amdgpu_vcn_early_init(&mut a, 0));

    let p = &a.psp;
    d("asd", &p.asd_context.bin_desc);
    d("xgmi", &p.xgmi_context.context.bin_desc);
    d("ras", &p.ras_context.context.bin_desc);
    d("hdcp", &p.hdcp_context.context.bin_desc);
    d("dtm", &p.dtm_context.context.bin_desc);
    d("rap", &p.rap_context.context.bin_desc);
    d("securedisplay", &p.securedisplay_context.context.bin_desc);
    println!("ta_fw_version {:#x}", p.ta_fw_version);
    let g = &a.gfx;
    println!("pfp {:#x}/{} me {:#x}/{} ce {:#x}/{} mec {:#x}/{} mec2 {:#x}/{} rlc {:#x}/{}", g.pfp_fw_version,
             g.pfp_feature_version, g.me_fw_version, g.me_feature_version, g.ce_fw_version, g.ce_feature_version,
             g.mec_fw_version, g.mec_feature_version, g.mec2_fw_version, g.mec2_feature_version, g.rlc_fw_version,
             g.rlc_feature_version);
    println!("srlc {:#x}/{} srlg {:#x}/{} srls {:#x}/{} rlcp {:#x}/{} rlcv {:#x}/{}", g.rlc_srlc_fw_version,
             g.rlc_srlc_feature_version, g.rlc_srlg_fw_version, g.rlc_srlg_feature_version, g.rlc_srls_fw_version,
             g.rlc_srls_feature_version, g.rlcp_ucode_version, g.rlcp_ucode_feature_version, g.rlcv_ucode_version,
             g.rlcv_ucode_feature_version);
    let r = &g.rlc;
    println!("rlc v2_1 {} sr_offset {:#x} csd {:#x} scratch {} restore_size {} fmt_start {} fmt_sep {} starts {}",
             r.is_rlc_v2_1 as i32, r.save_and_restore_offset, r.clear_state_descriptor_offset,
             r.avail_scratch_ram_locations, r.reg_restore_list_size, r.reg_list_format_start,
             r.reg_list_format_separate_start, r.starting_offsets_start);
    let mut sum: u64 = 0;
    for &w in &r.register_list_format {
        sum = sum.wrapping_mul(31).wrapping_add(w as u64);
    }
    println!("rlc lists {}+{} words, restore at {}, hash {:#x}", r.reg_list_format_size_bytes / 4,
             r.reg_list_size_bytes / 4, r.register_restore_at, sum);
    macro_rules! S { ($($f:ident, $sz:ident);*) => { $(println!("rlc {} size {} at {}", stringify!($f), r.$sz,
        r.$f.map_or(-1, |o| o as i64));)* } }
    S!(save_restore_list_cntl, save_restore_list_cntl_size_bytes; save_restore_list_gpm, save_restore_list_gpm_size_bytes;
       save_restore_list_srm, save_restore_list_srm_size_bytes; rlc_iram_ucode, rlc_iram_ucode_size_bytes;
       rlc_dram_ucode, rlc_dram_ucode_size_bytes; rlcp_ucode, rlcp_ucode_size_bytes; rlcv_ucode, rlcv_ucode_size_bytes;
       global_tap_delays_ucode, global_tap_delays_ucode_size_bytes; se0_tap_delays_ucode, se0_tap_delays_ucode_size_bytes;
       se1_tap_delays_ucode, se1_tap_delays_ucode_size_bytes; se2_tap_delays_ucode, se2_tap_delays_ucode_size_bytes;
       se3_tap_delays_ucode, se3_tap_delays_ucode_size_bytes);
    println!("rlc direct_reg_list_length {}", r.reg_list_format_direct_reg_list_length);
    println!("write_wait me {} mec {} pg_flags {:#x} pp_feature {:#x}", g.me_fw_write_wait as i32,
             g.mec_fw_write_wait as i32, a.pg_flags, a.pm.pp_feature);
    for i in 0..a.sdma.num_instances as usize {
        let s = &a.sdma.instance[i];
        println!("sdma{} fw_version {:#x} feature {} burst_nop {}", i, s.fw_version, s.feature_version, s.burst_nop as i32);
    }
    println!("sdma has_page_queue {}", a.sdma.has_page_queue as i32);
    println!("vcn fw {} dmub fw {}", a.vcn.inst[0].fw.as_ref().map_or(0, |f| f.data.len()),
             a.dm_dmub_fw.as_ref().map_or(0, |f| f.data.len()));
    use amdgpu_ucode::fw_source as F;
    for (i, u) in a.firmware.ucode.iter().enumerate() {
        let Some(src) = u.fw else { continue };
        let name = match src {
            F::gfx_pfp => "pfp", F::gfx_me => "me", F::gfx_ce => "ce", F::gfx_mec => "mec", F::gfx_rlc => "rlc",
            F::sdma(0) => "sdma0", _ => "?",
        };
        println!("ucode[{}] id {} from {}", i, u.ucode_id, name);
    }
    println!("fw_size {}", a.firmware.fw_size);
}
