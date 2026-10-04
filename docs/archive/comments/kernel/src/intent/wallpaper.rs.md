# `kernel/src/intent/wallpaper.rs` @ 5e0102684

## L1-5 · `use crate::{kprintln, kprint};`

```
//! Wallpaper intent — set, clear, list, random wallpaper.
//!
//! Images are stored in npkFS under home/<user>/wallpapers/.
//! The wallpaper WASM module (sys/wasm/wallpaper) decodes PNG→BGRA.
//! Without the WASM module, raw BGRA data is used directly.
```

## L11 · `pub fn intent_wallpaper(args: &str) {`

```
/// Set a wallpaper by name (relative to CWD or absolute).
```

## L42-43 · `fn parse_resolution(arg: &str) -> (u32, u32) {`

```
/// Parse an optional `WxH` resolution argument. Returns native
/// framebuffer resolution when the string is empty or malformed.
```

## L68 · `fn ensure_wallpaper_dir() {`

```
/// Ensure the wallpapers directory exists.
```

## L74 · `fn list_wallpapers() {`

```
/// List all wallpapers in the user's wallpapers/ directory.
```

## L94 · `fn get_wallpaper_names() -> Vec<String> {`

```
/// Get all wallpaper names (full paths) for random selection.
```

## L108 · `fn set_wallpaper(name: &str) {`

```
/// Set wallpaper from a specific file in npkFS.
```

## L115-119 · `let resolved = super::resolve_path(name);`

```
// Search order: exact CWD path, exact wallpaper-dir path, then
// wallpaper-dir + common image extensions. The extension fallback
// lets users type `wallpaper set npk01` without remembering whether
// it's `.png` or `.jpg` — file managers + terminals universally
// tolerate the bare-name form.
```

## L154-155 · `fn apply_wallpaper_data(name: &str, data: &[u8]) {`

```
/// Apply raw image data as wallpaper.
/// Tries WASM PNG decoder first, falls back to raw BGRA.
```

## L157 · `let is_png = data.len() > 8 && data[..8] == [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];`

```
// Check for PNG magic: \x89PNG\r\n\x1a\n
```

## L161-163 · `if decode_with_wasm(name) {`

```
// Try WASM PNG decoder module
// The module sets the wallpaper through npk_set_wallpaper, which
// redraws (on Core 0, see `shade::force_redraw`).
```

## L167-169 · `return;`

```
// decode_with_wasm prints its own diagnostic on the WASM
// error path (fuel exhaustion, missing module, …) — no
// generic follow-up needed.
```

## L173-174 · `if data.len() < 8 {`

```
// Raw BGRA data — need to know dimensions
// Convention: first 8 bytes = width (u32 LE) + height (u32 LE), then pixels
```

## L194 · `crate::config::set("wallpaper", name);`

```
// Save active wallpaper to config
```

## L200-201 · `fn decode_with_wasm(name: &str) -> bool {`

```
/// Decode PNG via WASM module and set as wallpaper. The module fetches the
/// image bytes itself through `npk_fetch`, so only the name goes across.
```

## L203 · `let (wasm_bytes, _) = match crate::npkfs::fetch("sys/wasm/wallpaper") {`

```
// Check if wallpaper WASM module is installed
```

## L209 · `let _ = crate::npkfs::store(".npk-wallpaper-target", name.as_bytes(),`

```
// Store the target filename for the module to read via npk_fetch
```

## L213 · `let module_cap = match crate::capability::create_module_cap(`

```
// Delegate capability: READ (fetch image) + WRITE (set wallpaper) + EXECUTE
```

## L216 · `Some(30_000), // 5 minutes (PNG decode can be slow)`

```
// 5 minutes (PNG decode can be slow)
```

## L222-229 · `let fuel: u64 = u64::MAX / 2;`

```
// PNG decode is one of the few WASM call sites where fuel-metering
// bites: a 4K image (3840×2400 → ~37 MB BGRA) can take many
// billions of wasmi instructions to DEFLATE-decompress + filter +
// swizzle. Earlier scaling heuristics (`data.len() * 200`) under-
// estimated the cost. Wallpaper is a bundled / signature-trusted
// module that runs once per intent and exits, so there's no DoS
// surface to defend against here — bypass fuel-metering with the
// interactive cap (= u64::MAX / 2, effectively unlimited).
```

## L232 · `match crate::wasm::execute_sandboxed_with_fuel(&wasm_bytes, "_start", &[], module_cap, fuel) {`

```
// Run the WASM module (_start reads .npk-wallpaper-target, decodes, calls npk_set_wallpaper)
```

## L236 · `let _ = crate::npkfs::delete(".npk-wallpaper-target");`

```
// Clean up temp file
```

## L248-256 · `pub fn apply_startup_wallpaper() {`

```
/// Set a random wallpaper from the user's collection.
/// The wallpaper to bring up at boot.
///
/// A wallpaper someone chose outlives the boot that follows it — so the
/// stored one wins, and the random pick is only for a system that has never
/// been asked. Boot used to call `random_wallpaper` unconditionally, which
/// not only ignored the choice but **overwrote** it: applying a wallpaper
/// persists its name, so every boot silently replaced `wallpaper` in the
/// config with whatever it had just rolled.
```

## L259-260 · `Some(name) if name.is_empty() => {}`

```
// Explicitly cleared (`wallpaper clear` stores an empty value) —
// keep the default background, do not roll a new one.
```

## L267-268 · `random_wallpaper();`

```
// Configured but gone (deleted, or a fresh install that never
// got the file) — fall back rather than boot to a bare desktop.
```

## L271 · `None => random_wallpaper(),`

```
// Never chosen → first impression is a random one.
```

## L279 · `return; // Silent — no wallpapers, keep aurora`

```
// Silent — no wallpapers, keep aurora
```

## L293-295 · `fn delegate_to_wallpaper(target: String, w: u32, h: u32, label: &str) {`

```
/// Run the wallpaper WASM module with an `@<mode>:…` target string.
/// Module handles all pixel math + stores under wp_dir + calls
/// npk_set_wallpaper itself for modes that apply immediately.
```

## L322 · `let fuel = (w as u64) * (h as u64) * 4 * 40 + 50_000_000;`

```
// ~40 instructions per pixel × 4 themes worst case + constant overhead.
```

## L393 · `fn clear_wallpaper() {`

```
/// Clear wallpaper, revert to default background.
```

