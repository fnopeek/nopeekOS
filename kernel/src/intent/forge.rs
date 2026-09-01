//! `forge` — translate a module to machine code, and say what it cost.
//!
//! The compiler runs on the device long before anything it produces is
//! executed there. That order is deliberate: translating exercises the
//! allocator, the parser and the whole generator under the kernel's own
//! `no_std` conditions, and a failure there says something quite different
//! from a failure while running. One thing at a time.

use crate::{kprint, kprintln};
use alloc::format;

/// Milliseconds since boot, from the tick counter.
fn ms() -> u64 {
    crate::interrupts::ticks() * 10
}

/// Compile the embedded modules, run them, and compare against what the same
/// compiler produced on the development machine.
///
/// The expectations in `forge_tests.rs` were not written by hand: each one was
/// measured there, by a generator whose output is checked against the
/// interpreter case by case. So this asks a sharper question than "did it
/// work" — it asks whether the device agrees with the host, down to the trap
/// codes.
/// Ein Modul von 70 Bytes, das genau eine Sache tut: einen Import rufen und
/// danach 99 zurueckgeben. Die 99 darf NIE herauskommen — der Import verlaesst
/// den Lauf ueber `forge_rt::host_trap`, und wenn das Abrollen nicht stimmt,
/// sagt es genau hier Bescheid statt spaeter in python.
///
/// Von Hand erzeugt (Typen, ein Import, ein Export "f", drei Instruktionen).
const TRAP_PROBE: &[u8] = &[
    0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01, 0x09, 0x02, 0x60,
    0x00, 0x00, 0x60, 0x01, 0x7f, 0x01, 0x7f, 0x02, 0x1c, 0x01, 0x03, 0x65,
    0x6e, 0x76, 0x14, 0x6e, 0x70, 0x6b, 0x5f, 0x66, 0x6f, 0x72, 0x67, 0x65,
    0x5f, 0x74, 0x72, 0x61, 0x70, 0x5f, 0x70, 0x72, 0x6f, 0x62, 0x65, 0x00,
    0x00, 0x03, 0x02, 0x01, 0x01, 0x07, 0x05, 0x01, 0x01, 0x66, 0x00, 0x01,
    0x0a, 0x08, 0x01, 0x06, 0x00, 0x10, 0x00, 0x41, 0x63, 0x0b,
];

extern "C" fn probe_exit(vm: *const u64) -> i32 {
    // SAFETY: `vm` ist der vmctx der laufenden Instanz, den der Generator als
    // erstes Argument uebergibt. Kehrt nicht zurueck.
    unsafe { crate::forge_rt::host_trap(vm, forge_core::trap::EXIT) }
}

struct ProbeHost;
impl crate::forge_rt::HostImports for ProbeHost {
    fn ctx_ptr(&self) -> u64 {
        // Der Stumpf fasst den Zustand nicht an; ein Zeiger waere gelogen.
        0
    }
    fn resolve(&self, module: &str, name: &str) -> Option<u64> {
        (module == "env" && name == "npk_forge_trap_probe")
            .then(|| probe_exit as *const () as u64)
    }
}

/// Kann eine Host-Funktion den Lauf beenden, statt zurueckzukehren?
///
/// Das ist der eine Mechanismus, fuer den der Host-Zwilling keine Antwort hat:
/// dort ist jeder Lauf ein eigener Prozess und `proc_exit` beendet ihn einfach.
/// Im Kernel muss abgerollt werden, und ein Assembler-Stub, der `rsp` und `rbp`
/// wiederherstellt, gehoert geprueft, bevor python davon abhaengt.
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
    // Kaeme 99 heraus, waere die Host-Funktion zurueckgekehrt statt zu traps.
    if got == 99 {
        kprintln!("[npk] forge: Trap-Probe ist ZURUECKGEKEHRT — das Abrollen hat nicht gegriffen");
        return false;
    }
    true
}

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
        // `forge python -c "..."` — derselbe Lauf, anderer Motor.
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
    if name.is_empty() {
        kprintln!("[npk] Usage: forge <module> | forge run <module> | forge python <args> | forge selftest");
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

    // Tenths, printed with the point where it belongs — the value is around
    // eight, and "84" reads like a different number entirely.
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

    // Uebersetzen ist das eine, hinauskommen das andere. Ein Import, den die
    // Bruecke nicht kennt, behaelt den Trap-Stumpf — das Modul wuerde beim
    // ersten Aufruf stehenbleiben, nicht falsch rechnen. Also hier zaehlen,
    // wo es noch niemandem weh tut.
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
