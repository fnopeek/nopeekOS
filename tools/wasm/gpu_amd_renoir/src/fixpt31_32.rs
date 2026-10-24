//! display/include/fixed31_32.h + dc/basics/fixpt31_32.c (Linux 6.18.55):
//! the signed 31.32 fixed-point type of DC. Functions join as callers reach
//! them.
#![allow(dead_code)]

const FIXED31_32_BITS_PER_FRACTIONAL_PART: u32 = 32;
const FRACTIONAL_PART_MASK: u64 = (1u64 << FIXED31_32_BITS_PER_FRACTIONAL_PART) - 1;

/// `struct fixed31_32`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct fixed31_32 {
    pub value: i64,
}

/// `dc_fixpt_half`.
pub const dc_fixpt_half: fixed31_32 = fixed31_32 { value: 0x8000_0000 };

fn get_integer_part(x: u64) -> u64 {
    x >> FIXED31_32_BITS_PER_FRACTIONAL_PART
}

fn get_fractional_part(x: u64) -> u64 {
    FRACTIONAL_PART_MASK & x
}

/// `dc_fixpt_from_int`.
pub fn dc_fixpt_from_int(arg: i32) -> fixed31_32 {
    fixed31_32 { value: (arg as i64) << FIXED31_32_BITS_PER_FRACTIONAL_PART }
}

/// `dc_fixpt_mul`. The overflow ASSERTs of Linux are checks the C code only
/// logs; the arithmetic wraps as the u64/i64 mix does there.
pub fn dc_fixpt_mul(arg1: fixed31_32, arg2: fixed31_32) -> fixed31_32 {
    let arg1_negative = arg1.value < 0;
    let arg2_negative = arg2.value < 0;

    let arg1_value = if arg1_negative { arg1.value.wrapping_neg() } else { arg1.value } as u64;
    let arg2_value = if arg2_negative { arg2.value.wrapping_neg() } else { arg2.value } as u64;

    let arg1_int = get_integer_part(arg1_value);
    let arg2_int = get_integer_part(arg2_value);

    let arg1_fra = get_fractional_part(arg1_value);
    let arg2_fra = get_fractional_part(arg2_value);

    let mut res = arg1_int.wrapping_mul(arg2_int) as i64;

    res <<= FIXED31_32_BITS_PER_FRACTIONAL_PART;

    res = res.wrapping_add(arg1_int.wrapping_mul(arg2_fra) as i64);

    res = res.wrapping_add(arg2_int.wrapping_mul(arg1_fra) as i64);

    let mut tmp = arg1_fra.wrapping_mul(arg2_fra);

    tmp = (tmp >> FIXED31_32_BITS_PER_FRACTIONAL_PART) + (tmp >= dc_fixpt_half.value as u64) as u64;

    res = res.wrapping_add(tmp as i64);

    if arg1_negative ^ arg2_negative {
        res = res.wrapping_neg();
    }

    fixed31_32 { value: res }
}

/// `dc_fixpt_mul_int`.
pub fn dc_fixpt_mul_int(arg1: fixed31_32, arg2: i32) -> fixed31_32 {
    dc_fixpt_mul(arg1, dc_fixpt_from_int(arg2))
}

/// `dc_fixpt_floor`.
pub fn dc_fixpt_floor(arg: fixed31_32) -> i32 {
    let arg_value = if arg.value > 0 { arg.value } else { arg.value.wrapping_neg() } as u64;

    if arg.value >= 0 {
        (arg_value >> FIXED31_32_BITS_PER_FRACTIONAL_PART) as i32
    } else {
        -((arg_value >> FIXED31_32_BITS_PER_FRACTIONAL_PART) as i32)
    }
}

/// `dc_fixpt_one` / `dc_fixpt_zero` / `dc_fixpt_epsilon`.
pub const dc_fixpt_one: fixed31_32 = fixed31_32 { value: 0x1_0000_0000 };
pub const dc_fixpt_zero: fixed31_32 = fixed31_32 { value: 0 };
pub const dc_fixpt_epsilon: fixed31_32 = fixed31_32 { value: 1 };

