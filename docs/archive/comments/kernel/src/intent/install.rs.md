# `kernel/src/intent/install.rs` @ 5e0102684

## L1-5 · `use crate::{kprintln, kprint};`

```
//! npk install — module package manager.
//!
//! Downloads WASM modules from GitHub release/modules/,
//! verifies ECDSA P-384 signature + SHA-384 hash,
//! stores in npkFS under sys/wasm/<name>.
```

## L13 · `const MAX_MODULE_SIZE: usize = 16 * 1024 * 1024; // 16 MB (buffer is min(content_length, cap), so no waste)`

```
// 16 MB (buffer is min(content_length, cap), so no waste)
```

## L25-32 · `fn parse_manifest(data: &[u8]) -> Result<Vec<ModuleEntry>, &'static str> {`

```
/// Parse the module manifest (one module per block, separated by blank lines).
/// Format:
/// `​``
/// [wallpaper]
/// version=0.1.0
/// size=12345
/// sha384=abcdef...
/// `​``
```

## L44 · `if let (Some(n), Some(v), Some(s), Some(h)) = (name.take(), version.take(), size.take(), sha384.take()) {`

```
// End of block — flush if complete
```

## L51 · `if let (Some(n), Some(v), Some(s), Some(h)) = (name.take(), version.take(), size.take(), sha384.take()) {`

```
// Flush previous if any
```

## L70 · `if let (Some(n), Some(v), Some(s), Some(h)) = (name, version, size, sha384) {`

```
// Flush last block
```

## L88 · `pub fn intent_install(args: &str) {`

```
/// `install <name>` — download and install a WASM module.
```

## L121 · `let store_name = alloc::format!("sys/wasm/{}", name);`

```
// Check if already installed with same version
```

## L136 · `let wasm_path = alloc::format!("{}/{}.wasm", MODULE_BASE, name);`

```
// Download module
```

## L138-142 · `if entry.size == 0 || entry.size > MAX_MODULE_SIZE {`

```
// Bound the fetch by the manifest's own size rather than a fixed cap, so a
// growing module can never be silently truncated at a constant nobody
// remembered to raise (that is exactly how OTA broke when the kernel
// crossed 4 MiB). The manifest is unauthenticated here — SHA-384 and the
// signature below are the real gate — so it may only lower the bound.
```

## L158 · `kprint!("[npk] Verifying SHA-384... ");`

```
// Verify SHA-384
```

## L168 · `kprint!("[npk] Verifying signature... ");`

```
// Verify ECDSA P-384 signature
```

## L184 · `let _ = crate::npkfs::delete(&store_name);`

```
// Delete old version before storing (npkFS doesn't overwrite)
```

## L188 · `if let Err(e) = crate::npkfs::store(&store_name, &wasm_data, crate::capability::CAP_NULL) {`

```
// Store in npkFS
```

## L194 · `let _ = crate::npkfs::store(&version_key, entry.version.as_bytes(), crate::capability::CAP_NULL);`

```
// Store version metadata
```

## L200 · `pub struct ModulePlan {`

```
/// One installed module that the release has a newer build of.
```

## L203 · `pub local: Option<String>,`

```
/// Installed version — `None` when the module has no `.version` sidecar.
```

## L210-212 · `pub fn plan_modules() -> (Vec<ModulePlan>, usize) {`

```
/// Compare installed modules against the release manifest WITHOUT touching
/// anything. Returns what needs updating plus how many were already current,
/// so the caller can show a plan before asking to apply it.
```

## L225-226 · `let installed: Vec<String> =`

```
// List installed modules straight from `sys/wasm` instead of
// walking the entire FS.
```

## L241-242 · `if !installed.iter().any(|n| n.as_str() == remote.name) { continue; }`

```
// Only update modules that are already installed (skip
// `.version` sidecars; we want bare `<name>` to be present).
```

## L266-267 · `pub fn apply_module(p: &ModulePlan) -> bool {`

```
/// Download, verify and install one planned module. Prints its own result
/// line; returns whether the module was written.
```

## L272-275 · `kprintln!("[npk]   + module   {:<10} {}", p.name, p.remote);`

```
// A COMPLETE line before the work, never one left open waiting for its
// `OK`: the download itself prints (redirects, ESP writes, progress), and
// the tail then landed on some later line. Success adds nothing — the
// summary counts it; only a failure speaks again, on its own `!` line.
```

## L311 · `let _ = crate::npkfs::delete(&store_name);`

```
// Delete old module + version before storing new one (npkFS doesn't overwrite)
```

## L323-341 · `pub fn intent_uninstall(args: &str) {`

```
/// `uninstall <name> [--force]` — remove a WASM module, with safety
/// guards that prevent the user from bricking their system:
///
///  1. **Hard block:** the module configured as the active launcher
///     (`sys/config/launcher`, default `drun`) cannot be uninstalled —
///     without it, Mod+D / spawn flow has nothing to open. The user
///     must point `sys/config/launcher` somewhere else first.
///
///  2. **--force gate for bundled modules:** every kernel-bundled
///     module (drun, loft, wifi, wallpaper, top, debug, …) is on the
///     OTA recovery path, so removing one is reversible — but easy
///     to do by accident. Without `--force` we refuse and print the
///     reinstall hint. User-installed third-party modules (none yet)
///     skip this check.
///
/// The block + gate are deliberately implemented in the kernel rather
/// than in a wrapper script: the intent loop is the only path that
/// reaches `npkfs::delete` for `sys/wasm/*`, so this is the right
/// place to keep the invariants honest.
```

## L360 · `if is_active_launcher(name) {`

```
// Guard 1: never uninstall the configured launcher.
```

## L368-369 · `if is_active_picker(name) {`

```
// Same for the file dialog: without it `npk_pick` has nothing to
// spawn, and every app loses Open and Save at once.
```

## L377 · `let store_name = alloc::format!("sys/wasm/{}", name);`

```
// Guard 2: bundled modules need --force.
```

## L403-404 · `fn is_active_picker(name: &str) -> bool {`

```
/// True iff `name` matches the module serving `npk_pick`
/// (`sys/config/picker`, default `pick` — same fallback as the kernel).
```

## L421-422 · `fn is_active_launcher(name: &str) -> bool {`

```
/// True iff `name` matches the active launcher (`sys/config/launcher`).
/// Empty / unset config falls back to `drun` to mirror the boot path.
```

## L439-446 · `fn is_bundled_module(name: &str) -> bool {`

```
/// True iff `name` is shipped as a bundled asset by the kernel.
///
/// The canonical list lives in `install_data/assets/mod.rs` as
/// `BUNDLED_ASSETS`, but that module is `#[cfg(feature = "installer")]`
/// — only the installer build embeds the wasm bytes. The runtime
/// kernel (this code path) needs the names but not the bytes, so we
/// keep a parallel string list here. Keep both in sync when adding
/// or removing a bundled module.
```

## L465 · `pub fn intent_modules() {`

```
/// `modules` — list installed and available modules.
```

## L478-480 · `let version = crate::npkfs::fetch(&version_key).ok()`

```
// `.trim()` is not cosmetic: the sidecar carries a trailing newline,
// and printing it un-trimmed put a blank line after every module that
// had one.
```

## L504-507 · `pub fn intent_assets() {`

```
/// `assets` — what the system carries besides modules: fonts, icons, the
/// microvm payloads, wallpapers. They arrive over the same signed OTA path
/// as modules but were invisible until now, so a 261 MB userspace bundle
/// sat on the disk with nothing to show it.
```

## L509-510 · `const DIRS: &[(&str, &str)] = &[`

```
// Directories rather than the update table: this lists what npkFS
// actually holds, including anything dropped in by hand.
```

