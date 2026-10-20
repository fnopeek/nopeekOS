//! amdgpu/gfx_v9_0.c, amdgpu_gfx.c and amdgpu_rlc.c (Linux 6.18.55): the
//! GFX block's early_init, which requests and reads the CP and RLC
//! microcode. GC 9.3.0 loads no MEC2 image.

use crate::amdgpu_device::amdgpu_device;
use crate::amdgpu_ucode::{amdgpu_ucode_ip_version_decode, amdgpu_ucode_request, fw_source, le16, le32, page_align};
use crate::gmc_v9_0::amdgpu_irq_src;
use crate::linux::{firmware, Line};
use crate::regs::*;
use alloc::vec;
use alloc::vec::Vec;
use alloc::format;
use alloc::string::String;
use crate::amdgpu_ring::{amdgpu_irq_src_id, amdgpu_ring, amdgpu_ring_init, ring_funcs};

/// `struct amdgpu_rlcg_reg_access_ctrl`.
#[derive(Default, Clone, Copy)]
pub struct amdgpu_rlcg_reg_access_ctrl {
    pub scratch_reg0: u32,
    pub scratch_reg1: u32,
    pub scratch_reg2: u32,
    pub scratch_reg3: u32,
    pub grbm_cntl: u32,
    pub grbm_idx: u32,
    pub spare_int: u32,
}

/// `struct amdgpu_rlc`, the part the RLC microcode fills. A pointer into
/// the RLC image is its offset there, `None` while Linux's is NULL.
#[derive(Default)]
pub struct amdgpu_rlc {
    pub funcs: Option<&'static str>,
    pub is_rlc_v2_1: bool,
    pub save_and_restore_offset: u32,
    pub clear_state_descriptor_offset: u32,
    pub avail_scratch_ram_locations: u32,
    pub reg_restore_list_size: u32,
    pub reg_list_format_start: u32,
    pub reg_list_format_separate_start: u32,
    pub starting_offsets_start: u32,
    pub reg_list_format_size_bytes: u32,
    pub reg_list_size_bytes: u32,
    /// `register_list_format`, with `register_restore` after it: one
    /// allocation, as in Linux; `register_restore` starts at index
    /// `register_restore_at`.
    pub register_list_format: Vec<u32>,
    pub register_restore_at: usize,
    pub save_restore_list_cntl_size_bytes: u32,
    pub save_restore_list_cntl: Option<u32>,
    pub save_restore_list_gpm_size_bytes: u32,
    pub save_restore_list_gpm: Option<u32>,
    pub save_restore_list_srm_size_bytes: u32,
    pub save_restore_list_srm: Option<u32>,
    pub reg_list_format_direct_reg_list_length: u32,
    pub rlc_iram_ucode_size_bytes: u32,
    pub rlc_iram_ucode: Option<u32>,
    pub rlc_dram_ucode_size_bytes: u32,
    pub rlc_dram_ucode: Option<u32>,
    pub rlcp_ucode_size_bytes: u32,
    pub rlcp_ucode: Option<u32>,
    pub rlcv_ucode_size_bytes: u32,
    pub rlcv_ucode: Option<u32>,
    pub global_tap_delays_ucode_size_bytes: u32,
    pub global_tap_delays_ucode: Option<u32>,
    pub se0_tap_delays_ucode_size_bytes: u32,
    pub se0_tap_delays_ucode: Option<u32>,
    pub se1_tap_delays_ucode_size_bytes: u32,
    pub se1_tap_delays_ucode: Option<u32>,
    pub se2_tap_delays_ucode_size_bytes: u32,
    pub se2_tap_delays_ucode: Option<u32>,
    pub se3_tap_delays_ucode_size_bytes: u32,
    pub se3_tap_delays_ucode: Option<u32>,
    pub reg_access_ctrl: [amdgpu_rlcg_reg_access_ctrl; 1],
    pub rlcg_reg_access_supported: bool,
    /// `in_safe_mode[0]`.
    pub in_safe_mode: bool,
    /// `cs_data`: gfx9_cs_data once rlc init has set it.
    pub cs_data: bool,
    pub clear_state_size: u32,
    pub clear_state_obj: Option<usize>,
    pub clear_state_gpu_addr: u64,
    pub cs_ptr: Option<crate::amdgpu_object::amdgpu_kptr>,
    pub cp_table_size: u32,
    pub cp_table_obj: Option<usize>,
    pub cp_table_gpu_addr: u64,
    pub cp_table_ptr: Option<crate::amdgpu_object::amdgpu_kptr>,
}

/// `struct amdgpu_mec`.
#[derive(Default)]
pub struct amdgpu_mec {
    pub num_mec: u32,
    pub num_pipe_per_mec: u32,
    pub num_queue_per_pipe: u32,
    pub hpd_eop_obj: Option<usize>,
    pub hpd_eop_gpu_addr: u64,
    pub mec_fw_obj: Option<usize>,
    pub mec_fw_gpu_addr: u64,
    pub mqd_backup: Vec<Vec<u8>>,
}

/// `struct amdgpu_kiq`.
#[derive(Default)]
pub struct amdgpu_kiq {
    pub eop_obj: Option<usize>,
    pub eop_gpu_addr: u64,
    /// Index into `adev.rings`.
    pub ring: Option<usize>,
    pub irq: amdgpu_irq_src,
    pub mqd_backup: Vec<u8>,
}

/// `struct amdgpu_gds`.
#[derive(Default)]
pub struct amdgpu_gds {
    pub gds_size: u32,
    pub gds_compute_max_wave_id: u32,
    pub gws_size: u32,
    pub oa_size: u32,
}


/// `amdgpu_gfx_get_num_kcq`.
fn amdgpu_gfx_get_num_kcq() -> u32 {
    let n = crate::tables::amdgpu_num_kcq[0];
    if n == -1 {
        8
    } else if !(0..=8).contains(&n) {
        Line::new().s("set kernel compute queue number to 8 due to invalid parameter provided by user").say();
        8
    } else {
        n as u32
    }
}

/// The image a `fw_source` names.
pub fn fw_of(adev: &amdgpu_device, s: fw_source) -> Option<&firmware> {
    let g = &adev.gfx;
    match s {
        fw_source::gfx_pfp => g.pfp_fw.as_ref(),
        fw_source::gfx_me => g.me_fw.as_ref(),
        fw_source::gfx_ce => g.ce_fw.as_ref(),
        fw_source::gfx_mec => g.mec_fw.as_ref(),
        fw_source::gfx_mec2 => g.mec2_fw.as_ref(),
        fw_source::gfx_rlc => g.rlc_fw.as_ref(),
        fw_source::sdma(i) => adev.sdma.instance.get(i as usize).and_then(|x| x.fw.as_ref()),
        fw_source::dm_dmub => adev.dm_dmub_fw.as_ref(),
        fw_source::vcn(i) => adev.vcn.inst.get(i as usize).and_then(|v| v.fw.as_ref()),
    }
}

/// `amdgpu_gfx_cp_init_microcode`, the v1 header ids gfx_v9_0 passes.
fn amdgpu_gfx_cp_init_microcode(adev: &mut amdgpu_device, ucode_id: u32) {
    let (src, versions) = match ucode_id {
        AMDGPU_UCODE_ID_CP_PFP => (fw_source::gfx_pfp, true),
        AMDGPU_UCODE_ID_CP_ME => (fw_source::gfx_me, true),
        AMDGPU_UCODE_ID_CP_CE => (fw_source::gfx_ce, true),
        AMDGPU_UCODE_ID_CP_MEC1 => (fw_source::gfx_mec, true),
        AMDGPU_UCODE_ID_CP_MEC1_JT => (fw_source::gfx_mec, false),
        AMDGPU_UCODE_ID_CP_MEC2 => (fw_source::gfx_mec2, true),
        AMDGPU_UCODE_ID_CP_MEC2_JT => (fw_source::gfx_mec2, false),
        _ => {
            Line::new().s("Invalid ucode id ").dec(ucode_id as u64).say();
            return;
        }
    };
    let Some(fw) = fw_of(adev, src) else { return };
    let ucode_version = le32(fw, COMMON_FIRMWARE_HEADER_ucode_version);
    let feature = le32(fw, GFX_FIRMWARE_HEADER_V1_0_ucode_feature_version);
    let size = le32(fw, COMMON_FIRMWARE_HEADER_ucode_size_bytes);
    let jt = le32(fw, GFX_FIRMWARE_HEADER_V1_0_jt_size).wrapping_mul(4);
    let fw_size = match ucode_id {
        AMDGPU_UCODE_ID_CP_MEC1 | AMDGPU_UCODE_ID_CP_MEC2 => size.wrapping_sub(jt),
        AMDGPU_UCODE_ID_CP_MEC1_JT | AMDGPU_UCODE_ID_CP_MEC2_JT => jt,
        _ => size,
    };
    if versions {
        let g = &mut adev.gfx;
        let (v, f) = match ucode_id {
            AMDGPU_UCODE_ID_CP_PFP => (&mut g.pfp_fw_version, &mut g.pfp_feature_version),
            AMDGPU_UCODE_ID_CP_ME => (&mut g.me_fw_version, &mut g.me_feature_version),
            AMDGPU_UCODE_ID_CP_CE => (&mut g.ce_fw_version, &mut g.ce_feature_version),
            AMDGPU_UCODE_ID_CP_MEC1 => (&mut g.mec_fw_version, &mut g.mec_feature_version),
            _ => (&mut g.mec2_fw_version, &mut g.mec2_feature_version),
        };
        *v = ucode_version;
        *f = feature;
    }
    if adev.firmware.load_type == AMDGPU_FW_LOAD_PSP {
        let info = &mut adev.firmware.ucode[ucode_id as usize];
        info.ucode_id = ucode_id;
        info.fw = Some(src);
        adev.firmware.fw_size = adev.firmware.fw_size.wrapping_add(page_align(fw_size));
    }
}

/// Registers one RLC sub-image for PSP loading.
fn rlc_ucode(adev: &mut amdgpu_device, id: u32, size: u32) {
    if size != 0 {
        let info = &mut adev.firmware.ucode[id as usize];
        info.ucode_id = id;
        info.fw = Some(fw_source::gfx_rlc);
        adev.firmware.fw_size = adev.firmware.fw_size.wrapping_add(page_align(size));
    }
}

/// `amdgpu_gfx_rlc_init_microcode_v2_0`.
fn amdgpu_gfx_rlc_init_microcode_v2_0(adev: &mut amdgpu_device) -> i32 {
    let Some(fw) = adev.gfx.rlc_fw.take() else { return -22 };
    let f = |o: u32| le32(&fw, o);
    let g = &mut adev.gfx;
    g.rlc_fw_version = f(COMMON_FIRMWARE_HEADER_ucode_version);
    g.rlc_feature_version = f(RLC_FIRMWARE_HEADER_V2_0_ucode_feature_version);
    let r = &mut g.rlc;
    r.save_and_restore_offset = f(RLC_FIRMWARE_HEADER_V2_0_save_and_restore_offset);
    r.clear_state_descriptor_offset = f(RLC_FIRMWARE_HEADER_V2_0_clear_state_descriptor_offset);
    r.avail_scratch_ram_locations = f(RLC_FIRMWARE_HEADER_V2_0_avail_scratch_ram_locations);
    r.reg_restore_list_size = f(RLC_FIRMWARE_HEADER_V2_0_reg_restore_list_size);
    r.reg_list_format_start = f(RLC_FIRMWARE_HEADER_V2_0_reg_list_format_start);
    r.reg_list_format_separate_start = f(RLC_FIRMWARE_HEADER_V2_0_reg_list_format_separate_start);
    r.starting_offsets_start = f(RLC_FIRMWARE_HEADER_V2_0_starting_offsets_start);
    r.reg_list_format_size_bytes = f(RLC_FIRMWARE_HEADER_V2_0_reg_list_format_size_bytes);
    r.reg_list_size_bytes = f(RLC_FIRMWARE_HEADER_V2_0_reg_list_size_bytes);
    let n_format = (r.reg_list_format_size_bytes >> 2) as usize;
    let n_restore = (r.reg_list_size_bytes >> 2) as usize;
    let mut list = alloc::vec![0u32; n_format + n_restore];
    let format_at = f(RLC_FIRMWARE_HEADER_V2_0_reg_list_format_array_offset_bytes);
    for (i, v) in list[..n_format].iter_mut().enumerate() {
        *v = f(format_at.wrapping_add(4 * i as u32));
    }
    let restore_at = f(RLC_FIRMWARE_HEADER_V2_0_reg_list_array_offset_bytes);
    for (i, v) in list[n_format..].iter_mut().enumerate() {
        *v = f(restore_at.wrapping_add(4 * i as u32));
    }
    let size = f(COMMON_FIRMWARE_HEADER_ucode_size_bytes);
    r.register_list_format = list;
    r.register_restore_at = n_format;
    adev.gfx.rlc_fw = Some(fw);

    if adev.firmware.load_type == AMDGPU_FW_LOAD_PSP {
        let info = &mut adev.firmware.ucode[AMDGPU_UCODE_ID_RLC_G as usize];
        info.ucode_id = AMDGPU_UCODE_ID_RLC_G;
        info.fw = Some(fw_source::gfx_rlc);
        adev.firmware.fw_size = adev.firmware.fw_size.wrapping_add(page_align(size));
    }
    0
}

/// `amdgpu_gfx_rlc_init_microcode_v2_1`.
fn amdgpu_gfx_rlc_init_microcode_v2_1(adev: &mut amdgpu_device) {
    let Some(fw) = adev.gfx.rlc_fw.take() else { return };
    let f = |o: u32| le32(&fw, o);
    let g = &mut adev.gfx;
    g.rlc_srlc_fw_version = f(RLC_FIRMWARE_HEADER_V2_1_save_restore_list_cntl_ucode_ver);
    g.rlc_srlc_feature_version = f(RLC_FIRMWARE_HEADER_V2_1_save_restore_list_cntl_feature_ver);
    g.rlc.save_restore_list_cntl_size_bytes = f(RLC_FIRMWARE_HEADER_V2_1_save_restore_list_cntl_size_bytes);
    g.rlc.save_restore_list_cntl = Some(f(RLC_FIRMWARE_HEADER_V2_1_save_restore_list_cntl_offset_bytes));
    g.rlc_srlg_fw_version = f(RLC_FIRMWARE_HEADER_V2_1_save_restore_list_gpm_ucode_ver);
    g.rlc_srlg_feature_version = f(RLC_FIRMWARE_HEADER_V2_1_save_restore_list_gpm_feature_ver);
    g.rlc.save_restore_list_gpm_size_bytes = f(RLC_FIRMWARE_HEADER_V2_1_save_restore_list_gpm_size_bytes);
    g.rlc.save_restore_list_gpm = Some(f(RLC_FIRMWARE_HEADER_V2_1_save_restore_list_gpm_offset_bytes));
    g.rlc_srls_fw_version = f(RLC_FIRMWARE_HEADER_V2_1_save_restore_list_srm_ucode_ver);
    g.rlc_srls_feature_version = f(RLC_FIRMWARE_HEADER_V2_1_save_restore_list_srm_feature_ver);
    g.rlc.save_restore_list_srm_size_bytes = f(RLC_FIRMWARE_HEADER_V2_1_save_restore_list_srm_size_bytes);
    g.rlc.save_restore_list_srm = Some(f(RLC_FIRMWARE_HEADER_V2_1_save_restore_list_srm_offset_bytes));
    g.rlc.reg_list_format_direct_reg_list_length = f(RLC_FIRMWARE_HEADER_V2_1_reg_list_format_direct_reg_list_length);

    adev.gfx.rlc_fw = Some(fw);
    if adev.firmware.load_type == AMDGPU_FW_LOAD_PSP {
        let r = &adev.gfx.rlc;
        let sizes = [
            (AMDGPU_UCODE_ID_RLC_RESTORE_LIST_CNTL, r.save_restore_list_cntl_size_bytes),
            (AMDGPU_UCODE_ID_RLC_RESTORE_LIST_GPM_MEM, r.save_restore_list_gpm_size_bytes),
            (AMDGPU_UCODE_ID_RLC_RESTORE_LIST_SRM_MEM, r.save_restore_list_srm_size_bytes),
        ];
        for (id, size) in sizes {
            rlc_ucode(adev, id, size);
        }
    }
}

/// `amdgpu_gfx_rlc_init_microcode_v2_2`.
fn amdgpu_gfx_rlc_init_microcode_v2_2(adev: &mut amdgpu_device) {
    let Some(fw) = adev.gfx.rlc_fw.take() else { return };
    let f = |o: u32| le32(&fw, o);
    let r = &mut adev.gfx.rlc;
    r.rlc_iram_ucode_size_bytes = f(RLC_FIRMWARE_HEADER_V2_2_rlc_iram_ucode_size_bytes);
    r.rlc_iram_ucode = Some(f(RLC_FIRMWARE_HEADER_V2_2_rlc_iram_ucode_offset_bytes));
    r.rlc_dram_ucode_size_bytes = f(RLC_FIRMWARE_HEADER_V2_2_rlc_dram_ucode_size_bytes);
    r.rlc_dram_ucode = Some(f(RLC_FIRMWARE_HEADER_V2_2_rlc_dram_ucode_offset_bytes));

    adev.gfx.rlc_fw = Some(fw);
    if adev.firmware.load_type == AMDGPU_FW_LOAD_PSP {
        let sizes = [(AMDGPU_UCODE_ID_RLC_IRAM, adev.gfx.rlc.rlc_iram_ucode_size_bytes),
                     (AMDGPU_UCODE_ID_RLC_DRAM, adev.gfx.rlc.rlc_dram_ucode_size_bytes)];
        for (id, size) in sizes {
            rlc_ucode(adev, id, size);
        }
    }
}

/// `amdgpu_gfx_rlc_init_microcode_v2_3`.
fn amdgpu_gfx_rlc_init_microcode_v2_3(adev: &mut amdgpu_device) {
    let Some(fw) = adev.gfx.rlc_fw.take() else { return };
    let f = |o: u32| le32(&fw, o);
    let g = &mut adev.gfx;
    g.rlcp_ucode_version = f(RLC_FIRMWARE_HEADER_V2_3_rlcp_ucode_version);
    g.rlcp_ucode_feature_version = f(RLC_FIRMWARE_HEADER_V2_3_rlcp_ucode_feature_version);
    g.rlc.rlcp_ucode_size_bytes = f(RLC_FIRMWARE_HEADER_V2_3_rlcp_ucode_size_bytes);
    g.rlc.rlcp_ucode = Some(f(RLC_FIRMWARE_HEADER_V2_3_rlcp_ucode_offset_bytes));
    g.rlcv_ucode_version = f(RLC_FIRMWARE_HEADER_V2_3_rlcv_ucode_version);
    g.rlcv_ucode_feature_version = f(RLC_FIRMWARE_HEADER_V2_3_rlcv_ucode_feature_version);
    g.rlc.rlcv_ucode_size_bytes = f(RLC_FIRMWARE_HEADER_V2_3_rlcv_ucode_size_bytes);
    g.rlc.rlcv_ucode = Some(f(RLC_FIRMWARE_HEADER_V2_3_rlcv_ucode_offset_bytes));

    adev.gfx.rlc_fw = Some(fw);
    if adev.firmware.load_type == AMDGPU_FW_LOAD_PSP {
        let sizes = [(AMDGPU_UCODE_ID_RLC_P, adev.gfx.rlc.rlcp_ucode_size_bytes),
                     (AMDGPU_UCODE_ID_RLC_V, adev.gfx.rlc.rlcv_ucode_size_bytes)];
        for (id, size) in sizes {
            rlc_ucode(adev, id, size);
        }
    }
}

/// `amdgpu_gfx_rlc_init_microcode_v2_4`.
fn amdgpu_gfx_rlc_init_microcode_v2_4(adev: &mut amdgpu_device) {
    let Some(fw) = adev.gfx.rlc_fw.take() else { return };
    let f = |o: u32| le32(&fw, o);
    let r = &mut adev.gfx.rlc;
    r.global_tap_delays_ucode_size_bytes = f(RLC_FIRMWARE_HEADER_V2_4_global_tap_delays_ucode_size_bytes);
    r.global_tap_delays_ucode = Some(f(RLC_FIRMWARE_HEADER_V2_4_global_tap_delays_ucode_offset_bytes));
    r.se0_tap_delays_ucode_size_bytes = f(RLC_FIRMWARE_HEADER_V2_4_se0_tap_delays_ucode_size_bytes);
    r.se0_tap_delays_ucode = Some(f(RLC_FIRMWARE_HEADER_V2_4_se0_tap_delays_ucode_offset_bytes));
    r.se1_tap_delays_ucode_size_bytes = f(RLC_FIRMWARE_HEADER_V2_4_se1_tap_delays_ucode_size_bytes);
    r.se1_tap_delays_ucode = Some(f(RLC_FIRMWARE_HEADER_V2_4_se1_tap_delays_ucode_offset_bytes));
    r.se2_tap_delays_ucode_size_bytes = f(RLC_FIRMWARE_HEADER_V2_4_se2_tap_delays_ucode_size_bytes);
    r.se2_tap_delays_ucode = Some(f(RLC_FIRMWARE_HEADER_V2_4_se2_tap_delays_ucode_offset_bytes));
    r.se3_tap_delays_ucode_size_bytes = f(RLC_FIRMWARE_HEADER_V2_4_se3_tap_delays_ucode_size_bytes);
    r.se3_tap_delays_ucode = Some(f(RLC_FIRMWARE_HEADER_V2_4_se3_tap_delays_ucode_offset_bytes));

    adev.gfx.rlc_fw = Some(fw);
    if adev.firmware.load_type == AMDGPU_FW_LOAD_PSP {
        let sizes = [
            (AMDGPU_UCODE_ID_GLOBAL_TAP_DELAYS, adev.gfx.rlc.global_tap_delays_ucode_size_bytes),
            (AMDGPU_UCODE_ID_SE0_TAP_DELAYS, adev.gfx.rlc.se0_tap_delays_ucode_size_bytes),
            (AMDGPU_UCODE_ID_SE1_TAP_DELAYS, adev.gfx.rlc.se1_tap_delays_ucode_size_bytes),
            (AMDGPU_UCODE_ID_SE2_TAP_DELAYS, adev.gfx.rlc.se2_tap_delays_ucode_size_bytes),
            (AMDGPU_UCODE_ID_SE3_TAP_DELAYS, adev.gfx.rlc.se3_tap_delays_ucode_size_bytes),
        ];
        for (id, size) in sizes {
            rlc_ucode(adev, id, size);
        }
    }
}

/// `amdgpu_gfx_rlc_init_microcode`.
fn amdgpu_gfx_rlc_init_microcode(adev: &mut amdgpu_device, version_major: u16, version_minor: u16) -> i32 {
    if version_major < 2 {
        // only support rlc_hdr v2.x and onwards
        Line::new().s("unsupported rlc fw hdr").say();
        return -22;
    }
    // is_rlc_v2_1 is still used in APU code path
    if version_major == 2 && version_minor == 1 {
        adev.gfx.rlc.is_rlc_v2_1 = true;
    }
    let err = amdgpu_gfx_rlc_init_microcode_v2_0(adev);
    if err != 0 {
        Line::new().s("fail to init rlc v2_0 microcode").say();
        return err;
    }
    if version_minor >= 1 {
        amdgpu_gfx_rlc_init_microcode_v2_1(adev);
    }
    if version_minor >= 2 {
        amdgpu_gfx_rlc_init_microcode_v2_2(adev);
    }
    if version_minor == 3 {
        amdgpu_gfx_rlc_init_microcode_v2_3(adev);
    }
    if version_minor == 4 {
        amdgpu_gfx_rlc_init_microcode_v2_4(adev);
    }
    0
}

/// `gfx_v9_0_init_cp_gfx_microcode`.
fn gfx_v9_0_init_cp_gfx_microcode(adev: &mut amdgpu_device, chip_name: &str) -> i32 {
    let mut err;
    'out: {
        let (fw, e) = amdgpu_ucode_request(&alloc::format!("amdgpu/{}_pfp.bin", chip_name));
        adev.gfx.pfp_fw = fw;
        err = e;
        if err != 0 {
            break 'out;
        }
        amdgpu_gfx_cp_init_microcode(adev, AMDGPU_UCODE_ID_CP_PFP);

        let (fw, e) = amdgpu_ucode_request(&alloc::format!("amdgpu/{}_me.bin", chip_name));
        adev.gfx.me_fw = fw;
        err = e;
        if err != 0 {
            break 'out;
        }
        amdgpu_gfx_cp_init_microcode(adev, AMDGPU_UCODE_ID_CP_ME);

        let (fw, e) = amdgpu_ucode_request(&alloc::format!("amdgpu/{}_ce.bin", chip_name));
        adev.gfx.ce_fw = fw;
        err = e;
        if err != 0 {
            break 'out;
        }
        amdgpu_gfx_cp_init_microcode(adev, AMDGPU_UCODE_ID_CP_CE);
    }
    if err != 0 {
        adev.gfx.pfp_fw = None;
        adev.gfx.me_fw = None;
        adev.gfx.ce_fw = None;
    }
    err
}

/// `gfx_v9_0_init_rlc_microcode`: "renoir", so neither the Picasso AM4
/// nor the Raven kicker image.
fn gfx_v9_0_init_rlc_microcode(adev: &mut amdgpu_device, chip_name: &str) -> i32 {
    let (fw, mut err) = amdgpu_ucode_request(&alloc::format!("amdgpu/{}_rlc.bin", chip_name));
    adev.gfx.rlc_fw = fw;
    if err == 0 {
        let (major, minor) = adev.gfx.rlc_fw.as_ref().map_or((0, 0), |f| {
            (le16(f, COMMON_FIRMWARE_HEADER_header_version_major), le16(f, COMMON_FIRMWARE_HEADER_header_version_minor))
        });
        err = amdgpu_gfx_rlc_init_microcode(adev, major, minor);
    }
    if err != 0 {
        adev.gfx.rlc_fw = None;
    }
    err
}

