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
//! so it's compiled out of host-side test builds of the SDK.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::abi::IconId;
use crate::app_meta::{self, AppMeta, IconRef};

// Host functions are WASM imports from the `env` module, resolved by the
// kernel at instantiation. Naming the module explicitly is what makes them
// imports rather than ordinary undefined C symbols, which rust-lld rejects.
#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn npk_list_modules(ptr: i32, max: i32) -> i32;
    // The kernel extracts just the `.npk.app_meta` custom section of
    // sys/wasm/<name> and copies it here — no whole-module fetch, so module
    // size (beak carries megabytes of embedded fonts) is irrelevant.
    fn npk_app_meta(name_ptr: i32, name_len: i32, buf_ptr: i32, buf_max: i32) -> i32;
}

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

// Scratch buffers. Each wasm instance owns its own copy, so drun and
// dock never share these — no cross-app aliasing.
const LIST_BUF_SIZE: usize = 4096;
static mut LIST_BUF: [u8; LIST_BUF_SIZE] = [0; LIST_BUF_SIZE];

// The kernel returns just the app_meta payload (name + description + icon),
// tens of bytes — 4 KB is ample and independent of module size.
const META_BUF_SIZE: usize = 4096;
static mut META_BUF: [u8; META_BUF_SIZE] = [0; META_BUF_SIZE];

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

    let buf_ptr = core::ptr::addr_of_mut!(LIST_BUF) as *mut u8;
    // SAFETY: the host writes at most `LIST_BUF_SIZE` bytes into `LIST_BUF`,
    // which this single-threaded module uses only here.
    let n = unsafe { npk_list_modules(buf_ptr as i32, LIST_BUF_SIZE as i32) };
    if n > 0 {
        // SAFETY: a positive return is the byte count written, at most `LIST_BUF_SIZE`.
        let slice = unsafe { core::slice::from_raw_parts(buf_ptr as *const u8, n as usize) };
        for chunk in slice.split(|&b| b == 0) {
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
    let buf_ptr = core::ptr::addr_of_mut!(META_BUF) as *mut u8;
    // SAFETY: the host reads `name`, borrowed for the call, and writes at most
    // `META_BUF_SIZE` bytes into `META_BUF`, used only here on this single thread.
    let n = unsafe {
        npk_app_meta(
            name.as_ptr() as i32,
            name.len() as i32,
            buf_ptr as i32,
            META_BUF_SIZE as i32,
        )
    };
    if n <= 0 { return None; }
    // SAFETY: a positive return is the byte count written, at most `META_BUF_SIZE`.
    let meta_bytes = unsafe { core::slice::from_raw_parts(buf_ptr as *const u8, n as usize) };
    app_meta::decode(meta_bytes).ok()
}

fn icon_ref_to_id(r: &IconRef) -> IconId {
    // Exhaustive in-crate: a future IconRef variant forces an update here.
    match r {
        IconRef::Builtin(id) => *id,
    }
}
