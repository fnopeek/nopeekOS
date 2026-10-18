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
}

impl Default for amdgpu_firmware {
    fn default() -> Self {
        amdgpu_firmware {
            ucode: alloc::vec![amdgpu_firmware_info::default(); AMDGPU_UCODE_ID_MAXIMUM as usize],
            load_type: 0,
            fw_size: 0,
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