/// `gfx_v9_0_should_disable_gfxoff`.
fn gfx_v9_0_should_disable_gfxoff(adev: &amdgpu_device) -> bool {
    let list = [
        (GFXOFF_QUIRK_0_chip_vendor, GFXOFF_QUIRK_0_chip_device, GFXOFF_QUIRK_0_subsys_vendor, GFXOFF_QUIRK_0_subsys_device, GFXOFF_QUIRK_0_revision),
        (GFXOFF_QUIRK_1_chip_vendor, GFXOFF_QUIRK_1_chip_device, GFXOFF_QUIRK_1_subsys_vendor, GFXOFF_QUIRK_1_subsys_device, GFXOFF_QUIRK_1_revision),
        (GFXOFF_QUIRK_2_chip_vendor, GFXOFF_QUIRK_2_chip_device, GFXOFF_QUIRK_2_subsys_vendor, GFXOFF_QUIRK_2_subsys_device, GFXOFF_QUIRK_2_revision),
        (GFXOFF_QUIRK_3_chip_vendor, GFXOFF_QUIRK_3_chip_device, GFXOFF_QUIRK_3_subsys_vendor, GFXOFF_QUIRK_3_subsys_device, GFXOFF_QUIRK_3_revision),
        (GFXOFF_QUIRK_4_chip_vendor, GFXOFF_QUIRK_4_chip_device, GFXOFF_QUIRK_4_subsys_vendor, GFXOFF_QUIRK_4_subsys_device, GFXOFF_QUIRK_4_revision),
        (GFXOFF_QUIRK_5_chip_vendor, GFXOFF_QUIRK_5_chip_device, GFXOFF_QUIRK_5_subsys_vendor, GFXOFF_QUIRK_5_subsys_device, GFXOFF_QUIRK_5_revision),
        (GFXOFF_QUIRK_6_chip_vendor, GFXOFF_QUIRK_6_chip_device, GFXOFF_QUIRK_6_subsys_vendor, GFXOFF_QUIRK_6_subsys_device, GFXOFF_QUIRK_6_revision),
    ];
    for &(vendor, device, sv, sd, rev) in &list[..GFXOFF_QUIRK_COUNT as usize] {
        if device == 0 {
            break;
        }
        if 0x1002 == vendor && adev.device as u32 == device && adev.subsystem_vendor as u32 == sv
            && adev.subsystem_device as u32 == sd && adev.revision as u32 == rev
        {
            return true;
        }
    }
    false
}

/// `gfx_v9_0_check_if_need_gfxoff`, GC 9.3.0.
fn gfx_v9_0_check_if_need_gfxoff(adev: &mut amdgpu_device) {
    if gfx_v9_0_should_disable_gfxoff(adev) {
        adev.pm.pp_feature &= !PP_GFXOFF_MASK;
    }
    if adev.pm.pp_feature & PP_GFXOFF_MASK != 0 {
        adev.pg_flags |= AMD_PG_SUPPORT_GFX_PG | AMD_PG_SUPPORT_CP | AMD_PG_SUPPORT_RLC_SMU_HS;
    }
}

/// `gfx_v9_0_check_fw_write_wait`, GC 9.3.0: the default case.
fn gfx_v9_0_check_fw_write_wait(adev: &mut amdgpu_device) {
    let g = &mut adev.gfx;
    g.me_fw_write_wait = false;
    g.mec_fw_write_wait = false;
    if g.mec_fw_version < 0x000001a5 || g.mec_feature_version < 46 || g.pfp_fw_version < 0x000000b7
        || g.pfp_feature_version < 46
    {
        Line::new().s("CP firmware version too old, please update!").say();
    }
    g.me_fw_write_wait = true;
    g.mec_fw_write_wait = true;
}

/// `gfx_v9_0_init_cp_compute_microcode`: bare metal, no MEC2 image for
/// GC 9.3.0 (`gfx_v9_0_load_mec2_fw_bin_support`).
fn gfx_v9_0_init_cp_compute_microcode(adev: &mut amdgpu_device, chip_name: &str) -> i32 {
    let (fw, err) = amdgpu_ucode_request(&alloc::format!("amdgpu/{}_mec.bin", chip_name));
    adev.gfx.mec_fw = fw;
    if err != 0 {
        adev.gfx.mec_fw = None;
        return err;
    }
    amdgpu_gfx_cp_init_microcode(adev, AMDGPU_UCODE_ID_CP_MEC1);
    amdgpu_gfx_cp_init_microcode(adev, AMDGPU_UCODE_ID_CP_MEC1_JT);

    adev.gfx.mec2_fw_version = adev.gfx.mec_fw_version;
    adev.gfx.mec2_feature_version = adev.gfx.mec_feature_version;

    gfx_v9_0_check_if_need_gfxoff(adev);
    gfx_v9_0_check_fw_write_wait(adev);
    0
}

/// `gfx_v9_0_init_microcode`.
pub fn gfx_v9_0_init_microcode(adev: &mut amdgpu_device) -> i32 {
    let ucode_prefix = amdgpu_ucode_ip_version_decode(adev, GC_HWIP);

    // No CPG in Arcturus
    if adev.gfx.num_gfx_rings != 0 {
        let r = gfx_v9_0_init_cp_gfx_microcode(adev, &ucode_prefix);
        if r != 0 {
            return r;
        }
    }
    let r = gfx_v9_0_init_rlc_microcode(adev, &ucode_prefix);
    if r != 0 {
        return r;
    }
    gfx_v9_0_init_cp_compute_microcode(adev, &ucode_prefix)
}

/// `SOC15_REG_OFFSET(GC, 0, reg)`.
fn gc_reg(adev: &amdgpu_device, reg: u32, base_idx: u32) -> u32 {
    adev.reg_offset[GC_HWIP as usize][0].get(base_idx as usize).copied().unwrap_or(0) + reg
}

/// `gfx_v9_0_early_init`.
pub fn gfx_v9_0_early_init(adev: &mut amdgpu_device) -> i32 {
    adev.gfx.funcs = Some("gfx_v9_0_gfx_funcs");
    adev.gfx.num_gfx_rings = GFX9_NUM_GFX_RINGS;
    adev.gfx.xcc_mask = 1;
    adev.gfx.num_compute_rings = amdgpu_gfx_get_num_kcq().min(AMDGPU_MAX_COMPUTE_RINGS);

    // gfx_v9_0_set_kiq_pm4_funcs
    adev.gfx.kiq0_pmf = Some("gfx_v9_0_kiq_pm4_funcs");

    // gfx_v9_0_set_ring_funcs
    let g = &mut adev.gfx;
    g.kiq0_ring_funcs = Some("gfx_v9_0_ring_funcs_kiq");
    for i in 0..g.num_gfx_rings as usize {
        g.gfx_ring_funcs[i] = Some("gfx_v9_0_ring_funcs_gfx");
    }
    if g.mcbp && g.num_gfx_rings != 0 {
        for f in g.sw_gfx_ring_funcs.iter_mut() {
            *f = Some("gfx_v9_0_sw_ring_funcs_gfx");
        }
    }
    for i in 0..g.num_compute_rings as usize {
        g.compute_ring_funcs[i] = Some("gfx_v9_0_ring_funcs_compute");
    }

    // gfx_v9_0_set_irq_funcs
    g.eop_irq = amdgpu_irq_src { num_types: AMDGPU_CP_IRQ_LAST, funcs: Some("gfx_v9_0_eop_irq_funcs") };
    g.priv_reg_irq = amdgpu_irq_src { num_types: 1, funcs: Some("gfx_v9_0_priv_reg_irq_funcs") };
    g.bad_op_irq = amdgpu_irq_src { num_types: 1, funcs: Some("gfx_v9_0_bad_op_irq_funcs") };
    g.priv_inst_irq = amdgpu_irq_src { num_types: 1, funcs: Some("gfx_v9_0_priv_inst_irq_funcs") };
    // C5 ECC error and C9 FUE error
    g.cp_ecc_error_irq = amdgpu_irq_src { num_types: 2, funcs: Some("gfx_v9_0_cp_ecc_error_irq_funcs") };

    // gfx_v9_0_set_gds_init: GC 9.3.0, the default cases
    adev.gds.gds_size = 0x10000;
    // this really depends on the chip
    adev.gds.gds_compute_max_wave_id = 0x7ff;
    adev.gds.gws_size = 64;
    adev.gds.oa_size = 16;

    // gfx_v9_0_set_rlc_funcs
    adev.gfx.rlc.funcs = Some("gfx_v9_0_rlc_funcs");

    // init rlcg reg access ctrl: gfx_v9_0_init_rlcg_reg_access_ctrl
    let ctrl = amdgpu_rlcg_reg_access_ctrl {
        scratch_reg0: gc_reg(adev, mmSCRATCH_REG0, mmSCRATCH_REG0_BASE_IDX),
        scratch_reg1: gc_reg(adev, mmSCRATCH_REG1, mmSCRATCH_REG1_BASE_IDX),
        scratch_reg2: gc_reg(adev, mmSCRATCH_REG2, mmSCRATCH_REG2_BASE_IDX),
        scratch_reg3: gc_reg(adev, mmSCRATCH_REG3, mmSCRATCH_REG3_BASE_IDX),
        grbm_cntl: gc_reg(adev, mmGRBM_GFX_CNTL, mmGRBM_GFX_CNTL_BASE_IDX),
        grbm_idx: gc_reg(adev, mmGRBM_GFX_INDEX, mmGRBM_GFX_INDEX_BASE_IDX),
        spare_int: gc_reg(adev, mmRLC_SPARE_INT, mmRLC_SPARE_INT_BASE_IDX),
    };
    adev.gfx.rlc.reg_access_ctrl[0] = ctrl;
    adev.gfx.rlc.rlcg_reg_access_supported = true;

    gfx_v9_0_init_microcode(adev)
}

/// `gfx9_cs_data`: one SECT_CONTEXT section of eight extents, each
/// `(extent, reg_index, reg_count)`.
pub static GFX9_SECT_CONTEXT_DEFS: [(&[u32], u32, u32); 8] = [
    (&crate::tables::gfx9_SECT_CONTEXT_def_1, CS_EXTENT_0_REG_INDEX, CS_EXTENT_0_REG_COUNT),
    (&crate::tables::gfx9_SECT_CONTEXT_def_2, CS_EXTENT_1_REG_INDEX, CS_EXTENT_1_REG_COUNT),
    (&crate::tables::gfx9_SECT_CONTEXT_def_3, CS_EXTENT_2_REG_INDEX, CS_EXTENT_2_REG_COUNT),
    (&crate::tables::gfx9_SECT_CONTEXT_def_4, CS_EXTENT_3_REG_INDEX, CS_EXTENT_3_REG_COUNT),
    (&crate::tables::gfx9_SECT_CONTEXT_def_5, CS_EXTENT_4_REG_INDEX, CS_EXTENT_4_REG_COUNT),
    (&crate::tables::gfx9_SECT_CONTEXT_def_6, CS_EXTENT_5_REG_INDEX, CS_EXTENT_5_REG_COUNT),
    (&crate::tables::gfx9_SECT_CONTEXT_def_7, CS_EXTENT_6_REG_INDEX, CS_EXTENT_6_REG_COUNT),
    (&crate::tables::gfx9_SECT_CONTEXT_def_8, CS_EXTENT_7_REG_INDEX, CS_EXTENT_7_REG_COUNT),
];
// The shape the table above writes out, as GCC read it from
// clearstate_gfx9.h.
const _: () = assert!(CS_SECTIONS == 2 && CS_SECTION_0_IS_CONTEXT_DEFS == 1 && CS_SECTION_1_END == 1
    && CS_SECTION_0_ID == SECT_CONTEXT && CS_EXTENTS == 9 && CS_EXTENT_8_END == 1
    && CS_EXTENT_0_IS_DEF_1 == 1 && CS_EXTENT_1_IS_DEF_2 == 1 && CS_EXTENT_2_IS_DEF_3 == 1
    && CS_EXTENT_3_IS_DEF_4 == 1 && CS_EXTENT_4_IS_DEF_5 == 1 && CS_EXTENT_5_IS_DEF_6 == 1
    && CS_EXTENT_6_IS_DEF_7 == 1 && CS_EXTENT_7_IS_DEF_8 == 1);

/// `gfx_v9_0_get_csb_size`.
fn gfx_v9_0_get_csb_size() -> u32 {
    // begin clear state
    let mut count = 2;
    // context control state
    count += 3;
    for (_, _, reg_count) in GFX9_SECT_CONTEXT_DEFS.iter() {
        count += 2 + reg_count;
    }
    // end clear state
    count += 2;
    // clear state
    count += 2;
    count
}

fn create_kernel(adev: &mut amdgpu_device, size: u64, domain: u32)
                 -> Result<(usize, u64, Option<crate::amdgpu_object::amdgpu_kptr>), i32> {
    match crate::amdgpu_object::amdgpu_bo_create_kernel(adev, size, crate::amdgpu_object::PAGE_SIZE, domain, true) {
        Ok(Some(h)) => Ok((h, crate::amdgpu_object::amdgpu_bo_gpu_offset(adev, h), crate::amdgpu_object::bo(adev, h).kptr)),
        Ok(None) => Err(-12),
        Err(r) => Err(r),
    }
}

/// `amdgpu_gfx_rlc_init_csb`.
fn amdgpu_gfx_rlc_init_csb(adev: &mut amdgpu_device) -> i32 {
    // allocate clear state block
    let dws = gfx_v9_0_get_csb_size();
    adev.gfx.rlc.clear_state_size = dws;
    match create_kernel(adev, dws as u64 * 4, AMDGPU_GEM_DOMAIN_VRAM | AMDGPU_GEM_DOMAIN_GTT) {
        Ok((h, a, p)) => {
            let rlc = &mut adev.gfx.rlc;
            (rlc.clear_state_obj, rlc.clear_state_gpu_addr, rlc.cs_ptr) = (Some(h), a, p);
            0
        }
        Err(r) => {
            Line::new().s("(").dec((-r) as u64).s(") failed to create rlc csb bo").say();
            r
        }
    }
}

/// The firmware a cp table entry reads its jump table from:
/// CE, PFP, ME, MEC, MEC2.
fn cp_table_fw(adev: &amdgpu_device, me: u32) -> Option<&firmware> {
    let g = &adev.gfx;
    match me {
        0 => g.ce_fw.as_ref(),
        1 => g.pfp_fw.as_ref(),
        2 => g.me_fw.as_ref(),
        3 => g.mec_fw.as_ref(),
        _ => g.mec2_fw.as_ref(),
    }
}

/// `amdgpu_gfx_rlc_setup_cp_table`. `gfx_v9_0_cp_jump_table_num` is 4:
/// GC 9.3.0 has no MEC2 image.
fn amdgpu_gfx_rlc_setup_cp_table(adev: &mut amdgpu_device) {
    let max_me = 4;
    let Some(dst) = adev.gfx.rlc.cp_table_ptr else { return };
    let mut bo_offset = 0u32;
    for me in 0..max_me {
        let Some(fw) = cp_table_fw(adev, me) else { continue };
        let fw_data = le32(fw, UCODE_HDR_UCODE_ARRAY_OFFSET_BYTES) as usize;
        let table_offset = le32(fw, GFX_HDR_JT_OFFSET);
        let table_size = le32(fw, GFX_HDR_JT_SIZE);
        let d = &fw.data;
        let at = fw_data + table_offset as usize * 4;
        let bytes = d.get(at..at + table_size as usize * 4).unwrap_or(&[]).to_vec();
        crate::amdgpu_gart::kptr_write(adev, dst, bo_offset as u64 * 4, &bytes);
        bo_offset += table_size;
    }
}

/// `amdgpu_gfx_rlc_init_cpt`.
fn amdgpu_gfx_rlc_init_cpt(adev: &mut amdgpu_device) -> i32 {
    match create_kernel(adev, adev.gfx.rlc.cp_table_size as u64, AMDGPU_GEM_DOMAIN_VRAM | AMDGPU_GEM_DOMAIN_GTT) {
        Ok((h, a, p)) => {
            let rlc = &mut adev.gfx.rlc;
            (rlc.cp_table_obj, rlc.cp_table_gpu_addr, rlc.cp_table_ptr) = (Some(h), a, p);
        }
        Err(r) => {
            Line::new().s("(").dec((-r) as u64).s(") failed to create cp table bo").say();
            return r;
        }
    }
    // set up the cp table
    amdgpu_gfx_rlc_setup_cp_table(adev);
    0
}

/// `gfx_v9_0_rlc_init`.
fn gfx_v9_0_rlc_init(adev: &mut amdgpu_device) -> i32 {
    adev.gfx.rlc.cs_data = true;
    // init clear state block
    let r = amdgpu_gfx_rlc_init_csb(adev);
    if r != 0 {
        return r;
    }
    if adev.flags & AMD_IS_APU != 0 {
        // TODO: double check the cp_table_size for RV
        adev.gfx.rlc.cp_table_size = crate::linux::align(96 * 5 * 4, 2048) as u32 + 64 * 1024; // JT + GDS
        let r = amdgpu_gfx_rlc_init_cpt(adev);
        if r != 0 {
            return r;
        }
    }
    0
}

/// `amdgpu_gfx_mec_queue_to_bit`.
fn amdgpu_gfx_mec_queue_to_bit(adev: &amdgpu_device, mec: u32, pipe: u32, queue: u32) -> u32 {
    let m = &adev.gfx.mec;
    mec * m.num_pipe_per_mec * m.num_queue_per_pipe + pipe * m.num_queue_per_pipe + queue
}

fn queue_bit(adev: &amdgpu_device, bit: u32) -> bool {
    adev.gfx.mec_queue_bitmap[bit as usize / 64] & (1 << (bit % 64)) != 0
}

/// `amdgpu_gfx_is_mec_queue_enabled`, XCC 0.
fn amdgpu_gfx_is_mec_queue_enabled(adev: &amdgpu_device, mec: u32, pipe: u32, queue: u32) -> bool {
    queue_bit(adev, amdgpu_gfx_mec_queue_to_bit(adev, mec, pipe, queue))
}

/// `amdgpu_gfx_compute_queue_acquire`: one XCC, and
/// `amdgpu_gfx_is_compute_multipipe_capable` says yes past GC 9.0.0 unless
/// `amdgpu_compute_multipipe` forces a policy.
fn amdgpu_gfx_compute_queue_acquire(adev: &mut amdgpu_device) {
    let multipipe = match crate::tables::amdgpu_compute_multipipe[0] {
        -1 => adev.ip_version(GC_HWIP, 0) > 9 << 24 || adev.gfx.mec.num_mec > 1,
        p => {
            Line::new().s("amdgpu: forcing compute pipe policy ").dec(p as u64).say();
            p == 1
        }
    };
    let m = &adev.gfx.mec;
    let max_queues_per_mec = (m.num_pipe_per_mec * m.num_queue_per_pipe).min(adev.gfx.num_compute_rings);
    let (pipes, queues) = (m.num_pipe_per_mec, m.num_queue_per_pipe);
    for i in 0..max_queues_per_mec {
        let bit = if multipipe {
            // policy: make queues evenly cross all pipes on MEC1 only
            let pipe = i % pipes;
            let queue = (i / pipes) % queues;
            pipe * queues + queue
        } else {
            // policy: amdgpu owns all queues in the given pipe
            i
        };
        adev.gfx.mec_queue_bitmap[bit as usize / 64] |= 1 << (bit % 64);
    }
}

/// `gfx_v9_0_mec_init`.
fn gfx_v9_0_mec_init(adev: &mut amdgpu_device) -> i32 {
    adev.gfx.mec_queue_bitmap = [0; (AMDGPU_MAX_COMPUTE_QUEUES as usize).div_ceil(64)];
    // take ownership of the relevant compute queues
    amdgpu_gfx_compute_queue_acquire(adev);
    let mec_hpd_size = adev.gfx.num_compute_rings as u64 * GFX9_MEC_HPD_SIZE as u64;
    if mec_hpd_size != 0 {
        match crate::amdgpu_object::amdgpu_bo_create_reserved(adev, mec_hpd_size, crate::amdgpu_object::PAGE_SIZE,
            AMDGPU_GEM_DOMAIN_VRAM | AMDGPU_GEM_DOMAIN_GTT, true) {
            Ok(Some(h)) => {
                adev.gfx.mec.hpd_eop_obj = Some(h);
                adev.gfx.mec.hpd_eop_gpu_addr = crate::amdgpu_object::amdgpu_bo_gpu_offset(adev, h);
                if let Some(k) = crate::amdgpu_object::bo(adev, h).kptr {
                    crate::amdgpu_gart::kptr_fill32(adev, k, 0, mec_hpd_size, 0);
                }
            }
            Ok(None) => return -12,
            Err(r) => {
                Line::new().s("(").dec((-r) as u64).s(") create HDP EOP bo failed").say();
                return r;
            }
        }
    }

    let Some(mec_fw) = adev.gfx.mec_fw.as_ref() else { return -22 };
    let fw_data = le32(mec_fw, UCODE_HDR_UCODE_ARRAY_OFFSET_BYTES) as usize;
    let fw_size = le32(mec_fw, UCODE_HDR_UCODE_SIZE_BYTES) as usize;
    let d = &mec_fw.data;
    let image = d.get(fw_data..fw_data + fw_size).unwrap_or(&[]).to_vec();
    match crate::amdgpu_object::amdgpu_bo_create_reserved(adev, fw_size as u64, crate::amdgpu_object::PAGE_SIZE,
        AMDGPU_GEM_DOMAIN_GTT, true) {
        Ok(Some(h)) => {
            adev.gfx.mec.mec_fw_obj = Some(h);
            adev.gfx.mec.mec_fw_gpu_addr = crate::amdgpu_object::amdgpu_bo_gpu_offset(adev, h);
            if let Some(k) = crate::amdgpu_object::bo(adev, h).kptr {
                crate::amdgpu_gart::kptr_write(adev, k, 0, &image);
            }
            0
        }
        Ok(None) => -12,
        Err(r) => {
            Line::new().s("(").dec((-r) as u64).s(") create mec firmware bo failed").say();
            r
        }
    }
}

/// `gfx_v9_0_compute_ring_init`.
fn gfx_v9_0_compute_ring_init(adev: &mut amdgpu_device, ring_id: u32, mec: u32, pipe: u32, queue: u32) -> i32 {
    let Some(funcs) = ring_funcs(adev.gfx.compute_ring_funcs[ring_id as usize]) else { return -22 };
    // mec0 is me1
    let me = mec + 1;
    let mut ring = amdgpu_ring::new(funcs, format!("comp_{}.{}.{}", me, pipe, queue));
    ring.me = me;
    ring.pipe = pipe;
    ring.queue = queue;
    ring.use_doorbell = true;
    ring.doorbell_index = (adev.doorbell_index.mec_ring0 + ring_id) << 1;
    ring.eop_gpu_addr = adev.gfx.mec.hpd_eop_gpu_addr + ring_id as u64 * GFX9_MEC_HPD_SIZE as u64;
    ring.vm_hub = AMDGPU_GFXHUB_0_;
    let irq_type = AMDGPU_CP_IRQ_COMPUTE_MEC1_PIPE0_EOP + (me - 1) * adev.gfx.mec.num_pipe_per_mec + pipe;
    // amdgpu_gfx_is_high_priority_compute_queue: the first of several
    let hw_prio = if adev.gfx.num_compute_rings > 1 && ring_id == 0 {
        AMDGPU_RING_PRIO_2
    } else {
        AMDGPU_RING_PRIO_DEFAULT
    };
    // type-2 packets are deprecated on MEC, use type-3 instead
    match amdgpu_ring_init(adev, ring, 1024, amdgpu_irq_src_id::gfx_eop, irq_type, hw_prio) {
        Ok(i) => {
            adev.gfx.compute_ring.push(i);
            0
        }
        Err(r) => r,
    }
}

/// `amdgpu_gfx_kiq_init`, XCC 0.
fn amdgpu_gfx_kiq_init(adev: &mut amdgpu_device, hpd_size: u32) -> i32 {
    match create_kernel(adev, hpd_size as u64, AMDGPU_GEM_DOMAIN_GTT) {
        Ok((h, a, p)) => {
            adev.gfx.kiq.eop_obj = Some(h);
            adev.gfx.kiq.eop_gpu_addr = a;
            if let Some(k) = p {
                crate::amdgpu_gart::kptr_fill32(adev, k, 0, hpd_size as u64, 0);
            }
            0
        }
        Err(r) => {
            Line::new().s("failed to create KIQ bo (").dec((-r) as u64).s(").").say();
            r
        }
    }
}

/// `amdgpu_gfx_kiq_acquire`, XCC 0: the highest queue bit the compute
/// rings did not take, on queue 0 and not on pipes 2/3 of MEC 2.
fn amdgpu_gfx_kiq_acquire(adev: &amdgpu_device, ring: &mut amdgpu_ring) -> i32 {
    let m = &adev.gfx.mec;
    let mut queue_bit = (m.num_mec * m.num_pipe_per_mec * m.num_queue_per_pipe) as i32;
    while {
        queue_bit -= 1;
        queue_bit >= 0
    } {
        if queue_bit_set(adev, queue_bit as u32) {
            continue;
        }
        // amdgpu_queue_mask_bit_to_mec_queue
        let bit = queue_bit as u32;
        let queue = bit % m.num_queue_per_pipe;
        let pipe = (bit / m.num_queue_per_pipe) % m.num_pipe_per_mec;
        let mec = (bit / m.num_queue_per_pipe) / m.num_pipe_per_mec;
        // 1. Using pipes 2/3 from MEC 2 seems cause problems.
        // 2. It must use queue id 0, because CGPG_IDLE/SAVE/LOAD/RUN only
        //    can be issued on queue 0.
        if (mec == 1 && pipe > 1) || queue != 0 {
            continue;
        }
        ring.me = mec + 1;
        ring.pipe = pipe;
        ring.queue = queue;
        return 0;
    }
    Line::new().s("Failed to find a queue for KIQ").say();
    -22
}

fn queue_bit_set(adev: &amdgpu_device, bit: u32) -> bool {
    queue_bit(adev, bit)
}

/// `amdgpu_gfx_kiq_init_ring`, XCC 0.
fn amdgpu_gfx_kiq_init_ring(adev: &mut amdgpu_device) -> i32 {
    let Some(funcs) = ring_funcs(adev.gfx.kiq0_ring_funcs) else { return -22 };
    let mut ring = amdgpu_ring::new(funcs, String::new());
    ring.use_doorbell = true;
    ring.xcc_id = 0;
    ring.vm_hub = AMDGPU_GFXHUB_0_;
    ring.doorbell_index = adev.doorbell_index.kiq << 1;
    let r = amdgpu_gfx_kiq_acquire(adev, &mut ring);
    if r != 0 {
        return r;
    }
    ring.eop_gpu_addr = adev.gfx.kiq.eop_gpu_addr;
    ring.no_scheduler = true;
    ring.name = format!("kiq_{}.{}.{}.{}", 0, ring.me as u8, ring.pipe as u8, ring.queue as u8);
    match amdgpu_ring_init(adev, ring, 1024, amdgpu_irq_src_id::kiq, AMDGPU_CP_KIQ_IRQ_DRIVER0, AMDGPU_RING_PRIO_DEFAULT) {
        Ok(i) => {
            adev.gfx.kiq.ring = Some(i);
            0
        }
        Err(r) => {
            Line::new().s("(").dec((-r) as u64).s(") failed to init kiq ring").say();
            r
        }
    }
}

