# `kernel/src/intent/wasm.rs` @ 5e0102684

## L1 · `use crate::{kprint, kprintln};`

```
//! WASM intents: run, driver
```

## L17 · `let resolved = resolve_path(module_name);`

```
// Load module from npkFS: try cwd-relative, then sys/wasm/
```

## L28 · `let declared = capability::widget_rights_from_wasm(&wasm_bytes);`

```
// BLAKE3 integrity verified by npkfs::fetch
```

## L30-47 · `let declared = capability::widget_rights_from_wasm(&wasm_bytes);`

```
// Delegate full standard caps (READ + WRITE + EXECUTE + RENDER) for
// 60 s. Trust comes from: (a) the module is ECDSA-P-384-signed and
// verified at install time, (b) the user explicitly typed `run`,
// (c) the wasmi sandbox bounds memory + fuel + host-fn surface.
// AUDIT stays off — apps should not introspect kernel state.
// 600_000 ticks ≈ 100 minutes at 100 Hz. wasmi's instantiate +
// first-touch of large bump heaps eats tens of seconds on the N100
// before the module's first host-fn call; the old 60 s TTL was
// expired by the time WASM actually started running. 100 min is
// generous + bounded so a hung worker still gets reaped.
// Was die feste Liste seit v0.83.x vergibt, PLUS was das Modul in
// `.npk.caps` deklariert. Der Klickweg (`npk_spawn_module`) liest die
// Deklaration laengst; der Terminalweg tat es nicht — deshalb hatte beak
// vom Prompt aus kein NET und kein CANVAS, iris kein CANVAS, snap kein
// CAPTURE. Vereinigung statt Ersetzung: so verliert kein Modul ein Recht,
// auf das es sich hier bisher verlassen konnte. Dass die feste Liste ein
// WRITE an neun Module verschenkt, die es nie deklariert haben, bleibt
// offen — das ist eine eigene Entscheidung und ein eigener Commit.
```

## L65 · `let args_vec: alloc::vec::Vec<Val> = arg_str.split_whitespace()`

```
// Parse args as i32 values
```

## L71 · `let func_name = if args_vec.is_empty() { "_start" } else { module_name };`

```
// Determine function name: if no args, try _start; otherwise use module name
```

## L74-81 · `let launch_arg = if args_vec.is_empty() && !arg_str.trim().is_empty() {`

