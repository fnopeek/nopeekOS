//! The run of the port, up to stage 2:
//! `amdgpu_device_init` with the IP discovery, the IP block list,
//! `soc15_common_early_init`, the VBIOS from VFCT, `amdgpu_atombios_init`,
//! the early_init of the nine blocks after COMMON (their microcode), the
//! doorbells, the PCIe caps, the reset on init, need_post and the clocks;
//! then `amdgpu_device_ip_init` through the COMMON block, whose hw_init
//! writes the NBIO registers. What it found goes to the device log.

use crate::amdgpu_device::{self as dev, amdgpu_device};
use crate::atom::cstr;
use crate::linux::Line;
use crate::regs;

/// The stage on a bound card whose BAR5 is mapped; `bdf` is its bus
/// address, `bar2` the doorbell BAR's bus address and length.
pub fn run(adev: &mut amdgpu_device, bdf: (u8, u8, u8), bar2: (u64, u64)) {
    if up_to_fw_loading(adev, bdf, bar2) {
        ip_init_phase2(adev);
    }
}

/// Every step through `amdgpu_device_fw_loading`; true when it got there.
pub fn up_to_fw_loading(adev: &mut amdgpu_device, bdf: (u8, u8, u8), bar2: (u64, u64)) -> bool {
    device_init(adev, bdf, bar2) && ip_init_common(adev) && ip_init_gmc(adev) && ip_init_sw(adev)
        && ip_init_fw_loading(adev)
}

/// `amdgpu_device_init` up to `amdgpu_device_ip_init`, reported; true when
/// it got there.
pub fn device_init(adev: &mut amdgpu_device, bdf: (u8, u8, u8), bar2: (u64, u64)) -> bool {
    // amdgpu_pciidlist: 0x164C is CHIP_RENOIR | AMD_IS_APU
    let r = dev::amdgpu_device_init(adev, regs::CHIP_RENOIR | regs::AMD_IS_APU, bdf, bar2);
    if !adev.mman.discovery_bin.is_empty() {
        report_ip_versions(adev);
        compare_dc_bases(adev);
    }
    match r {
        Ok(reset) => {
            report(adev);
            report_microcode(adev);
            report_device_init(adev, &reset);
            let valid = adev.ip_blocks.iter().filter(|b| b.status.valid).count();
            Line::new().s("amdgpu_device_init up to amdgpu_device_ip_init, ")
                .dec(adev.ip_blocks.len() as u64).s(" blocks, ").dec(valid as u64).s(" valid").say();
            true
        }
        Err(e) => {
            Line::new().s("amdgpu_device_init failed (").dec((-e) as u64).s(")").say();
            false
        }
    }
}

/// The registers `soc15_common_hw_init` writes, read back.
fn common_regs(adev: &amdgpu_device, l: &mut Line) {
    use crate::nbio_v7_0::rreg32_soc15;
    let rd = |idx, reg| rreg32_soc15(adev, regs::NBIO_HWIP, 0, idx, reg).unwrap_or(0);
    l.s("strap4 ").hex(rd(regs::regRCC_DEV0_EPF6_STRAP4_BASE_IDX, regs::regRCC_DEV0_EPF6_STRAP4))
        .s(", remap_hdp mem ").hex(rd(regs::mmREMAP_HDP_MEM_FLUSH_CNTL_BASE_IDX, regs::mmREMAP_HDP_MEM_FLUSH_CNTL))
        .s(" reg ").hex(rd(regs::mmREMAP_HDP_REG_FLUSH_CNTL_BASE_IDX, regs::mmREMAP_HDP_REG_FLUSH_CNTL))
        .s(", doorbell aper ").hex(rd(regs::mmRCC_DOORBELL_APER_EN_BASE_IDX, regs::mmRCC_DOORBELL_APER_EN))
        .s(", sdma0 doorbell range ").hex(rd(regs::mmBIF_SDMA0_DOORBELL_RANGE_BASE_IDX, regs::mmBIF_SDMA0_DOORBELL_RANGE));
}