/// `amdgpu_gfx_mqd_sw_init`, XCC 0: GC 9 keeps the compute MQDs in GTT;
/// no KGQ MQDs before Navi.
fn amdgpu_gfx_mqd_sw_init(adev: &mut amdgpu_device, mqd_size: u32) -> i32 {
    let domain = AMDGPU_GEM_DOMAIN_GTT;
    // create MQD for KIQ
    if let Some(k) = adev.gfx.kiq.ring {
        if !adev.enable_mes_kiq && adev.rings[k].mqd_obj.is_none() {
            // KIQ MQD in the VRAM domain no matter SRIOV or bare metal
            match create_kernel(adev, mqd_size as u64, AMDGPU_GEM_DOMAIN_VRAM | AMDGPU_GEM_DOMAIN_GTT) {
                Ok((h, a, p)) => {
                    let ring = &mut adev.rings[k];
                    (ring.mqd_obj, ring.mqd_gpu_addr, ring.mqd_ptr) = (Some(h), a, p);
                }
                Err(r) => {
                    Line::new().s("failed to create ring mqd ob (").dec((-r) as u64).s(")").say();
                    return r;
                }
            }
            // prepare MQD backup
            adev.gfx.kiq.mqd_backup = vec![0; mqd_size as usize];
        }
    }

    // create MQD for each KCQ
    adev.gfx.mec.mqd_backup = (0..adev.gfx.num_compute_rings).map(|_| Vec::new()).collect();
    for i in 0..adev.gfx.num_compute_rings as usize {
        let Some(&k) = adev.gfx.compute_ring.get(i) else { continue };
        if adev.rings[k].mqd_obj.is_some() {
            continue;
        }
        match create_kernel(adev, mqd_size as u64, domain) {
            Ok((h, a, p)) => {
                let ring = &mut adev.rings[k];
                (ring.mqd_obj, ring.mqd_gpu_addr, ring.mqd_ptr) = (Some(h), a, p);
                ring.mqd_size = mqd_size;
            }
            Err(r) => {
                Line::new().s("failed to create ring mqd bo (").dec((-r) as u64).s(")").say();
                return r;
            }
        }
        // prepare MQD backup
        adev.gfx.mec.mqd_backup[i] = vec![0; mqd_size as usize];
    }
    0
}

/// `gfx_v9_0_gpu_early_init`, GC 9.3.0.
fn gfx_v9_0_gpu_early_init(adev: &mut amdgpu_device) -> i32 {
    if adev.ip_version(GC_HWIP, 0) != 9 << 24 | 3 << 16 {
        Line::new().s("gfx_v9_0_gpu_early_init: only GC 9.3.0 is ported").say();
        return -22;
    }
    let c = &mut adev.gfx.config;
    c.max_hw_contexts = 8;
    c.sc_prim_fifo_size_frontend = 0x20;
    c.sc_prim_fifo_size_backend = 0x100;
    c.sc_hiz_tile_fifo_size = 0x80;
    c.sc_earlyz_tile_fifo_size = 0x4C0;
    let reg = gc_reg(adev, mmGB_ADDR_CONFIG, mmGB_ADDR_CONFIG_BASE_IDX);
    let mut gb_addr_config = adev.rreg32(reg).unwrap_or(0);
    gb_addr_config &= !0xf3e7_77ff;
    gb_addr_config |= 0x2201_0042;

    let c = &mut adev.gfx.config;
    c.gb_addr_config = gb_addr_config;
    let field = |mask: u32, shift: u32| (gb_addr_config & mask) >> shift;
    let f = &mut c.gb_addr_config_fields;
    f.num_pipes = 1 << field(GB_ADDR_CONFIG__NUM_PIPES_MASK, GB_ADDR_CONFIG__NUM_PIPES__SHIFT);
    c.max_tile_pipes = f.num_pipes;
    f.num_banks = 1 << field(GB_ADDR_CONFIG__NUM_BANKS_MASK, GB_ADDR_CONFIG__NUM_BANKS__SHIFT);
    f.max_compress_frags =
        1 << field(GB_ADDR_CONFIG__MAX_COMPRESSED_FRAGS_MASK, GB_ADDR_CONFIG__MAX_COMPRESSED_FRAGS__SHIFT);
    f.num_rb_per_se = 1 << field(GB_ADDR_CONFIG__NUM_RB_PER_SE_MASK, GB_ADDR_CONFIG__NUM_RB_PER_SE__SHIFT);
    f.num_se = 1 << field(GB_ADDR_CONFIG__NUM_SHADER_ENGINES_MASK, GB_ADDR_CONFIG__NUM_SHADER_ENGINES__SHIFT);
    f.pipe_interleave_size =
        1 << (8 + field(GB_ADDR_CONFIG__PIPE_INTERLEAVE_SIZE_MASK, GB_ADDR_CONFIG__PIPE_INTERLEAVE_SIZE__SHIFT));
    0
}

/// `gfx_v9_0_sw_init`, GC 9.3.0: two MECs, the cleaner shader when the
/// CP firmware is new enough, one gfx ring, no MCBP software rings.
pub fn gfx_v9_0_sw_init(adev: &mut amdgpu_device) -> i32 {
    if adev.ip_version(GC_HWIP, 0) != 9 << 24 | 3 << 16 {
        Line::new().s("gfx_v9_0_sw_init: only GC 9.3.0 is ported").say();
        return -22;
    }
    adev.gfx.mec.num_mec = 2;

    adev.gfx.cleaner_shader_size = SIZEOF_gfx_9_4_2_cleaner_shader_hex;
    if adev.gfx.me_fw_version >= 167 && adev.gfx.pfp_fw_version >= 196 && adev.gfx.mec_fw_version >= 474 {
        adev.gfx.enable_cleaner_shader = true;
        // amdgpu_gfx_cleaner_shader_sw_init
        match create_kernel(adev, adev.gfx.cleaner_shader_size as u64, AMDGPU_GEM_DOMAIN_VRAM | AMDGPU_GEM_DOMAIN_GTT) {
            Ok((h, a, p)) => {
                (adev.gfx.cleaner_shader_obj, adev.gfx.cleaner_shader_gpu_addr, adev.gfx.cleaner_shader_cpu_ptr) =
                    (Some(h), a, p);
            }
            Err(_) => {
                adev.gfx.enable_cleaner_shader = false;
                Line::new().s("Failed to initialize cleaner shader").say();
            }
        }
    }

    adev.gfx.mec.num_pipe_per_mec = 4;
    adev.gfx.mec.num_queue_per_pipe = 8;

    let eop = adev.gfx.eop_irq;
    let bad_op = adev.gfx.bad_op_irq;
    let priv_reg = adev.gfx.priv_reg_irq;
    let priv_inst = adev.gfx.priv_inst_irq;
    let ecc = adev.gfx.cp_ecc_error_irq;
    for (src_id, src, name) in [
        // EOP Event
        (GFX_9_0__SRCID__CP_EOP_INTERRUPT, &eop, "gfx.eop_irq"),
        // Bad opcode Event
        (GFX_9_0__SRCID__CP_BAD_OPCODE_ERROR, &bad_op, "gfx.bad_op_irq"),
        // Privileged reg
        (GFX_9_0__SRCID__CP_PRIV_REG_FAULT, &priv_reg, "gfx.priv_reg_irq"),
        // Privileged inst
        (GFX_9_0__SRCID__CP_PRIV_INSTR_FAULT, &priv_inst, "gfx.priv_inst_irq"),
        // ECC error
        (GFX_9_0__SRCID__CP_ECC_ERROR, &ecc, "gfx.cp_ecc_error_irq"),
        // FUE error
        (GFX_9_0__SRCID__CP_FUE_ERROR, &ecc, "gfx.cp_ecc_error_irq"),
    ] {
        let r = crate::amdgpu_irq::amdgpu_irq_add_id(adev, SOC15_IH_CLIENTID_GRBM_CP, src_id, src, name);
        if r != 0 {
            return r;
        }
    }

    adev.gfx.gfx_current_status = AMDGPU_GFX_NORMAL_MODE;

    if adev.gfx.rlc.funcs.is_some() {
        let r = gfx_v9_0_rlc_init(adev);
        if r != 0 {
            Line::new().s("Failed to init rlc BOs!").say();
            return r;
        }
    }

    let r = gfx_v9_0_mec_init(adev);
    if r != 0 {
        Line::new().s("Failed to init MEC BOs!").say();
        return r;
    }

    // set up the gfx ring
    for i in 0..adev.gfx.num_gfx_rings as usize {
        let Some(funcs) = ring_funcs(adev.gfx.gfx_ring_funcs[i]) else { return -22 };
        let name = if i == 0 { String::from("gfx") } else { format!("gfx_{}", i) };
        let mut ring = amdgpu_ring::new(funcs, name);
        ring.use_doorbell = true;
        ring.doorbell_index = adev.doorbell_index.gfx_ring0 << 1;
        // disable scheduler on the real ring
        ring.no_scheduler = adev.gfx.mcbp;
        ring.vm_hub = AMDGPU_GFXHUB_0_;
        match amdgpu_ring_init(adev, ring, 1024, amdgpu_irq_src_id::gfx_eop, AMDGPU_CP_IRQ_GFX_ME0_PIPE0_EOP,
            AMDGPU_RING_PRIO_DEFAULT) {
            Ok(k) => adev.gfx.gfx_ring.push(k),
            Err(r) => return r,
        }
    }
    // set up the software rings: only with MCBP
    if adev.gfx.mcbp && adev.gfx.num_gfx_rings != 0 {
        Line::new().s("gfx software rings: MCBP is not Renoir's default, not ported").say();
        return -22;
    }

    // set up the compute queues - allocate horizontally across pipes
    let mut ring_id = 0;
    for i in 0..adev.gfx.mec.num_mec {
        for j in 0..adev.gfx.mec.num_queue_per_pipe {
            for k in 0..adev.gfx.mec.num_pipe_per_mec {
                if !amdgpu_gfx_is_mec_queue_enabled(adev, i, k, j) {
                    continue;
                }
                let r = gfx_v9_0_compute_ring_init(adev, ring_id, i, k, j);
                if r != 0 {
                    return r;
                }
                ring_id += 1;
            }
        }
    }

    // TODO: Add queue reset mask when FW fully supports it
    let gfx0 = adev.gfx.gfx_ring.first().copied();
    let comp0 = adev.gfx.compute_ring.first().copied();
    adev.gfx.gfx_supported_reset = crate::amdgpu_ring::amdgpu_get_soft_full_reset_mask(adev, gfx0);
    adev.gfx.compute_supported_reset = crate::amdgpu_ring::amdgpu_get_soft_full_reset_mask(adev, comp0);
    if !adev.debug_disable_gpu_ring_reset {
        adev.gfx.compute_supported_reset |= AMDGPU_RESET_TYPE_PER_QUEUE;
    }

    let r = amdgpu_gfx_kiq_init(adev, GFX9_MEC_HPD_SIZE);
    if r != 0 {
        Line::new().s("Failed to init KIQ BOs!").say();
        return r;
    }
    let r = amdgpu_gfx_kiq_init_ring(adev);
    if r != 0 {
        return r;
    }

    // create MQD for all compute queues as wel as KIQ for SRIOV case
    let r = amdgpu_gfx_mqd_sw_init(adev, SIZEOF_v9_mqd_allocation);
    if r != 0 {
        return r;
    }

    adev.gfx.ce_ram_size = 0x8000;

    let r = gfx_v9_0_gpu_early_init(adev);
    if r != 0 {
        return r;
    }
    // amdgpu_gfx_ras_sw_init: GC 9.3.0 sets no adev->gfx.ras.

    // gfx_v9_0_alloc_ip_dump
    adev.gfx.ip_dump_core = vec![0; GC_REG_LIST_9_COUNT as usize];
    let m = &adev.gfx.mec;
    let inst = m.num_mec * m.num_pipe_per_mec * m.num_queue_per_pipe;
    adev.gfx.ip_dump_compute_queues = vec![0; (GC_CP_REG_LIST_9_COUNT * inst) as usize];
    // amdgpu_gfx_sysfs_init: no sysfs.
    0
}

// ---- hw_init (stage 4) ----

/// `PACKET3(op, n)` (soc15d.h).
#[allow(non_snake_case)]
pub fn PACKET3(op: u32, n: u32) -> u32 {
    (PACKET_TYPE3 << 30) | ((op & 0xFF) << 8) | ((n & 0x3FFF) << 16)
}

/// `RREG32_SOC15(GC, 0, reg)` and `_OFFSET`.
fn gc_rd(adev: &amdgpu_device, idx: u32, reg: u32) -> u32 {
    crate::nbio_v7_0::rreg32_soc15(adev, GC_HWIP, 0, idx, reg).unwrap_or(0)
}

/// `WREG32_SOC15(GC, 0, reg, v)` and its `_RLC`, `_RLC_SHADOW` and
/// `_OFFSET` forms, which are the same write on bare metal.
fn gc_wr(adev: &amdgpu_device, idx: u32, reg: u32, v: u32) {
    crate::nbio_v7_0::wreg32_soc15(adev, GC_HWIP, 0, idx, reg, v);
}

/// `REG_SET_FIELD`.
fn set_field(v: u32, mask: u32, shift: u32, f: u32) -> u32 {
    (v & !mask) | (mask & (f << shift))
}

/// `WREG32_FIELD15(GC, 0, reg, field, val)`.
fn gc_field15(adev: &amdgpu_device, idx: u32, reg: u32, mask: u32, shift: u32, val: u32) {
    let v = gc_rd(adev, idx, reg);
    gc_wr(adev, idx, reg, (v & !mask) | (val << shift));
}

/// `amdgpu_gfx_create_bitmask`.
fn amdgpu_gfx_create_bitmask(bit_width: u32) -> u32 {
    ((1u64 << bit_width) - 1) as u32
}

/// `soc15_grbm_select`.
pub fn soc15_grbm_select(adev: &amdgpu_device, me: u32, pipe: u32, queue: u32, vmid: u32) {
    let mut v = 0;
    v = set_field(v, GC_GRBM_GFX_CNTL__PIPEID_MASK, GC_GRBM_GFX_CNTL__PIPEID__SHIFT, pipe);
    v = set_field(v, GC_GRBM_GFX_CNTL__MEID_MASK, GC_GRBM_GFX_CNTL__MEID__SHIFT, me);
    v = set_field(v, GC_GRBM_GFX_CNTL__VMID_MASK, GC_GRBM_GFX_CNTL__VMID__SHIFT, vmid);
    v = set_field(v, GC_GRBM_GFX_CNTL__QUEUEID_MASK, GC_GRBM_GFX_CNTL__QUEUEID__SHIFT, queue);
    gc_wr(adev, GC_mmGRBM_GFX_CNTL_BASE_IDX, GC_mmGRBM_GFX_CNTL, v);
}

/// `gfx_v9_0_select_se_sh` (`amdgpu_gfx_select_se_sh`).
pub fn gfx_v9_0_select_se_sh(adev: &amdgpu_device, se_num: u32, sh_num: u32, instance: u32) {
    let mut data = if instance == 0xffffffff {
        set_field(0, GC_GRBM_GFX_INDEX__INSTANCE_BROADCAST_WRITES_MASK,
                  GC_GRBM_GFX_INDEX__INSTANCE_BROADCAST_WRITES__SHIFT, 1)
    } else {
        set_field(0, GC_GRBM_GFX_INDEX__INSTANCE_INDEX_MASK, GC_GRBM_GFX_INDEX__INSTANCE_INDEX__SHIFT, instance)
    };
    data = if se_num == 0xffffffff {
        set_field(data, GC_GRBM_GFX_INDEX__SE_BROADCAST_WRITES_MASK, GC_GRBM_GFX_INDEX__SE_BROADCAST_WRITES__SHIFT, 1)
    } else {
        set_field(data, GC_GRBM_GFX_INDEX__SE_INDEX_MASK, GC_GRBM_GFX_INDEX__SE_INDEX__SHIFT, se_num)
    };
    data = if sh_num == 0xffffffff {
        set_field(data, GC_GRBM_GFX_INDEX__SH_BROADCAST_WRITES_MASK, GC_GRBM_GFX_INDEX__SH_BROADCAST_WRITES__SHIFT, 1)
    } else {
        set_field(data, GC_GRBM_GFX_INDEX__SH_INDEX_MASK, GC_GRBM_GFX_INDEX__SH_INDEX__SHIFT, sh_num)
    };
    gc_wr(adev, GC_mmGRBM_GFX_INDEX_BASE_IDX, GC_mmGRBM_GFX_INDEX, data);
}

/// `soc15_program_register_sequence` over a `struct soc15_reg_golden`
/// table, six u32 per entry: hwip, instance, segment, reg, and_mask,
/// or_mask. The read is `RREG32_SOC15_IP` or `RREG32`, the write
/// `WREG32_RLC`, `WREG32_SOC15_IP` or `WREG32`: on bare metal each is the
/// plain register access.
pub fn soc15_program_register_sequence(adev: &amdgpu_device, regs: &[u32]) {
    for e in regs.chunks_exact(6) {
        let (hwip, instance, segment, r, and_mask, or_mask) = (e[0], e[1], e[2], e[3], e[4], e[5]);
        let reg = adev.reg_offset[hwip as usize][instance as usize].get(segment as usize).copied().unwrap_or(0) + r;
        let tmp = if and_mask == 0xffffffff {
            or_mask
        } else {
            let mut t = adev.rreg32(reg).unwrap_or(0);
            t &= !and_mask;
            t | (or_mask & and_mask)
        };
        adev.wreg32(reg, tmp);
    }
}

/// `gfx_v9_0_init_golden_registers`, GC 9.3.0: only the Renoir table.
fn gfx_v9_0_init_golden_registers(adev: &amdgpu_device) {
    if adev.ip_version(GC_HWIP, 0) == 9 << 24 | 3 << 16 {
        soc15_program_register_sequence(adev, &crate::tables::golden_settings_gc_9_1_rn);
        // for renoir, don't need common goldensetting
    }
}

/// `gfx_v9_0_get_rb_active_bitmap`.
fn gfx_v9_0_get_rb_active_bitmap(adev: &amdgpu_device) -> u32 {
    let mut data = gc_rd(adev, GC_mmCC_RB_BACKEND_DISABLE_BASE_IDX, GC_mmCC_RB_BACKEND_DISABLE);
    data |= gc_rd(adev, GC_mmGC_USER_RB_BACKEND_DISABLE_BASE_IDX, GC_mmGC_USER_RB_BACKEND_DISABLE);
    data &= GC_CC_RB_BACKEND_DISABLE__BACKEND_DISABLE_MASK;
    data >>= GC_GC_USER_RB_BACKEND_DISABLE__BACKEND_DISABLE__SHIFT;
    let c = &adev.gfx.config;
    let mask = amdgpu_gfx_create_bitmask(c.max_backends_per_se / c.max_sh_per_se);
    !data & mask
}

/// `gfx_v9_0_setup_rb`.
fn gfx_v9_0_setup_rb(adev: &mut amdgpu_device) {
    let c = &adev.gfx.config;
    let (se, sh) = (c.max_shader_engines, c.max_sh_per_se);
    let rb_bitmap_width_per_sh = c.max_backends_per_se / c.max_sh_per_se;
    let mut active_rbs = 0u32;
    for i in 0..se {
        for j in 0..sh {
            gfx_v9_0_select_se_sh(adev, i, j, 0xffffffff);
            let data = gfx_v9_0_get_rb_active_bitmap(adev);
            active_rbs |= data << ((i * sh + j) * rb_bitmap_width_per_sh);
        }
    }
    gfx_v9_0_select_se_sh(adev, 0xffffffff, 0xffffffff, 0xffffffff);
    adev.gfx.config.backend_enable_mask = active_rbs;
    adev.gfx.config.num_rbs = active_rbs.count_ones();
}

/// `gfx_v9_0_set_user_cu_inactive_bitmap`.
fn gfx_v9_0_set_user_cu_inactive_bitmap(adev: &amdgpu_device, bitmap: u32) {
    if bitmap == 0 {
        return;
    }
    let mut data = bitmap << GC_GC_USER_SHADER_ARRAY_CONFIG__INACTIVE_CUS__SHIFT;
    data &= GC_GC_USER_SHADER_ARRAY_CONFIG__INACTIVE_CUS_MASK;
    gc_wr(adev, GC_mmGC_USER_SHADER_ARRAY_CONFIG_BASE_IDX, GC_mmGC_USER_SHADER_ARRAY_CONFIG, data);
}

/// `gfx_v9_0_get_cu_active_bitmap`.
fn gfx_v9_0_get_cu_active_bitmap(adev: &amdgpu_device) -> u32 {
    let mut data = gc_rd(adev, GC_mmCC_GC_SHADER_ARRAY_CONFIG_BASE_IDX, GC_mmCC_GC_SHADER_ARRAY_CONFIG);
    data |= gc_rd(adev, GC_mmGC_USER_SHADER_ARRAY_CONFIG_BASE_IDX, GC_mmGC_USER_SHADER_ARRAY_CONFIG);
    data &= GC_CC_GC_SHADER_ARRAY_CONFIG__INACTIVE_CUS_MASK;
    data >>= GC_CC_GC_SHADER_ARRAY_CONFIG__INACTIVE_CUS__SHIFT;
    let mask = amdgpu_gfx_create_bitmask(adev.gfx.config.max_cu_per_sh);
    !data & mask
}

/// `gfx_v9_0_get_cu_info`. `amdgpu_gfx_parse_disable_cu` gives all zeros:
/// `amdgpu_disable_cu` is unset.
fn gfx_v9_0_get_cu_info(adev: &mut amdgpu_device) -> i32 {
    let c = &adev.gfx.config;
    let (max_se, max_sh, max_cu) = (c.max_shader_engines, c.max_sh_per_se, c.max_cu_per_sh);
    // 16 comes from bitmap array size 4*4, and it can cover all gfx9 ASICs
    if max_se * max_sh > 16 {
        return -22;
    }
    let disable_masks = [0u32; 16];
    let mut active_cu_number = 0;
    let mut ao_cu_mask = 0u32;
    for i in 0..max_se {
        for j in 0..max_sh {
            let mut mask = 1u32;
            let mut ao_bitmap = 0u32;
            let mut counter = 0;
            gfx_v9_0_select_se_sh(adev, i, j, 0xffffffff);
            gfx_v9_0_set_user_cu_inactive_bitmap(adev, disable_masks[(i * max_sh + j) as usize]);
            let bitmap = gfx_v9_0_get_cu_active_bitmap(adev);
            adev.gfx.cu_info.bitmap[0][(i % 4) as usize][(j + i / 4) as usize] = bitmap;
            for _ in 0..max_cu {
                if bitmap & mask != 0 {
                    if counter < max_cu {
                        ao_bitmap |= mask;
                    }
                    counter += 1;
                }
                mask <<= 1;
            }
            active_cu_number += counter;
            if i < 2 && j < 2 {
                ao_cu_mask |= ao_bitmap << (i * 16 + j * 8);
            }
            adev.gfx.cu_info.ao_cu_bitmap[(i % 4) as usize][(j + i / 4) as usize] = ao_bitmap;
        }
    }
    gfx_v9_0_select_se_sh(adev, 0xffffffff, 0xffffffff, 0xffffffff);
    adev.gfx.cu_info.number = active_cu_number;
    adev.gfx.cu_info.ao_cu_mask = ao_cu_mask;
    adev.gfx.cu_info.simd_per_cu = NUM_SIMD_PER_CU;
    0
}

/// `gfx_v9_0_init_compute_vmid`.
fn gfx_v9_0_init_compute_vmid(adev: &amdgpu_device) {
    // Configure apertures:
    // LDS:         0x60000000'00000000 - 0x60000001'00000000 (4GB)
    // Scratch:     0x60000001'00000000 - 0x60000002'00000000 (4GB)
    // GPUVM:       0x60010000'00000000 - 0x60020000'00000000 (1TB)
    let sh_mem_bases = DEFAULT_SH_MEM_BASES | (DEFAULT_SH_MEM_BASES << 16);
    let sh_mem_config =
        SH_MEM_ADDRESS_MODE_64 | SH_MEM_ALIGNMENT_MODE_UNALIGNED << GC_SH_MEM_CONFIG__ALIGNMENT_MODE__SHIFT;
    let first = adev.vm_manager.first_kfd_vmid;
    for i in first..AMDGPU_NUM_VMID {
        soc15_grbm_select(adev, 0, 0, 0, i);
        // CP and shaders
        gc_wr(adev, GC_mmSH_MEM_CONFIG_BASE_IDX, GC_mmSH_MEM_CONFIG, sh_mem_config);
        gc_wr(adev, GC_mmSH_MEM_BASES_BASE_IDX, GC_mmSH_MEM_BASES, sh_mem_bases);
    }
    soc15_grbm_select(adev, 0, 0, 0, 0);
    // Initialize all compute VMIDs to have no GDS, GWS, or OA access. These
    // should be enabled by FW for target VMIDs.
    for i in first..AMDGPU_NUM_VMID {
        gc_wr(adev, GC_mmGDS_VMID0_BASE_BASE_IDX, GC_mmGDS_VMID0_BASE + 2 * i, 0);
        gc_wr(adev, GC_mmGDS_VMID0_SIZE_BASE_IDX, GC_mmGDS_VMID0_SIZE + 2 * i, 0);
        gc_wr(adev, GC_mmGDS_GWS_VMID0_BASE_IDX, GC_mmGDS_GWS_VMID0 + i, 0);
        gc_wr(adev, GC_mmGDS_OA_VMID0_BASE_IDX, GC_mmGDS_OA_VMID0 + i, 0);
    }
}

/// `gfx_v9_0_init_gds_vmid`.
fn gfx_v9_0_init_gds_vmid(adev: &amdgpu_device) {
    // Initialize all compute and user-gfx VMIDs to have no GDS, GWS, or OA
    // access. Compute VMIDs should be enabled by FW for target VMIDs, the
    // driver can enable them for graphics. VMID0 should maintain access so
    // that HWS firmware can save/restore entries.
    for vmid in 1..AMDGPU_NUM_VMID {
        gc_wr(adev, GC_mmGDS_VMID0_BASE_BASE_IDX, GC_mmGDS_VMID0_BASE + 2 * vmid, 0);
        gc_wr(adev, GC_mmGDS_VMID0_SIZE_BASE_IDX, GC_mmGDS_VMID0_SIZE + 2 * vmid, 0);
        gc_wr(adev, GC_mmGDS_GWS_VMID0_BASE_IDX, GC_mmGDS_GWS_VMID0 + vmid, 0);
        gc_wr(adev, GC_mmGDS_OA_VMID0_BASE_IDX, GC_mmGDS_OA_VMID0 + vmid, 0);
    }
}

