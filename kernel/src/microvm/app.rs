//! The app a microvm runs: the image it boots and the home image that
//! keeps its profile. One microvm runs at a time; the launch sets which.

use core::sync::atomic::{AtomicU8, Ordering};

pub struct App {
    /// The intent that starts it, and the title of its windows: the dock
    /// matches its entry against it.
    pub name: &'static str,
    /// npkFS object of its read-only squashfs image.
    pub sqfs: &'static str,
    /// npkFS object of its encrypted home image.
    pub home: &'static str,
}

/// LibreWolf, frozen; its image is part of every install.
pub static LIBREWOLF: App = App {
    name: "librewolf",
    sqfs: "sys/microvm/userspace.sqfs",
    home: "sys/microvm/apps/browser/home.img",
};

/// Firefox, with video through VA-API (Intel's media driver).
pub static FIREFOX: App = App {
    name: "firefox",
    sqfs: "sys/microvm/firefox.sqfs",
    home: "sys/microvm/apps/firefox/home.img",
};

static APPS: [&App; 2] = [&LIBREWOLF, &FIREFOX];

static CURRENT: AtomicU8 = AtomicU8::new(0);

/// The app the next launch starts.
pub fn set(app: &'static App) {
    let i = APPS.iter().position(|a| core::ptr::eq(*a, app)).unwrap_or(0);
    CURRENT.store(i as u8, Ordering::Release);
}

/// The app of the running (or starting) microvm.
pub fn current() -> &'static App {
    APPS[CURRENT.load(Ordering::Acquire) as usize % APPS.len()]
}
