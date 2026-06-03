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
// EC field offsets decoded from the DSDT's Field(ECRM) (via the `dsdt`
// intent) — the same fields EC0.BTST/BTIF read: select the battery via BSEL,
// then read the multiplexed capacity/status fields. Validated live on an HP
// Elite Dragonfly G1 against the BIOS readout: BFC_=6496 mAh ≈ 50.01 Wh,
// BDC_=7300 mAh ≈ 56.2 Wh design (units are mAh, ~7.7 V pack). Full charge
// is read live (self-calibrating as the pack ages). HP-Elite-specific; the
// is_mobile() gate keeps desktops out and a plausibility check guards others.
pub const EC_BSEL: u8 = 0x86; // battery select (4-bit nibble); 0 on this single-battery laptop
const EC_BFC: u8 = 0x8d;      // u16 full-charge capacity (mAh)
const EC_BRC: u8 = 0xa1;      // u16 remaining capacity (mAh)
const EC_BST: u8 = 0x99;      // status nibble: bit0 = discharging, bit1 = charging
const EC_CHARGER_WATTS: u8 = 0xf9; // AC adapter present when nonzero
const BST_DISCHARGING: u8 = 0x01;
const BST_CHARGING: u8 = 0x02;

fn read_ec() -> Option<BatteryState> {
    // Laptops only — desktops have no battery (and bogus EC RAM here).
    if !crate::acpi::is_mobile() { return None; }

    // BSEL is already 0 (single battery) on the Dragonfly, so no EC write is
    // needed to select BAT0 — we stay read-only.
    let full = crate::ec::read_u16(EC_BFC)? as u32;
    let remaining = crate::ec::read_u16(EC_BRC)? as u32;
    // Plausibility (mAh): guards non-HP laptops whose 0x8d/0xa1 mean nothing.
    if !(500..=20_000).contains(&full) { return None; }
    if remaining > full + full / 8 { return None; }

    let percent = ((remaining * 100 + full / 2) / full).min(100) as u8;

    let bst = crate::ec::read(EC_BST).unwrap_or(0) & 0x0F;
    let ac = crate::ec::read(EC_CHARGER_WATTS).map(|w| w != 0).unwrap_or(false);

    let status = if bst & BST_CHARGING != 0 {
        ChargeStatus::Charging
    } else if bst & BST_DISCHARGING != 0 {
        ChargeStatus::Discharging
    } else if percent >= 99 {
        ChargeStatus::Full
    } else if ac {
        // Plugged in, not charging (HP Adaptive Battery Care holds ~80 %).
        ChargeStatus::PluggedIdle
    } else {
        ChargeStatus::Discharging
    };

    Some(BatteryState { percent, status })
}
