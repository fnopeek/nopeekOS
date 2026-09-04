//! WASM intents: run, driver

use crate::{kprint, kprintln};
use super::resolve_path;

pub fn intent_run(args: &str) {
    use crate::{wasm, npkfs, capability};
    use wasmi::Val;

    let mut parts = args.trim().splitn(2, ' ');
    let module_name = match parts.next() {
        Some(n) if !n.is_empty() => n,
        _ => { kprintln!("[npk] Usage: run <module> [args...]"); return; }
    };
    let arg_str = parts.next().unwrap_or("");

    // Load module from npkFS: try cwd-relative, then sys/wasm/
    let resolved = resolve_path(module_name);
    let sys_path = alloc::format!("sys/wasm/{}", module_name);
    let (wasm_bytes, hash) = match npkfs::fetch(&resolved) {
        Ok(v) => v,
        Err(_) => match npkfs::fetch(&sys_path) {
            Ok(v) => v,
            Err(e) => { kprintln!("[npk] Module '{}': {}", module_name, e); return; }
        }
    };

    // BLAKE3 integrity verified by npkfs::fetch

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
    let declared = capability::widget_rights_from_wasm(&wasm_bytes);
    let module_cap = match capability::create_module_cap(
        capability::Rights::READ
            | capability::Rights::WRITE
            | capability::Rights::EXECUTE
            | capability::Rights::RENDER
            | declared,
        Some(600_000),
    ) {
        Ok(id) => id,
        Err(e) => { kprintln!("[npk] Cap delegation failed: {}", e); return; }
    };

    kprint!("[npk] Running '{}' (hash: ", module_name);
    for b in &hash[..4] { kprint!("{:02x}", b); }
    kprintln!("..., cap: {:08x})", capability::short_id(&module_cap));

    // Parse args as i32 values
    let args_vec: alloc::vec::Vec<Val> = arg_str.split_whitespace()
        .filter_map(|s| s.parse::<i32>().ok())
        .map(|v| Val::I32(v))
        .collect();

    // Determine function name: if no args, try _start; otherwise use module name
    let func_name = if args_vec.is_empty() { "_start" } else { module_name };

    // Alles, was KEINE Zahl ist, geht als STARTARGUMENT mit — dieselbe
    // Zeichenkette, die `npk_open` einer App gibt und die sie mit
    // `npk_launch_arg` abholt. Bisher kam sie nur von einer anderen App;
    // von der Shell aus fiel sie auf den Boden, und `beak https://…`
    // startete beak ohne die Adresse, obwohl beak sie beim Start liest.
    //
    // Die Zahlenform bleibt, wie sie war: wer `<modul> 3 4` tippt, ruft
    // weiterhin den gleichnamigen Export mit zwei i32.
    let launch_arg = if args_vec.is_empty() && !arg_str.trim().is_empty() {
        Some(alloc::string::String::from(arg_str.trim()))
    } else {
        None
    };

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
    if declared.contains(capability::Rights::RENDER) {
        let term_idx = crate::shade::terminal::active_idx();
        if !wasm::spawn_on_worker_with_arg(
            wasm_bytes.to_vec(), module_cap, term_idx, module_name, launch_arg)
        {
            kprintln!("[npk] Failed to spawn '{}'", module_name);
        }
        return;
    }

    match wasm::execute_sandboxed_with_arg(
        &wasm_bytes, func_name, &args_vec, module_cap, 10_000_000_000, launch_arg,
    ) {
        Ok(result) => {
            if !result.output.is_empty() {
                kprintln!("{}", result.output);
            }
        }
        Err(e) => kprintln!("[npk] Execution error: {}", e),
    }
}

