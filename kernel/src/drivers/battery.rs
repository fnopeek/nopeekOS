//! Smart Battery System (SBS) client over the i801 SMBus.
//!
//! Standardised registers from Linux `drivers/power/supply/sbs-battery.c`:
//! a smart battery answers at SMBus address 0x0B, exposing relative state
//! of charge (0x0D, 0..100 %) and a status word (0x16) whose bits classify
//! charging vs discharging. No ACPI/AML needed — but it only works if the
//! pack sits directly on the SMBus (some laptops hide it behind the EC, in
//! which case [`read`] simply returns None and the bar segment stays empty).

use crate::smbus;

const SBS_ADDR: u8 = 0x0B;
const REG_REL_STATE_OF_CHARGE: u8 = 0x0D;
const REG_BATTERY_STATUS: u8 = 0x16;

// BatteryStatus bits (sbs-battery.c).
const BATTERY_DISCHARGING: u16 = 0x40;
const BATTERY_FULL_CHARGED: u16 = 0x20;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ChargeStatus {
    Discharging = 0,
    Charging = 1,
    Full = 2,
}

#[derive(Clone, Copy)]
pub struct BatteryState {
    pub percent: u8,
    pub status: ChargeStatus,
}

/// Read the current battery state, or None if no smart battery responds.
pub fn read() -> Option<BatteryState> {
    let soc = smbus::read_word(SBS_ADDR, REG_REL_STATE_OF_CHARGE)?;
    let percent = soc.min(100) as u8;

    // Status read is best-effort; default to discharging if it NAKs.
    let status = match smbus::read_word(SBS_ADDR, REG_BATTERY_STATUS) {
        Some(s) if s & BATTERY_FULL_CHARGED != 0 => ChargeStatus::Full,
        Some(s) if s & BATTERY_DISCHARGING != 0 => ChargeStatus::Discharging,
        Some(_) => ChargeStatus::Charging,
        None => ChargeStatus::Discharging,
    };

    Some(BatteryState { percent, status })
}
