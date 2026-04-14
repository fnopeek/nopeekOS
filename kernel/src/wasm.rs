//! WASM Runtime
//!
//! Sandboxed execution via wasmi interpreter.
//! Every host function is capability-gated.
//! Modules loaded from npkFS execute with delegated capabilities —
//! no ambient authority, no access beyond what was explicitly granted.

use alloc::string::String;
use alloc::vec::Vec;
use wasmi::{Caller, Config, Engine, Linker, Module, Store, Val};
use spin::Mutex;
use crate::{kprint, kprintln, capability};
use crate::capability::CapId;

pub struct WasmResult {
    pub output: String,
}

struct HostState {
    output: String,
    cap_id: CapId,
    /// When true, npk_print writes directly to terminal instead of buffering
    direct_output: bool,
    /// Terminal index for direct output (255 = use active terminal via kprint)
    terminal_idx: u8,
    /// Core ID this WASM app is running on (for CPU usage tracking)
    core_id: usize,
    /// Process ID in the process table
    pid: u32,
}

static ENGINE: Mutex<Option<Engine>> = Mutex::new(None);

/// Default fuel budget per module execution (~10M instructions)
const DEFAULT_FUEL: u64 = 10_000_000;

/// Fuel budget for interactive apps (top, etc.) — effectively unlimited
const INTERACTIVE_FUEL: u64 = 1_000_000_000;

// ── Worker-Core WASM Jobs ──────────────────────────────────────

const MAX_WASM_JOBS: usize = 4;

struct WasmJob {
    bytes: Vec<u8>,
    cap_id: CapId,
    terminal_idx: u8,
    name: [u8; 32],
    name_len: u8,
}

static WASM_JOBS: Mutex<[Option<WasmJob>; MAX_WASM_JOBS]> = Mutex::new([
    None, None, None, None,
]);

/// Per-job completion flag (set by worker, read by BSP)
static JOB_DONE: [core::sync::atomic::AtomicBool; MAX_WASM_JOBS] = [
    core::sync::atomic::AtomicBool::new(false),
    core::sync::atomic::AtomicBool::new(false),
    core::sync::atomic::AtomicBool::new(false),
    core::sync::atomic::AtomicBool::new(false),
];

// ── Per-App Key Buffers (Core 0 writes, worker reads) ─────────
//
// Each terminal has its own SPSC ring buffer. Core 0 pushes keys
// based on which window is focused. Apps read via npk_input_wait.

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering as AtOrd};

const APP_KEY_BUF_SIZE: usize = 32;
const MAX_APP_BUFS: usize = 256;

static mut APP_KEY_BUFS: [([u8; APP_KEY_BUF_SIZE], AtomicUsize, AtomicUsize); MAX_APP_BUFS] = {
    const INIT: ([u8; APP_KEY_BUF_SIZE], AtomicUsize, AtomicUsize) =
        ([0; APP_KEY_BUF_SIZE], AtomicUsize::new(0), AtomicUsize::new(0));
    [INIT; MAX_APP_BUFS]
};

/// Per-terminal flag: true if a WASM app is running in this terminal.
static APP_RUNNING: [AtomicBool; MAX_APP_BUFS] = {
    const FALSE: AtomicBool = AtomicBool::new(false);
    [FALSE; MAX_APP_BUFS]
};

/// Push a key to an app's input buffer. Called from Core 0.
pub fn push_app_key(terminal_idx: u8, key: u8) {
    let idx = terminal_idx as usize;
    if idx >= MAX_APP_BUFS { return; }
    // SAFETY: single producer (Core 0), idx bounds checked
    let (buf, head, tail) = unsafe { &mut APP_KEY_BUFS[idx] };
    let h = head.load(AtOrd::Relaxed);
    let next = (h + 1) % APP_KEY_BUF_SIZE;
    if next != tail.load(AtOrd::Acquire) {
        buf[h] = key;
        head.store(next, AtOrd::Release);
    }
}

/// Pop a key from an app's input buffer. Called from worker core.
fn pop_app_key(terminal_idx: u8) -> Option<u8> {
    let idx = terminal_idx as usize;
    if idx >= MAX_APP_BUFS { return None; }
    // SAFETY: single consumer (worker core), idx bounds checked
    let (buf, head, tail) = unsafe { &APP_KEY_BUFS[idx] };
    let t = tail.load(AtOrd::Relaxed);
    if t == head.load(AtOrd::Acquire) { return None; }
    let key = buf[t];
    tail.store((t + 1) % APP_KEY_BUF_SIZE, AtOrd::Release);
    Some(key)
}

