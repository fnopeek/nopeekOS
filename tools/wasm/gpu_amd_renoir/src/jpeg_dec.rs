//! Host-side JPEG decode on the JPEG block: the bitstream as the engine
//! wants it and the command stream that runs it, after Mesa 26.2.6
//! `src/amd/common/ac_vcn_dec.c` (`vcn_jpeg_build_decode_cmd`,
//! `RDECODE_JPEG_VER_2` for VCN 2.x) and `src/gallium/frontends/va/
//! picture_mjpeg.c` (`vlVaGetJpegSliceHeader`, the headers the engine
//! parses). The output is packed YUYV, linear, for a 4:2:2 frame.

use alloc::vec::Vec;

const RDECODE_TILE_LINEAR: u32 = 0;
const ADDR_SW_LINEAR: u32 = 0;
const COND0: u32 = 0;
const COND3: u32 = 3;
const TYPE0: u32 = 0;
const TYPE3: u32 = 3;

const UVD_JPEG_DEC_SOFT_RST: u32 = 0x402f;
const UVD_JRBC_IB_COND_RD_TIMER: u32 = 0x408e;
const UVD_JRBC_IB_REF_DATA: u32 = 0x408f;
const UVD_LMI_JPEG_READ_64BIT_BAR_HIGH: u32 = 0x40e1;
const UVD_LMI_JPEG_READ_64BIT_BAR_LOW: u32 = 0x40e0;
const UVD_JPEG_RB_BASE: u32 = 0x4001;
const UVD_JPEG_RB_SIZE: u32 = 0x4004;
const UVD_JPEG_RB_WPTR: u32 = 0x4002;
const UVD_JPEG_PITCH: u32 = 0x401f;
const UVD_JPEG_UV_PITCH: u32 = 0x4020;
const JPEG_DEC_ADDR_MODE: u32 = 0x4027;
const JPEG_DEC_Y_GFX10_TILING_SURFACE: u32 = 0x4024;
const JPEG_DEC_UV_GFX10_TILING_SURFACE: u32 = 0x4025;
const UVD_LMI_JPEG_WRITE_64BIT_BAR_HIGH: u32 = 0x40e3;
const UVD_LMI_JPEG_WRITE_64BIT_BAR_LOW: u32 = 0x40e2;
const UVD_JPEG_INDEX: u32 = 0x402c;
const UVD_JPEG_DATA: u32 = 0x402d;
const UVD_JPEG_TIER_CNTL2: u32 = 0x400f;
const UVD_JPEG_OUTBUF_RPTR: u32 = 0x401e;
const UVD_JPEG_OUTBUF_CNTL: u32 = 0x401c;
const UVD_JPEG_INT_EN: u32 = 0x400a;
const UVD_JPEG_CNTL: u32 = 0x4000;
const UVD_JPEG_RB_RPTR: u32 = 0x4003;
const UVD_JPEG_OUTBUF_WPTR: u32 = 0x401d;

/// `cap->bitstream_size_alignment`.
pub const BITSTREAM_ALIGN: usize = 128;

/// `RDECODE_PKTJ`.
fn pktj(reg: u32, cond: u32, ty: u32) -> u32 {
    (reg & 0x3FFFF) | ((cond & 0xF) << 24) | ((ty & 0xF) << 28)
}

