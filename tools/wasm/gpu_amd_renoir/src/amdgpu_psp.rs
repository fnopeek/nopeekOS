//! amdgpu/amdgpu_psp.c and psp_v12_0.c (Linux 6.18.55): the PSP block's
//! early_init, which picks psp_v12_0 for MP0 11.0.3 and requests the ASD
//! and TA microcode.

use crate::amdgpu_device::amdgpu_device;
use crate::amdgpu_ucode::{amdgpu_is_kicker_fw, amdgpu_ucode_ip_version_decode, amdgpu_ucode_request, le16, le32};
use crate::linux::{firmware, Line};
use crate::regs::*;

/// Which image a `psp_bin_desc.start_addr` points into.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum psp_image {
    asd_fw,
    ta_fw,
}

/// `struct psp_bin_desc`; `start_addr` is (image, offset).
#[derive(Clone, Copy, Default)]
pub struct psp_bin_desc {
    pub fw_version: u32,
    pub feature_version: u32,
    pub size_bytes: u32,
    pub start_addr: Option<(psp_image, u32)>,
}

/// `struct ta_context`, the part early_init fills.
#[derive(Clone, Copy, Default)]
pub struct ta_context {
    pub bin_desc: psp_bin_desc,
}

/// The `struct psp_*_context`s that hold a `ta_context` as `context`.
#[derive(Clone, Copy, Default)]
pub struct psp_ta_context {
    pub context: ta_context,
}

/// `struct psp_context`, the part early_init fills.
#[derive(Default)]
pub struct psp_context {
    /// `psp->funcs`, by the name of the table.
    pub funcs: Option<&'static str>,
    pub autoload_supported: bool,
    pub boot_time_tmr: bool,
    pub sup_pd_fw_up: bool,
    pub sup_ifwi_up: bool,
    pub pmfw_centralized_cstate_management: bool,
    pub asd_fw: Option<firmware>,
    pub ta_fw: Option<firmware>,
    pub asd_context: ta_context,
    pub xgmi_context: psp_ta_context,
    pub ras_context: psp_ta_context,
    pub hdcp_context: psp_ta_context,
    pub dtm_context: psp_ta_context,
    pub rap_context: psp_ta_context,
    pub securedisplay_context: psp_ta_context,
    pub ta_fw_version: u32,
    /// `psp->cmd`: the command built in memory, then copied to `cmd_buf_mem`.
    pub cmd: alloc::vec::Vec<u8>,
    pub xgmi_supports_extended_data: bool,
    pub enable_mem_training: bool,
    pub buf: psp_buffers,
}

/// `psp_v12_0_set_psp_funcs`.
fn psp_v12_0_set_psp_funcs(psp: &mut psp_context) {
    psp.funcs = Some("psp_v12_0_funcs");
}

/// `psp_check_pmfw_centralized_cstate_management`: bare metal, MP0 11.0.3
/// is not in the list.
fn psp_check_pmfw_centralized_cstate_management(psp: &mut psp_context) {
    psp.pmfw_centralized_cstate_management = false;
}

/// `psp_init_asd_microcode`.
pub fn psp_init_asd_microcode(adev: &mut amdgpu_device, chip_name: &str) -> i32 {
    let (fw, err) = amdgpu_ucode_request(&alloc::format!("amdgpu/{}_asd.bin", chip_name));
    if err != 0 {
        adev.psp.asd_fw = None;
        return err;
    }
    let Some(fw) = fw else { return -19 };
    let d = &mut adev.psp.asd_context.bin_desc;
    d.fw_version = le32(&fw, COMMON_FIRMWARE_HEADER_ucode_version);
    d.feature_version = le32(&fw, PSP_FIRMWARE_HEADER_V1_0_sos + PSP_FW_LEGACY_BIN_DESC_fw_version);
    d.size_bytes = le32(&fw, COMMON_FIRMWARE_HEADER_ucode_size_bytes);
    d.start_addr = Some((psp_image::asd_fw, le32(&fw, COMMON_FIRMWARE_HEADER_ucode_array_offset_bytes)));
    adev.psp.asd_fw = Some(fw);
    0
}

