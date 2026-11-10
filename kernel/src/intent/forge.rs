//! `forge` — translate a module to machine code, and say what it cost.
//!
//! Translating alone exercises the allocator, the parser and the whole
//! generator under the kernel's `no_std` conditions, separately from
//! running the result: a compile failure and a run failure mean different
//! things.

use crate::{kprint, kprintln};
use alloc::format;

/// Milliseconds since boot, from the tick counter.
fn ms() -> u64 {
    crate::time::ticks() * 10
}

pub fn intent_forge(args: &str, vault: &'static spin::Mutex<crate::security::capability::Vault>, session: crate::security::capability::CapId) {
    use crate::npkfs;

    let name = args.trim();
    if name == "selftest" || name == "test" {
        crate::forge_check::selftest();
        return;
    }
    if let Some(rest) = args.trim_start().strip_prefix("python") {
        // `forge python -c "..."`: the same run on the forge engine.
        super::python::intent_python_forge(rest.trim_start(), vault, session);
        return;
    }
    if let Some(rest) = name.strip_prefix("run ") {
        let m = rest.trim();
        if m.is_empty() {
            kprintln!("[npk] Usage: forge run <module>");
            return;
        }
        super::wasm::intent_run_interactive_forge(m);
        return;
    }
    // Persistent engine default. Modules started by autostart or the driver
    // path (`deck`, `audio_hda`, `wifid`) can only run under forge
    // this way.
    if let Some(rest) = name.strip_prefix("default") {
        let arg = rest.trim();
        match arg {
            "on" | "forge" => {
                crate::wasm::set_engine_default(true);
                kprintln!("[npk] forge is the default engine from the next start");
                kprintln!("[npk] (running modules stay on the engine they started with)");
            }
            "off" | "wasmi" => {
                crate::wasm::set_engine_default(false);
                kprintln!("[npk] wasmi is the default engine from the next start");
            }
            "" => kprintln!("[npk] default engine: {}",
                if crate::wasm::forge_is_default() { "forge" } else { "wasmi" }),
            _ => kprintln!("[npk] Usage: forge default [on|off]"),
        }
        return;
    }
    if name.is_empty() {
        kprintln!("[npk] Usage: forge <module> | forge run <module> | forge python <args>");
        kprintln!("[npk]        forge default [on|off] | forge selftest");
        return;
    }

    let sys_path = format!("sys/wasm/{}", name);
    let (wasm, _hash) = match npkfs::fetch(&super::resolve_path(name)) {
        Ok(v) => v,
        Err(_) => match npkfs::fetch(&sys_path) {
            Ok(v) => v,
            Err(e) => {
                kprintln!("[npk] Module '{}': {}", name, e);
                return;
            }
        },
    };

    kprintln!("[npk] forge: {} ({} B wasm)", name, wasm.len());

    let t0 = ms();
    let m = match forge_core::compile(&wasm) {
        Ok(m) => m,
        Err(e) => {
            kprintln!("[npk] forge: rejected: {}", e);
            return;
        }
    };
    let dt = ms().saturating_sub(t0);

    let total = m.funcs.len();
    let mut done = 0usize;
    let mut first_refusal: Option<&'static str> = None;
    for o in &m.funcs {
        match o {
            forge_core::codegen::Outcome::Done(_) => done += 1,
            forge_core::codegen::Outcome::Unsupported(why) => {
                if first_refusal.is_none() {
                    first_refusal = Some(why);
                }
            }
        }
    }
    let instrs: u64 = m.plan.total_instrs();

    // Tenths, printed with a decimal point: "8.4" rather than "84".
    let tenths = if instrs > 0 { m.code.len() as u64 * 10 / instrs } else { 0 };
    kprintln!(
        "[npk] forge: {}/{} functions, {} instructions -> {} B x86 ({}.{} B per instr)",
        done,
        total,
        instrs,
        m.code.len(),
        tenths / 10,
        tenths % 10
    );
    kprint!("[npk] forge: {} ms", dt);
    if instrs > 0 && dt > 0 {
        kprint!(" ({} K instructions/s)", instrs / dt);
    }
    kprintln!("");

    if let Some(why) = first_refusal {
        kprintln!("[npk] forge: {} functions rejected, first: {}", total - done, why);
    }

    // An import the glue cannot resolve keeps the trap stub: the module would
    // stop at its first call. Count them here, before anything runs.
    let imports = m.plan.imported_funcs.len();
    let mut resolved = 0usize;
    let mut first_missing: Option<(&str, &str)> = None;
    for (module, name) in &m.plan.imported_funcs {
        if crate::wasm::forge_glue::resolve(module, name).is_some() {
            resolved += 1;
        } else if first_missing.is_none() {
            first_missing = Some((module, name));
        }
    }
    if imports > 0 {
        kprintln!("[npk] forge: {}/{} imports resolved", resolved, imports);
        if let Some((mo, na)) = first_missing {
            kprintln!("[npk] forge: first open import: {}::{}", mo, na);
        }
    }
}