/// `vcn_jpeg_build_decode_cmd` for `RDECODE_JPEG_VER_2`, one packed plane
/// (pitch in pixels, `dt_top_offset[0]` 0), linear.
pub fn build_ib(bitstream_va: u64, bitstream_size: u32, out_va: u64, pitch_px: u32) -> Vec<u32> {
    let (pitch, uv_pitch) = (pitch_px >> 4, 0);
    let mut c = Vec::with_capacity(128);
    let mut set = |reg: u32, cond: u32, ty: u32, val: u32| {
        c.push(pktj(reg, cond, ty));
        c.push(val);
    };
    let dt_addr_mode = RDECODE_TILE_LINEAR;
    let tiling = ADDR_SW_LINEAR;
    // jpeg soft reset
    set(UVD_JPEG_DEC_SOFT_RST, COND0, TYPE0, 1);
    // ensuring the Reset is asserted in SCLK domain
    set(UVD_JRBC_IB_COND_RD_TIMER, COND0, TYPE0, 0x0140_0200);
    set(UVD_JRBC_IB_REF_DATA, COND0, TYPE0, 1 << 0x10);
    set(UVD_JPEG_DEC_SOFT_RST, COND3, TYPE3, 1 << 0x10);
    // wait mem
    set(UVD_JPEG_DEC_SOFT_RST, COND0, TYPE0, 0);
    // ensuring the Reset is de-asserted in SCLK domain
    set(UVD_JRBC_IB_REF_DATA, COND0, TYPE0, 0);
    set(UVD_JPEG_DEC_SOFT_RST, COND3, TYPE3, 1 << 0x10);
    set(UVD_LMI_JPEG_READ_64BIT_BAR_HIGH, COND0, TYPE0, (bitstream_va >> 32) as u32);
    set(UVD_LMI_JPEG_READ_64BIT_BAR_LOW, COND0, TYPE0, bitstream_va as u32);
    set(UVD_JPEG_RB_BASE, COND0, TYPE0, 0);
    set(UVD_JPEG_RB_SIZE, COND0, TYPE0, 0xffff_fff0);
    set(UVD_JPEG_RB_WPTR, COND0, TYPE0, bitstream_size >> 2);
    set(UVD_JPEG_PITCH, COND0, TYPE0, pitch);
    set(UVD_JPEG_UV_PITCH, COND0, TYPE0, uv_pitch);
    set(JPEG_DEC_ADDR_MODE, COND0, TYPE0, dt_addr_mode | (dt_addr_mode << 2));
    set(JPEG_DEC_Y_GFX10_TILING_SURFACE, COND0, TYPE0, tiling);
    set(JPEG_DEC_UV_GFX10_TILING_SURFACE, COND0, TYPE0, tiling);
    set(UVD_LMI_JPEG_WRITE_64BIT_BAR_HIGH, COND0, TYPE0, (out_va >> 32) as u32);
    set(UVD_LMI_JPEG_WRITE_64BIT_BAR_LOW, COND0, TYPE0, out_va as u32);
    // output buffer data address: the plane offsets
    for i in 0..3 {
        set(UVD_JPEG_INDEX, COND0, TYPE0, i);
        set(UVD_JPEG_DATA, COND0, TYPE0, 0);
    }
    set(UVD_JPEG_TIER_CNTL2, COND0, 0, 0);
    set(UVD_JPEG_OUTBUF_RPTR, COND0, TYPE0, 0);
    set(UVD_JPEG_OUTBUF_CNTL, COND0, TYPE0, (0x0000_1587 & !0x0000_0180) | (1 << 7) | (1 << 6));
    // enable error interrupts
    set(UVD_JPEG_INT_EN, COND0, TYPE0, 0xffff_fffe);
    // start engine command
    set(UVD_JPEG_CNTL, COND0, TYPE0, 0x6);
    // wait for job completion, wait for job JBSI fetch done
    set(UVD_JRBC_IB_REF_DATA, COND0, TYPE0, bitstream_size >> 2);
    set(UVD_JRBC_IB_COND_RD_TIMER, COND0, TYPE0, 0x0140_0200);
    set(UVD_JPEG_RB_RPTR, COND3, TYPE3, 0xffff_ffff);
    // wait for job jpeg outbuf idle
    set(UVD_JRBC_IB_REF_DATA, COND0, TYPE0, 0xffff_ffff);
    set(UVD_JPEG_OUTBUF_WPTR, COND3, TYPE3, 0x0000_0001);
    // stop engine
    set(UVD_JPEG_CNTL, COND0, TYPE0, 0x4);
    c
}

