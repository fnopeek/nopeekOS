//! drivers/video/hdmi.c + include/linux/hdmi.h (Linux rpi-6.18): the
//! AVI, SPD and HDMI vendor infoframes and their packing. The audio
//! infoframe (written by ALSA) and the Dynamic Range and Mastering
//! infoframe (from an HDR metadata property) have no source here and are
//! not ported.

use crate::linux::{EINVAL, ENOSPC};
use crate::regs::*;

/// `struct hdmi_avi_infoframe`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct hdmi_avi_infoframe {
    pub r#type: u32,
    pub version: u8,
    pub length: u8,
    pub colorspace: u32,
    pub scan_mode: u32,
    pub colorimetry: u32,
    pub picture_aspect: u32,
    pub active_aspect: u32,
    pub itc: bool,
    pub extended_colorimetry: u32,
    pub quantization_range: u32,
    pub nups: u32,
    pub video_code: u8,
    pub ycc_quantization_range: u32,
    pub content_type: u32,
    pub pixel_repeat: u8,
    pub top_bar: u16,
    pub bottom_bar: u16,
    pub left_bar: u16,
    pub right_bar: u16,
}

/// `struct hdmi_spd_infoframe`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct hdmi_spd_infoframe {
    pub r#type: u32,
    pub version: u8,
    pub length: u8,
    pub vendor: [u8; 8],
    pub product: [u8; 16],
    pub sdi: u32,
}

/// `struct hdmi_vendor_infoframe`. `s3d_struct` is the signed enum
/// (`HDMI_3D_STRUCTURE_INVALID` is -1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct hdmi_vendor_infoframe {
    pub r#type: u32,
    pub version: u8,
    pub length: u8,
    pub oui: u32,
    pub vic: u8,
    pub s3d_struct: i32,
    pub s3d_ext_data: u32,
}

/// `union hdmi_infoframe`, the kinds this crate builds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum hdmi_infoframe {
    avi(hdmi_avi_infoframe),
    spd(hdmi_spd_infoframe),
    vendor(hdmi_vendor_infoframe),
}

impl hdmi_infoframe {
    /// `frame->any.type`.
    pub fn r#type(&self) -> u32 {
        match self {
            hdmi_infoframe::avi(f) => f.r#type,
            hdmi_infoframe::spd(f) => f.r#type,
            hdmi_infoframe::vendor(f) => f.r#type,
        }
    }
}

/// `hdmi_infoframe_checksum`.
fn hdmi_infoframe_checksum(ptr: &[u8]) -> u8 {
    let mut csum: u8 = 0;

    // compute checksum
    for &b in ptr {
        csum = csum.wrapping_add(b);
    }

    0u8.wrapping_sub(csum)
}

/// `hdmi_infoframe_set_checksum`.
fn hdmi_infoframe_set_checksum(buffer: &mut [u8], size: usize) {
    buffer[3] = hdmi_infoframe_checksum(&buffer[..size]);
}

/// `hdmi_avi_infoframe_init`.
pub fn hdmi_avi_infoframe_init(frame: &mut hdmi_avi_infoframe) {
    *frame = hdmi_avi_infoframe::default();

    frame.r#type = HDMI_INFOFRAME_TYPE_AVI;
    frame.version = 2;
    frame.length = HDMI_AVI_INFOFRAME_SIZE as u8;
}

/// `hdmi_avi_infoframe_check_only`.
fn hdmi_avi_infoframe_check_only(frame: &hdmi_avi_infoframe) -> i32 {
    if frame.r#type != HDMI_INFOFRAME_TYPE_AVI || frame.version != 2
        || frame.length as u32 != HDMI_AVI_INFOFRAME_SIZE
    {
        return -EINVAL;
    }

    if frame.picture_aspect > HDMI_PICTURE_ASPECT_16_9 {
        return -EINVAL;
    }

    0
}

