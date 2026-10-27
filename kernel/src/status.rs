//! Status items: what the panel shows on its right side.
//!
//! A module publishes plain data under its own name (the kernel sets the
//! owner); the panel reads the list and draws it, and a click opens the tool
//! the item names. The kernel's own items (network, volume, battery) are not
//! stored: they are built from the live state each time the list is read,
//! so there is one source for each value.

use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

use crate::shade::widgets::abi::IconId;
use crate::smp::fiber::Waker;

pub const STATE_NORMAL: u8 = 0;
pub const STATE_ATTENTION: u8 = 1;
pub const STATE_OFF: u8 = 2;

pub const PLACE_BAR: u8 = 0;
pub const PLACE_QUICK: u8 = 1;

pub const MAX_KEY: usize = 16;
pub const MAX_TEXT: usize = 48;
const PER_OWNER: usize = 4;
const MAX_ITEMS: usize = 32;

pub struct Item {
    pub owner: String,
    pub key:   String,
    pub icon:  u16,
    pub value: i32,
    pub state: u8,
    pub place: u8,
    pub text:  String,
    pub opens: String,
    /// The publishing fiber; its items go when it ends.
    waker: Waker,
}

static ITEMS: Mutex<Vec<Item>> = Mutex::new(Vec::new());

fn changed() { crate::notify::notify(crate::notify::TOPIC_STATUS); }

/// Create or replace `owner`'s item `key`. False when the owner or the
/// table is full.
pub fn set(owner: &str, waker: Waker, key: &str, icon: u16, value: i32, state: u8, place: u8,
           text: &str, opens: &str) -> bool {
    let mut items = ITEMS.lock();
    let item = Item {
        owner: owner.into(), key: key.into(), icon, value,
        state: state.min(STATE_OFF), place: place.min(PLACE_QUICK),
        text: text.into(), opens: opens.into(), waker,
    };
    if let Some(i) = items.iter_mut().find(|i| i.owner == owner && i.key == key) {
        *i = item;
    } else {
        if items.len() >= MAX_ITEMS || items.iter().filter(|i| i.owner == owner).count() >= PER_OWNER {
            return false;
        }
        items.push(item);
        items.sort_by(|a, b| (&a.owner, &a.key).cmp(&(&b.owner, &b.key)));
    }
    drop(items);
    changed();
    true
}

pub fn clear(owner: &str, key: &str) {
    let mut items = ITEMS.lock();
    let n = items.len();
    items.retain(|i| !(i.owner == owner && i.key == key));
    let gone = items.len() != n;
    drop(items);
    if gone { changed(); }
}

/// The fiber `w` ended: its items go with it.
pub fn forget(w: Waker) {
    let Some(mut items) = ITEMS.try_lock() else { return };
    let n = items.len();
    items.retain(|i| i.waker != w);
    let gone = items.len() != n;
    drop(items);
    if gone { changed(); }
}

/// The whole list in bar order, serialized for `npk_status_list`:
/// `count: u16`, then per item `owner, key: u8-length strings, icon: u16,
/// value: i32, state: u8, place: u8, text, opens: u8-length strings`.
pub fn serialize() -> Vec<u8> {
    let mut out = Vec::new();
    let items = ITEMS.lock();
    // One network icon, for the link the traffic takes: a linked wired
    // primary wins, else the WLAN manager's item (signal, network name),
    // else the kernel's own view of the WLAN.
    let wired_up = crate::netdev::list().iter().any(|i| i.name != "wlan" && i.primary && i.link_up);
    let wlan_item = items.iter().find(|i| is_wlan_item(i)).filter(|_| !wired_up);
    let own = kernel_items(wlan_item.is_some());
    let (net, rest_own) = own.split_at(own.iter().take_while(|i| i.key == "net").count());
    let ordered: Vec<&Item> = net.iter()
        .chain(wlan_item)
        .chain(rest_own.iter())
        .chain(items.iter().filter(|i| !is_wlan_item(i)))
        .collect();
    out.extend_from_slice(&(ordered.len() as u16).to_le_bytes());
    for i in ordered {
        put_str(&mut out, &i.owner);
        put_str(&mut out, &i.key);
        out.extend_from_slice(&i.icon.to_le_bytes());
        out.extend_from_slice(&i.value.to_le_bytes());
        out.push(i.state);
        out.push(i.place);
        put_str(&mut out, &i.text);
        put_str(&mut out, &i.opens);
    }
    out
}

