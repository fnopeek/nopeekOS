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
