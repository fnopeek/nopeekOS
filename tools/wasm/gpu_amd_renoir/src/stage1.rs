//! Stage 1 of the port (docs/plan/GPU_AMD_LINUX_MAP.md §1), parts a and b:
//! `amdgpu_device_init` through `amdgpu_device_ip_early_init`. The IP
//! discovery, the IP block list, `soc15_common_early_init`, the VBIOS from
//! VFCT, `amdgpu_atombios_init`, and the early_init of the nine blocks
//! after COMMON, which request and read their microcode; then what it
//! found, for the device log. Nothing is written to the card but the
//! MM_INDEX window VRAM is read through.

use crate::amdgpu_device::{self as dev, amdgpu_device};
use crate::atom::cstr;
use crate::linux::Line;
use crate::regs;

/// The stage on a bound card whose BAR5 is mapped; `bdf` is its bus
/// address, for matching the VFCT image.
pub fn run(adev: &mut amdgpu_device, bdf: (u8, u8, u8)) {
    dev::amdgpu_device_init_apu_flags(adev);
    Line::new().s("apu_flags ").hex(adev.apu_flags).s(if adev.apu_flags & regs::AMD_APU_IS_RENOIR != 0 {
        " (AMD_APU_IS_RENOIR: renoir_* firmware)"
    } else {
        ""
    }).say();
    // amdgpu_device_check_arguments: the firmware load type.
    adev.firmware.load_type = dev::amdgpu_ucode_get_load_type(crate::tables::amdgpu_fw_load_type[0]);
    dev::amdgpu_device_set_mcbp(adev);

    let r = dev::amdgpu_device_ip_early_init(adev, bdf);
    if !adev.mman.discovery_bin.is_empty() {
        report_ip_versions(adev);
        compare_dc_bases(adev);
    }
    match r {
        Ok(n) => {
            report(adev);
            report_microcode(adev);
            let valid = adev.ip_blocks.iter().filter(|b| b.status.valid).count();
            Line::new().s("stage 1b done: early_init of ").dec(n as u64).s(" blocks, ").dec(valid as u64)
                .s(" valid").say();
        }
        Err(e) => {
            Line::new().s("amdgpu_device_ip_early_init failed (").dec((-e) as u64).s(")").say();
        }
    }
}

/// What the COMMON block and `amdgpu_atombios_init` left in `adev`.
fn report(adev: &amdgpu_device) {
    Line::new().s("family ").dec(adev.family as u64).s(", flags ").hex(adev.flags).s(", rev_id ").hex(adev.rev_id)
        .s(", external_rev_id ").hex(adev.external_rev_id).say();
    Line::new().s("cg_flags ").hex(adev.cg_flags as u32).s(", pg_flags ").hex(adev.pg_flags).s(", pp_feature ")
        .hex(adev.pm.pp_feature).s(", rmmio_remap ").hex(adev.rmmio_remap.reg_offset).s(" (bus ")
        .hex((adev.rmmio_remap.bus_addr >> 32) as u32).s(":").hex(adev.rmmio_remap.bus_addr as u32).s(")").say();
    let c = &adev.gfx.config;
    Line::new().s("gc info: ").dec(c.max_shader_engines as u64).s(" SE, ").dec(c.max_sh_per_se as u64)
        .s(" SH/SE, ").dec(c.max_cu_per_sh as u64).s(" CU/SH, ").dec(c.max_backends_per_se as u64).s(" RB/SE, wave ")
        .dec(adev.gfx.cu_info.wave_front_size as u64).s(", LDS ").dec(adev.gfx.cu_info.lds_size as u64).say();
    let Some(ctx) = adev.mode_info.atom_context.as_ref() else {
        Line::new().s("no ATOM context").say();
        return;
    };
    Line::new().s("ATOM: ").bytes(cstr(&ctx.name)).s(", ").bytes(cstr(&ctx.date)).s(", version ")
        .bytes(cstr(&ctx.vbios_ver_str)).s(", build ").bytes(cstr(&ctx.build_num)).s(", fw revision ")
        .hex(ctx.version).say();
    Line::new().s("atomfirmware: capability ").hex(adev.mode_info.firmware_flags).s(", bios scratch at ")
        .hex(adev.bios_scratch_reg_offset).s(", fb scratch ").dec(ctx.scratch_size_bytes as u64).s(" bytes")
        .say();
}

