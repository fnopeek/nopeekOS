//! amdgpu/gfx_v9_0.c, amdgpu_gfx.c and amdgpu_rlc.c (Linux 6.18.55): the
//! GFX block's early_init, which requests and reads the CP and RLC
//! microcode. GC 9.3.0 loads no MEC2 image.

use crate::amdgpu_device::amdgpu_device;
use crate::amdgpu_ucode::{amdgpu_ucode_ip_version_decode, amdgpu_ucode_request, fw_source, le16, le32, page_align};
use crate::gmc_v9_0::amdgpu_irq_src;
use crate::linux::{firmware, Line};
use crate::regs::*;
use alloc::vec::Vec;

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
