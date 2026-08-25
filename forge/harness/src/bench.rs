//! Run a real module under forge and under wasmi, and time both.
//!
//! Coverage is not speed. Everything up to here proved the generated code
//! computes the same thing the interpreter does; this is where it says whether
//! it does so faster, on the module whose slowness started the whole thing.
//!
//! `beakbench.wasm` is the vehicle because it is beak's own engine with a
//! single import, and because its warm layout has a known fuel count — if that
//! count does not appear, the two sides are not doing the same work.

use crate::selftest::{Exec, Inst};
use std::time::Instant;

fn ms(d: std::time::Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// `bench_log(ptr, len)` — the module's one import. It reaches into linear
/// memory through the instance context, exactly as a kernel host function
/// would.
extern "C" fn h_bench_log(ctx: *const u64, ptr: u32, len: u32) {
    use forge_core::vmctx as v;
    // SAFETY: `ctx` is the instance context of the module doing the call, and
    // the range is checked against the memory's own recorded size.
    unsafe {
        let base = *ctx.add(v::MEM_BASE as usize / 8) as *const u8;
        let size = *ctx.add(v::MEM_SIZE as usize / 8) as usize;
        let (s, e) = (ptr as usize, ptr as usize + len as usize);
        if e <= size {
            let bytes = std::slice::from_raw_parts(base.add(s), len as usize);
            eprintln!("  wasm: {}", String::from_utf8_lossy(bytes));
        }
    }
}

pub fn host_for(module: &str, name: &str) -> Option<u64> {
    match (module, name) {
        ("env", "bench_log") => Some(h_bench_log as usize as u64),
        _ => None,
    }
}

struct Phases {
    fuel: u64,
    compile: f64,
    init: f64,
    parse: (f64, u32),
    cascade: (f64, u32),
    layout: (f64, u32),
    layout_warm: f64,
}

fn under_forge(wasm: &[u8], page: u32, width: u32, vh: u32, reps: u32) -> Option<Phases> {
    let t = Instant::now();
    let m = forge_core::compile(wasm).ok()?;
    let compile = ms(t.elapsed());

    let refused = m
        .funcs
        .iter()
        .filter(|o| matches!(o, forge_core::codegen::Outcome::Unsupported(_)))
        .count();
    if refused > 0 {
        eprintln!("  forge: {refused} Funktionen nicht uebersetzt — Lauf waere unehrlich");
        return None;
    }

    let exec = Exec::new(&m.code)?;
    let inst = Inst::new(&m, exec.base())?;
    let at = |name: &str| -> Option<usize> {
        let (_, i) = *m.plan.exports.iter().find(|(n, _)| n == name)?;
        m.offset_of(i)
    };
    let (f_init, f_parse, f_casc, f_lay) = (
        at("init")?,
        at("phase_parse")?,
        at("phase_cascade")?,
        at("phase_layout")?,
    );

    let t = Instant::now();
    exec.call_entry(m.entry_offset, f_init, inst.ptr(), page, vh, 0);
    let init = ms(t.elapsed());

    let (mut parse, mut cascade, mut layout) = ((0.0, 0), (0.0, 0), (0.0, 0));
    let (mut warm, mut fuel_used) = (0.0, 0u64);
    for i in 0..reps {
        let t = Instant::now();
        let n = exec.call_entry(m.entry_offset, f_parse, inst.ptr(), 0, 0, 0);
        parse = (ms(t.elapsed()), n);

        let t = Instant::now();
        let r = exec.call_entry(m.entry_offset, f_casc, inst.ptr(), width, 0, 0);
        cascade = (ms(t.elapsed()), r);

        let f0 = inst.fuel_left();
        let t = Instant::now();
        let h = exec.call_entry(m.entry_offset, f_lay, inst.ptr(), width, 0, 0);
        layout = (ms(t.elapsed()), h);
        if i > 0 || reps == 1 {
            warm = layout.0;
            fuel_used = f0.saturating_sub(inst.fuel_left()) as u64;
        }
    }
    Some(Phases {
        fuel: fuel_used,
        compile,
        init,
        parse,
        cascade,
        layout,
        layout_warm: warm,
    })
}

fn under_wasmi(wasm: &[u8], page: u32, width: u32, vh: u32, reps: u32) -> Option<(Phases, u64)> {
    use wasmi::{Caller, Config, Engine, Extern, Linker, Module, Store};
    let mut cfg = Config::default();
    cfg.consume_fuel(true);
    let engine = Engine::new(&cfg);

    let t = Instant::now();
    let module = Module::new(&engine, wasm).ok()?;
    let compile = ms(t.elapsed());

    let mut store = Store::new(&engine, ());
    store.set_fuel(u64::MAX / 4).ok()?;
    let mut linker = <Linker<()>>::new(&engine);
    linker
        .func_wrap(
            "env",
            "bench_log",
            |mut caller: Caller<'_, ()>, ptr: i32, len: i32| {
                if let Some(Extern::Memory(mem)) = caller.get_export("memory") {
                    let d = mem.data(&caller);
                    let (s, e) = (ptr as usize, (ptr + len) as usize);
                    if e <= d.len() {
                        eprintln!("  wasm: {}", String::from_utf8_lossy(&d[s..e]));
                    }
                }
            },
        )
        .ok()?;
    linker
        .func_wrap("env", "fuel_now", |caller: Caller<'_, ()>| -> u64 {
            caller.get_fuel().unwrap_or(0)
        })
        .ok()?;
    let instance = linker.instantiate_and_start(&mut store, &module).ok()?;

    let f_init = instance
        .get_typed_func::<(u32, u32), ()>(&store, "init")
        .ok()?;
    let f_parse = instance
        .get_typed_func::<(), u32>(&store, "phase_parse")
        .ok()?;
    let f_casc = instance
        .get_typed_func::<u32, u32>(&store, "phase_cascade")
        .ok()?;
    let f_lay = instance
        .get_typed_func::<u32, u32>(&store, "phase_layout")
        .ok()?;

    let t = Instant::now();
    f_init.call(&mut store, (page, vh)).ok()?;
    let init = ms(t.elapsed());

    let (mut parse, mut cascade, mut layout) = ((0.0, 0), (0.0, 0), (0.0, 0));
    let (mut warm, mut fuel) = (0.0, 0u64);
    for i in 0..reps {
        let t = Instant::now();
        let n = f_parse.call(&mut store, ()).ok()?;
        parse = (ms(t.elapsed()), n);

        let t = Instant::now();
        let r = f_casc.call(&mut store, width).ok()?;
        cascade = (ms(t.elapsed()), r);

        let f0 = store.get_fuel().unwrap_or(0);
        let t = Instant::now();
        let h = f_lay.call(&mut store, width).ok()?;
        layout = (ms(t.elapsed()), h);
        if i > 0 || reps == 1 {
            warm = layout.0;
            fuel = f0.saturating_sub(store.get_fuel().unwrap_or(0));
        }
    }
    Some((
        Phases {
            fuel,
            compile,
            init,
            parse,
            cascade,
            layout,
            layout_warm: warm,
        },
        fuel,
    ))
}