fn put_str(out: &mut Vec<u8>, s: &str) {
    let b = &s.as_bytes()[..s.len().min(255)];
    out.push(b.len() as u8);
    out.extend_from_slice(b);
}

fn kernel_item(key: &str, icon: IconId, value: i32, state: u8, text: String, opens: &str) -> Item {
    Item {
        owner: "kernel".into(), key: key.into(), icon: icon.0, value, state, place: PLACE_BAR,
        text, opens: opens.into(), waker: crate::smp::fiber::NO_WAKER,
    }
}

/// The WLAN manager's item; it stands for the WLAN in place of the kernel's.
fn is_wlan_item(i: &Item) -> bool {
    i.owner == "wifid" && i.key == "wlan"
}

/// Network, volume, battery — left to right before the clock.
fn kernel_items(wlan_shown: bool) -> Vec<Item> {
    let mut v = Vec::new();
    if let Some(net) = net_item(wlan_shown) { v.push(net); }

    let vol = crate::audio::get_volume();
    let icon = if vol == 0 { IconId::SpeakerX } else if vol <= 50 { IconId::SpeakerLow } else { IconId::SpeakerHigh };
    v.push(kernel_item("volume", icon, vol as i32, STATE_NORMAL, alloc::format!("{}%", vol), "volume"));

    if let Some(b) = battery_packed() {
        let percent = (b & 0xFF) as u8;
        let status = (b >> 8) & 0xFF; // 0 discharging, 1 charging, 2 full, 3 plugged idle
        let icon = match status {
            1 => IconId::BatteryCharging,
            2 => IconId::BatteryFull,
            3 => IconId::Plug,
            _ => match percent {
                0..=10  => IconId::BatteryWarning,
                11..=30 => IconId::BatteryLow,
                31..=55 => IconId::BatteryMedium,
                56..=85 => IconId::BatteryHigh,
                _       => IconId::BatteryFull,
            },
        };
        let state = if status == 0 && percent <= 10 { STATE_ATTENTION } else { STATE_NORMAL };
        v.push(kernel_item("battery", icon, percent as i32, state, alloc::format!("{}%", percent), ""));
    }
    v
}

/// The driver's report, or a smart battery on the SMBus; `None` without one.
pub fn battery_packed() -> Option<i32> {
    let cached = crate::battery::cached();
    if cached >= 0 { return Some(cached); }
    crate::battery::read().map(|b| ((b.status as i32) << 8) | b.percent as i32)
}

/// The interface the traffic takes, unless the WLAN manager's item stands
/// for it (`wlan_shown`). Nothing without an interface. A click opens `link`.
fn net_item(wlan_shown: bool) -> Option<Item> {
    if wlan_shown {
        return None;
    }
    let ifaces = crate::netdev::list();
    let shown = ifaces.iter().find(|i| i.primary).or(ifaces.first())?;
    let wlan = shown.name == "wlan";
    let (icon, state) = match (wlan, shown.link_up) {
        (true, true) => (IconId::WifiHigh, STATE_NORMAL),
        (true, false) => (IconId::WifiSlash, STATE_OFF),
        (false, true) => (IconId::Network, STATE_NORMAL),
        (false, false) => (IconId::Network, STATE_OFF),
    };
    Some(kernel_item("net", icon, -1, state, String::new(), "link"))
}