/// `is_ta_fw_applicable`: the XGMI AUX choice is MP0 13.0.6 only.
fn is_ta_fw_applicable(_adev: &amdgpu_device, _fw_type: u32) -> bool {
    true
}

/// `parse_ta_bin_descriptor`: the descriptor at `desc` of the v2 TA image.
fn parse_ta_bin_descriptor(adev: &mut amdgpu_device, desc: u32) -> i32 {
    let Some(ta) = adev.psp.ta_fw.as_ref() else { return -22 };
    let fw_type = le32(ta, desc + PSP_FW_BIN_DESC_fw_type);
    if !is_ta_fw_applicable(adev, fw_type) {
        return 0;
    }
    let fw_version = le32(ta, desc + PSP_FW_BIN_DESC_fw_version);
    let size_bytes = le32(ta, desc + PSP_FW_BIN_DESC_size_bytes);
    let start = le32(ta, desc + PSP_FW_BIN_DESC_offset_bytes)
        .wrapping_add(le32(ta, COMMON_FIRMWARE_HEADER_ucode_array_offset_bytes));
    let start_addr = Some((psp_image::ta_fw, start));
    let psp = &mut adev.psp;
    let set = |c: &mut ta_context| {
        c.bin_desc.fw_version = fw_version;
        c.bin_desc.size_bytes = size_bytes;
        c.bin_desc.start_addr = start_addr;
    };
    match fw_type {
        TA_FW_TYPE_PSP_ASD => {
            psp.asd_context.bin_desc.fw_version = fw_version;
            psp.asd_context.bin_desc.feature_version = fw_version;
            psp.asd_context.bin_desc.size_bytes = size_bytes;
            psp.asd_context.bin_desc.start_addr = start_addr;
        }
        TA_FW_TYPE_PSP_XGMI | TA_FW_TYPE_PSP_XGMI_AUX => set(&mut psp.xgmi_context.context),
        TA_FW_TYPE_PSP_RAS => set(&mut psp.ras_context.context),
        TA_FW_TYPE_PSP_HDCP => set(&mut psp.hdcp_context.context),
        TA_FW_TYPE_PSP_DTM => set(&mut psp.dtm_context.context),
        TA_FW_TYPE_PSP_RAP => set(&mut psp.rap_context.context),
        TA_FW_TYPE_PSP_SECUREDISPLAY => set(&mut psp.securedisplay_context.context),
        t => {
            Line::new().s("Unsupported TA type: ").dec(t as u64).say();
        }
    }
    0
}

/// `parse_ta_v1_microcode`.
fn parse_ta_v1_microcode(adev: &mut amdgpu_device) -> i32 {
    let Some(ta) = adev.psp.ta_fw.as_ref() else { return -22 };
    if le16(ta, COMMON_FIRMWARE_HEADER_header_version_major) != 1 {
        return -22;
    }
    let desc = |d: u32, f: u32| le32(ta, d + f);
    let array = le32(ta, COMMON_FIRMWARE_HEADER_ucode_array_offset_bytes);
    let x = TA_FIRMWARE_HEADER_V1_0_xgmi;
    let r = TA_FIRMWARE_HEADER_V1_0_ras;
    let h = TA_FIRMWARE_HEADER_V1_0_hdcp;
    let d = TA_FIRMWARE_HEADER_V1_0_dtm;
    let s = TA_FIRMWARE_HEADER_V1_0_securedisplay;
    let (fv, sz, off) = (PSP_FW_LEGACY_BIN_DESC_fw_version, PSP_FW_LEGACY_BIN_DESC_size_bytes,
                         PSP_FW_LEGACY_BIN_DESC_offset_bytes);
    let ver = le32(ta, COMMON_FIRMWARE_HEADER_ucode_version);
    let vals = [
        (desc(x, fv), desc(x, sz), array),
        (desc(r, fv), desc(r, sz), array.wrapping_add(desc(r, off))),
        (desc(h, fv), desc(h, sz), array),
        (desc(d, fv), desc(d, sz), array.wrapping_add(desc(d, off))),
        (desc(s, fv), desc(s, sz), array.wrapping_add(desc(s, off))),
    ];
    let psp = &mut adev.psp;
    for (c, (fw_version, size_bytes, start)) in [
        &mut psp.xgmi_context, &mut psp.ras_context, &mut psp.hdcp_context, &mut psp.dtm_context,
        &mut psp.securedisplay_context,
    ].into_iter().zip(vals) {
        c.context.bin_desc.fw_version = fw_version;
        c.context.bin_desc.size_bytes = size_bytes;
        c.context.bin_desc.start_addr = Some((psp_image::ta_fw, start));
    }
    psp.ta_fw_version = ver;
    0
}