/// Run a WASM module as a background task in the current window.
/// The intent shell stays active — the module runs in parallel, sharing the
/// terminal (output visible) but NOT capturing input. Used by debug.wasm.
pub fn intent_run_background(module_name: &str) {
    use crate::{wasm, npkfs, capability};

    let sys_path = alloc::format!("sys/wasm/{}", module_name);
    let resolved = resolve_path(module_name);
    let (wasm_bytes, hash) = match npkfs::fetch(&resolved) {
        Ok(v) => v,
        Err(_) => match npkfs::fetch(&sys_path) {
            Ok(v) => v,
            Err(e) => { kprintln!("[npk] Module '{}': {}", module_name, e); return; }
        }
    };

    // Was die feste Liste seit v0.83.x vergibt, PLUS was das Modul in
    // `.npk.caps` deklariert. Der Klickweg (`npk_spawn_module`) liest die
    // Deklaration laengst; der Terminalweg tat es nicht — deshalb hatte beak
    // vom Prompt aus kein NET und kein CANVAS, iris kein CANVAS, snap kein
    // CAPTURE. Vereinigung statt Ersetzung: so verliert kein Modul ein Recht,
    // auf das es sich hier bisher verlassen konnte. Dass die feste Liste ein
    // WRITE an neun Module verschenkt, die es nie deklariert haben, bleibt
    // offen — das ist eine eigene Entscheidung und ein eigener Commit.
    let declared = capability::widget_rights_from_wasm(&wasm_bytes);
    let module_cap = match capability::create_module_cap(
        capability::Rights::READ
            | capability::Rights::WRITE
            | capability::Rights::EXECUTE
            | capability::Rights::RENDER
            | declared,
        Some(600_000),
    ) {
        Ok(id) => id,
        Err(e) => { kprintln!("[npk] Cap delegation failed: {}", e); return; }
    };

    let term_idx = crate::shade::terminal::active_idx();

    kprint!("[npk] '{}' started background (hash: ", module_name);
    for b in &hash[..4] { kprint!("{:02x}", b); }
    kprintln!("...)");

    if !wasm::spawn_on_worker_background(wasm_bytes.to_vec(), module_cap, term_idx, module_name) {
        kprintln!("[npk] Failed to spawn '{}'", module_name);
    }
}

/// Run a WASM module on a worker core in the current window.
/// Returns immediately — intent loop routes keys when this window is focused.
pub fn intent_run_interactive(module_name: &str) {
    run_interactive_on(module_name, false)
}

/// Dasselbe, aber unter forge. Eigener Eingang statt einer globalen Fahne:
/// so laeuft genau EIN Modul auf dem neuen Motor und alles andere wie bisher.
pub fn intent_run_interactive_forge(module_name: &str) {
    run_interactive_on(module_name, true)
}

fn run_interactive_on(module_name: &str, use_forge: bool) {
    use crate::{wasm, npkfs, capability};

    let sys_path = alloc::format!("sys/wasm/{}", module_name);
    let resolved = resolve_path(module_name);
    let (wasm_bytes, hash) = match npkfs::fetch(&resolved) {
        Ok(v) => v,
        Err(_) => match npkfs::fetch(&sys_path) {
            Ok(v) => v,
            Err(e) => { kprintln!("[npk] Module '{}': {}", module_name, e); return; }
        }
    };

    // Was die feste Liste seit v0.83.x vergibt, PLUS was das Modul in
    // `.npk.caps` deklariert. Der Klickweg (`npk_spawn_module`) liest die
    // Deklaration laengst; der Terminalweg tat es nicht — deshalb hatte beak
    // vom Prompt aus kein NET und kein CANVAS, iris kein CANVAS, snap kein
    // CAPTURE. Vereinigung statt Ersetzung: so verliert kein Modul ein Recht,
    // auf das es sich hier bisher verlassen konnte. Dass die feste Liste ein
    // WRITE an neun Module verschenkt, die es nie deklariert haben, bleibt
    // offen — das ist eine eigene Entscheidung und ein eigener Commit.
    let declared = capability::widget_rights_from_wasm(&wasm_bytes);
    let module_cap = match capability::create_module_cap(
        capability::Rights::READ
            | capability::Rights::WRITE
            | capability::Rights::EXECUTE
            | capability::Rights::RENDER
            | declared,
        Some(600_000),
    ) {
        Ok(id) => id,
        Err(e) => { kprintln!("[npk] Cap delegation failed: {}", e); return; }
    };

    // Use current terminal — top takes over this window
    let term_idx = crate::shade::terminal::active_idx();

    kprint!("[npk] '{}' started (hash: ", module_name);
    for b in &hash[..4] { kprint!("{:02x}", b); }
    kprintln!("...)");

    // Spawn on worker core — returns immediately
    // Intent loop will route keys when this window is focused
    let ok = if use_forge {
        wasm::spawn_on_worker_forge(wasm_bytes.to_vec(), module_cap, term_idx, module_name)
    } else {
        wasm::spawn_on_worker(wasm_bytes.to_vec(), module_cap, term_idx, module_name)
    };
    if !ok {
        kprintln!("[npk] Failed to spawn '{}'", module_name);
    }
}