/// `gfx_v9_0_constants_init`. `gfx_v9_0_tiling_mode_table_init` is empty
/// and `gfx_v9_0_init_sq_config` does nothing for GC 9.3.0.
fn gfx_v9_0_constants_init(adev: &mut amdgpu_device) {
    gc_field15(adev, GC_mmGRBM_CNTL_BASE_IDX, GC_mmGRBM_CNTL, GC_GRBM_CNTL__READ_TIMEOUT_MASK,
               GC_GRBM_CNTL__READ_TIMEOUT__SHIFT, 0xff);
    if adev.gfx.num_gfx_rings != 0 {
        gfx_v9_0_setup_rb(adev);
    }
    gfx_v9_0_get_cu_info(adev);
    adev.gfx.config.db_debug2 = gc_rd(adev, GC_mmDB_DEBUG2_BASE_IDX, GC_mmDB_DEBUG2);

    // XXX SH_MEM regs
    // where to put LDS, scratch, GPUVM in FSA64 space
    let noretry = (adev.gmc.noretry != 0) as u32;
    for i in 0..adev.vm_manager.id_mgr[AMDGPU_GFXHUB_0 as usize].num_ids {
        soc15_grbm_select(adev, 0, 0, 0, i);
        // CP and shaders
        let mut tmp = set_field(0, GC_SH_MEM_CONFIG__ALIGNMENT_MODE_MASK, GC_SH_MEM_CONFIG__ALIGNMENT_MODE__SHIFT,
                                SH_MEM_ALIGNMENT_MODE_UNALIGNED);
        tmp = set_field(tmp, GC_SH_MEM_CONFIG__RETRY_DISABLE_MASK, GC_SH_MEM_CONFIG__RETRY_DISABLE__SHIFT, noretry);
        gc_wr(adev, GC_mmSH_MEM_CONFIG_BASE_IDX, GC_mmSH_MEM_CONFIG, tmp);
        if i == 0 {
            gc_wr(adev, GC_mmSH_MEM_BASES_BASE_IDX, GC_mmSH_MEM_BASES, 0);
        } else {
            let mut tmp = set_field(0, GC_SH_MEM_BASES__PRIVATE_BASE_MASK, GC_SH_MEM_BASES__PRIVATE_BASE__SHIFT,
                                    (adev.gmc.private_aperture_start >> 48) as u32);
            tmp = set_field(tmp, GC_SH_MEM_BASES__SHARED_BASE_MASK, GC_SH_MEM_BASES__SHARED_BASE__SHIFT,
                            (adev.gmc.shared_aperture_start >> 48) as u32);
            gc_wr(adev, GC_mmSH_MEM_BASES_BASE_IDX, GC_mmSH_MEM_BASES, tmp);
        }
    }
    soc15_grbm_select(adev, 0, 0, 0, 0);

    gfx_v9_0_init_compute_vmid(adev);
    gfx_v9_0_init_gds_vmid(adev);
}

/// `gfx_v9_0_enable_gui_idle_interrupt`.
fn gfx_v9_0_enable_gui_idle_interrupt(adev: &amdgpu_device, enable: bool) {
    // These interrupts should be enabled to drive DS clock
    let e = enable as u32;
    let mut tmp = gc_rd(adev, GC_mmCP_INT_CNTL_RING0_BASE_IDX, GC_mmCP_INT_CNTL_RING0);
    tmp = set_field(tmp, GC_CP_INT_CNTL_RING0__CNTX_BUSY_INT_ENABLE_MASK,
                    GC_CP_INT_CNTL_RING0__CNTX_BUSY_INT_ENABLE__SHIFT, e);
    tmp = set_field(tmp, GC_CP_INT_CNTL_RING0__CNTX_EMPTY_INT_ENABLE_MASK,
                    GC_CP_INT_CNTL_RING0__CNTX_EMPTY_INT_ENABLE__SHIFT, e);
    tmp = set_field(tmp, GC_CP_INT_CNTL_RING0__CMP_BUSY_INT_ENABLE_MASK,
                    GC_CP_INT_CNTL_RING0__CMP_BUSY_INT_ENABLE__SHIFT, e);
    if adev.gfx.num_gfx_rings != 0 {
        tmp = set_field(tmp, GC_CP_INT_CNTL_RING0__GFX_IDLE_INT_ENABLE_MASK,
                        GC_CP_INT_CNTL_RING0__GFX_IDLE_INT_ENABLE__SHIFT, e);
    }
    gc_wr(adev, GC_mmCP_INT_CNTL_RING0_BASE_IDX, GC_mmCP_INT_CNTL_RING0, tmp);
}

/// `gfx_v9_0_wait_for_rlc_serdes`.
fn gfx_v9_0_wait_for_rlc_serdes(adev: &amdgpu_device) {
    let c = &adev.gfx.config;
    for i in 0..c.max_shader_engines {
        for j in 0..c.max_sh_per_se {
            gfx_v9_0_select_se_sh(adev, i, j, 0xffffffff);
            let mut k = 0;
            while k < adev.usec_timeout {
                if gc_rd(adev, GC_mmRLC_SERDES_CU_MASTER_BUSY_BASE_IDX, GC_mmRLC_SERDES_CU_MASTER_BUSY) == 0 {
                    break;
                }
                crate::linux::udelay(1);
                k += 1;
            }
            if k == adev.usec_timeout {
                gfx_v9_0_select_se_sh(adev, 0xffffffff, 0xffffffff, 0xffffffff);
                Line::new().s("Timeout wait for RLC serdes ").dec(i as u64).s(",").dec(j as u64).say();
                return;
            }
        }
    }
    gfx_v9_0_select_se_sh(adev, 0xffffffff, 0xffffffff, 0xffffffff);
    let mask = GC_RLC_SERDES_NONCU_MASTER_BUSY__SE_MASTER_BUSY_MASK
        | GC_RLC_SERDES_NONCU_MASTER_BUSY__GC_MASTER_BUSY_MASK
        | GC_RLC_SERDES_NONCU_MASTER_BUSY__TC0_MASTER_BUSY_MASK
        | GC_RLC_SERDES_NONCU_MASTER_BUSY__TC1_MASTER_BUSY_MASK;
    for _ in 0..adev.usec_timeout {
        if gc_rd(adev, GC_mmRLC_SERDES_NONCU_MASTER_BUSY_BASE_IDX, GC_mmRLC_SERDES_NONCU_MASTER_BUSY) & mask == 0 {
            break;
        }
        crate::linux::udelay(1);
    }
}

/// `gfx_v9_0_rlc_stop`.
fn gfx_v9_0_rlc_stop(adev: &amdgpu_device) {
    gc_field15(adev, GC_mmRLC_CNTL_BASE_IDX, GC_mmRLC_CNTL, GC_RLC_CNTL__RLC_ENABLE_F32_MASK,
               GC_RLC_CNTL__RLC_ENABLE_F32__SHIFT, 0);
    gfx_v9_0_enable_gui_idle_interrupt(adev, false);
    gfx_v9_0_wait_for_rlc_serdes(adev);
}

/// `gfx_v9_0_rlc_start`: an APU enables the CP interrupt after the CP is
/// up.
fn gfx_v9_0_rlc_start(adev: &amdgpu_device) {
    gc_field15(adev, GC_mmRLC_CNTL_BASE_IDX, GC_mmRLC_CNTL, GC_RLC_CNTL__RLC_ENABLE_F32_MASK,
               GC_RLC_CNTL__RLC_ENABLE_F32__SHIFT, 1);
    crate::linux::udelay(50);
    // carrizo do enable cp interrupt after cp inited
    if adev.flags & AMD_IS_APU == 0 {
        gfx_v9_0_enable_gui_idle_interrupt(adev, true);
        crate::linux::udelay(50);
    }
}

/// `gfx_v9_0_get_csb_buffer`: `amdgpu_gfx_csb_preamble_start`,
/// `amdgpu_gfx_csb_data_parser`, `amdgpu_gfx_csb_preamble_end`.
fn gfx_v9_0_get_csb_buffer(adev: &amdgpu_device) -> Vec<u32> {
    let mut b = Vec::new();
    if !adev.gfx.rlc.cs_data {
        return b;
    }
    b.push(PACKET3(PACKET3_PREAMBLE_CNTL, 0));
    b.push(PACKET3_PREAMBLE_BEGIN_CLEAR_STATE);
    b.push(PACKET3(PACKET3_CONTEXT_CONTROL, 1));
    b.push(0x80000000);
    b.push(0x80000000);
    for (extent, reg_index, reg_count) in GFX9_SECT_CONTEXT_DEFS.iter() {
        b.push(PACKET3(PACKET3_SET_CONTEXT_REG, *reg_count));
        b.push(reg_index - PACKET3_SET_CONTEXT_REG_START);
        b.extend_from_slice(&extent[..*reg_count as usize]);
    }
    b.push(PACKET3(PACKET3_PREAMBLE_CNTL, 0));
    b.push(PACKET3_PREAMBLE_END_CLEAR_STATE);
    b.push(PACKET3(PACKET3_CLEAR_STATE, 0));
    b.push(0);
    b
}

/// `gfx_v9_0_init_csb`.
fn gfx_v9_0_init_csb(adev: &amdgpu_device) {
    let buf = gfx_v9_0_get_csb_buffer(adev);
    if let Some(k) = adev.gfx.rlc.cs_ptr {
        if !buf.is_empty() {
            let bytes: Vec<u8> = buf.iter().flat_map(|w| w.to_le_bytes()).collect();
            crate::amdgpu_gart::kptr_write(adev, k, 0, &bytes);
        }
    }
    // csib
    let rlc = &adev.gfx.rlc;
    gc_wr(adev, GC_mmRLC_CSIB_ADDR_HI_BASE_IDX, GC_mmRLC_CSIB_ADDR_HI, (rlc.clear_state_gpu_addr >> 32) as u32);
    gc_wr(adev, GC_mmRLC_CSIB_ADDR_LO_BASE_IDX, GC_mmRLC_CSIB_ADDR_LO, rlc.clear_state_gpu_addr as u32 & 0xfffffffc);
    gc_wr(adev, GC_mmRLC_CSIB_LENGTH_BASE_IDX, GC_mmRLC_CSIB_LENGTH, rlc.clear_state_size);
}

/// `gfx_v9_0_init_gfx_power_gating`. GC 9.3.0 skips
/// `pwr_10_0_gfxip_control_over_cgpg`.
fn gfx_v9_0_init_gfx_power_gating(adev: &amdgpu_device) {
    if adev.pg_flags & (AMD_PG_SUPPORT_GFX_PG | AMD_PG_SUPPORT_GFX_SMG | AMD_PG_SUPPORT_GFX_DMG) == 0 {
        return;
    }
    // init IDLE_POLL_COUNT = 60
    let mut data = gc_rd(adev, GC_mmCP_RB_WPTR_POLL_CNTL_BASE_IDX, GC_mmCP_RB_WPTR_POLL_CNTL);
    data &= !GC_CP_RB_WPTR_POLL_CNTL__IDLE_POLL_COUNT_MASK;
    data |= 0x60 << GC_CP_RB_WPTR_POLL_CNTL__IDLE_POLL_COUNT__SHIFT;
    gc_wr(adev, GC_mmCP_RB_WPTR_POLL_CNTL_BASE_IDX, GC_mmCP_RB_WPTR_POLL_CNTL, data);

    // init RLC PG Delay
    let mut data = 0;
    data |= 0x10 << GC_RLC_PG_DELAY__POWER_UP_DELAY__SHIFT;
    data |= 0x10 << GC_RLC_PG_DELAY__POWER_DOWN_DELAY__SHIFT;
    data |= 0x10 << GC_RLC_PG_DELAY__CMD_PROPAGATE_DELAY__SHIFT;
    data |= 0x40 << GC_RLC_PG_DELAY__MEM_SLEEP_DELAY__SHIFT;
    gc_wr(adev, GC_mmRLC_PG_DELAY_BASE_IDX, GC_mmRLC_PG_DELAY, data);

    let mut data = gc_rd(adev, GC_mmRLC_PG_DELAY_2_BASE_IDX, GC_mmRLC_PG_DELAY_2);
    data &= !GC_RLC_PG_DELAY_2__SERDES_CMD_DELAY_MASK;
    data |= 0x4 << GC_RLC_PG_DELAY_2__SERDES_CMD_DELAY__SHIFT;
    gc_wr(adev, GC_mmRLC_PG_DELAY_2_BASE_IDX, GC_mmRLC_PG_DELAY_2, data);

    let mut data = gc_rd(adev, GC_mmRLC_PG_DELAY_3_BASE_IDX, GC_mmRLC_PG_DELAY_3);
    data &= !GC_RLC_PG_DELAY_3__CGCG_ACTIVE_BEFORE_CGPG_MASK;
    data |= 0xff << GC_RLC_PG_DELAY_3__CGCG_ACTIVE_BEFORE_CGPG__SHIFT;
    gc_wr(adev, GC_mmRLC_PG_DELAY_3_BASE_IDX, GC_mmRLC_PG_DELAY_3, data);

    let mut data = gc_rd(adev, GC_mmRLC_AUTO_PG_CTRL_BASE_IDX, GC_mmRLC_AUTO_PG_CTRL);
    data &= !GC_RLC_AUTO_PG_CTRL__GRBM_REG_SAVE_GFX_IDLE_THRESHOLD_MASK;
    // program GRBM_REG_SAVE_GFX_IDLE_THRESHOLD to 0x55f0
    data |= 0x55f0 << GC_RLC_AUTO_PG_CTRL__GRBM_REG_SAVE_GFX_IDLE_THRESHOLD__SHIFT;
    gc_wr(adev, GC_mmRLC_AUTO_PG_CTRL_BASE_IDX, GC_mmRLC_AUTO_PG_CTRL, data);
}

/// `gfx_v9_0_init_pg`. GC 9.3.0 is neither 9.2.1 nor Raven2, so no
/// `gfx_v9_1_init_rlc_save_restore_list`.
fn gfx_v9_0_init_pg(adev: &amdgpu_device) {
    gfx_v9_0_init_csb(adev);
    // Rlc save restore list is workable since v2_1. And it's needed by
    // gfxoff feature.
    if adev.gfx.rlc.is_rlc_v2_1 {
        // gfx_v9_0_enable_save_restore_machine
        gc_field15(adev, GC_mmRLC_SRM_CNTL_BASE_IDX, GC_mmRLC_SRM_CNTL, GC_RLC_SRM_CNTL__SRM_ENABLE_MASK,
                   GC_RLC_SRM_CNTL__SRM_ENABLE__SHIFT, 1);
    }
    if adev.pg_flags
        & (AMD_PG_SUPPORT_GFX_PG | AMD_PG_SUPPORT_GFX_SMG | AMD_PG_SUPPORT_GFX_DMG | AMD_PG_SUPPORT_CP
            | AMD_PG_SUPPORT_GDS | AMD_PG_SUPPORT_RLC_SMU_HS)
        != 0
    {
        gc_wr(adev, GC_mmRLC_JUMP_TABLE_RESTORE_BASE_IDX, GC_mmRLC_JUMP_TABLE_RESTORE,
              (adev.gfx.rlc.cp_table_gpu_addr >> 8) as u32);
        gfx_v9_0_init_gfx_power_gating(adev);
    }
}

/// `gfx_v9_0_update_spm_vmid_internal`: not one VF.
fn gfx_v9_0_update_spm_vmid_internal(adev: &amdgpu_device, vmid: u32) {
    let mut data = gc_rd(adev, GC_mmRLC_SPM_MC_CNTL_BASE_IDX, GC_mmRLC_SPM_MC_CNTL);
    data &= !GC_RLC_SPM_MC_CNTL__RLC_SPM_VMID_MASK;
    data |= (vmid & GC_RLC_SPM_MC_CNTL__RLC_SPM_VMID_MASK) << GC_RLC_SPM_MC_CNTL__RLC_SPM_VMID__SHIFT;
    gc_wr(adev, GC_mmRLC_SPM_MC_CNTL_BASE_IDX, GC_mmRLC_SPM_MC_CNTL, data);
}

/// `gfx_v9_0_rlc_resume`: the PSP loaded the RLC, and GC 9.3.0 has no
/// LBPW.
fn gfx_v9_0_rlc_resume(adev: &amdgpu_device) -> i32 {
    gfx_v9_0_rlc_stop(adev);
    // disable CG
    gc_wr(adev, GC_mmRLC_CGCG_CGLS_CTRL_BASE_IDX, GC_mmRLC_CGCG_CGLS_CTRL, 0);
    gfx_v9_0_init_pg(adev);
    gfx_v9_0_update_spm_vmid_internal(adev, 0xf);
    gfx_v9_0_rlc_start(adev);
    0
}

/// `gfx_v9_0_cp_gfx_enable`.
fn gfx_v9_0_cp_gfx_enable(adev: &amdgpu_device, enable: bool) {
    let d = (!enable) as u32;
    let mut tmp = gc_rd(adev, GC_mmCP_ME_CNTL_BASE_IDX, GC_mmCP_ME_CNTL);
    for (mask, shift) in [
        (GC_CP_ME_CNTL__CE_INVALIDATE_ICACHE_MASK, GC_CP_ME_CNTL__CE_INVALIDATE_ICACHE__SHIFT),
        (GC_CP_ME_CNTL__PFP_INVALIDATE_ICACHE_MASK, GC_CP_ME_CNTL__PFP_INVALIDATE_ICACHE__SHIFT),
        (GC_CP_ME_CNTL__ME_INVALIDATE_ICACHE_MASK, GC_CP_ME_CNTL__ME_INVALIDATE_ICACHE__SHIFT),
        (GC_CP_ME_CNTL__CE_PIPE0_RESET_MASK, GC_CP_ME_CNTL__CE_PIPE0_RESET__SHIFT),
        (GC_CP_ME_CNTL__CE_PIPE1_RESET_MASK, GC_CP_ME_CNTL__CE_PIPE1_RESET__SHIFT),
        (GC_CP_ME_CNTL__PFP_PIPE0_RESET_MASK, GC_CP_ME_CNTL__PFP_PIPE0_RESET__SHIFT),
        (GC_CP_ME_CNTL__PFP_PIPE1_RESET_MASK, GC_CP_ME_CNTL__PFP_PIPE1_RESET__SHIFT),
        (GC_CP_ME_CNTL__ME_PIPE0_RESET_MASK, GC_CP_ME_CNTL__ME_PIPE0_RESET__SHIFT),
        (GC_CP_ME_CNTL__ME_PIPE1_RESET_MASK, GC_CP_ME_CNTL__ME_PIPE1_RESET__SHIFT),
        (GC_CP_ME_CNTL__ME_HALT_MASK, GC_CP_ME_CNTL__ME_HALT__SHIFT),
        (GC_CP_ME_CNTL__PFP_HALT_MASK, GC_CP_ME_CNTL__PFP_HALT__SHIFT),
        (GC_CP_ME_CNTL__CE_HALT_MASK, GC_CP_ME_CNTL__CE_HALT__SHIFT),
    ] {
        tmp = set_field(tmp, mask, shift, d);
    }
    gc_wr(adev, GC_mmCP_ME_CNTL_BASE_IDX, GC_mmCP_ME_CNTL, tmp);
    crate::linux::udelay(50);
}

/// `gfx_v9_0_cp_compute_enable`.
fn gfx_v9_0_cp_compute_enable(adev: &mut amdgpu_device, enable: bool) {
    if enable {
        gc_wr(adev, GC_mmCP_MEC_CNTL_BASE_IDX, GC_mmCP_MEC_CNTL, 0);
    } else {
        gc_wr(adev, GC_mmCP_MEC_CNTL_BASE_IDX, GC_mmCP_MEC_CNTL,
              GC_CP_MEC_CNTL__MEC_INVALIDATE_ICACHE_MASK
                  | GC_CP_MEC_CNTL__MEC_ME1_PIPE0_RESET_MASK
                  | GC_CP_MEC_CNTL__MEC_ME1_PIPE1_RESET_MASK
                  | GC_CP_MEC_CNTL__MEC_ME1_PIPE2_RESET_MASK
                  | GC_CP_MEC_CNTL__MEC_ME1_PIPE3_RESET_MASK
                  | GC_CP_MEC_CNTL__MEC_ME2_PIPE0_RESET_MASK
                  | GC_CP_MEC_CNTL__MEC_ME2_PIPE1_RESET_MASK
                  | GC_CP_MEC_CNTL__MEC_ME1_HALT_MASK
                  | GC_CP_MEC_CNTL__MEC_ME2_HALT_MASK);
        if let Some(k) = adev.gfx.kiq.ring {
            adev.rings[k].sched.ready = false;
        }
    }
    crate::linux::udelay(50);
}

/// `order_base_2`.
fn order_base_2(n: u32) -> u32 {
    if n <= 1 { 0 } else { 32 - (n - 1).leading_zeros() }
}

/// `gfx_v9_0_cp_gfx_start`: not resuming from S3.
fn gfx_v9_0_cp_gfx_start(adev: &mut amdgpu_device) -> i32 {
    let ring = adev.gfx.gfx_ring[0];
    // init the CP
    gc_wr(adev, GC_mmCP_MAX_CONTEXT_BASE_IDX, GC_mmCP_MAX_CONTEXT, adev.gfx.config.max_hw_contexts - 1);
    gc_wr(adev, GC_mmCP_DEVICE_ID_BASE_IDX, GC_mmCP_DEVICE_ID, 1);
    gfx_v9_0_cp_gfx_enable(adev, true);

    let r = crate::amdgpu_ring::amdgpu_ring_alloc(adev, ring, gfx_v9_0_get_csb_size() + 4 + 3);
    if r != 0 {
        Line::new().s("amdgpu: cp failed to lock ring (").dec((-r) as u64).s(").").say();
        return r;
    }
    let w = |adev: &mut amdgpu_device, v: u32| crate::amdgpu_ring::amdgpu_ring_write(adev, ring, v);
    w(adev, PACKET3(PACKET3_PREAMBLE_CNTL, 0));
    w(adev, PACKET3_PREAMBLE_BEGIN_CLEAR_STATE);
    w(adev, PACKET3(PACKET3_CONTEXT_CONTROL, 1));
    w(adev, 0x80000000);
    w(adev, 0x80000000);
    for (extent, reg_index, reg_count) in GFX9_SECT_CONTEXT_DEFS.iter() {
        w(adev, PACKET3(PACKET3_SET_CONTEXT_REG, *reg_count));
        w(adev, reg_index - PACKET3_SET_CONTEXT_REG_START);
        for i in 0..*reg_count as usize {
            w(adev, extent[i]);
        }
    }
    w(adev, PACKET3(PACKET3_PREAMBLE_CNTL, 0));
    w(adev, PACKET3_PREAMBLE_END_CLEAR_STATE);
    w(adev, PACKET3(PACKET3_CLEAR_STATE, 0));
    w(adev, 0);
    w(adev, PACKET3(PACKET3_SET_BASE, 2));
    w(adev, PACKET3_BASE_INDEX_CE_PARTITION_BASE);
    w(adev, 0x8000);
    w(adev, 0x8000);
    w(adev, PACKET3(PACKET3_SET_UCONFIG_REG, 1));
    let vgt_index_type = adev.reg_offset[GC_HWIP as usize][0][GC_mmVGT_INDEX_TYPE_BASE_IDX as usize]
        + GC_mmVGT_INDEX_TYPE;
    w(adev, PACKET3_SET_UCONFIG_REG_INDEX_TYPE | (vgt_index_type - PACKET3_SET_UCONFIG_REG_START));
    w(adev, 0);
    crate::amdgpu_ring::amdgpu_ring_commit(adev, ring);
    0
}

/// `gfx_v9_0_cp_gfx_resume`.
fn gfx_v9_0_cp_gfx_resume(adev: &mut amdgpu_device) -> i32 {
    // Set the write pointer delay
    gc_wr(adev, GC_mmCP_RB_WPTR_DELAY_BASE_IDX, GC_mmCP_RB_WPTR_DELAY, 0);
    // set the RB to use vmid 0
    gc_wr(adev, GC_mmCP_RB_VMID_BASE_IDX, GC_mmCP_RB_VMID, 0);

    // Set ring buffer size
    let r = adev.gfx.gfx_ring[0];
    let rb_bufsz = order_base_2(adev.rings[r].ring_size / 8);
    let mut tmp = set_field(0, GC_CP_RB0_CNTL__RB_BUFSZ_MASK, GC_CP_RB0_CNTL__RB_BUFSZ__SHIFT, rb_bufsz);
    tmp = set_field(tmp, GC_CP_RB0_CNTL__RB_BLKSZ_MASK, GC_CP_RB0_CNTL__RB_BLKSZ__SHIFT, rb_bufsz - 2);
    gc_wr(adev, GC_mmCP_RB0_CNTL_BASE_IDX, GC_mmCP_RB0_CNTL, tmp);

    // Initialize the ring buffer's write pointers
    adev.rings[r].wptr = 0;
    gc_wr(adev, GC_mmCP_RB0_WPTR_BASE_IDX, GC_mmCP_RB0_WPTR, 0);
    gc_wr(adev, GC_mmCP_RB0_WPTR_HI_BASE_IDX, GC_mmCP_RB0_WPTR_HI, 0);

    // set the wb address whether it's enabled or not
    let ring = &adev.rings[r];
    let rptr_addr = ring.rptr_gpu_addr;
    gc_wr(adev, GC_mmCP_RB0_RPTR_ADDR_BASE_IDX, GC_mmCP_RB0_RPTR_ADDR, rptr_addr as u32);
    gc_wr(adev, GC_mmCP_RB0_RPTR_ADDR_HI_BASE_IDX, GC_mmCP_RB0_RPTR_ADDR_HI,
          (rptr_addr >> 32) as u32 & GC_CP_RB_RPTR_ADDR_HI__RB_RPTR_ADDR_HI_MASK);

    let wptr_gpu_addr = ring.wptr_gpu_addr;
    gc_wr(adev, GC_mmCP_RB_WPTR_POLL_ADDR_LO_BASE_IDX, GC_mmCP_RB_WPTR_POLL_ADDR_LO, wptr_gpu_addr as u32);
    gc_wr(adev, GC_mmCP_RB_WPTR_POLL_ADDR_HI_BASE_IDX, GC_mmCP_RB_WPTR_POLL_ADDR_HI, (wptr_gpu_addr >> 32) as u32);

    crate::linux::mdelay(1);
    gc_wr(adev, GC_mmCP_RB0_CNTL_BASE_IDX, GC_mmCP_RB0_CNTL, tmp);

    let rb_addr = ring.gpu_addr >> 8;
    gc_wr(adev, GC_mmCP_RB0_BASE_BASE_IDX, GC_mmCP_RB0_BASE, rb_addr as u32);
    gc_wr(adev, GC_mmCP_RB0_BASE_HI_BASE_IDX, GC_mmCP_RB0_BASE_HI, (rb_addr >> 32) as u32);

    let mut tmp = gc_rd(adev, GC_mmCP_RB_DOORBELL_CONTROL_BASE_IDX, GC_mmCP_RB_DOORBELL_CONTROL);
    if ring.use_doorbell {
        tmp = set_field(tmp, GC_CP_RB_DOORBELL_CONTROL__DOORBELL_OFFSET_MASK,
                        GC_CP_RB_DOORBELL_CONTROL__DOORBELL_OFFSET__SHIFT, ring.doorbell_index);
        tmp = set_field(tmp, GC_CP_RB_DOORBELL_CONTROL__DOORBELL_EN_MASK, GC_CP_RB_DOORBELL_CONTROL__DOORBELL_EN__SHIFT,
                        1);
    } else {
        tmp = set_field(tmp, GC_CP_RB_DOORBELL_CONTROL__DOORBELL_EN_MASK, GC_CP_RB_DOORBELL_CONTROL__DOORBELL_EN__SHIFT,
                        0);
    }
    gc_wr(adev, GC_mmCP_RB_DOORBELL_CONTROL_BASE_IDX, GC_mmCP_RB_DOORBELL_CONTROL, tmp);

    let tmp = set_field(0, GC_CP_RB_DOORBELL_RANGE_LOWER__DOORBELL_RANGE_LOWER_MASK,
                        GC_CP_RB_DOORBELL_RANGE_LOWER__DOORBELL_RANGE_LOWER__SHIFT, ring.doorbell_index);
    gc_wr(adev, GC_mmCP_RB_DOORBELL_RANGE_LOWER_BASE_IDX, GC_mmCP_RB_DOORBELL_RANGE_LOWER, tmp);
    gc_wr(adev, GC_mmCP_RB_DOORBELL_RANGE_UPPER_BASE_IDX, GC_mmCP_RB_DOORBELL_RANGE_UPPER,
          GC_CP_RB_DOORBELL_RANGE_UPPER__DOORBELL_RANGE_UPPER_MASK);

    // start the ring
    gfx_v9_0_cp_gfx_start(adev);
    0
}

