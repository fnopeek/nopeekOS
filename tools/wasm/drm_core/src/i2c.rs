//! include/uapi/linux/i2c.h + include/linux/i2c.h: an I2C message and the
//! adapter's transfer, as DDC reads reach them. The adapter is the
//! caller's: `i2c_transfer` is a closure with `__i2c_transfer`'s contract.

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

/// `i2c_transfer(adapter, msgs, num)`: the number of messages
/// transferred, or a negative errno.
pub type i2c_transfer_fn<'a> = dyn FnMut(&mut [i2c_msg]) -> i32 + 'a;
