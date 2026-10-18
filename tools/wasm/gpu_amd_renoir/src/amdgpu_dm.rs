//! display/amdgpu_dm/amdgpu_dm.c (Linux 6.18.55): the DM block's
//! early_init, which sizes the display for DCN 2.1 and requests DMCUB, and
//! its sw_init, which lays out the DMUB regions in one buffer.

use crate::amdgpu_device::amdgpu_device;
use crate::amdgpu_ucode::amdgpu_ucode_request;
use crate::atom::amdgpu_atom_parse_data_header;
use crate::linux::{drm_info, Line};
use crate::regs::*;
use crate::dmub_srv::*;

/// `struct amdgpu_display_manager`, the part sw_init fills.
#[derive(Default)]
pub struct amdgpu_display_manager {
    /// `cgs_device`: amdgpu_cgs_create_device allocates it; nothing in
    /// this port reads through it.
    pub cgs_device: bool,
    pub dmcub_fw_version: u32,
    pub dmub_srv: Option<dmub_srv>,
    pub dmub_bo: Option<usize>,
    pub dmub_bo_gpu_addr: u64,
    pub dmub_bo_cpu_addr: Option<crate::amdgpu_object::amdgpu_kptr>,
    pub dmub_fb_info: Option<dmub_srv_fb_info>,
}

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


fn le32(b: &[u8], at: u32) -> u32 {
    let at = at as usize;
    b.get(at..at + 4).map_or(0, |x| u32::from_le_bytes([x[0], x[1], x[2], x[3]]))
}

// DMUB_WINDOW_0_INST_CONST to DMUB_WINDOW_SHARED_STATE in the frame
// buffer; DMUB_WINDOW_LSDMA_BUFFER is not initialized and so 0, the frame
// buffer as well.
pub static WINDOW_MEMORY_TYPE: [u32; DMUB_WINDOW_TOTAL as usize] = [DMUB_WINDOW_MEMORY_TYPE_FB; DMUB_WINDOW_TOTAL as usize];

/// The middle of `dm_dmub_sw_init`: create the DMUB service and calculate
/// the size of all the regions for it.
pub fn dm_dmub_regions(data: &[u8], vbios_size: u32, dmub_asic: u32, dmub_srv: &mut dmub_srv,
                       region_info: &mut dmub_srv_region_info) -> i32 {
    let inst_const_bytes = le32(data, DMCUB_HDR_INST_CONST_BYTES);
    let bss_data_bytes = le32(data, DMCUB_HDR_BSS_DATA_BYTES);
    let ucode_array_offset = le32(data, UCODE_HDR_UCODE_ARRAY_OFFSET_BYTES);
    // Create the DMUB service.
    let status = dmub_srv_create(dmub_srv, dmub_asic, 0);
    if status != DMUB_STATUS_OK {
        Line::new().s("Error creating DMUB service: ").dec(status as u64).say();
        return -22;
    }
    // Calculate the size of all the regions for the DMUB service.
    let blob = |off: u32, len: u32| data.get(off as usize..(off as usize).saturating_add(len as usize));
    let inst_const_size = inst_const_bytes.wrapping_sub(PSP_HEADER_BYTES).wrapping_sub(PSP_FOOTER_BYTES);
    let bss_off = ucode_array_offset.wrapping_add(inst_const_bytes);
    let region_params = dmub_srv_region_params {
        inst_const_size,
        bss_data_size: bss_data_bytes,
        vbios_size,
        fw_bss_data: if bss_data_bytes != 0 { blob(bss_off, bss_data_bytes) } else { None },
        fw_inst_const: blob(ucode_array_offset.wrapping_add(PSP_HEADER_BYTES), inst_const_size),
        window_memory_type: &WINDOW_MEMORY_TYPE,
    };
    let status = dmub_srv_calc_region_info(dmub_srv, &region_params, region_info);
    if status != DMUB_STATUS_OK {
        Line::new().s("Error calculating DMUB region info: ").dec(status as u64).say();
        return -22;
    }
    0
}

/// `dm_dmub_sw_init`.
fn dm_dmub_sw_init(adev: &mut amdgpu_device) -> i32 {
    let dmub_asic = match adev.ip_version(DCE_HWIP, 0) {
        v if v == 2 << 24 | 1 << 16 => DMUB_ASIC_DCN21,
        // ASIC doesn't support DMUB.
        _ => return 0,
    };
    let Some(fw) = adev.dm_dmub_fw.as_ref() else { return -22 };
    let data = &fw.data;
    adev.dm.dmcub_fw_version = le32(data, UCODE_HDR_UCODE_VERSION);
    let inst_const_bytes = le32(data, DMCUB_HDR_INST_CONST_BYTES);

    if adev.firmware.load_type == AMDGPU_FW_LOAD_PSP {
        let info = &mut adev.firmware.ucode[AMDGPU_UCODE_ID_DMCUB as usize];
        info.ucode_id = AMDGPU_UCODE_ID_DMCUB;
        info.fw = Some(crate::amdgpu_ucode::fw_source::dm_dmub);
        adev.firmware.fw_size = adev.firmware.fw_size
            .wrapping_add(crate::linux::align(inst_const_bytes as u64, crate::amdgpu_object::PAGE_SIZE) as u32);
        Line::new().s("Loading DMUB firmware via PSP: version=0x").hexw(adev.dm.dmcub_fw_version, 8).say();
    }

    let mut dmub_srv = dmub_srv::default();
    let mut region_info = dmub_srv_region_info::default();
    if dm_dmub_regions(data, adev.bios.len() as u32, dmub_asic, &mut dmub_srv, &mut region_info) != 0 {
        return -22;
    }
    adev.dm.dmub_srv = Some(dmub_srv);

    // Allocate a framebuffer based on the total size of all the regions.
    let h = match crate::amdgpu_object::amdgpu_bo_create_kernel(adev, region_info.fb_size as u64,
        crate::amdgpu_object::PAGE_SIZE, AMDGPU_GEM_DOMAIN_VRAM | AMDGPU_GEM_DOMAIN_GTT, true) {
        Ok(Some(h)) => h,
        Ok(None) => return -12,
        Err(r) => return r,
    };
    adev.dm.dmub_bo = Some(h);
    adev.dm.dmub_bo_gpu_addr = crate::amdgpu_object::amdgpu_bo_gpu_offset(adev, h);
    adev.dm.dmub_bo_cpu_addr = crate::amdgpu_object::bo(adev, h).kptr;

    // Rebase the regions on the framebuffer address.
    let mut fb_info = dmub_srv_fb_info::default();
    let status = dmub_srv_calc_mem_info(adev.dm.dmub_srv.as_ref().unwrap(), &region_info, &WINDOW_MEMORY_TYPE,
        (0, adev.dm.dmub_bo_gpu_addr), (0, 0), &mut fb_info);
    if status != DMUB_STATUS_OK {
        Line::new().s("Error calculating DMUB FB info: ").dec(status as u64).say();
        return -22;
    }
    adev.dm.dmub_fb_info = Some(fb_info);
    // dm_dmub_get_vbios_bounding_box: DCN 4.0.1 only.
    0
}

/// `dm_sw_init`. `load_dmcu_fw` returns 0 for DCN 2.1.0: it has no DMCU.
pub fn dm_sw_init(adev: &mut amdgpu_device) -> i32 {
    adev.dm.cgs_device = true;
    dm_dmub_sw_init(adev)
}
