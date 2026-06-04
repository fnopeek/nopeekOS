//! Smart Battery System (SBS) client over the i801 SMBus.
//!
//! Standardised registers from Linux `drivers/power/supply/sbs-battery.c`:
//! a smart battery answers at SMBus address 0x0B, exposing relative state
//! of charge (0x0D, 0..100 %) and a status word (0x16) whose bits classify
//! charging vs discharging. No ACPI/AML needed — but it only works if the
//! pack sits directly on the SMBus (some laptops hide it behind the EC, in
//! which case [`read`] simply returns None and the bar segment stays empty).

use crate::smbus;
use core::sync::atomic::{AtomicI32, Ordering};

// Battery state reported by the AML driver (aml.wasm), encoded as the bar
// expects: (status << 8) | percent, or -1 for "no battery / no report yet".
// The driver runs the firmware's _BST/_BIF (vendor-independent) and pushes
// here; `npk_battery()` returns this. Replaces the old per-device EC offset
// hardcode (which only worked on one HP model).
static REPORT: AtomicI32 = AtomicI32::new(-1);

/// Called by the AML driver via `npk_battery_report`.
pub fn report(packed: i32) {
    REPORT.store(packed, Ordering::Release);
}

/// The latest driver report (or -1). `npk_battery()` returns this, falling
/// back to the standardised SBS-over-SMBus path for desktops/SBS laptops.
pub fn cached() -> i32 {
    REPORT.load(Ordering::Acquire)
}

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
    /// On AC but not actively charging (e.g. HP Adaptive Battery Care holds
    /// the charge at ~85 %). Shown with a plug icon, not the charging bolt.
    PluggedIdle = 3,
}

#[derive(Clone, Copy)]
pub struct BatteryState {
    pub percent: u8,
    pub status: ChargeStatus,
}

/// Read the current battery state via the standardised SBS-over-SMBus path
/// (works when the pack sits directly on the bus). Laptops that hide the pack
/// behind the EC report through the AML driver instead — see [`cached`]. The
/// former per-device EC offset hardcode was removed in favour of aml.wasm,
/// which runs the firmware's own `_BST`/`_BIF` and is vendor-independent.
pub fn read() -> Option<BatteryState> {
    read_sbs()
}

/// Smart Battery System over the i801 SMBus (works when the pack is wired
/// directly to the bus). None when it NAKs.
fn read_sbs() -> Option<BatteryState> {
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
