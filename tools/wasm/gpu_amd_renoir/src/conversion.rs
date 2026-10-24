//! display/dc/basics/conversion.c + custom_float.c (Linux 6.18.55): fixed
//! point to the register formats of the colour blocks.
#![allow(dead_code)]

use crate::fixpt31_32::*;
use crate::regs::{CONVERSION_DIVIDER, S2D13_MAX, S2D13_MIN};

/// `fixed_point_to_int_frac` (conversion.c).
pub fn fixed_point_to_int_frac(arg: fixed31_32, integer_bits: u8, fractional_bits: u8) -> u16 {
    let divisor: i32 = 1 << fractional_bits;

    let d = dc_fixpt_floor(dc_fixpt_abs(arg)) as u16;

    let numerator: i32 = if d <= ((1u32 << integer_bits) as u16).wrapping_sub(1 / (divisor as u16)) {
        dc_fixpt_round(dc_fixpt_mul_int(arg, divisor)) as u16 as i32
    } else {
        dc_fixpt_floor(dc_fixpt_sub(dc_fixpt_from_int(1 << integer_bits),
            dc_fixpt_recip(dc_fixpt_from_int(divisor))))
    };

    let mut result: u16 = if numerator >= 0 {
        numerator as u16
    } else {
        ((1i32 << (integer_bits + fractional_bits + 1)) + numerator) as u16
    };

    if result != 0 && dc_fixpt_lt(arg, dc_fixpt_zero) {
        result |= 1 << (integer_bits + fractional_bits);
    }

    result
}

/// `convert_float_matrix` (conversion.c): S2.13, clamped.
pub fn convert_float_matrix(matrix: &mut [u16], flt: &[fixed31_32], buffer_size: u32) {
    let min_2_13 = dc_fixpt_from_fraction(S2D13_MIN as i64, CONVERSION_DIVIDER as i64);
    let max_2_13 = dc_fixpt_from_fraction(S2D13_MAX as i64, CONVERSION_DIVIDER as i64);

    for i in 0..buffer_size as usize {
        let reg_value = fixed_point_to_int_frac(dc_fixpt_clamp(flt[i], min_2_13, max_2_13), 2, 13);

        matrix[i] = reg_value;
    }
}

/// `struct custom_float_format` (custom_float.h).
#[derive(Clone, Copy, Default, Debug)]
pub struct custom_float_format {
    pub mantissa_bits: u32,
    pub exponenta_bits: u32,
    pub sign: bool,
}

/// `build_custom_float` (custom_float.c).
fn build_custom_float(mut value: fixed31_32, format: &custom_float_format, negative: &mut bool, mantissa: &mut u32,
                      exponenta: &mut u32) -> bool {
    let exp_offset: u32 = (1 << (format.exponenta_bits - 1)) - 1;

    let mantissa_constant_plus_max_fraction =
        dc_fixpt_from_fraction((1i64 << (format.mantissa_bits + 1)) - 1, 1i64 << format.mantissa_bits);

    if dc_fixpt_eq(value, dc_fixpt_zero) {
        *negative = false;
        *mantissa = 0;
        *exponenta = 0;
        return true;
    }

    if dc_fixpt_lt(value, dc_fixpt_zero) {
        *negative = format.sign;
        value = dc_fixpt_neg(value);
    } else {
        *negative = false;
    }

    if dc_fixpt_lt(value, dc_fixpt_one) {
        let mut i: u32 = 1;

        loop {
            value = dc_fixpt_shl(value, 1);
            i += 1;
            if !dc_fixpt_lt(value, dc_fixpt_one) {
                break;
            }
        }

        i -= 1;

        if exp_offset <= i {
            *mantissa = 0;
            *exponenta = 0;
            return true;
        }

        *exponenta = exp_offset - i;
    } else if dc_fixpt_le(mantissa_constant_plus_max_fraction, value) {
        let mut i: u32 = 1;

        loop {
            value = dc_fixpt_shr(value, 1);
            i += 1;
            if !dc_fixpt_lt(mantissa_constant_plus_max_fraction, value) {
                break;
            }
        }

        *exponenta = exp_offset + i - 1;
    } else {
        *exponenta = exp_offset;
    }

    let mut mantiss = dc_fixpt_sub(value, dc_fixpt_one);

    if dc_fixpt_lt(mantiss, dc_fixpt_zero) || dc_fixpt_lt(dc_fixpt_one, mantiss) {
        mantiss = dc_fixpt_zero;
    } else {
        mantiss = dc_fixpt_shl(mantiss, format.mantissa_bits as u8);
    }

    *mantissa = dc_fixpt_floor(mantiss) as u32;

    true
}

/// `setup_custom_float` (custom_float.c).
fn setup_custom_float(format: &custom_float_format, negative: bool, mut mantissa: u32, mut exponenta: u32,
                      result: &mut u32) -> bool {
    let mut i = 0;
    let mut j = 0;
    let mut value: u32 = 0;

    let mantissa_mask: u32 = (1 << (format.mantissa_bits + 1)) - 1;

    let exponenta_mask: u32 = (1 << (format.exponenta_bits + 1)) - 1;

    if mantissa & !mantissa_mask != 0 {
        mantissa = mantissa_mask;
    }

    if exponenta & !exponenta_mask != 0 {
        exponenta = exponenta_mask;
    }

    while i < format.mantissa_bits {
        let mask = 1 << i;

        if mantissa & mask != 0 {
            value |= mask;
        }

        i += 1;
    }

    while j < format.exponenta_bits {
        let mask = 1 << j;

        if exponenta & mask != 0 {
            value |= mask << i;
        }

        j += 1;
    }

    if negative && format.sign {
        value |= 1 << (i + j);
    }

    *result = value;

    true
}

/// `convert_to_custom_float_format` (custom_float.c).
pub fn convert_to_custom_float_format(value: fixed31_32, format: &custom_float_format, result: &mut u32) -> bool {
    let mut mantissa = 0;
    let mut exponenta = 0;
    let mut negative = false;

    build_custom_float(value, format, &mut negative, &mut mantissa, &mut exponenta)
        && setup_custom_float(format, negative, mantissa, exponenta, result)
}
