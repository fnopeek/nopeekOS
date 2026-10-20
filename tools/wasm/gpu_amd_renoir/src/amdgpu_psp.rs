//! amdgpu/amdgpu_psp.c, psp_v12_0.c and amdgpu_securedisplay.c (Linux
//! 6.18.55): the PSP block's early_init, which picks psp_v12_0 for MP0
//! 11.0.3 and requests the ASD and TA microcode, its sw_init, and its
//! hw_init, which hands every other block's firmware to the PSP.
//!
//! An APU's PSP is up before the driver: the system BIOS loaded its
//! bootloader stages and secure OS, so psp_v12_0 finds no sOS, KDB, SPL or
//! system driver image to load and starts at the command ring.

use crate::amdgpu_device::amdgpu_device;
use crate::amdgpu_ucode::{amdgpu_is_kicker_fw, amdgpu_ucode_ip_version_decode, amdgpu_ucode_request, le16, le32};
use crate::linux::{firmware, Line};
use crate::regs::*;
use alloc::vec::Vec;

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

/// `struct ta_mem_context`.
#[derive(Clone, Copy, Default)]
pub struct ta_mem_context {
    pub shared_bo: Option<usize>,
    pub shared_mc_addr: u64,
    pub shared_buf: Option<crate::amdgpu_object::amdgpu_kptr>,
    pub shared_mem_size: u32,
}

/// `struct ta_context`.
#[derive(Clone, Copy, Default)]
pub struct ta_context {
    pub bin_desc: psp_bin_desc,
    pub mem_context: ta_mem_context,
    pub ta_load_type: u32,
    pub session_id: u32,
    pub resp_status: u32,
    pub initialized: bool,
}

/// `struct psp_ring`.
#[derive(Clone, Copy, Default)]
pub struct psp_ring {
    pub ring_type: u32,
    pub ring_size: u32,
    pub ring_mem_mc_addr: u64,
    pub ring_mem: Option<crate::amdgpu_object::amdgpu_kptr>,
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
    pub km_ring: psp_ring,
    pub fence_value: u32,
    pub tmr_bo: Option<usize>,
    pub tmr_mc_addr: u64,
    /// `sos`, `toc` and `rl`: none in an APU's PSP firmware.
    pub sos: psp_bin_desc,
    pub toc: psp_bin_desc,
    pub rl: psp_bin_desc,
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

fn put32(cmd: &mut [u8], at: u32, v: u32) {
    cmd[at as usize..at as usize + 4].copy_from_slice(&v.to_le_bytes());
}

fn get32(cmd: &[u8], at: u32) -> u32 {
    let a = at as usize;
    u32::from_le_bytes([cmd[a], cmd[a + 1], cmd[a + 2], cmd[a + 3]])
}

/// `SOC15_REG_OFFSET(MP0, 0, reg)`.
fn mp0(adev: &amdgpu_device, base_idx: u32, reg: u32) -> u32 {
    adev.reg_offset[MP0_HWIP as usize][0].get(base_idx as usize).copied().unwrap_or(0) + reg
}

/// `psp_wait_for`.
pub fn psp_wait_for(adev: &amdgpu_device, reg_index: u32, reg_val: u32, mask: u32, flags: u32) -> i32 {
    let check_changed = flags & PSP_WAITREG_CHANGED != 0;
    let verbose = flags & PSP_WAITREG_NOVERBOSE == 0;
    let mut val = 0;
    for _ in 0..adev.usec_timeout {
        val = adev.rreg32(reg_index).unwrap_or(0);
        if check_changed {
            if val != reg_val {
                return 0;
            }
        } else if val & mask == reg_val {
            return 0;
        }
        crate::linux::udelay(1);
    }
    if verbose {
        Line::new().s("psp reg (0x").hex(reg_index).s(") wait timed out, mask: ").hex(mask).s(" read: ").hex(val)
            .s(" exp: ").hex(reg_val).say();
    }
    -62
}

/// `psp_v12_0_ring_create`.
fn psp_v12_0_ring_create(adev: &mut amdgpu_device, ring_type: u32) -> i32 {
    let ring = adev.psp.km_ring;
    let w = |adev: &amdgpu_device, idx: u32, reg: u32, v: u32| adev.wreg32(mp0(adev, idx, reg), v);
    // Write low address of the ring to C2PMSG_69
    w(adev, MP0_mmMP0_SMN_C2PMSG_69_BASE_IDX, MP0_mmMP0_SMN_C2PMSG_69, ring.ring_mem_mc_addr as u32);
    // Write high address of the ring to C2PMSG_70
    w(adev, MP0_mmMP0_SMN_C2PMSG_70_BASE_IDX, MP0_mmMP0_SMN_C2PMSG_70, (ring.ring_mem_mc_addr >> 32) as u32);
    // Write size of ring to C2PMSG_71
    w(adev, MP0_mmMP0_SMN_C2PMSG_71_BASE_IDX, MP0_mmMP0_SMN_C2PMSG_71, ring.ring_size);
    // Write the ring initialization command to C2PMSG_64
    w(adev, MP0_mmMP0_SMN_C2PMSG_64_BASE_IDX, MP0_mmMP0_SMN_C2PMSG_64, ring_type << 16);
    // Wait for response flag (bit 31) in C2PMSG_64
    let reg = mp0(adev, MP0_mmMP0_SMN_C2PMSG_64_BASE_IDX, MP0_mmMP0_SMN_C2PMSG_64);
    psp_wait_for(adev, reg, MBOX_TOS_RESP_FLAG, MBOX_TOS_RESP_MASK, 0)
}

/// `psp_v12_0_ring_get_wptr`, bare metal.
fn psp_v12_0_ring_get_wptr(adev: &amdgpu_device) -> u32 {
    adev.rreg32(mp0(adev, MP0_mmMP0_SMN_C2PMSG_67_BASE_IDX, MP0_mmMP0_SMN_C2PMSG_67)).unwrap_or(0)
}

/// `psp_v12_0_ring_set_wptr`, bare metal.
fn psp_v12_0_ring_set_wptr(adev: &amdgpu_device, value: u32) {
    adev.wreg32(mp0(adev, MP0_mmMP0_SMN_C2PMSG_67_BASE_IDX, MP0_mmMP0_SMN_C2PMSG_67), value);
}

/// `psp_ring_init`: 4k of frame buffer for the ring.
fn psp_ring_init(adev: &mut amdgpu_device, ring_type: u32) -> i32 {
    adev.psp.km_ring.ring_type = ring_type;
    adev.psp.km_ring.ring_size = 0x1000;
    match create(adev, 0x1000, crate::amdgpu_object::PAGE_SIZE, AMDGPU_GEM_DOMAIN_VRAM | AMDGPU_GEM_DOMAIN_GTT) {
        Ok((b, a, p)) => {
            adev.firmware.rbuf = b;
            adev.psp.km_ring.ring_mem_mc_addr = a;
            adev.psp.km_ring.ring_mem = p;
            0
        }
        Err(r) => {
            adev.psp.km_ring.ring_size = 0;
            r
        }
    }
}

/// `psp_ring_cmd_submit`.
fn psp_ring_cmd_submit(adev: &mut amdgpu_device, cmd_buf_mc_addr: u64, fence_mc_addr: u64, index: u32) -> i32 {
    let ring = adev.psp.km_ring;
    let Some(mem) = ring.ring_mem else { return -22 };
    let frame = SIZEOF_psp_gfx_rb_frame;
    let ring_size_dw = ring.ring_size / 4;
    let rb_frame_size_dw = frame / 4;
    // KM (GPCOM) prepare write pointer
    let mut psp_write_ptr_reg = psp_v12_0_ring_get_wptr(adev);
    // Update KM RB frame pointer to new frame
    let write_frame = if psp_write_ptr_reg % ring_size_dw == 0 { 0 } else { psp_write_ptr_reg / rb_frame_size_dw };
    // Check invalid write_frame ptr address
    if write_frame > ring.ring_size / frame - 1 {
        Line::new().s("write_frame is pointing to address out of bounds").say();
        return -22;
    }
    let at = write_frame as u64 * frame as u64;
    // Initialize KM RB frame
    crate::amdgpu_gart::kptr_fill32(adev, mem, at, frame as u64, 0);
    // Update KM RB frame
    let w = |adev: &amdgpu_device, off: u32, v: u32| crate::amdgpu_gart::kptr_w32(adev, mem, at + off as u64, v);
    w(adev, FRAME_CMD_BUF_ADDR_HI, (cmd_buf_mc_addr >> 32) as u32);
    w(adev, FRAME_CMD_BUF_ADDR_LO, cmd_buf_mc_addr as u32);
    w(adev, FRAME_FENCE_ADDR_HI, (fence_mc_addr >> 32) as u32);
    w(adev, FRAME_FENCE_ADDR_LO, fence_mc_addr as u32);
    w(adev, FRAME_FENCE_VALUE, index);
    // amdgpu_device_flush_hdp: nothing on an APU.
    // Update the write Pointer in DWORDs
    psp_write_ptr_reg = (psp_write_ptr_reg + rb_frame_size_dw) % ring_size_dw;
    psp_v12_0_ring_set_wptr(adev, psp_write_ptr_reg);
    0
}

/// `acquire_psp_cmd_buf`: a zeroed command.
fn acquire_psp_cmd_buf() -> Vec<u8> {
    alloc::vec![0u8; SIZEOF_psp_gfx_cmd_resp as usize]
}

/// `psp_gfx_cmd_name`.
fn psp_gfx_cmd_name(cmd_id: u32) -> &'static str {
    match cmd_id {
        GFX_CMD_ID_LOAD_TA => "LOAD_TA",
        GFX_CMD_ID_UNLOAD_TA => "UNLOAD_TA",
        GFX_CMD_ID_INVOKE_CMD => "INVOKE_CMD",
        GFX_CMD_ID_LOAD_ASD => "LOAD_ASD",
        GFX_CMD_ID_SETUP_TMR => "SETUP_TMR",
        GFX_CMD_ID_LOAD_IP_FW => "LOAD_IP_FW",
        GFX_CMD_ID_DESTROY_TMR => "DESTROY_TMR",
        GFX_CMD_ID_SAVE_RESTORE => "SAVE_RESTORE_IP_FW",
        GFX_CMD_ID_SETUP_VMR => "SETUP_VMR",
        GFX_CMD_ID_DESTROY_VMR => "DESTROY_VMR",
        GFX_CMD_ID_PROG_REG => "PROG_REG",
        GFX_CMD_ID_GET_FW_ATTESTATION => "GET_FW_ATTESTATION",
        GFX_CMD_ID_LOAD_TOC => "ID_LOAD_TOC",
        GFX_CMD_ID_AUTOLOAD_RLC => "AUTOLOAD_RLC",
        GFX_CMD_ID_BOOT_CFG => "BOOT_CFG",
        GFX_CMD_ID_CONFIG_SQ_PERFMON => "CONFIG_SQ_PERFMON",
        GFX_CMD_ID_FB_FW_RESERV_ADDR => "FB_FW_RESERV_ADDR",
        GFX_CMD_ID_FB_FW_RESERV_EXT_ADDR => "FB_FW_RESERV_EXT_ADDR",
        GFX_CMD_ID_SRIOV_SPATIAL_PART => "SPATIAL_PARTITION",
        GFX_CMD_ID_FB_NPS_MODE => "NPS_MODE_CHANGE",
        _ => "UNKNOWN CMD",
    }
}