/// Stage 2: `amdgpu_device_ip_init` through the COMMON block.
pub fn ip_init_common(adev: &mut amdgpu_device) -> bool {
    let mut l = Line::new();
    l.s("common before: ");
    common_regs(adev, &mut l);
    l.say();
    let r = dev::amdgpu_device_ip_init_common(adev);
    let mut l = Line::new();
    l.s("common after:  ");
    common_regs(adev, &mut l);
    l.say();
    Line::new().s("ras_hw_enabled ").hex(adev.ras_hw_enabled).s(", ras_enabled ").hex(adev.ras_enabled)
        .s(", df hash 64k/2m/1g ").dec(adev.df_hash_status.hash_64k as u64).s("/")
        .dec(adev.df_hash_status.hash_2m as u64).s("/").dec(adev.df_hash_status.hash_1g as u64).say();
    if r != 0 {
        Line::new().s("amdgpu_device_ip_init failed (").dec((-r) as u64).s(")").say();
        return false;
    }
    Line::new().s("stage 2 done: amdgpu_device_ip_init through COMMON (sw ")
        .dec(adev.ip_blocks[0].status.sw as u64).s(", hw ").dec(adev.ip_blocks[0].status.hw as u64).s(")").say();
    true
}

/// Stage 3a: the GMC block's sw_init.
pub fn ip_init_gmc(adev: &mut amdgpu_device) -> bool {
    let r = dev::amdgpu_device_ip_init_gmc(adev);
    if r != 0 {
        Line::new().s("amdgpu_device_ip_init failed (").dec((-r) as u64).s(")").say();
        return false;
    }
    let g = &adev.gmc;
    Line::new().s("gmc: vram ").hex64(g.vram_start).s("-").hex64(g.vram_end).s(" aper ").hex64(g.aper_base)
        .s(" gart ").hex64(g.gart_start).say();
    let mut l = Line::new();
    l.s("gmc: gart table ");
    match adev.gart.bo {
        Some(h) => {
            l.hex64(crate::amdgpu_object::amdgpu_bo_gpu_offset(adev, h)).s(" (").dec(adev.gart.table_size >> 10)
                .s(" KiB)");
        }
        None => {
            l.s("none");
        }
    }
    l.s(", ").dec(adev.mman.bos.iter().flatten().count() as u64).s(" kernel buffers").say();
    Line::new().s("stage 3a done: gmc_v9_0_sw_init").say();
    true
}

/// Stage 3b: the sw_init of the remaining blocks.
pub fn ip_init_sw(adev: &mut amdgpu_device) -> bool {
    let r = dev::amdgpu_device_ip_init_sw(adev);
    if r != 0 {
        Line::new().s("amdgpu_device_ip_init failed (").dec((-r) as u64).s(")").say();
        return false;
    }
    let mut l = Line::new();
    l.s("rings:");
    for ring in adev.rings.iter() {
        l.s(" ").s(&ring.name);
    }
    l.say();
    for ring in adev.rings.iter() {
        Line::new().s("ring ").dec(ring.idx as u64).s(" ").s(&ring.name).s(" size ").dec(ring.ring_size as u64)
            .s(" gpu ").hex64(ring.gpu_addr).s(" doorbell ").hex(ring.doorbell_index).s(" me/pipe/queue ")
            .dec(ring.me as u64).s("/").dec(ring.pipe as u64).s("/").dec(ring.queue as u64).dbg();
    }
    let b = &adev.smu.smu_table.boot_values;
    Line::new().s("smu boot: gfxclk ").dec(b.gfxclk as u64).s(" uclk ").dec(b.uclk as u64).s(" socclk ")
        .dec(b.socclk as u64).s(" dcefclk ").dec(b.dcefclk as u64).s(" vclk ").dec(b.vclk as u64).s(" dclk ")
        .dec(b.dclk as u64).s(" fclk ").dec(b.fclk as u64).s(" (10 kHz)").say();
    if let Some(f) = adev.dm.dmub_fb_info.as_ref() {
        Line::new().s("dmub: fb ").hex64(adev.dm.dmub_bo_gpu_addr).s(" windows ").dec(f.num_fb as u64)
            .s(", fw 0x").hexw(adev.dm.dmcub_fw_version, 8).say();
    }
    Line::new().s("fw_size ").dec(adev.firmware.fw_size as u64).s(", ")
        .dec(adev.mman.bos.iter().flatten().count() as u64).s(" kernel buffers").say();
    Line::new().s("stage 3b done: sw_init of every block").say();
    true
}

