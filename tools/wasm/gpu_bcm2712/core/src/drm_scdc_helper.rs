//! `drivers/gpu/drm/display/drm_scdc_helper.c` (Linux rpi-6.18): the HDMI
//! 2.0 Status and Control Data Channel on the DDC bus.

use alloc::vec;

use crate::hw::Hw;
use crate::i2c_brcmstb::{Ddc, I2cMsg, I2C_M_RD};

/// `include/drm/display/drm_scdc.h`.
pub const SCDC_I2C_SLAVE_ADDRESS: u16 = 0x54;
pub const SCDC_TMDS_CONFIG: u8 = 0x20;
pub const SCDC_TMDS_BIT_CLOCK_RATIO_BY_40: u8 = 1 << 1;
pub const SCDC_SCRAMBLING_ENABLE: u8 = 1 << 0;

const EPROTO: i32 = 71;

/// `drm_scdc_read`.
pub fn drm_scdc_read(hw: &mut impl Hw, ddc: &mut Ddc, offset: u8, buffer: &mut [u8]) -> Result<(), i32> {
    let mut off = [offset];
    let mut msgs = [
        I2cMsg { addr: SCDC_I2C_SLAVE_ADDRESS, flags: 0, buf: &mut off },
        I2cMsg { addr: SCDC_I2C_SLAVE_ADDRESS, flags: I2C_M_RD, buf: buffer },
    ];
    match ddc.transfer(hw, &mut msgs)? {
        2 => Ok(()),
        _ => Err(-EPROTO),
    }
}

/// `drm_scdc_write`.
pub fn drm_scdc_write(hw: &mut impl Hw, ddc: &mut Ddc, offset: u8, buffer: &[u8]) -> Result<(), i32> {
    let mut data = vec![offset];
    data.extend_from_slice(buffer);
    let mut msgs = [I2cMsg { addr: SCDC_I2C_SLAVE_ADDRESS, flags: 0, buf: &mut data }];
    match ddc.transfer(hw, &mut msgs)? {
        1 => Ok(()),
        _ => Err(-EPROTO),
    }
}

/// `drm_scdc_readb`.
pub fn drm_scdc_readb(hw: &mut impl Hw, ddc: &mut Ddc, offset: u8) -> Result<u8, i32> {
    let mut v = [0u8];
    drm_scdc_read(hw, ddc, offset, &mut v)?;
    Ok(v[0])
}

/// `drm_scdc_writeb`.
pub fn drm_scdc_writeb(hw: &mut impl Hw, ddc: &mut Ddc, offset: u8, value: u8) -> Result<(), i32> {
    drm_scdc_write(hw, ddc, offset, &[value])
}

/// `drm_scdc_set_scrambling`.
pub fn drm_scdc_set_scrambling(hw: &mut impl Hw, ddc: &mut Ddc, enable: bool) -> bool {
    let Ok(mut config) = drm_scdc_readb(hw, ddc, SCDC_TMDS_CONFIG) else { return false };
    if enable {
        config |= SCDC_SCRAMBLING_ENABLE;
    } else {
        config &= !SCDC_SCRAMBLING_ENABLE;
    }
    drm_scdc_writeb(hw, ddc, SCDC_TMDS_CONFIG, config).is_ok()
}

/// `drm_scdc_set_high_tmds_clock_ratio`: a source waits at least 1 ms
/// after writing the clock ratio.
pub fn drm_scdc_set_high_tmds_clock_ratio(hw: &mut impl Hw, ddc: &mut Ddc, set: bool) -> bool {
    let Ok(mut config) = drm_scdc_readb(hw, ddc, SCDC_TMDS_CONFIG) else { return false };
    if set {
        config |= SCDC_TMDS_BIT_CLOCK_RATIO_BY_40;
    } else {
        config &= !SCDC_TMDS_BIT_CLOCK_RATIO_BY_40;
    }
    if drm_scdc_writeb(hw, ddc, SCDC_TMDS_CONFIG, config).is_err() {
        return false;
    }
    hw.delay_us(1000);
    true
}