/// `gfx_v9_0_kiq_setting`.
fn gfx_v9_0_kiq_setting(adev: &amdgpu_device, r: usize) {
    let ring = &adev.rings[r];
    // tell RLC which is KIQ queue
    let mut tmp = gc_rd(adev, GC_mmRLC_CP_SCHEDULERS_BASE_IDX, GC_mmRLC_CP_SCHEDULERS);
    tmp &= 0xffffff00;
    tmp |= (ring.me << 5) | (ring.pipe << 3) | ring.queue;
    gc_wr(adev, GC_mmRLC_CP_SCHEDULERS_BASE_IDX, GC_mmRLC_CP_SCHEDULERS, tmp | 0x80);
}

/// A `struct v9_mqd` being built: dwords, indexed by `offsetof / 4`.
struct v9_mqd(Vec<u32>);

impl v9_mqd {
    fn new() -> Self {
        v9_mqd(vec![0; SIZEOF_v9_mqd_allocation as usize / 4])
    }
    fn set(&mut self, off: u32, v: u32) {
        self.0[(off / 4) as usize] = v;
    }
    fn get(&self, off: u32) -> u32 {
        self.0[(off / 4) as usize]
    }
    fn bytes(&self) -> Vec<u8> {
        self.0.iter().flat_map(|w| w.to_le_bytes()).collect()
    }
}

/// `amdgpu_gfx_is_high_priority_compute_queue`.
fn amdgpu_gfx_is_high_priority_compute_queue(adev: &amdgpu_device, r: usize) -> bool {
    // Policy: use 1st queue as high priority compute queue if we have more
    // than one compute queue.
    adev.gfx.num_compute_rings > 1 && adev.gfx.compute_ring.first() == Some(&r)
}

/// `gfx_v9_0_mqd_init`.
fn gfx_v9_0_mqd_init(adev: &mut amdgpu_device, r: usize, mqd: &mut v9_mqd) {
    mqd.set(V9_MQD_header, 0xC0310800);
    mqd.set(V9_MQD_compute_pipelinestat_enable, 0x00000001);
    for off in [
        V9_MQD_compute_static_thread_mgmt_se0, V9_MQD_compute_static_thread_mgmt_se1,
        V9_MQD_compute_static_thread_mgmt_se2, V9_MQD_compute_static_thread_mgmt_se3,
        V9_MQD_compute_static_thread_mgmt_se4, V9_MQD_compute_static_thread_mgmt_se5,
        V9_MQD_compute_static_thread_mgmt_se6, V9_MQD_compute_static_thread_mgmt_se7,
    ] {
        mqd.set(off, 0xffffffff);
    }
    mqd.set(V9_MQD_compute_misc_reserved, 0x00000003);

    let ring = &adev.rings[r];
    let cu_mask_addr = ring.mqd_gpu_addr + V9_MQD_ALLOCATION_dynamic_cu_mask as u64;
    mqd.set(V9_MQD_dynamic_cu_mask_addr_lo, cu_mask_addr as u32);
    mqd.set(V9_MQD_dynamic_cu_mask_addr_hi, (cu_mask_addr >> 32) as u32);

    let eop_base_addr = ring.eop_gpu_addr >> 8;
    mqd.set(V9_MQD_cp_hqd_eop_base_addr_lo, eop_base_addr as u32);
    mqd.set(V9_MQD_cp_hqd_eop_base_addr_hi, (eop_base_addr >> 32) as u32);

    // set the EOP size, register value is 2^(EOP_SIZE+1) dwords
    let mut tmp = gc_rd(adev, GC_mmCP_HQD_EOP_CONTROL_BASE_IDX, GC_mmCP_HQD_EOP_CONTROL);
    tmp = set_field(tmp, GC_CP_HQD_EOP_CONTROL__EOP_SIZE_MASK, GC_CP_HQD_EOP_CONTROL__EOP_SIZE__SHIFT,
                    order_base_2(GFX9_MEC_HPD_SIZE / 4) - 1);
    mqd.set(V9_MQD_cp_hqd_eop_control, tmp);

    // enable doorbell?
    let mut tmp = gc_rd(adev, GC_mmCP_HQD_PQ_DOORBELL_CONTROL_BASE_IDX, GC_mmCP_HQD_PQ_DOORBELL_CONTROL);
    if ring.use_doorbell {
        tmp = set_field(tmp, GC_CP_HQD_PQ_DOORBELL_CONTROL__DOORBELL_OFFSET_MASK,
                        GC_CP_HQD_PQ_DOORBELL_CONTROL__DOORBELL_OFFSET__SHIFT, ring.doorbell_index);
        tmp = set_field(tmp, GC_CP_HQD_PQ_DOORBELL_CONTROL__DOORBELL_EN_MASK,
                        GC_CP_HQD_PQ_DOORBELL_CONTROL__DOORBELL_EN__SHIFT, 1);
        tmp = set_field(tmp, GC_CP_HQD_PQ_DOORBELL_CONTROL__DOORBELL_SOURCE_MASK,
                        GC_CP_HQD_PQ_DOORBELL_CONTROL__DOORBELL_SOURCE__SHIFT, 0);
        tmp = set_field(tmp, GC_CP_HQD_PQ_DOORBELL_CONTROL__DOORBELL_HIT_MASK,
                        GC_CP_HQD_PQ_DOORBELL_CONTROL__DOORBELL_HIT__SHIFT, 0);
    } else {
        tmp = set_field(tmp, GC_CP_HQD_PQ_DOORBELL_CONTROL__DOORBELL_EN_MASK,
                        GC_CP_HQD_PQ_DOORBELL_CONTROL__DOORBELL_EN__SHIFT, 0);
    }
    mqd.set(V9_MQD_cp_hqd_pq_doorbell_control, tmp);

    // disable the queue if it's active
    let (mqd_gpu_addr, gpu_addr, ring_size, rptr_gpu_addr, wptr_gpu_addr, type_) =
        (ring.mqd_gpu_addr, ring.gpu_addr, ring.ring_size, ring.rptr_gpu_addr, ring.wptr_gpu_addr, ring.funcs.type_);
    adev.rings[r].wptr = 0;
    mqd.set(V9_MQD_cp_hqd_dequeue_request, 0);
    mqd.set(V9_MQD_cp_hqd_pq_rptr, 0);
    mqd.set(V9_MQD_cp_hqd_pq_wptr_lo, 0);
    mqd.set(V9_MQD_cp_hqd_pq_wptr_hi, 0);

    // set the pointer to the MQD
    mqd.set(V9_MQD_cp_mqd_base_addr_lo, mqd_gpu_addr as u32 & 0xfffffffc);
    mqd.set(V9_MQD_cp_mqd_base_addr_hi, (mqd_gpu_addr >> 32) as u32);

    // set MQD vmid to 0
    let mut tmp = gc_rd(adev, GC_mmCP_MQD_CONTROL_BASE_IDX, GC_mmCP_MQD_CONTROL);
    tmp = set_field(tmp, GC_CP_MQD_CONTROL__VMID_MASK, GC_CP_MQD_CONTROL__VMID__SHIFT, 0);
    mqd.set(V9_MQD_cp_mqd_control, tmp);

    // set the pointer to the HQD, this is similar CP_RB0_BASE/_HI
    let hqd_gpu_addr = gpu_addr >> 8;
    mqd.set(V9_MQD_cp_hqd_pq_base_lo, hqd_gpu_addr as u32);
    mqd.set(V9_MQD_cp_hqd_pq_base_hi, (hqd_gpu_addr >> 32) as u32);

    // set up the HQD, this is similar to CP_RB0_CNTL
    let mut tmp = gc_rd(adev, GC_mmCP_HQD_PQ_CONTROL_BASE_IDX, GC_mmCP_HQD_PQ_CONTROL);
    tmp = set_field(tmp, GC_CP_HQD_PQ_CONTROL__QUEUE_SIZE_MASK, GC_CP_HQD_PQ_CONTROL__QUEUE_SIZE__SHIFT,
                    order_base_2(ring_size / 4) - 1);
    tmp = set_field(tmp, GC_CP_HQD_PQ_CONTROL__RPTR_BLOCK_SIZE_MASK, GC_CP_HQD_PQ_CONTROL__RPTR_BLOCK_SIZE__SHIFT,
                    order_base_2(AMDGPU_GPU_PAGE_SIZE / 4) - 1);
    tmp = set_field(tmp, GC_CP_HQD_PQ_CONTROL__UNORD_DISPATCH_MASK, GC_CP_HQD_PQ_CONTROL__UNORD_DISPATCH__SHIFT, 0);
    tmp = set_field(tmp, GC_CP_HQD_PQ_CONTROL__ROQ_PQ_IB_FLIP_MASK, GC_CP_HQD_PQ_CONTROL__ROQ_PQ_IB_FLIP__SHIFT, 0);
    tmp = set_field(tmp, GC_CP_HQD_PQ_CONTROL__PRIV_STATE_MASK, GC_CP_HQD_PQ_CONTROL__PRIV_STATE__SHIFT, 1);
    tmp = set_field(tmp, GC_CP_HQD_PQ_CONTROL__KMD_QUEUE_MASK, GC_CP_HQD_PQ_CONTROL__KMD_QUEUE__SHIFT, 1);
    mqd.set(V9_MQD_cp_hqd_pq_control, tmp);

    // set the wb address whether it's enabled or not
    mqd.set(V9_MQD_cp_hqd_pq_rptr_report_addr_lo, rptr_gpu_addr as u32 & 0xfffffffc);
    mqd.set(V9_MQD_cp_hqd_pq_rptr_report_addr_hi, (rptr_gpu_addr >> 32) as u32 & 0xffff);

    // only used if CP_PQ_WPTR_POLL_CNTL.CP_PQ_WPTR_POLL_CNTL__EN_MASK=1
    mqd.set(V9_MQD_cp_hqd_pq_wptr_poll_addr_lo, wptr_gpu_addr as u32 & 0xfffffffc);
    mqd.set(V9_MQD_cp_hqd_pq_wptr_poll_addr_hi, (wptr_gpu_addr >> 32) as u32 & 0xffff);

    // reset read and write pointers, similar to CP_RB0_WPTR/_RPTR
    adev.rings[r].wptr = 0;
    mqd.set(V9_MQD_cp_hqd_pq_rptr, gc_rd(adev, GC_mmCP_HQD_PQ_RPTR_BASE_IDX, GC_mmCP_HQD_PQ_RPTR));

    // set the vmid for the queue
    mqd.set(V9_MQD_cp_hqd_vmid, 0);

    let mut tmp = gc_rd(adev, GC_mmCP_HQD_PERSISTENT_STATE_BASE_IDX, GC_mmCP_HQD_PERSISTENT_STATE);
    tmp = set_field(tmp, GC_CP_HQD_PERSISTENT_STATE__PRELOAD_SIZE_MASK,
                    GC_CP_HQD_PERSISTENT_STATE__PRELOAD_SIZE__SHIFT, 0x53);
    mqd.set(V9_MQD_cp_hqd_persistent_state, tmp);

    // set MIN_IB_AVAIL_SIZE
    let mut tmp = gc_rd(adev, GC_mmCP_HQD_IB_CONTROL_BASE_IDX, GC_mmCP_HQD_IB_CONTROL);
    tmp = set_field(tmp, GC_CP_HQD_IB_CONTROL__MIN_IB_AVAIL_SIZE_MASK, GC_CP_HQD_IB_CONTROL__MIN_IB_AVAIL_SIZE__SHIFT,
                    3);
    mqd.set(V9_MQD_cp_hqd_ib_control, tmp);

    // set static priority for a queue/ring: gfx_v9_0_mqd_set_priority
    if type_ == AMDGPU_RING_TYPE_COMPUTE && amdgpu_gfx_is_high_priority_compute_queue(adev, r) {
        mqd.set(V9_MQD_cp_hqd_pipe_priority, AMDGPU_GFX_PIPE_PRIO_HIGH);
        mqd.set(V9_MQD_cp_hqd_queue_priority, AMDGPU_GFX_QUEUE_PRIORITY_MAXIMUM);
    }
    mqd.set(V9_MQD_cp_hqd_quantum, gc_rd(adev, GC_mmCP_HQD_QUANTUM_BASE_IDX, GC_mmCP_HQD_QUANTUM));

    // map_queues packet doesn't need activate the queue, so only kiq need
    // set this field.
    if type_ == AMDGPU_RING_TYPE_KIQ {
        mqd.set(V9_MQD_cp_hqd_active, 1);
    }
}

/// `check_if_enlarge_doorbell_range`.
fn check_if_enlarge_doorbell_range(adev: &amdgpu_device) -> bool {
    adev.ip_version(GC_HWIP, 0) == 9 << 24 | 3 << 16
        && adev.gfx.me_fw_version >= 0x000000a5
        && adev.gfx.me_feature_version >= 52
}

/// `gfx_v9_0_kiq_init_register`.
fn gfx_v9_0_kiq_init_register(adev: &amdgpu_device, r: usize, mqd: &v9_mqd) {
    let ring = &adev.rings[r];
    // disable wptr polling
    gc_field15(adev, GC_mmCP_PQ_WPTR_POLL_CNTL_BASE_IDX, GC_mmCP_PQ_WPTR_POLL_CNTL, GC_CP_PQ_WPTR_POLL_CNTL__EN_MASK,
               GC_CP_PQ_WPTR_POLL_CNTL__EN__SHIFT, 0);
    gc_wr(adev, GC_mmCP_HQD_EOP_BASE_ADDR_BASE_IDX, GC_mmCP_HQD_EOP_BASE_ADDR, mqd.get(V9_MQD_cp_hqd_eop_base_addr_lo));
    gc_wr(adev, GC_mmCP_HQD_EOP_BASE_ADDR_HI_BASE_IDX, GC_mmCP_HQD_EOP_BASE_ADDR_HI,
          mqd.get(V9_MQD_cp_hqd_eop_base_addr_hi));
    // set the EOP size, register value is 2^(EOP_SIZE+1) dwords
    gc_wr(adev, GC_mmCP_HQD_EOP_CONTROL_BASE_IDX, GC_mmCP_HQD_EOP_CONTROL, mqd.get(V9_MQD_cp_hqd_eop_control));
    // enable doorbell?
    gc_wr(adev, GC_mmCP_HQD_PQ_DOORBELL_CONTROL_BASE_IDX, GC_mmCP_HQD_PQ_DOORBELL_CONTROL,
          mqd.get(V9_MQD_cp_hqd_pq_doorbell_control));

    // disable the queue if it's active
    if gc_rd(adev, GC_mmCP_HQD_ACTIVE_BASE_IDX, GC_mmCP_HQD_ACTIVE) & 1 != 0 {
        gc_wr(adev, GC_mmCP_HQD_DEQUEUE_REQUEST_BASE_IDX, GC_mmCP_HQD_DEQUEUE_REQUEST, 1);
        for _ in 0..adev.usec_timeout {
            if gc_rd(adev, GC_mmCP_HQD_ACTIVE_BASE_IDX, GC_mmCP_HQD_ACTIVE) & 1 == 0 {
                break;
            }
            crate::linux::udelay(1);
        }
        gc_wr(adev, GC_mmCP_HQD_DEQUEUE_REQUEST_BASE_IDX, GC_mmCP_HQD_DEQUEUE_REQUEST,
              mqd.get(V9_MQD_cp_hqd_dequeue_request));
        gc_wr(adev, GC_mmCP_HQD_PQ_RPTR_BASE_IDX, GC_mmCP_HQD_PQ_RPTR, mqd.get(V9_MQD_cp_hqd_pq_rptr));
        gc_wr(adev, GC_mmCP_HQD_PQ_WPTR_LO_BASE_IDX, GC_mmCP_HQD_PQ_WPTR_LO, mqd.get(V9_MQD_cp_hqd_pq_wptr_lo));
        gc_wr(adev, GC_mmCP_HQD_PQ_WPTR_HI_BASE_IDX, GC_mmCP_HQD_PQ_WPTR_HI, mqd.get(V9_MQD_cp_hqd_pq_wptr_hi));
    }

    // set the pointer to the MQD
    gc_wr(adev, GC_mmCP_MQD_BASE_ADDR_BASE_IDX, GC_mmCP_MQD_BASE_ADDR, mqd.get(V9_MQD_cp_mqd_base_addr_lo));
    gc_wr(adev, GC_mmCP_MQD_BASE_ADDR_HI_BASE_IDX, GC_mmCP_MQD_BASE_ADDR_HI, mqd.get(V9_MQD_cp_mqd_base_addr_hi));
    // set MQD vmid to 0
    gc_wr(adev, GC_mmCP_MQD_CONTROL_BASE_IDX, GC_mmCP_MQD_CONTROL, mqd.get(V9_MQD_cp_mqd_control));
    // set the pointer to the HQD, this is similar CP_RB0_BASE/_HI
    gc_wr(adev, GC_mmCP_HQD_PQ_BASE_BASE_IDX, GC_mmCP_HQD_PQ_BASE, mqd.get(V9_MQD_cp_hqd_pq_base_lo));
    gc_wr(adev, GC_mmCP_HQD_PQ_BASE_HI_BASE_IDX, GC_mmCP_HQD_PQ_BASE_HI, mqd.get(V9_MQD_cp_hqd_pq_base_hi));
    // set up the HQD, this is similar to CP_RB0_CNTL
    gc_wr(adev, GC_mmCP_HQD_PQ_CONTROL_BASE_IDX, GC_mmCP_HQD_PQ_CONTROL, mqd.get(V9_MQD_cp_hqd_pq_control));
    // set the wb address whether it's enabled or not
    gc_wr(adev, GC_mmCP_HQD_PQ_RPTR_REPORT_ADDR_BASE_IDX, GC_mmCP_HQD_PQ_RPTR_REPORT_ADDR,
          mqd.get(V9_MQD_cp_hqd_pq_rptr_report_addr_lo));
    gc_wr(adev, GC_mmCP_HQD_PQ_RPTR_REPORT_ADDR_HI_BASE_IDX, GC_mmCP_HQD_PQ_RPTR_REPORT_ADDR_HI,
          mqd.get(V9_MQD_cp_hqd_pq_rptr_report_addr_hi));
    // only used if CP_PQ_WPTR_POLL_CNTL.CP_PQ_WPTR_POLL_CNTL__EN_MASK=1
    gc_wr(adev, GC_mmCP_HQD_PQ_WPTR_POLL_ADDR_BASE_IDX, GC_mmCP_HQD_PQ_WPTR_POLL_ADDR,
          mqd.get(V9_MQD_cp_hqd_pq_wptr_poll_addr_lo));
    gc_wr(adev, GC_mmCP_HQD_PQ_WPTR_POLL_ADDR_HI_BASE_IDX, GC_mmCP_HQD_PQ_WPTR_POLL_ADDR_HI,
          mqd.get(V9_MQD_cp_hqd_pq_wptr_poll_addr_hi));

    // enable the doorbell if requested
    if ring.use_doorbell {
        gc_wr(adev, GC_mmCP_MEC_DOORBELL_RANGE_LOWER_BASE_IDX, GC_mmCP_MEC_DOORBELL_RANGE_LOWER,
              (adev.doorbell_index.kiq * 2) << 2);
        // If GC has entered CGPG, ringing doorbell > first page doesn't
        // wakeup GC. Enlarge CP_MEC_DOORBELL_RANGE_UPPER to workaround this
        // issue. And this change has to align with firmware update.
        if check_if_enlarge_doorbell_range(adev) {
            gc_wr(adev, GC_mmCP_MEC_DOORBELL_RANGE_UPPER_BASE_IDX, GC_mmCP_MEC_DOORBELL_RANGE_UPPER,
                  adev.doorbell.size as u32 - 4);
        } else {
            gc_wr(adev, GC_mmCP_MEC_DOORBELL_RANGE_UPPER_BASE_IDX, GC_mmCP_MEC_DOORBELL_RANGE_UPPER,
                  (adev.doorbell_index.userqueue_end * 2) << 2);
        }
    }

    gc_wr(adev, GC_mmCP_HQD_PQ_DOORBELL_CONTROL_BASE_IDX, GC_mmCP_HQD_PQ_DOORBELL_CONTROL,
          mqd.get(V9_MQD_cp_hqd_pq_doorbell_control));
    // reset read and write pointers, similar to CP_RB0_WPTR/_RPTR
    gc_wr(adev, GC_mmCP_HQD_PQ_WPTR_LO_BASE_IDX, GC_mmCP_HQD_PQ_WPTR_LO, mqd.get(V9_MQD_cp_hqd_pq_wptr_lo));
    gc_wr(adev, GC_mmCP_HQD_PQ_WPTR_HI_BASE_IDX, GC_mmCP_HQD_PQ_WPTR_HI, mqd.get(V9_MQD_cp_hqd_pq_wptr_hi));
    // set the vmid for the queue
    gc_wr(adev, GC_mmCP_HQD_VMID_BASE_IDX, GC_mmCP_HQD_VMID, mqd.get(V9_MQD_cp_hqd_vmid));
    gc_wr(adev, GC_mmCP_HQD_PERSISTENT_STATE_BASE_IDX, GC_mmCP_HQD_PERSISTENT_STATE,
          mqd.get(V9_MQD_cp_hqd_persistent_state));
    // activate the queue
    gc_wr(adev, GC_mmCP_HQD_ACTIVE_BASE_IDX, GC_mmCP_HQD_ACTIVE, mqd.get(V9_MQD_cp_hqd_active));

    if ring.use_doorbell {
        gc_field15(adev, GC_mmCP_PQ_STATUS_BASE_IDX, GC_mmCP_PQ_STATUS, GC_CP_PQ_STATUS__DOORBELL_ENABLE_MASK,
                   GC_CP_PQ_STATUS__DOORBELL_ENABLE__SHIFT, 1);
    }
}

/// A fresh MQD as `memset` + the two dynamic masks leave it.
fn v9_mqd_clean() -> v9_mqd {
    let mut mqd = v9_mqd::new();
    mqd.set(V9_MQD_ALLOCATION_dynamic_cu_mask, 0xFFFFFFFF);
    mqd.set(V9_MQD_ALLOCATION_dynamic_rb_mask, 0xFFFFFFFF);
    mqd
}

/// `gfx_v9_0_kiq_init_queue`: not in reset, so a fresh MQD.
fn gfx_v9_0_kiq_init_queue(adev: &mut amdgpu_device, r: usize) -> i32 {
    gfx_v9_0_kiq_setting(adev, r);
    let mut mqd = v9_mqd_clean();
    let ring = &adev.rings[r];
    let (me, pipe, queue) = (ring.me, ring.pipe, ring.queue);
    soc15_grbm_select(adev, me, pipe, queue, 0);
    gfx_v9_0_mqd_init(adev, r, &mut mqd);
    gfx_v9_0_kiq_init_register(adev, r, &mqd);
    soc15_grbm_select(adev, 0, 0, 0, 0);
    let bytes = mqd.bytes();
    if let Some(k) = adev.rings[r].mqd_ptr {
        crate::amdgpu_gart::kptr_write(adev, k, 0, &bytes);
    }
    if !adev.gfx.kiq.mqd_backup.is_empty() {
        let n = adev.gfx.kiq.mqd_backup.len().min(bytes.len());
        adev.gfx.kiq.mqd_backup[..n].copy_from_slice(&bytes[..n]);
    }
    0
}

/// `gfx_v9_0_kcq_init_queue`, `restore` false and not in reset or
/// suspend: a fresh MQD.
fn gfx_v9_0_kcq_init_queue(adev: &mut amdgpu_device, mqd_idx: usize) -> i32 {
    let r = adev.gfx.compute_ring[mqd_idx];
    let mut mqd = v9_mqd_clean();
    let ring = &adev.rings[r];
    let (me, pipe, queue) = (ring.me, ring.pipe, ring.queue);
    soc15_grbm_select(adev, me, pipe, queue, 0);
    gfx_v9_0_mqd_init(adev, r, &mut mqd);
    soc15_grbm_select(adev, 0, 0, 0, 0);
    let bytes = mqd.bytes();
    if let Some(k) = adev.rings[r].mqd_ptr {
        crate::amdgpu_gart::kptr_write(adev, k, 0, &bytes);
    }
    if let Some(b) = adev.gfx.mec.mqd_backup.get_mut(mqd_idx) {
        let n = b.len().min(bytes.len());
        b[..n].copy_from_slice(&bytes[..n]);
    }
    0
}

/// `gfx_v9_0_kiq_set_resources`.
fn gfx_v9_0_kiq_set_resources(adev: &mut amdgpu_device, kiq_ring: usize, queue_mask: u64) {
    // Cleaner shader MC address
    let shader_mc_addr = adev.gfx.cleaner_shader_gpu_addr >> 8;
    let w = |adev: &mut amdgpu_device, v: u32| crate::amdgpu_ring::amdgpu_ring_write(adev, kiq_ring, v);
    w(adev, PACKET3(PACKET3_SET_RESOURCES, 6));
    // vmid_mask:0* queue_type:0 (KIQ)
    w(adev, PACKET3_SET_RESOURCES_VMID_MASK_1 * 0 | PACKET3_SET_RESOURCES_QUEUE_TYPE_1 * 0);
    w(adev, queue_mask as u32); // queue mask lo
    w(adev, (queue_mask >> 32) as u32); // queue mask hi
    w(adev, shader_mc_addr as u32); // cleaner shader addr lo
    w(adev, (shader_mc_addr >> 32) as u32); // cleaner shader addr hi
    w(adev, 0); // oac mask
    w(adev, 0); // gds heap base:0, gds heap size:0
}

