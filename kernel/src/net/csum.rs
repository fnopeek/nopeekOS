//! Internet checksum (RFC 1071), summed eight bytes at a time like Linux
//! `do_csum`: big-endian 32-bit words added into a 64-bit accumulator are
//! congruent to the 16-bit word sum modulo 0xFFFF.

/// Unfolded ones-complement sum of `data` as big-endian 16-bit words, an
/// odd last byte padded with zero.
pub fn sum(data: &[u8]) -> u64 {
    let mut acc: u64 = 0;
    let mut chunks = data.chunks_exact(8);
    for c in &mut chunks {
        acc += u32::from_be_bytes([c[0], c[1], c[2], c[3]]) as u64;
        acc += u32::from_be_bytes([c[4], c[5], c[6], c[7]]) as u64;
    }
    let rest = chunks.remainder();
    let mut i = 0;
    while i + 1 < rest.len() {
        acc += u16::from_be_bytes([rest[i], rest[i + 1]]) as u64;
        i += 2;
    }
    if i < rest.len() {
        acc += (rest[i] as u64) << 8;
    }
    acc
}

/// IPv4 pseudo-header sum (source, destination, protocol, L4 length).
pub fn pseudo(src: [u8; 4], dst: [u8; 4], proto: u8, len: usize) -> u64 {
    u32::from_be_bytes(src) as u64 + u32::from_be_bytes(dst) as u64
        + proto as u64 + len as u64
}

/// Fold a sum to 16 bits, not complemented (`csum_fold` without the `~`).
pub fn fold(mut s: u64) -> u16 {
    while s >> 16 != 0 {
        s = (s & 0xFFFF) + (s >> 16);
    }
    s as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reference(data: &[u8]) -> u16 {
        let mut s: u32 = 0;
        let mut i = 0;
        while i + 1 < data.len() {
            s += u16::from_be_bytes([data[i], data[i + 1]]) as u32;
            i += 2;
        }
        if i < data.len() { s += (data[i] as u32) << 8; }
        while s >> 16 != 0 { s = (s & 0xFFFF) + (s >> 16); }
        s as u16
    }

    #[test]
    fn matches_16_bit_sum() {
        let mut data = [0u8; 1500];
        for (i, b) in data.iter_mut().enumerate() { *b = (i as u32 * 131 + 7) as u8; }
        for len in [0, 1, 2, 7, 8, 9, 15, 16, 17, 63, 1499, 1500] {
            assert_eq!(fold(sum(&data[..len])), reference(&data[..len]), "len {len}");
        }
        let ff = [0xFFu8; 1500];
        assert_eq!(fold(sum(&ff)), reference(&ff));
    }
}
