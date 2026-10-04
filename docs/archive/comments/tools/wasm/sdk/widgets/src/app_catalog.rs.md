# `tools/wasm/sdk/widgets/src/app_catalog.rs` @ 5e0102684

## L1-11 · `use alloc::string::{String, ToString};`

```
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
```

## L19-21 · `#[link(wasm_import_module = "env")]`

```
// Host functions are WASM imports from the `env` module, resolved by the
// kernel at instantiation. Naming the module explicitly is what makes them
// imports rather than ordinary undefined C symbols, which rust-lld rejects.
```

## L25-28 · `fn npk_app_meta(name_ptr: i32, name_len: i32, buf_ptr: i32, buf_max: i32) -> i32;`

```
// Kernel extracts just the `.npk.app_meta` custom section of sys/wasm/<name>
// and copies it here — no whole-module fetch, so module size is irrelevant
// (beak is >2 MB of embedded fonts; the old whole-module reader truncated it
// and dropped the app from the catalog).
```

## L32 · `#[derive(Clone, Copy, PartialEq, Eq)]`

```
/// What a catalog entry launches.
```

## L35 · `Module,`

```
/// A WASM module under `sys/wasm/<name>` — spawn via `npk_spawn_module`.
```

## L37 · `Intent,`

```
/// A built-in system intent (e.g. "browser") — invoke via `npk_run_intent`.
```

## L41 · `#[derive(Clone)]`

```
/// One launchable app.
```

## L44 · `pub launch_name:  String,`

```
/// Module name (`Module`) or intent verb (`Intent`).
```

## L52-53 · `const LIST_BUF_SIZE: usize = 4096;`

```
// Scratch buffers. Each wasm instance owns its own copy, so drun and
// dock never share these — no cross-app aliasing.
```

## L57-58 · `const META_BUF_SIZE: usize = 4096;`

```
// The kernel returns just the app_meta payload (name + description + icon),
// tens of bytes — 4 KB is ample and independent of module size.
```

## L62-66 · `const SYSTEM_HIDDEN: &[&str] = &["debug", "testdisk", "wifi", "wallpaper", "snap"];`

```
/// System / background / dev modules that are never user-launchable apps, so
/// they don't clutter the launcher or dock. (Panels dock/bar/drun are excluded
/// per-caller via `exclude`.) Background drivers that ship no `.npk.app_meta`
/// section are hidden automatically — this list is only for modules that DO
/// carry app_meta but still aren't apps.
```

## L69-71 · `pub fn load(exclude: &[&str]) -> Vec<AppEntry> {`

```
/// Load the full catalog: installed modules + standard built-in intents,
/// sorted by display name. `exclude` skips module names (e.g. the
/// caller's own module so it doesn't list itself).
```

## L83 · `if let Some(e) = hydrate_module(s) { entries.push(e); }`

```
// No app_meta ⇒ background/driver module ⇒ not a launchable app.
```

## L97-98 · `pub fn builtin_intents() -> Vec<AppEntry> {`

```
/// Standard built-in intents — apps that aren't WASM modules. Future
/// apps (office, ide, …) join here; the launch path is `npk_run_intent`.
```

## L111-113 · `fn hydrate_module(module_name: &str) -> Option<AppEntry> {`

```
/// Build a catalog entry from a module's `.npk.app_meta`. Returns None when the
/// module has no app_meta — that marks it a background/driver module (e.g. the
/// AML battery driver), which is never shown as a launchable app.
```

## L141 · `match r {`

```
// Exhaustive in-crate: a future IconRef variant forces an update here.
```