/// `gfx_v9_0_kiq_map_queues`.
fn gfx_v9_0_kiq_map_queues(adev: &mut amdgpu_device, kiq_ring: usize, r: usize) {
    let ring = &adev.rings[r];
    let mqd_addr = crate::amdgpu_object::amdgpu_bo_gpu_offset(adev, ring.mqd_obj.unwrap_or(0));
    let wptr_addr = ring.wptr_gpu_addr;
    let eng_sel = if ring.funcs.type_ == AMDGPU_RING_TYPE_GFX { 4 } else { 0 };
    let (queue, pipe, me, doorbell_index) = (ring.queue, ring.pipe, ring.me, ring.doorbell_index);
    let w = |adev: &mut amdgpu_device, v: u32| crate::amdgpu_ring::amdgpu_ring_write(adev, kiq_ring, v);
    w(adev, PACKET3(PACKET3_MAP_QUEUES, 5));
    // Q_sel: 0, vmid: 0, engine: 0, num_Q: 1
    w(adev, PACKET3_MAP_QUEUES_QUEUE_SEL_1 * 0 // Queue_Sel
        | PACKET3_MAP_QUEUES_VMID_1 * 0 // VMID
        | PACKET3_MAP_QUEUES_QUEUE_1 * queue
        | PACKET3_MAP_QUEUES_PIPE_1 * pipe
        | PACKET3_MAP_QUEUES_ME_1 * (if me == 1 { 0 } else { 1 })
        // queue_type: normal compute queue
        | PACKET3_MAP_QUEUES_QUEUE_TYPE_1 * 0
        // alloc format: all_on_one_pipe
        | PACKET3_MAP_QUEUES_ALLOC_FORMAT_1 * 0
        | PACKET3_MAP_QUEUES_ENGINE_SEL_1 * eng_sel
        // num_queues: must be 1
        | PACKET3_MAP_QUEUES_NUM_QUEUES_1);
    w(adev, PACKET3_MAP_QUEUES_DOORBELL_OFFSET_1 * doorbell_index);
    w(adev, mqd_addr as u32);
    w(adev, (mqd_addr >> 32) as u32);
    w(adev, wptr_addr as u32);
    w(adev, (wptr_addr >> 32) as u32);
}

/// `amdgpu_queue_mask_bit_to_set_resource_bit`.
fn amdgpu_queue_mask_bit_to_set_resource_bit(adev: &amdgpu_device, bit: u32) -> u32 {
    // amdgpu_queue_mask_bit_to_mec_queue
    let m = &adev.gfx.mec;
    let queue = bit % m.num_queue_per_pipe;
    let pipe = (bit / m.num_queue_per_pipe) % m.num_pipe_per_mec;
    let mec = (bit / m.num_queue_per_pipe) / m.num_pipe_per_mec;
    mec * 4 * 8 + pipe * 8 + queue
}

/// `amdgpu_gfx_enable_kcq`: no MES, `gfx_v9_0_kiq_pm4_funcs`.
fn amdgpu_gfx_enable_kcq(adev: &mut amdgpu_device) -> i32 {
    let Some(kiq_ring) = adev.gfx.kiq.ring else { return -22 };
    let mut queue_mask = 0u64;
    for i in 0..AMDGPU_MAX_COMPUTE_QUEUES {
        if adev.gfx.mec_queue_bitmap[(i / 64) as usize] & (1 << (i % 64)) == 0 {
            continue;
        }
        // This situation may be hit in the future if a new HW generation
        // exposes more than 64 queues. If so, the definition of queue_mask
        // needs updating
        if i > 64 {
            Line::new().s("Invalid KCQ enabled: ").dec(i as u64).say();
            break;
        }
        queue_mask |= 1u64 << amdgpu_queue_mask_bit_to_set_resource_bit(adev, i);
    }
    // amdgpu_device_flush_hdp(adev, NULL): nothing on an APU.
    let k = &adev.rings[kiq_ring];
    Line::new().s("kiq ring mec ").dec(k.me as u64).s(" pipe ").dec(k.pipe as u64).s(" q ").dec(k.queue as u64).say();
    let r = crate::amdgpu_ring::amdgpu_ring_alloc(adev, kiq_ring,
        GFX_V9_0_KIQ_MAP_QUEUES_SIZE * adev.gfx.num_compute_rings + GFX_V9_0_KIQ_SET_RESOURCES_SIZE);
    if r != 0 {
        Line::new().s("Failed to lock KIQ (").dec((-r) as u64).s(").").say();
        return r;
    }
    gfx_v9_0_kiq_set_resources(adev, kiq_ring, queue_mask);
    for i in 0..adev.gfx.num_compute_rings as usize {
        let c = adev.gfx.compute_ring[i];
        gfx_v9_0_kiq_map_queues(adev, kiq_ring, c);
    }
    // Submit map queue packet
    crate::amdgpu_ring::amdgpu_ring_commit(adev, kiq_ring);
    // Ring test will do a basic scratch register change check. Just run
    // this to ensure that map queues that is submitted before got processed
    // successfully before returning.
    let r = crate::amdgpu_ring::amdgpu_ring_test_helper(adev, kiq_ring);
    if r != 0 {
        Line::new().s("KCQ enable failed").say();
    }
    r
}

/// `gfx_v9_0_kcq_resume`.
fn gfx_v9_0_kcq_resume(adev: &mut amdgpu_device) -> i32 {
    gfx_v9_0_cp_compute_enable(adev, true);
    for i in 0..adev.gfx.num_compute_rings as usize {
        let r = gfx_v9_0_kcq_init_queue(adev, i);
        if r != 0 {
            return r;
        }
    }
    amdgpu_gfx_enable_kcq(adev)
}

/// `gfx_v9_0_cp_resume`: an APU, firmware loaded by the PSP.
fn gfx_v9_0_cp_resume(adev: &mut amdgpu_device) -> i32 {
    if adev.gfx.num_gfx_rings != 0 {
        gfx_v9_0_cp_gfx_enable(adev, false);
    }
    gfx_v9_0_cp_compute_enable(adev, false);

    // gfx_v9_0_kiq_resume
    if let Some(k) = adev.gfx.kiq.ring {
        gfx_v9_0_kiq_init_queue(adev, k);
    }

    if adev.gfx.num_gfx_rings != 0 {
        let r = gfx_v9_0_cp_gfx_resume(adev);
        if r != 0 {
            return r;
        }
    }

    let r = gfx_v9_0_kcq_resume(adev);
    if r != 0 {
        return r;
    }

    if adev.gfx.num_gfx_rings != 0 {
        let ring = adev.gfx.gfx_ring[0];
        let r = crate::amdgpu_ring::amdgpu_ring_test_helper(adev, ring);
        if r != 0 {
            return r;
        }
    }
    for i in 0..adev.gfx.num_compute_rings as usize {
        let ring = adev.gfx.compute_ring[i];
        crate::amdgpu_ring::amdgpu_ring_test_helper(adev, ring);
    }

    gfx_v9_0_enable_gui_idle_interrupt(adev, true);
    0
}

/// `amdgpu_gfx_cleaner_shader_init`: the shader into its buffer, when
/// sw_init enabled it.
fn amdgpu_gfx_cleaner_shader_init(adev: &amdgpu_device, shader: &[u32]) {
    if !adev.gfx.enable_cleaner_shader {
        return;
    }
    if let Some(k) = adev.gfx.cleaner_shader_cpu_ptr {
        let bytes: Vec<u8> = shader.iter().flat_map(|w| w.to_le_bytes()).collect();
        let n = bytes.len().min(adev.gfx.cleaner_shader_size as usize);
        crate::amdgpu_gart::kptr_write(adev, k, 0, &bytes[..n]);
    }
}

/// `gfx_v9_0_hw_init`: bare metal, GC 9.3.0, whose cleaner shader is
/// `gfx_9_4_2_cleaner_shader_hex`; `gfx_v9_0_init_tcp_config` is for GC
/// 9.4.1/9.4.2 only.
pub fn gfx_v9_0_hw_init(adev: &mut amdgpu_device) -> i32 {
    amdgpu_gfx_cleaner_shader_init(adev, &crate::tables::gfx_9_4_2_cleaner_shader_hex);
    gfx_v9_0_init_golden_registers(adev);
    gfx_v9_0_constants_init(adev);
    let r = gfx_v9_0_rlc_resume(adev);
    if r != 0 {
        return r;
    }
    gfx_v9_0_cp_resume(adev)
}

/// `gfx_v9_ring_insert_nop`.
pub fn gfx_v9_ring_insert_nop(adev: &mut amdgpu_device, r: usize, num_nop: u32) {
    // Header itself is a NOP packet
    if num_nop == 1 {
        let nop = adev.rings[r].funcs.nop;
        crate::amdgpu_ring::amdgpu_ring_write(adev, r, nop);
        return;
    }
    // Max HW optimization till 0x3ffe, followed by remaining one NOP at a
    // time
    crate::amdgpu_ring::amdgpu_ring_write(adev, r, PACKET3(PACKET3_NOP, (num_nop - 2).min(0x3ffe)));
    // Header is at index 0, followed by num_nops - 1 NOP packet's
    crate::amdgpu_ring::amdgpu_ring_insert_nop(adev, r, num_nop - 1);
}

/// `gfx_v9_0_ring_set_wptr_gfx`.
pub fn gfx_v9_0_ring_set_wptr_gfx(adev: &mut amdgpu_device, r: usize) {
    let ring = &adev.rings[r];
    let (wptr, db) = (ring.wptr, ring.doorbell_index);
    if ring.use_doorbell {
        crate::amdgpu_ring::ring_wptr_cpu_set(adev, r, wptr);
        crate::amdgpu_ring::amdgpu_mm_wdoorbell64(adev, db, wptr);
    } else {
        gc_wr(adev, GC_mmCP_RB0_WPTR_BASE_IDX, GC_mmCP_RB0_WPTR, wptr as u32);
        gc_wr(adev, GC_mmCP_RB0_WPTR_HI_BASE_IDX, GC_mmCP_RB0_WPTR_HI, (wptr >> 32) as u32);
    }
}

/// `gfx_v9_0_ring_set_wptr_compute`, also the KIQ's.
pub fn gfx_v9_0_ring_set_wptr_compute(adev: &mut amdgpu_device, r: usize) {
    let ring = &adev.rings[r];
    let (wptr, db) = (ring.wptr, ring.doorbell_index);
    if ring.use_doorbell {
        crate::amdgpu_ring::ring_wptr_cpu_set(adev, r, wptr);
        crate::amdgpu_ring::amdgpu_mm_wdoorbell64(adev, db, wptr);
    } else {
        // only DOORBELL method supported on gfx9 now
        Line::new().s("BUG: gfx_v9_0_ring_set_wptr_compute without a doorbell").say();
    }
}

/// `gfx_v9_0_ring_test_ring`.
pub fn gfx_v9_0_ring_test_ring(adev: &mut amdgpu_device, r: usize) -> i32 {
    let scratch = adev.reg_offset[GC_HWIP as usize][0][mmSCRATCH_REG0_BASE_IDX as usize] + mmSCRATCH_REG0;
    adev.wreg32(scratch, 0xCAFEDEAD);
    let ret = crate::amdgpu_ring::amdgpu_ring_alloc(adev, r, 3);
    if ret != 0 {
        return ret;
    }
    crate::amdgpu_ring::amdgpu_ring_write(adev, r, PACKET3(PACKET3_SET_UCONFIG_REG, 1));
    crate::amdgpu_ring::amdgpu_ring_write(adev, r, scratch - PACKET3_SET_UCONFIG_REG_START);
    crate::amdgpu_ring::amdgpu_ring_write(adev, r, 0xDEADBEEF);
    crate::amdgpu_ring::amdgpu_ring_commit(adev, r);
    let mut i = 0;
    while i < adev.usec_timeout {
        if adev.rreg32(scratch).unwrap_or(0) == 0xDEADBEEF {
            break;
        }
        crate::linux::udelay(1);
        i += 1;
    }
    if i >= adev.usec_timeout { -110 } else { 0 }
}

// ---- fences, IBs and interrupts (stage 4) ----

use crate::amdgpu_ring::amdgpu_ring_write as rw;

/// `gfx_v9_0_ring_emit_fence`.
pub fn gfx_v9_0_ring_emit_fence(adev: &mut amdgpu_device, r: usize, addr: u64, seq: u64, flags: u32) {
    let write64bit = flags & AMDGPU_FENCE_FLAG_64BIT != 0;
    let int_sel = flags & AMDGPU_FENCE_FLAG_INT != 0;
    let writeback = flags & AMDGPU_FENCE_FLAG_TC_WB_ONLY != 0;
    let exec = flags & AMDGPU_FENCE_FLAG_EXEC != 0;

    // RELEASE_MEM - flush caches, send int
    rw(adev, r, PACKET3(PACKET3_RELEASE_MEM, 6));
    let mut dw2 = if writeback { EOP_TC_NC_ACTION_EN } else { EOP_TCL1_ACTION_EN | EOP_TC_ACTION_EN | EOP_TC_MD_ACTION_EN };
    dw2 |= EOP_TC_WB_ACTION_EN | EVENT_TYPE_CACHE_FLUSH_AND_INV_TS_EVENT | EVENT_INDEX_5;
    if exec {
        dw2 |= EOP_EXEC;
    }
    rw(adev, r, dw2);
    rw(adev, r, (if write64bit { DATA_SEL_2 } else { DATA_SEL_1 }) | (if int_sel { INT_SEL_2 } else { INT_SEL_0 }));
    // the address should be Qword aligned if 64bit write, Dword aligned if
    // only send 32bit data low (discard data high)
    if (write64bit && addr & 0x7 != 0) || addr & 0x3 != 0 {
        Line::new().s("WARNING: gfx_v9_0_ring_emit_fence misaligned address").say();
    }
    rw(adev, r, addr as u32);
    rw(adev, r, (addr >> 32) as u32);
    rw(adev, r, seq as u32);
    rw(adev, r, (seq >> 32) as u32);
    rw(adev, r, 0);
}

/// `gfx_v9_0_ring_emit_fence_kiq`.
pub fn gfx_v9_0_ring_emit_fence_kiq(adev: &mut amdgpu_device, r: usize, addr: u64, seq: u64, flags: u32) {
    // write fence seq to the "addr"
    rw(adev, r, PACKET3(PACKET3_WRITE_DATA, 3));
    rw(adev, r, WRITE_DATA_ENGINE_SEL_0 | WRITE_DATA_DST_SEL_5 | WR_CONFIRM);
    rw(adev, r, addr as u32);
    rw(adev, r, (addr >> 32) as u32);
    rw(adev, r, seq as u32);
    if flags & AMDGPU_FENCE_FLAG_INT != 0 {
        // set register to trigger INT
        let reg = adev.reg_offset[GC_HWIP as usize][0][GC_mmCPC_INT_STATUS_BASE_IDX as usize] + GC_mmCPC_INT_STATUS;
        rw(adev, r, PACKET3(PACKET3_WRITE_DATA, 3));
        rw(adev, r, WRITE_DATA_ENGINE_SEL_0 | WRITE_DATA_DST_SEL_0 | WR_CONFIRM);
        rw(adev, r, reg);
        rw(adev, r, 0);
        rw(adev, r, 0x20000000); // src_id is 178
    }
}

/// `gfx_v9_0_emit_mem_sync`.
pub fn gfx_v9_0_emit_mem_sync(adev: &mut amdgpu_device, r: usize) {
    // ACQUIRE_MEM -make one or more surfaces valid for use by the
    // subsequent operations
    rw(adev, r, PACKET3(PACKET3_ACQUIRE_MEM, 5));
    rw(adev, r, GFX_V9_0_MEM_SYNC_CP_COHER_CNTL); // CP_COHER_CNTL
    rw(adev, r, 0xffffffff); // CP_COHER_SIZE
    rw(adev, r, 0xffffff); // CP_COHER_SIZE_HI
    rw(adev, r, 0); // CP_COHER_BASE
    rw(adev, r, 0); // CP_COHER_BASE_HI
    rw(adev, r, 0x0000000A); // POLL_INTERVAL
}

/// `gfx_v9_0_ring_emit_wreg`.
pub fn gfx_v9_0_ring_emit_wreg(adev: &mut amdgpu_device, r: usize, reg: u32, val: u32) {
    let t = adev.rings[r].funcs.type_;
    let cmd = if t == AMDGPU_RING_TYPE_GFX {
        WRITE_DATA_ENGINE_SEL_1 | WR_CONFIRM
    } else if t == AMDGPU_RING_TYPE_KIQ {
        1 << 16 // no inc addr
    } else {
        WR_CONFIRM
    };
    rw(adev, r, PACKET3(PACKET3_WRITE_DATA, 3));
    rw(adev, r, cmd);
    rw(adev, r, reg);
    rw(adev, r, 0);
    rw(adev, r, val);
}

/// `gfx_v9_0_emit_wave_limit_cs`.
fn gfx_v9_0_emit_wave_limit_cs(adev: &mut amdgpu_device, r: usize, pipe: u32, enable: bool) {
    // mmSPI_WCL_PIPE_PERCENT_CS[0-7]_DEFAULT values are same
    let val = if enable { 0x1 } else { GC_mmSPI_WCL_PIPE_PERCENT_CS0_DEFAULT };
    let (idx, reg) = match pipe {
        0 => (GC_mmSPI_WCL_PIPE_PERCENT_CS0_BASE_IDX, GC_mmSPI_WCL_PIPE_PERCENT_CS0),
        1 => (GC_mmSPI_WCL_PIPE_PERCENT_CS1_BASE_IDX, GC_mmSPI_WCL_PIPE_PERCENT_CS1),
        2 => (GC_mmSPI_WCL_PIPE_PERCENT_CS2_BASE_IDX, GC_mmSPI_WCL_PIPE_PERCENT_CS2),
        3 => (GC_mmSPI_WCL_PIPE_PERCENT_CS3_BASE_IDX, GC_mmSPI_WCL_PIPE_PERCENT_CS3),
        _ => {
            Line::new().s("invalid pipe ").dec(pipe as u64).dbg();
            return;
        }
    };
    let wcl_cs_reg = adev.reg_offset[GC_HWIP as usize][0][idx as usize] + reg;
    crate::amdgpu_ring::amdgpu_ring_emit_wreg(adev, r, wcl_cs_reg, val);
}

/// `gfx_v9_0_emit_wave_limit`.
pub fn gfx_v9_0_emit_wave_limit(adev: &mut amdgpu_device, r: usize, enable: bool) {
    // mmSPI_WCL_PIPE_PERCENT_GFX is 7 bit multiplier register to limit
    // number of gfx waves. Setting 5 bit will make sure gfx only gets
    // around 25% of gpu resources.
    let val = if enable { 0x1f } else { GC_mmSPI_WCL_PIPE_PERCENT_GFX_DEFAULT };
    let reg = adev.reg_offset[GC_HWIP as usize][0][GC_mmSPI_WCL_PIPE_PERCENT_GFX_BASE_IDX as usize]
        + GC_mmSPI_WCL_PIPE_PERCENT_GFX;
    crate::amdgpu_ring::amdgpu_ring_emit_wreg(adev, r, reg, val);
    // Restrict waves for normal/low priority compute queues as well to get
    // best QoS for high priority compute jobs.
    //
    // amdgpu controls only 1st ME(0-3 CS pipes).
    for i in 0..adev.gfx.mec.num_pipe_per_mec {
        if i != adev.rings[r].pipe {
            gfx_v9_0_emit_wave_limit_cs(adev, r, i, enable);
        }
    }
}

/// `gfx_v9_0_ring_emit_init_cond_exec`.
pub fn gfx_v9_0_ring_emit_init_cond_exec(adev: &mut amdgpu_device, r: usize, addr: u64) -> u32 {
    rw(adev, r, PACKET3(PACKET3_COND_EXEC, 3));
    rw(adev, r, addr as u32);
    rw(adev, r, (addr >> 32) as u32);
    // discard following DWs if *cond_exec_gpu_addr==0
    rw(adev, r, 0);
    let ret = (adev.rings[r].wptr & adev.rings[r].buf_mask as u64) as u32;
    // patch dummy value later
    rw(adev, r, 0);
    ret
}

/// `gfx_v9_0_ring_emit_ib_gfx` without a job (VMID 0, no preemption).
/// `amdgpu_csa_vaddr`: the CSA at the top of every client's VM.
pub fn amdgpu_csa_vaddr(adev: &amdgpu_device) -> u64 {
    // AMDGPU_VA_RESERVED_CSA_START
    let addr = (adev.vm_manager.max_pfn << AMDGPU_GPU_PAGE_SHIFT) - AMDGPU_VA_RESERVED_CSA_SIZE as u64;
    crate::amdgpu_object::amdgpu_gmc_sign_extend(addr)
}

/// A WRITE_DATA of a zeroed metadata payload of `bytes` to `addr`. On
/// resume Linux writes the CSA's saved copy, which only exists with MCBP,
/// off on bare metal.
fn emit_meta(adev: &mut amdgpu_device, r: usize, engine: u32, addr: u64, payload: &[u32]) {
    let cnt = payload.len() as u32 + 4 - 2;
    rw(adev, r, PACKET3(PACKET3_WRITE_DATA, cnt));
    rw(adev, r, (engine | WRITE_DATA_DST_SEL_8 as u32 | WR_CONFIRM) | WRITE_DATA_CACHE_POLICY_0 as u32);
    rw(adev, r, addr as u32);
    rw(adev, r, (addr >> 32) as u32);
    // amdgpu_ring_ib_on_emit_ce/_de: not a software ring.
    for &w in payload {
        rw(adev, r, w);
    }
}

/// `gfx_v9_0_ring_emit_ce_meta`.
fn gfx_v9_0_ring_emit_ce_meta(adev: &mut amdgpu_device, r: usize, _resume: bool) {
    let addr = amdgpu_csa_vaddr(adev) + V9_META_CE_PAYLOAD as u64;
    let payload = alloc::vec![0u32; (SIZEOF_v9_ce_ib_state / 4) as usize];
    emit_meta(adev, r, WRITE_DATA_ENGINE_SEL_2 as u32, addr, &payload);
}

/// `gfx_v9_0_ring_emit_de_meta`.
fn gfx_v9_0_ring_emit_de_meta(adev: &mut amdgpu_device, r: usize, _resume: bool, usegds: bool) {
    let csa = amdgpu_csa_vaddr(adev);
    let addr = csa + V9_META_DE_PAYLOAD as u64;
    let gds_addr = crate::linux::align(csa + AMDGPU_CSA_SIZE as u64 - adev.gds.gds_size as u64,
                                       crate::amdgpu_object::PAGE_SIZE);
    let mut payload = alloc::vec![0u32; (SIZEOF_v9_de_ib_state / 4) as usize];
    if usegds {
        payload[(V9_DE_GDS_BACKUP_ADDRLO / 4) as usize] = gds_addr as u32;
        payload[(V9_DE_GDS_BACKUP_ADDRHI / 4) as usize] = (gds_addr >> 32) as u32;
    }
    emit_meta(adev, r, WRITE_DATA_ENGINE_SEL_1, addr, &payload);
}

/// `gfx_v9_ring_emit_cntxcntl`.
pub fn gfx_v9_ring_emit_cntxcntl(adev: &mut amdgpu_device, r: usize, flags: u32) {
    gfx_v9_0_ring_emit_ce_meta(adev, r, flags & AMDGPU_IB_PREEMPTED != 0);
    let mut dw2 = 0x80000000u32; // set load_enable otherwise this package is just NOPs
    if flags & AMDGPU_HAVE_CTX_SWITCH != 0 {
        // set load_global_config & load_global_uconfig
        dw2 |= 0x8001;
        // set load_cs_sh_regs
        dw2 |= 0x01000000;
        // set load_per_context_state & load_gfx_sh_regs for GFX
        dw2 |= 0x10002;
        // set load_ce_ram if preamble presented
        if flags & AMDGPU_PREAMBLE_IB_PRESENT != 0 {
            dw2 |= 0x10000000;
        }
    } else if flags & AMDGPU_PREAMBLE_IB_PRESENT_FIRST != 0 {
        // still load_ce_ram if this is the first time preamble presented
        // although there is no context switch happens.
        dw2 |= 0x10000000;
    }
    rw(adev, r, PACKET3(PACKET3_CONTEXT_CONTROL, 1));
    rw(adev, r, dw2);
    rw(adev, r, 0);
}

/// `gfx_v9_ring_emit_sb`, the GFX ring's `emit_switch_buffer`.
pub fn gfx_v9_ring_emit_sb(adev: &mut amdgpu_device, r: usize) {
    rw(adev, r, PACKET3(PACKET3_SWITCH_BUFFER, 0));
    rw(adev, r, 0);
}

/// `gfx_v9_0_ring_emit_frame_cntl`.
pub fn gfx_v9_0_ring_emit_frame_cntl(adev: &mut amdgpu_device, r: usize, start: bool, secure: bool) {
    let v = if secure { FRAME_TMZ } else { 0 };
    rw(adev, r, PACKET3(PACKET3_FRAME_CONTROL, 0));
    rw(adev, r, v | if start { 0 } else { FRAME_CMD_1 });
}

/// `gfx_v9_0_ring_emit_ib_gfx`; `gds` is whether the job uses GDS.
pub fn gfx_v9_0_ring_emit_ib_gfx(adev: &mut amdgpu_device, r: usize, vmid: u32, gds: bool,
                                 ib: &crate::amdgpu_ib::amdgpu_ib, flags: u32) {
    let header = if ib.flags & AMDGPU_IB_FLAG_CE != 0 {
        PACKET3(PACKET3_INDIRECT_BUFFER_CONST, 2)
    } else {
        PACKET3(PACKET3_INDIRECT_BUFFER, 2)
    };
    let mut control = ib.length_dw | (vmid << 24);
    if ib.flags & AMDGPU_IB_FLAG_PREEMPT != 0 {
        control |= INDIRECT_BUFFER_PRE_ENB_1;
        if flags & AMDGPU_IB_PREEMPTED != 0 {
            control |= INDIRECT_BUFFER_PRE_RESUME_1;
        }
        if ib.flags & AMDGPU_IB_FLAG_CE == 0 && vmid != 0 {
            gfx_v9_0_ring_emit_de_meta(adev, r, flags & AMDGPU_IB_PREEMPTED != 0, gds);
        }
    }
    rw(adev, r, header);
    if ib.gpu_addr & 0x3 != 0 {
        Line::new().s("WARNING: IB not dword aligned").say();
    }
    rw(adev, r, ib.gpu_addr as u32);
    rw(adev, r, (ib.gpu_addr >> 32) as u32);
    // amdgpu_ring_ib_on_emit_cntl: not a software ring.
    rw(adev, r, control);
}

/// `gfx_v9_0_ring_emit_ib_compute`.
pub fn gfx_v9_0_ring_emit_ib_compute(adev: &mut amdgpu_device, r: usize, vmid: u32, ib: &crate::amdgpu_ib::amdgpu_ib,
                                     _flags: u32) {
    let control = INDIRECT_BUFFER_VALID | ib.length_dw | (vmid << 24);
    // Currently, there is a high possibility to get wave ID mismatch
    // between ME and GDS, leading to a hw deadlock, because ME generates
    // different wave IDs than the GDS expects. This situation happens
    // randomly when at least 5 compute pipes use GDS ordered append. The
    // wave IDs generated by ME are also wrong after suspend/resume. Those
    // are probably bugs somewhere else in the kernel driver.
    //
    // Writing GDS_COMPUTE_MAX_WAVE_ID resets wave ID counters in ME and GDS
    // to 0 for this ring (me/pipe).
    if ib.flags & AMDGPU_IB_FLAG_RESET_GDS_MAX_WAVE_ID != 0 {
        rw(adev, r, PACKET3(PACKET3_SET_CONFIG_REG, 1));
        rw(adev, r, GC_mmGDS_COMPUTE_MAX_WAVE_ID);
        rw(adev, r, adev.gds.gds_compute_max_wave_id);
    }
    rw(adev, r, PACKET3(PACKET3_INDIRECT_BUFFER, 2));
    if ib.gpu_addr & 0x3 != 0 {
        Line::new().s("WARNING: IB not dword aligned").say();
    }
    rw(adev, r, ib.gpu_addr as u32);
    rw(adev, r, (ib.gpu_addr >> 32) as u32);
    rw(adev, r, control);
}

