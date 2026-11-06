//! System settings: one place that validates a value, stores it and makes
//! it take effect. The loop's `set`, `theme` and `mouse` and the apps'
//! `npk_setting_get` / `npk_setting_set` all go through here, so a key has
//! one meaning. Contract: docs/spec/SETTINGS.md.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

/// Who asks. An app reaches only keys the table lets it write.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Loop,
    App,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    /// The key is not one an app may write (unknown, or loop-only).
    NotWritable,
    /// The value is not valid for the key; the text says what is.
    Invalid(String),
}

/// How a key lives: in the config, or only in the hardware it drives.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Store {
    Config,
    /// The value is the hardware state (brightness), or the apply step
    /// stores it once it worked (wallpaper).
    Apply,
}

struct Spec {
    key: &'static str,
    app_writes: bool,
    store: Store,
    /// The value to store, normalized, or why the value is refused.
    check: fn(&str) -> Result<String, String>,
}

const TABLE: &[Spec] = &[
    Spec { key: "theme", app_writes: true, store: Store::Config, check: check_theme },
    Spec { key: "accent", app_writes: true, store: Store::Config, check: check_accent },
    Spec { key: "wallpaper", app_writes: true, store: Store::Apply, check: check_wallpaper },
    Spec { key: "shade.scale", app_writes: true, store: Store::Config, check: check_scale },
    Spec { key: "code.scheme", app_writes: true, store: Store::Config, check: check_code_scheme },
    Spec { key: "brightness", app_writes: true, store: Store::Apply, check: check_brightness },
    Spec { key: "keyboard", app_writes: true, store: Store::Config, check: check_keyboard },
    Spec { key: "mouse_speed", app_writes: true, store: Store::Config, check: check_mouse_speed },
    Spec { key: "mouse_size", app_writes: true, store: Store::Config, check: check_mouse_size },
    Spec { key: "lang", app_writes: true, store: Store::Config, check: check_lang },
    Spec { key: "timezone", app_writes: true, store: Store::Config, check: check_timezone },
    // These lower a security guarantee: an app reads them, only the loop
    // changes them.
    Spec { key: "net.allow_plain_http", app_writes: false, store: Store::Config, check: check_any },
    Spec { key: "net.lan_devices", app_writes: false, store: Store::Config, check: check_any },
];

fn spec(key: &str) -> Option<&'static Spec> {
    TABLE.iter().find(|s| s.key == key)
}

/// Whether an app may read `key`.
pub fn app_reads(key: &str) -> bool {
    spec(key).is_some()
}

/// Longest key and value an app may pass.
pub const MAX_KEY: usize = 64;
pub const MAX_VALUE: usize = 256;

/// Validate, store and apply `key = value`. A key outside the table is
/// stored as given for the loop (debug and driver knobs) and refused for
/// an app. Returns the value as stored.
pub fn set(key: &str, value: &str, origin: Origin) -> Result<String, Error> {
    let key = key.trim();
    let value = value.trim();
    // The config is `key=value` lines: a control character or `=` in a key,
    // or a control character in a value, would forge another entry.
    if key.chars().any(|c| c.is_control() || c == '=') || value.chars().any(char::is_control) {
        return Err(Error::Invalid("a setting holds no control characters".into()));
    }
    let Some(s) = spec(key) else {
        if origin == Origin::App || key.is_empty() {
            return Err(Error::NotWritable);
        }
        crate::config::set(key, value);
        apply(key);
        return Ok(value.to_string());
    };
    if origin == Origin::App && !s.app_writes {
        return Err(Error::NotWritable);
    }
    let v = (s.check)(value).map_err(Error::Invalid)?;
    match s.store {
        Store::Config => {
            crate::config::set(key, &v);
            apply(key);
        }
        Store::Apply => defer(key, &v),
    }
    Ok(v)
}

/// The value an app may read: a table key's stored or live value. None
/// for a key outside the table, or one with no value yet.
pub fn get_for_app(key: &str) -> Option<String> {
    let s = spec(key)?;
    match s.key {
        "brightness" => {
            let level = crate::gpu::gpudev::backlight_cached();
            if level.is_none() && crate::gpu::gpudev::backlight_supported() {
                defer("brightness", "");
            }
            level.map(|l| (l / (crate::gpu::gpudev::BACKLIGHT_SCALE / 100)).to_string())
        }
        "mouse_speed" => Some(crate::shade::cursor::speed().to_string()),
        "mouse_size" => Some(crate::shade::cursor::size().to_string()),
        k => crate::config::get(k),
    }
}

/// Make a changed key visible at once. Also after `unset`.
pub fn apply(key: &str) {
    match key {
        "mouse_speed" | "mouse_size" => {
            let n = crate::config::get(key).and_then(|v| v.parse::<i32>().ok());
            if key == "mouse_speed" { crate::shade::cursor::set_speed(n.unwrap_or(100)); }
            else { crate::shade::cursor::set_size(n.unwrap_or(100)); }
        }
        // The shared knob is the master: it drops the panel's own override
        // so it always moves the panel.
        "shade.chrome_opacity" => {
            crate::shade::widgets::palette::clear_panel_opacity_override();
        }
        _ => {}
    }
    // Panel translucency and theme colours are baked into the cached scenes
    // at rasterize time, so they are re-rasterized before the redraw.
    if key.starts_with("shade.") || key.starts_with("code.")
        || key == "theme" || key == "accent" {
        if key == "shade.blur" { crate::gui::background::reblur(); }
        if key == "shade.scale" { crate::shade::rescale(); }
        crate::shade::widgets::refresh_all_scenes();
        crate::shade::force_redraw();
    }
}