/// Stage 4: the IB pools, the firmware buffer, IH up (and with it bus
/// mastering, so the IOMMU), and the PSP loading every block's firmware.
fn ip_init_fw_loading(adev: &mut amdgpu_device) -> bool {
    let r = dev::amdgpu_device_ib_pool_init_logged(adev);
    if r != 0 {
        return false;
    }
    let r = crate::amdgpu_ucode::amdgpu_ucode_create_bo(adev);
    if r != 0 {
        Line::new().s("amdgpu_device_ip_init failed (").dec((-r) as u64).s(")").say();
        return false;
    }
    let r = dev::amdgpu_device_ip_hw_init_phase1(adev);
    if r != 0 {
        Line::new().s("amdgpu_device_ip_init failed (").dec((-r) as u64).s(")").say();
        return false;
    }
    let ih = &adev.irq.ih;
    Line::new().s("ih: ring ").hex64(ih.gpu_addr).s(" size ").dec(ih.ring_size as u64).s(" doorbell ")
        .hex(ih.doorbell_index).s(", enabled ").dec(ih.enabled as u64).s(", msi ").dec(adev.irq.msi_enabled as u64)
        .say();
    let r = dev::amdgpu_device_fw_loading(adev);
    let p = &adev.psp;
    Line::new().s("psp: ring ").hex64(p.km_ring.ring_mem_mc_addr).s(", ").dec(p.fence_value as u64)
        .s(" commands, tmr ").hex64(p.tmr_mc_addr).say();
    let mut l = Line::new();
    let loaded = adev.firmware.ucode.iter().filter(|u| u.fw.is_some() && u.ucode_size != 0);
    for (n, u) in loaded.enumerate() {
        if n % 6 == 0 {
            if n != 0 {
                l.say();
            }
            l = Line::new();
            l.s("psp loaded:");
        }
        l.s(" ").dec(u.ucode_id as u64).s("@").hex(u.tmr_mc_addr_lo);
    }
    l.say();
    Line::new().s("ta: asd ").dec(p.asd_context.initialized as u64).s(" hdcp ")
        .dec(p.hdcp_context.context.initialized as u64).s(" dtm ").dec(p.dtm_context.context.initialized as u64)
        .s(" securedisplay ").dec(p.securedisplay_context.context.initialized as u64).say();
    if r != 0 {
        Line::new().s("amdgpu_device_ip_init failed (").dec((-r) as u64).s(")").say();
        return false;
    }
    Line::new().s("fw_loading done").say();
    true
}

/// The rings' state after a test: `sched.ready` of each, by name.
fn report_rings(adev: &amdgpu_device, what: &str) {
    let mut l = Line::new();
    l.s(what);
    for r in adev.rings.iter() {
        if matches!(r.funcs.ops, crate::amdgpu_ring::ring_ops::gfx_v9_0_gfx | crate::amdgpu_ring::ring_ops::gfx_v9_0_compute
            | crate::amdgpu_ring::ring_ops::gfx_v9_0_kiq | crate::amdgpu_ring::ring_ops::sdma_v4_0
            | crate::amdgpu_ring::ring_ops::vcn_v2_0_dec | crate::amdgpu_ring::ring_ops::vcn_v2_0_enc
            | crate::amdgpu_ring::ring_ops::jpeg_v2_0_dec)
        {
            l.s(" ").s(&r.name).s(if r.sched.ready { "+" } else { "-" });
        }
    }
    l.say();
}

