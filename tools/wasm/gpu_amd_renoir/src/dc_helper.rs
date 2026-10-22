//! display/dc/dc_helper.c + dc/dm_services.h + dc/inc/reg_helper.h (Linux
//! 6.18.55): the register access layer every DC IP block uses.
//!
//! `dm_read_reg` / `dm_write_reg` go to amdgpu's MMIO (`adev.rreg32` /
//! `adev.wreg32`); `adev` stands in for Linux' `ctx->driver_context`. Linux'
//! variadic REG_SET / REG_UPDATE / REG_GET field macros expand to
//! `generic_reg_*_ex(ctx, addr, n, shift, mask, value, …)` with a va_list of
//! (shift, mask, value) triples; a port has no va_list, so the triples are an
//! explicit slice here. The emitted register address/value is identical, which
//! is what the oracle compares.
#![allow(dead_code)]

use crate::amdgpu_device::amdgpu_device;
use crate::linux::{msleep, udelay, Line};

/// `dm_read_reg_func` (amdgpu_dm.c): a DC register read through amdgpu MMIO.
pub fn dm_read_reg(adev: &amdgpu_device, addr: u32) -> u32 {
    adev.rreg32(addr).unwrap_or(0)
}

/// `dm_write_reg_func` (amdgpu_dm.c).
pub fn dm_write_reg(adev: &amdgpu_device, addr: u32, value: u32) {
    adev.wreg32(addr, value);
}

/// `set_reg_field_value_ex` (dm_services.h).
#[inline]
pub fn set_reg_field_value_ex(reg_value: u32, value: u32, mask: u32, shift: u8) -> u32 {
    // ASSERT(mask != 0)
    (reg_value & !mask) | (mask & (value << shift))
}

/// `get_reg_field_value_ex` (dm_services.h).
#[inline]
pub fn get_reg_field_value_ex(reg_value: u32, mask: u32, shift: u8) -> u32 {
    (mask & reg_value) >> shift
}

/// `struct dc_reg_value_masks` (dc_helper.c): the fields' combined value + the
/// combined mask of the bits touched.
#[derive(Default, Clone, Copy)]
pub struct dc_reg_value_masks {
    pub value: u32,
    pub mask: u32,
}

/// `set_reg_field_value_masks` (dc_helper.c): fold one field in.
#[inline]
fn set_reg_field_value_masks(fvm: &mut dc_reg_value_masks, value: u32, mask: u32, shift: u8) {
    // ASSERT(mask != 0)
    fvm.value = (fvm.value & !mask) | (mask & (value << shift));
    fvm.mask |= mask;
}

/// `set_reg_field_values` (dc_helper.c): gather every field's value/mask.
/// `fields` are the (shift, mask, value) triples Linux passes as a va_list.
fn set_reg_field_values(fvm: &mut dc_reg_value_masks, fields: &[(u8, u32, u32)]) {
    for &(shift, mask, value) in fields {
        set_reg_field_value_masks(fvm, value, mask, shift);
    }
}

// The DMUB reg-offload gather path (`ctx->dmub_srv->reg_helper_offload
// .gather_in_progress` → `dmub_reg_value_*_pack`) is the reg_sequence
// batching used by some hwss sequences; it is a later stage. Until then every
// write is a direct MMIO write, which is Linux' behaviour with
// gather_in_progress == false.

/// `generic_reg_set_ex` (dc_helper.c): apply the fields onto `reg_val`, write.
pub fn generic_reg_set_ex(adev: &amdgpu_device, addr: u32, mut reg_val: u32,
                          fields: &[(u8, u32, u32)]) -> u32 {
    let mut fvm = dc_reg_value_masks::default();
    set_reg_field_values(&mut fvm, fields);
    reg_val = (reg_val & !fvm.mask) | fvm.value;
    dm_write_reg(adev, addr, reg_val);
    reg_val
}

/// `generic_reg_update_ex` (dc_helper.c): read-modify-write the fields.
pub fn generic_reg_update_ex(adev: &amdgpu_device, addr: u32,
                             fields: &[(u8, u32, u32)]) -> u32 {
    let mut fvm = dc_reg_value_masks::default();
    set_reg_field_values(&mut fvm, fields);
    let mut reg_val = dm_read_reg(adev, addr);
    reg_val = (reg_val & !fvm.mask) | fvm.value;
    dm_write_reg(adev, addr, reg_val);
    reg_val
}

/// `generic_reg_get` (dc_helper.c): read `addr`, extract one field.
/// Returns (whole register value, field value), as Linux returns the register
/// value and writes the field through the out-pointer.
pub fn generic_reg_get(adev: &amdgpu_device, addr: u32, shift: u8, mask: u32) -> (u32, u32) {
    let reg_val = dm_read_reg(adev, addr);
    (reg_val, get_reg_field_value_ex(reg_val, mask, shift))
}

/// REG_GET_2 … REG_GET_N: one read, several fields into `out` (in order).
pub fn generic_reg_get_n(adev: &amdgpu_device, addr: u32,
                         fields: &[(u8, u32)], out: &mut [u32]) -> u32 {
    let reg_val = dm_read_reg(adev, addr);
    for (i, &(shift, mask)) in fields.iter().enumerate() {
        if let Some(o) = out.get_mut(i) {
            *o = get_reg_field_value_ex(reg_val, mask, shift);
        }
    }
    reg_val
}

/// `generic_reg_wait` (dc_helper.c): poll `addr` until the field equals
/// `condition_value`, or time out after `time_out_num_tries` polls.
#[allow(clippy::too_many_arguments)]
pub fn generic_reg_wait(adev: &amdgpu_device, addr: u32, shift: u8, mask: u32,
                        condition_value: u32, delay_between_poll_us: u32,
                        time_out_num_tries: u32, func_name: &str) {
    // ASSERT(delay_between_poll_us * time_out_num_tries <= 3_000_000)
    for i in 0..=time_out_num_tries {
        if i != 0 {
            if delay_between_poll_us >= 1000 {
                msleep((delay_between_poll_us / 1000) as u64);
            } else if delay_between_poll_us > 0 {
                udelay(delay_between_poll_us as u64);
            }
        }
        let reg_val = dm_read_reg(adev, addr);
        if get_reg_field_value_ex(reg_val, mask, shift) == condition_value {
            return;
        }
    }
    // BREAK_TO_DEBUGGER(): a timeout is a bug in the sequence, not a retry.
    Line::new().s("REG_WAIT timeout in ").s(func_name).say();
}
