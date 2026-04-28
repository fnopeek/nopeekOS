//! WASM intents: run, add, multiply, bootstrap

use crate::{kprint, kprintln, capability};
use super::resolve_path;

fn parse_two_ints(args: &str) -> Option<(i32, i32)> {
    let mut parts = args.trim().splitn(2, ' ');
    let a = parts.next()?.trim().parse::<i32>().ok()?;
    let b = parts.next()?.trim().parse::<i32>().ok()?;
    Some((a, b))
}

pub fn intent_wasm_add(args: &str) {
    use crate::wasm;
    let (a, b) = match parse_two_ints(args) {
        Some(v) => v,
        None => { kprintln!("[npk] Usage: add <a> <b>"); return; }
    };

    match wasm::execute(wasm::MODULE_ADD, "add", &[wasm::val_i32(a), wasm::val_i32(b)]) {
        Ok(result) => kprintln!("{}", result.output),
        Err(e) => kprintln!("[npk] WASM error: {}", e),
    }
}

pub fn intent_wasm_multiply(args: &str) {
    use crate::wasm;
    let (a, b) = match parse_two_ints(args) {
        Some(v) => v,
        None => { kprintln!("[npk] Usage: multiply <a> <b>"); return; }
    };

    match wasm::execute(wasm::MODULE_MULTIPLY, "multiply", &[wasm::val_i32(a), wasm::val_i32(b)]) {
        Ok(result) => kprintln!("{}", result.output),
        Err(e) => kprintln!("[npk] WASM error: {}", e),
    }
}

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
    let module_cap = match capability::create_module_cap(
        capability::Rights::READ
            | capability::Rights::WRITE
            | capability::Rights::EXECUTE
            | capability::Rights::RENDER,
        Some(6000), // 60 seconds at 100Hz
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

    // 1 B fuel ≈ ~1 s of busy WASM on the N100 — enough headroom for
    // benchmark-style modules (testdisk does ~10 M instr per phase) but
    // still finite, so a buggy infinite loop bails instead of locking
    // the worker. The default 10 M is too tight for any module that
    // does real I/O in a loop.
    match wasm::execute_sandboxed_with_fuel(
        &wasm_bytes, func_name, &args_vec, module_cap, 1_000_000_000,
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

    let module_cap = match capability::create_module_cap(
        capability::Rights::READ
            | capability::Rights::WRITE
            | capability::Rights::EXECUTE
            | capability::Rights::RENDER,
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

    let module_cap = match capability::create_module_cap(
        capability::Rights::READ
            | capability::Rights::WRITE
            | capability::Rights::EXECUTE
            | capability::Rights::RENDER,
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
    if !wasm::spawn_on_worker(wasm_bytes.to_vec(), module_cap, term_idx, module_name) {
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
    // "01:00.0" -> (1, 0, 0)
    let mut parts = s.splitn(2, ':');
    let bus: u8 = parts.next()?.parse().ok()?;
    let rest = parts.next()?;
    let mut parts = rest.splitn(2, '.');
    let dev: u8 = parts.next()?.parse().ok()?;
    let func: u8 = parts.next()?.parse().ok()?;
    Some((bus, dev, func))
}

fn auto_detect_device(name: &str) -> Option<crate::drivers::pci::PciDevice> {
    use crate::drivers::pci;
    match name {
        "wifi" | "wlan" | "wireless" => {
            // Class 02:80 = Network controller (other — WiFi)
            pci::find_by_class(0x02, 0x80)
                .or_else(|| pci::find_by_class(0x0D, 0x80))
        }
        "bluetooth" | "bt" => {
            // Bluetooth is often on the same device or a USB subfunction
            pci::find_by_class(0x0D, 0x01)
        }
        "gpu" | "graphics" => {
            pci::find_by_class(0x03, 0x00)
        }
        "audio" | "sound" => {
            pci::find_by_class(0x04, 0x03)
        }
        _ => None,
    }
}

/// Store built-in WASM modules to npkFS on first boot.
pub fn bootstrap_wasm() {
    use crate::{wasm, npkfs};

    if !npkfs::is_mounted() { return; }

    let modules: &[(&str, &[u8])] = &[
        ("sys/wasm/hello", wasm::MODULE_HELLO),
        ("sys/wasm/fib", wasm::MODULE_FIB),
        ("sys/wasm/add", wasm::MODULE_ADD),
        ("sys/wasm/multiply", wasm::MODULE_MULTIPLY),
    ];

    let mut stored = 0;
    for (name, data) in modules {
        if npkfs::fetch(name).is_err() {
            if npkfs::store(name, data, capability::CAP_NULL).is_ok() {
                stored += 1;
            }
        }
    }
    if stored > 0 {
        kprintln!("[npk] Bootstrap: stored {} WASM modules", stored);
    }
}
