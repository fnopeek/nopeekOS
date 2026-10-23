//! display/dc/hwss/dcn21/dcn21_hwseq.c (Linux 6.18.55): the DCN 2.1 steps of
//! the hw sequencer, as far as the hardware init reaches them.
#![allow(dead_code)]

use crate::amdgpu_device::amdgpu_device;
use crate::dc::dc;
use crate::dc_helper::reg_read;

/// `dcn21_s0i3_golden_init_wa` (dcn21_hwseq.c): true, which skips the VBIOS
/// golden init, unless MICROSECOND_TIME_BASE_DIV reads 0x120464.
pub fn dcn21_s0i3_golden_init_wa(adev: &amdgpu_device, dc: &dc) -> bool {
    let hws = dc.hwseq.as_deref().expect("hwseq");
    let value = reg_read!(adev, hws.rsm().0, MICROSECOND_TIME_BASE_DIV);
    value != 0x00120464
}
