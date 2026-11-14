//! The bit packing of Mesa's `util/bitpack_helpers.h` and
//! `cle/v3d_packet_helpers.h`, which the generated packets
//! (`v3d_packet_gen`) call: every field ORed into a zeroed packet, little
//! endian, as gen_pack_header.py lays the bytes out.

/// `util_bitpack_uint`: `v` at bits `start..=end`.
pub fn put_uint(cl: &mut [u8], start: u32, end: u32, v: u64) {
    let width = end - start + 1;
    let v = if width >= 64 { v } else { v & ((1u64 << width) - 1) };
    let mut bit = start;
    let mut rest = v;
    while bit <= end {
        let byte = (bit / 8) as usize;
        let shift = bit % 8;
        let take = (8 - shift).min(end - bit + 1);
        let part = (rest & ((1u64 << take) - 1)) as u8;
        cl[byte] |= part << shift;
        rest >>= take;
        bit += take;
    }
}

/// `util_bitpack_sint`: the low `end - start + 1` bits of `v`.
pub fn put_sint(cl: &mut [u8], start: u32, end: u32, v: i64) {
    put_uint(cl, start, end, v as u64);
}

/// An address field: the address's bytes from the field's first byte on,
/// shifted right by `extra` bits when the field is narrower than 32 bits
/// (its low bits are zero by alignment), ORed over bytes `first..=last`.
pub fn put_address(cl: &mut [u8], first: usize, last: usize, extra: u32, addr: u32) {
    for (k, b) in cl[first..=last].iter_mut().enumerate() {
        let shift = 8 * k as u32 + extra;
        if shift < 32 {
            *b |= (addr >> shift) as u8;
        }
    }
}

/// `llroundf`: to the nearest integer, halfway cases away from zero (f64
/// holds every f32 exactly, so adding a half does not round).
fn llroundf(v: f32) -> i64 {
    let d = v as f64;
    if d >= 0.0 { (d + 0.5) as i64 } else { -((-d + 0.5) as i64) }
}

/// `util_bitpack_ufixed`'s integer: `llroundf(v * 2^frac)`.
pub fn ufixed(v: f32, frac: u32) -> u64 {
    llroundf(v * (1u32 << frac) as f32) as u64
}

/// `util_bitpack_sfixed`'s integer.
pub fn sfixed(v: f32, frac: u32) -> i64 {
    llroundf(v * (1u32 << frac) as f32)
}