/// `psp_cmd_submit_buf`. `ucode` is the index into `adev.firmware.ucode`
/// a firmware load reports to. The response is copied back into `cmd`.
fn psp_cmd_submit_buf(adev: &mut amdgpu_device, ucode: Option<usize>, cmd: &mut [u8], fence_mc_addr: u64) -> i32 {
    let mut timeout = adev.psp_timeout;
    let b = adev.psp.buf;
    let (Some(cmd_mem), Some(fence)) = (b.cmd_buf_mem, b.fence_buf) else { return -22 };
    crate::amdgpu_gart::kptr_fill32(adev, cmd_mem, 0, PSP_CMD_BUFFER_SIZE as u64, 0);
    crate::amdgpu_gart::kptr_write(adev, cmd_mem, 0, cmd);

    adev.psp.fence_value = adev.psp.fence_value.wrapping_add(1);
    let index = adev.psp.fence_value;
    let ret = psp_ring_cmd_submit(adev, b.cmd_buf_mc_addr, fence_mc_addr, index);
    if ret != 0 {
        adev.psp.fence_value = adev.psp.fence_value.wrapping_sub(1);
        return ret;
    }
    // amdgpu_device_invalidate_hdp: nothing on an APU.
    while crate::amdgpu_gart::kptr_r32(adev, fence, 0) != index {
        timeout -= 1;
        if timeout == 0 {
            break;
        }
        crate::linux::udelay(60);
    }

    let resp_at = CMDRESP_RESP as u64;
    let mut resp = alloc::vec![0u8; SIZEOF_psp_gfx_resp as usize];
    for (i, c) in resp.chunks_mut(4).enumerate() {
        let v = crate::amdgpu_gart::kptr_r32(adev, cmd_mem, resp_at + i as u64 * 4).to_le_bytes();
        let n = c.len();
        c.copy_from_slice(&v[..n]);
    }
    let status = get32(&resp, RESP_STATUS);
    let cmd_id = crate::amdgpu_gart::kptr_r32(adev, cmd_mem, CMDRESP_CMD_ID as u64);
    let r = CMDRESP_RESP as usize;
    cmd[r..r + resp.len()].copy_from_slice(&resp);

    // In some cases, psp response status is not 0 even there is no problem
    // while the command is submitted. Some version of PSP FW doesn't write
    // 0 to that field. So here we would like to only print a warning
    // instead of an error during psp initialization to avoid breaking
    // hw_init and it doesn't return -EINVAL.
    if status != 0 || timeout == 0 {
        if let Some(u) = ucode {
            let id = adev.firmware.ucode[u].ucode_id;
            Line::new().s("failed to load ucode id ").dec(id as u64).s(" (0x").hexu(id).s(")").say();
        }
        Line::new().s("psp gfx command ").s(psp_gfx_cmd_name(cmd_id)).s("(0x").hexu(cmd_id)
            .s(") failed and response status is (0x").hexu(status).s(")").say();
        // Also return failure in case of timeout
        if timeout == 0 {
            return -22;
        }
    }
    if let Some(u) = ucode {
        adev.firmware.ucode[u].tmr_mc_addr_lo = get32(&resp, RESP_FW_ADDR_LO);
        adev.firmware.ucode[u].tmr_mc_addr_hi = get32(&resp, RESP_FW_ADDR_HI);
    }
    0
}