/// Stage 4: `amdgpu_device_ip_hw_init_phase2` (SMU, GFX with its ring
/// tests, SDMA with its ring test), the fence interrupts, late_init with
/// clock and power gating, the IB tests of the delayed init work, the GPU
/// service while it runs, and then `amdgpu_device_fini_hw` before the card
/// goes back. Between phase2
/// and late_init Linux also sets up RAS recovery (no RAS here), the
/// schedulers and TTM's buffer functions, KFD and the FRU (an APU has
/// none).
fn ip_init_phase2(adev: &mut amdgpu_device) {
    let r = dev::amdgpu_device_ip_hw_init_phase2(adev);
    let smu = &adev.smu;
    Line::new().s("smu: if 0x").hexw(smu.smc_fw_if_version, 8).s(" fw 0x").hexw(smu.smc_fw_version, 8)
        .s(", dpm ").dec(adev.pm.dpm_enabled as u64).s(", features 0x").hexw((smu.feature_supported >> 32) as u32, 8)
        .hexw(smu.feature_supported as u32, 8).say();
    let g = &adev.gfx;
    Line::new().s("gfx: rbs ").dec(g.config.num_rbs as u64).s(" (mask 0x").hexu(g.config.backend_enable_mask)
        .s("), cu ").dec(g.cu_info.number as u64).s(" (ao 0x").hexu(g.cu_info.ao_cu_mask).s("), db_debug2 0x")
        .hexw(g.config.db_debug2, 8).say();
    report_rings(adev, "ring tests:");
    if r != 0 {
        Line::new().s("amdgpu_device_ip_init failed (").dec((-r) as u64).s(")").say();
        dev::amdgpu_device_fini_hw(adev);
        return;
    }
    let r = dev::amdgpu_device_init_schedulers(adev);
    if r != 0 {
        dev::amdgpu_device_fini_hw(adev);
        return;
    }
    let buffer_ring = adev.sdma.instance[0].ring;
    if buffer_ring.is_some_and(|b| adev.rings[b].sched.ready) {
        crate::amdgpu_ttm::amdgpu_ttm_set_buffer_funcs_status(adev, true);
    }
    crate::amdgpu_fence::amdgpu_fence_driver_hw_init(adev);
    adev.accel_working = true;
    // amdgpu_vm_check_compute_bug: GFX 9 has none, the rings' flags stay
    // Initialize the buffer migration limit.
    let max_mbps: u32 = if crate::tables::amdgpu_moverate[0] >= 0 {
        crate::tables::amdgpu_moverate[0] as u32
    } else {
        // Allow 8 MB/s.
        8
    };
    // Get a log2 for easy divisions.
    adev.mm_stats.log2_max_MBps = crate::linux::ilog2(max_mbps.max(1) as u64);
    let r = dev::amdgpu_device_ip_late_init(adev);
    if r != 0 {
        Line::new().s("amdgpu_device_ip_late_init failed").say();
        dev::amdgpu_device_fini_hw(adev);
        return;
    }
    Line::new().s("late_init: cg 0x").hexw(adev.cg_flags as u32, 8).s(" pg 0x").hexw(adev.pg_flags, 8)
        .s(", gfxoff ").dec(adev.gfx.gfx_off_state as u64).s(", workload 0x").hexu(adev.smu.workload_mask)
        .s(", gfxclk ").dec(adev.smu.gfx_default_hard_min_freq as u64).s("-")
        .dec(adev.smu.gfx_default_soft_max_freq as u64).s(" MHz").say();
    // The IB tests run AMDGPU_RESUME_MS after init, when the idle work
    // has gated VCN and JPEG again: their begin_use starts them anew.
    crate::workqueue::mod_delayed_work(adev, crate::workqueue::work::delayed_init, dev::AMDGPU_RESUME_US);
    crate::workqueue::wait_until(adev, i64::MAX, |a| a.delayed_init_result.is_some());
    let r = adev.delayed_init_result.unwrap_or(-22);
    report_rings(adev, "ib tests:");
    if r == 0 {
        Line::new().s("stage 4 done: ring and IB tests").say();
        crate::gpudev::serve(adev);
    }
    dev::amdgpu_device_fini_hw(adev);
    let faults = crate::amdgpu_irq::amdgpu_irq_counts(adev);
    Line::new().s("fini: done, ").dec(faults as u64).s(" interrupts processed").say();
}

