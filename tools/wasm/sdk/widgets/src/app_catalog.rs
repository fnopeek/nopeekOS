//! App catalog — shared launcher/dock data source.
//!
//! Enumerates installed WASM modules (`sys/wasm/*`), hydrates each
//! app's `.npk.app_meta` custom section (icon / name / description),
//! and appends the standard built-in intents (apps that live as
//! microvm bundles / Surface windows rather than WASM modules, e.g.
//! the browser). Both `drun` and `dock` consume this so they show the
//! same set of launchable apps.
//!
//! wasm32-only: it calls the `npk_list_modules` / `npk_fetch` host fns,
//! so it's compiled out of host-side test builds of the SDK.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::abi::IconId;
use crate::app_meta::{self, AppMeta, IconRef};

unsafe extern "C" {
    fn npk_list_modules(ptr: i32, max: i32) -> i32;
    fn npk_fetch(name_ptr: i32, name_len: i32, buf_ptr: i32, buf_max: i32) -> i32;
}

/// What a catalog entry launches.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    /// A WASM module under `sys/wasm/<name>` — spawn via `npk_spawn_module`.
    Module,
    /// A built-in system intent (e.g. "browser") — invoke via `npk_run_intent`.
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

// 2 MB covers every first-party module including wifi (~1.5 MB). Reused
// per hydrate call — only the meta bytes are extracted, then the wasm
// is discarded before the next fetch.
const WASM_FETCH_BUF_SIZE: usize = 2 * 1024 * 1024;
static mut WASM_FETCH_BUF: [u8; WASM_FETCH_BUF_SIZE] = [0; WASM_FETCH_BUF_SIZE];

/// Load the full catalog: installed modules + standard built-in intents,
/// sorted by display name. `exclude` skips module names (e.g. the
/// caller's own module so it doesn't list itself).
pub fn load(exclude: &[&str]) -> Vec<AppEntry> {
    let mut entries: Vec<AppEntry> = Vec::new();

    let buf_ptr = core::ptr::addr_of_mut!(LIST_BUF) as *mut u8;
    let n = unsafe { npk_list_modules(buf_ptr as i32, LIST_BUF_SIZE as i32) };
    if n > 0 {
        let slice = unsafe { core::slice::from_raw_parts(buf_ptr as *const u8, n as usize) };
        for chunk in slice.split(|&b| b == 0) {
            if chunk.is_empty() { continue; }
            if let Ok(s) = core::str::from_utf8(chunk) {
                if exclude.contains(&s) { continue; }
                entries.push(hydrate_module(s));
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
            launch_name:  "browser".to_string(),
            display_name: "Browser".to_string(),
            description:  "LibreWolf in a sandboxed MicroVM".to_string(),
            // TODO: a proper Globe glyph; Monitor is the closest existing one.
            icon:         IconId::Monitor,
            kind:         EntryKind::Intent,
        },
    ]
}

fn hydrate_module(module_name: &str) -> AppEntry {
    if let Some(meta) = read_meta(module_name) {
        return AppEntry {
            launch_name:  module_name.to_string(),
            display_name: meta.display_name,
            description:  meta.description,
            icon:         icon_ref_to_id(&meta.icon),
            kind:         EntryKind::Module,
        };
    }
    AppEntry {
        launch_name:  module_name.to_string(),
        display_name: module_name.to_string(),
        description:  String::new(),
        icon:         IconId::List,
        kind:         EntryKind::Module,
    }
}

fn read_meta(name: &str) -> Option<AppMeta> {
    let path = alloc::format!("sys/wasm/{}", name);
    let buf_ptr = core::ptr::addr_of_mut!(WASM_FETCH_BUF) as *mut u8;
    let n = unsafe {
        npk_fetch(
            path.as_ptr() as i32,
            path.len() as i32,
            buf_ptr as i32,
            WASM_FETCH_BUF_SIZE as i32,
        )
    };
    if n <= 0 { return None; }
    let wasm = unsafe { core::slice::from_raw_parts(buf_ptr as *const u8, n as usize) };
    let meta_bytes = extract_custom_section(wasm, ".npk.app_meta")?;
    app_meta::decode(meta_bytes).ok()
}

fn icon_ref_to_id(r: &IconRef) -> IconId {
    // Exhaustive in-crate: a future IconRef variant forces an update here.
    match r {
        IconRef::Builtin(id) => *id,
    }
}

fn extract_custom_section<'a>(wasm: &'a [u8], target: &str) -> Option<&'a [u8]> {
    if wasm.len() < 8 { return None; }
    if &wasm[0..4] != b"\0asm" { return None; }
    if &wasm[4..8] != &[0x01, 0x00, 0x00, 0x00] { return None; }
    let mut cur = &wasm[8..];
    while !cur.is_empty() {
        let section_id = cur[0];
        cur = &cur[1..];
        let (size, consumed) = read_leb128_u32(cur)?;
        cur = &cur[consumed..];
        if size as usize > cur.len() { return None; }
        let (payload, rest) = cur.split_at(size as usize);
        cur = rest;
        if section_id != 0 { continue; }
        let (name_len, consumed) = match read_leb128_u32(payload) {
            Some(p) => p,
            None => continue,
        };
        let name_end = consumed + name_len as usize;
        if name_end > payload.len() { continue; }
        if &payload[consumed..name_end] == target.as_bytes() {
            return Some(&payload[name_end..]);
        }
    }
    None
}

fn read_leb128_u32(buf: &[u8]) -> Option<(u32, usize)> {
    let mut result: u32 = 0;
    let mut shift: u32 = 0;
    for (i, &b) in buf.iter().enumerate() {
        if shift >= 32 { return None; }
        let payload = (b & 0x7F) as u32;
        if shift == 28 && (payload & !0x0F) != 0 { return None; }
        result |= payload << shift;
        if (b & 0x80) == 0 {
            return Some((result, i + 1));
        }
        shift += 7;
    }
    None
}
