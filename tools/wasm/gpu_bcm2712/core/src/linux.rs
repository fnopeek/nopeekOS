//! Kernel constants the ported code returns (`include/uapi/asm-generic/errno*.h`).

pub const ENXIO: i32 = 6;
pub const EIO: i32 = 5;
pub const EACCES: i32 = 13;
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

/// `wait_for(COND, MS)` (vc4_drv.h `__wait_for`): poll `cond` with
/// growing sleeps (10 us up to 1 ms) until it holds, at most `ms`; 0 or
/// -ETIMEDOUT. The condition is checked once more after the deadline.
pub fn wait_for<H: crate::hw::Hw>(hw: &mut H, ms: u64, mut cond: impl FnMut(&mut H) -> bool) -> i32 {
    let end = hw.now_us() + ms * 1000;
    let mut wait = 10;
    loop {
        let expired = hw.now_us() > end;
        if cond(hw) {
            return 0;
        }
        if expired {
            return -ETIMEDOUT;
        }
        hw.delay_us(wait);
        if wait < 1000 {
            wait <<= 1;
        }
    }
}
