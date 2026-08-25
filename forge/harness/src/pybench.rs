//! Run `python.wasm` under both engines and time it.
//!
//! A wasi program finishes by leaving through `proc_exit`, not by returning —
//! the interpreter can turn that into an error and unwind, generated code
//! cannot. So both runs happen in a forked child that reports its own time,
//! exit status and stdout through a pipe on the way out. Same exit path, same
//! measurement, and the outputs can be compared byte for byte.

use crate::selftest::{Exec, Inst};
use crate::wasi_core::{self, WasiCtx};
use crate::wasi_glue;
use std::path::PathBuf;
use std::time::Instant;

pub fn wasi_host(module: &str, name: &str) -> Option<u64> {
    if module != "wasi_snapshot_preview1" {
        return None;
    }
    wasi_glue::forge_table()
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, a)| *a)
}

struct Run {
    ms: f64,
    code: i32,
    stdout: Vec<u8>,
}

/// Fork, let the child do the work and report, collect what it sent.
fn in_child(body: impl FnOnce(i32)) -> Option<Run> {
    let mut fds = [0i32; 2];
    // SAFETY: `fds` is a two-element array, which is what `pipe` writes.
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return None;
    }
    let (rd, wr) = (fds[0], fds[1]);
    // SAFETY: the child only runs `body` and never returns from it.
    let pid = unsafe { libc::fork() };
    if pid == 0 {
        // SAFETY: closing the read end this child does not use.
        unsafe { libc::close(rd) };
        body(wr);
        // Only reached if the module returned instead of calling `proc_exit`.
        // SAFETY: ends the child without unwinding.
        unsafe { libc::_exit(0) };
    }
    // SAFETY: closing the write end the parent does not use.
    unsafe { libc::close(wr) };
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        // SAFETY: reading into a local buffer from a pipe we own.
        let n = unsafe { libc::read(rd, chunk.as_mut_ptr() as *mut libc::c_void, chunk.len()) };
        if n <= 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n as usize]);
    }
    // SAFETY: our own descriptor, closed once.
    unsafe { libc::close(rd) };
    let mut st: libc::c_int = 0;
    // SAFETY: waiting for the child we just forked.
    unsafe { libc::waitpid(pid, &mut st, 0) };

    if buf.len() < 16 {
        return None;
    }
    let ms = f64::from_le_bytes(buf[0..8].try_into().ok()?);
    let code = i32::from_le_bytes(buf[8..12].try_into().ok()?);
    let len = u32::from_le_bytes(buf[12..16].try_into().ok()?) as usize;
    let stdout = buf.get(16..16 + len)?.to_vec();
    Some(Run { ms, code, stdout })
}

fn ctx_for(root: &PathBuf, argv: &[String]) -> WasiCtx {
    let mut args = vec!["/python.wasm".to_string()];
    args.extend(argv.iter().cloned());
    let env = vec![
        "PYTHONHOME=/".to_string(),
        "PYTHONDONTWRITEBYTECODE=1".to_string(),
    ];
    WasiCtx::new(root.clone(), "/", args, env)
}

fn forge_run(wasm: &[u8], root: &PathBuf, argv: &[String]) -> Option<(Run, f64)> {
    // Translation happens in the parent, so its cost is reported separately
    // and does not land inside the run.
    let t = Instant::now();
    let m = forge_core::compile(wasm).ok()?;
    let compile = t.elapsed().as_secs_f64() * 1000.0;
    let refused = m
        .funcs
        .iter()
        .filter(|o| matches!(o, forge_core::codegen::Outcome::Unsupported(_)))
        .count();
    if refused > 0 {
        eprintln!("  forge: {refused} Funktionen nicht uebersetzt");
        return None;
    }
    let root = root.clone();
    let argv = argv.to_vec();
    let run = in_child(move |wr| {
        let Some(exec) = Exec::new(&m.code) else { return };
        let Some(inst) = Inst::new(&m, exec.base()) else { return };
        let Some(fidx) = m.plan.exports.iter().find(|(n, _)| n == "_start").map(|(_, i)| *i)
        else {
            return;
        };
        let Some(off) = m.offset_of(fidx) else { return };
        let mut ctx = ctx_for(&root, &argv);
        wasi_glue::install(&mut ctx);
        wasi_core::arm_report(wr, Instant::now());
        exec.call_entry(m.entry_offset, off, inst.ptr(), 0, 0, 0);
        // Returned without `proc_exit` — report anyway.
        wasi_core::report(&ctx, 0);
    })?;
    Some((run, compile))
}

fn wasmi_run(wasm: &[u8], root: &PathBuf, argv: &[String]) -> Option<(Run, f64)> {
    use wasmi::{Config, Engine, Linker, Module, Store};
    let mut cfg = Config::default();
    cfg.consume_fuel(true);
    let engine = Engine::new(&cfg);
    let t = Instant::now();
    let module = Module::new(&engine, wasm).ok()?;
    let compile = t.elapsed().as_secs_f64() * 1000.0;

    let root = root.clone();
    let argv = argv.to_vec();
    let run = in_child(move |wr| {
        let ctx = ctx_for(&root, &argv);
        let mut store = Store::new(&engine, ctx);
        let _ = store.set_fuel(u64::MAX / 4);
        let mut linker = <Linker<WasiCtx>>::new(&engine);
        if wasi_glue::link_wasmi(&mut linker).is_err() {
            return;
        }
        let Ok(instance) = linker.instantiate_and_start(&mut store, &module) else {
            return;
        };
        let Ok(start) = instance.get_typed_func::<(), ()>(&store, "_start") else {
            return;
        };
        wasi_core::arm_report(wr, Instant::now());
        let _ = start.call(&mut store, ());
        wasi_core::report(store.data(), 0);
    })?;
    Some((run, compile))
}

pub fn run(args: &[String]) -> bool {
    if args.len() < 2 {
        println!("usage: --python <python.wasm> <root-dir> [guest args...]");
        return false;
    }
    let Ok(wasm) = std::fs::read(&args[0]) else {
        println!("{}: nicht lesbar", args[0]);
        return false;
    };
    let root = PathBuf::from(&args[1]);
    let argv: Vec<String> = args[2..].to_vec();

    println!("python.wasm  {}", argv.join(" "));
    let Some((w, wc)) = wasmi_run(&wasm, &root, &argv) else {
        println!("wasmi konnte nicht laufen");
        return false;
    };
    let Some((f, fc)) = forge_run(&wasm, &root, &argv) else {
        println!("forge konnte nicht laufen");
        return false;
    };

    let same = w.stdout == f.stdout && w.code == f.code;
    println!(
        "  Ausgabe: {:?} (Status {})  ->  {}",
        String::from_utf8_lossy(&w.stdout).trim_end(),
        w.code,
        if same {
            "GLEICH".to_string()
        } else {
            format!(
                "ABWEICHEND — forge: {:?} (Status {})",
                String::from_utf8_lossy(&f.stdout).trim_end(),
                f.code
            )
        }
    );
    println!(
        "  uebersetzen: wasmi {wc:.0} ms, forge {fc:.0} ms\n\
         \x20 LAUF:        wasmi {:.1} ms, forge {:.1} ms   ->  {:.2}x",
        w.ms,
        f.ms,
        if f.ms > 0.0 { w.ms / f.ms } else { 0.0 }
    );
    same
}
