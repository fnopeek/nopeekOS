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

    // Delegate a capability for this module: READ + EXECUTE, 60s TTL
    let module_cap = match capability::create_module_cap(
        capability::Rights::READ | capability::Rights::EXECUTE,
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

    match wasm::execute_sandboxed(&wasm_bytes, func_name, &args_vec, module_cap) {
        Ok(result) => {
            if !result.output.is_empty() {
                kprintln!("{}", result.output);
            }
        }
        Err(e) => kprintln!("[npk] Execution error: {}", e),
    }
}

/// Run a WASM module on a worker core with its own window.
/// Returns immediately — the module runs in background.
/// Used for apps like `top` that update the screen continuously.
pub fn intent_run_interactive(module_name: &str) {
    use crate::{wasm, npkfs, capability};

    // Load module from npkFS
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
        capability::Rights::READ | capability::Rights::EXECUTE,
        Some(600_000), // 100 minutes at 100Hz
    ) {
        Ok(id) => id,
        Err(e) => { kprintln!("[npk] Cap delegation failed: {}", e); return; }
    };

    // Create a new window for this app
    let terminal_idx = crate::shade::with_compositor(|comp| {
        let wid = comp.create_window(module_name, 0, 0, 800, 600);
        comp.windows.iter().find(|w| w.id == wid).map(|w| w.terminal_idx)
    }).flatten();

    let term_idx = match terminal_idx {
        Some(idx) => idx as u8,
        None => {
            kprintln!("[npk] Failed to create window for '{}'", module_name);
            return;
        }
    };

    kprint!("[npk] Running '{}' on worker core (hash: ", module_name);
    for b in &hash[..4] { kprint!("{:02x}", b); }
    kprintln!("..., window={}))", term_idx);

    // Spawn on worker core — returns immediately
    if !wasm::spawn_on_worker(wasm_bytes.to_vec(), module_cap, term_idx) {
        kprintln!("[npk] Failed to spawn '{}' on worker core", module_name);
    }

    // Render the new window layout
    crate::shade::render_frame();
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
