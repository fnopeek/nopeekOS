//! i2c_hid_core — HID over I2C on a Synopsys DesignWare bus.
//!
//! no_std + alloc; the same library builds for the std harness and for the
//! wasm32 module. Every part follows the Linux driver; deviations are
//! commented where they occur.
//!
//! Modules in the order they run:
//!
//! 1. [`discover`] — what the firmware (DSDT) says: controller, slave
//!    address, descriptor register, GPIO pin.
//! 2. [`dw_i2c`] — the Synopsys DesignWare bus the device sits on.
//! 3. [`hid`] — HID over I2C: descriptor, power, reset, input reports.
//! 4. [`report`] — the report descriptor: what each byte of a report means.
//! 5. [`gesture`] — contact points become pointer motion and gestures.
//! 6. [`gpio`] — the pin that says whether anything is pending.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod discover;
pub mod dw_i2c;
pub mod hid;
pub mod gesture;
pub mod gpio;
pub mod report;
