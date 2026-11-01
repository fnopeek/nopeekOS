//! CRC-32 (IEEE 802.3, reflected), as `efi_crc32` in Linux
//! block/partitions/efi.c: `crc32_le(~0, buf, len) ^ ~0`.

pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xEDB8_8320 & (crc & 1).wrapping_neg());
        }
    }
    !crc
}