/// `gfx_v9_0_write_data_to_reg`.
fn gfx_v9_0_write_data_to_reg(adev: &mut amdgpu_device, r: usize, eng_sel: u32, wc: bool, reg: u32, val: u32) {
    rw(adev, r, PACKET3(PACKET3_WRITE_DATA, 3));
    rw(adev, r, eng_sel * WRITE_DATA_ENGINE_SEL_1 | WRITE_DATA_DST_SEL_0 | if wc { WR_CONFIRM } else { 0 });
    rw(adev, r, reg);
    rw(adev, r, 0);
    rw(adev, r, val);
}

/// `gfx_v9_0_wait_reg_mem`.
fn gfx_v9_0_wait_reg_mem(adev: &mut amdgpu_device, r: usize, eng_sel: u32, mem_space: u32, opt: u32, addr0: u32,
                         addr1: u32, ref_: u32, mask: u32, inv: u32) {
    rw(adev, r, PACKET3(PACKET3_WAIT_REG_MEM, 5));
    // memory (1) or register (0), write, wait, reg_write_reg_wait, equal
    rw(adev, r, mem_space * WAIT_REG_MEM_MEM_SPACE_1 | opt * WAIT_REG_MEM_OPERATION_1 | WAIT_REG_MEM_FUNCTION_3
        | eng_sel * WAIT_REG_MEM_ENGINE_1);
    if mem_space != 0 && addr0 & 0x3 != 0 {
        Line::new().s("WARNING: wait_reg_mem address not dword aligned").say();
    }
    rw(adev, r, addr0);
    rw(adev, r, addr1);
    rw(adev, r, ref_);
    rw(adev, r, mask);
    rw(adev, r, inv); // poll interval
}

/// `gfx_v9_0_ring_emit_pipeline_sync`.
pub fn gfx_v9_0_ring_emit_pipeline_sync(adev: &mut amdgpu_device, r: usize) {
    let usepfp = (adev.rings[r].funcs.type_ == AMDGPU_RING_TYPE_GFX) as u32;
    let seq = adev.rings[r].fence_drv.sync_seq;
    let addr = adev.rings[r].fence_drv.gpu_addr;
    gfx_v9_0_wait_reg_mem(adev, r, usepfp, 1, 0, addr as u32, (addr >> 32) as u32, seq, 0xffffffff, 4);
}

/// `gfx_v9_0_ring_emit_vm_flush`.
pub fn gfx_v9_0_ring_emit_vm_flush(adev: &mut amdgpu_device, r: usize, vmid: u32, pd_addr: u64) {
    crate::gmc_v9_0::gmc_v9_0_emit_flush_gpu_tlb(adev, r, vmid, pd_addr);
    // compute doesn't have PFP
    if adev.rings[r].funcs.type_ == AMDGPU_RING_TYPE_GFX {
        // sync PFP to ME, otherwise we might get invalid PFP reads
        rw(adev, r, PACKET3(PACKET3_PFP_SYNC_ME, 0));
        rw(adev, r, 0x0);
    }
}

/// `gfx_v9_0_ring_emit_reg_wait`.
pub fn gfx_v9_0_ring_emit_reg_wait(adev: &mut amdgpu_device, r: usize, reg: u32, val: u32, mask: u32) {
    gfx_v9_0_wait_reg_mem(adev, r, 0, 0, 0, reg, 0, val, mask, 0x20);
}

/// `gfx_v9_0_ring_emit_reg_write_reg_wait`.
pub fn gfx_v9_0_ring_emit_reg_write_reg_wait(adev: &mut amdgpu_device, r: usize, reg0: u32, reg1: u32, ref_: u32,
                                             mask: u32) {
    let gfx = adev.rings[r].funcs.type_ == AMDGPU_RING_TYPE_GFX;
    let usepfp = gfx as u32;
    let fw_version_ok = if gfx { adev.gfx.me_fw_write_wait } else { adev.gfx.mec_fw_write_wait };
    if fw_version_ok {
        gfx_v9_0_wait_reg_mem(adev, r, usepfp, 0, 1, reg0, reg1, ref_, mask, 0x20);
    } else {
        crate::amdgpu_ring::amdgpu_ring_emit_reg_write_reg_wait_helper(adev, r, reg0, reg1, ref_, mask);
    }
}

/// `gfx_v9_0_ring_emit_gds_switch`.
pub fn gfx_v9_0_ring_emit_gds_switch(adev: &mut amdgpu_device, r: usize, vmid: u32, gds_base: u32, gds_size: u32,
                                     gws_base: u32, gws_size: u32, oa_base: u32, oa_size: u32) {
    // GDS Base
    let reg = gc_reg(adev, GC_mmGDS_VMID0_BASE, GC_mmGDS_VMID0_BASE_BASE_IDX) + 2 * vmid;
    gfx_v9_0_write_data_to_reg(adev, r, 0, false, reg, gds_base);
    // GDS Size
    let reg = gc_reg(adev, GC_mmGDS_VMID0_SIZE, GC_mmGDS_VMID0_SIZE_BASE_IDX) + 2 * vmid;
    gfx_v9_0_write_data_to_reg(adev, r, 0, false, reg, gds_size);
    // GWS
    let reg = gc_reg(adev, GC_mmGDS_GWS_VMID0, GC_mmGDS_GWS_VMID0_BASE_IDX) + vmid;
    gfx_v9_0_write_data_to_reg(adev, r, 0, false, reg, gws_size << GC_GDS_GWS_VMID0__SIZE__SHIFT | gws_base);
    // OA
    let reg = gc_reg(adev, GC_mmGDS_OA_VMID0, GC_mmGDS_OA_VMID0_BASE_IDX) + vmid;
    gfx_v9_0_write_data_to_reg(adev, r, 0, false, reg, (1u32 << (oa_size + oa_base)).wrapping_sub(1 << oa_base));
}

/// `gfx_v9_0_ring_test_ib`.
pub fn gfx_v9_0_ring_test_ib(adev: &mut amdgpu_device, r: usize, timeout: i64) -> i32 {
    use crate::amdgpu_ib::{amdgpu_ib, amdgpu_ib_free, amdgpu_ib_get, amdgpu_ib_schedule};
    let index = match crate::amdgpu_ring::amdgpu_device_wb_get(adev) {
        Ok(i) => i,
        Err(e) => return e,
    };
    let gpu_addr = adev.wb.gpu_addr + index as u64 * 4;
    let Some(wb) = adev.wb.wb else { return -22 };
    crate::amdgpu_gart::kptr_w32(adev, wb, index as u64 * 4, 0xCAFEDEAD);
    let mut ib = amdgpu_ib::new();
    let mut ret = amdgpu_ib_get(adev, 20, AMDGPU_IB_POOL_DIRECT, &mut ib);
    if ret == 0 {
        ib.set(adev, 0, PACKET3(PACKET3_WRITE_DATA, 3));
        ib.set(adev, 1, WRITE_DATA_DST_SEL_5 | WR_CONFIRM);
        ib.set(adev, 2, gpu_addr as u32);
        ib.set(adev, 3, (gpu_addr >> 32) as u32);
        ib.set(adev, 4, 0xDEADBEEF);
        ib.length_dw = 5;
        ret = match amdgpu_ib_schedule(adev, r, core::slice::from_ref(&ib), None) {
            Err(e) => e,
            Ok(f) => {
                let t = crate::dma_fence::dma_fence_wait_timeout(adev, &f, timeout);
                if t == 0 {
                    -110
                } else if crate::amdgpu_gart::kptr_r32(adev, wb, index as u64 * 4) == 0xDEADBEEF {
                    0
                } else {
                    -22
                }
            }
        };
        amdgpu_ib_free(adev, &mut ib, None);
    }
    crate::amdgpu_ring::amdgpu_device_wb_free(adev, index);
    ret
}

/// `WREG32_FIELD15(GC, 0, CP_INT_CNTL_RING0, field, v)`.
fn cp_int_cntl_ring0(adev: &amdgpu_device, mask: u32, shift: u32, v: u32) {
    gc_field15(adev, GC_mmCP_INT_CNTL_RING0_BASE_IDX, GC_mmCP_INT_CNTL_RING0, mask, shift, v);
}

/// `gfx_v9_0_get_cpc_int_cntl`: amdgpu controls only the first MEC.
fn gfx_v9_0_get_cpc_int_cntl(adev: &amdgpu_device, me: u32, pipe: u32) -> u32 {
    if me != 1 {
        return 0;
    }
    let (idx, reg) = match pipe {
        0 => (GC_mmCP_ME1_PIPE0_INT_CNTL_BASE_IDX, GC_mmCP_ME1_PIPE0_INT_CNTL),
        1 => (GC_mmCP_ME1_PIPE1_INT_CNTL_BASE_IDX, GC_mmCP_ME1_PIPE1_INT_CNTL),
        2 => (GC_mmCP_ME1_PIPE2_INT_CNTL_BASE_IDX, GC_mmCP_ME1_PIPE2_INT_CNTL),
        3 => (GC_mmCP_ME1_PIPE3_INT_CNTL_BASE_IDX, GC_mmCP_ME1_PIPE3_INT_CNTL),
        _ => return 0,
    };
    adev.reg_offset[GC_HWIP as usize][0][idx as usize] + reg
}

/// `gfx_v9_0_set_compute_eop_interrupt_state`.
fn gfx_v9_0_set_compute_eop_interrupt_state(adev: &amdgpu_device, me: u32, pipe: u32, state: u32) {
    // amdgpu controls only the first MEC. That's why this function only
    // handles the setting of interrupts for this specific MEC. All other
    // pipes' interrupts are set by amdkfd.
    if me != 1 || pipe > 3 {
        Line::new().s("invalid me ").dec(me as u64).s(" pipe ").dec(pipe as u64).dbg();
        return;
    }
    let reg = gfx_v9_0_get_cpc_int_cntl(adev, me, pipe);
    if state != AMDGPU_IRQ_STATE_DISABLE && state != AMDGPU_IRQ_STATE_ENABLE {
        return;
    }
    let mut v = adev.rreg32(reg).unwrap_or(0);
    v = set_field(v, GC_CP_ME1_PIPE0_INT_CNTL__TIME_STAMP_INT_ENABLE_MASK,
                  GC_CP_ME1_PIPE0_INT_CNTL__TIME_STAMP_INT_ENABLE__SHIFT, (state == AMDGPU_IRQ_STATE_ENABLE) as u32);
    adev.wreg32(reg, v);
}

/// `gfx_v9_0_set_eop_interrupt_state`.
pub fn gfx_v9_0_set_eop_interrupt_state(adev: &mut amdgpu_device, type_: u32, state: u32) -> i32 {
    match type_ {
        AMDGPU_CP_IRQ_GFX_ME0_PIPE0_EOP => {
            // gfx_v9_0_set_gfx_eop_interrupt_state
            if state == AMDGPU_IRQ_STATE_DISABLE || state == AMDGPU_IRQ_STATE_ENABLE {
                cp_int_cntl_ring0(adev, GC_CP_INT_CNTL_RING0__TIME_STAMP_INT_ENABLE_MASK,
                                  GC_CP_INT_CNTL_RING0__TIME_STAMP_INT_ENABLE__SHIFT,
                                  (state == AMDGPU_IRQ_STATE_ENABLE) as u32);
            }
        }
        AMDGPU_CP_IRQ_COMPUTE_MEC1_PIPE0_EOP => gfx_v9_0_set_compute_eop_interrupt_state(adev, 1, 0, state),
        AMDGPU_CP_IRQ_COMPUTE_MEC1_PIPE1_EOP => gfx_v9_0_set_compute_eop_interrupt_state(adev, 1, 1, state),
        AMDGPU_CP_IRQ_COMPUTE_MEC1_PIPE2_EOP => gfx_v9_0_set_compute_eop_interrupt_state(adev, 1, 2, state),
        AMDGPU_CP_IRQ_COMPUTE_MEC1_PIPE3_EOP => gfx_v9_0_set_compute_eop_interrupt_state(adev, 1, 3, state),
        AMDGPU_CP_IRQ_COMPUTE_MEC2_PIPE0_EOP => gfx_v9_0_set_compute_eop_interrupt_state(adev, 2, 0, state),
        AMDGPU_CP_IRQ_COMPUTE_MEC2_PIPE1_EOP => gfx_v9_0_set_compute_eop_interrupt_state(adev, 2, 1, state),
        AMDGPU_CP_IRQ_COMPUTE_MEC2_PIPE2_EOP => gfx_v9_0_set_compute_eop_interrupt_state(adev, 2, 2, state),
        AMDGPU_CP_IRQ_COMPUTE_MEC2_PIPE3_EOP => gfx_v9_0_set_compute_eop_interrupt_state(adev, 2, 3, state),
        _ => {}
    }
    0
}

/// The shared body of `gfx_v9_0_set_priv_reg_fault_state` and
/// `gfx_v9_0_set_bad_op_fault_state`: the RING0 field, then the same field
/// on every pipe of the first MEC.
fn set_fault_state(adev: &amdgpu_device, state: u32, ring0: (u32, u32), pipe: (u32, u32)) {
    if state != AMDGPU_IRQ_STATE_DISABLE && state != AMDGPU_IRQ_STATE_ENABLE {
        return;
    }
    let e = (state == AMDGPU_IRQ_STATE_ENABLE) as u32;
    cp_int_cntl_ring0(adev, ring0.0, ring0.1, e);
    for i in 0..adev.gfx.mec.num_mec {
        for j in 0..adev.gfx.mec.num_pipe_per_mec {
            // MECs start at 1
            let reg = gfx_v9_0_get_cpc_int_cntl(adev, i + 1, j);
            if reg != 0 {
                let v = adev.rreg32(reg).unwrap_or(0);
                adev.wreg32(reg, set_field(v, pipe.0, pipe.1, e));
            }
        }
    }
}

/// `gfx_v9_0_set_priv_reg_fault_state`.
pub fn gfx_v9_0_set_priv_reg_fault_state(adev: &mut amdgpu_device, _type: u32, state: u32) -> i32 {
    set_fault_state(adev, state,
        (GC_CP_INT_CNTL_RING0__PRIV_REG_INT_ENABLE_MASK, GC_CP_INT_CNTL_RING0__PRIV_REG_INT_ENABLE__SHIFT),
        (GC_CP_ME1_PIPE0_INT_CNTL__PRIV_REG_INT_ENABLE_MASK, GC_CP_ME1_PIPE0_INT_CNTL__PRIV_REG_INT_ENABLE__SHIFT));
    0
}

/// `gfx_v9_0_set_bad_op_fault_state`.
pub fn gfx_v9_0_set_bad_op_fault_state(adev: &mut amdgpu_device, _type: u32, state: u32) -> i32 {
    set_fault_state(adev, state,
        (GC_CP_INT_CNTL_RING0__OPCODE_ERROR_INT_ENABLE_MASK, GC_CP_INT_CNTL_RING0__OPCODE_ERROR_INT_ENABLE__SHIFT),
        (GC_CP_ME1_PIPE0_INT_CNTL__OPCODE_ERROR_INT_ENABLE_MASK,
         GC_CP_ME1_PIPE0_INT_CNTL__OPCODE_ERROR_INT_ENABLE__SHIFT));
    0
}

/// `gfx_v9_0_set_priv_inst_fault_state`.
pub fn gfx_v9_0_set_priv_inst_fault_state(adev: &mut amdgpu_device, _type: u32, state: u32) -> i32 {
    if state == AMDGPU_IRQ_STATE_DISABLE || state == AMDGPU_IRQ_STATE_ENABLE {
        cp_int_cntl_ring0(adev, GC_CP_INT_CNTL_RING0__PRIV_INSTR_INT_ENABLE_MASK,
                          GC_CP_INT_CNTL_RING0__PRIV_INSTR_INT_ENABLE__SHIFT, (state == AMDGPU_IRQ_STATE_ENABLE) as u32);
    }
    0
}

/// `gfx_v9_0_set_cp_ecc_error_state`, with `ENABLE_ECC_ON_ME_PIPE` and
/// `DISABLE_ECC_ON_ME_PIPE` on ME1 pipes 0-3.
pub fn gfx_v9_0_set_cp_ecc_error_state(adev: &mut amdgpu_device, _type: u32, state: u32) -> i32 {
    if state != AMDGPU_IRQ_STATE_DISABLE && state != AMDGPU_IRQ_STATE_ENABLE {
        return 0;
    }
    let e = (state == AMDGPU_IRQ_STATE_ENABLE) as u32;
    cp_int_cntl_ring0(adev, GC_CP_INT_CNTL_RING0__CP_ECC_ERROR_INT_ENABLE_MASK,
                      GC_CP_INT_CNTL_RING0__CP_ECC_ERROR_INT_ENABLE__SHIFT, e);
    for (idx, reg, mask, shift) in [
        (GC_mmCP_ME1_PIPE0_INT_CNTL_BASE_IDX, GC_mmCP_ME1_PIPE0_INT_CNTL,
         GC_CP_ME1_PIPE0_INT_CNTL__CP_ECC_ERROR_INT_ENABLE_MASK, GC_CP_ME1_PIPE0_INT_CNTL__CP_ECC_ERROR_INT_ENABLE__SHIFT),
        (GC_mmCP_ME1_PIPE1_INT_CNTL_BASE_IDX, GC_mmCP_ME1_PIPE1_INT_CNTL,
         GC_CP_ME1_PIPE1_INT_CNTL__CP_ECC_ERROR_INT_ENABLE_MASK, GC_CP_ME1_PIPE1_INT_CNTL__CP_ECC_ERROR_INT_ENABLE__SHIFT),
        (GC_mmCP_ME1_PIPE2_INT_CNTL_BASE_IDX, GC_mmCP_ME1_PIPE2_INT_CNTL,
         GC_CP_ME1_PIPE2_INT_CNTL__CP_ECC_ERROR_INT_ENABLE_MASK, GC_CP_ME1_PIPE2_INT_CNTL__CP_ECC_ERROR_INT_ENABLE__SHIFT),
        (GC_mmCP_ME1_PIPE3_INT_CNTL_BASE_IDX, GC_mmCP_ME1_PIPE3_INT_CNTL,
         GC_CP_ME1_PIPE3_INT_CNTL__CP_ECC_ERROR_INT_ENABLE_MASK, GC_CP_ME1_PIPE3_INT_CNTL__CP_ECC_ERROR_INT_ENABLE__SHIFT),
    ] {
        gc_field15(adev, idx, reg, mask, shift, e);
    }
    0
}

/// `gfx_v9_0_eop_irq`: no MCBP, so the one GFX ring itself.
pub fn gfx_v9_0_eop_irq(adev: &mut amdgpu_device, entry: &crate::amdgpu_irq::amdgpu_iv_entry) -> i32 {
    Line::new().s("IH: CP EOP").dbg();
    let me_id = (entry.ring_id & 0x0c) >> 2;
    let pipe_id = entry.ring_id & 0x03;
    let queue_id = (entry.ring_id & 0x70) >> 4;
    match me_id {
        0 => {
            if adev.gfx.num_gfx_rings != 0 {
                let r = adev.gfx.gfx_ring[0];
                crate::amdgpu_fence::amdgpu_fence_process(adev, r);
            }
        }
        1 | 2 => {
            for i in 0..adev.gfx.num_compute_rings as usize {
                let r = adev.gfx.compute_ring[i];
                let ring = &adev.rings[r];
                // Per-queue interrupt is supported for MEC starting from VI.
                // The interrupt can only be enabled/disabled per pipe
                // instead of per queue.
                if ring.me == me_id && ring.pipe == pipe_id && ring.queue == queue_id {
                    crate::amdgpu_fence::amdgpu_fence_process(adev, r);
                }
            }
        }
        _ => {}
    }
    0
}

/// `gfx_v9_0_fault`: `drm_sched_fault` tells the scheduler; there is none,
/// so the ring is named.
fn gfx_v9_0_fault(adev: &amdgpu_device, entry: &crate::amdgpu_irq::amdgpu_iv_entry) {
    let me_id = (entry.ring_id & 0x0c) >> 2;
    let pipe_id = entry.ring_id & 0x03;
    let queue_id = (entry.ring_id & 0x70) >> 4;
    let ring = match me_id {
        0 => adev.gfx.gfx_ring.first().copied(),
        1 | 2 => adev.gfx.compute_ring.iter().copied().find(|&r| {
            let x = &adev.rings[r];
            x.me == me_id && x.pipe == pipe_id && x.queue == queue_id
        }),
        _ => None,
    };
    if let Some(r) = ring {
        Line::new().s("  on ring ").s(&adev.rings[r].name).say();
    }
}

/// `gfx_v9_0_priv_reg_irq`.
pub fn gfx_v9_0_priv_reg_irq(adev: &mut amdgpu_device, entry: &crate::amdgpu_irq::amdgpu_iv_entry) -> i32 {
    Line::new().s("Illegal register access in command stream").say();
    gfx_v9_0_fault(adev, entry);
    0
}

/// `gfx_v9_0_bad_op_irq`.
pub fn gfx_v9_0_bad_op_irq(adev: &mut amdgpu_device, entry: &crate::amdgpu_irq::amdgpu_iv_entry) -> i32 {
    Line::new().s("Illegal opcode in command stream").say();
    gfx_v9_0_fault(adev, entry);
    0
}

/// `gfx_v9_0_priv_inst_irq`.
pub fn gfx_v9_0_priv_inst_irq(adev: &mut amdgpu_device, entry: &crate::amdgpu_irq::amdgpu_iv_entry) -> i32 {
    Line::new().s("Illegal instruction in command stream").say();
    gfx_v9_0_fault(adev, entry);
    0
}

// ---- late_init, gating, hw_fini (stage 4) ----

/// `gfx_v9_0_is_rlc_enabled`.
fn gfx_v9_0_is_rlc_enabled(adev: &amdgpu_device) -> bool {
    // if RLC is not enabled, do nothing
    gc_rd(adev, GC_mmRLC_CNTL_BASE_IDX, GC_mmRLC_CNTL) & GC_RLC_CNTL__RLC_ENABLE_F32_MASK != 0
}

const SAFE_MODE_CG: u64 = AMD_CG_SUPPORT_GFX_CGCG as u64 | AMD_CG_SUPPORT_GFX_MGCG as u64
    | AMD_CG_SUPPORT_GFX_3D_CGCG as u64;

/// `amdgpu_gfx_rlc_enter_safe_mode` with `gfx_v9_0_set_safe_mode`.
fn amdgpu_gfx_rlc_enter_safe_mode(adev: &mut amdgpu_device) {
    if adev.gfx.rlc.in_safe_mode || !gfx_v9_0_is_rlc_enabled(adev) {
        return;
    }
    if adev.cg_flags & SAFE_MODE_CG != 0 {
        let mut data = GC_RLC_SAFE_MODE__CMD_MASK;
        data |= 1 << GC_RLC_SAFE_MODE__MESSAGE__SHIFT;
        gc_wr(adev, GC_mmRLC_SAFE_MODE_BASE_IDX, GC_mmRLC_SAFE_MODE, data);
        // wait for RLC_SAFE_MODE
        for _ in 0..adev.usec_timeout {
            let v = gc_rd(adev, GC_mmRLC_SAFE_MODE_BASE_IDX, GC_mmRLC_SAFE_MODE);
            if (v & GC_RLC_SAFE_MODE__CMD_MASK) >> GC_RLC_SAFE_MODE__CMD__SHIFT == 0 {
                break;
            }
            crate::linux::udelay(1);
        }
        adev.gfx.rlc.in_safe_mode = true;
    }
}

/// `amdgpu_gfx_rlc_exit_safe_mode` with `gfx_v9_0_unset_safe_mode`.
fn amdgpu_gfx_rlc_exit_safe_mode(adev: &mut amdgpu_device) {
    if !adev.gfx.rlc.in_safe_mode || !gfx_v9_0_is_rlc_enabled(adev) {
        return;
    }
    if adev.cg_flags & SAFE_MODE_CG != 0 {
        gc_wr(adev, GC_mmRLC_SAFE_MODE_BASE_IDX, GC_mmRLC_SAFE_MODE, GC_RLC_SAFE_MODE__CMD_MASK);
        adev.gfx.rlc.in_safe_mode = false;
    }
}

fn cg(adev: &amdgpu_device, f: u32) -> bool {
    adev.cg_flags & f as u64 != 0
}

