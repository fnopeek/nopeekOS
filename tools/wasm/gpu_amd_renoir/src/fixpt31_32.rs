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