/// `hdmi_avi_infoframe_pack_only`: the packed length or a negative errno.
pub fn hdmi_avi_infoframe_pack_only(frame: &hdmi_avi_infoframe, buffer: &mut [u8]) -> isize {
    let ret = hdmi_avi_infoframe_check_only(frame);
    if ret != 0 {
        return ret as isize;
    }

    let length = HDMI_INFOFRAME_HEADER_SIZE as usize + frame.length as usize;

    if buffer.len() < length {
        return -ENOSPC as isize;
    }

    buffer.fill(0);

    buffer[0] = frame.r#type as u8;
    buffer[1] = frame.version;
    buffer[2] = frame.length;
    buffer[3] = 0; // checksum

    // start infoframe payload
    let ptr = &mut buffer[HDMI_INFOFRAME_HEADER_SIZE as usize..];

    ptr[0] = (((frame.colorspace & 0x3) << 5) | (frame.scan_mode & 0x3)) as u8;

    // Data byte 1, bit 4 has to be set if we provide the active format
    // aspect ratio
    if frame.active_aspect & 0xf != 0 {
        ptr[0] |= 1 << 4;
    }

    // Bit 3 and 2 indicate if we transmit horizontal/vertical bar data
    if frame.top_bar != 0 || frame.bottom_bar != 0 {
        ptr[0] |= 1 << 3;
    }

    if frame.left_bar != 0 || frame.right_bar != 0 {
        ptr[0] |= 1 << 2;
    }

    ptr[1] = (((frame.colorimetry & 0x3) << 6) | ((frame.picture_aspect & 0x3) << 4) | (frame.active_aspect & 0xf))
        as u8;

    ptr[2] = (((frame.extended_colorimetry & 0x7) << 4) | ((frame.quantization_range & 0x3) << 2)
        | (frame.nups & 0x3)) as u8;

    if frame.itc {
        ptr[2] |= 1 << 7;
    }

    ptr[3] = frame.video_code & 0x7f;

    ptr[4] = (((frame.ycc_quantization_range & 0x3) << 6) | ((frame.content_type & 0x3) << 4)
        | (frame.pixel_repeat as u32 & 0xf)) as u8;

    ptr[5] = (frame.top_bar & 0xff) as u8;
    ptr[6] = ((frame.top_bar >> 8) & 0xff) as u8;
    ptr[7] = (frame.bottom_bar & 0xff) as u8;
    ptr[8] = ((frame.bottom_bar >> 8) & 0xff) as u8;
    ptr[9] = (frame.left_bar & 0xff) as u8;
    ptr[10] = ((frame.left_bar >> 8) & 0xff) as u8;
    ptr[11] = (frame.right_bar & 0xff) as u8;
    ptr[12] = ((frame.right_bar >> 8) & 0xff) as u8;

    hdmi_infoframe_set_checksum(buffer, length);

    length as isize
}

/// `hdmi_avi_infoframe_pack`.
pub fn hdmi_avi_infoframe_pack(frame: &mut hdmi_avi_infoframe, buffer: &mut [u8]) -> isize {
    let ret = hdmi_avi_infoframe_check_only(frame);
    if ret != 0 {
        return ret as isize;
    }

    hdmi_avi_infoframe_pack_only(frame, buffer)
}

/// `hdmi_spd_infoframe_init`.
pub fn hdmi_spd_infoframe_init(frame: &mut hdmi_spd_infoframe, vendor: &str, product: &str) -> i32 {
    *frame = hdmi_spd_infoframe::default();

    frame.r#type = HDMI_INFOFRAME_TYPE_SPD;
    frame.version = 1;
    frame.length = HDMI_SPD_INFOFRAME_SIZE as u8;

    let len = vendor.len().min(frame.vendor.len());
    frame.vendor[..len].copy_from_slice(&vendor.as_bytes()[..len]);
    let len = product.len().min(frame.product.len());
    frame.product[..len].copy_from_slice(&product.as_bytes()[..len]);

    0
}

/// `hdmi_spd_infoframe_check_only`.
fn hdmi_spd_infoframe_check_only(frame: &hdmi_spd_infoframe) -> i32 {
    if frame.r#type != HDMI_INFOFRAME_TYPE_SPD || frame.version != 1
        || frame.length as u32 != HDMI_SPD_INFOFRAME_SIZE
    {
        return -EINVAL;
    }

    0
}

/// `hdmi_spd_infoframe_pack_only`.
pub fn hdmi_spd_infoframe_pack_only(frame: &hdmi_spd_infoframe, buffer: &mut [u8]) -> isize {
    let ret = hdmi_spd_infoframe_check_only(frame);
    if ret != 0 {
        return ret as isize;
    }

    let length = HDMI_INFOFRAME_HEADER_SIZE as usize + frame.length as usize;

    if buffer.len() < length {
        return -ENOSPC as isize;
    }

    buffer.fill(0);

    buffer[0] = frame.r#type as u8;
    buffer[1] = frame.version;
    buffer[2] = frame.length;
    buffer[3] = 0; // checksum

    // start infoframe payload
    let ptr = &mut buffer[HDMI_INFOFRAME_HEADER_SIZE as usize..];

    ptr[..8].copy_from_slice(&frame.vendor);
    ptr[8..24].copy_from_slice(&frame.product);

    ptr[24] = frame.sdi as u8;

    hdmi_infoframe_set_checksum(buffer, length);

    length as isize
}

/// `hdmi_spd_infoframe_pack`.
pub fn hdmi_spd_infoframe_pack(frame: &mut hdmi_spd_infoframe, buffer: &mut [u8]) -> isize {
    let ret = hdmi_spd_infoframe_check_only(frame);
    if ret != 0 {
        return ret as isize;
    }

    hdmi_spd_infoframe_pack_only(frame, buffer)
}

