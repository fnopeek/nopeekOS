//! System Configuration
//!
//! Key-value config stored as ".npk-config" in npkFS (encrypted at rest).
//! Loaded into memory after unlock, persisted on every change.
//!
//! Format: "key=value\n" lines, UTF-8.

use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

const MAX_ENTRIES: usize = 32;
const CONFIG_OBJECT: &str = ".npk-config";

struct ConfigEntry {
    key: String,
    value: String,
}

struct ConfigStore {
    entries: Vec<ConfigEntry>,
}

impl ConfigStore {
    const fn new() -> Self {
        ConfigStore { entries: Vec::new() }
    }

    fn get(&self, key: &str) -> Option<&str> {
        self.entries.iter()
            .find(|e| e.key == key)
            .map(|e| e.value.as_str())
    }

    fn set(&mut self, key: &str, value: &str) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.key == key) {
            entry.value = String::from(value);
        } else if self.entries.len() < MAX_ENTRIES {
            self.entries.push(ConfigEntry {
                key: String::from(key),
                value: String::from(value),
            });
        }
    }

    #[allow(dead_code)]
    fn remove(&mut self, key: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|e| e.key != key);
        self.entries.len() < before
    }

    fn serialize(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for e in &self.entries {
            out.extend_from_slice(e.key.as_bytes());
            out.push(b'=');
            out.extend_from_slice(e.value.as_bytes());
            out.push(b'\n');
        }
        out
    }

    fn deserialize(&mut self, data: &[u8]) {
        self.entries.clear();
        if let Ok(text) = core::str::from_utf8(data) {
            for line in text.lines() {
                if let Some((k, v)) = line.split_once('=') {
                    let k = k.trim();
                    let v = v.trim();
                    if !k.is_empty() && self.entries.len() < MAX_ENTRIES {
                        self.entries.push(ConfigEntry {
                            key: String::from(k),
                            value: String::from(v),
                        });
                    }
                }
            }
        }
    }

    fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.entries.iter().map(|e| (e.key.as_str(), e.value.as_str()))
    }
}

static CONFIG: Mutex<ConfigStore> = Mutex::new(ConfigStore::new());

/// Load config from npkFS. Call after unlock.
pub fn load() {
    match crate::npkfs::fetch(CONFIG_OBJECT) {
        Ok((data, _)) => {
            CONFIG.lock().deserialize(&data);
        }
        Err(_) => {
            // No config yet — fresh system
        }
    }
}

/// Persist config to npkFS.
fn save() {
    let data = CONFIG.lock().serialize();
    let _ = crate::npkfs::upsert(CONFIG_OBJECT, &data, crate::capability::CAP_NULL);
}

/// Get a config value.
pub fn get(key: &str) -> Option<String> {
    CONFIG.lock().get(key).map(String::from)
}

/// Set a config value and persist.
pub fn set(key: &str, value: &str) {
    CONFIG.lock().set(key, value);
    save();
}

/// Remove a config value and persist.
#[allow(dead_code)]
pub fn unset(key: &str) -> bool {
    let removed = CONFIG.lock().remove(key);
    if removed { save(); }
    removed
}

/// Get timezone offset in minutes (e.g. +120 for UTC+2).
pub fn timezone_offset_minutes() -> i32 {
    let lock = CONFIG.lock();
    match lock.get("timezone") {
        Some(val) => parse_timezone_offset(val),
        None => 0,
    }
}

/// Parse timezone string: "+2", "-5", "+5:30", "+05:45"
fn parse_timezone_offset(s: &str) -> i32 {
    let s = s.trim();
    if s.is_empty() { return 0; }

    let (sign, rest) = if s.starts_with('-') {
        (-1i32, &s[1..])
    } else if s.starts_with('+') {
        (1i32, &s[1..])
    } else {
        (1i32, s)
    };

    if let Some((h, m)) = rest.split_once(':') {
        let hours: i32 = h.trim().parse().unwrap_or(0);
        let mins: i32 = m.trim().parse().unwrap_or(0);
        sign * (hours * 60 + mins)
    } else {
        let hours: i32 = rest.parse().unwrap_or(0);
        sign * hours * 60
    }
}

/// List all config entries.
pub fn list() -> Vec<(String, String)> {
    CONFIG.lock().iter()
        .map(|(k, v)| (String::from(k), String::from(v)))
        .collect()
}

/// Known config keys with descriptions.
pub const KNOWN_KEYS: &[(&str, &str)] = &[
    ("timezone", "UTC offset (e.g. +2, -5, +5:30)"),
    ("keyboard", "Keyboard layout (e.g. de_CH, us, de_DE)"),
    ("lang", "Display language (e.g. de, en, fr)"),
];