// Annex K.3, the tables MJPEG frames leave out (DHT segments as written).
const DHT_DEFAULT: [u8; 420] = {
    const DC_L: [u8; 16] = [0, 1, 5, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0];
    const DC_C: [u8; 16] = [0, 3, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0];
    const DCV: [u8; 12] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
    const AC_L: [u8; 16] = [0, 2, 1, 3, 3, 2, 4, 3, 5, 5, 4, 4, 0, 0, 1, 0x7d];
    const AC_C: [u8; 16] = [0, 2, 1, 2, 4, 4, 3, 4, 7, 5, 4, 4, 0, 1, 2, 0x77];
    const ACV_L: [u8; 162] = [
        0x01, 0x02, 0x03, 0x00, 0x04, 0x11, 0x05, 0x12, 0x21, 0x31, 0x41, 0x06, 0x13, 0x51, 0x61, 0x07, 0x22,
        0x71, 0x14, 0x32, 0x81, 0x91, 0xa1, 0x08, 0x23, 0x42, 0xb1, 0xc1, 0x15, 0x52, 0xd1, 0xf0, 0x24, 0x33,
        0x62, 0x72, 0x82, 0x09, 0x0a, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2a, 0x34,
        0x35, 0x36, 0x37, 0x38, 0x39, 0x3a, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4a, 0x53, 0x54, 0x55,
        0x56, 0x57, 0x58, 0x59, 0x5a, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6a, 0x73, 0x74, 0x75, 0x76,
        0x77, 0x78, 0x79, 0x7a, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8a, 0x92, 0x93, 0x94, 0x95, 0x96,
        0x97, 0x98, 0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4, 0xb5,
        0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3, 0xc4, 0xc5, 0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2, 0xd3, 0xd4,
        0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda, 0xe1, 0xe2, 0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea, 0xf1,
        0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8, 0xf9, 0xfa,
    ];
    const ACV_C: [u8; 162] = [
        0x00, 0x01, 0x02, 0x03, 0x11, 0x04, 0x05, 0x21, 0x31, 0x06, 0x12, 0x41, 0x51, 0x07, 0x61, 0x71, 0x13,
        0x22, 0x32, 0x81, 0x08, 0x14, 0x42, 0x91, 0xa1, 0xb1, 0xc1, 0x09, 0x23, 0x33, 0x52, 0xf0, 0x15, 0x62,
        0x72, 0xd1, 0x0a, 0x16, 0x24, 0x34, 0xe1, 0x25, 0xf1, 0x17, 0x18, 0x19, 0x1a, 0x26, 0x27, 0x28, 0x29,
        0x2a, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3a, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4a, 0x53, 0x54,
        0x55, 0x56, 0x57, 0x58, 0x59, 0x5a, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6a, 0x73, 0x74, 0x75,
        0x76, 0x77, 0x78, 0x79, 0x7a, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8a, 0x92, 0x93, 0x94,
        0x95, 0x96, 0x97, 0x98, 0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xb2, 0xb3,
        0xb4, 0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3, 0xc4, 0xc5, 0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2,
        0xd3, 0xd4, 0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda, 0xe2, 0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea,
        0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8, 0xf9, 0xfa,
    ];
    // FF C4, length 418, then class/id + 16 counts + values, four times.
    let mut o = [0u8; 420];
    o[0] = 0xFF;
    o[1] = 0xC4;
    o[2] = (418 >> 8) as u8;
    o[3] = (418 & 0xFF) as u8;
    let mut p = 4;
    p = put_table(&mut o, p, 0x00, &DC_L, &DCV);
    p = put_table(&mut o, p, 0x01, &DC_C, &DCV);
    p = put_table(&mut o, p, 0x10, &AC_L, &ACV_L);
    let _ = put_table(&mut o, p, 0x11, &AC_C, &ACV_C);
    o
};

/// One table of a DHT segment at `p`: class and id, the 16 counts, the
/// values. Where the next one goes.
const fn put_table(o: &mut [u8; 420], mut p: usize, tc: u8, bits: &[u8; 16], vals: &[u8]) -> usize {
    o[p] = tc;
    p += 1;
    let mut i = 0;
    while i < 16 { o[p + i] = bits[i]; i += 1; }
    p += 16;
    let mut i = 0;
    while i < vals.len() { o[p + i] = vals[i]; i += 1; }
    p + vals.len()
}