/// Run a WASM driver module with PCI device access.
/// Usage: driver <module> [bus:dev.func]
/// If no BDF given, auto-detects by module name.
pub fn intent_run_driver(args: &str) {
    use crate::{wasm, npkfs, capability};
    use crate::drivers::pci;

    let mut parts = args.trim().splitn(2, ' ');
    let module_name = match parts.next() {
        Some(n) if !n.is_empty() => n,
        _ => { kprintln!("[npk] Usage: driver <module> [bus:dev.func]"); return; }
    };
    let bdf_arg = parts.next().unwrap_or("").trim();

    // One card, one driver. Nothing used to stop a second `driver wifi_ax200`
    // next to the autostarted one: both map the MMIO, both run nic_init — so
    // the newcomer resets the card and reloads its firmware UNDER the running
    // instance — both post their own RB rings, and together they need twice the
    // per-module DMA budget. The frames that come out of that read as corrupt
    // (`RX payload offset mismatch ... found nowhere`), which sends the next
    // hour of debugging after the radio instead of after the second process.
    if crate::drivers::netdev::wasm_nic_available() {
        kprintln!("[npk] a WASM network driver is already registered — refusing \
                   a second instance (it would reset the card under the running \
                   one). Stop the first, or use `wlan` to inspect it.");
        return;
    }

    // Load WASM module from npkFS
    let sys_path = alloc::format!("sys/wasm/{}", module_name);
    let resolved = resolve_path(module_name);
    let (wasm_bytes, hash) = match npkfs::fetch(&resolved) {
        Ok(v) => v,
        Err(_) => match npkfs::fetch(&sys_path) {
            Ok(v) => v,
            Err(e) => { kprintln!("[npk] Module '{}': {}", module_name, e); return; }
        }
    };

    // Find PCI device: manual BDF or auto-detect by module name
    let dev = if !bdf_arg.is_empty() {
        // Parse "bus:dev.func" format
        parse_bdf(bdf_arg).and_then(|(bus, dev, func)| {
            let addr = pci::PciAddr { bus, device: dev, function: func };
            let id = pci::read32(addr, 0x00);
            if id == 0xFFFF_FFFF || id == 0 { return None; }
            Some(pci::PciDevice {
                addr,
                vendor_id: (id & 0xFFFF) as u16,
                device_id: ((id >> 16) & 0xFFFF) as u16,
                bar0: pci::read32(addr, 0x10),
                irq_line: pci::read8(addr, 0x3C),
            })
        })
    } else {
        // Auto-detect: "wifi" -> class 02:80 (Network controller, other)
        auto_detect_device(module_name)
    };

    let dev = match dev {
        Some(d) => d,
        None => {
            kprintln!("[npk] No PCI device found for driver '{}'", module_name);
            return;
        }
    };

    // Create PCI device capability
    let a = dev.addr;
    let driver_cap = match capability::create_driver_cap(
        a.bus, a.device, a.function,
        capability::Rights::READ | capability::Rights::WRITE | capability::Rights::EXECUTE | capability::Rights::DELEGATE,
        None, // no expiry for drivers
    ) {
        Ok(id) => id,
        Err(e) => { kprintln!("[npk] Cap delegation failed: {}", e); return; }
    };

    kprint!("[npk] Driver '{}' for {:02x}:{:02x}.{} [{:04x}:{:04x}] (hash: ",
        module_name, a.bus, a.device, a.function, dev.vendor_id, dev.device_id);
    for b in &hash[..4] { kprint!("{:02x}", b); }
    kprintln!("...)");

    let term_idx = crate::shade::terminal::active_idx();
    if !wasm::spawn_on_worker(wasm_bytes.to_vec(), driver_cap, term_idx, module_name) {
        kprintln!("[npk] Failed to spawn driver '{}'", module_name);
    }
}

fn parse_bdf(s: &str) -> Option<(u8, u8, u8)> {
    // "6c:00.0" -> (0x6c, 0, 0). Hex to match lspci output.
    let mut parts = s.splitn(2, ':');
    let bus = u8::from_str_radix(parts.next()?, 16).ok()?;
    let rest = parts.next()?;
    let mut parts = rest.splitn(2, '.');
    let dev = u8::from_str_radix(parts.next()?, 16).ok()?;
    let func = u8::from_str_radix(parts.next()?, 16).ok()?;
    Some((bus, dev, func))
}

fn auto_detect_device(name: &str) -> Option<crate::drivers::pci::PciDevice> {
    use crate::drivers::pci;
    // Prefix-match so chip-specific names (wifi_ax200, wifi_rtl8852be, …) all
    // resolve to the network class without hardcoding each chip in the kernel.
    if name.starts_with("wifi") || name.starts_with("wlan") || name.starts_with("wireless") {
        // Class 02:80 = Network controller (other — WiFi)
        return pci::find_by_class(0x02, 0x80)
            .or_else(|| pci::find_by_class(0x0D, 0x80));
    }
    if name.starts_with("bluetooth") {
        // Bluetooth is often on the same device or a USB subfunction
        return pci::find_by_class(0x0D, 0x01);
    }
    match name {
        "bt" => pci::find_by_class(0x0D, 0x01),
        "gpu" | "graphics" => pci::find_by_class(0x03, 0x00),
        "audio" | "sound" => pci::find_by_class(0x04, 0x03),
        _ => None,
    }
}

