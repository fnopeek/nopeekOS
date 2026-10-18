//! amdgpu/amdgpu_ucode.c (Linux 6.18.55): naming, requesting and checking
//! the microcode, and the table PSP loads it from (`adev->firmware`).

use crate::amdgpu_device::amdgpu_device;
use crate::linux::{firmware, request_firmware, Line};
use crate::regs::*;
use alloc::vec::Vec;

/// `le32` of a firmware image at `at`; 0 past its end.
pub fn le32(fw: &firmware, at: u32) -> u32 {
    let at = at as usize;
    fw.data.get(at..at + 4).map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// `le16` of a firmware image at `at`; 0 past its end.
pub fn le16(fw: &firmware, at: u32) -> u16 {
    let at = at as usize;
    fw.data.get(at..at + 2).map_or(0, |b| u16::from_le_bytes([b[0], b[1]]))
}

/// `struct amdgpu_firmware_info`: which image a ucode id loads from.
#[derive(Clone, Copy, Default)]
pub struct amdgpu_firmware_info {
    pub ucode_id: u32,
    /// The image `fw` points at, by the field of `adev` that holds it.
    pub fw: Option<fw_source>,
    pub ucode_size: u32,
    pub mc_addr: u64,
    /// `kaddr`: a byte offset into `adev->firmware.fw_buf`.
    pub kaddr: u64,
    pub tmr_mc_addr_lo: u32,
    pub tmr_mc_addr_hi: u32,
}

/// Where an `amdgpu_firmware_info.fw` points.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum fw_source {
    gfx_pfp, gfx_me, gfx_ce, gfx_mec, gfx_mec2, gfx_rlc, sdma(u32), dm_dmub, vcn(u32),
}

/// `struct amdgpu_firmware`, the part early_init fills.
pub struct amdgpu_firmware {
    pub ucode: Vec<amdgpu_firmware_info>,
    pub load_type: u32,
    pub fw_size: u32,
    pub max_ucodes: u32,
    pub fw_buf: Option<usize>,
    pub fw_buf_mc: u64,
    pub fw_buf_ptr: Option<crate::amdgpu_object::amdgpu_kptr>,
    /// `rbuf`: the PSP ring's buffer.
    pub rbuf: Option<usize>,
}

impl Default for amdgpu_firmware {
    fn default() -> Self {
        amdgpu_firmware {
            ucode: alloc::vec![amdgpu_firmware_info::default(); AMDGPU_UCODE_ID_MAXIMUM as usize],
            load_type: 0,
            fw_size: 0,
            max_ucodes: 0,
            fw_buf: None,
            fw_buf_mc: 0,
            fw_buf_ptr: None,
            rbuf: None,
        }
    }
}

/// `ALIGN(x, PAGE_SIZE)`.
pub fn page_align(x: u32) -> u32 {
    x.wrapping_add(4095) & !4095
}

/// `amdgpu_ucode_validate`.
fn amdgpu_ucode_validate(fw: &firmware) -> i32 {
    if fw.data.len() as u32 == le32(fw, COMMON_FIRMWARE_HEADER_size_bytes) {
        0
    } else {
        -22
    }
}

/// `amdgpu_ucode_legacy_naming`: the cases Renoir's versions reach
/// (tools/gpu_amd_renoir.ipversions).
fn amdgpu_ucode_legacy_naming(adev: &amdgpu_device, block_type: u32) -> Option<&'static str> {
    let v = adev.ip_version(block_type, 0);
    let renoir = adev.apu_flags & AMD_APU_IS_RENOIR != 0;
    if block_type == MP0_HWIP {
        (v == 11 << 24 | 3 << 8).then_some("renoir")
    } else if block_type == SDMA0_HWIP {
        (v == 4 << 24 | 1 << 16 | 2 << 8).then_some(if renoir { "renoir_sdma" } else { "green_sardine_sdma" })
    } else if block_type == UVD_HWIP {
        (v == 2 << 24 | 2 << 16).then_some(if renoir { "renoir_vcn" } else { "green_sardine_vcn" })
    } else if block_type == GC_HWIP {
        (v == 9 << 24 | 3 << 16).then_some(if renoir { "renoir" } else { "green_sardine" })
    } else {
        None
    }
}