/// Clear an app's key buffer. Called when spawning a new app.
fn clear_app_key_buf(terminal_idx: u8) {
    let idx = terminal_idx as usize;
    if idx >= MAX_APP_BUFS { return; }
    let (_, head, tail) = unsafe { &mut APP_KEY_BUFS[idx] };
    head.store(0, AtOrd::Relaxed);
    tail.store(0, AtOrd::Relaxed);
}

/// Check if the given terminal has a running WASM app.
pub fn has_wasm_app(terminal_idx: u8) -> bool {
    let idx = terminal_idx as usize;
    if idx >= MAX_APP_BUFS { return false; }
    APP_RUNNING[idx].load(AtOrd::Acquire)
}

/// Spawn a WASM module on a worker core. Returns immediately.
/// The app gets its own window and terminal.
pub fn spawn_on_worker(wasm_bytes: Vec<u8>, cap_id: CapId, terminal_idx: u8, module_name: &str) -> bool {
    let mut jobs = WASM_JOBS.lock();
    let slot = match jobs.iter().position(|j| j.is_none()) {
        Some(i) => i,
        None => { kprintln!("[npk] No free WASM job slots"); return false; }
    };

    let mut name = [0u8; 32];
    let nlen = module_name.len().min(32);
    name[..nlen].copy_from_slice(&module_name.as_bytes()[..nlen]);

    JOB_DONE[slot].store(false, core::sync::atomic::Ordering::Relaxed);
    jobs[slot] = Some(WasmJob { bytes: wasm_bytes, cap_id, terminal_idx, name, name_len: nlen as u8 });
    drop(jobs);

    // Clear per-app input buffer + mark terminal as having an app
    clear_app_key_buf(terminal_idx);
    if (terminal_idx as usize) < MAX_APP_BUFS {
        APP_RUNNING[terminal_idx as usize].store(true, AtOrd::Release);
    }

    crate::smp::scheduler::spawn(
        crate::smp::scheduler::Priority::Interactive,
        wasm_worker_task,
        slot as u64,
    );

    true
}

/// Worker-core entry: runs WASM module, signals completion.
fn wasm_worker_task(arg: u64) {
    let slot = arg as usize;
    let job = {
        let mut jobs = WASM_JOBS.lock();
        if slot >= MAX_WASM_JOBS { return; }
        jobs[slot].take()
    };
    let job = match job {
        Some(j) => j,
        None => return,
    };
    let terminal_idx = job.terminal_idx;

    // Clone engine (Arc internally, cheap)
    let engine = match ENGINE.lock().as_ref().cloned() {
        Some(e) => e,
        None => { JOB_DONE[slot].store(true, core::sync::atomic::Ordering::Release); return; }
    };

    let module = match Module::new(&engine, &job.bytes) {
        Ok(m) => m,
        Err(_) => { JOB_DONE[slot].store(true, core::sync::atomic::Ordering::Release); return; }
    };

    let core_id = crate::smp::per_core::current_core_id();

    // Register process in process table
    let name_str = core::str::from_utf8(&job.name[..job.name_len as usize]).unwrap_or("?");
    let pid = crate::process::spawn(name_str, crate::process::KIND_WASM, terminal_idx, core_id as u8);

    let mut store = Store::new(&engine, HostState {
        output: String::new(),
        cap_id: job.cap_id,
        direct_output: true,
        terminal_idx: job.terminal_idx,
        core_id,
        pid,
    });
    let _ = store.set_fuel(INTERACTIVE_FUEL);

    let mut linker = <Linker<HostState>>::new(&engine);
    if register_host_functions(&mut linker).is_err() {
        crate::process::exit(pid);
        JOB_DONE[slot].store(true, core::sync::atomic::Ordering::Release);
        return;
    }

    let instance = match linker.instantiate_and_start(&mut store, &module) {
        Ok(i) => i,
        Err(_) => {
            crate::process::exit(pid);
            JOB_DONE[slot].store(true, core::sync::atomic::Ordering::Release);
            return;
        }
    };

    // Track WASM linear memory size
    if let Some(mem) = instance.get_memory(&store, "memory") {
        crate::process::set_memory(pid, mem.data_size(&store) as u32);
    }

    let func = match instance.get_func(&store, "_start") {
        Some(f) => f,
        None => {
            crate::process::exit(pid);
            JOB_DONE[slot].store(true, core::sync::atomic::Ordering::Release);
            return;
        }
    };

    let _ = func.call(&mut store, &[], &mut []);

    // Update final memory usage
    if let Some(mem) = instance.get_memory(&store, "memory") {
        crate::process::set_memory(pid, mem.data_size(&store) as u32);
    }

    // Deregister process + clear app marker + signal completion
    crate::process::exit(pid);
    if (terminal_idx as usize) < MAX_APP_BUFS {
        APP_RUNNING[terminal_idx as usize].store(false, AtOrd::Release);
    }
    JOB_DONE[slot].store(true, core::sync::atomic::Ordering::Release);
}