// Keys whose apply step waits on a module (the GPU module for the
// backlight, the decoder module for a wallpaper) run as their own task, so
// the caller, an app's hostcall included, does not wait.
static DEFERRED: Mutex<Vec<(&'static str, String)>> = Mutex::new(Vec::new());
/// A task drains `DEFERRED`; at most one is queued or running.
static DRAINING: AtomicBool = AtomicBool::new(false);

fn defer(key: &str, value: &str) {
    let Some(s) = spec(key) else { return };
    {
        let mut q = DEFERRED.lock();
        // A newer value replaces a queued one: a dragged slider sends many.
        // A read never replaces a queued write.
        if value.is_empty() && q.iter().any(|(k, _)| *k == s.key) { return; }
        q.retain(|(k, _)| *k != s.key);
        q.push((s.key, value.to_string()));
    }
    if !DRAINING.swap(true, Ordering::AcqRel) {
        crate::smp::scheduler::spawn("setting", run_deferred, 0);
    }
}

fn run_deferred(_: u64) {
    loop {
        let next = {
            let mut q = DEFERRED.lock();
            if q.is_empty() {
                // Cleared under the lock, so a push after this spawns anew.
                DRAINING.store(false, Ordering::Release);
                None
            } else {
                Some(q.remove(0))
            }
        };
        let Some((key, value)) = next else { return };
        match key {
            "brightness" => {
                use crate::gpu::gpudev::{backlight, BACKLIGHT_SCALE};
                // Empty: only read, so the cached level is known.
                let level = value.parse::<u32>().ok().map(|p| p * (BACKLIGHT_SCALE / 100));
                backlight(level);
            }
            "wallpaper" => crate::intent::wallpaper::set_by_name(&value),
            _ => {}
        }
    }
}

fn check_any(v: &str) -> Result<String, String> {
    Ok(v.to_string())
}

fn check_theme(v: &str) -> Result<String, String> {
    match v {
        "dark" | "light" | "auto" => Ok(v.to_string()),
        _ => Err("theme takes dark, light or auto".into()),
    }
}

fn check_accent(v: &str) -> Result<String, String> {
    if crate::shade::widgets::palette::accent_is_valid(v) {
        Ok(v.to_ascii_lowercase())
    } else {
        Err("accent takes auto, rose, sage, blue, amber or #RRGGBB".into())
    }
}

fn check_wallpaper(v: &str) -> Result<String, String> {
    // A bare name in the wallpapers folder: never a path, so the decoder
    // reads nothing else for an app.
    if v.is_empty() || v.contains('/') || v.starts_with('.') {
        return Err("wallpaper takes a file name from pictures/wallpapers".into());
    }
    if crate::intent::wallpaper::find_in_dir(v).is_none() {
        return Err(alloc::format!("no wallpaper '{}' in pictures/wallpapers", v));
    }
    Ok(v.to_string())
}

fn check_scale(v: &str) -> Result<String, String> {
    match crate::shade::widgets::scale::parse(v) {
        Some(_) => Ok(v.to_string()),
        None => Err("shade.scale takes 1.0 to 3.0, e.g. 1.25".into()),
    }
}

fn check_code_scheme(v: &str) -> Result<String, String> {
    use crate::shade::widgets::palette as pal;
    if v == "auto" || pal::code_scheme_exists(v) {
        return Ok(v.to_string());
    }
    let mut msg = alloc::format!("unknown code scheme '{}'; known: auto", v);
    for n in pal::code_scheme_names() {
        msg.push_str(", ");
        msg.push_str(n);
    }
    Err(msg)
}

fn check_brightness(v: &str) -> Result<String, String> {
    if !crate::gpu::gpudev::backlight_supported() {
        return Err("no driver sets the backlight of this display".into());
    }
    match v.parse::<u32>() {
        Ok(n) if n <= 100 => Ok(n.to_string()),
        _ => Err("brightness takes 0 to 100".into()),
    }
}

fn check_keyboard(v: &str) -> Result<String, String> {
    match v {
        "de_CH" | "de_DE" | "us" => Ok(v.to_string()),
        _ => Err("keyboard takes de_CH, de_DE or us".into()),
    }
}

fn check_percent(v: &str, lo: i32, hi: i32, what: &str) -> Result<String, String> {
    match v.parse::<i32>() {
        Ok(n) if (lo..=hi).contains(&n) => Ok(n.to_string()),
        _ => Err(alloc::format!("{} takes {} to {}", what, lo, hi)),
    }
}

fn check_mouse_speed(v: &str) -> Result<String, String> {
    check_percent(v, 25, 600, "mouse speed")
}

fn check_mouse_size(v: &str) -> Result<String, String> {
    check_percent(v, 50, 300, "mouse size")
}

fn check_lang(v: &str) -> Result<String, String> {
    match v {
        "en" | "de" => Ok(v.to_string()),
        _ => Err("lang takes en or de".into()),
    }
}

fn check_timezone(v: &str) -> Result<String, String> {
    // The ESP copy is read back through a 128-byte buffer.
    if v.len() > 64 {
        return Err("timezone is at most 64 characters".into());
    }
    match crate::time::zone::parse(v) {
        Some(_) => Ok(v.to_string()),
        None => Err(alloc::format!("unknown timezone '{}'", v)),
    }
}