/// The register bases from the discovery table against the fixed ones of
/// renoir_ip_offset.h, which DC (and only DC) uses.
fn compare_dc_bases(adev: &amdgpu_device) {
    use crate::regs::*;
    use crate::tables::*;
    let seg = MAX_SEGMENT as usize;
    let list: &[(u32, &str, &[u32])] = &[
        (ATHUB_HWIP, "ATHUB", &ATHUB_BASE), (DF_HWIP, "DF", &DF_BASE), (DCE_HWIP, "DCE/DMU", &DMU_BASE),
        (GC_HWIP, "GC", &GC_BASE), (HDP_HWIP, "HDP", &HDP_BASE), (MMHUB_HWIP, "MMHUB", &MMHUB_BASE),
        (MP0_HWIP, "MP0", &MP0_BASE), (MP1_HWIP, "MP1", &MP1_BASE), (NBIO_HWIP, "NBIO/NBIF0", &NBIF0_BASE),
        (OSSSYS_HWIP, "OSSSYS", &OSSSYS_BASE), (SDMA0_HWIP, "SDMA0", &SDMA0_BASE), (SMUIO_HWIP, "SMUIO", &SMUIO_BASE),
        (THM_HWIP, "THM", &THM_BASE), (UMC_HWIP, "UMC", &UMC_BASE), (UVD_HWIP, "UVD/VCN", &UVD0_BASE),
    ];
    let mut same = 0;
    for &(ip, name, fixed) in list {
        let found = &adev.reg_offset[ip as usize][0];
        let want = &fixed[..seg];
        let n = want.iter().rposition(|&b| b != 0).map_or(0, |i| i + 1);
        if found.len() >= n && found[..n] == want[..n] {
            same += 1;
            continue;
        }
        let mut l = Line::new();
        l.s("base ").s(name).s(" differs: discovery");
        for &b in found.iter().take(seg) {
            l.s(" ").hex(b);
        }
        l.s(", renoir_ip_offset.h");
        for &b in &want[..n] {
            l.s(" ").hex(b);
        }
        l.say();
    }
    Line::new().s("register bases: ").dec(same as u64).s(" of ").dec(list.len() as u64)
        .s(" IPs as in renoir_ip_offset.h").say();
}

/// `adev->ip_versions` by hardware IP, as the IP blocks will be chosen
/// from them.
fn report_ip_versions(adev: &amdgpu_device) {
    use crate::regs::*;
    const IPS: &[(u32, &str)] = &[
        (GC_HWIP, "GC"), (HDP_HWIP, "HDP"), (SDMA0_HWIP, "SDMA0"), (SDMA1_HWIP, "SDMA1"), (MMHUB_HWIP, "MMHUB"),
        (ATHUB_HWIP, "ATHUB"), (NBIO_HWIP, "NBIO"), (MP0_HWIP, "MP0"), (MP1_HWIP, "MP1"), (UVD_HWIP, "UVD/VCN/JPEG"),
        (VCN1_HWIP, "VCN1"), (VCE_HWIP, "VCE"), (DF_HWIP, "DF"), (DCE_HWIP, "DCE"), (OSSSYS_HWIP, "OSSSYS"),
        (SMUIO_HWIP, "SMUIO"), (PWR_HWIP, "PWR"), (NBIF_HWIP, "NBIF"), (THM_HWIP, "THM"), (CLK_HWIP, "CLK"),
        (UMC_HWIP, "UMC"), (RSMU_HWIP, "RSMU"), (XGMI_HWIP, "XGMI"), (DCI_HWIP, "DCI"), (PCIE_HWIP, "PCIE"),
        (ISP_HWIP, "ISP"),
    ];
    for &(ip, name) in IPS {
        let v = adev.ip_versions[ip as usize][0];
        if v == 0 {
            continue;
        }
        Line::new().s("ip_versions[").s(name).s("][0] = IP_VERSION(").dec((v >> 24) as u64).s(", ")
            .dec(((v >> 16) & 0xff) as u64).s(", ").dec(((v >> 8) & 0xff) as u64).s(") variant ")
            .dec(((v >> 4) & 0xf) as u64).s(" subrev ").dec((v & 0xf) as u64).say();
    }
    Line::new().s("instances: sdma ").dec(adev.sdma.num_instances as u64).s(" (mask ").hex(adev.sdma.sdma_mask)
        .s("), vcn ").dec(adev.vcn.num_vcn_inst as u64).s(" (mask ").hex(adev.vcn.inst_mask).s("), gc xcc mask ")
        .hex(adev.gfx.xcc_mask).s(", umc ").dec(adev.gmc.num_umc as u64).say();
}