/// `parse_ta_v2_microcode`.
fn parse_ta_v2_microcode(adev: &mut amdgpu_device) -> i32 {
    let Some(ta) = adev.psp.ta_fw.as_ref() else { return -22 };
    if le16(ta, COMMON_FIRMWARE_HEADER_header_version_major) != 2 {
        return -22;
    }
    let count = le32(ta, TA_FIRMWARE_HEADER_V2_0_ta_fw_bin_count);
    if count >= UCODE_MAX_PSP_PACKAGING {
        Line::new().s("packed TA count exceeds maximum limit").say();
        return -22;
    }
    for i in 0..count {
        let err = parse_ta_bin_descriptor(adev, TA_FIRMWARE_HEADER_V2_0_ta_fw_bin + i * PSP_FW_BIN_DESC_SIZE);
        if err != 0 {
            return err;
        }
    }
    0
}

/// `psp_init_ta_microcode`.
pub fn psp_init_ta_microcode(adev: &mut amdgpu_device, chip_name: &str) -> i32 {
    let name = if amdgpu_is_kicker_fw(adev) {
        alloc::format!("amdgpu/{}_ta_kicker.bin", chip_name)
    } else {
        alloc::format!("amdgpu/{}_ta.bin", chip_name)
    };
    let (fw, err) = amdgpu_ucode_request(&name);
    adev.psp.ta_fw = fw;
    if err != 0 {
        return err;
    }
    let major = adev.psp.ta_fw.as_ref().map_or(0, |f| le16(f, COMMON_FIRMWARE_HEADER_header_version_major));
    let err = match major {
        1 => parse_ta_v1_microcode(adev),
        2 => parse_ta_v2_microcode(adev),
        _ => {
            Line::new().s("unsupported TA header version").say();
            -22
        }
    };
    if err != 0 {
        adev.psp.ta_fw = None;
    }
    err
}

/// `psp_v12_0_init_microcode`.
pub fn psp_v12_0_init_microcode(adev: &mut amdgpu_device) -> i32 {
    let ucode_prefix = amdgpu_ucode_ip_version_decode(adev, MP0_HWIP);

    let err = psp_init_asd_microcode(adev, &ucode_prefix);
    if err != 0 {
        return err;
    }
    let err = psp_init_ta_microcode(adev, &ucode_prefix);
    if err != 0 {
        return err;
    }
    // only supported on renoir
    if adev.apu_flags & AMD_APU_IS_RENOIR == 0 {
        adev.psp.securedisplay_context.context.bin_desc.size_bytes = 0;
    }
    0
}