/// `psp_tmr_init`: no TOC, so `PSP_TMR_SIZE`, aligned to itself as the
/// hardware wants ("naturally aligned").
fn psp_tmr_init(adev: &mut amdgpu_device) -> i32 {
    let tmr_size = PSP_TMR_SIZE_RENOIR;
    if adev.psp.toc.start_addr.is_some() && adev.psp.toc.size_bytes != 0 {
        Line::new().s("psp_load_toc: not ported, an APU's PSP firmware has no TOC").say();
        return -22;
    }
    if adev.psp.tmr_bo.is_none() && !adev.psp.boot_time_tmr {
        match create(adev, tmr_size as u64, PSP_TMR_ALIGNMENT as u64, AMDGPU_GEM_DOMAIN_GTT | AMDGPU_GEM_DOMAIN_VRAM) {
            Ok((b, a, _)) => {
                adev.psp.tmr_bo = b;
                adev.psp.tmr_mc_addr = a;
            }
            Err(r) => return r,
        }
    }
    0
}

/// `psp_tmr_load`.
fn psp_tmr_load(adev: &mut amdgpu_device) -> i32 {
    let mut cmd = acquire_psp_cmd_buf();
    // psp_prep_tmr_cmd_buf
    let (size, tmr_pa) = match adev.psp.tmr_bo {
        Some(h) => {
            let gpu = crate::amdgpu_object::amdgpu_bo_gpu_offset(adev, h);
            (crate::amdgpu_object::amdgpu_bo_size(adev, h) as u32, crate::amdgpu_gmc::amdgpu_gmc_vram_mc2pa(adev, gpu))
        }
        None => (0, 0),
    };
    let tmr_mc = adev.psp.tmr_mc_addr;
    let c = CMDRESP_CMD;
    put32(&mut cmd, CMDRESP_CMD_ID, GFX_CMD_ID_SETUP_TMR);
    put32(&mut cmd, c + SETUPTMR_BUF_PHY_ADDR_LO, tmr_mc as u32);
    put32(&mut cmd, c + SETUPTMR_BUF_PHY_ADDR_HI, (tmr_mc >> 32) as u32);
    put32(&mut cmd, c + SETUPTMR_BUF_SIZE, size);
    put32(&mut cmd, c + SETUPTMR_TMR_FLAGS, SETUPTMR_VIRT_PHY_ADDR_FLAG);
    put32(&mut cmd, c + SETUPTMR_SYSTEM_PHY_ADDR_LO, tmr_pa as u32);
    put32(&mut cmd, c + SETUPTMR_SYSTEM_PHY_ADDR_HI, (tmr_pa >> 32) as u32);
    if let Some(h) = adev.psp.tmr_bo {
        Line::new().s("reserve ").hex(crate::amdgpu_object::amdgpu_bo_size(adev, h) as u32).s(" from ")
            .hex64(tmr_mc).s(" for PSP TMR").say();
    }
    let fence = adev.psp.buf.fence_buf_mc_addr;
    psp_cmd_submit_buf(adev, None, &mut cmd, fence)
}

/// `psp_hw_start`: no bootloader images (see the module comment), MP0
/// 11.0.3 updates no firmware reservation, and the TMR is the driver's.
fn psp_hw_start(adev: &mut amdgpu_device) -> i32 {
    let r = psp_v12_0_ring_create(adev, PSP_RING_TYPE__KM);
    if r != 0 {
        Line::new().s("PSP create ring failed!").say();
        return r;
    }
    if !adev.psp.boot_time_tmr || adev.psp.autoload_supported {
        let r = psp_tmr_init(adev);
        if r != 0 {
            Line::new().s("PSP tmr init failed!").say();
            return r;
        }
    }
    // For ASICs with DF Cstate management centralized to PMFW, TMR setup
    // should be performed after PMFW loaded and before other non-psp
    // firmware loaded.
    if adev.psp.pmfw_centralized_cstate_management {
        Line::new().s("psp_load_smu_fw: an APU has no SMU image").say();
        return -22;
    }
    if !adev.psp.boot_time_tmr || !adev.psp.autoload_supported {
        let r = psp_tmr_load(adev);
        if r != 0 {
            Line::new().s("PSP load tmr failed!").say();
            return r;
        }
    }
    0
}

