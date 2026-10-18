//! amdgpu/gmc_v9_0.c (Linux 6.18.55): the GMC block's early_init.

use crate::amdgpu_device::amdgpu_device;
use crate::regs::*;

/// `struct amdgpu_irq_src`, as early_init sets it: the type count and
/// the name of the `funcs` table.
#[derive(Default, Clone, Copy)]
pub struct amdgpu_irq_src {
    pub num_types: u32,
    pub funcs: Option<&'static str>,
}

/// `gmc_v9_0_set_gmc_funcs`.
fn gmc_v9_0_set_gmc_funcs(adev: &mut amdgpu_device) {
    adev.gmc.gmc_funcs = Some("gmc_v9_0_gmc_funcs");
}

/// `gmc_v9_0_set_irq_funcs`. Bare metal.
fn gmc_v9_0_set_irq_funcs(adev: &mut amdgpu_device) {
    adev.gmc.vm_fault = amdgpu_irq_src { num_types: 1, funcs: Some("gmc_v9_0_irq_funcs") };
    if !adev.gmc.xgmi.connected_to_cpu && !adev.gmc.is_app_apu {
        adev.gmc.ecc_irq = amdgpu_irq_src { num_types: 1, funcs: Some("gmc_v9_0_ecc_funcs") };
    }
}

/// `gmc_v9_0_set_mmhub_funcs`: MMHUB 1.5.0, the default case.
fn gmc_v9_0_set_mmhub_funcs(adev: &mut amdgpu_device) {
    adev.mmhub_funcs = Some("mmhub_v1_0_funcs");
}

/// `gmc_v9_0_set_gfxhub_funcs`: not multi-AID.
fn gmc_v9_0_set_gfxhub_funcs(adev: &mut amdgpu_device) {
    adev.gfxhub_funcs = Some(if adev.aid_mask != 0 { "gfxhub_v1_2_funcs" } else { "gfxhub_v1_0_funcs" });
}

/// `gmc_v9_0_set_hdp_ras_funcs`.
fn gmc_v9_0_set_hdp_ras_funcs(adev: &mut amdgpu_device) {
    adev.hdp_ras = Some("hdp_v4_0_ras");
}

/// `gmc_v9_0_set_xgmi_ras_funcs`.
fn gmc_v9_0_set_xgmi_ras_funcs(adev: &mut amdgpu_device) {
    if !adev.gmc.xgmi.connected_to_cpu {
        adev.gmc.xgmi.ras = Some("xgmi_ras");
    }
}

/// `gmc_v9_0_early_init`. GC 9.3.0 with no XGMI and one AID: no XGMI and
/// no app APU; UMC 7.6.0 and MMHUB 1.5.0 take no umc, mmhub-ras or mca
/// funcs.
pub fn gmc_v9_0_early_init(adev: &mut amdgpu_device) -> i32 {
    // 9.4.0, 9.4.1 and 9.4.3 don't have XGMI defined
    // in their IP discovery tables
    let gc = adev.ip_version(GC_HWIP, 0);
    if gc == 9 << 24 | 4 << 16 || gc == 9 << 24 | 4 << 16 | 1 << 8 || adev.aid_mask != 0 {
        adev.gmc.xgmi.supported = true;
    }
    // XGMI 6.1.0 (is_host_gpu_xgmi_supported) and GC 9.4.3 (get_pkg_type)
    // are not Renoir's.

    gmc_v9_0_set_gmc_funcs(adev);
    gmc_v9_0_set_irq_funcs(adev);
    gmc_v9_0_set_mmhub_funcs(adev);
    gmc_v9_0_set_gfxhub_funcs(adev);
    gmc_v9_0_set_hdp_ras_funcs(adev);
    gmc_v9_0_set_xgmi_ras_funcs(adev);

    adev.gmc.shared_aperture_start = 0x2000000000000000;
    adev.gmc.shared_aperture_end = adev.gmc.shared_aperture_start + (4u64 << 30) - 1;
    adev.gmc.private_aperture_start = 0x1000000000000000;
    adev.gmc.private_aperture_end = adev.gmc.private_aperture_start + (4u64 << 30) - 1;
    adev.gmc.noretry_flags = AMDGPU_VM_NORETRY_FLAGS_TF;
    0
}