pub fn init() {
    let mut config = Config::default();
    config.consume_fuel(true);
    let engine = Engine::new(&config);
    *ENGINE.lock() = Some(engine);
    kprintln!("[npk] WASM runtime: wasmi v1.0 (fuel-metered)");
}

/// Execute a WASM module with basic host functions (legacy API for built-in add/multiply)
pub fn execute(wasm_bytes: &[u8], func_name: &str, args: &[Val]) -> Result<WasmResult, WasmError> {
    execute_inner(wasm_bytes, func_name, args, capability::CAP_NULL)
}

/// Execute a WASM module loaded from npkFS with capability-gated host functions.
/// The module receives a delegated capability token.
pub fn execute_sandboxed(
    wasm_bytes: &[u8], func_name: &str, args: &[Val], cap_id: CapId,
) -> Result<WasmResult, WasmError> {
    execute_inner(wasm_bytes, func_name, args, cap_id)
}

/// Execute a WASM module in interactive mode (live display).
/// npk_print writes directly to terminal. Used for long-running apps (top).
pub fn execute_interactive(
    wasm_bytes: &[u8], func_name: &str, args: &[Val], cap_id: CapId,
) -> Result<WasmResult, WasmError> {
    // Clone engine to release ENGINE lock — interactive apps run for a long time
    let engine = {
        let guard = ENGINE.lock();
        guard.as_ref().ok_or(WasmError::NotInitialized)?.clone()
    };

    let module = Module::new(&engine, wasm_bytes)
        .map_err(|_| WasmError::InvalidModule)?;

    let mut store = Store::new(&engine, HostState {
        output: String::new(),
        cap_id,
        direct_output: true,
        terminal_idx: 255, // active terminal
        core_id: 0, // runs on Core 0 (non-worker path)
        pid: 0,
    });
    store.set_fuel(INTERACTIVE_FUEL).map_err(|_| WasmError::ExecutionFailed)?;

    let mut linker = <Linker<HostState>>::new(&engine);
    register_host_functions(&mut linker)?;

    let instance = linker.instantiate_and_start(&mut store, &module)
        .map_err(|_| WasmError::InstantiationFailed)?;

    let func = instance.get_func(&store, func_name)
        .ok_or(WasmError::FunctionNotFound)?;

    func.call(&mut store, args, &mut [])
        .map_err(|e| map_exec_error(e))?;

    Ok(WasmResult { output: String::new() })
}

fn execute_inner(
    wasm_bytes: &[u8], func_name: &str, args: &[Val], cap_id: CapId,
) -> Result<WasmResult, WasmError> {
    let engine_guard = ENGINE.lock();
    let engine = engine_guard.as_ref().ok_or(WasmError::NotInitialized)?;

    let module = Module::new(engine, wasm_bytes)
        .map_err(|_| WasmError::InvalidModule)?;

    let mut store = Store::new(engine, HostState {
        output: String::new(),
        cap_id,
        direct_output: false,
        terminal_idx: 255,
        core_id: 0,
        pid: 0,
    });
    store.set_fuel(DEFAULT_FUEL).map_err(|_| WasmError::ExecutionFailed)?;

    let mut linker = <Linker<HostState>>::new(engine);
    register_host_functions(&mut linker)?;

    let instance = linker.instantiate_and_start(&mut store, &module)
        .map_err(|_| WasmError::InstantiationFailed)?;

    let func = instance.get_func(&store, func_name)
        .ok_or(WasmError::FunctionNotFound)?;

    let ty = func.ty(&store);
    let num_results = ty.results().len();

    if num_results == 0 {
        func.call(&mut store, args, &mut [])
            .map_err(|e| map_exec_error(e))?;
    } else {
        let mut results = [Val::I32(0)];
        func.call(&mut store, args, &mut results)
            .map_err(|e| map_exec_error(e))?;

        let host = store.data();
        if host.output.is_empty() {
            let output = match results[0] {
                Val::I32(v) => alloc::format!("{}", v),
                Val::I64(v) => alloc::format!("{}", v),
                _ => alloc::format!("{:?}", results[0]),
            };
            return Ok(WasmResult { output });
        }
    }

    Ok(WasmResult { output: store.data().output.clone() })
}

