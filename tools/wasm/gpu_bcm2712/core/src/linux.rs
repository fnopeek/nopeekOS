//! Kernel constants the ported code returns (`include/uapi/asm-generic/errno*.h`).

pub const ENXIO: i32 = 6;
pub const EIO: i32 = 5;
pub const ENOMEM: i32 = 12;
pub const EINVAL: i32 = 22;
pub const ENOSPC: i32 = 28;
pub const ETIMEDOUT: i32 = 110;
pub const EREMOTEIO: i32 = 121;

/// `FIELD_PREP` (`VC4_SET_FIELD` without its `WARN_ON`).
pub const fn field_prep(mask: u32, value: u32) -> u32 {
    (value << mask.trailing_zeros()) & mask
}

/// `FIELD_GET` (`VC4_GET_FIELD`).
pub const fn field_get(mask: u32, word: u32) -> u32 {
    (word & mask) >> mask.trailing_zeros()
}
