//! Host harness for forge. Runs the same `no_std` code the kernel will run,
//! on the real modules, and reports what it found.
//!
//!   forge_harness <file.wasm>...              census per module
//!   forge_harness --roadmap <file.wasm>       how far are we, and what next
//!   forge_harness --selftest                  generated code vs wasmi
//!   forge_harness --run <wasm> [page w h n]   run it, and time it

mod bench;
mod faults;
mod wasi_core;
mod pybench;
mod wasi_glue;
mod selftest;

use std::collections::{BTreeMap, BTreeSet};

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    // Run one module and print "<result> <trap>". Used by `gentests` to
    // produce the device check's expectations by measurement rather than by
    // hand — and it runs in its own process, so a trap that is meant to fault
    // cannot take the generator down with it.
    if args.first().map(|a| a == "--oneshot").unwrap_or(false) {
        let bytes: Vec<u8> = args[1]
            .as_bytes()
            .chunks(2)
            .map(|c| u8::from_str_radix(std::str::from_utf8(c).unwrap(), 16).unwrap())
            .collect();
        let arg: u32 = args[2].parse().unwrap();
        let fuel: i64 = args[3].parse().unwrap();
        let (r, t) = selftest::oneshot(&bytes, arg, if fuel < 0 { None } else { Some(fuel) })
            .unwrap_or((0, u32::MAX));
        println!("{r} {t}");
        return;
    }
    if args.first().map(|a| a == "--python").unwrap_or(false) {
        args.remove(0);
        std::process::exit(if pybench::run(&args) { 0 } else { 1 });
    }
    if args.first().map(|a| a == "--run").unwrap_or(false) {
        args.remove(0);
        std::process::exit(if bench::run(&args) { 0 } else { 1 });
    }
    if args.first().map(|a| a == "--selftest").unwrap_or(false) {
        std::process::exit(if selftest::run() { 0 } else { 1 });
    }
    let roadmap = args.first().map(|a| a == "--roadmap").unwrap_or(false);
    if roadmap {
        args.remove(0);
    }
    if args.is_empty() {
        eprintln!("usage: forge_harness [--selftest | --roadmap] <file.wasm>...");
        std::process::exit(2);
    }
    if roadmap {
        for f in &args {
            report_roadmap(f);
        }
    } else {
        census(&args);
    }
}

fn read_plan(f: &str) -> Option<forge_core::ModulePlan> {
    let bytes = match std::fs::read(f) {
        Ok(b) => b,
        Err(e) => {
            println!("{f}: lesen: {e}");
            return None;
        }
    };
    match forge_core::plan(&bytes) {
        Ok(p) => Some(p),
        Err(e) => {
            println!("{f}: ABGELEHNT: {e}");
            None
        }
    }
}

fn census(files: &[String]) {
    println!(
        "{:<12} {:>7} {:>7} {:>10} {:>7} {:>8} {:>7} {:>7} {:>6}",
        "Modul", "Fn", "Import", "Instr", "Spanne", "Stapel", "Aufruf", "indir.", "Mem"
    );
    println!("{}", "-".repeat(82));

    let (mut ok, mut bad) = (0u32, 0u32);
    let (mut instrs, mut flushes) = (0u64, 0u64);
    let mut big = 0u64;

    for f in files {
        let name = std::path::Path::new(f)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| f.clone());
        let Some(p) = read_plan(f) else {
            bad += 1;
            continue;
        };
        ok += 1;
        instrs += p.total_instrs();
        flushes += p.total_flushes();
        let span = if p.total_flushes() > 0 {
            p.total_instrs() as f64 / p.total_flushes() as f64
        } else {
            0.0
        };
        println!(
            "{:<12} {:>7} {:>7} {:>10} {:>7.1} {:>8} {:>7} {:>7} {:>6}",
            name,
            p.funcs.len(),
            p.imported_funcs.len(),
            p.total_instrs(),
            span,
            p.max_stack(),
            p.bodies.iter().map(|b| b.calls as u64).sum::<u64>(),
            p.bodies.iter().map(|b| b.indirect as u64).sum::<u64>(),
            p.bodies.iter().map(|b| b.mem_ops as u64).sum::<u64>(),
        );
        big += p.bodies.iter().map(|b| b.big_offsets as u64).sum::<u64>();
    }

    println!("{}", "-".repeat(82));
    println!(
        "{ok} angenommen, {bad} abgelehnt · {instrs} Instruktionen, {flushes} Spannen \
         ({:.1} Instr je Fuel-Abzug) · {big} Speicheroffsets ueber 2 GiB",
        if flushes > 0 { instrs as f64 / flushes as f64 } else { 0.0 }
    );
}

