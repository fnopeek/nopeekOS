//! display/dc/dce/dce_scl_filters.c (Linux 6.18.55): the 64-phase scaler
//! filter coefficients and their choice by tap count and scaling ratio.
//! The tables themselves are Linux' own, read off GCC's data (tables.rs).
#![allow(dead_code)]

use crate::fixpt31_32::{dc_fixpt_from_fraction, dc_fixpt_one, fixed31_32};
use crate::tables::*;

/// The choice of every `get_filter_Ntap_64p`: upscale, then the 1.16,
/// 1.49 and 1.83 downscale tables.
fn pick(ratio: fixed31_32, upscale: &'static [u16], f116: &'static [u16], f149: &'static [u16],
        f183: &'static [u16]) -> &'static [u16] {
    if ratio.value < dc_fixpt_one.value {
        upscale
    } else if ratio.value < dc_fixpt_from_fraction(4, 3).value {
        f116
    } else if ratio.value < dc_fixpt_from_fraction(5, 3).value {
        f149
    } else {
        f183
    }
}

/// `get_filter_3tap_64p`.
pub fn get_filter_3tap_64p(ratio: fixed31_32) -> &'static [u16] {
    pick(ratio, &filter_3tap_64p_upscale, &filter_3tap_64p_116, &filter_3tap_64p_149, &filter_3tap_64p_183)
}

/// `get_filter_4tap_64p`.
pub fn get_filter_4tap_64p(ratio: fixed31_32) -> &'static [u16] {
    pick(ratio, &filter_4tap_64p_upscale, &filter_4tap_64p_116, &filter_4tap_64p_149, &filter_4tap_64p_183)
}

/// `get_filter_5tap_64p`.
pub fn get_filter_5tap_64p(ratio: fixed31_32) -> &'static [u16] {
    pick(ratio, &filter_5tap_64p_upscale, &filter_5tap_64p_116, &filter_5tap_64p_149, &filter_5tap_64p_183)
}

/// `get_filter_6tap_64p`.
pub fn get_filter_6tap_64p(ratio: fixed31_32) -> &'static [u16] {
    pick(ratio, &filter_6tap_64p_upscale, &filter_6tap_64p_116, &filter_6tap_64p_149, &filter_6tap_64p_183)
}

/// `get_filter_7tap_64p`.
pub fn get_filter_7tap_64p(ratio: fixed31_32) -> &'static [u16] {
    pick(ratio, &filter_7tap_64p_upscale, &filter_7tap_64p_116, &filter_7tap_64p_149, &filter_7tap_64p_183)
}

/// `get_filter_8tap_64p`.
pub fn get_filter_8tap_64p(ratio: fixed31_32) -> &'static [u16] {
    pick(ratio, &filter_8tap_64p_upscale, &filter_8tap_64p_116, &filter_8tap_64p_149, &filter_8tap_64p_183)
}

/// `get_filter_2tap_64p`.
pub fn get_filter_2tap_64p() -> &'static [u16] {
    &filter_2tap_64p
}