/// `gfx_v9_0_update_medium_grain_clock_gating`.
fn gfx_v9_0_update_medium_grain_clock_gating(adev: &amdgpu_device, enable: bool) {
    let not_921 = adev.ip_version(GC_HWIP, 0) != 9 << 24 | 2 << 16 | 1 << 8;
    let (oi, o) = (GC_mmRLC_CGTT_MGCG_OVERRIDE_BASE_IDX, GC_mmRLC_CGTT_MGCG_OVERRIDE);
    // It is disabled by HW by default
    if enable && cg(adev, AMD_CG_SUPPORT_GFX_MGCG) {
        // 1 - RLC_CGTT_MGCG_OVERRIDE
        let def = gc_rd(adev, oi, o);
        let mut data = def;
        if not_921 {
            data &= !GC_RLC_CGTT_MGCG_OVERRIDE__CPF_CGTT_SCLK_OVERRIDE_MASK;
        }
        data &= !(GC_RLC_CGTT_MGCG_OVERRIDE__GRBM_CGTT_SCLK_OVERRIDE_MASK
            | GC_RLC_CGTT_MGCG_OVERRIDE__GFXIP_MGCG_OVERRIDE_MASK
            | GC_RLC_CGTT_MGCG_OVERRIDE__GFXIP_MGLS_OVERRIDE_MASK);
        // only for Vega10 & Raven1
        data |= GC_RLC_CGTT_MGCG_OVERRIDE__RLC_CGTT_SCLK_OVERRIDE_MASK;
        if def != data {
            gc_wr(adev, oi, o, data);
        }
        // MGLS is a global flag to control all MGLS in GFX
        if cg(adev, AMD_CG_SUPPORT_GFX_MGLS) {
            // 2 - RLC memory Light sleep
            if cg(adev, AMD_CG_SUPPORT_GFX_RLC_LS) {
                let def = gc_rd(adev, GC_mmRLC_MEM_SLP_CNTL_BASE_IDX, GC_mmRLC_MEM_SLP_CNTL);
                let data = def | GC_RLC_MEM_SLP_CNTL__RLC_MEM_LS_EN_MASK;
                if def != data {
                    gc_wr(adev, GC_mmRLC_MEM_SLP_CNTL_BASE_IDX, GC_mmRLC_MEM_SLP_CNTL, data);
                }
            }
            // 3 - CP memory Light sleep
            if cg(adev, AMD_CG_SUPPORT_GFX_CP_LS) {
                let def = gc_rd(adev, GC_mmCP_MEM_SLP_CNTL_BASE_IDX, GC_mmCP_MEM_SLP_CNTL);
                let data = def | GC_CP_MEM_SLP_CNTL__CP_MEM_LS_EN_MASK;
                if def != data {
                    gc_wr(adev, GC_mmCP_MEM_SLP_CNTL_BASE_IDX, GC_mmCP_MEM_SLP_CNTL, data);
                }
            }
        }
    } else {
        // 1 - MGCG_OVERRIDE
        let def = gc_rd(adev, oi, o);
        let mut data = def;
        if not_921 {
            data |= GC_RLC_CGTT_MGCG_OVERRIDE__CPF_CGTT_SCLK_OVERRIDE_MASK;
        }
        data |= GC_RLC_CGTT_MGCG_OVERRIDE__RLC_CGTT_SCLK_OVERRIDE_MASK
            | GC_RLC_CGTT_MGCG_OVERRIDE__GRBM_CGTT_SCLK_OVERRIDE_MASK
            | GC_RLC_CGTT_MGCG_OVERRIDE__GFXIP_MGCG_OVERRIDE_MASK
            | GC_RLC_CGTT_MGCG_OVERRIDE__GFXIP_MGLS_OVERRIDE_MASK;
        if def != data {
            gc_wr(adev, oi, o, data);
        }
        // 2 - disable MGLS in RLC
        let data = gc_rd(adev, GC_mmRLC_MEM_SLP_CNTL_BASE_IDX, GC_mmRLC_MEM_SLP_CNTL);
        if data & GC_RLC_MEM_SLP_CNTL__RLC_MEM_LS_EN_MASK != 0 {
            gc_wr(adev, GC_mmRLC_MEM_SLP_CNTL_BASE_IDX, GC_mmRLC_MEM_SLP_CNTL,
                  data & !GC_RLC_MEM_SLP_CNTL__RLC_MEM_LS_EN_MASK);
        }
        // 3 - disable MGLS in CP
        let data = gc_rd(adev, GC_mmCP_MEM_SLP_CNTL_BASE_IDX, GC_mmCP_MEM_SLP_CNTL);
        if data & GC_CP_MEM_SLP_CNTL__CP_MEM_LS_EN_MASK != 0 {
            gc_wr(adev, GC_mmCP_MEM_SLP_CNTL_BASE_IDX, GC_mmCP_MEM_SLP_CNTL,
                  data & !GC_CP_MEM_SLP_CNTL__CP_MEM_LS_EN_MASK);
        }
    }
}

/// The IDLE_POLL_COUNT write of the CGCG paths: `0x00900100`.
fn set_idle_poll_count(adev: &amdgpu_device) {
    let def = gc_rd(adev, GC_mmCP_RB_WPTR_POLL_CNTL_BASE_IDX, GC_mmCP_RB_WPTR_POLL_CNTL);
    let data = (0x0100 << GC_CP_RB_WPTR_POLL_CNTL__POLL_FREQUENCY__SHIFT)
        | (0x0090 << GC_CP_RB_WPTR_POLL_CNTL__IDLE_POLL_COUNT__SHIFT);
    if def != data {
        gc_wr(adev, GC_mmCP_RB_WPTR_POLL_CNTL_BASE_IDX, GC_mmCP_RB_WPTR_POLL_CNTL, data);
    }
}

/// `gfx_v9_0_update_3d_clock_gating`.
fn gfx_v9_0_update_3d_clock_gating(adev: &amdgpu_device, enable: bool) {
    if adev.gfx.num_gfx_rings == 0 {
        return;
    }
    let (ci, c) = (GC_mmRLC_CGCG_CGLS_CTRL_3D_BASE_IDX, GC_mmRLC_CGCG_CGLS_CTRL_3D);
    if enable {
        // write cmd to clear cgcg/cgls ov
        let def = gc_rd(adev, GC_mmRLC_CGTT_MGCG_OVERRIDE_BASE_IDX, GC_mmRLC_CGTT_MGCG_OVERRIDE);
        // unset CGCG override
        let data = def & !GC_RLC_CGTT_MGCG_OVERRIDE__GFXIP_GFX3D_CG_OVERRIDE_MASK;
        // update CGCG and CGLS override bits
        if def != data {
            gc_wr(adev, GC_mmRLC_CGTT_MGCG_OVERRIDE_BASE_IDX, GC_mmRLC_CGTT_MGCG_OVERRIDE, data);
        }
        // enable 3Dcgcg FSM(0x0000363f)
        let def = gc_rd(adev, ci, c);
        let mut data = if cg(adev, AMD_CG_SUPPORT_GFX_3D_CGCG) {
            (0x36 << GC_RLC_CGCG_CGLS_CTRL_3D__CGCG_GFX_IDLE_THRESHOLD__SHIFT) | GC_RLC_CGCG_CGLS_CTRL_3D__CGCG_EN_MASK
        } else {
            0x0 << GC_RLC_CGCG_CGLS_CTRL_3D__CGCG_GFX_IDLE_THRESHOLD__SHIFT
        };
        if cg(adev, AMD_CG_SUPPORT_GFX_3D_CGLS) {
            data |= (0x000F << GC_RLC_CGCG_CGLS_CTRL_3D__CGLS_REP_COMPANSAT_DELAY__SHIFT)
                | GC_RLC_CGCG_CGLS_CTRL_3D__CGLS_EN_MASK;
        }
        if def != data {
            gc_wr(adev, ci, c, data);
        }
        // set IDLE_POLL_COUNT(0x00900100)
        set_idle_poll_count(adev);
    } else {
        // Disable CGCG/CGLS
        let def = gc_rd(adev, ci, c);
        // disable cgcg, cgls should be disabled
        let data = def & !(GC_RLC_CGCG_CGLS_CTRL_3D__CGCG_EN_MASK | GC_RLC_CGCG_CGLS_CTRL_3D__CGLS_EN_MASK);
        // disable cgcg and cgls in FSM
        if def != data {
            gc_wr(adev, ci, c, data);
        }
    }
}

/// `gfx_v9_0_update_coarse_grain_clock_gating`: GC 9.3.0 is not 9.4.1.
fn gfx_v9_0_update_coarse_grain_clock_gating(adev: &amdgpu_device, enable: bool) {
    let (ci, c) = (GC_mmRLC_CGCG_CGLS_CTRL_BASE_IDX, GC_mmRLC_CGCG_CGLS_CTRL);
    if enable && cg(adev, AMD_CG_SUPPORT_GFX_CGCG) {
        let def = gc_rd(adev, GC_mmRLC_CGTT_MGCG_OVERRIDE_BASE_IDX, GC_mmRLC_CGTT_MGCG_OVERRIDE);
        // unset CGCG override
        let mut data = def & !GC_RLC_CGTT_MGCG_OVERRIDE__GFXIP_CGCG_OVERRIDE_MASK;
        if cg(adev, AMD_CG_SUPPORT_GFX_CGLS) {
            data &= !GC_RLC_CGTT_MGCG_OVERRIDE__GFXIP_CGLS_OVERRIDE_MASK;
        } else {
            data |= GC_RLC_CGTT_MGCG_OVERRIDE__GFXIP_CGLS_OVERRIDE_MASK;
        }
        // update CGCG and CGLS override bits
        if def != data {
            gc_wr(adev, GC_mmRLC_CGTT_MGCG_OVERRIDE_BASE_IDX, GC_mmRLC_CGTT_MGCG_OVERRIDE, data);
        }
        // enable cgcg FSM(0x0000363F)
        let def = gc_rd(adev, ci, c);
        let thr = if adev.ip_version(GC_HWIP, 0) == 9 << 24 | 4 << 16 | 1 << 8 { 0x2000 } else { 0x36 };
        let mut data = (thr << GC_RLC_CGCG_CGLS_CTRL__CGCG_GFX_IDLE_THRESHOLD__SHIFT) | GC_RLC_CGCG_CGLS_CTRL__CGCG_EN_MASK;
        if cg(adev, AMD_CG_SUPPORT_GFX_CGLS) {
            data |= (0x000F << GC_RLC_CGCG_CGLS_CTRL__CGLS_REP_COMPANSAT_DELAY__SHIFT) | GC_RLC_CGCG_CGLS_CTRL__CGLS_EN_MASK;
        }
        if def != data {
            gc_wr(adev, ci, c, data);
        }
        // set IDLE_POLL_COUNT(0x00900100)
        set_idle_poll_count(adev);
    } else {
        let def = gc_rd(adev, ci, c);
        // reset CGCG/CGLS bits
        let data = def & !(GC_RLC_CGCG_CGLS_CTRL__CGCG_EN_MASK | GC_RLC_CGCG_CGLS_CTRL__CGLS_EN_MASK);
        // disable cgcg and cgls in FSM
        if def != data {
            gc_wr(adev, ci, c, data);
        }
    }
}

/// `gfx_v9_0_set_clockgating_state` (`gfx_v9_0_update_gfx_clock_gating`).
pub fn gfx_v9_0_set_clockgating_state(adev: &mut amdgpu_device, gate: bool) -> i32 {
    amdgpu_gfx_rlc_enter_safe_mode(adev);
    if gate {
        // CGCG/CGLS should be enabled after MGCG/MGLS
        // ===  MGCG + MGLS ===
        gfx_v9_0_update_medium_grain_clock_gating(adev, gate);
        // ===  CGCG /CGLS for GFX 3D Only ===
        gfx_v9_0_update_3d_clock_gating(adev, gate);
        // ===  CGCG + CGLS ===
        gfx_v9_0_update_coarse_grain_clock_gating(adev, gate);
    } else {
        // CGCG/CGLS should be disabled before MGCG/MGLS
        // ===  CGCG + CGLS ===
        gfx_v9_0_update_coarse_grain_clock_gating(adev, gate);
        // ===  CGCG /CGLS for GFX 3D Only ===
        gfx_v9_0_update_3d_clock_gating(adev, gate);
        // ===  MGCG + MGLS ===
        gfx_v9_0_update_medium_grain_clock_gating(adev, gate);
    }
    amdgpu_gfx_rlc_exit_safe_mode(adev);
    0
}

/// One RLC_PG_CNTL field, written only when it changes.
fn rlc_pg_cntl(adev: &amdgpu_device, mask: u32, shift: u32, v: u32) {
    let def = gc_rd(adev, GC_mmRLC_PG_CNTL_BASE_IDX, GC_mmRLC_PG_CNTL);
    let data = set_field(def, mask, shift, v);
    if def != data {
        gc_wr(adev, GC_mmRLC_PG_CNTL_BASE_IDX, GC_mmRLC_PG_CNTL, data);
    }
}

/// `gfx_v9_0_update_gfx_cg_power_gating`.
fn gfx_v9_0_update_gfx_cg_power_gating(adev: &mut amdgpu_device, enable: bool) {
    amdgpu_gfx_rlc_enter_safe_mode(adev);
    let on = adev.pg_flags & AMD_PG_SUPPORT_GFX_PG != 0 && enable;
    // gfx_v9_0_enable_gfx_cg_power_gating
    rlc_pg_cntl(adev, GC_RLC_PG_CNTL__GFX_POWER_GATING_ENABLE_MASK, GC_RLC_PG_CNTL__GFX_POWER_GATING_ENABLE__SHIFT,
                on as u32);
    if adev.pg_flags & AMD_PG_SUPPORT_GFX_PIPELINE != 0 {
        // gfx_v9_0_enable_gfx_pipeline_powergating
        rlc_pg_cntl(adev, GC_RLC_PG_CNTL__GFX_PIPELINE_PG_ENABLE_MASK, GC_RLC_PG_CNTL__GFX_PIPELINE_PG_ENABLE__SHIFT,
                    on as u32);
        if !on {
            // read any GFX register to wake up GFX
            gc_rd(adev, GC_mmDB_RENDER_CONTROL_BASE_IDX, GC_mmDB_RENDER_CONTROL);
        }
    }
    amdgpu_gfx_rlc_exit_safe_mode(adev);
}

/// `gfx_v9_0_update_gfx_mg_power_gating`.
fn gfx_v9_0_update_gfx_mg_power_gating(adev: &amdgpu_device, enable: bool) {
    rlc_pg_cntl(adev, GC_RLC_PG_CNTL__STATIC_PER_CU_PG_ENABLE_MASK, GC_RLC_PG_CNTL__STATIC_PER_CU_PG_ENABLE__SHIFT,
                (adev.pg_flags & AMD_PG_SUPPORT_GFX_SMG != 0 && enable) as u32);
    rlc_pg_cntl(adev, GC_RLC_PG_CNTL__DYN_PER_CU_PG_ENABLE_MASK, GC_RLC_PG_CNTL__DYN_PER_CU_PG_ENABLE__SHIFT,
                (adev.pg_flags & AMD_PG_SUPPORT_GFX_DMG != 0 && enable) as u32);
}

/// `amdgpu_gfx_do_off_ctrl`: GFXOFF comes back `GFX_OFF_DELAY_ENABLE`
/// (100 ms) after the last user is done, or at once with `no_delay`.
/// GC 9.3.0 has no `init_spm_golden`.
fn amdgpu_gfx_do_off_ctrl(adev: &mut amdgpu_device, enable: bool, no_delay: bool) {
    if adev.pm.pp_feature & PP_GFXOFF_MASK == 0 {
        return;
    }
    let w = crate::workqueue::work::gfx_off_delay;
    if enable {
        // If the count is already 0, it means there's an imbalance bug
        // somewhere.
        if adev.gfx.gfx_off_req_count == 0 {
            Line::new().s("WARNING: gfx_off_req_count is already 0").say();
            return;
        }
        adev.gfx.gfx_off_req_count -= 1;
        if adev.gfx.gfx_off_req_count == 0 && !adev.gfx.gfx_off_state {
            if no_delay {
                if crate::amdgpu_smu::amdgpu_dpm_set_powergating_by_smu(adev, AMD_IP_BLOCK_TYPE_GFX, true, 0) == 0 {
                    adev.gfx.gfx_off_state = true;
                }
            } else if !crate::workqueue::delayed_work_pending(adev, w) {
                // schedule_delayed_work
                crate::workqueue::mod_delayed_work(adev, w, 100_000);
            }
        }
    } else {
        if adev.gfx.gfx_off_req_count == 0 {
            crate::workqueue::cancel_delayed_work(adev, w);
            if adev.gfx.gfx_off_state
                && crate::amdgpu_smu::amdgpu_dpm_set_powergating_by_smu(adev, AMD_IP_BLOCK_TYPE_GFX, false, 0) == 0
            {
                adev.gfx.gfx_off_state = false;
            }
        }
        adev.gfx.gfx_off_req_count += 1;
    }
}

/// `amdgpu_gfx_off_ctrl`: no delay when going to s2idle.
pub fn amdgpu_gfx_off_ctrl(adev: &mut amdgpu_device, enable: bool) {
    let no_delay = adev.in_s0ix;
    amdgpu_gfx_do_off_ctrl(adev, enable, no_delay);
}

/// `amdgpu_gfx_off_ctrl_immediate`.
pub fn amdgpu_gfx_off_ctrl_immediate(adev: &mut amdgpu_device, enable: bool) {
    amdgpu_gfx_do_off_ctrl(adev, enable, true);
}

/// `amdgpu_device_delay_enable_gfx_off`.
pub fn amdgpu_device_delay_enable_gfx_off(adev: &mut amdgpu_device) {
    if adev.gfx.gfx_off_state || adev.gfx.gfx_off_req_count != 0 {
        Line::new().s("WARNING: delayed GFXOFF with GFXOFF on or requested off").say();
    }
    if crate::amdgpu_smu::amdgpu_dpm_set_powergating_by_smu(adev, AMD_IP_BLOCK_TYPE_GFX, true, 0) == 0 {
        adev.gfx.gfx_off_state = true;
    }
}

/// `gfx_v9_0_update_spm_vmid`.
pub fn gfx_v9_0_update_spm_vmid(adev: &mut amdgpu_device, vmid: u32) {
    amdgpu_gfx_off_ctrl(adev, false);
    gfx_v9_0_update_spm_vmid_internal(adev, vmid);
    amdgpu_gfx_off_ctrl(adev, true);
}

/// `gfx_v9_0_set_powergating_state`, GC 9.3.0.
pub fn gfx_v9_0_set_powergating_state(adev: &mut amdgpu_device, gate: bool) -> i32 {
    if !gate {
        amdgpu_gfx_off_ctrl_immediate(adev, false);
    }
    let hs = (adev.pg_flags & AMD_PG_SUPPORT_RLC_SMU_HS != 0) as u32;
    // gfx_v9_0_enable_sck_slow_down_on_power_up / _down
    rlc_pg_cntl(adev, GC_RLC_PG_CNTL__SMU_CLK_SLOWDOWN_ON_PU_ENABLE_MASK,
                GC_RLC_PG_CNTL__SMU_CLK_SLOWDOWN_ON_PU_ENABLE__SHIFT, hs);
    rlc_pg_cntl(adev, GC_RLC_PG_CNTL__SMU_CLK_SLOWDOWN_ON_PD_ENABLE_MASK,
                GC_RLC_PG_CNTL__SMU_CLK_SLOWDOWN_ON_PD_ENABLE__SHIFT, hs);
    // gfx_v9_0_enable_cp_power_gating
    rlc_pg_cntl(adev, GC_RLC_PG_CNTL__CP_PG_DISABLE_MASK, GC_RLC_PG_CNTL__CP_PG_DISABLE__SHIFT,
                (adev.pg_flags & AMD_PG_SUPPORT_CP == 0) as u32);
    // update gfx cgpg state
    gfx_v9_0_update_gfx_cg_power_gating(adev, gate);
    // update mgcg state
    gfx_v9_0_update_gfx_mg_power_gating(adev, gate);
    if gate {
        amdgpu_gfx_off_ctrl_immediate(adev, true);
    }
    0
}

/// `gfx_v9_0_debug_trap_config_init`.
fn gfx_v9_0_debug_trap_config_init(adev: &amdgpu_device, first_vmid: u32, last_vmid: u32) {
    // Calculate trap config vmid mask
    let mut trap_config_vmid_mask = 0u32;
    for i in first_vmid..last_vmid {
        trap_config_vmid_mask |= 1 << i;
    }
    let mut data = set_field(0, GC_SPI_GDBG_TRAP_CONFIG__VMID_SEL_MASK, GC_SPI_GDBG_TRAP_CONFIG__VMID_SEL__SHIFT,
                             trap_config_vmid_mask);
    data = set_field(data, GC_SPI_GDBG_TRAP_CONFIG__TRAP_EN_MASK, GC_SPI_GDBG_TRAP_CONFIG__TRAP_EN__SHIFT, 1);
    gc_wr(adev, GC_mmSPI_GDBG_TRAP_CONFIG_BASE_IDX, GC_mmSPI_GDBG_TRAP_CONFIG, data);
    gc_wr(adev, GC_mmSPI_GDBG_TRAP_MASK_BASE_IDX, GC_mmSPI_GDBG_TRAP_MASK, 0);
    gc_wr(adev, GC_mmSPI_GDBG_TRAP_DATA0_BASE_IDX, GC_mmSPI_GDBG_TRAP_DATA0, 0);
    gc_wr(adev, GC_mmSPI_GDBG_TRAP_DATA1_BASE_IDX, GC_mmSPI_GDBG_TRAP_DATA1, 0);
}

/// `gfx_v9_0_late_init`. `gfx_v9_0_ecc_late_init` does nothing without
/// RAS: both EDC workarounds return at once, and there is no gfx.ras.
pub fn gfx_v9_0_late_init(adev: &mut amdgpu_device) -> i32 {
    for name in ["gfx.priv_reg_irq", "gfx.priv_inst_irq", "gfx.bad_op_irq"] {
        let r = crate::amdgpu_irq::amdgpu_irq_get(adev, name, 0);
        if r != 0 {
            return r;
        }
    }
    let first = adev.vm_manager.first_kfd_vmid;
    gfx_v9_0_debug_trap_config_init(adev, first, AMDGPU_NUM_VMID);
    0
}

/// `gfx_v9_0_kiq_unmap_queues`, RESET_QUEUES.
fn gfx_v9_0_kiq_unmap_queues(adev: &mut amdgpu_device, kiq_ring: usize, r: usize) {
    let ring = &adev.rings[r];
    let eng_sel = if ring.funcs.type_ == AMDGPU_RING_TYPE_GFX { 4 } else { 0 };
    let doorbell_index = ring.doorbell_index;
    let w = |adev: &mut amdgpu_device, v: u32| crate::amdgpu_ring::amdgpu_ring_write(adev, kiq_ring, v);
    w(adev, PACKET3(PACKET3_UNMAP_QUEUES, 4));
    // Q_sel: 0, vmid: 0, engine: 0, num_Q: 1
    w(adev, PACKET3_UNMAP_QUEUES_ACTION_1 * RESET_QUEUES
        | PACKET3_UNMAP_QUEUES_QUEUE_SEL_1 * 0
        | PACKET3_UNMAP_QUEUES_ENGINE_SEL_1 * eng_sel
        | PACKET3_UNMAP_QUEUES_NUM_QUEUES_1);
    w(adev, PACKET3_UNMAP_QUEUES_DOORBELL_OFFSET0_1 * doorbell_index);
    w(adev, 0);
    w(adev, 0);
    w(adev, 0);
}

/// `amdgpu_gfx_disable_kcq`: no MES.
fn amdgpu_gfx_disable_kcq(adev: &mut amdgpu_device) -> i32 {
    let Some(kiq_ring) = adev.gfx.kiq.ring else { return -22 };
    if !adev.rings[kiq_ring].sched.ready {
        return 0;
    }
    if crate::amdgpu_ring::amdgpu_ring_alloc(adev, kiq_ring,
        GFX_V9_0_KIQ_UNMAP_QUEUES_SIZE * adev.gfx.num_compute_rings) != 0
    {
        return -12;
    }
    for i in 0..adev.gfx.num_compute_rings as usize {
        let c = adev.gfx.compute_ring[i];
        gfx_v9_0_kiq_unmap_queues(adev, kiq_ring, c);
    }
    // Submit unmap queue packet
    crate::amdgpu_ring::amdgpu_ring_commit(adev, kiq_ring);
    // Ring test will do a basic scratch register change check. Just run
    // this to ensure that unmap queues that is submitted before got
    // processed successfully before returning.
    crate::amdgpu_ring::amdgpu_ring_test_helper(adev, kiq_ring)
}

/// `gfx_v9_0_kiq_fini_register`.
fn gfx_v9_0_kiq_fini_register(adev: &amdgpu_device) {
    // disable the queue if it's active
    if gc_rd(adev, GC_mmCP_HQD_ACTIVE_BASE_IDX, GC_mmCP_HQD_ACTIVE) & 1 != 0 {
        gc_wr(adev, GC_mmCP_HQD_DEQUEUE_REQUEST_BASE_IDX, GC_mmCP_HQD_DEQUEUE_REQUEST, 1);
        let mut j = 0;
        while j < adev.usec_timeout {
            if gc_rd(adev, GC_mmCP_HQD_ACTIVE_BASE_IDX, GC_mmCP_HQD_ACTIVE) & 1 == 0 {
                break;
            }
            crate::linux::udelay(1);
            j += 1;
        }
        if j == AMDGPU_MAX_USEC_TIMEOUT {
            Line::new().s("KIQ dequeue request failed.").dbg();
            // Manual disable if dequeue request times out
            gc_wr(adev, GC_mmCP_HQD_ACTIVE_BASE_IDX, GC_mmCP_HQD_ACTIVE, 0);
        }
        gc_wr(adev, GC_mmCP_HQD_DEQUEUE_REQUEST_BASE_IDX, GC_mmCP_HQD_DEQUEUE_REQUEST, 0);
    }
    gc_wr(adev, GC_mmCP_HQD_IQ_TIMER_BASE_IDX, GC_mmCP_HQD_IQ_TIMER, 0);
    gc_wr(adev, GC_mmCP_HQD_IB_CONTROL_BASE_IDX, GC_mmCP_HQD_IB_CONTROL, 0);
    gc_wr(adev, GC_mmCP_HQD_PERSISTENT_STATE_BASE_IDX, GC_mmCP_HQD_PERSISTENT_STATE, 0);
    gc_wr(adev, GC_mmCP_HQD_PQ_DOORBELL_CONTROL_BASE_IDX, GC_mmCP_HQD_PQ_DOORBELL_CONTROL, 0x40000000);
    gc_wr(adev, GC_mmCP_HQD_PQ_DOORBELL_CONTROL_BASE_IDX, GC_mmCP_HQD_PQ_DOORBELL_CONTROL, 0);
    gc_wr(adev, GC_mmCP_HQD_PQ_RPTR_BASE_IDX, GC_mmCP_HQD_PQ_RPTR, 0);
    gc_wr(adev, GC_mmCP_HQD_PQ_WPTR_HI_BASE_IDX, GC_mmCP_HQD_PQ_WPTR_HI, 0);
    gc_wr(adev, GC_mmCP_HQD_PQ_WPTR_LO_BASE_IDX, GC_mmCP_HQD_PQ_WPTR_LO, 0);
}

/// `gfx_v9_0_hw_fini`: bare metal, not in reset or suspend, GC 9.3.0.
pub fn gfx_v9_0_hw_fini(adev: &mut amdgpu_device) -> i32 {
    for name in ["gfx.priv_reg_irq", "gfx.priv_inst_irq", "gfx.bad_op_irq"] {
        crate::amdgpu_irq::amdgpu_irq_put(adev, name, 0);
    }
    // DF freeze and kcq disable will fail; no RAS interrupt here.
    // disable KCQ to avoid CPC touch memory not valid anymore
    amdgpu_gfx_disable_kcq(adev);

    // Use deinitialize sequence from CAIL when unbinding device from
    // driver, otherwise KIQ is hanging when binding back
    if let Some(k) = adev.gfx.kiq.ring {
        let (me, pipe, queue) = (adev.rings[k].me, adev.rings[k].pipe, adev.rings[k].queue);
        soc15_grbm_select(adev, me, pipe, queue, 0);
        gfx_v9_0_kiq_fini_register(adev);
        soc15_grbm_select(adev, 0, 0, 0, 0);
    }

    // gfx_v9_0_cp_enable(adev, false)
    if adev.gfx.num_gfx_rings != 0 {
        gfx_v9_0_cp_gfx_enable(adev, false);
    }
    gfx_v9_0_cp_compute_enable(adev, false);

    // Skip stopping RLC with A+A reset or when RLC controls GFX clock:
    // GC 9.3.0 is below 9.4.2.
    gfx_v9_0_rlc_stop(adev);
    0
}