/// `psp_get_fw_type`.
fn psp_get_fw_type(ucode_id: u32) -> Option<u32> {
    Some(match ucode_id {
        AMDGPU_UCODE_ID_CAP => GFX_FW_TYPE_CAP,
        AMDGPU_UCODE_ID_SDMA0 => GFX_FW_TYPE_SDMA0,
        AMDGPU_UCODE_ID_SDMA1 => GFX_FW_TYPE_SDMA1,
        AMDGPU_UCODE_ID_SDMA2 => GFX_FW_TYPE_SDMA2,
        AMDGPU_UCODE_ID_SDMA3 => GFX_FW_TYPE_SDMA3,
        AMDGPU_UCODE_ID_SDMA4 => GFX_FW_TYPE_SDMA4,
        AMDGPU_UCODE_ID_SDMA5 => GFX_FW_TYPE_SDMA5,
        AMDGPU_UCODE_ID_SDMA6 => GFX_FW_TYPE_SDMA6,
        AMDGPU_UCODE_ID_SDMA7 => GFX_FW_TYPE_SDMA7,
        AMDGPU_UCODE_ID_CP_MES => GFX_FW_TYPE_CP_MES,
        AMDGPU_UCODE_ID_CP_MES_DATA => GFX_FW_TYPE_MES_STACK,
        AMDGPU_UCODE_ID_CP_MES1 => GFX_FW_TYPE_CP_MES_KIQ,
        AMDGPU_UCODE_ID_CP_MES1_DATA => GFX_FW_TYPE_MES_KIQ_STACK,
        AMDGPU_UCODE_ID_CP_CE => GFX_FW_TYPE_CP_CE,
        AMDGPU_UCODE_ID_CP_PFP => GFX_FW_TYPE_CP_PFP,
        AMDGPU_UCODE_ID_CP_ME => GFX_FW_TYPE_CP_ME,
        AMDGPU_UCODE_ID_CP_MEC1 | AMDGPU_UCODE_ID_CP_MEC2 => GFX_FW_TYPE_CP_MEC,
        AMDGPU_UCODE_ID_CP_MEC1_JT => GFX_FW_TYPE_CP_MEC_ME1,
        AMDGPU_UCODE_ID_CP_MEC2_JT => GFX_FW_TYPE_CP_MEC_ME2,
        AMDGPU_UCODE_ID_RLC_P => GFX_FW_TYPE_RLC_P,
        AMDGPU_UCODE_ID_RLC_V => GFX_FW_TYPE_RLC_V,
        AMDGPU_UCODE_ID_RLC_G => GFX_FW_TYPE_RLC_G,
        AMDGPU_UCODE_ID_RLC_RESTORE_LIST_CNTL => GFX_FW_TYPE_RLC_RESTORE_LIST_SRM_CNTL,
        AMDGPU_UCODE_ID_RLC_RESTORE_LIST_GPM_MEM => GFX_FW_TYPE_RLC_RESTORE_LIST_GPM_MEM,
        AMDGPU_UCODE_ID_RLC_RESTORE_LIST_SRM_MEM => GFX_FW_TYPE_RLC_RESTORE_LIST_SRM_MEM,
        AMDGPU_UCODE_ID_RLC_IRAM => GFX_FW_TYPE_RLC_IRAM,
        AMDGPU_UCODE_ID_RLC_DRAM => GFX_FW_TYPE_RLC_DRAM_BOOT,
        AMDGPU_UCODE_ID_GLOBAL_TAP_DELAYS => GFX_FW_TYPE_GLOBAL_TAP_DELAYS,
        AMDGPU_UCODE_ID_SE0_TAP_DELAYS => GFX_FW_TYPE_SE0_TAP_DELAYS,
        AMDGPU_UCODE_ID_SE1_TAP_DELAYS => GFX_FW_TYPE_SE1_TAP_DELAYS,
        AMDGPU_UCODE_ID_SE2_TAP_DELAYS => GFX_FW_TYPE_SE2_TAP_DELAYS,
        AMDGPU_UCODE_ID_SE3_TAP_DELAYS => GFX_FW_TYPE_SE3_TAP_DELAYS,
        AMDGPU_UCODE_ID_SMC => GFX_FW_TYPE_SMU,
        AMDGPU_UCODE_ID_PPTABLE => GFX_FW_TYPE_PPTABLE,
        AMDGPU_UCODE_ID_UVD => GFX_FW_TYPE_UVD,
        AMDGPU_UCODE_ID_UVD1 => GFX_FW_TYPE_UVD1,
        AMDGPU_UCODE_ID_VCE => GFX_FW_TYPE_VCE,
        AMDGPU_UCODE_ID_VCN => GFX_FW_TYPE_VCN,
        AMDGPU_UCODE_ID_VCN1 => GFX_FW_TYPE_VCN1,
        AMDGPU_UCODE_ID_DMCU_ERAM => GFX_FW_TYPE_DMCU_ERAM,
        AMDGPU_UCODE_ID_DMCU_INTV => GFX_FW_TYPE_DMCU_ISR,
        AMDGPU_UCODE_ID_VCN0_RAM => GFX_FW_TYPE_VCN0_RAM,
        AMDGPU_UCODE_ID_VCN1_RAM => GFX_FW_TYPE_VCN1_RAM,
        AMDGPU_UCODE_ID_DMCUB => GFX_FW_TYPE_DMUB,
        AMDGPU_UCODE_ID_SDMA_UCODE_TH0 | AMDGPU_UCODE_ID_SDMA_RS64 => GFX_FW_TYPE_SDMA_UCODE_TH0,
        AMDGPU_UCODE_ID_SDMA_UCODE_TH1 => GFX_FW_TYPE_SDMA_UCODE_TH1,
        AMDGPU_UCODE_ID_IMU_I => GFX_FW_TYPE_IMU_I,
        AMDGPU_UCODE_ID_IMU_D => GFX_FW_TYPE_IMU_D,
        AMDGPU_UCODE_ID_CP_RS64_PFP => GFX_FW_TYPE_RS64_PFP,
        AMDGPU_UCODE_ID_CP_RS64_ME => GFX_FW_TYPE_RS64_ME,
        AMDGPU_UCODE_ID_CP_RS64_MEC => GFX_FW_TYPE_RS64_MEC,
        AMDGPU_UCODE_ID_CP_RS64_PFP_P0_STACK => GFX_FW_TYPE_RS64_PFP_P0_STACK,
        AMDGPU_UCODE_ID_CP_RS64_PFP_P1_STACK => GFX_FW_TYPE_RS64_PFP_P1_STACK,
        AMDGPU_UCODE_ID_CP_RS64_ME_P0_STACK => GFX_FW_TYPE_RS64_ME_P0_STACK,
        AMDGPU_UCODE_ID_CP_RS64_ME_P1_STACK => GFX_FW_TYPE_RS64_ME_P1_STACK,
        AMDGPU_UCODE_ID_CP_RS64_MEC_P0_STACK => GFX_FW_TYPE_RS64_MEC_P0_STACK,
        AMDGPU_UCODE_ID_CP_RS64_MEC_P1_STACK => GFX_FW_TYPE_RS64_MEC_P1_STACK,
        AMDGPU_UCODE_ID_CP_RS64_MEC_P2_STACK => GFX_FW_TYPE_RS64_MEC_P2_STACK,
        AMDGPU_UCODE_ID_CP_RS64_MEC_P3_STACK => GFX_FW_TYPE_RS64_MEC_P3_STACK,
        AMDGPU_UCODE_ID_VPE_CTX => GFX_FW_TYPE_VPEC_FW1,
        AMDGPU_UCODE_ID_VPE_CTL => GFX_FW_TYPE_VPEC_FW2,
        AMDGPU_UCODE_ID_VPE => GFX_FW_TYPE_VPE,
        AMDGPU_UCODE_ID_UMSCH_MM_UCODE => GFX_FW_TYPE_UMSCH_UCODE,
        AMDGPU_UCODE_ID_UMSCH_MM_DATA => GFX_FW_TYPE_UMSCH_DATA,
        AMDGPU_UCODE_ID_UMSCH_MM_CMD_BUFFER => GFX_FW_TYPE_UMSCH_CMD_BUFFER,
        AMDGPU_UCODE_ID_P2S_TABLE => GFX_FW_TYPE_P2S_TABLE,
        AMDGPU_UCODE_ID_JPEG_RAM => GFX_FW_TYPE_JPEG_RAM,
        AMDGPU_UCODE_ID_ISP => GFX_FW_TYPE_ISP,
        _ => return None,
    })
}

