//! Kernel constants the ported code returns (`include/uapi/asm-generic/errno*.h`).

pub const ENXIO: i32 = 6;
pub const EIO: i32 = 5;
pub const EACCES: i32 = 13;
pub const ENOMEM: i32 = 12;
pub const ENODEV: i32 = 19;
pub const EINVAL: i32 = 22;
pub const ENOSPC: i32 = 28;
pub const ETIMEDOUT: i32 = 110;
pub const EREMOTEIO: i32 = 121;
/// `ENOTSUPP` (include/linux/errno.h), kernel-internal.
pub const ENOTSUPP: i32 = 524;

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

/// `rational_best_approximation` (lib/math/rational.c): the fraction
/// `(n, d)` closest to `given_numerator / given_denominator` with
/// `n <= max_numerator` and `d <= max_denominator`, by continued fractions.
pub fn rational_best_approximation(given_numerator: u64, given_denominator: u64, max_numerator: u64,
    max_denominator: u64) -> (u64, u64)
{
    // n/d is the starting rational, which is continually decreased each
    // iteration using the Euclidean algorithm. dp is the value of d from
    // the prior iteration. n2/d2, n1/d1, and n0/d0 are our successively
    // more accurate approximations of the rational. a is the current term
    // of the continued fraction.
    let (mut n, mut d) = (given_numerator, given_denominator);
    let (mut n0, mut d1) = (0u64, 0u64);
    let (mut n1, mut d0) = (1u64, 1u64);

    loop {
        if d == 0 {
            break;
        }
        // Find next term in continued fraction, 'a', via Euclidean
        // algorithm.
        let dp = d;
        let a = n / d;
        d = n % d;
        n = dp;

        // Calculate the current rational approximation (aka convergent),
        // n2/d2, using the term just found and the two prior
        // approximations.
        let n2 = n0.wrapping_add(a.wrapping_mul(n1));
        let d2 = d0.wrapping_add(a.wrapping_mul(d1));

        // If the current convergent exceeds the maxes, then return either
        // the previous convergent or the largest semi-convergent, the
        // final term of which is found below as 't'.
        if n2 > max_numerator || d2 > max_denominator {
            let mut t = u64::MAX;

            if d1 != 0 {
                t = max_denominator.wrapping_sub(d0) / d1;
            }
            if n1 != 0 {
                t = t.min(max_numerator.wrapping_sub(n0) / n1);
            }

            // This tests if the semi-convergent is closer than the
            // previous convergent. If d1 is zero there is no previous
            // convergent as this is the 1st iteration, so always choose
            // the semi-convergent.
            if d1 == 0 || 2u64.wrapping_mul(t) > a || (2u64.wrapping_mul(t) == a && d0.wrapping_mul(dp) > d1.wrapping_mul(d)) {
                n1 = n0.wrapping_add(t.wrapping_mul(n1));
                d1 = d0.wrapping_add(t.wrapping_mul(d1));
            }
            break;
        }
        n0 = n1;
        n1 = n2;
        d0 = d1;
        d1 = d2;
    }
    (n1, d1)
}
