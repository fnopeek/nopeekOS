//! include/linux/i2c.h + drivers/i2c/i2c-core-base.c (Linux 6.18.55): an I2C
//! adapter and `i2c_transfer`, as far as EDID reads reach them. Linux finds
//! the adapter's owner through `algo_data`/`container_of`; the port names
//! the owner (`i2c_bus`) and the algorithm (`i2c_algo`) instead.
#![allow(dead_code)]

use crate::amdgpu_dm_helpers::{amdgpu_dm_i2c_xfer, amdgpu_i2c_adapter, dm_ctx};
use crate::drm_dp_helper::{drm_dp_aux, drm_dp_i2c_xfer};
use crate::linux::{EAGAIN, EINVAL, EOPNOTSUPP};
use alloc::string::String;

/// `I2C_M_RD` / `I2C_M_STOP` (uapi/linux/i2c.h).
pub const I2C_M_RD: u16 = 0x0001;
pub const I2C_M_STOP: u16 = 0x8000;

/// `struct i2c_msg`. `len` is the buffer's.
pub struct i2c_msg<'a> {
    pub addr: u16,
    pub flags: u16,
    pub len: u16,
    pub buf: &'a mut [u8],
}

/// Which `struct i2c_algorithm` an adapter has.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum i2c_algo {
    /// drm_dp_helper.c: I2C over the AUX channel.
    drm_dp_i2c_algo,
    /// amdgpu_dm.c: DC's I2C engines (`dc_submit_i2c`).
    amdgpu_dm_i2c_algo,
}

/// `struct i2c_adapter`, the members a transfer reads.
#[derive(Default)]
pub struct i2c_adapter {
    pub name: String,
    pub algo: Option<i2c_algo>,
    pub retries: i32,
}

/// The adapter and what owns it.
pub enum i2c_bus<'a> {
    aux(&'a mut drm_dp_aux),
    dm(&'a mut amdgpu_i2c_adapter),
}

impl i2c_bus<'_> {
    pub fn adapter(&self) -> &i2c_adapter {
        match self {
            i2c_bus::aux(a) => &a.ddc,
            i2c_bus::dm(i) => &i.base,
        }
    }
}

/// `i2c_transfer` → `__i2c_transfer` (i2c-core-base.c): the adapter's
/// `master_xfer`, again on -EAGAIN up to `adap->retries` times. The bus
/// lock guards nothing in a single-threaded module; the jiffies timeout
/// only ends -EAGAIN retries, which neither algorithm returns.
pub fn i2c_transfer(bus: &mut i2c_bus, ctx: &mut dm_ctx, msgs: &mut [i2c_msg]) -> i32 {
    let Some(algo) = bus.adapter().algo else { return -(EOPNOTSUPP as i32) };
    if msgs.is_empty() {
        return -(EINVAL as i32);
    }
    let retries = bus.adapter().retries;

    let mut ret = 0;
    for _try in 0..=retries {
        ret = match (algo, &mut *bus) {
            (i2c_algo::drm_dp_i2c_algo, i2c_bus::aux(a)) => drm_dp_i2c_xfer(a, ctx, msgs),
            (i2c_algo::amdgpu_dm_i2c_algo, i2c_bus::dm(i)) => amdgpu_dm_i2c_xfer(i, ctx, msgs),
            _ => return -(EOPNOTSUPP as i32),
        };
        if ret != -(EAGAIN as i32) {
            break;
        }
    }
    ret
}