/// One firmware image: its name, header version and size.
fn image(l: &mut Line, name: &str, fw: Option<&crate::linux::firmware>) {
    use crate::amdgpu_ucode::le16;
    l.s(name);
    match fw {
        Some(f) => {
            l.s(" v").dec(le16(f, regs::COMMON_FIRMWARE_HEADER_header_version_major) as u64).s(".")
                .dec(le16(f, regs::COMMON_FIRMWARE_HEADER_header_version_minor) as u64).s(" ")
                .dec(f.data.len() as u64).s("B");
        }
        None => {
            l.s(" -");
        }
    }
}

/// The microcode early_init requested, and what PSP will load.
fn report_microcode(adev: &amdgpu_device) {
    let g = &adev.gfx;
    Line::new().s("gfx: pfp ").hex(g.pfp_fw_version).s("/").dec(g.pfp_feature_version as u64).s(", me ")
        .hex(g.me_fw_version).s("/").dec(g.me_feature_version as u64).s(", ce ").hex(g.ce_fw_version).s("/")
        .dec(g.ce_feature_version as u64).s(", mec ").hex(g.mec_fw_version).s("/").dec(g.mec_feature_version as u64)
        .s(", rlc ").hex(g.rlc_fw_version).s("/").dec(g.rlc_feature_version as u64).say();
    Line::new().s("rlc: v2.1 ").s(if g.rlc.is_rlc_v2_1 { "yes" } else { "no" }).s(", srlc ").hex(g.rlc_srlc_fw_version)
        .s(", srlg ").hex(g.rlc_srlg_fw_version).s(", srls ").hex(g.rlc_srls_fw_version).s(", lists ")
        .dec(g.rlc.reg_list_format_size_bytes as u64).s("+").dec(g.rlc.reg_list_size_bytes as u64).s(" bytes")
        .s(", write_wait me ").s(if g.me_fw_write_wait { "1" } else { "0" }).s(" mec ")
        .s(if g.mec_fw_write_wait { "1" } else { "0" }).say();
    let s = &adev.sdma.instance[0];
    let p = &adev.psp;
    Line::new().s("sdma0 ").hex(s.fw_version).s("/").dec(s.feature_version as u64).s(" burst_nop ")
        .s(if s.burst_nop { "1" } else { "0" }).s(", asd ").hex(p.asd_context.bin_desc.fw_version).s(" ")
        .dec(p.asd_context.bin_desc.size_bytes as u64).s("B, ta ").hex(p.ta_fw_version).s(" (hdcp ")
        .hex(p.hdcp_context.context.bin_desc.fw_version).s(", dtm ").hex(p.dtm_context.context.bin_desc.fw_version)
        .s(", securedisplay ").hex(p.securedisplay_context.context.bin_desc.fw_version).s(")").say();
    let mut l = Line::new();
    l.s("images: ");
    image(&mut l, "dmcub", adev.dm_dmub_fw.as_ref());
    l.s(", ");
    image(&mut l, "vcn", adev.vcn.inst[0].fw.as_ref());
    l.say();
    let ids = adev.firmware.ucode.iter().filter(|u| u.fw.is_some()).count();
    Line::new().s("psp load list: ").dec(ids as u64).s(" ucode ids, fw_size ").dec(adev.firmware.fw_size as u64)
        .s(" bytes; pg_flags ").hex(adev.pg_flags).s(", pp_feature ").hex(adev.pm.pp_feature).s(", num_crtc ")
        .dec(adev.mode_info.num_crtc as u64).say();
}
