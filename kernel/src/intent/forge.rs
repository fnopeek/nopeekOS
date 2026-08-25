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
    if bad == 0 {
        kprintln!("[npk] forge: Wachseite, Tabelle, Division, unreachable und Fuel");
        kprintln!("[npk] forge: melden sich am Geraet mit demselben Grund.");
    }
}

pub fn intent_forge(args: &str) {
    use crate::npkfs;

    let name = args.trim();
    if name == "selftest" || name == "test" {
        selftest();
        return;
    }
    if name.is_empty() {
        kprintln!("[npk] Usage: forge <module> | forge selftest");
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

    kprintln!(
        "[npk] forge: {}/{} Funktionen, {} Instruktionen -> {} B x86 ({} B je Instr)",
        done,
        total,
        instrs,
        m.code.len(),
        if instrs > 0 { m.code.len() as u64 * 10 / instrs } else { 0 }
    );
    kprint!("[npk] forge: {} ms", dt);
    if instrs > 0 && dt > 0 {
        kprint!(" ({} K Instruktionen/s)", instrs / dt);
    }
    kprintln!("");

    if let Some(why) = first_refusal {
        kprintln!("[npk] forge: {} Funktionen abgelehnt, erste: {}", total - done, why);
    }
}
