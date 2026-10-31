//! Run an SDK app's `_start` under forge, with the few `npk_*` host calls a
//! command-line app uses: print, serial log and `npk_sys_info`'s clock keys.
//! Made for fbench, whose numbers on the host then compare directly with
//! its native build.
//!
//!   forge_harness --app <file.wasm>

use crate::selftest::{Exec, Inst};
use std::sync::OnceLock;

/// The bytes `ptr..ptr+len` of the calling instance's memory, if in range.
///
/// # Safety
/// `ctx` must be the instance context of the module doing the call.
unsafe fn guest_bytes<'a>(ctx: *const u64, ptr: u32, len: u32) -> Option<&'a [u8]> {
    use forge_core::vmctx as v;
    // SAFETY: the caller passes the live instance context; the range is
    // checked against the memory's recorded size.
    unsafe {
        let base = *ctx.add(v::MEM_BASE as usize / 8) as *const u8;
        let size = *ctx.add(v::MEM_SIZE as usize / 8) as usize;
        let end = (ptr as usize).checked_add(len as usize)?;
        (end <= size).then(|| std::slice::from_raw_parts(base.add(ptr as usize), len as usize))
    }
}

extern "C" fn h_print(ctx: *const u64, ptr: u32, len: u32) {
    // SAFETY: called by generated code with its own instance context.
    if let Some(b) = unsafe { guest_bytes(ctx, ptr, len) } {
        print!("{}", String::from_utf8_lossy(b));
    }
}

/// The TSC rate in MHz, measured once against the host clock.
fn tsc_mhz() -> u64 {
    static MHZ: OnceLock<u64> = OnceLock::new();
    *MHZ.get_or_init(|| {
        let t = std::time::Instant::now();
        // SAFETY: rdtsc has no preconditions.
        let a = unsafe { core::arch::x86_64::_rdtsc() };
        while t.elapsed().as_millis() < 100 {}
        // SAFETY: as above.
        let b = unsafe { core::arch::x86_64::_rdtsc() };
        (b - a) / t.elapsed().as_micros() as u64
    })
}

extern "C" fn h_sys_info(_ctx: *const u64, key: u32) -> u64 {
    match key & 0xff {
        10 => tsc_mhz(),
        // SAFETY: rdtsc has no preconditions.
        19 => unsafe { core::arch::x86_64::_rdtsc() },
        _ => 0,
    }
}

pub fn host_for(module: &str, name: &str) -> Option<u64> {
    match (module, name) {
        ("env", "npk_print") | ("env", "npk_log_serial") => Some(h_print as *const () as u64),
        ("env", "npk_sys_info") => Some(h_sys_info as *const () as u64),
        _ => None,
    }
}

pub fn run(args: &[String]) -> bool {
    let Some(path) = args.first() else {
        eprintln!("usage: forge_harness --app <file.wasm>");
        return false;
    };
    let wasm = match std::fs::read(path) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("{path}: {e}");
            return false;
        }
    };
    crate::faults::install();
    tsc_mhz();
    let t = std::time::Instant::now();
    let m = match forge_core::compile(&wasm) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("compile: {e}");
            return false;
        }
    };
    let compile = t.elapsed().as_secs_f64() * 1000.0;
    let refused: Vec<&str> = m
        .funcs
        .iter()
        .filter_map(|o| match o {
            forge_core::codegen::Outcome::Unsupported(n) => Some(*n),
            _ => None,
        })
        .collect();
    eprintln!("forge: {compile:.1} ms, {} B x86, {} refused {:?}", m.code.len(), refused.len(), refused);
    let Some(fidx) = m.plan.exports.iter().find(|(n, _)| n == "_start").map(|(_, i)| *i) else {
        eprintln!("no _start");
        return false;
    };
    let (Some(off), Some(exec)) = (m.offset_of(fidx), Exec::new(&m.code)) else {
        eprintln!("_start not compiled");
        return false;
    };
    exec.arm_traps(&m);
    let Some(mut inst) = Inst::new(&m, exec.base()) else {
        eprintln!("instance");
        return false;
    };
    inst.set_stack_budget(4 << 20);
    exec.call_entry(m.entry_offset, off, inst.ptr(), 0, 0, 0);
    let trap = inst.trap_code();
    if trap != forge_core::trap::NONE {
        eprintln!("trap: {}", forge_core::trap::name(trap));
        return false;
    }
    true
}