/// The PCIe link registers `amdgpu_device_get_pcie_info` read, raw.
fn report_pcie(l: &mut Line, name: &str, d: &crate::linux::pci_dev) {
    let cap = d.pcie_cap();
    l.s(name).s(" ").hex((d.bus as u32) << 8 | d.devfn as u32).s(" vendor ").hex(d.vendor() as u32);
    if cap == 0 {
        l.s(" no pcie cap");
        return;
    }
    l.s(" flags ").hex(d.read_word(cap + regs::PCI_EXP_FLAGS) as u32).s(" lnkcap ")
        .hex(d.read_dword(cap + regs::PCI_EXP_LNKCAP)).s(" lnkcap2 ").hex(d.read_dword(cap + regs::PCI_EXP_LNKCAP2));
}

/// What the rest of `amdgpu_device_init` decided.
fn report_device_init(adev: &amdgpu_device, reset: &dev::reset_on_init) {
    let mut l = Line::new();
    report_pcie(&mut l, "pcie: gpu", &adev.pdev);
    l.say();
    let mut l = Line::new();
    match adev.pdev.upstream_bridge() {
        Some(b) => report_pcie(&mut l, "pcie: bridge", &b),
        None => {
            l.s("pcie: gpu on the root bus");
        }
    }
    l.say();
    Line::new().s("pcie_gen_mask ").hex(adev.pm.pcie_gen_mask).s(", pcie_mlw_mask ").hex(adev.pm.pcie_mlw_mask)
        .s(", tmz ").s(if adev.gmc.tmz_enabled { "on" } else { "off" }).s(", noretry ").dec(adev.gmc.noretry as u64)
        .s(", atomics ").s(if adev.have_atomics_support { "yes" } else { "no" }).s(", timeouts gfx ")
        .dec(adev.gfx_timeout).s(" ms").say();
    let d = &adev.doorbell;
    Line::new().s("doorbell: BAR2 ").hex((d.base >> 32) as u32).s(":").hex(d.base as u32).s(", ")
        .dec(d.size >> 10).s(" KiB, ").dec(d.num_kernel_doorbells as u64).s(" kernel doorbells, max_assignment ")
        .dec(adev.doorbell_index.max_assignment as u64).say();
    let method = match reset.method {
        m if m == regs::AMD_RESET_METHOD_MODE2 => "MODE2",
        m if m == regs::AMD_RESET_METHOD_MODE1 => "MODE1",
        _ => "?",
    };
    let mut l = Line::new();
    l.s("reset on init: ");
    if !reset.needed {
        l.s("not needed");
    } else {
        l.s("needed, ").s(method).s(" (SMU GfxDeviceDriverReset) ");
        match reset.taken {
            Some(0) => l.s("- taken, SMU answered OK"),
            Some(r) => l.s("- taken, failed ").dec((-r) as u64),
            None => l.s("- not taken"),
        };
    }
    l.say();
    let c = &adev.clock;
    Line::new().s("need_post: no (bios scratch 7 has ASIC_INIT_COMPLETE); clocks: sclk ").dec((c.default_sclk / 100) as u64)
        .s(" MHz, mclk ").dec((c.default_mclk / 100) as u64).s(" MHz, spll ref ").dec((c.spll.reference_freq / 100) as u64)
        .s(" MHz").say();
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
