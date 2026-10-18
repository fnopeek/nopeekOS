//! amdgpu/amdgpu_ras.c (Linux 6.18.55): `amdgpu_ras_init` as far as Renoir
//! takes it. CHIP_RENOIR is not an ASIC with RAS, so the context is
//! released again and `ras_enabled` stays 0.

use crate::amdgpu_device::amdgpu_device;
use crate::regs::*;

/// `amdgpu_ras_asic_supported`: bare metal, not CHIP_IP_DISCOVERY.
fn amdgpu_ras_asic_supported(adev: &amdgpu_device) -> bool {
    adev.asic_type == CHIP_VEGA10
        || adev.asic_type == CHIP_VEGA20
        || adev.asic_type == CHIP_ARCTURUS
        || adev.asic_type == CHIP_ALDEBARAN
        || adev.asic_type == CHIP_SIENNA_CICHLID
}

/// `amdgpu_ras_check_supported`, up to its first return on Renoir.
fn amdgpu_ras_check_supported(adev: &mut amdgpu_device) {
    adev.ras_hw_enabled = 0;
    adev.ras_enabled = 0;
    if !amdgpu_ras_asic_supported(adev) {
        return;
    }
    // The PSP, VBIOS and poison queries are for the ASICs above.
}

/// `amdgpu_ras_init`.
pub fn amdgpu_ras_init(adev: &mut amdgpu_device) -> i32 {
    amdgpu_ras_check_supported(adev);
    if adev.ras_enabled == 0 || adev.asic_type == CHIP_VEGA10 {
        // !ras_enabled on VEGA20 keeps a GFX feature bit; release_con.
        return 0;
    }
    // A RAS ASIC is not ported.
    -22
}
