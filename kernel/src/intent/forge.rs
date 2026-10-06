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
    crate::interrupts::ticks() * 10
}

/// A 70-byte module that calls one import and then returns 99. The 99 must
/// never come out: the import leaves the run via `forge_rt::host_trap`, so
/// a broken unwind shows up here rather than later in python.
///
/// Hand-assembled (types, one import, export "f", three instructions).
const TRAP_PROBE: &[u8] = &[
    0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01, 0x09, 0x02, 0x60,
    0x00, 0x00, 0x60, 0x01, 0x7f, 0x01, 0x7f, 0x02, 0x1c, 0x01, 0x03, 0x65,
    0x6e, 0x76, 0x14, 0x6e, 0x70, 0x6b, 0x5f, 0x66, 0x6f, 0x72, 0x67, 0x65,
    0x5f, 0x74, 0x72, 0x61, 0x70, 0x5f, 0x70, 0x72, 0x6f, 0x62, 0x65, 0x00,
    0x00, 0x03, 0x02, 0x01, 0x01, 0x07, 0x05, 0x01, 0x01, 0x66, 0x00, 0x01,
    0x0a, 0x08, 0x01, 0x06, 0x00, 0x10, 0x00, 0x41, 0x63, 0x0b,
];

extern "C" fn probe_exit(vm: *const u64) -> i32 {
    // SAFETY: `vm` is the running instance's vmctx, passed by the generator
    // as the first argument. Does not return.
    unsafe { crate::forge_rt::host_trap(vm, forge_core::trap::EXIT) }
}

struct ProbeHost;
impl crate::forge_rt::HostImports for ProbeHost {
    fn ctx_ptr(&self) -> u64 {
        // The stub never touches host state, so there is no pointer to give.
        0
    }
    fn resolve(&self, module: &str, name: &str) -> Option<(u64, crate::forge_rt::HostSig)> {
        let sig = crate::forge_rt::HostSig { params: &[], result: None };
        (module == "env" && name == "npk_forge_trap_probe")
            .then(|| (probe_exit as *const () as u64, sig))
    }
}

/// Can a host function end the run instead of returning?
///
/// The host harness cannot test this: there each run is its own process and
/// `proc_exit` just ends it. In the kernel the run must be unwound by an
/// assembly stub that restores `rsp` and `rbp`.
fn trap_probe() -> bool {
    use crate::forge_rt::Instance;
    let Ok(m) = forge_core::compile(TRAP_PROBE) else {
        kprintln!("[npk] forge: Trap-Probe liess sich nicht uebersetzen");
        return false;
    };
    let entry = m.plan.exports.iter().find(|(n, _)| n == "f").map(|(_, i)| *i)
        .and_then(|i| m.offset_of(i));
    let Some(off) = entry else {
        kprintln!("[npk] forge: Trap-Probe hat kein f");
        return false;
    };
    let Some(mut inst) = Instance::new_with_host(&m, &ProbeHost) else {
        kprintln!("[npk] forge: Trap-Probe — Instanz liess sich nicht bauen");
        return false;
    };
    if inst.unresolved_imports() != 0 {
        kprintln!("[npk] forge: Trap-Probe — Import nicht aufgeloest");
        return false;
    }
    inst.set_fuel(i64::MAX / 4);
    let (got, trap) = inst.call(off, 0, 0, 0);
    if trap != forge_core::trap::EXIT {
        kprintln!("[npk] forge: Trap-Probe -> {} statt EXIT (Ergebnis {})",
            forge_core::trap::name(trap), got);
        return false;
    }
    // 99 means the host function returned instead of trapping.
    if got == 99 {
        kprintln!("[npk] forge: Trap-Probe ist ZURUECKGEKEHRT — das Abrollen hat nicht gegriffen");
        return false;
    }
    true
}