/// `fw_load_skip_check`: no SMU image on an APU, no autoload.
fn fw_load_skip_check(adev: &amdgpu_device, i: usize) -> bool {
    let u = &adev.firmware.ucode[i];
    if u.fw.is_none() || u.ucode_size == 0 {
        return true;
    }
    if u.ucode_id == AMDGPU_UCODE_ID_P2S_TABLE {
        return true;
    }
    if u.ucode_id == AMDGPU_UCODE_ID_SMC && (adev.psp.autoload_supported || adev.psp.pmfw_centralized_cstate_management) {
        return true;
    }
    if adev.psp.autoload_supported && (u.ucode_id == AMDGPU_UCODE_ID_CP_MEC1_JT || u.ucode_id == AMDGPU_UCODE_ID_CP_MEC2_JT) {
        // skip mec JT when autoload is enabled
        return true;
    }
    false
}

/// `psp_execute_ip_fw_load`, with `psp_prep_load_ip_fw_cmd_buf`.
fn psp_execute_ip_fw_load(adev: &mut amdgpu_device, i: usize) -> i32 {
    let mut cmd = acquire_psp_cmd_buf();
    let u = adev.firmware.ucode[i];
    put32(&mut cmd, CMDRESP_CMD_ID, GFX_CMD_ID_LOAD_IP_FW);
    let c = CMDRESP_CMD;
    put32(&mut cmd, c + LOADIPFW_FW_PHY_ADDR_LO, u.mc_addr as u32);
    put32(&mut cmd, c + LOADIPFW_FW_PHY_ADDR_HI, (u.mc_addr >> 32) as u32);
    put32(&mut cmd, c + LOADIPFW_FW_SIZE, u.ucode_size);
    let Some(t) = psp_get_fw_type(u.ucode_id) else {
        Line::new().s("Unknown firmware type").say();
        return -22;
    };
    put32(&mut cmd, c + LOADIPFW_FW_TYPE, t);
    let fence = adev.psp.buf.fence_buf_mc_addr;
    psp_cmd_submit_buf(adev, Some(i), &mut cmd, fence)
}

/// `psp_load_non_psp_fw`: no autoload, no P2S table, no SMU image.
fn psp_load_non_psp_fw(adev: &mut amdgpu_device) -> i32 {
    for i in 0..adev.firmware.max_ucodes as usize {
        let id = adev.firmware.ucode[i].ucode_id;
        if id == AMDGPU_UCODE_ID_SMC && !fw_load_skip_check(adev, i) {
            Line::new().s("psp_load_smu_fw: an APU has no SMU image").say();
            return -22;
        }
        if fw_load_skip_check(adev, i) {
            continue;
        }
        Line::new().s("psp: load ucode id ").dec(id as u64).s(", ").dec(adev.firmware.ucode[i].ucode_size as u64)
            .s(" bytes").dbg();
        let r = psp_execute_ip_fw_load(adev, i);
        if r != 0 {
            return r;
        }
    }
    0
}

/// The bytes a `psp_bin_desc` describes.
fn bin_bytes(adev: &amdgpu_device, d: &psp_bin_desc) -> Option<Vec<u8>> {
    let (img, at) = d.start_addr?;
    let fw = match img {
        psp_image::asd_fw => adev.psp.asd_fw.as_ref()?,
        psp_image::ta_fw => adev.psp.ta_fw.as_ref()?,
    };
    fw.data.get(at as usize..at as usize + d.size_bytes as usize).map(|b| b.to_vec())
}

/// `psp_copy_fw`.
fn psp_copy_fw(adev: &mut amdgpu_device, bin: &[u8]) -> i32 {
    if bin.is_empty() || bin.len() > PSP_1_MEG as usize {
        Line::new().s("PSP firmware is invalid").say();
        return -22;
    }
    let Some(k) = adev.psp.buf.fw_pri_buf else { return -22 };
    crate::amdgpu_gart::kptr_fill32(adev, k, 0, PSP_1_MEG as u64, 0);
    crate::amdgpu_gart::kptr_write(adev, k, 0, bin);
    0
}

/// Which `ta_context` of `adev.psp`.
#[derive(Clone, Copy)]
enum ta_id {
    asd,
    ras,
    hdcp,
    dtm,
    rap,
    securedisplay,
}

fn ta(adev: &mut amdgpu_device, t: ta_id) -> &mut ta_context {
    let p = &mut adev.psp;
    match t {
        ta_id::asd => &mut p.asd_context,
        ta_id::ras => &mut p.ras_context.context,
        ta_id::hdcp => &mut p.hdcp_context.context,
        ta_id::dtm => &mut p.dtm_context.context,
        ta_id::rap => &mut p.rap_context.context,
        ta_id::securedisplay => &mut p.securedisplay_context.context,
    }
}