/// `amdgpu_ucode_ip_version_decode`: the prefix of a block's firmware names.
pub fn amdgpu_ucode_ip_version_decode(adev: &amdgpu_device, block_type: u32) -> alloc::string::String {
    if let Some(legacy) = amdgpu_ucode_legacy_naming(adev, block_type) {
        return alloc::string::String::from(legacy);
    }
    let ip_name = match block_type {
        GC_HWIP => "gc",
        SDMA0_HWIP => "sdma",
        MP0_HWIP => "psp",
        MP1_HWIP => "smu",
        UVD_HWIP => "vcn",
        VPE_HWIP => "vpe",
        ISP_HWIP => "isp",
        _ => panic!("amdgpu_ucode_ip_version_decode: block {}", block_type),
    };
    let v = adev.ip_version(block_type, 0);
    alloc::format!("{}_{}_{}_{}", ip_name, v >> 24, (v >> 16) & 0xff, (v >> 8) & 0xff)
}

/// `amdgpu_ucode_request`, required: request and validate. The error is
/// -ENODEV for a missing file, -EINVAL for one that fails validation, and
/// in that case the image is still handed back, as Linux leaves it in `*fw`
/// for `amdgpu_ucode_release`.
pub fn amdgpu_ucode_request(name: &str) -> (Option<firmware>, i32) {
    if name.len() >= AMDGPU_UCODE_NAME_MAX as usize {
        Line::new().s("amdgpu firmware name buffer overflow").say();
        return (None, -75);
    }
    let fw = match request_firmware(name.as_bytes()) {
        Ok(fw) => fw,
        Err(_) => {
            Line::new().s("Direct firmware load for ").s(name).s(" failed with error -2").say();
            return (None, -19);
        }
    };
    let r = amdgpu_ucode_validate(&fw);
    if r != 0 {
        Line::new().s("\"").s(name).s("\" failed to validate").dbg();
    }
    (Some(fw), r)
}

/// `amdgpu_is_kicker_fw`.
pub fn amdgpu_is_kicker_fw(adev: &amdgpu_device) -> bool {
    let list = [(KICKER_DEVICE_0_device, KICKER_DEVICE_0_revision), (KICKER_DEVICE_1_device, KICKER_DEVICE_1_revision)];
    list[..KICKER_DEVICE_LIST_COUNT as usize].iter().any(|&(d, r)| adev.device as u32 == d && adev.revision as u32 == r)
}

/// `amdgpu_ucode_create_bo`: bare metal, the buffer in GTT.
pub fn amdgpu_ucode_create_bo(adev: &mut amdgpu_device) -> i32 {
    let lt = adev.firmware.load_type;
    if lt != AMDGPU_FW_LOAD_DIRECT && lt != AMDGPU_FW_LOAD_RLC_BACKDOOR_AUTO {
        let size = adev.firmware.fw_size as u64;
        match crate::amdgpu_object::amdgpu_bo_create_kernel(adev, size, crate::amdgpu_object::PAGE_SIZE,
            AMDGPU_GEM_DOMAIN_GTT, true) {
            Ok(Some(h)) => {
                adev.firmware.fw_buf = Some(h);
                adev.firmware.fw_buf_mc = crate::amdgpu_object::amdgpu_bo_gpu_offset(adev, h);
                adev.firmware.fw_buf_ptr = crate::amdgpu_object::bo(adev, h).kptr;
            }
            _ => {
                Line::new().s("failed to create kernel buffer for firmware.fw_buf").say();
                return -12;
            }
        }
    }
    0
}