/// A function is translatable exactly when every opcode it uses is one the
/// generator emits. Partial credit does not exist — so the honest question is
/// not "how many opcodes are done" but "how many FUNCTIONS does the next
/// opcode unlock". Greedy answers it, and the answer is the work order.
///
/// Kept incremental: each function carries a count of the opcodes it still
/// misses, and an opcode's gain is the number of its functions sitting at
/// exactly one. Otherwise python's 9939 x 147 turns into minutes.
fn report_roadmap(f: &str) {
    let name = std::path::Path::new(f)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| f.to_string());
    let bytes = match std::fs::read(f) {
        Ok(b) => b,
        Err(e) => {
            println!("{f}: lesen: {e}");
            return;
        }
    };
    // The headline comes from the generator, not from a list of opcode names:
    // a function counts only when `compile()` actually produced code for it.
    let t0 = std::time::Instant::now();
    let m = match forge_core::compile(&bytes) {
        Ok(m) => m,
        Err(e) => {
            println!("{f}: ABGELEHNT: {e}");
            return;
        }
    };
    let compile_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let p = &m.plan;
    let (mut done_fn, mut done_instr) = (0usize, 0u64);
    let mut blocked: BTreeMap<&str, usize> = BTreeMap::new();
    for (o, b) in m.funcs.iter().zip(p.bodies.iter()) {
        match o {
            forge_core::codegen::Outcome::Done(_) => {
                done_fn += 1;
                done_instr += b.instrs as u64;
            }
            forge_core::codegen::Outcome::Unsupported(name) => {
                *blocked.entry(name).or_default() += 1;
            }
        }
    }

    let all_ops: Vec<&'static str> = {
        let s: BTreeSet<&'static str> = p.bodies.iter().flat_map(|b| b.ops.iter().copied()).collect();
        s.into_iter().collect()
    };
    let idx: BTreeMap<&str, usize> = all_ops.iter().enumerate().map(|(i, o)| (*o, i)).collect();

    // funcs[i] = (opcode indices, instruction count)
    let funcs: Vec<(Vec<usize>, u32)> = p
        .bodies
        .iter()
        .map(|b| (b.ops.iter().map(|o| idx[o]).collect::<Vec<_>>(), b.instrs))
        .collect();
    let total_instrs: u64 = funcs.iter().map(|(_, n)| *n as u64).sum();

    // Inverted index: which functions use each opcode.
    let mut users: Vec<Vec<usize>> = vec![Vec::new(); all_ops.len()];
    for (fi, (ops, _)) in funcs.iter().enumerate() {
        for &o in ops {
            users[o].push(fi);
        }
    }

    let mut have = vec![false; all_ops.len()];
    for o in forge_core::IMPLEMENTED {
        if let Some(&i) = idx.get(o) {
            have[i] = true;
        }
    }
    // How many opcodes each function still misses.
    let mut missing: Vec<u32> = funcs
        .iter()
        .map(|(ops, _)| ops.iter().filter(|&&o| !have[o]).count() as u32)
        .collect();

    let (mut cov_fn, mut cov_instr) = (0usize, 0u64);
    for (fi, m) in missing.iter().enumerate() {
        if *m == 0 {
            cov_fn += 1;
            cov_instr += funcs[fi].1 as u64;
        }
    }

    println!(
        "{name}: {} Funktionen, {total_instrs} Instruktionen, {} Opcodes",
        funcs.len(),
        all_ops.len()
    );
    println!(
        "  GENERATOR schafft heute: {done_fn} Funktionen ({:.1} %), {done_instr} Instruktionen ({:.1} %)",
        pct(done_fn as u64, funcs.len() as u64),
        pct(done_instr, total_instrs)
    );
    // Code size is one of the paper's cost items, so it is worth reporting
    // against the wasm the functions came from — not against the whole file,
    // which is mostly data.
    let wasm_code: u64 = m
        .funcs
        .iter()
        .zip(p.bodies.iter())
        .filter(|(o, _)| matches!(o, forge_core::codegen::Outcome::Done(_)))
        .map(|(_, b)| b.instrs as u64)
        .sum();
    println!("  uebersetzt in {compile_ms:.0} ms");
    println!(
        "  erzeugt: {} B x86 fuer {} wasm-Instruktionen ({:.1} B je Instruktion)",
        m.code.len(),
        wasm_code,
        if wasm_code > 0 { m.code.len() as f64 / wasm_code as f64 } else { 0.0 }
    );

    // Auszaehlung: wohin gehen die Bytes? Die Frage ist, ob der Abstand zu
    // Cranelift an der Registerhaltung ueber Blockgrenzen haengt.
    {
        let (sb, sn, rb, rn, lb, ln) = forge_core::codegen::census::read();
        let total = m.code.len() as u64;
        let pct = |x: u64| if total > 0 { x as f64 * 100.0 / total as f64 } else { 0.0 };
        println!("  ---- wohin die Bytes gehen ----");
        println!("    Spill   {:>9} B ({:>4.1} %) in {:>8} Befehlen", sb, pct(sb), sn);
        println!("    Reload  {:>9} B ({:>4.1} %) in {:>8} Befehlen", rb, pct(rb), rn);
        println!("    Lokale  {:>9} B ({:>4.1} %) in {:>8} Befehlen", lb, pct(lb), ln);
        println!("    zusammen{:>9} B ({:>4.1} %) von {} B",
            sb + rb + lb, pct(sb + rb + lb), total);
    }
    if !blocked.is_empty() {
        let mut v: Vec<_> = blocked.iter().collect();
        v.sort_by(|a, b| b.1.cmp(a.1));
        let top: Vec<String> = v.iter().take(6).map(|(n, c)| format!("{n} ({c})")).collect();
        println!("  woran es abbricht: {}", top.join(", "));
    }
    println!(
        "\n  Wenn die {} Opcodes aus IMPLEMENTED voll saessen (Bloecke eingeschlossen):\
 {cov_fn} Funktionen ({:.1} %), {:.1} % der Instruktionen\n",
        have.iter().filter(|h| **h).count(),
        pct(cov_fn as u64, funcs.len() as u64),
        pct(cov_instr, total_instrs)
    );

    println!("  {:<4} {:<24} {:>8} {:>9} {:>9} {:>9}", "#", "Opcode", "Fn neu", "Fn kum.", "Fn %", "Instr %");
    println!("  {}", "-".repeat(70));

    let mut step = 0;
    loop {
        // Gain of an opcode: functions that are missing only this one.
        let (mut best, mut best_gain, mut best_users) = (usize::MAX, -1i64, 0usize);
        for o in 0..all_ops.len() {
            if have[o] {
                continue;
            }
            let gain = users[o].iter().filter(|&&fi| missing[fi] == 1).count() as i64;
            if gain > best_gain || (gain == best_gain && users[o].len() > best_users) {
                best = o;
                best_gain = gain;
                best_users = users[o].len();
            }
        }
        if best == usize::MAX {
            break;
        }
        have[best] = true;
        step += 1;
        let mut new_fn = 0usize;
        for &fi in &users[best] {
            missing[fi] -= 1;
            if missing[fi] == 0 {
                new_fn += 1;
                cov_fn += 1;
                cov_instr += funcs[fi].1 as u64;
            }
        }
        println!(
            "  {:<4} {:<24} {:>8} {:>9} {:>8.1}% {:>8.1}%",
            step,
            all_ops[best],
            new_fn,
            cov_fn,
            pct(cov_fn as u64, funcs.len() as u64),
            pct(cov_instr, total_instrs)
        );
    }
}

fn pct(a: u64, b: u64) -> f64 {
    if b == 0 { 0.0 } else { 100.0 * a as f64 / b as f64 }
}
