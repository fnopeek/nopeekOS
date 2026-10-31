//! A sampling profiler for generated code, for hosts without `perf`.
//!
//! `FORGE_PROF=<file>` arms it for a `--python` run: a `SIGPROF` timer
//! records the interrupted `rip` every 100 µs of CPU time, and at the end the
//! samples inside generated code are written to `<file>` as the hottest
//! functions and the hottest instructions (function index and offset inside
//! it, for `--dump`).

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

const CAP: usize = 1 << 20;
static SAMPLES: [AtomicU64; CAP] = [const { AtomicU64::new(0) }; CAP];
static N: AtomicUsize = AtomicUsize::new(0);
static ACTIVE: AtomicBool = AtomicBool::new(false);

struct Layout {
    base: u64,
    len: u64,
    /// (offset, wasm function index), sorted by offset.
    starts: Vec<(usize, u32)>,
    out: String,
}

static LAYOUT: std::sync::Mutex<Option<Layout>> = std::sync::Mutex::new(None);

extern "C" fn on_prof(_sig: i32, _info: *mut libc::siginfo_t, uc: *mut libc::c_void) {
    // SAFETY: the third argument of a `SA_SIGINFO` handler is a `ucontext_t`.
    let rip = unsafe { (*(uc as *mut libc::ucontext_t)).uc_mcontext.gregs[libc::REG_RIP as usize] as u64 };
    let i = N.fetch_add(1, Ordering::Relaxed);
    if i < CAP {
        SAMPLES[i].store(rip, Ordering::Relaxed);
    }
}

fn timer(us: i64) {
    let tv = libc::timeval { tv_sec: 0, tv_usec: us as libc::suseconds_t };
    let it = libc::itimerval { it_interval: tv, it_value: tv };
    // SAFETY: plain setitimer with a valid value.
    unsafe { libc::setitimer(libc::ITIMER_PROF, &it, std::ptr::null_mut()) };
}

/// Arm the profiler if `FORGE_PROF` names an output file.
pub fn start(m: &forge_core::CompiledModule, base: u64) {
    let Ok(out) = std::env::var("FORGE_PROF") else { return };
    let n_imp = m.plan.imported_funcs.len() as u32;
    let mut starts: Vec<(usize, u32)> = (0..m.offsets.len())
        .filter_map(|i| m.offsets[i].map(|o| (o, n_imp + i as u32)))
        .collect();
    starts.sort();
    *LAYOUT.lock().unwrap() = Some(Layout { base, len: m.code.len() as u64, starts, out });
    // SAFETY: a `sigaction` whose handler only stores into atomics.
    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = on_prof as *const () as usize;
        sa.sa_flags = libc::SA_SIGINFO | libc::SA_RESTART;
        libc::sigemptyset(&mut sa.sa_mask);
        libc::sigaction(libc::SIGPROF, &sa, std::ptr::null_mut());
    }
    ACTIVE.store(true, Ordering::Relaxed);
    timer(100);
}

/// Stop and write the report, once.
pub fn finish() {
    if !ACTIVE.swap(false, Ordering::Relaxed) {
        return;
    }
    timer(0);
    let Some(l) = LAYOUT.lock().unwrap().take() else { return };
    let n = N.load(Ordering::Relaxed).min(CAP);
    let mut per_fn: std::collections::HashMap<u32, u64> = Default::default();
    let mut per_ins: std::collections::HashMap<(u32, usize), u64> = Default::default();
    let mut inside = 0u64;
    for s in SAMPLES.iter().take(n) {
        let rip = s.load(Ordering::Relaxed);
        if rip < l.base || rip >= l.base + l.len {
            continue;
        }
        let off = (rip - l.base) as usize;
        let k = l.starts.partition_point(|&(o, _)| o <= off);
        let Some(&(fo, fi)) = k.checked_sub(1).and_then(|k| l.starts.get(k)) else { continue };
        inside += 1;
        *per_fn.entry(fi).or_default() += 1;
        *per_ins.entry((fi, off - fo)).or_default() += 1;
    }
    let mut out = format!("samples {n}, in generated code {inside}\n\n== functions\n");
    let mut fns: Vec<_> = per_fn.into_iter().collect();
    fns.sort_by(|a, b| b.1.cmp(&a.1));
    for (fi, c) in fns.iter().take(40) {
        out += &format!("{:6.2}%  function {fi}\n", 100.0 * *c as f64 / inside.max(1) as f64);
    }
    out += "\n== instructions (function, offset)\n";
    let mut ins: Vec<_> = per_ins.into_iter().collect();
    ins.sort_by(|a, b| b.1.cmp(&a.1));
    for ((fi, off), c) in ins.iter().take(200) {
        out += &format!("{:6.2}%  {fi} {off:#x}\n", 100.0 * *c as f64 / inside.max(1) as f64);
    }
    let _ = std::fs::write(&l.out, out);
}