```
// Alles, was KEINE Zahl ist, geht als STARTARGUMENT mit — dieselbe
// Zeichenkette, die `npk_open` einer App gibt und die sie mit
// `npk_launch_arg` abholt. Bisher kam sie nur von einer anderen App;
// von der Shell aus fiel sie auf den Boden, und `beak https://…`
// startete beak ohne die Adresse, obwohl beak sie beim Start liest.
//
// Die Zahlenform bleibt, wie sie war: wer `<modul> 3 4` tippt, ruft
// weiterhin den gleichnamigen Export mit zwei i32.
```

## L88-103 · `if declared.contains(capability::Rights::RENDER) {`

```
// 10 B fuel — bumped from 1 B after testdisk's 100 MB phase
// exhausted it. wasmi charges ~1 fuel per WASM instruction; bulk
// memory ops (memory.fill / memory.copy) for 100+ MB buffers
// burn through hundreds of millions of fuel units in a single
// call. 10 B keeps the bulk-bench paths comfortable without
// making infinite loops free.
// Eine Fensteranwendung gehoert auf einen Arbeitskern, nicht in die
// Shell-Schleife. Woran man sie erkennt: sie hat RENDER SELBST deklariert
// (die feste Liste oben vergibt es an jeden, taugt also nicht als
// Unterscheidung).
//
// Das war bisher der Unterschied zwischen „beak vom Dock" und „beak vom
// Prompt": der Klickweg spawnt, der Terminalweg fuehrte blockierend aus —
// mit `pid: 0`, und ohne Prozessnummer lehnt `fetch::begin_one` jeden
// asynchronen Abruf ab. beak ging auf und blieb leer, mit
// „async fetch needs a process" im Log.
```

## L126-128 · `pub fn intent_run_background(module_name: &str) {`

```
/// Run a WASM module as a background task in the current window.
/// The intent shell stays active — the module runs in parallel, sharing the
/// terminal (output visible) but NOT capturing input. Used by debug.wasm.
```

## L142-149 · `let declared = capability::widget_rights_from_wasm(&wasm_bytes);`

```
// Was die feste Liste seit v0.83.x vergibt, PLUS was das Modul in
// `.npk.caps` deklariert. Der Klickweg (`npk_spawn_module`) liest die
// Deklaration laengst; der Terminalweg tat es nicht — deshalb hatte beak
// vom Prompt aus kein NET und kein CANVAS, iris kein CANVAS, snap kein
// CAPTURE. Vereinigung statt Ersetzung: so verliert kein Modul ein Recht,
// auf das es sich hier bisher verlassen konnte. Dass die feste Liste ein
// WRITE an neun Module verschenkt, die es nie deklariert haben, bleibt
// offen — das ist eine eigene Entscheidung und ein eigener Commit.
```

## L174-175 · `pub fn intent_run_interactive(module_name: &str) {`

```
/// Run a WASM module on a worker core in the current window.
/// Returns immediately — intent loop routes keys when this window is focused.
```

## L180-181 · `pub fn intent_run_interactive_forge(module_name: &str) {`

```
/// Dasselbe, aber unter forge. Eigener Eingang statt einer globalen Fahne:
/// so laeuft genau EIN Modul auf dem neuen Motor und alles andere wie bisher.
```

## L199-206 · `let declared = capability::widget_rights_from_wasm(&wasm_bytes);`

```
// Was die feste Liste seit v0.83.x vergibt, PLUS was das Modul in
// `.npk.caps` deklariert. Der Klickweg (`npk_spawn_module`) liest die
// Deklaration laengst; der Terminalweg tat es nicht — deshalb hatte beak
// vom Prompt aus kein NET und kein CANVAS, iris kein CANVAS, snap kein
// CAPTURE. Vereinigung statt Ersetzung: so verliert kein Modul ein Recht,
// auf das es sich hier bisher verlassen konnte. Dass die feste Liste ein
// WRITE an neun Module verschenkt, die es nie deklariert haben, bleibt
// offen — das ist eine eigene Entscheidung und ein eigener Commit.
```

## L220 · `let term_idx = crate::shade::terminal::active_idx();`

```
// Use current terminal — top takes over this window
```

## L227-228 · `let ok = if use_forge {`

```
// Spawn on worker core — returns immediately
// Intent loop will route keys when this window is focused
```

## L239-241 · `pub fn intent_run_driver(args: &str) {`

```
/// Run a WASM driver module with PCI device access.
/// Usage: driver <module> [bus:dev.func]
/// If no BDF given, auto-detects by module name.
```

## L253-259 · `if crate::drivers::netdev::wasm_nic_available() {`

```
// One card, one driver. Nothing used to stop a second `driver wifi_ax200`
// next to the autostarted one: both map the MMIO, both run nic_init — so
// the newcomer resets the card and reloads its firmware UNDER the running
// instance — both post their own RB rings, and together they need twice the
// per-module DMA budget. The frames that come out of that read as corrupt
// (`RX payload offset mismatch ... found nowhere`), which sends the next
// hour of debugging after the radio instead of after the second process.
```

## L267 · `let sys_path = alloc::format!("sys/wasm/{}", module_name);`

```
// Load WASM module from npkFS
```

## L278 · `let dev = if !bdf_arg.is_empty() {`

```
// Find PCI device: manual BDF or auto-detect by module name
```

## L280 · `parse_bdf(bdf_arg).and_then(|(bus, dev, func)| {`

```
// Parse "bus:dev.func" format
```

## L294 · `auto_detect_device(module_name)`

```
// Auto-detect: "wifi" -> class 02:80 (Network controller, other)
```

## L306 · `let a = dev.addr;`

```
// Create PCI device capability
```

## L311 · `None, // no expiry for drivers`

```
// no expiry for drivers
```

## L329 · `let mut parts = s.splitn(2, ':');`

```
// "6c:00.0" -> (0x6c, 0, 0). Hex to match lspci output.
```

## L341-342 · `if name.starts_with("wifi") || name.starts_with("wlan") || name.starts_with("wireless") {`

```
// Prefix-match so chip-specific names (wifi_ax200, wifi_rtl8852be, …) all
// resolve to the network class without hardcoding each chip in the kernel.
```

## L344 · `return pci::find_by_class(0x02, 0x80)`

```
// Class 02:80 = Network controller (other — WiFi)
```

## L349 · `return pci::find_by_class(0x0D, 0x01);`

```
// Bluetooth is often on the same device or a USB subfunction
```