/// `psp_ta_load`, with `psp_prep_ta_load_cmd_buf`.
fn psp_ta_load(adev: &mut amdgpu_device, t: ta_id) -> i32 {
    let mut cmd = acquire_psp_cmd_buf();
    let context = *ta(adev, t);
    let Some(bin) = bin_bytes(adev, &context.bin_desc) else { return -22 };
    let r = psp_copy_fw(adev, &bin);
    if r != 0 {
        return r;
    }
    let ta_bin_mc = adev.psp.buf.fw_pri_mc_addr;
    let c = CMDRESP_CMD;
    put32(&mut cmd, CMDRESP_CMD_ID, context.ta_load_type);
    put32(&mut cmd, c + LOADTA_APP_PHY_ADDR_LO, ta_bin_mc as u32);
    put32(&mut cmd, c + LOADTA_APP_PHY_ADDR_HI, (ta_bin_mc >> 32) as u32);
    put32(&mut cmd, c + LOADTA_APP_LEN, context.bin_desc.size_bytes);
    let m = context.mem_context;
    put32(&mut cmd, c + LOADTA_CMD_BUF_PHY_ADDR_LO, m.shared_mc_addr as u32);
    put32(&mut cmd, c + LOADTA_CMD_BUF_PHY_ADDR_HI, (m.shared_mc_addr >> 32) as u32);
    put32(&mut cmd, c + LOADTA_CMD_BUF_LEN, m.shared_mem_size);
    let fence = adev.psp.buf.fence_buf_mc_addr;
    let ret = psp_cmd_submit_buf(adev, None, &mut cmd, fence);
    let ctx = ta(adev, t);
    ctx.resp_status = get32(&cmd, CMDRESP_RESP + RESP_STATUS);
    if ret == 0 {
        ctx.session_id = get32(&cmd, CMDRESP_RESP + RESP_SESSION_ID);
    }
    ret
}

/// `psp_ta_invoke`, with `psp_prep_ta_invoke_cmd_buf`.
fn psp_ta_invoke(adev: &mut amdgpu_device, ta_cmd_id: u32, t: ta_id) -> i32 {
    let mut cmd = acquire_psp_cmd_buf();
    let session_id = ta(adev, t).session_id;
    put32(&mut cmd, CMDRESP_CMD_ID, GFX_CMD_ID_INVOKE_CMD);
    put32(&mut cmd, CMDRESP_CMD + INVOKE_SESSION_ID, session_id);
    put32(&mut cmd, CMDRESP_CMD + INVOKE_TA_CMD_ID, ta_cmd_id);
    let fence = adev.psp.buf.fence_buf_mc_addr;
    let ret = psp_cmd_submit_buf(adev, None, &mut cmd, fence);
    ta(adev, t).resp_status = get32(&cmd, CMDRESP_RESP + RESP_STATUS);
    ret
}

/// `psp_ta_init_shared_buf`: 16k aligned to 4k from the frame buffer for
/// TA to host memory.
fn psp_ta_init_shared_buf(adev: &mut amdgpu_device, t: ta_id) -> i32 {
    let size = ta(adev, t).mem_context.shared_mem_size as u64;
    match create(adev, size, crate::amdgpu_object::PAGE_SIZE, AMDGPU_GEM_DOMAIN_VRAM | AMDGPU_GEM_DOMAIN_GTT) {
        Ok((b, a, p)) => {
            let m = &mut ta(adev, t).mem_context;
            (m.shared_bo, m.shared_mc_addr, m.shared_buf) = (b, a, p);
            0
        }
        Err(r) => r,
    }
}

/// `psp_asd_initialize`.
fn psp_asd_initialize(adev: &mut amdgpu_device) -> i32 {
    if adev.psp.asd_context.bin_desc.size_bytes == 0 {
        return 0;
    }
    let c = &mut adev.psp.asd_context;
    c.mem_context.shared_mc_addr = 0;
    c.mem_context.shared_mem_size = PSP_ASD_SHARED_MEM_SIZE;
    c.ta_load_type = GFX_CMD_ID_LOAD_ASD;
    let r = psp_ta_load(adev, ta_id::asd);
    if r == 0 {
        adev.psp.asd_context.initialized = true;
    }
    r
}

/// `amdgpu_device_has_display_hardware`, the IP discovery branch.
fn amdgpu_device_has_display_hardware(adev: &amdgpu_device) -> bool {
    adev.ip_version(DCE_HWIP, 0) != 0 && adev.harvest_ip_mask & AMD_HARVEST_IP_DMU_MASK == 0
}

/// `psp_ras_initialize`: Renoir's TA image carries no RAS TA; with one
/// this port stops rather than guess its boot config.
fn psp_ras_initialize(adev: &mut amdgpu_device) -> i32 {
    let d = adev.psp.ras_context.context.bin_desc;
    if d.size_bytes == 0 || d.start_addr.is_none() {
        Line::new().s("RAS: optional ras ta ucode is not available").say();
        return 0;
    }
    Line::new().s("RAS: the RAS TA is not ported").say();
    -22
}

/// `psp_hdcp_initialize` and `psp_dtm_initialize`: load the TA with its
/// shared buffer.
fn psp_display_ta_initialize(adev: &mut amdgpu_device, t: ta_id, missing: &str, shared_mem_size: u32) -> i32 {
    // bypass initialization if dmu is harvested
    if !amdgpu_device_has_display_hardware(adev) {
        return 0;
    }
    let d = ta(adev, t).bin_desc;
    if d.size_bytes == 0 || d.start_addr.is_none() {
        Line::new().s(missing).say();
        return 0;
    }
    let c = ta(adev, t);
    c.mem_context.shared_mem_size = shared_mem_size;
    c.ta_load_type = GFX_CMD_ID_LOAD_TA;
    if c.mem_context.shared_buf.is_none() {
        let r = psp_ta_init_shared_buf(adev, t);
        if r != 0 {
            return r;
        }
    }
    let r = psp_ta_load(adev, t);
    if r == 0 {
        ta(adev, t).initialized = true;
    }
    r
}

/// `psp_rap_initialize`: no RAP TA on Renoir.
fn psp_rap_initialize(adev: &mut amdgpu_device) -> i32 {
    let d = adev.psp.rap_context.context.bin_desc;
    if d.size_bytes == 0 || d.start_addr.is_none() {
        Line::new().s("RAP: optional rap ta ucode is not available").say();
        return 0;
    }
    Line::new().s("RAP: the RAP TA is not ported").say();
    -22
}

/// `psp_securedisplay_parse_resp_status`.
fn psp_securedisplay_parse_resp_status(status: u32) {
    let what = match status {
        TA_SECUREDISPLAY_STATUS__SUCCESS => return,
        TA_SECUREDISPLAY_STATUS__GENERIC_FAILURE => "Generic Failure.",
        _ => {
            Line::new().s("Secure display: Failed to parse status: ").dec(status as u64).say();
            return;
        }
    };
    Line::new().s("Secure display: ").s(what).say();
}