/// The image and the bytes `amdgpu_ucode_init_single_fw` copies for one
/// ucode, with PSP loading: (size, offset into the image). `None` for an
/// id whose header this port does not read; none of those is loaded on
/// Renoir.
fn single_fw_span(adev: &amdgpu_device, id: u32, fw: &firmware) -> Option<(u32, u32)> {
    let size_bytes = le32(fw, COMMON_FIRMWARE_HEADER_ucode_size_bytes);
    let array = le32(fw, COMMON_FIRMWARE_HEADER_ucode_array_offset_bytes);
    let r = &adev.gfx.rlc;
    let rlc = |size: u32, at: Option<u32>| at.map(|a| (size, a));
    match id {
        AMDGPU_UCODE_ID_CP_MEC1 | AMDGPU_UCODE_ID_CP_MEC2 => {
            Some((size_bytes.wrapping_sub(le32(fw, GFX_HDR_JT_SIZE).wrapping_mul(4)), array))
        }
        AMDGPU_UCODE_ID_CP_MEC1_JT | AMDGPU_UCODE_ID_CP_MEC2_JT => {
            Some((le32(fw, GFX_HDR_JT_SIZE).wrapping_mul(4), array.wrapping_add(le32(fw, GFX_HDR_JT_OFFSET).wrapping_mul(4))))
        }
        AMDGPU_UCODE_ID_RLC_RESTORE_LIST_CNTL => rlc(r.save_restore_list_cntl_size_bytes, r.save_restore_list_cntl),
        AMDGPU_UCODE_ID_RLC_RESTORE_LIST_GPM_MEM => rlc(r.save_restore_list_gpm_size_bytes, r.save_restore_list_gpm),
        AMDGPU_UCODE_ID_RLC_RESTORE_LIST_SRM_MEM => rlc(r.save_restore_list_srm_size_bytes, r.save_restore_list_srm),
        AMDGPU_UCODE_ID_RLC_IRAM => rlc(r.rlc_iram_ucode_size_bytes, r.rlc_iram_ucode),
        AMDGPU_UCODE_ID_RLC_DRAM => rlc(r.rlc_dram_ucode_size_bytes, r.rlc_dram_ucode),
        AMDGPU_UCODE_ID_RLC_P => rlc(r.rlcp_ucode_size_bytes, r.rlcp_ucode),
        AMDGPU_UCODE_ID_RLC_V => rlc(r.rlcv_ucode_size_bytes, r.rlcv_ucode),
        AMDGPU_UCODE_ID_GLOBAL_TAP_DELAYS => rlc(r.global_tap_delays_ucode_size_bytes, r.global_tap_delays_ucode),
        AMDGPU_UCODE_ID_SE0_TAP_DELAYS => rlc(r.se0_tap_delays_ucode_size_bytes, r.se0_tap_delays_ucode),
        AMDGPU_UCODE_ID_SE1_TAP_DELAYS => rlc(r.se1_tap_delays_ucode_size_bytes, r.se1_tap_delays_ucode),
        AMDGPU_UCODE_ID_SE2_TAP_DELAYS => rlc(r.se2_tap_delays_ucode_size_bytes, r.se2_tap_delays_ucode),
        AMDGPU_UCODE_ID_SE3_TAP_DELAYS => rlc(r.se3_tap_delays_ucode_size_bytes, r.se3_tap_delays_ucode),
        AMDGPU_UCODE_ID_DMCUB => Some((le32(fw, DMCUB_HDR_INST_CONST_BYTES), array)),
        AMDGPU_UCODE_ID_SDMA_UCODE_TH0 | AMDGPU_UCODE_ID_SDMA_UCODE_TH1 | AMDGPU_UCODE_ID_SDMA_RS64
        | AMDGPU_UCODE_ID_CP_MES | AMDGPU_UCODE_ID_CP_MES_DATA | AMDGPU_UCODE_ID_CP_MES1
        | AMDGPU_UCODE_ID_CP_MES1_DATA | AMDGPU_UCODE_ID_DMCU_ERAM | AMDGPU_UCODE_ID_DMCU_INTV
        | AMDGPU_UCODE_ID_PPTABLE | AMDGPU_UCODE_ID_P2S_TABLE | AMDGPU_UCODE_ID_IMU_I | AMDGPU_UCODE_ID_IMU_D
        | AMDGPU_UCODE_ID_CP_RS64_PFP | AMDGPU_UCODE_ID_CP_RS64_PFP_P0_STACK | AMDGPU_UCODE_ID_CP_RS64_PFP_P1_STACK
        | AMDGPU_UCODE_ID_CP_RS64_ME | AMDGPU_UCODE_ID_CP_RS64_ME_P0_STACK | AMDGPU_UCODE_ID_CP_RS64_ME_P1_STACK
        | AMDGPU_UCODE_ID_CP_RS64_MEC | AMDGPU_UCODE_ID_CP_RS64_MEC_P0_STACK | AMDGPU_UCODE_ID_CP_RS64_MEC_P1_STACK
        | AMDGPU_UCODE_ID_CP_RS64_MEC_P2_STACK | AMDGPU_UCODE_ID_CP_RS64_MEC_P3_STACK | AMDGPU_UCODE_ID_VPE_CTX
        | AMDGPU_UCODE_ID_VPE_CTL | AMDGPU_UCODE_ID_UMSCH_MM_UCODE | AMDGPU_UCODE_ID_UMSCH_MM_DATA => None,
        _ => Some((size_bytes, array)),
    }
}