/// `dc_fixpt_from_fraction` (fixpt31_32.c): the integer part by division,
/// then 32 fraction bits by restoring division, the last one rounded.
pub fn dc_fixpt_from_fraction(numerator: i64, denominator: i64) -> fixed31_32 {
    let arg1_negative = numerator < 0;
    let arg2_negative = denominator < 0;

    let arg1_value = if arg1_negative { numerator.wrapping_neg() } else { numerator } as u64;
    let arg2_value = if arg2_negative { denominator.wrapping_neg() } else { denominator } as u64;

    // determine integer part
    let mut res_value = arg1_value / arg2_value;
    let mut remainder = arg1_value % arg2_value;

    // determine fractional part
    for _ in 0..FIXED31_32_BITS_PER_FRACTIONAL_PART {
        remainder <<= 1;

        res_value <<= 1;

        if remainder >= arg2_value {
            res_value |= 1;
            remainder -= arg2_value;
        }
    }

    // round up LSB
    let summand = ((remainder << 1) >= arg2_value) as u64;

    res_value = res_value.wrapping_add(summand);

    let mut res = fixed31_32 { value: res_value as i64 };

    if arg1_negative ^ arg2_negative {
        res.value = res.value.wrapping_neg();
    }

    res
}

/// `dc_fixpt_round`.
pub fn dc_fixpt_round(arg: fixed31_32) -> i32 {
    let mut arg_value = if arg.value > 0 { arg.value } else { arg.value.wrapping_neg() } as u64;

    arg_value = arg_value.wrapping_add(dc_fixpt_half.value as u64);

    if arg.value >= 0 {
        (arg_value >> FIXED31_32_BITS_PER_FRACTIONAL_PART) as i32
    } else {
        -((arg_value >> FIXED31_32_BITS_PER_FRACTIONAL_PART) as i32)
    }
}

/// `dc_fixpt_ceil`.
pub fn dc_fixpt_ceil(arg: fixed31_32) -> i32 {
    let mut arg_value = if arg.value > 0 { arg.value } else { arg.value.wrapping_neg() } as u64;

    arg_value = arg_value.wrapping_add((dc_fixpt_one.value - dc_fixpt_epsilon.value) as u64);

    if arg.value >= 0 {
        (arg_value >> FIXED31_32_BITS_PER_FRACTIONAL_PART) as i32
    } else {
        -((arg_value >> FIXED31_32_BITS_PER_FRACTIONAL_PART) as i32)
    }
}

/// `dc_fixpt_truncate`: keep `frac_bits` of the fraction.
pub fn dc_fixpt_truncate(mut arg: fixed31_32, frac_bits: u32) -> fixed31_32 {
    let negative = arg.value < 0;

    if frac_bits >= FIXED31_32_BITS_PER_FRACTIONAL_PART {
        return arg;
    }

    if negative {
        arg.value = arg.value.wrapping_neg();
    }
    arg.value &= ((!0u64) << (FIXED31_32_BITS_PER_FRACTIONAL_PART - frac_bits)) as i64;
    if negative {
        arg.value = arg.value.wrapping_neg();
    }
    arg
}

/// `dc_fixpt_add` / `dc_fixpt_sub` / `dc_fixpt_add_int`.
pub fn dc_fixpt_add(arg1: fixed31_32, arg2: fixed31_32) -> fixed31_32 {
    fixed31_32 { value: arg1.value.wrapping_add(arg2.value) }
}

pub fn dc_fixpt_sub(arg1: fixed31_32, arg2: fixed31_32) -> fixed31_32 {
    fixed31_32 { value: arg1.value.wrapping_sub(arg2.value) }
}

pub fn dc_fixpt_add_int(arg1: fixed31_32, arg2: i32) -> fixed31_32 {
    dc_fixpt_add(arg1, dc_fixpt_from_int(arg2))
}

