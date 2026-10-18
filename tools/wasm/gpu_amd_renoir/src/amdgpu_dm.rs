//! display/amdgpu_dm/amdgpu_dm.c (Linux 6.18.55): the DM block's
//! early_init, which sizes the display for DCN 2.1 and requests DMCUB.

use crate::amdgpu_device::amdgpu_device;
use crate::amdgpu_ucode::amdgpu_ucode_request;
use crate::atom::amdgpu_atom_parse_data_header;
use crate::linux::{drm_info, Line};
use crate::regs::*;

/// `dm_init_microcode`: DCN 2.1.0, Renoir's DMCUB unless the revision is
/// Green Sardine's.
pub fn dm_init_microcode(adev: &mut amdgpu_device) -> i32 {
    let fw_name_dmub = match adev.ip_version(DCE_HWIP, 0) {
        v if v == 2 << 24 | 1 << 16 => {
            // ASICREV_IS_GREEN_SARDINE
            if adev.external_rev_id >= GREEN_SARDINE_A0 && adev.external_rev_id < 0xFF {
                FIRMWARE_GREEN_SARDINE_DMUB
            } else {
                FIRMWARE_RENOIR_DMUB
            }
        }
        // ASIC doesn't support DMUB.
        _ => return 0,
    };
    let (fw, r) = amdgpu_ucode_request(core::str::from_utf8(fw_name_dmub).unwrap_or(""));
    adev.dm_dmub_fw = fw;
    r
}

/// `dm_early_init`. CHIP_RENOIR takes the default branch of the
/// `asic_type` switch, then DCN 2.1.0.
pub fn dm_early_init(adev: &mut amdgpu_device) -> i32 {
    // if there is no object header, skip DM
    let has_object_header = adev.mode_info.atom_context.as_ref()
        .is_some_and(|ctx| amdgpu_atom_parse_data_header(ctx, DATA_TABLE_Object_Header).is_some());
    if !has_object_header {
        adev.harvest_ip_mask |= AMD_HARVEST_IP_DMU_MASK;
        drm_info("No object header, skipping DM");
        return -2;
    }

    match adev.ip_version(DCE_HWIP, 0) {
        v if v == 2 << 24 | 1 << 16 => {
            adev.mode_info.num_crtc = 4;
            adev.mode_info.num_hpd = 4;
            adev.mode_info.num_dig = 4;
        }
        v => {
            Line::new().s("Unsupported DCE IP versions: ").hex(v).say();
            return -22;
        }
    }

    if adev.mode_info.funcs.is_none() {
        adev.mode_info.funcs = Some("dm_display_funcs");
    }
    adev.dc_enabled = true;

    dm_init_microcode(adev)
}

