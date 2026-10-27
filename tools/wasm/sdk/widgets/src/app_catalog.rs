//! App catalog — shared launcher/dock data source.
//!
//! Enumerates installed WASM modules (`sys/wasm/*`), hydrates each
//! app's `.npk.app_meta` custom section (icon / name / description),
//! and appends the standard built-in intents (apps that live as
//! microvm bundles / Surface windows rather than WASM modules, e.g.
//! the browser). Both `drun` and `dock` consume this so they show the
//! same set of launchable apps.
//!
//! wasm32-only: it calls the `npk_list_modules` / `npk_app_meta` host fns,
//! so it's compiled out of host-side test builds of the SDK. The kernel
//! extracts just the `.npk.app_meta` section of a module, so module size
//! (beak carries megabytes of embedded fonts) is irrelevant.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::abi::IconId;
use crate::app_meta::{self, AppMeta, IconRef};
use npk_sys::read_all;

/// What a catalog entry launches.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    /// A WASM module under `sys/wasm/<name>` — spawn via `npk_spawn_module`.
    Module,
    /// A built-in system intent (e.g. "firefox") — invoke via `npk_run_intent`.
    Intent,
}

/// One launchable app.
#[derive(Clone)]
pub struct AppEntry {
    /// Module name (`Module`) or intent verb (`Intent`).
    pub launch_name:  String,
    pub display_name: String,
    pub description:  String,
    pub icon:         IconId,
    pub kind:         EntryKind,
}

/// System / background / dev modules that are never user-launchable apps, so
/// they don't clutter the launcher or dock. (Panels dock/bar/drun are excluded
/// per-caller via `exclude`.) Background drivers that ship no `.npk.app_meta`
/// section are hidden automatically — this list is only for modules that do
/// carry app_meta but still aren't apps.
const SYSTEM_HIDDEN: &[&str] = &["debug", "testdisk", "wifi", "wallpaper", "snap"];

/// Load the full catalog: installed modules + standard built-in intents,
/// sorted by display name. `exclude` skips module names (e.g. the
/// caller's own module so it doesn't list itself).
pub fn load(exclude: &[&str]) -> Vec<AppEntry> {
    let mut entries: Vec<AppEntry> = Vec::new();

    if let Some(list) = read_all(npk_sys::list_modules) {
        for chunk in list.split(|&b| b == 0) {
            if chunk.is_empty() { continue; }
            if let Ok(s) = core::str::from_utf8(chunk) {
                if exclude.contains(&s) || SYSTEM_HIDDEN.contains(&s) { continue; }
                // No app_meta ⇒ background/driver module ⇒ not a launchable app.
                if let Some(e) = hydrate_module(s) { entries.push(e); }
            }
        }
    }

    for b in builtin_intents() {
        entries.push(b);
    }

    entries.sort_by(|a, b| a.display_name.cmp(&b.display_name));
    entries
}

/// Standard built-in intents — apps that aren't WASM modules. Future
/// apps (office, ide, …) join here; the launch path is `npk_run_intent`.
pub fn builtin_intents() -> Vec<AppEntry> {
    alloc::vec![
        AppEntry {
            launch_name:  "firefox".to_string(),
            display_name: "Firefox".to_string(),
            description:  "Firefox in a sandboxed MicroVM, video on the GPU".to_string(),
            icon:         IconId::Fire,
            kind:         EntryKind::Intent,
        },
    ]
}

/// Build a catalog entry from a module's `.npk.app_meta`. Returns None when the
/// module has no app_meta — that marks it a background/driver module (e.g. the
/// AML battery driver), which is never shown as a launchable app.
fn hydrate_module(module_name: &str) -> Option<AppEntry> {
    let meta = read_meta(module_name)?;
    Some(AppEntry {
        launch_name:  module_name.to_string(),
        display_name: meta.display_name,
        description:  meta.description,
        icon:         icon_ref_to_id(&meta.icon),
        kind:         EntryKind::Module,
    })
}

fn read_meta(name: &str) -> Option<AppMeta> {
    let meta = read_all(|b| npk_sys::app_meta(name.as_bytes(), b))?;
    if meta.is_empty() { return None; }
    app_meta::decode(&meta).ok()
}

fn icon_ref_to_id(r: &IconRef) -> IconId {
    // Exhaustive in-crate: a future IconRef variant forces an update here.
    match r {
        IconRef::Builtin(id) => *id,
    }
}
