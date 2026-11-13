//! `drivers/gpu/drm/drm_edid.c` (Linux rpi-6.18): reading the EDID over
//! DDC.

use crate::hw::Hw;
use crate::i2c_brcmstb::{Ddc, I2cMsg, I2C_M_RD};
use crate::linux::ENXIO;

pub const EDID_LENGTH: usize = 128;
/// `include/drm/drm_edid.h`.
pub const DDC_SEGMENT_ADDR: u16 = 0x30;
pub const DDC_ADDR: u16 = 0x50;

/// `drm_do_probe_ddc_edid`: `buf.len()` bytes of EDID block `block`, up
/// to five tries.
pub fn drm_do_probe_ddc_edid(hw: &mut impl Hw, ddc: &mut Ddc, buf: &mut [u8], block: u32) -> Result<(), ()> {
    let mut start = [(block as usize * EDID_LENGTH) as u8];
    let mut segment = [(block >> 1) as u8];
    let xfers = if segment[0] != 0 { 3 } else { 2 };
    let mut retries = 5;
    loop {
        let mut msgs = [
            I2cMsg { addr: DDC_SEGMENT_ADDR, flags: 0, buf: &mut segment },
            I2cMsg { addr: DDC_ADDR, flags: 0, buf: &mut start },
            I2cMsg { addr: DDC_ADDR, flags: I2C_M_RD, buf: &mut *buf },
        ];
        let ret = ddc.transfer(hw, &mut msgs[3 - xfers..]);
        if ret == Err(-ENXIO) {
            return Err(());
        }
        if ret == Ok(xfers) {
            return Ok(());
        }
        retries -= 1;
        if retries == 0 {
            return Err(());
        }
    }
}