pub fn run(args: &[String]) -> bool {
    let path = &args[0];
    let num = |i: usize, d: u32| args.get(i).and_then(|s| s.parse().ok()).unwrap_or(d);
    let (page, width, vh, reps) = (num(1, 0), num(2, 1400), num(3, 1000), num(4, 3));

    let Ok(wasm) = std::fs::read(path) else {
        println!("{path}: nicht lesbar");
        return false;
    };
    println!("{path}  Seite {page}  {width}x{vh}  {reps} Wiederholungen\n");

    let Some((w, _fuel)) = under_wasmi(&wasm, page, width, vh, reps) else {
        println!("wasmi konnte nicht laufen");
        return false;
    };
    let Some(f) = under_forge(&wasm, page, width, vh, reps) else {
        println!("forge konnte nicht laufen");
        return false;
    };

    // The results must agree before any timing is worth reading.
    let same = w.parse.1 == f.parse.1 && w.cascade.1 == f.cascade.1 && w.layout.1 == f.layout.1;
    println!(
        "Ergebnisse: parse {} el, cascade {} Regeln, layout h={}  ->  {}",
        w.parse.1,
        w.cascade.1,
        w.layout.1,
        if same {
            "GLEICH".to_string()
        } else {
            format!(
                "ABWEICHEND (forge: {} / {} / {})",
                f.parse.1, f.cascade.1, f.layout.1
            )
        }
    );
    println!(
        "Fuel des warmen Layouts: wasmi {} · forge {} ({:+.2} %)\n",
        w.fuel,
        f.fuel,
        100.0 * (f.fuel as f64 - w.fuel as f64) / w.fuel as f64
    );

    println!("{:<16} {:>12} {:>12} {:>10}", "", "wasmi", "forge", "Faktor");
    println!("{}", "-".repeat(54));
    let row = |name: &str, a: f64, b: f64| {
        println!(
            "{:<16} {:>9.1} ms {:>9.1} ms {:>9.2}x",
            name,
            a,
            b,
            if b > 0.0 { a / b } else { 0.0 }
        );
    };
    row("uebersetzen", w.compile, f.compile);
    row("init", w.init, f.init);
    row("parse", w.parse.0, f.parse.0);
    row("cascade", w.cascade.0, f.cascade.0);
    row("layout warm", w.layout_warm, f.layout_warm);
    same
}