/// `psp_early_init`: MP0 11.0.3, so psp_v12_0.
pub fn psp_early_init(adev: &mut amdgpu_device) -> i32 {
    let psp = &mut adev.psp;
    psp.autoload_supported = true;
    psp.boot_time_tmr = true;

    if adev.ip_version(MP0_HWIP, 0) == 11 << 24 | 3 << 8 {
        psp_v12_0_set_psp_funcs(&mut adev.psp);
        adev.psp.autoload_supported = false;
        adev.psp.boot_time_tmr = false;
    } else {
        return -22;
    }

    adev.psp_timeout = 20000;
    psp_check_pmfw_centralized_cstate_management(&mut adev.psp);

    // Bare metal: psp_init_microcode, `funcs->init_microcode`.
    psp_v12_0_init_microcode(adev)
}

/// The buffers `psp_sw_init` creates.
#[derive(Default, Clone, Copy)]
pub struct psp_buffers {
    pub fw_pri_bo: Option<usize>,
    pub fw_pri_mc_addr: u64,
    pub fw_pri_buf: Option<crate::amdgpu_object::amdgpu_kptr>,
    pub fence_buf_bo: Option<usize>,
    pub fence_buf_mc_addr: u64,
    pub fence_buf: Option<crate::amdgpu_object::amdgpu_kptr>,
    pub cmd_buf_bo: Option<usize>,
    pub cmd_buf_mc_addr: u64,
    pub cmd_buf_mem: Option<crate::amdgpu_object::amdgpu_kptr>,
}

/// One `amdgpu_bo_create_kernel` with its GPU address and mapping.
fn create(adev: &mut amdgpu_device, size: u64, align: u64, domain: u32)
          -> Result<(Option<usize>, u64, Option<crate::amdgpu_object::amdgpu_kptr>), i32> {
    let h = crate::amdgpu_object::amdgpu_bo_create_kernel(adev, size, align, domain, true)?;
    let Some(h) = h else { return Ok((None, 0, None)) };
    Ok((Some(h), crate::amdgpu_object::amdgpu_bo_gpu_offset(adev, h), crate::amdgpu_object::bo(adev, h).kptr))
}

/// `psp_sw_init`: an APU has no PSP runtime database, so SCPM is off and
/// two-stage memory training is asked for; `psp_memory_training_init`
/// finds no reservation and psp_v12_0 has no `mem_training`.
pub fn psp_sw_init(adev: &mut amdgpu_device) -> i32 {
    adev.psp.cmd = alloc::vec![0u8; PSP_GFX_CMD_RESP_SIZE as usize];
    adev.psp.xgmi_supports_extended_data = false;
    adev.scpm_enabled = false;
    adev.scpm_status = 0;
    adev.psp.enable_mem_training = true;
    let gtt = AMDGPU_GEM_DOMAIN_GTT;
    let both = AMDGPU_GEM_DOMAIN_VRAM | AMDGPU_GEM_DOMAIN_GTT;
    match create(adev, PSP_1_MEG as u64, PSP_1_MEG as u64, gtt) {
        Ok((b, a, p)) => (adev.psp.buf.fw_pri_bo, adev.psp.buf.fw_pri_mc_addr, adev.psp.buf.fw_pri_buf) = (b, a, p),
        Err(r) => return r,
    }
    match create(adev, PSP_FENCE_BUFFER_SIZE as u64, 4096, both) {
        Ok((b, a, p)) => (adev.psp.buf.fence_buf_bo, adev.psp.buf.fence_buf_mc_addr, adev.psp.buf.fence_buf) = (b, a, p),
        Err(r) => {
            crate::amdgpu_object::amdgpu_bo_free_kernel(adev, &mut adev.psp.buf.fw_pri_bo.clone());
            return r;
        }
    }
    match create(adev, PSP_CMD_BUFFER_SIZE as u64, 4096, both) {
        Ok((b, a, p)) => (adev.psp.buf.cmd_buf_bo, adev.psp.buf.cmd_buf_mc_addr, adev.psp.buf.cmd_buf_mem) = (b, a, p),
        Err(r) => return r,
    }
    0
}
