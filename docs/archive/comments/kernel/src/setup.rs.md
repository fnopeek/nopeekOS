# `kernel/src/setup.rs` @ 5e0102684

## L1-4 · `use crate::{kprint, kprintln, serial, crypto, config, npkfs, blkdev, capability};`

```
//! First-Boot Setup Wizard
//!
//! Runs on first boot (no .npk-keycheck found).
//! Collects: storage, name, passphrase, timezone, keyboard, language.
```

## L8 · `fn read_line() -> alloc::string::String {`

```
/// Read a line from serial (with echo). Returns trimmed string.
```

## L16 · `fn read_line_default(default: &str) -> alloc::string::String {`

```
/// Read a line, return default if empty.
```

## L26 · `fn read_passphrase() -> (alloc::vec::Vec<u8>, usize) {`

```
/// Read masked passphrase from serial.
```

## L36-37 · `pub fn run_fresh_install(salt: &[u8; 16]) -> bool {`

```
/// Run fresh install setup (npkFS already formatted and mounted).
/// Collects identity + settings only, no storage questions.
```

## L49 · `pub fn run_first_boot(salt: &[u8; 16]) -> bool {`

```
/// Run the first-boot setup wizard (legacy, with storage questions).
```

## L58 · `if blkdev::is_available() {`

```
// === Storage ===
```

## L78 · `match npkfs::mount() {`

```
// Check if already formatted
```

## L102-105 · `kprintln!();`

```
// Hier endet die Installation, und danach haelt die Maschine an —
// kein Prompt, also kein `dmesg`, und `dmesg prev` braeuchte npkFS,
// also genau die Platte, die fehlt. Was der Bildschirm JETZT zeigt,
// ist alles, was es gibt. Also zeigen, was da war.
```

## L118-124 · `fn setup_default_tree(name: &str) -> Result<(), npkfs::fs::Error> {`

```
/// Create the npkFS v3 locked default tree. Idempotent.
///
/// `home/<name>/` mirrors loft's sidebar; `sys/` holds system-managed
/// read-mostly content; `.system/` holds boot-time metadata that the
/// kernel reads before `validate_user_name` filters it from listings.
/// Falls back to "florian" if `name` is empty so the tree still has
/// a usable home dir.
```

## L146-148 · `npkfs::fs::ensure_dir(&alloc::format!("home/{}/music", user))?;`

```
// Trailing extras under the user dir (kept separate so the array
// above stays a fixed-length slice, easier to spot when reviewing
// the canonical layout).
```

## L155 · `fn setup_identity_and_settings(salt: &[u8; 16]) -> bool {`

```
/// Common identity + settings setup (used by both fresh install and legacy first boot)
```

## L157 · `kprintln!("[npk] Identity:");`

```
// === Identity ===
```

## L160-164 · `kprint!("[npk]   Your name: ");`

```
// Name — captured into a local but NOT persisted yet. Writing
// before the master key is set would land a plaintext config blob
// on disk; later config::set calls overwrite to encrypted but the
// plaintext blob lingers as an orphan in the v2 B-tree, and `gc`
// would crash trying to decrypt it during the mark phase.
```

## L168 · `let master_key = loop {`

```
// Passphrase
```

## L193-194 · `if !name.is_empty() {`

```
// From here on every FS write goes through the AEAD path, so it's
// safe to persist the name.
```

## L199-200 · `match npkfs::store(config::KEYCHECK_PATH, config::KEYCHECK_VALUE, capability::CAP_NULL) {`

```
// Store keycheck (encrypted with the new master key, lives at
// .system/keycheck per the v2 locked tree).
```

## L206-208 · `if let Err(e) = setup_default_tree(&name) {`

```
// Lay down the locked default tree once. Idempotent — re-runs are
// a no-op. Apps assume these dirs exist; the installer is the only
// thing that creates them. See docs/archive/NPKFS_V2.md for the spec.
```

## L215 · `kprintln!("[npk] Settings (Enter = default):");`

```
// === Settings ===
```

## L230-234 · `config::set("autostart", "dock bar");`

```
// Default autostart: the app dock + the top bar. Resident apps no
// longer pin their worker core — npk_sleep now runs pending scheduler
// work while it waits, so multiple looping panels coexist without
// starving intents. A config seed, not a kernel hardcode; `set
// autostart ...` overrides it.
```

## L237-239 · `config::set("theme", "auto");`

```
// Theme follows the wallpaper: `auto` picks light/dark from the
// background luminance (palette::is_light_theme). `theme light|dark`
// overrides.
```