/// What `normalize` read from the frame header.
pub struct header {
    pub width: u32,
    pub height: u32,
}

/// The bitstream the engine parses, as `vlVaGetJpegSliceHeader` writes
/// it: SOI, DQT, DHT (the Annex K tables when the frame has none), DRI,
/// SOF0, SOS and the entropy-coded data to EOI; APPn and COM left out;
/// zero-padded to `BITSTREAM_ALIGN`. Only baseline 4:2:2 in whole MCUs is
/// accepted, the layout the YUYV output has.
pub fn normalize(src: &[u8], out: &mut Vec<u8>) -> Result<header, i32> {
    const EINVAL: i32 = 22;
    out.clear();
    if src.len() < 4 || src[0] != 0xFF || src[1] != 0xD8 { return Err(-EINVAL); }
    out.extend_from_slice(&[0xFF, 0xD8]);
    let mut pos = 2;
    let mut dht = false;
    let mut hdr = None;
    loop {
        while src.get(pos) == Some(&0xFF) && src.get(pos + 1) == Some(&0xFF) { pos += 1; }
        if src.get(pos) != Some(&0xFF) { return Err(-EINVAL); }
        let m = *src.get(pos + 1).ok_or(-EINVAL)?;
        if m == 0xD8 || m == 0x01 { pos += 2; continue; }
        if m == 0xD9 { return Err(-EINVAL); }
        let len = ((*src.get(pos + 2).ok_or(-EINVAL)? as usize) << 8) | *src.get(pos + 3).ok_or(-EINVAL)? as usize;
        if len < 2 || pos + 2 + len > src.len() { return Err(-EINVAL); }
        let seg = &src[pos..pos + 2 + len];
        match m {
            0xDB | 0xDD => out.extend_from_slice(seg),
            0xC4 => {
                dht = true;
                out.extend_from_slice(seg);
            }
            // One frame header: the size checked is the size decoded.
            0xC0 | 0xC1 if hdr.is_some() => return Err(-EINVAL),
            0xC0 | 0xC1 => {
                let s = &seg[4..];
                if s.len() < 15 || s[0] != 8 || s[5] != 3 { return Err(-EINVAL); }
                let height = ((s[1] as u32) << 8) | s[2] as u32;
                let width = ((s[3] as u32) << 8) | s[4] as u32;
                // 4:2:2 in either spelling (Y 2x1 and chroma 1x1, or Y 2x2
                // and chroma 1x2): chroma half as wide, as tall; whole MCUs.
                let (y, cb, cr) = (s[7], s[10], s[13]);
                let (yh, yv) = ((y >> 4) as u32, (y & 0xF) as u32);
                if cb != cr || yh != 2 * (cb >> 4) as u32 || yv != (cb & 0xF) as u32 || !(1..=2).contains(&yv) {
                    return Err(-EINVAL);
                }
                if width == 0 || height == 0 || width % (8 * yh) != 0 || height % (8 * yv) != 0 { return Err(-EINVAL); }
                // SOF0 for the engine: a SOF1 baseline-Huffman frame is the same.
                out.extend_from_slice(&[0xFF, 0xC0]);
                out.extend_from_slice(&seg[2..]);
                hdr = Some(header { width, height });
            }
            0xDA => {
                if hdr.is_none() { return Err(-EINVAL); }
                if !dht { out.extend_from_slice(&DHT_DEFAULT); }
                // SOS and everything up to and with EOI. A frame without
                // EOI was cut short: the engine would wait for the rest.
                let rest = &src[pos..];
                let end = rest.windows(2).rposition(|w| w == [0xFF, 0xD9]).ok_or(-EINVAL)? + 2;
                if end < 2 + len + 4 { return Err(-EINVAL); }
                out.extend_from_slice(&rest[..end]);
                break;
            }
            0xC2 | 0xC3 | 0xC5..=0xCF => return Err(-EINVAL),
            _ => {}
        }
        pos += 2 + len;
    }
    out.resize(out.len().next_multiple_of(BITSTREAM_ALIGN), 0);
    hdr.ok_or(-EINVAL)
}
