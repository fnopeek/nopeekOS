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
    /// On AC but not actively charging (e.g. HP Adaptive Battery Care holds
    /// the charge at ~85 %). Shown with a plug icon, not the charging bolt.
    PluggedIdle = 3,
}

#[derive(Clone, Copy)]
pub struct BatteryState {
    pub percent: u8,
    pub status: ChargeStatus,
}

/// Read the current battery state, or None if no battery is found. Tries the
/// standardised SBS-over-SMBus path first; if the pack doesn't sit on the
/// SMBus (HP and most laptops hide it behind the EC), falls back to the EC
/// path below.
pub fn read() -> Option<BatteryState> {
    read_sbs().or_else(read_ec)
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

// ── EC-RAM battery (HP Elite/Dragonfly control-method-battery) ──────────
// Offsets reverse-engineered live on an HP Elite Dragonfly G1 and validated
// against its BIOS readout (remaining 40 Wh, full-charge 50.01 Wh). The pack
// is a degraded 57.8 Wh design (~88 % health). These offsets are specific to
// the HP Elite EC layout; the `is_mobile()` gate keeps desktops out, and a
// plausibility check keeps non-HP laptops from showing garbage.
const EC_REMAINING_MWH: u8 = 0x4a;   // u16 LE: remaining capacity (mWh)
const EC_CHARGE_FLAGS: u8 = 0xb7;    // bit 0x40 SET = not charging
const EC_CHARGER_WATTS: u8 = 0xf9;   // 0 on battery, charger wattage on AC
const EC_NOT_CHARGING: u8 = 0x40;
// Last-full-charge reference for the percentage. Read from the BIOS rather
// than the (noisy) EC capacity region; recalibrate if % drifts as the pack
// ages further.
const EC_FULL_CHARGE_MWH: u32 = 50_010;

/// Read the remaining-capacity word, rejecting single-read glitches: take a
/// value only when two reads agree (the EC can return a transient mid-update
/// byte). Falls back to the third read if all differ.
#[allow(dead_code)]
fn read_remaining_stable() -> Option<u32> {
    let a = crate::ec::read_u16(EC_REMAINING_MWH)?;
    let b = crate::ec::read_u16(EC_REMAINING_MWH)?;
    if a == b { return Some(a as u32); }
    let c = crate::ec::read_u16(EC_REMAINING_MWH)?;
    Some(c as u32)
}

fn read_ec() -> Option<BatteryState> {
    // Laptops only — desktops have no battery (and bogus EC RAM here).
    if !crate::acpi::is_mobile() { return None; }

    // DISABLED: 0x4a (EC_REMAINING_MWH) turned out to be a monotonic counter,
    // not remaining capacity (rose past full-charge after 30 min on battery).
    // The real remaining-capacity EC offset comes from the DSDT EmbeddedControl
    // Field (`dsdt` intent). Until that offset is known, report no battery
    // rather than a bogus %. Re-enable by pointing EC_REMAINING_MWH at the
    // real offset and removing this early return.
    return None;
    #[allow(unreachable_code)]
    let remaining = read_remaining_stable()?;
    // Sanity: a real mWh capacity, not a stray/garbage read.
    if !(1_000..=120_000).contains(&remaining) { return None; }

    let percent = ((remaining * 100 + EC_FULL_CHARGE_MWH / 2)
        / EC_FULL_CHARGE_MWH).min(100) as u8;

    let ac = crate::ec::read(EC_CHARGER_WATTS).map(|w| w != 0).unwrap_or(false);
    let charging = crate::ec::read(EC_CHARGE_FLAGS)
        .map(|f| f & EC_NOT_CHARGING == 0).unwrap_or(false);

    let status = if ac && charging {
        ChargeStatus::Charging
    } else if percent >= 99 {
        ChargeStatus::Full
    } else if ac {
        // Plugged in but not charging (HP Adaptive Battery Care holds ~85 %).
        ChargeStatus::PluggedIdle
    } else {
        ChargeStatus::Discharging
    };

    Some(BatteryState { percent, status })
}
