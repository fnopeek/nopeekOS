//! Driver self-reports.
//!
//! A bound WASM driver publishes a short plain-text status snapshot here
//! (`npk_driver_report`); an intent prints it back. The kernel stores bytes and
//! a timestamp and never parses the content — what a driver considers worth
//! reporting stays the driver's business, so this works for any device class
//! without vendor knowledge in the kernel.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use spin::Mutex;

/// Largest snapshot a driver may publish.
pub const REPORT_MAX: usize = 4096;

/// How many drivers can hold a slot at once.
const SLOTS: usize = 4;

struct Slot {
    name: String,
    text: String,
    at_ms: u64,
}

static SLOTS_TABLE: Mutex<Vec<Slot>> = Mutex::new(Vec::new());

/// Replace `name`'s snapshot. Oldest slot is evicted when the table is full.
pub fn store(name: &str, text: &str) {
    let at_ms = crate::interrupts::ticks().saturating_mul(10);
    let mut table = SLOTS_TABLE.lock();
    if let Some(slot) = table.iter_mut().find(|s| s.name == name) {
        slot.text.clear();
        slot.text.push_str(text);
        slot.at_ms = at_ms;
        return;
    }
    if table.len() >= SLOTS {
        let oldest = table
            .iter()
            .enumerate()
            .min_by_key(|(_, s)| s.at_ms)
            .map(|(i, _)| i)
            .unwrap_or(0);
        table.remove(oldest);
    }
    table.push(Slot { name: name.to_string(), text: text.to_string(), at_ms });
}

/// The snapshot published by `name`, with the tick-milliseconds it was stored.
pub fn get(name: &str) -> Option<(String, u64)> {
    let table = SLOTS_TABLE.lock();
    table.iter().find(|s| s.name == name).map(|s| (s.text.clone(), s.at_ms))
}

/// Every driver currently holding a slot, newest first.
pub fn names() -> Vec<String> {
    let table = SLOTS_TABLE.lock();
    let mut v: Vec<(String, u64)> =
        table.iter().map(|s| (s.name.clone(), s.at_ms)).collect();
    v.sort_by(|a, b| b.1.cmp(&a.1));
    v.into_iter().map(|(n, _)| n).collect()
}

/// Drop a driver's slot — called when its module exits, so a dead driver's
/// numbers can't be read as live ones.
pub fn clear(name: &str) {
    SLOTS_TABLE.lock().retain(|s| s.name != name);
}
