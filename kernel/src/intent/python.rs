//! `python` — run Python as a wasi guest.
//!
//! The interpreter is an ordinary signed WASM module in `sys/wasm/`; the
//! standard library is an ordinary npkFS object in `sys/python/`. Nothing
//! about Python is special-cased in the kernel: it reaches the system
//! through the same `wasi_snapshot_preview1` grant any other wasi binary
//! would get, and the only Python-shaped thing here is which two
//! directories it gets handed.

use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use crate::kprintln;
use crate::capability::{self, Rights};
use super::{home_dir, resolve_path};

/// npkFS path of the interpreter module.
const MODULE: &str = "sys/wasm/python";
/// npkFS directory holding `lib/python313.zip` + `lib/python3.13/os.py`.
const BUNDLE: &str = "sys/python";

/// Fuel for one Python run.
///
/// Interpreter startup and stdlib imports alone cost several G, so the 10 G
/// that `run` grants would stop almost any real script. 600 G leaves room
/// for real computation while still being a ceiling (on the order of a
/// minute).
const PYTHON_FUEL: u64 = 600_000_000_000;

pub fn intent_python(args: &str, vault: &'static spin::Mutex<capability::Vault>, session: capability::CapId) {
    intent_python_on(args, vault, session, crate::wasm::forge_is_default())
}

/// The same under forge. A separate entry point rather than a flag, so only
/// this one run uses the compiler.
pub fn intent_python_forge(args: &str, vault: &'static spin::Mutex<capability::Vault>, session: capability::CapId) {
    intent_python_on(args, vault, session, true)
}

fn intent_python_on(args: &str, vault: &'static spin::Mutex<capability::Vault>, session: capability::CapId, use_forge: bool) {
    let args = args.trim();

    let wasm = match crate::npkfs::fetch(MODULE) {
        Ok((b, _)) => b,
        Err(_) => {
            kprintln!("[python] not installed — 'install python' fetches the interpreter");
            return;
        }
    };
    if !crate::npkfs::exists(&alloc::format!("{}/lib", BUNDLE)) {
        kprintln!("[python] the standard library is missing under {}/lib", BUNDLE);
        kprintln!("[python] 'update' installs it as an asset");
        return;
    }

    // What the guest will see. Two grants, both named, neither implicit:
    //   /      the interpreter's own bundle, READ-ONLY
    //   /home  the user's tree, writable
    // A script outside home is refused rather than silently granted —
    // widening the grant to reach one file is how a boundary stops
    // meaning anything.
    let home = home_dir();

    let mut argv: Vec<String> = vec!["/python.wasm".to_string()];
    let mut guest_script: Option<String> = None;

    if args.is_empty() {
        // No stdin reaches a wasi guest yet, so an interactive prompt
        // would just sit there. Say so instead of hanging.
        kprintln!("[python] usage: python <file.py> [args...]   |   python -c \"code\"");
        kprintln!("[python] (no REPL yet — nothing types into a wasi guest)");
        return;
    }

    if let Some(code) = args.strip_prefix("-c ") {
        argv.push("-c".to_string());
        argv.push(code.trim().trim_matches('"').to_string());
    } else if args == "-V" || args == "--version" {
        argv.push("-V".to_string());
    } else {
        let mut it = args.split_whitespace();
        let file = it.next().unwrap_or("");
        let full = resolve_path(file);
        if !crate::npkfs::exists(&full) {
            kprintln!("[python] '{}': not found", file);
            return;
        }
        let rel = match full.strip_prefix(&alloc::format!("{}/", home)) {
            Some(r) => r,
            None => {
                kprintln!("[python] '{}' is outside {} — python only reaches your own tree", file, home);
                return;
            }
        };
        let g = alloc::format!("/home/{}", rel);
        argv.push(g.clone());
        guest_script = Some(g);
        for a in it { argv.push(a.to_string()); }
    }

    // PYTHONHOME because this interpreter was configured with the
    // default prefix (/usr/local) and would look for its stdlib there.
    // Building it with --prefix=/ would drop this line.
    let env = vec![
        "PYTHONHOME=/".to_string(),
        "PYTHONDONTWRITEBYTECODE=1".to_string(),
        "PYTHONUNBUFFERED=1".to_string(),
    ];

    let mut ctx = crate::wasi::ctx(argv, env);
    ctx.preopen(BUNDLE, "/", false);
    ctx.preopen(&home, "/home", true);

    let cap = match capability::create_module_cap(
        Rights::READ | Rights::WRITE | Rights::EXECUTE,
        Some(600_000),
    ) {
        Ok(id) => id,
        Err(e) => { kprintln!("[python] cap delegation failed: {}", e); return; }
    };
    let _ = (vault, session);

    if let Some(s) = &guest_script {
        kprintln!("[python] {}", s);
    }

    let term = crate::shade::terminal::active_idx() as u8;
    crate::heap::reset_counters();
    let t0 = crate::interrupts::ticks();
    let r = if use_forge {
        crate::wasm::execute_wasi_forge(&wasm, cap, PYTHON_FUEL, ctx, term)
    } else {
        crate::wasm::execute_wasi(&wasm, cap, PYTHON_FUEL, ctx, term)
    };
    // Timed here rather than inside the engine, so both engines are measured
    // the same way.
    let ms = crate::interrupts::ticks().saturating_sub(t0) * 10;
    let c = crate::heap::counters();
    kprintln!("[python] {} ms ({})", ms, if use_forge { "forge" } else { "wasmi" });
    // Allocator counters for this run. `steps` are free-list nodes visited,
    // which shows whether the linear search is the bottleneck.
    kprintln!("[heap] {} allocs / {} frees, Schritte: {} beim Belegen + {} beim Freigeben",
        c.allocs, c.frees, c.alloc_steps, c.free_steps);
    kprintln!("[heap] Freiliste jetzt {} Knoten, Spitze {}, {} mal gewachsen",
        c.free_nodes, c.max_free_nodes, c.grows);
    let mut line = alloc::string::String::new();
    for (i, n) in c.size_hist.iter().enumerate() {
        if *n > 0 {
            use core::fmt::Write;
            let _ = write!(line, " <{}B:{}", 32usize << i, n);
        }
    }
    kprintln!("[heap] Groessen:{}", line);
    match r {
        Ok(0) => {}
        Ok(code) => kprintln!("[python] exit {}", code),
        Err(e) => kprintln!("[python] {}", e),
    }
}