fn register_host_functions(linker: &mut Linker<HostState>) -> Result<(), WasmError> {
    // npk_print(ptr, len) — write to output buffer or directly to terminal
    linker.func_wrap("env", "npk_print",
        |mut caller: Caller<'_, HostState>, ptr: i32, len: i32| {
            if let Some(s) = read_wasm_str(&caller, ptr, len) {
                if caller.data().direct_output {
                    let idx = caller.data().terminal_idx;
                    if (idx as usize) < MAX_APP_BUFS {
                        // Write to specific terminal (worker-core safe)
                        crate::shade::terminal::write_idx(idx as usize, &s);
                    } else {
                        // Fallback: write to active terminal via kprint
                        kprint!("{}", s);
                    }
                } else {
                    caller.data_mut().output.push_str(&s);
                }
            }
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_log(ptr, len) — write to serial console (no cap needed, output only)
    linker.func_wrap("env", "npk_log",
        |caller: Caller<'_, HostState>, ptr: i32, len: i32| {
            if let Some(s) = read_wasm_str(&caller, ptr, len) {
                kprintln!("{}", s);
            }
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_fetch(name_ptr, name_len, buf_ptr, buf_max) -> bytes or -1
    linker.func_wrap("env", "npk_fetch",
        |mut caller: Caller<'_, HostState>, name_ptr: i32, name_len: i32,
         buf_ptr: i32, buf_max: i32| -> i32 {
            let cap_id = caller.data().cap_id;
            if capability::check_global(&cap_id, capability::Rights::READ).is_err() {
                kprintln!("[npk] WASM: npk_fetch DENIED (no READ)");
                return -1;
            }

            let name = match read_wasm_str(&caller, name_ptr, name_len) {
                Some(s) => s,
                None => return -1,
            };

            let (content, _) = match crate::npkfs::fetch(&name) {
                Ok(v) => v,
                Err(_) => return -1,
            };

            let write_len = content.len().min(buf_max as usize);
            let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(m) => m,
                None => return -1,
            };
            let data = mem.data_mut(&mut caller);
            let start = buf_ptr as usize;
            if start + write_len <= data.len() {
                data[start..start + write_len].copy_from_slice(&content[..write_len]);
                write_len as i32
            } else {
                -1
            }
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_store(name_ptr, name_len, data_ptr, data_len) -> 0 or -1
    linker.func_wrap("env", "npk_store",
        |caller: Caller<'_, HostState>, name_ptr: i32, name_len: i32,
         data_ptr: i32, data_len: i32| -> i32 {
            let cap_id = caller.data().cap_id;
            if capability::check_global(&cap_id, capability::Rights::WRITE).is_err() {
                kprintln!("[npk] WASM: npk_store DENIED (no WRITE)");
                return -1;
            }

            let name = match read_wasm_str(&caller, name_ptr, name_len) {
                Some(s) => s,
                None => return -1,
            };

            let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(m) => m,
                None => return -1,
            };
            let data = mem.data(&caller);
            let start = data_ptr as usize;
            let end = (start + data_len as usize).min(data.len());
            if start >= end { return -1; }

            match crate::npkfs::store(&name, &data[start..end], cap_id) {
                Ok(_) => 0,
                Err(_) => -1,
            }
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_get_fb_size() -> (width << 16) | height
    linker.func_wrap("env", "npk_get_fb_size",
        |_caller: Caller<'_, HostState>| -> i64 {
            let (w, h) = crate::framebuffer::get_resolution();
            ((w as i64) << 32) | (h as i64)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_set_wallpaper(ptr, len, width, height) -> 0 or -1
    // Receives raw BGRA pixel data, sets it as the compositor wallpaper.
    linker.func_wrap("env", "npk_set_wallpaper",
        |mut caller: Caller<'_, HostState>, ptr: i32, len: i32,
         width: i32, height: i32| -> i32 {
            let cap_id = caller.data().cap_id;
            if capability::check_global(&cap_id, capability::Rights::WRITE).is_err() {
                kprintln!("[npk] WASM: npk_set_wallpaper DENIED (no WRITE)");
                return -1;
            }

            let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(m) => m,
                None => return -1,
            };
            let data = mem.data(&caller);
            let start = ptr as usize;
            let pixel_bytes = (width as usize) * (height as usize) * 4;
            let end = start + pixel_bytes;
            if end > data.len() || end > len as usize + start { return -1; }

            let info = crate::framebuffer::get_info();
            crate::gui::background::set_wallpaper(
                &data[start..end], width as u32, height as u32, &info);

            // Force compositor full redraw
            crate::shade::force_redraw();
            kprintln!("[npk] Wallpaper set ({}x{}, theme extracted)", width, height);
            0
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_set_theme(ptr) -> 0 or -1
    // Receives 16 u32 colors (64 bytes), sets as theme palette.
    linker.func_wrap("env", "npk_set_theme",
        |mut caller: Caller<'_, HostState>, ptr: i32| -> i32 {
            let cap_id = caller.data().cap_id;
            if capability::check_global(&cap_id, capability::Rights::WRITE).is_err() {
                return -1;
            }

            let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(m) => m,
                None => return -1,
            };
            let data = mem.data(&caller);
            let start = ptr as usize;
            if start + 64 > data.len() { return -1; }

            let mut colors = [0u32; 16];
            for i in 0..16 {
                let off = start + i * 4;
                colors[i] = u32::from_le_bytes([
                    data[off], data[off + 1], data[off + 2], data[off + 3],
                ]);
            }
            crate::theme::set_palette(&colors);
            crate::shade::force_redraw();
            0
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_sys_info(key) -> i64 — system information for apps (e.g. top)
    // Keys: 0=cores, 1=uptime_secs, 2=free_mb, 3=heap_used, 4=heap_total,
    //        5=tasks_spawned, 6=tasks_completed, 7=steals, 8=workers,
    //        9=has_mwait, 10=tsc_mhz, 11=queue_len(core N, pass core in high bits)
    linker.func_wrap("env", "npk_sys_info",
        |_caller: Caller<'_, HostState>, key: i32| -> i64 {
            match key & 0xFF {
                0 => crate::smp::per_core::core_count() as i64,
                1 => crate::interrupts::uptime_secs() as i64,
                2 => { let (_, mb) = crate::memory::stats(); mb as i64 },
                3 => { let (used, _) = crate::heap::stats(); used as i64 },
                4 => { let (_, total) = crate::heap::stats(); total as i64 },
                5 => { let (s, _, _, _) = crate::smp::scheduler::stats(); s as i64 },
                6 => { let (_, c, _, _) = crate::smp::scheduler::stats(); c as i64 },
                7 => { let (_, _, st, _) = crate::smp::scheduler::stats(); st as i64 },
                8 => { let (_, _, _, w) = crate::smp::scheduler::stats(); w as i64 },
                9 => if crate::smp::per_core::has_mwait() { 1 } else { 0 },
                10 => (crate::interrupts::tsc_freq() / 1_000_000) as i64,
                11 => {
                    let core = (key >> 8) as usize;
                    crate::smp::scheduler::queue_len(core) as i64
                },
                12 => {
                    let core = (key >> 8) as usize;
                    crate::smp::per_core::core_freq_mhz(core) as i64
                },
                13 => crate::smp::per_core::max_turbo_mhz() as i64,
                14 => crate::smp::per_core::min_eff_mhz() as i64,
                15 => {
                    let core = (key >> 8) as usize;
                    crate::smp::per_core::core_usage(core) as i64
                },
                // CPUID 0x15 raw values for diagnostics
                16 => { let (eax, _, _) = crate::interrupts::cpuid15(); eax as i64 },
                17 => { let (_, ebx, _) = crate::interrupts::cpuid15(); ebx as i64 },
                18 => { let (_, _, ecx) = crate::interrupts::cpuid15(); ecx as i64 },

                // ── Process tracking (keys 20-29) → process table ──
                // 20: count, 21: pid_at_index, 22-29: query by PID
                20..=29 => crate::process::sys_info(key),
                _ => -1,
            }
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_sleep(ms) -> 0 — sleep for N milliseconds.
    // Worker-core: just waits (Core 0 handles rendering via poll_render).
    linker.func_wrap("env", "npk_sleep",
        |_caller: Caller<'_, HostState>, ms: i32| -> i32 {
            if ms <= 0 || ms > 60000 { return -1; }

            let freq = crate::interrupts::tsc_freq();
            let ticks_per_ms = freq / 1000;
            let target = crate::interrupts::rdtsc() + (ms as u64) * ticks_per_ms;
            while crate::interrupts::rdtsc() < target {
                core::hint::spin_loop();
            }

            0
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_input_poll() -> key or -1 — non-blocking read from per-app buffer
    linker.func_wrap("env", "npk_input_poll",
        |caller: Caller<'_, HostState>| -> i32 {
            match pop_app_key(caller.data().terminal_idx) {
                Some(k) => k as i32,
                None => -1,
            }
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_input_wait(timeout_ms) -> key or -1 — blocking wait with timeout
    // Spins on worker core checking per-app key buffer + TSC deadline.
    // Flushes busy-TSC and marks core idle during wait for accurate CPU usage.
    linker.func_wrap("env", "npk_input_wait",
        |caller: Caller<'_, HostState>, timeout_ms: i32| -> i32 {
            let term_idx = caller.data().terminal_idx;
            let core_id = caller.data().core_id;
            if timeout_ms <= 0 {
                return match pop_app_key(term_idx) {
                    Some(k) => k as i32,
                    None => -1,
                };
            }

            // Flush work done since last checkpoint, update process table
            let flushed = crate::smp::per_core::flush_busy(core_id);
            crate::process::add_busy_tsc(caller.data().pid, flushed);
            crate::smp::per_core::update_core_freq(core_id);
            crate::smp::per_core::set_active(core_id, false);

            let ms = (timeout_ms as u64).min(60_000);
            let freq = crate::interrupts::tsc_freq();
            let ticks_per_ms = freq / 1000;
            let deadline = crate::interrupts::rdtsc() + ms * ticks_per_ms;

            let result = loop {
                if let Some(k) = pop_app_key(term_idx) {
                    break k as i32;
                }
                if crate::interrupts::rdtsc() >= deadline {
                    break -1;
                }
                core::hint::spin_loop();
            };

            // Resume work tracking
            crate::smp::per_core::set_active(core_id, true);
            crate::smp::per_core::start_work(core_id);

            result
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_clear() — clear the app's terminal
    linker.func_wrap("env", "npk_clear",
        |caller: Caller<'_, HostState>| {
            let idx = caller.data().terminal_idx;
            if (idx as usize) < MAX_APP_BUFS {
                crate::shade::terminal::clear_idx(idx as usize);
            } else {
                crate::shade::terminal::clear();
            }
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    Ok(())
}

fn read_wasm_str(caller: &Caller<'_, HostState>, ptr: i32, len: i32) -> Option<String> {
    let mem = caller.get_export("memory").and_then(|e| e.into_memory())?;
    let data = mem.data(caller);
    let start = ptr as usize;
    let end = (start + len as usize).min(data.len());
    if start >= end { return None; }
    let mut buf = alloc::vec![0u8; end - start];
    buf.copy_from_slice(&data[start..end]);
    core::str::from_utf8(&buf).ok().map(String::from)
}

fn map_exec_error(e: wasmi::Error) -> WasmError {
    let msg = alloc::format!("{}", e);
    if msg.contains("fuel") { WasmError::FuelExhausted } else { WasmError::ExecutionFailed }
}

#[derive(Debug)]
pub enum WasmError {
    NotInitialized,
    InvalidModule,
    InstantiationFailed,
    FunctionNotFound,
    ExecutionFailed,
    FuelExhausted,
    HostFunctionError,
}

impl core::fmt::Display for WasmError {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        match self {
            WasmError::NotInitialized => write!(f, "WASM runtime not initialized"),
            WasmError::InvalidModule => write!(f, "invalid WASM module"),
            WasmError::InstantiationFailed => write!(f, "instantiation failed"),
            WasmError::FunctionNotFound => write!(f, "function not found"),
            WasmError::ExecutionFailed => write!(f, "execution failed"),
            WasmError::FuelExhausted => write!(f, "execution limit exceeded (fuel exhausted)"),
            WasmError::HostFunctionError => write!(f, "host function error"),
        }
    }
}

pub fn val_i32(v: i32) -> Val { Val::I32(v) }

// === Built-in WASM modules ===

/// add(i32, i32) -> i32
pub const MODULE_ADD: &[u8] = &[
    0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00,
    0x01, 0x07, 0x01, 0x60, 0x02, 0x7F, 0x7F, 0x01, 0x7F,
    0x03, 0x02, 0x01, 0x00,
    0x07, 0x07, 0x01, 0x03, 0x61, 0x64, 0x64, 0x00, 0x00,
    0x0A, 0x09, 0x01, 0x07, 0x00, 0x20, 0x00, 0x20, 0x01, 0x6A, 0x0B,
];

/// multiply(i32, i32) -> i32
pub const MODULE_MULTIPLY: &[u8] = &[
    0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00,
    0x01, 0x07, 0x01, 0x60, 0x02, 0x7F, 0x7F, 0x01, 0x7F,
    0x03, 0x02, 0x01, 0x00,
    0x07, 0x0C, 0x01, 0x08, 0x6D, 0x75, 0x6C, 0x74, 0x69, 0x70, 0x6C, 0x79, 0x00, 0x00,
    0x0A, 0x09, 0x01, 0x07, 0x00, 0x20, 0x00, 0x20, 0x01, 0x6C, 0x0B,
];

/// _start() — calls npk_log("Hello from WASM sandbox!")
pub const MODULE_HELLO: &[u8] = &[
    0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, // header
    0x01, 0x09, 0x02, 0x60, 0x02, 0x7F, 0x7F, 0x00, 0x60, 0x00, 0x00, // types
    0x02, 0x0F, 0x01, 0x03, 0x65, 0x6E, 0x76, 0x07, 0x6E, 0x70, 0x6B,
        0x5F, 0x6C, 0x6F, 0x67, 0x00, 0x00, // import npk_log
    0x03, 0x02, 0x01, 0x01, // function _start: type 1
    0x05, 0x03, 0x01, 0x00, 0x01, // memory: 1 page
    0x07, 0x13, 0x02, 0x06, 0x6D, 0x65, 0x6D, 0x6F, 0x72, 0x79, 0x02, 0x00,
        0x06, 0x5F, 0x73, 0x74, 0x61, 0x72, 0x74, 0x00, 0x01, // exports
    0x0A, 0x0A, 0x01, 0x08, 0x00, 0x41, 0x00, 0x41, 0x18, 0x10, 0x00, 0x0B, // code
    0x0B, 0x1E, 0x01, 0x00, 0x41, 0x00, 0x0B, 0x18, // data header
        0x48, 0x65, 0x6C, 0x6C, 0x6F, 0x20, 0x66, 0x72, 0x6F, 0x6D, 0x20,
        0x57, 0x41, 0x53, 0x4D, 0x20, 0x73, 0x61, 0x6E, 0x64, 0x62, 0x6F,
        0x78, 0x21, // "Hello from WASM sandbox!"
];

/// fib(i32) -> i32 — recursive fibonacci (Python-verified bytes)
pub const MODULE_FIB: &[u8] = &[
    0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00,
    0x01, 0x06, 0x01, 0x60, 0x01, 0x7f, 0x01, 0x7f,
    0x03, 0x02, 0x01, 0x00,
    0x07, 0x07, 0x01, 0x03, 0x66, 0x69, 0x62, 0x00, 0x00,
    0x0a, 0x1e, 0x01, 0x1c, 0x00,
        0x20, 0x00, 0x41, 0x02, 0x48,
        0x04, 0x7f,
            0x20, 0x00,
        0x05,
            0x20, 0x00, 0x41, 0x01, 0x6b, 0x10, 0x00,
            0x20, 0x00, 0x41, 0x02, 0x6b, 0x10, 0x00,
            0x6a,
        0x0b,
        0x0b,
];