/// Compile the embedded modules, run them, and compare against what the same
/// compiler produced on the host. The expectations in `forge_tests.rs` are
/// generated there and checked against the interpreter, so this asks whether
/// kernel and host agree down to the trap codes.
fn selftest() {
    use crate::forge_rt::Instance;
    use crate::forge_tests::CASES;

    let (mut ok, mut bad) = (0u32, 0u32);
    for c in CASES {
        let m = match forge_core::compile(c.wasm) {
            Ok(m) => m,
            Err(e) => {
                kprintln!("[npk] forge: {} — uebersetzen: {}", c.name, e);
                bad += 1;
                continue;
            }
        };
        let Some(fidx) = m.plan.exports.iter().find(|(n, _)| n == "f").map(|(_, i)| *i) else {
            kprintln!("[npk] forge: {} — kein Export f", c.name);
            bad += 1;
            continue;
        };
        let Some(off) = m.offset_of(fidx) else {
            kprintln!("[npk] forge: {} — keine Adresse", c.name);
            bad += 1;
            continue;
        };
        let Some(mut inst) = Instance::new(&m) else {
            kprintln!("[npk] forge: {} — Instanz liess sich nicht bauen", c.name);
            bad += 1;
            continue;
        };
        inst.set_fuel(if c.fuel < 0 { i64::MAX / 4 } else { c.fuel });

        let (got, trap) = inst.call(off, c.arg, 0, 0);
        if got == c.want && trap == c.trap {
            ok += 1;
        } else {
            bad += 1;
            kprintln!(
                "[npk] forge: {} (arg {}) -> {} / {} statt {} / {}",
                c.name, c.arg, got, forge_core::trap::name(trap), c.want,
                forge_core::trap::name(c.trap)
            );
        }
    }

    kprintln!("[npk] forge selftest: {}/{} wie auf dem Host", ok, ok + bad);
    kprintln!("[npk] forge: Host-Trap (Abrollen aus einer Host-Funktion): {}",
        if trap_probe() { "geht" } else { "GESCHEITERT" });
    if bad == 0 {
        kprintln!("[npk] forge: Wachseite, Tabelle, Division, unreachable und Fuel");
        kprintln!("[npk] forge: melden sich am Geraet mit demselben Grund.");
    }
}

pub fn intent_forge(args: &str, vault: &'static spin::Mutex<crate::security::capability::Vault>, session: crate::security::capability::CapId) {
    use crate::npkfs;

    let name = args.trim();
    if name == "selftest" || name == "test" {
        selftest();
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
    // path (`dock`, `bar`, `audio_hda`, `wifid`) can only run under forge
    // this way.
    if let Some(rest) = name.strip_prefix("default") {
        let arg = rest.trim();
        match arg {
            "on" | "forge" => {
                crate::wasm::set_engine_default(true);
                kprintln!("[npk] forge ist ab dem naechsten Start der Vorgabemotor");
                kprintln!("[npk] (laufende Module bleiben auf dem Motor, mit dem sie gestartet sind)");
            }
            "off" | "wasmi" => {
                crate::wasm::set_engine_default(false);
                kprintln!("[npk] wasmi ist ab dem naechsten Start der Vorgabemotor");
            }
            "" => kprintln!("[npk] Vorgabemotor: {}",
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
            kprintln!("[npk] forge: abgelehnt — {}", e);
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
        "[npk] forge: {}/{} Funktionen, {} Instruktionen -> {} B x86 ({}.{} B je Instr)",
        done,
        total,
        instrs,
        m.code.len(),
        tenths / 10,
        tenths % 10
    );
    kprint!("[npk] forge: {} ms", dt);
    if instrs > 0 && dt > 0 {
        kprint!(" ({} K Instruktionen/s)", instrs / dt);
    }
    kprintln!("");

    if let Some(why) = first_refusal {
        kprintln!("[npk] forge: {} Funktionen abgelehnt, erste: {}", total - done, why);
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
        kprintln!("[npk] forge: {}/{} Importe aufgeloest", resolved, imports);
        if let Some((mo, na)) = first_missing {
            kprintln!("[npk] forge: erster offener Import: {}::{}", mo, na);
        }
    }
}