/// `psp_securedisplay_initialize`, with the QUERY_TA invoke.
fn psp_securedisplay_initialize(adev: &mut amdgpu_device) -> i32 {
    // bypass securedisplay initialization if dmu is harvested
    if !amdgpu_device_has_display_hardware(adev) {
        return 0;
    }
    let d = adev.psp.securedisplay_context.context.bin_desc;
    if d.size_bytes == 0 || d.start_addr.is_none() {
        Line::new().s("SECUREDISPLAY: optional securedisplay ta ucode is not available").say();
        return 0;
    }
    let c = &mut adev.psp.securedisplay_context.context;
    c.mem_context.shared_mem_size = PSP_SECUREDISPLAY_SHARED_MEM_SIZE;
    c.ta_load_type = GFX_CMD_ID_LOAD_TA;
    if !c.initialized {
        let r = psp_ta_init_shared_buf(adev, ta_id::securedisplay);
        if r != 0 {
            return r;
        }
    }
    let r = psp_ta_load(adev, ta_id::securedisplay);
    let c = &mut adev.psp.securedisplay_context.context;
    if r == 0 && c.resp_status == 0 {
        c.initialized = true;
    } else {
        // don't try again
        c.bin_desc.size_bytes = 0;
        return r;
    }

    // psp_prep_securedisplay_cmd_buf
    let Some(buf) = c.mem_context.shared_buf else { return -22 };
    crate::amdgpu_gart::kptr_fill32(adev, buf, 0, SIZEOF_ta_securedisplay_cmd as u64, 0);
    crate::amdgpu_gart::kptr_w32(adev, buf, SECUREDISPLAY_STATUS as u64, TA_SECUREDISPLAY_STATUS__GENERIC_FAILURE);
    crate::amdgpu_gart::kptr_w32(adev, buf, SECUREDISPLAY_CMD_ID as u64, TA_SECUREDISPLAY_COMMAND__QUERY_TA);

    // psp_securedisplay_invoke
    let r = psp_ta_invoke(adev, TA_SECUREDISPLAY_COMMAND__QUERY_TA, ta_id::securedisplay);
    if r != 0 {
        Line::new().s("SECUREDISPLAY TA initialize fail.").say();
        return -22;
    }
    let status = crate::amdgpu_gart::kptr_r32(adev, buf, SECUREDISPLAY_STATUS as u64);
    if status != TA_SECUREDISPLAY_STATUS__SUCCESS {
        psp_securedisplay_parse_resp_status(status);
        let ret = crate::amdgpu_gart::kptr_r32(adev, buf, SECUREDISPLAY_QUERY_CMD_RET as u64);
        Line::new().s("SECUREDISPLAY: query securedisplay TA failed. ret ").hex(ret).say();
        // don't try again
        adev.psp.securedisplay_context.context.bin_desc.size_bytes = 0;
    }
    0
}

/// `psp_v12_0_ring_stop`, bare metal.
fn psp_v12_0_ring_stop(adev: &mut amdgpu_device) -> i32 {
    // Write the ring destroy command
    adev.wreg32(mp0(adev, MP0_mmMP0_SMN_C2PMSG_64_BASE_IDX, MP0_mmMP0_SMN_C2PMSG_64), GFX_CTRL_CMD_ID_DESTROY_RINGS);
    // Wait for response flag (bit 31)
    let reg = mp0(adev, MP0_mmMP0_SMN_C2PMSG_64_BASE_IDX, MP0_mmMP0_SMN_C2PMSG_64);
    psp_wait_for(adev, reg, MBOX_TOS_RESP_FLAG, MBOX_TOS_RESP_MASK, 0)
}

/// `psp_v12_0_ring_destroy`.
fn psp_v12_0_ring_destroy(adev: &mut amdgpu_device) -> i32 {
    let r = psp_v12_0_ring_stop(adev);
    if r != 0 {
        Line::new().s("Fail to stop psp ring").say();
    }
    let mut rbuf = adev.firmware.rbuf.take();
    crate::amdgpu_object::amdgpu_bo_free_kernel(adev, &mut rbuf);
    adev.psp.km_ring.ring_mem_mc_addr = 0;
    adev.psp.km_ring.ring_mem = None;
    r
}

/// `psp_free_shared_bufs`: the TMR and every TA's shared buffer.
fn psp_free_shared_bufs(adev: &mut amdgpu_device) {
    // free TMR memory buffer
    let mut tmr = adev.psp.tmr_bo.take();
    crate::amdgpu_object::amdgpu_bo_free_kernel(adev, &mut tmr);
    adev.psp.tmr_mc_addr = 0;
    for t in [ta_id::ras, ta_id::hdcp, ta_id::dtm, ta_id::rap, ta_id::securedisplay] {
        // psp_ta_free_shared_buf
        let m = &mut ta(adev, t).mem_context;
        let mut bo = m.shared_bo.take();
        m.shared_mc_addr = 0;
        m.shared_buf = None;
        crate::amdgpu_object::amdgpu_bo_free_kernel(adev, &mut bo);
    }
}

/// `psp_load_fw`, bare metal and not in reset. `psp_rl_load` has no RL
/// image to load. The TA failures are reported and not fatal, as in
/// Linux.
fn psp_load_fw(adev: &mut amdgpu_device) -> i32 {
    if let Some(k) = adev.psp.buf.fence_buf {
        crate::amdgpu_gart::kptr_fill32(adev, k, 0, PSP_FENCE_BUFFER_SIZE as u64, 0);
    }
    let r = psp_ring_init(adev, PSP_RING_TYPE__KM);
    if r != 0 {
        Line::new().s("PSP ring init failed!").say();
        psp_v12_0_ring_destroy(adev);
        return r;
    }
    let r = psp_hw_start(adev);
    if r != 0 {
        psp_v12_0_ring_destroy(adev);
        return r;
    }
    let mut r = psp_load_non_psp_fw(adev);
    if r == 0 {
        r = psp_asd_initialize(adev);
        if r != 0 {
            Line::new().s("PSP load asd failed!").say();
        }
    }
    if r == 0 && adev.psp.rl.size_bytes != 0 {
        Line::new().s("psp_rl_load: an APU's PSP firmware has no RL image").say();
        r = -22;
    }
    if r != 0 {
        psp_free_shared_bufs(adev);
        psp_v12_0_ring_destroy(adev);
        return r;
    }
    if adev.psp.ta_fw.is_some() {
        if psp_ras_initialize(adev) != 0 {
            Line::new().s("RAS: Failed to initialize RAS").say();
        }
        if psp_display_ta_initialize(adev, ta_id::hdcp, "HDCP: optional hdcp ta ucode is not available",
            PSP_HDCP_SHARED_MEM_SIZE) != 0 {
            Line::new().s("HDCP: Failed to initialize HDCP").say();
        }
        if psp_display_ta_initialize(adev, ta_id::dtm, "DTM: optional dtm ta ucode is not available",
            PSP_DTM_SHARED_MEM_SIZE) != 0 {
            Line::new().s("DTM: Failed to initialize DTM").say();
        }
        if psp_rap_initialize(adev) != 0 {
            Line::new().s("RAP: Failed to initialize RAP").say();
        }
        if psp_securedisplay_initialize(adev) != 0 {
            Line::new().s("SECUREDISPLAY: Failed to initialize SECUREDISPLAY").say();
        }
    }
    0
}