/// `amdgpu_ucode_init_single_fw`, PSP loading.
fn amdgpu_ucode_init_single_fw(adev: &mut amdgpu_device, i: usize, mc_addr: u64, kaddr: u64) -> i32 {
    let Some(src) = adev.firmware.ucode[i].fw else { return 0 };
    adev.firmware.ucode[i].mc_addr = mc_addr;
    adev.firmware.ucode[i].kaddr = kaddr;
    let id = adev.firmware.ucode[i].ucode_id;
    if id == AMDGPU_UCODE_ID_STORAGE {
        return 0;
    }
    let Some(fw) = crate::gfx_v9_0::fw_of(adev, src) else { return 0 };
    let span = if adev.firmware.load_type == AMDGPU_FW_LOAD_PSP {
        single_fw_span(adev, id, fw)
    } else {
        Some((le32(fw, COMMON_FIRMWARE_HEADER_ucode_size_bytes), le32(fw, COMMON_FIRMWARE_HEADER_ucode_array_offset_bytes)))
    };
    let Some((size, at)) = span else {
        Line::new().s("amdgpu_ucode_init_single_fw: ucode id ").dec(id as u64).s(" is not ported").say();
        return -22;
    };
    let bytes = fw.data.get(at as usize..at as usize + size as usize).unwrap_or(&[]).to_vec();
    adev.firmware.ucode[i].ucode_size = size;
    if let Some(k) = adev.firmware.fw_buf_ptr {
        crate::amdgpu_gart::kptr_write(adev, k, kaddr, &bytes);
    }
    0
}

/// `amdgpu_ucode_patch_jt`: the jump table after the page-aligned image.
fn amdgpu_ucode_patch_jt(adev: &mut amdgpu_device, i: usize) {
    let Some(src) = adev.firmware.ucode[i].fw else { return };
    let Some(fw) = crate::gfx_v9_0::fw_of(adev, src) else { return };
    let kaddr = adev.firmware.ucode[i].kaddr;
    let dst = kaddr + page_align(le32(fw, COMMON_FIRMWARE_HEADER_ucode_size_bytes)) as u64;
    let at = le32(fw, COMMON_FIRMWARE_HEADER_ucode_array_offset_bytes) as usize + le32(fw, GFX_HDR_JT_OFFSET) as usize * 4;
    let n = le32(fw, GFX_HDR_JT_SIZE) as usize * 4;
    let bytes = fw.data.get(at..at + n).unwrap_or(&[]).to_vec();
    if let Some(k) = adev.firmware.fw_buf_ptr {
        crate::amdgpu_gart::kptr_write(adev, k, dst, &bytes);
    }
}

/// `amdgpu_ucode_init_bo`: bare metal, not in reset or suspend.
pub fn amdgpu_ucode_init_bo(adev: &mut amdgpu_device) -> i32 {
    // if SMU loaded firmware, it needn't add SMC, UVD, and VCE ucode info
    // here
    adev.firmware.max_ucodes = if adev.firmware.load_type != AMDGPU_FW_LOAD_PSP {
        AMDGPU_UCODE_ID_MAXIMUM - 4
    } else {
        AMDGPU_UCODE_ID_MAXIMUM
    };
    let mut fw_offset = 0u64;
    for i in 0..adev.firmware.max_ucodes as usize {
        let Some(src) = adev.firmware.ucode[i].fw else { continue };
        let mc = adev.firmware.fw_buf_mc + fw_offset;
        let r = amdgpu_ucode_init_single_fw(adev, i, mc, fw_offset);
        if r != 0 {
            return r;
        }
        if i as u32 == AMDGPU_UCODE_ID_CP_MEC1 && adev.firmware.load_type != AMDGPU_FW_LOAD_PSP {
            amdgpu_ucode_patch_jt(adev, i);
            let jt = crate::gfx_v9_0::fw_of(adev, src).map_or(0, |fw| le32(fw, GFX_HDR_JT_SIZE));
            fw_offset += page_align(jt << 2) as u64;
        }
        fw_offset += page_align(adev.firmware.ucode[i].ucode_size) as u64;
    }
    0
}
