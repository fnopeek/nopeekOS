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