/// `hdmi_vendor_infoframe_init`.
pub fn hdmi_vendor_infoframe_init(frame: &mut hdmi_vendor_infoframe) -> i32 {
    *frame = hdmi_vendor_infoframe::default();

    frame.r#type = HDMI_INFOFRAME_TYPE_VENDOR;
    frame.version = 1;

    frame.oui = HDMI_IEEE_OUI;

    // 0 is a valid value for s3d_struct, so we use a special "not set"
    // value
    frame.s3d_struct = HDMI_3D_STRUCTURE_INVALID as i32;
    frame.length = HDMI_VENDOR_INFOFRAME_SIZE as u8;

    0
}

/// `hdmi_vendor_infoframe_length`.
fn hdmi_vendor_infoframe_length(frame: &hdmi_vendor_infoframe) -> u8 {
    // for side by side (half) we also need to provide 3D_Ext_Data
    if frame.s3d_struct >= HDMI_3D_STRUCTURE_SIDE_BY_SIDE_HALF as i32 {
        6
    } else if frame.vic != 0 || frame.s3d_struct != HDMI_3D_STRUCTURE_INVALID as i32 {
        5
    } else {
        4
    }
}

/// `hdmi_vendor_infoframe_check_only`.
fn hdmi_vendor_infoframe_check_only(frame: &hdmi_vendor_infoframe) -> i32 {
    if frame.r#type != HDMI_INFOFRAME_TYPE_VENDOR || frame.version != 1 || frame.oui != HDMI_IEEE_OUI {
        return -EINVAL;
    }

    // only one of those can be supplied
    if frame.vic != 0 && frame.s3d_struct != HDMI_3D_STRUCTURE_INVALID as i32 {
        return -EINVAL;
    }

    if frame.length != hdmi_vendor_infoframe_length(frame) {
        return -EINVAL;
    }

    0
}

/// `hdmi_vendor_infoframe_check`: fixes the length up first.
pub fn hdmi_vendor_infoframe_check(frame: &mut hdmi_vendor_infoframe) -> i32 {
    frame.length = hdmi_vendor_infoframe_length(frame);

    hdmi_vendor_infoframe_check_only(frame)
}

/// `hdmi_vendor_infoframe_pack_only`.
pub fn hdmi_vendor_infoframe_pack_only(frame: &hdmi_vendor_infoframe, buffer: &mut [u8]) -> isize {
    let ret = hdmi_vendor_infoframe_check_only(frame);
    if ret != 0 {
        return ret as isize;
    }

    let length = HDMI_INFOFRAME_HEADER_SIZE as usize + frame.length as usize;

    if buffer.len() < length {
        return -ENOSPC as isize;
    }

    buffer.fill(0);

    buffer[0] = frame.r#type as u8;
    buffer[1] = frame.version;
    buffer[2] = frame.length;
    buffer[3] = 0; // checksum

    // HDMI OUI
    buffer[4] = 0x03;
    buffer[5] = 0x0c;
    buffer[6] = 0x00;

    if frame.s3d_struct != HDMI_3D_STRUCTURE_INVALID as i32 {
        buffer[7] = 0x2 << 5; // video format
        buffer[8] = ((frame.s3d_struct & 0xf) << 4) as u8;
        if frame.s3d_struct >= HDMI_3D_STRUCTURE_SIDE_BY_SIDE_HALF as i32 {
            buffer[9] = ((frame.s3d_ext_data & 0xf) << 4) as u8;
        }
    } else if frame.vic != 0 {
        buffer[7] = 0x1 << 5; // video format
        buffer[8] = frame.vic;
    } else {
        buffer[7] = 0x0 << 5; // video format
    }

    hdmi_infoframe_set_checksum(buffer, length);

    length as isize
}

/// `hdmi_vendor_infoframe_pack`.
pub fn hdmi_vendor_infoframe_pack(frame: &mut hdmi_vendor_infoframe, buffer: &mut [u8]) -> isize {
    let ret = hdmi_vendor_infoframe_check(frame);
    if ret != 0 {
        return ret as isize;
    }

    hdmi_vendor_infoframe_pack_only(frame, buffer)
}

/// `hdmi_infoframe_pack`: check, then pack into `buffer`.
pub fn hdmi_infoframe_pack(frame: &mut hdmi_infoframe, buffer: &mut [u8]) -> isize {
    match frame {
        hdmi_infoframe::avi(f) => hdmi_avi_infoframe_pack(f, buffer),
        hdmi_infoframe::spd(f) => hdmi_spd_infoframe_pack(f, buffer),
        hdmi_infoframe::vendor(f) => hdmi_vendor_infoframe_pack(f, buffer),
    }
}
