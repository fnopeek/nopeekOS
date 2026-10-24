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

/// `dm_get_timestamp` (amdgpu_dm_services.c): ktime in ns.
pub fn dm_get_timestamp() -> u64 {
    (npk_sys::now_us() as u64).wrapping_mul(1000)
}

/// `dm_get_elapse_time_in_ns` (amdgpu_dm_services.c).
pub fn dm_get_elapse_time_in_ns(current_time_stamp: u64, last_time_stamp: u64) -> u64 {
    current_time_stamp.wrapping_sub(last_time_stamp)
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

/// `generic_read_indirect_reg` (dc_helper.c): select `index`, read the
/// data register.
pub fn generic_read_indirect_reg(adev: &amdgpu_device, addr_index: u32, addr_data: u32, index: u32) -> u32 {
    dm_write_reg(adev, addr_index, index);
    dm_read_reg(adev, addr_data)
}

/// `generic_indirect_reg_get` (dc_helper.c), one field.
pub fn generic_indirect_reg_get(adev: &amdgpu_device, addr_index: u32, addr_data: u32, index: u32, shift: u8,
                                mask: u32) -> u32 {
    let value = generic_read_indirect_reg(adev, addr_index, addr_data, index);
    get_reg_field_value_ex(value, mask, shift)
}

/// `struct color_matrices_reg` (dcn10_cm_common.h).
#[derive(Clone, Copy, Default)]
pub struct color_matrices_reg {
    /// `shifts.csc_c11`, `shifts.csc_c12`.
    pub shift_csc_c11: u8,
    pub shift_csc_c12: u8,
    /// `masks.csc_c11`, `masks.csc_c12`.
    pub mask_csc_c11: u32,
    pub mask_csc_c12: u32,
    pub csc_c11_c12: u32,
    pub csc_c33_c34: u32,
}

/// `cm_helper_program_color_matrices` (dcn10_cm_common.c): two
/// coefficients per register, from `csc_c11_c12` to `csc_c33_c34`.
pub fn cm_helper_program_color_matrices(adev: &amdgpu_device, regval: &[u16], reg: &color_matrices_reg) {
    let mut i = 0;
    let mut cur_csc_reg = reg.csc_c11_c12;
    while cur_csc_reg <= reg.csc_c33_c34 {
        let regval0 = regval[2 * i];
        let regval1 = regval[2 * i + 1];

        generic_reg_set_ex(adev, cur_csc_reg, 0, &[
            (reg.shift_csc_c11, reg.mask_csc_c11, regval0 as u32),
            (reg.shift_csc_c12, reg.mask_csc_c12, regval1 as u32),
        ]);

        i += 1;
        cur_csc_reg += 1;
    }
}

/// `REG_READ` (reg_helper.h) on a block's register table: `reg_read!(adev,
/// regs, REG)` or `reg_read!(adev, regs, REG[i])`.
macro_rules! reg_read {
    ($adev:expr, $r:expr, $reg:ident $([$i:expr])?) => {
        $crate::dc_helper::dm_read_reg($adev, $r.$reg $([$i])?)
    };
}

/// `REG_WRITE` (reg_helper.h).
macro_rules! reg_write {
    ($adev:expr, $r:expr, $reg:ident $([$i:expr])?, $v:expr) => {
        $crate::dc_helper::dm_write_reg($adev, $r.$reg $([$i])?, ($v) as u32)
    };
}

/// `REG_GET` (reg_helper.h): one field of a register. `$c` is the block's
/// `(regs, shift, mask)` triple. Evaluates to the field value.
macro_rules! reg_get {
    ($adev:expr, $c:expr, $reg:ident $([$i:expr])?, $f:ident) => {{
        let (r, s, m) = $c;
        $crate::dc_helper::generic_reg_get($adev, r.$reg $([$i])?, s.$f, m.$f).1
    }};
}

/// `REG_GET_2` … `REG_GET_N` (reg_helper.h): one read, the fields in order.
/// Evaluates to an array of the field values.
macro_rules! reg_get_n {
    ($adev:expr, $c:expr, $reg:ident $([$i:expr])?, $($f:ident),+) => {{
        let (r, s, m) = $c;
        let mut out = [0u32; [$(stringify!($f)),+].len()];
        $crate::dc_helper::generic_reg_get_n($adev, r.$reg $([$i])?, &[$((s.$f, m.$f)),+], &mut out);
        out
    }};
}

/// `REG_SET` / `REG_SET_N` (reg_helper.h): the fields on top of `init`,
/// written without a read.
macro_rules! reg_set {
    ($adev:expr, $c:expr, $reg:ident $([$i:expr])?, $init:expr, $($f:ident, $v:expr),+) => {{
        let (r, s, m) = $c;
        $crate::dc_helper::generic_reg_set_ex($adev, r.$reg $([$i])?, ($init) as u32,
            &[$((s.$f, m.$f, ($v) as u32)),+])
    }};
}

/// `REG_UPDATE` / `REG_UPDATE_N` (reg_helper.h): read-modify-write.
macro_rules! reg_update {
    ($adev:expr, $c:expr, $reg:ident $([$i:expr])?, $($f:ident, $v:expr),+) => {{
        let (r, s, m) = $c;
        $crate::dc_helper::generic_reg_update_ex($adev, r.$reg $([$i])?, &[$((s.$f, m.$f, ($v) as u32)),+])
    }};
}

/// `REG_WAIT` (reg_helper.h): poll a field until it reads `cond`.
macro_rules! reg_wait {
    ($adev:expr, $c:expr, $reg:ident $([$i:expr])?, $f:ident, $cond:expr, $delay:expr, $tries:expr,
     $func:expr) => {{
        let (r, s, m) = $c;
        $crate::dc_helper::generic_reg_wait($adev, r.$reg $([$i])?, s.$f, m.$f, ($cond) as u32, $delay, $tries,
            $func)
    }};
}

/// `IX_REG_GET` (reg_helper.h): one field of an indirect register.
macro_rules! ix_reg_get {
    ($adev:expr, $c:expr, $index_reg:ident, $data_reg:ident, $index:expr, $f:ident) => {{
        let (r, s, m) = $c;
        $crate::dc_helper::generic_indirect_reg_get($adev, r.$index_reg, r.$data_reg, ($index) as u32, s.$f, m.$f)
    }};
}

pub(crate) use {ix_reg_get, reg_get, reg_get_n, reg_read, reg_set, reg_update, reg_wait, reg_write};
