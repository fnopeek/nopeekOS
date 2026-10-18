//! amdgpu/amdgpu_atombios.c (Linux 6.18.55): `amdgpu_atombios_init` on an
//! `is_atom_fw` image. The register, MC and PLL callbacks Linux puts into
//! `atom_card_info` serve the command table interpreter, which comes with
//! the first table the driver executes.

use crate::amdgpu_atomfirmware::*;
use crate::amdgpu_device::amdgpu_device;
use crate::atom::amdgpu_atom_parse;

/// `amdgpu_atombios_fini`.
pub fn amdgpu_atombios_fini(adev: &mut amdgpu_device) {
    adev.mode_info.atom_context = None;
}

/// `amdgpu_atombios_init`: 0, or -ENOMEM when the image does not parse.
pub fn amdgpu_atombios_init(adev: &mut amdgpu_device) -> i32 {
    adev.mode_info.atom_context = amdgpu_atom_parse(adev.bios.clone());
    if adev.mode_info.atom_context.is_none() {
        amdgpu_atombios_fini(adev);
        return -12;
    }

    if adev.is_atom_fw {
        amdgpu_atomfirmware_scratch_regs_init(adev);
        amdgpu_atomfirmware_allocate_fb_scratch(adev);
        // cached firmware_flags for further usage
        adev.mode_info.firmware_flags =
            adev.mode_info.atom_context.as_ref().map_or(0, amdgpu_atomfirmware_query_firmware_capability);
    }
    0
}