/// `psp_hw_init`.
pub fn psp_hw_init(adev: &mut amdgpu_device) -> i32 {
    let r = crate::amdgpu_ucode::amdgpu_ucode_init_bo(adev);
    if r == 0 {
        let r = psp_load_fw(adev);
        if r == 0 {
            return 0;
        }
        Line::new().s("PSP firmware loading failed").say();
    }
    adev.firmware.load_type = AMDGPU_FW_LOAD_DIRECT;
    -22
}

/// `psp_ta_unload`, with `psp_prep_ta_unload_cmd_buf`.
fn psp_ta_unload(adev: &mut amdgpu_device, t: ta_id) -> i32 {
    let mut cmd = acquire_psp_cmd_buf();
    let session_id = ta(adev, t).session_id;
    put32(&mut cmd, CMDRESP_CMD_ID, GFX_CMD_ID_UNLOAD_TA);
    put32(&mut cmd, CMDRESP_CMD + UNLOADTA_SESSION_ID, session_id);
    let fence = adev.psp.buf.fence_buf_mc_addr;
    let ret = psp_cmd_submit_buf(adev, None, &mut cmd, fence);
    ta(adev, t).resp_status = get32(&cmd, CMDRESP_RESP + RESP_STATUS);
    ret
}

/// `psp_ras_terminate`, `psp_securedisplay_terminate`,
/// `psp_rap_terminate`, `psp_dtm_terminate`, `psp_hdcp_terminate`: unload
/// an initialized TA.
fn psp_ta_terminate(adev: &mut amdgpu_device, t: ta_id) -> i32 {
    if !ta(adev, t).initialized {
        return 0;
    }
    let ret = psp_ta_unload(adev, t);
    ta(adev, t).initialized = false;
    ret
}

/// `psp_asd_terminate`.
fn psp_asd_terminate(adev: &mut amdgpu_device) -> i32 {
    if !adev.psp.asd_context.initialized {
        return 0;
    }
    let ret = psp_ta_unload(adev, ta_id::asd);
    if ret == 0 {
        adev.psp.asd_context.initialized = false;
    }
    ret
}

/// `psp_tmr_terminate` (`psp_tmr_unload`), with
/// `psp_prep_tmr_unload_cmd_buf` on bare metal.
fn psp_tmr_terminate(adev: &mut amdgpu_device) -> i32 {
    let mut cmd = acquire_psp_cmd_buf();
    put32(&mut cmd, CMDRESP_CMD_ID, GFX_CMD_ID_DESTROY_TMR);
    Line::new().s("free PSP TMR buffer").dbg();
    let fence = adev.psp.buf.fence_buf_mc_addr;
    psp_cmd_submit_buf(adev, None, &mut cmd, fence)
}

/// `psp_suspend`: no XGMI hive. The TAs, the ASD and the TMR are let go
/// and the ring stopped; its memory stays for the resume.
pub fn psp_suspend(adev: &mut amdgpu_device) -> i32 {
    if adev.psp.ta_fw.is_some() {
        for (t, what) in [(ta_id::ras, "ras"), (ta_id::hdcp, "hdcp"), (ta_id::dtm, "dtm"), (ta_id::rap, "rap"),
                          (ta_id::securedisplay, "securedisplay")] {
            let r = psp_ta_terminate(adev, t);
            if r != 0 {
                Line::new().s("Failed to terminate ").s(what).s(" ta").say();
                return r;
            }
        }
    }
    let r = psp_asd_terminate(adev);
    if r != 0 {
        Line::new().s("Failed to terminate asd").say();
        return r;
    }
    let r = psp_tmr_terminate(adev);
    if r != 0 {
        Line::new().s("Failed to terminate tmr").say();
        return r;
    }
    let r = psp_v12_0_ring_stop(adev);
    if r != 0 {
        Line::new().s("PSP ring stop failed").say();
    }
    r
}

/// `psp_resume`: no memory training, no XGMI hive. As `psp_load_fw`, on
/// the ring the suspend kept.
pub fn psp_resume(adev: &mut amdgpu_device) -> i32 {
    Line::new().s("PSP is resuming...").say();
    let failed = |r: i32| {
        Line::new().s("PSP resume failed").say();
        r
    };
    let r = crate::amdgpu_ucode::amdgpu_ucode_init_bo(adev);
    if r != 0 {
        return failed(r);
    }
    let r = psp_hw_start(adev);
    if r != 0 {
        return failed(r);
    }
    let r = psp_load_non_psp_fw(adev);
    if r != 0 {
        return failed(r);
    }
    let r = psp_asd_initialize(adev);
    if r != 0 {
        Line::new().s("PSP load asd failed!").say();
        return failed(r);
    }
    if adev.psp.rl.size_bytes != 0 {
        Line::new().s("psp_rl_load: an APU's PSP firmware has no RL image").say();
        Line::new().s("PSP load RL failed!").say();
        return failed(-22);
    }
    if adev.psp.ta_fw.is_some() {
        if psp_ras_initialize(adev) != 0 {
            Line::new().s("RAS: Failed to initialize RAS").say();
        }
        if psp_display_ta_initialize(adev, ta_id::hdcp, "HDCP: optional hdcp ta ucode is not available",
            PSP_HDCP_SHARED_MEM_SIZE) != 0 {
            Line::new().s("HDCP: Failed to initialize HDCP").say();
        }
        if psp_display_ta_initialize(adev, ta_id::dtm, "DTM: optional dtm ta ucode is not available",
            PSP_DTM_SHARED_MEM_SIZE) != 0 {
            Line::new().s("DTM: Failed to initialize DTM").say();
        }
        if psp_rap_initialize(adev) != 0 {
            Line::new().s("RAP: Failed to initialize RAP").say();
        }
        if psp_securedisplay_initialize(adev) != 0 {
            Line::new().s("SECUREDISPLAY: Failed to initialize SECUREDISPLAY").say();
        }
    }
    0
}

/// `psp_hw_fini`: no XGMI hive.
pub fn psp_hw_fini(adev: &mut amdgpu_device) -> i32 {
    if adev.psp.ta_fw.is_some() {
        psp_ta_terminate(adev, ta_id::ras);
        psp_ta_terminate(adev, ta_id::securedisplay);
        psp_ta_terminate(adev, ta_id::rap);
        psp_ta_terminate(adev, ta_id::dtm);
        psp_ta_terminate(adev, ta_id::hdcp);
    }
    psp_asd_terminate(adev);
    psp_tmr_terminate(adev);
    psp_v12_0_ring_destroy(adev);
    0
}
