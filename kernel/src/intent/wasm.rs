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

    // Delegate the standard caps (READ + WRITE + EXECUTE + RENDER). Trust
    // comes from: (a) the module is ECDSA-P-384-signed and verified at
    // install time, (b) the user explicitly typed `run`, (c) the sandbox
    // bounds memory, fuel and host-fn surface. AUDIT stays off: apps should
    // not introspect kernel state.
    // TTL 600_000 ticks ≈ 100 minutes at 100 Hz: instantiation and
    // first-touch of large heaps can take tens of seconds before the first
    // host call, and the bound still reaps a hung worker.
    // The fixed standard set plus what the module declares in `.npk.caps`,
    // the same as the click path (`npk_spawn_module`). A union, not a
    // replacement, so no module loses a right it relied on here. Open: the
    // fixed set grants WRITE to modules that never declared it.
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

    // Non-numeric arguments become the launch argument, the same string
    // `npk_open` hands an app and it reads with `npk_launch_arg`
    // (`beak https://…`). Numeric arguments keep calling the export of the
    // same name with i32 values (`<module> 3 4`).
    let launch_arg = if args_vec.is_empty() && !arg_str.trim().is_empty() {
        Some(alloc::string::String::from(arg_str.trim()))
    } else {
        None
    };

    // Fuel for the blocking path below: 10 B. wasmi charges ~1 fuel per
    // instruction, and bulk memory ops on 100+ MB buffers burn hundreds of
    // millions in one call; 10 B covers that without making infinite loops
    // free.
    // A window app belongs on a worker core, not in the shell loop. It is
    // recognised by declaring RENDER itself (the fixed set grants RENDER to
    // everyone). A blocking run would also have `pid: 0`, and
    // `fetch::begin_one` refuses async fetches without a process.
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

    // The fixed standard set plus what the module declares in `.npk.caps`,
    // the same as the click path (`npk_spawn_module`). A union, not a
    // replacement, so no module loses a right it relied on here. Open: the
    // fixed set grants WRITE to modules that never declared it.
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

/// The same under forge. A separate entry point rather than a global flag,
/// so only this one module runs on the forge engine.
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

    // The fixed standard set plus what the module declares in `.npk.caps`,
    // the same as the click path (`npk_spawn_module`). A union, not a
    // replacement, so no module loses a right it relied on here. Open: the
    // fixed set grants WRITE to modules that never declared it.
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

    // One card, one driver. A second instance would map the same MMIO, reset
    // the card and reload its firmware under the running one, post its own
    // RX rings and double the DMA budget; the result looks like corrupt RX
    // frames rather than a duplicate driver.
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

    // A capability for this PCI device, plus what the module declares in
    // `.npk.caps` (as on the `run` path): without that a driver started here
    // lacks HARDWARE for MMIO by address, GSI interrupts and the audio mixer.
    let a = dev.addr;
    let driver_cap = match capability::create_driver_cap(
        a.bus, a.device, a.function,
        capability::Rights::READ | capability::Rights::WRITE | capability::Rights::EXECUTE
            | capability::Rights::DELEGATE
            | capability::widget_rights_from_wasm(&wasm_bytes),
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