/// `dc_fixpt_div_int` / `dc_fixpt_div`.
pub fn dc_fixpt_div_int(arg1: fixed31_32, arg2: i64) -> fixed31_32 {
    dc_fixpt_from_fraction(arg1.value, dc_fixpt_from_int(arg2 as i32).value)
}

pub fn dc_fixpt_div(arg1: fixed31_32, arg2: fixed31_32) -> fixed31_32 {
    dc_fixpt_from_fraction(arg1.value, arg2.value)
}

/// `ux_dy`: `integer_bits`.`fractional_bits` unsigned.
fn ux_dy(value: i64, integer_bits: u32, fractional_bits: u32) -> u32 {
    // 1. create mask of integer part
    let mut result: u32 = (1u32 << integer_bits).wrapping_sub(1);
    // 2. mask out fractional part
    let mut fractional_part = (FRACTIONAL_PART_MASK & value as u64) as u32;
    // 3. shrink fixed point integer part to be of integer_bits width
    result &= get_integer_part(value as u64) as u32;
    // 4. make space for fractional part to be filled in after integer
    result <<= fractional_bits;
    // 5. shrink fixed point fractional part to of fractional_bits width
    fractional_part >>= FIXED31_32_BITS_PER_FRACTIONAL_PART - fractional_bits;
    // 6. merge the result
    result | fractional_part
}

/// `dc_fixpt_u2d19` / `dc_fixpt_u0d19`.
pub fn dc_fixpt_u2d19(arg: fixed31_32) -> u32 {
    ux_dy(arg.value, 2, 19)
}

pub fn dc_fixpt_u0d19(arg: fixed31_32) -> u32 {
    ux_dy(arg.value, 0, 19)
}

/// `dc_fixpt_u3d19`.
pub fn dc_fixpt_u3d19(arg: fixed31_32) -> u32 {
    ux_dy(arg.value, 3, 19)
}

/// `dc_fixpt_neg`.
pub fn dc_fixpt_neg(arg: fixed31_32) -> fixed31_32 {
    fixed31_32 { value: -arg.value }
}

/// `dc_fixpt_abs`.
pub fn dc_fixpt_abs(arg: fixed31_32) -> fixed31_32 {
    if arg.value < 0 { dc_fixpt_neg(arg) } else { arg }
}

/// `dc_fixpt_lt`.
pub fn dc_fixpt_lt(arg1: fixed31_32, arg2: fixed31_32) -> bool {
    arg1.value < arg2.value
}

/// `dc_fixpt_le`.
pub fn dc_fixpt_le(arg1: fixed31_32, arg2: fixed31_32) -> bool {
    arg1.value <= arg2.value
}

/// `dc_fixpt_eq`.
pub fn dc_fixpt_eq(arg1: fixed31_32, arg2: fixed31_32) -> bool {
    arg1.value == arg2.value
}

/// `dc_fixpt_clamp`.
pub fn dc_fixpt_clamp(arg: fixed31_32, min_value: fixed31_32, max_value: fixed31_32) -> fixed31_32 {
    if dc_fixpt_le(arg, min_value) {
        min_value
    } else if dc_fixpt_le(max_value, arg) {
        max_value
    } else {
        arg
    }
}

/// `dc_fixpt_shl`.
pub fn dc_fixpt_shl(mut arg: fixed31_32, shift: u8) -> fixed31_32 {
    arg.value <<= shift;
    arg
}

/// `dc_fixpt_shr`.
pub fn dc_fixpt_shr(mut arg: fixed31_32, shift: u8) -> fixed31_32 {
    let negative = arg.value < 0;

    if negative {
        arg.value = -arg.value;
    }
    arg.value >>= shift;
    if negative {
        arg.value = -arg.value;
    }
    arg
}

/// `dc_fixpt_recip`.
pub fn dc_fixpt_recip(arg: fixed31_32) -> fixed31_32 {
    dc_fixpt_from_fraction(dc_fixpt_one.value, arg.value)
}
