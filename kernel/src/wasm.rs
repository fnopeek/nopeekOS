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
use crate::drivers::pci;

pub struct WasmResult {
    pub output: String,
}

/// Hardware driver state for WASM modules that access PCI devices.
struct HwDriverState {
    pci_addr: pci::PciAddr,
    #[allow(dead_code)] // populated for future audit/debug, not yet read
    vendor_id: u16,
    #[allow(dead_code)]
    device_id: u16,
    mmio_maps: Vec<(u64, usize)>,   // handle -> (base_virt, page_count)
    dma_allocs: Vec<(u64, usize)>,  // handle -> (phys_addr, page_count)
    bus_master_enabled: bool,
    registered_as_netdev: bool,
}

const MAX_MMIO_MAPS: usize = 4;
/// Per-module DMA allocation slots. Was 128, which the AX200 driver hit with
/// 64 one-page receive buffers plus its rings — and 64 buffers is 12 ms of
/// headroom at 64 Mbit, against a `worker_idle_hlt` that parks the core for up
/// to 10 ms between drains. Linux allocates 2048 for this chip. Each slot is a
/// (phys, pages) pair, so the ceiling is bookkeeping, not memory.
const MAX_DMA_ALLOCS: usize = 1024;
const MAX_DMA_PAGES: usize = 2048; // 8MB total (iwlwifi FW sections ~1.3MB)
const MAX_DMA_PAGES_PER_CALL: usize = 1024; // 4MB; a single FW section can exceed 256KB

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
    /// Hardware driver state (only set for driver modules)
    hw: Option<HwDriverState>,
    /// Shade window id owned by this WASM app for widget rendering.
    /// 0 = no widget window yet (first scene_commit allocates one).
    /// Phase 10: set when the app calls npk_scene_commit, reused on
    /// subsequent commits so the same window is updated in place.
    widget_window_id: u32,
    /// Module name, used as the window title when the app's first
    /// scene_commit (or npk_window_set_overlay) creates its widget
    /// window. Cloned from the WASM job at worker entry.
    module_name: String,
    /// Optional launch argument (e.g. a file path to open) the app reads
    /// via `npk_launch_arg`. None = launched without an argument.
    launch_arg: Option<String>,
    /// URL the last `npk_http_request` body actually came from, after
    /// redirects. Read back via `npk_http_final_url` — a browser needs it
    /// as the document base URL for relative sub-resources.
    http_final_url: Option<String>,
    /// The last `npk_http_request`'s `Content-Type`. Read back via
    /// `npk_http_content_type` — a browser cannot decode a document
    /// without it, and guessing the charset wrong costs the WHOLE page.
    http_content_type: Option<String>,
    /// Why the last `npk_http_request` failed, as `kind\tmessage`. Read
    /// back via `npk_http_last_error`. Without it every failure reaches the
    /// caller as a bare -1, which is how an untrusted certificate ended up
    /// rendering as a blank page.
    http_last_error: Option<String>,
    /// The last `npk_http_send` response's header block and status. Read back
    /// via `npk_http_response_headers` / `npk_http_status`. A browser needs
    /// headers the body cannot carry — `Set-Cookie` above all, which repeats
    /// and so could never be a single-value getter.
    http_reply_headers: Option<String>,
    http_status: u16,
}

static ENGINE: Mutex<Option<Engine>> = Mutex::new(None);

/// Fuel budget for interactive apps and drivers — effectively unlimited.
const INTERACTIVE_FUEL: u64 = u64::MAX / 2;

/// Default dialog size for `npk_pick`. `set_overlay` clamps it to the
/// screen, so these are an upper bound, not a requirement.
const PICKER_W: u32 = 760;
const PICKER_H: u32 = 520;

/// Module that serves `npk_pick`, from `sys/config/picker` (default
/// `pick`). Read here rather than hardcoded so the dialog is replaceable,
/// but never taken from the caller — a requester that could name its own
/// picker could show the user a fake dialog and answer it itself.
fn picker_module_name() -> String {
    const DEFAULT_PICKER: &str = "pick";
    match crate::npkfs::fetch("sys/config/picker") {
        Ok((bytes, _)) => {
            let raw = core::str::from_utf8(&bytes).unwrap_or("").trim();
            let cleaned: String = raw.chars().take_while(|c| !c.is_control()).take(64).collect();
            if cleaned.is_empty() || cleaned.contains('/') || cleaned.contains("..") {
                String::from(DEFAULT_PICKER)
            } else {
                cleaned
            }
        }
        Err(_) => String::from(DEFAULT_PICKER),
    }
}

// ── Worker-Core WASM Jobs ──────────────────────────────────────

// Concurrent in-flight WASM spawn slots. A worker takes its slot out of
// the array as soon as it starts, so this caps *pending* (not-yet-started)
// spawns. 4 was too low once dock + bar + loft + iris are all resident —
// a transient one-shot (a screenshot) couldn't even get queued.
const MAX_WASM_JOBS: usize = 16;

struct WasmJob {
    bytes: Vec<u8>,
    cap_id: CapId,
    terminal_idx: u8,
    name: [u8; 32],
    name_len: u8,
    /// Pre-allocated widget window id for widget-kind apps. 0 = app
    /// will get a window on its first npk_scene_commit (classic path).
    widget_window_id: u32,
    /// Optional launch argument (e.g. a file path to open), readable by
    /// the app via `npk_launch_arg`. Set by `npk_open`.
    launch_arg: Option<String>,
}

static WASM_JOBS: Mutex<[Option<WasmJob>; MAX_WASM_JOBS]> =
    Mutex::new([const { None }; MAX_WASM_JOBS]);

/// Per-job completion flag (set by worker, read by BSP)
static JOB_DONE: [core::sync::atomic::AtomicBool; MAX_WASM_JOBS] =
    [const { core::sync::atomic::AtomicBool::new(false) }; MAX_WASM_JOBS];

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

/// Target IP/port for the debug reverse-mirror module. Packed as
/// `(ip as u64) << 16 | port as u64`. Set by the `debug` intent dispatcher
/// before spawning debug.wasm. 0 = unset.
static DEBUG_TARGET: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

pub fn set_debug_target(ip_packed: u32, port: u16) {
    let v = ((ip_packed as u64) << 16) | (port as u64);
    DEBUG_TARGET.store(v, AtOrd::Release);
}

#[derive(Clone, Copy)]
struct BenchCache {
    blake3_mbs: u64,
    aes_enc_mbs: u64,
    aes_dec_mbs: u64,
    raw_write_mbs: u64,
    raw_read_mbs: u64,
}

static BENCH_CACHE: Mutex<Option<BenchCache>> = Mutex::new(None);

fn ensure_bench() -> BenchCache {
    let mut lock = BENCH_CACHE.lock();
    if let Some(b) = *lock { return b; }

    let (blake3_mbs, aes_enc_mbs, aes_dec_mbs) = crate::intent::crypto_bench();
    let (raw_write_mbs, raw_read_mbs) =
        crate::storage::npkfs::storage::raw_blk_bench().unwrap_or((0, 0));

    let result = BenchCache {
        blake3_mbs, aes_enc_mbs, aes_dec_mbs, raw_write_mbs, raw_read_mbs,
    };
    *lock = Some(result);
    result
}

fn fsck_sys_info() -> i64 {
    match crate::storage::npkfs::storage::self_check() {
        Ok(r) => {
            let problems = r.double_alloc + r.out_of_range + r.free_but_referenced;
            crate::kprintln!(
                "[npk] fsck: objects={} nodes={} refd={}/{} | double_alloc={} oor={} free_but_refd={}",
                r.objects, r.btree_nodes, r.referenced, r.total_blocks,
                r.double_alloc, r.out_of_range, r.free_but_referenced);
            if problems == 0 {
                crate::kprintln!("[npk] fsck: CLEAN");
            } else {
                crate::kprintln!(
                    "[npk] fsck: {} PROBLEM(S) — first dup block {}, first oor ptr {}",
                    problems, r.first_dup_block, r.first_oor_ptr);
            }
            problems as i64
        }
        Err(e) => {
            crate::kprintln!("[npk] fsck: scan failed: {:?}", e);
            -1
        }
    }
}

fn bench_sys_info(key: i32) -> i64 {
    let b = ensure_bench();
    match key & 0xFF {
        30 => b.blake3_mbs as i64,
        31 => b.aes_enc_mbs as i64,
        32 => b.aes_dec_mbs as i64,
        33 => b.raw_write_mbs as i64,
        34 => b.raw_read_mbs as i64,
        _ => -1,
    }
}

pub fn get_debug_target() -> (u32, u16) {
    let v = DEBUG_TARGET.load(AtOrd::Acquire);
    ((v >> 16) as u32, (v & 0xFFFF) as u16)
}

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
    spawn_on_worker_inner(wasm_bytes, cap_id, terminal_idx, module_name, true, 0, None)
}

/// Spawn a WASM module as a background task. Unlike spawn_on_worker, this does
/// NOT set APP_RUNNING for the terminal — the intent shell keeps receiving keys
/// and the window continues to function normally. Used by debug.wasm.
pub fn spawn_on_worker_background(wasm_bytes: Vec<u8>, cap_id: CapId, terminal_idx: u8, module_name: &str) -> bool {
    spawn_on_worker_inner(wasm_bytes, cap_id, terminal_idx, module_name, false, 0, None)
}

/// Spawn a widget-kind WASM app (Phase 10). The caller pre-allocates a
/// widget window and passes its id — the worker sets `widget_window_id`
/// in HostState so the first `npk_scene_commit` targets it directly.
/// Does NOT allocate a terminal or set APP_RUNNING — widget apps use
/// `npk_event_poll` for input, not the per-terminal key buffer.
pub fn spawn_widget_app(wasm_bytes: Vec<u8>, cap_id: CapId, module_name: &str, widget_wid: u32) -> bool {
    spawn_on_worker_inner(wasm_bytes, cap_id, 255, module_name, false, widget_wid, None)
}

fn spawn_on_worker_inner(
    wasm_bytes: Vec<u8>, cap_id: CapId, terminal_idx: u8, module_name: &str,
    foreground: bool, widget_wid: u32, launch_arg: Option<String>,
) -> bool {
    let mut jobs = WASM_JOBS.lock();
    let slot = match jobs.iter().position(|j| j.is_none()) {
        Some(i) => i,
        None => { kprintln!("[npk] No free WASM job slots"); return false; }
    };

    let mut name = [0u8; 32];
    let nlen = module_name.len().min(32);
    name[..nlen].copy_from_slice(&module_name.as_bytes()[..nlen]);

    JOB_DONE[slot].store(false, core::sync::atomic::Ordering::Relaxed);
    jobs[slot] = Some(WasmJob {
        bytes: wasm_bytes, cap_id, terminal_idx, name, name_len: nlen as u8,
        widget_window_id: widget_wid, launch_arg,
    });
    drop(jobs);

    // Clear per-app input buffer + mark terminal as having an app (foreground only)
    if foreground {
        clear_app_key_buf(terminal_idx);
        if (terminal_idx as usize) < MAX_APP_BUFS {
            APP_RUNNING[terminal_idx as usize].store(true, AtOrd::Release);
        }
    }

    // Run the app on a fiber (own stack) so it can yield at npk_sleep /
    // npk_event_wait instead of pinning its worker core — see smp::fiber
    // + docs/plan/SCHEDULER_FIBERS.md. Native intents still use plain `spawn`.
    crate::smp::scheduler::spawn_fiber(
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
        hw: None,
        widget_window_id: job.widget_window_id,
        module_name: String::from(name_str),
        launch_arg: job.launch_arg.clone(),
        http_final_url: None,
        http_content_type: None,
        http_last_error: None,
        http_reply_headers: None,
        http_status: 0,
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

    // Cleanup hardware resources before process exit
    cleanup_hw_state(store.data_mut());

    // Drop this instance's per-path grants. They were handed out for one
    // pick and must not outlive the app that got them — a later instance
    // reusing the capability id would otherwise inherit a file it never
    // asked for.
    capability::revoke_path_grants(&store.data().cap_id);

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

/// Execute a WASM module with an explicit fuel budget. Use for trusted
/// first-party modules whose work is deterministically bounded by input
/// parameters (e.g. wallpaper generation sized by resolution).
pub fn execute_sandboxed_with_fuel(
    wasm_bytes: &[u8], func_name: &str, args: &[Val], cap_id: CapId, fuel: u64,
) -> Result<WasmResult, WasmError> {
    execute_inner(wasm_bytes, func_name, args, cap_id, fuel)
}

/// Execute a WASM module in interactive mode (live display).
/// npk_print writes directly to terminal. Used for long-running apps (top).
#[allow(dead_code)]
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
        hw: None,
        widget_window_id: 0,
        module_name: String::new(),
        launch_arg: None,
        http_final_url: None,
        http_content_type: None,
        http_last_error: None,
        http_reply_headers: None,
        http_status: 0,
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
    wasm_bytes: &[u8], func_name: &str, args: &[Val], cap_id: CapId, fuel: u64,
) -> Result<WasmResult, WasmError> {
    // Clone the engine (cheap Arc bump) and drop the ENGINE lock so two
    // one-shot decodes (run/wallpaper) can run concurrently. The resident
    // + `execute` paths already do this; only this one held the lock over
    // the whole instantiate+call.
    let engine = {
        let guard = ENGINE.lock();
        guard.as_ref().ok_or(WasmError::NotInitialized)?.clone()
    };

    let module = Module::new(&engine, wasm_bytes)
        .map_err(|_| WasmError::InvalidModule)?;

    let mut store = Store::new(&engine, HostState {
        output: String::new(),
        cap_id,
        direct_output: false,
        terminal_idx: 255,
        core_id: 0,
        pid: 0,
        hw: None,
        widget_window_id: 0,
        module_name: String::new(),
        launch_arg: None,
        http_final_url: None,
        http_content_type: None,
        http_last_error: None,
        http_reply_headers: None,
        http_status: 0,
    });
    store.set_fuel(fuel).map_err(|_| WasmError::ExecutionFailed)?;

    let mut linker = <Linker<HostState>>::new(&engine);
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

    // npk_log_serial(ptr, len) — write directly to the serial port,
    // bypassing the shade-terminal write path used by kprintln.
    //
    // Needed by widget-only apps (drun) that run when no terminal
    // window exists: kprintln locks SERIAL *and* routes a copy through
    // `shade::terminal::write`, which can stall during early boot or
    // when the active-terminal slot has no backing buffer. Direct
    // serial lives inside the same SERIAL mutex but skips the
    // terminal-side work, so it is safe to call from a worker core
    // regardless of shade state.
    linker.func_wrap("env", "npk_log_serial",
        |caller: Caller<'_, HostState>, ptr: i32, len: i32| {
            if let Some(s) = read_wasm_str(&caller, ptr, len) {
                let serial = crate::drivers::serial::SERIAL.lock();
                for byte in s.bytes() {
                    if byte == b'\n' { serial.write_byte(b'\r'); }
                    serial.write_byte(byte);
                }
                serial.write_byte(b'\r');
                serial.write_byte(b'\n');
            }
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_fetch(name_ptr, name_len, buf_ptr, buf_max) -> bytes or -1
    linker.func_wrap("env", "npk_fetch",
        |mut caller: Caller<'_, HostState>, name_ptr: i32, name_len: i32,
         buf_ptr: i32, buf_max: i32| -> i32 {
            let cap_id = caller.data().cap_id;
            if let Err(e) = capability::check_global(&cap_id, capability::Rights::READ) {
                kprintln!("[npk] WASM: npk_fetch DENIED (cap_id={:08x}, {:?})",
                    capability::short_id(&cap_id), e);
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
            let result = if start + write_len <= data.len() {
                data[start..start + write_len].copy_from_slice(&content[..write_len]);
                write_len as i32
            } else {
                -1
            };

            result
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_http_request(url_ptr, url_len, buf_ptr, buf_max) -> bytes or -1
    // Outbound HTTPS GET for the native browser (beak). Parses the URL,
    // fetches the body (following redirects) via the same TLS path OTA
    // uses, and copies up to buf_max bytes into the caller's buffer.
    // NET-gated — distinct from npkFS READ and from WiFi NETCTL.
    linker.func_wrap("env", "npk_http_request",
        |mut caller: Caller<'_, HostState>, url_ptr: i32, url_len: i32,
         buf_ptr: i32, buf_max: i32| -> i32 {
            let cap_id = caller.data().cap_id;
            if let Err(e) = capability::check_global(&cap_id, capability::Rights::NET) {
                kprintln!("[npk] WASM: npk_http_request DENIED (cap_id={:08x}, {:?})",
                    capability::short_id(&cap_id), e);
                return -1;
            }
            if buf_max <= 0 { return -1; }
            let cap = buf_max as usize;

            let url = match read_wasm_str(&caller, url_ptr, url_len) {
                Some(s) => s,
                None => return -1,
            };
            let (host, path) = match crate::intent::http::parse_url(&url) {
                Ok(hp) => hp,
                Err(e) => {
                    kprintln!("[npk] WASM: npk_http_request bad url: {}", e);
                    return -1;
                }
            };

            let mut out: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
            let mut info = crate::intent::http::FetchInfo::default();
            let res = crate::intent::http::https_get_streaming_ex(
                &host, &path, cap,
                &mut |chunk: &[u8]| -> Result<(), &'static str> {
                    if out.len() < cap {
                        let take = chunk.len().min(cap - out.len());
                        out.extend_from_slice(&chunk[..take]);
                    }
                    Ok(())
                },
                Some(&mut info),
            );
            // A failed request must not leave the previous request's final
            // URL readable as if it were this one's.
            let ok = res.is_ok();
            caller.data_mut().http_final_url =
                if ok && !info.final_url.is_empty() { Some(info.final_url) } else { None };
            // Same rule: a stale Content-Type would make the next document
            // decode against the last one's charset.
            caller.data_mut().http_content_type =
                if ok && !info.content_type.is_empty() { Some(info.content_type) } else { None };
            // Same rule for the reason: cleared on success, so a caller can
            // never read a stale error and attribute it to this request.
            caller.data_mut().http_last_error = match &res {
                Ok(_) => None,
                Err(e) => Some(alloc::format!("{}\t{}", crate::intent::http::error_kind(e), e)),
            };
            if res.is_err() { return -1; }

            let write_len = out.len().min(cap);
            // Bounds-checked write: buf_ptr is guest-controlled, and a
            // wrapping `start + len` would panic the KERNEL on the slice index.
            write_wasm_bytes(&mut caller, buf_ptr, &out[..write_len])
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_http_send(method_ptr, method_len, url_ptr, url_len,
    //               hdrs_ptr, hdrs_len, body_ptr, body_len,
    //               buf_ptr, buf_max) -> bytes, or -1
    //
    // The general request `npk_http_request` was the narrow case of: any
    // method, caller-supplied headers, a request body, and the response's
    // status + headers readable afterwards. That is what a login (POST) and
    // a cookie jar (`Set-Cookie`) need, and neither was expressible before.
    //
    // `hdrs` is newline-separated `Name: value` lines. Cookie POLICY stays
    // out of the kernel — which cookie belongs on which request is RFC 6265,
    // and that is the browser's job; the kernel only carries bytes.
    //
    // A non-2xx does NOT fail here: a 404 page and a 403 explaining itself
    // are documents a person needs to read. The status comes back through
    // `npk_http_status`.
    //
    // NET-gated, same capability as npk_http_request.
    linker.func_wrap("env", "npk_http_send",
        |mut caller: Caller<'_, HostState>, method_ptr: i32, method_len: i32,
         url_ptr: i32, url_len: i32, hdrs_ptr: i32, hdrs_len: i32,
         body_ptr: i32, body_len: i32, buf_ptr: i32, buf_max: i32| -> i32 {
            let cap_id = caller.data().cap_id;
            if let Err(e) = capability::check_global(&cap_id, capability::Rights::NET) {
                kprintln!("[npk] WASM: npk_http_send DENIED (cap_id={:08x}, {:?})",
                    capability::short_id(&cap_id), e);
                return -1;
            }
            if buf_max <= 0 { return -1; }
            let cap = buf_max as usize;

            let method = match read_wasm_str(&caller, method_ptr, method_len) {
                Some(s) => s,
                None => return -1,
            };
            // The method sits at the very front of the request line and the
            // headers end it — a newline in either rewrites the request, and
            // everything after it is read as a SECOND one. This is the check
            // that stops a sandboxed app from smuggling requests through us.
            if !crate::intent::http::method_is_safe(&method) {
                kprintln!("[npk] WASM: npk_http_send rejected method");
                return -1;
            }
            let url = match read_wasm_str(&caller, url_ptr, url_len) {
                Some(s) => s,
                None => return -1,
            };
            let hdr_blob = if hdrs_len > 0 {
                match read_wasm_str(&caller, hdrs_ptr, hdrs_len) {
                    Some(s) => s,
                    None => return -1,
                }
            } else {
                String::new()
            };
            let mut headers: alloc::vec::Vec<String> = alloc::vec::Vec::new();
            for line in hdr_blob.split('\n').map(str::trim).filter(|l| !l.is_empty()) {
                if !crate::intent::http::header_line_is_safe(line) {
                    kprintln!("[npk] WASM: npk_http_send rejected a header");
                    return -1;
                }
                headers.push(String::from(line));
            }
            let body = if body_len > 0 {
                match read_wasm_bytes(&caller, body_ptr, body_len) {
                    Some(b) => b,
                    None => return -1,
                }
            } else {
                alloc::vec::Vec::new()
            };

            let (host, path) = match crate::intent::http::parse_url(&url) {
                Ok(hp) => hp,
                Err(e) => {
                    kprintln!("[npk] WASM: npk_http_send bad url: {}", e);
                    return -1;
                }
            };

            let mut out: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
            let mut info = crate::intent::http::FetchInfo::default();
            let req = crate::intent::http::HttpRequest {
                method: &method, headers: &headers, body: &body,
            };
            let res = crate::intent::http::https_request_streaming(
                &host, &path, &req, cap,
                &mut |chunk: &[u8]| -> Result<(), &'static str> {
                    if out.len() < cap {
                        let take = chunk.len().min(cap - out.len());
                        out.extend_from_slice(&chunk[..take]);
                    }
                    Ok(())
                },
                Some(&mut info),
                true,
            );
            // Same rule as npk_http_request throughout: everything is cleared
            // on failure, so a caller can never read one request's answer and
            // attribute it to the next.
            let ok = res.is_ok();
            caller.data_mut().http_final_url =
                if ok && !info.final_url.is_empty() { Some(info.final_url) } else { None };
            caller.data_mut().http_content_type =
                if ok && !info.content_type.is_empty() { Some(info.content_type) } else { None };
            caller.data_mut().http_reply_headers =
                if ok { Some(info.headers) } else { None };
            caller.data_mut().http_status = if ok { info.status } else { 0 };
            caller.data_mut().http_last_error = match &res {
                Ok(_) => None,
                Err(e) => Some(alloc::format!("{}\t{}", crate::intent::http::error_kind(e), e)),
            };
            if res.is_err() { return -1; }

            let write_len = out.len().min(cap);
            write_wasm_bytes(&mut caller, buf_ptr, &out[..write_len])
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_http_response_headers(buf_ptr, buf_max) -> len, or -1
    // The last npk_http_send response's header block, minus the status line.
    // `Set-Cookie` repeats, so this is a raw block rather than a getter per
    // name. NET-gated.
    linker.func_wrap("env", "npk_http_response_headers",
        |mut caller: Caller<'_, HostState>, buf_ptr: i32, buf_max: i32| -> i32 {
            let cap_id = caller.data().cap_id;
            if capability::check_global(&cap_id, capability::Rights::NET).is_err() {
                return -1;
            }
            if buf_ptr < 0 || buf_max <= 0 { return -1; }
            let hdrs = match &caller.data().http_reply_headers {
                Some(h) => h.clone(),
                None => return -1,
            };
            let n = hdrs.len().min(buf_max as usize);
            let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(m) => m,
                None => return -1,
            };
            let data = mem.data_mut(&mut caller);
            let start = buf_ptr as usize;
            // checked_add: a wrapping `start + n` would panic the KERNEL on
            // the slice index — a guest-triggerable halt.
            match start.checked_add(n) {
                Some(end) if end <= data.len() => {
                    data[start..end].copy_from_slice(&hdrs.as_bytes()[..n]);
                    n as i32
                }
                _ => -1,
            }
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_http_status() -> status, or 0
    // The last npk_http_send response's HTTP status. NET-gated.
    linker.func_wrap("env", "npk_http_status",
        |caller: Caller<'_, HostState>| -> i32 {
            let cap_id = caller.data().cap_id;
            if capability::check_global(&cap_id, capability::Rights::NET).is_err() {
                return 0;
            }
            caller.data().http_status as i32
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_http_request_many(urls_ptr, urls_len, out_ptr, out_max,
    //                       lens_ptr, lens_max) -> count, or -1
    //
    // Fetch many URLs in ONE call, multiplexed over HTTP/2 where the host
    // offers it. `urls` is a newline-separated list; the bodies are written
    // back-to-back into `out`, and `lens` receives one little-endian i32 per
    // URL — the byte count written, or -1 for a resource that failed or did
    // not fit. The guest walks `lens` to slice `out`.
    //
    // Exists because sequential HTTP/1.1 sub-resource fetching is what walks
    // a page into rate limits and spends a round-trip per file. NET-gated,
    // same capability as npk_http_request.
    linker.func_wrap("env", "npk_http_request_many",
        |mut caller: Caller<'_, HostState>, urls_ptr: i32, urls_len: i32,
         out_ptr: i32, out_max: i32, lens_ptr: i32, lens_max: i32| -> i32 {
            let cap_id = caller.data().cap_id;
            if let Err(e) = capability::check_global(&cap_id, capability::Rights::NET) {
                kprintln!("[npk] WASM: npk_http_request_many DENIED (cap_id={:08x}, {:?})",
                    capability::short_id(&cap_id), e);
                return -1;
            }
            if out_max <= 0 || lens_max <= 0 || out_ptr < 0 || lens_ptr < 0 { return -1; }

            let blob = match read_wasm_str(&caller, urls_ptr, urls_len) {
                Some(s) => s,
                None => return -1,
            };
            let urls: alloc::vec::Vec<String> = blob
                .split('\n')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect();
            // Bound the work a single call can ask for, and make sure the
            // guest actually gave us room for one length per URL.
            const MAX_URLS: usize = 64;
            if urls.is_empty() || urls.len() > MAX_URLS { return -1; }
            if (lens_max as usize) < urls.len() * 4 { return -1; }

            let total_cap = out_max as usize;
            let bodies = crate::intent::http::https_get_many(&urls, total_cap);

            let mut blobs: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
            let mut lens: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
            for body in bodies {
                let n: i32 = match body {
                    // Drop a body that would overrun the caller's buffer
                    // rather than truncating it — half an image decodes to
                    // garbage, whereas a missing one draws a placeholder.
                    Some(b) if blobs.len() + b.len() <= total_cap => {
                        blobs.extend_from_slice(&b);
                        b.len() as i32
                    }
                    _ => -1,
                };
                lens.extend_from_slice(&n.to_le_bytes());
            }

            if write_wasm_bytes(&mut caller, lens_ptr, &lens) < 0 { return -1; }
            if !blobs.is_empty() && write_wasm_bytes(&mut caller, out_ptr, &blobs) < 0 { return -1; }
            urls.len() as i32
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_http_final_url(buf_ptr, buf_max) -> len, or -1
    // The URL the last npk_http_request's body actually came from, after
    // redirects. A browser resolves relative sub-resources against this
    // (the document base URL) — resolving against the *requested* URL
    // instead makes every sub-resource repeat the redirect. NET-gated.
    linker.func_wrap("env", "npk_http_final_url",
        |mut caller: Caller<'_, HostState>, buf_ptr: i32, buf_max: i32| -> i32 {
            let cap_id = caller.data().cap_id;
            if capability::check_global(&cap_id, capability::Rights::NET).is_err() {
                return -1;
            }
            if buf_ptr < 0 || buf_max <= 0 { return -1; }
            let url = match &caller.data().http_final_url {
                Some(u) => u.clone(),
                None => return -1,
            };
            let n = url.len().min(buf_max as usize);
            let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(m) => m,
                None => return -1,
            };
            let data = mem.data_mut(&mut caller);
            let start = buf_ptr as usize;
            // checked_add: a wrapping `start + n` would produce start > end and
            // panic the KERNEL on the slice index — a guest-triggerable halt.
            match start.checked_add(n) {
                Some(end) if end <= data.len() => {
                    data[start..end].copy_from_slice(&url.as_bytes()[..n]);
                    n as i32
                }
                _ => -1,
            }
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_http_content_type(buf_ptr, buf_max) -> len, or -1
    //
    // The last npk_http_request's Content-Type, verbatim (e.g.
    // "text/html; charset=ISO-8859-1"). Cleared when the request failed.
    //
    // A document's bytes do not say what encoding they are in. Without this
    // a browser can only assume UTF-8, and one byte that is not valid UTF-8
    // costs it the entire page — which is exactly what made google.ch render
    // blank. NET-gated, like the request it describes.
    linker.func_wrap("env", "npk_http_content_type",
        |mut caller: Caller<'_, HostState>, buf_ptr: i32, buf_max: i32| -> i32 {
            let cap_id = caller.data().cap_id;
            if capability::check_global(&cap_id, capability::Rights::NET).is_err() {
                return -1;
            }
            if buf_ptr < 0 || buf_max <= 0 { return -1; }
            let ct = match &caller.data().http_content_type {
                Some(c) => c.clone(),
                None => return -1,
            };
            let n = ct.len().min(buf_max as usize);
            let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(m) => m,
                None => return -1,
            };
            let data = mem.data_mut(&mut caller);
            let start = buf_ptr as usize;
            // checked_add: a wrapping `start + n` would produce start > end and
            // panic the KERNEL on the slice index — a guest-triggerable halt.
            match start.checked_add(n) {
                Some(end) if end <= data.len() => {
                    data[start..end].copy_from_slice(&ct.as_bytes()[..n]);
                    n as i32
                }
                _ => -1,
            }
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_http_last_error(buf_ptr, buf_max) -> len, or -1
    //
    // Why the last npk_http_request failed: `kind\tmessage`, where kind is
    // a stable token (`cert.untrusted`, `cert.expired`, `net.connect`, …)
    // and message is the human wording. Cleared on success.
    //
    // Exists because the request itself can only answer "no": every failure
    // arrives as -1, so a browser could not tell a rejected certificate from
    // an empty document and drew nothing either way. NET-gated, like the
    // request whose outcome it describes.
    linker.func_wrap("env", "npk_http_last_error",
        |mut caller: Caller<'_, HostState>, buf_ptr: i32, buf_max: i32| -> i32 {
            let cap_id = caller.data().cap_id;
            if capability::check_global(&cap_id, capability::Rights::NET).is_err() {
                return -1;
            }
            if buf_ptr < 0 || buf_max <= 0 { return -1; }
            let err = match &caller.data().http_last_error {
                Some(e) => e.clone(),
                None => return -1,
            };
            let n = err.len().min(buf_max as usize);
            let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(m) => m,
                None => return -1,
            };
            let data = mem.data_mut(&mut caller);
            let start = buf_ptr as usize;
            // checked_add: a wrapping `start + n` would produce start > end and
            // panic the KERNEL on the slice index — a guest-triggerable halt.
            match start.checked_add(n) {
                Some(end) if end <= data.len() => {
                    data[start..end].copy_from_slice(&err.as_bytes()[..n]);
                    n as i32
                }
                _ => -1,
            }
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_store(name_ptr, name_len, data_ptr, data_len) -> 0 or -1
    linker.func_wrap("env", "npk_store",
        |caller: Caller<'_, HostState>, name_ptr: i32, name_len: i32,
         data_ptr: i32, data_len: i32| -> i32 {
            let cap_id = caller.data().cap_id;
            let name = match read_wasm_str(&caller, name_ptr, name_len) {
                Some(s) => s,
                None => return -1,
            };

            // Apps may not write the module store or the trust store — see
            // is_trust_critical_path.
            // Checked BEFORE the grant path so a per-file grant can never
            // become a way in there.
            if is_trust_critical_path(&name) {
                kprintln!("[npk] WASM: npk_store DENIED ({} is read-only to apps)", name);
                return -1;
            }

            // Three ways to be allowed to write, narrowest last:
            //   1. blanket WRITE from `.npk.caps`
            //   2. a grant for exactly this file — what the user handed over
            //      by picking the path in a trusted dialog
            //   3. the app's OWN settings file, `sys/config/<module>`. An
            //      app that keeps preferences shouldn't need write access to
            //      the whole store for it, and the name is the kernel's to
            //      derive — a module can't claim someone else's.
            let own_config = alloc::format!("sys/config/{}", caller.data().module_name);
            if capability::check_global(&cap_id, capability::Rights::WRITE).is_err()
                && !capability::check_path_grant(&cap_id, &name, capability::Rights::WRITE)
                && name != own_config
            {
                kprintln!("[npk] WASM: npk_store DENIED (no WRITE, no grant for {})", name);
                return -1;
            }

            let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(m) => m,
                None => return -1,
            };
            let data = mem.data(&caller);
            let start = data_ptr as usize;
            let end = (start + data_len as usize).min(data.len());
            if start >= end { return -1; }

            // Insert-or-replace: apps with state to persist (panel
            // configs, etc.) re-write the same key on every change. The
            // strict-create `store` would fail on the second write and
            // leave the app's state diverging from disk.
            match crate::npkfs::upsert(&name, &data[start..end], cap_id) {
                Ok(_) => 0,
                Err(_) => -1,
            }
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_home_dir(buf_ptr, buf_max) -> i32
    // Write the current user's home directory ("home/<name>", or "home"
    // if unset) into the caller's buffer; returns bytes written or -1.
    // Apps need this because the username lives in the single encrypted
    // `.system/config` blob, not a fetchable `sys/config/name` object —
    // so they can't derive their home/documents path on their own.
    // READ-gated (it reveals the user identity from config).
    linker.func_wrap("env", "npk_home_dir",
        |mut caller: Caller<'_, HostState>, buf_ptr: i32, buf_max: i32| -> i32 {
            let cap_id = caller.data().cap_id;
            if capability::check_global(&cap_id, capability::Rights::READ).is_err() {
                return -1;
            }
            let home = crate::intent::home_dir();
            let bytes = home.as_bytes();
            if bytes.len() > buf_max as usize { return -1; }
            let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(m) => m,
                None => return -1,
            };
            let data = mem.data_mut(&mut caller);
            let start = buf_ptr as usize;
            let end = start + bytes.len();
            if end > data.len() { return -1; }
            data[start..end].copy_from_slice(bytes);
            bytes.len() as i32
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_fs_usage() -> i64
    // Filesystem fill level as (used_mib << 32) | total_mib, or -1 when
    // nothing is mounted. Feeds the file browser's capacity meter.
    // READ-gated — it says how much of the disk is in use.
    linker.func_wrap("env", "npk_fs_usage",
        |caller: Caller<'_, HostState>| -> i64 {
            let cap_id = caller.data().cap_id;
            if capability::check_global(&cap_id, capability::Rights::READ).is_err() {
                return -1;
            }
            let Some((total_blocks, free_blocks, _, _)) = crate::npkfs::stats() else {
                return -1;
            };
            let block = crate::npkfs::BLOCK_SIZE as u64;
            let to_mib = |blocks: u64| (blocks.saturating_mul(block)) >> 20;
            let total = to_mib(total_blocks);
            let used = to_mib(total_blocks.saturating_sub(free_blocks));
            (((used & 0xFFFF_FFFF) << 32) | (total & 0xFFFF_FFFF)) as i64
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_locale(buf_ptr, buf_max) -> i32
    // Write the UI language code (`lang` config key, e.g. "en" / "de")
    // into the caller's buffer; returns bytes written or -1. Defaults to
    // "en". The kernel stores the code only — the catalogs live in the
    // apps, so adding a language never touches the kernel.
    //
    // Deliberately ungated: which language to draw labels in is a display
    // preference, not access to data. Gating it on READ would force a
    // render-only app (beak declares RENDER|CANVAS|NET, no filesystem
    // read) to take a filesystem capability just to spell its own menu —
    // and a failed call falls back to English, so the symptom is one app
    // silently out of language with the rest of the desktop.
    linker.func_wrap("env", "npk_locale",
        |mut caller: Caller<'_, HostState>, buf_ptr: i32, buf_max: i32| -> i32 {
            let lang = crate::config::get("lang").unwrap_or_default();
            let lang = lang.trim();
            let bytes = if lang.is_empty() { b"en".as_slice() } else { lang.as_bytes() };
            if buf_max < 0 || bytes.len() > buf_max as usize { return -1; }
            let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(m) => m,
                None => return -1,
            };
            let data = mem.data_mut(&mut caller);
            let Ok(start) = usize::try_from(buf_ptr) else { return -1 };
            let Some(end) = start.checked_add(bytes.len()) else { return -1 };
            if end > data.len() { return -1; }
            data[start..end].copy_from_slice(bytes);
            bytes.len() as i32
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_launch_arg(buf_ptr, buf_max) -> i32
    // Read the launch argument the app was started with (e.g. a file
    // path passed by npk_open). Returns bytes written, 0 if none, -1 on
    // error. Apps call this once at startup.
    linker.func_wrap("env", "npk_launch_arg",
        |mut caller: Caller<'_, HostState>, buf_ptr: i32, buf_max: i32| -> i32 {
            let arg = match &caller.data().launch_arg {
                Some(s) => s.clone(),
                None => return 0,
            };
            let bytes = arg.as_bytes();
            if bytes.len() > buf_max as usize { return -1; }
            let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(m) => m,
                None => return -1,
            };
            let data = mem.data_mut(&mut caller);
            let start = buf_ptr as usize;
            let end = start + bytes.len();
            if end > data.len() { return -1; }
            data[start..end].copy_from_slice(bytes);
            bytes.len() as i32
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // ── Clipboard (cross-app copy/paste) ──────────────────────────────
    //
    // A single kernel-owned selection buffer (crate::shade::clipboard).
    // Gated on RENDER + focus: only the *currently focused* widget app may
    // read or write it — a background app cannot snoop the clipboard, the
    // same focus-ambient contract as receiving keystrokes. (When a future
    // third-party app store lands, promote clipboard-read to a declared
    // CLIPBOARD cap — needs a 2nd `.npk.caps` byte; the 1-byte section is
    // full today.)

    // npk_clipboard_set(ptr, len) -> i32
    // Copy `len` UTF-8 bytes from guest memory into the clipboard as Text.
    // Returns bytes stored, or -1 (denied / not focused / bad ptr).
    linker.func_wrap("env", "npk_clipboard_set",
        |caller: Caller<'_, HostState>, ptr: i32, len: i32| -> i32 {
            let cap_id = caller.data().cap_id;
            if capability::check_global(&cap_id, capability::Rights::RENDER).is_err() {
                return -1;
            }
            if !app_is_focused(&caller) { return -1; }
            let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(m) => m,
                None => return -1,
            };
            let data = mem.data(&caller);
            let start = ptr as usize;
            let end = (start + len.max(0) as usize).min(data.len());
            if start > end { return -1; }
            let slice = &data[start..end];
            crate::shade::clipboard::set_text(slice);
            slice.len() as i32
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_clipboard_len() -> i32
    // Byte length of the current clipboard text (0 if empty). Lets an app
    // size its buffer before npk_clipboard_get. Focus-gated like the rest.
    linker.func_wrap("env", "npk_clipboard_len",
        |caller: Caller<'_, HostState>| -> i32 {
            let cap_id = caller.data().cap_id;
            if capability::check_global(&cap_id, capability::Rights::RENDER).is_err() {
                return -1;
            }
            if !app_is_focused(&caller) { return -1; }
            crate::shade::clipboard::text_len() as i32
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_clipboard_get(ptr, max) -> i32
    // Write up to `max` clipboard bytes into the guest buffer. Returns the
    // FULL text length (so the app can detect truncation and re-query with
    // a bigger buffer), 0 if empty, or -1 (denied / not focused / bad ptr).
    linker.func_wrap("env", "npk_clipboard_get",
        |mut caller: Caller<'_, HostState>, ptr: i32, max: i32| -> i32 {
            let cap_id = caller.data().cap_id;
            if capability::check_global(&cap_id, capability::Rights::RENDER).is_err() {
                return -1;
            }
            if !app_is_focused(&caller) { return -1; }
            let text = match crate::shade::clipboard::get_text() {
                Some(t) => t,
                None => return 0,
            };
            let n = text.len().min(max.max(0) as usize);
            let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(m) => m,
                None => return -1,
            };
            let data = mem.data_mut(&mut caller);
            let start = ptr as usize;
            let end = start + n;
            if end > data.len() { return -1; }
            data[start..end].copy_from_slice(&text[..n]);
            text.len() as i32
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_open(app_ptr, app_len, arg_ptr, arg_len) -> i32
    // Launch widget module `app` (sys/wasm/<app>) with `arg` as its launch
    // argument (read by the app via npk_launch_arg). The launched app gets
    // its own per-app caps (from its `.npk.caps` section) and a fresh
    // window. EXECUTE-gated (launch authority). Used by loft for file
    // associations — open a file with its handler app. The kernel stays
    // generic: the ext→app mapping lives in the caller (loft + config),
    // never here.
    linker.func_wrap("env", "npk_open",
        |caller: Caller<'_, HostState>, app_ptr: i32, app_len: i32, arg_ptr: i32, arg_len: i32| -> i32 {
            let cap_id = caller.data().cap_id;
            if capability::check_global(&cap_id, capability::Rights::EXECUTE).is_err() {
                return -1;
            }
            let app = match read_wasm_str(&caller, app_ptr, app_len) {
                Some(s) => s,
                None => return -1,
            };
            // Module name only — no path traversal into the store.
            if app.is_empty() || app.contains('/') || app.contains("..") { return -1; }
            let arg = if arg_len > 0 { read_wasm_str(&caller, arg_ptr, arg_len) } else { None };

            // Singleton + tabs: if the target app already has a widget
            // window (titled with its module name), deliver the open as an
            // Event::Open to that instance and focus it instead of
            // spawning a duplicate. Only when there's something to open.
            if let Some(arg_str) = arg.clone() {
                let existing = crate::shade::with_compositor(|c| {
                    c.windows.iter()
                        .find(|w| w.kind == crate::shade::window::WindowKind::Widget
                            && w.title == app)
                        .map(|w| w.id)
                }).flatten();
                if let Some(id) = existing {
                    // Same deal as a pick: the user pointed at this file
                    // (a double-click in the file manager), so the app may
                    // read and save it — and nothing else.
                    if let Some(cap) = crate::shade::widgets::window_cap(id.0) {
                        capability::grant_path(cap, &arg_str,
                            capability::Rights::READ | capability::Rights::WRITE);
                    }
                    crate::shade::widgets::push_event(
                        id.0, crate::shade::widgets::abi::Event::Open(arg_str));
                    crate::shade::with_compositor(|c| c.focus_window(id));
                    crate::shade::request_render();
                    return 0;
                }
            }

            let path = alloc::format!("sys/wasm/{}", app);
            let bytes = match crate::npkfs::fetch(&path) {
                Ok((b, _)) => b,
                Err(_) => return -1,
            };
            let rights = capability::widget_rights_from_wasm(&bytes);
            let module_cap = match capability::create_module_cap(rights, Some(600_000)) {
                Ok(id) => id,
                Err(_) => return -1,
            };
            // Launching an app ON a file is the user pointing at it — grant
            // that one path so the app can save it back without holding
            // WRITE over the whole store.
            if let Some(a) = arg.as_deref() {
                capability::grant_path(module_cap, a,
                    capability::Rights::READ | capability::Rights::WRITE);
            }
            // Create the widget window NOW (synchronously, titled with the
            // module name) instead of lazily on first scene_commit. The
            // app spawns asynchronously, so without this a rapid second
            // open (e.g. a double-click = two opens) would see no window
            // yet and spawn a DUPLICATE instance. Pre-creating lets the
            // next open find it and route an Event::Open tab instead.
            let win = match crate::shade::with_compositor(|c| c.create_widget_window(&app)) {
                Some(id) => id,
                None => return -1,
            };
            crate::shade::focus_window(win); // bring the editor to the front
            crate::shade::request_render();
            if spawn_on_worker_inner(bytes, module_cap, 255, &app, false, win.0, arg) { 0 } else { -1 }
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_launch(app_ptr, app_len, arg_ptr, arg_len) -> 0 / -1
    // Fire-and-forget launch of sys/wasm/<app> with `arg` as its launch
    // argument + per-app caps — like npk_open but WITHOUT a pre-created
    // window and WITHOUT singleton routing. The window (if any) is created
    // lazily on the app's first scene_commit, so a one-shot tool that
    // never commits (e.g. a full-screen screenshot) never shows a window
    // — and so never appears in its own capture. EXECUTE-gated.
    linker.func_wrap("env", "npk_launch",
        |caller: Caller<'_, HostState>, app_ptr: i32, app_len: i32, arg_ptr: i32, arg_len: i32| -> i32 {
            let cap_id = caller.data().cap_id;
            if capability::check_global(&cap_id, capability::Rights::EXECUTE).is_err() {
                return -1;
            }
            let app = match read_wasm_str(&caller, app_ptr, app_len) {
                Some(s) => s,
                None => return -1,
            };
            if app.is_empty() || app.contains('/') || app.contains("..") { return -1; }
            let arg = if arg_len > 0 { read_wasm_str(&caller, arg_ptr, arg_len) } else { None };
            let path = alloc::format!("sys/wasm/{}", app);
            let bytes = match crate::npkfs::fetch(&path) {
                Ok((b, _)) => b,
                Err(_) => return -1,
            };
            let rights = capability::widget_rights_from_wasm(&bytes);
            let module_cap = match capability::create_module_cap(rights, Some(600_000)) {
                Ok(id) => id,
                Err(_) => return -1,
            };
            if spawn_on_worker_inner(bytes, module_cap, 255, &app, false, 0, arg) { 0 } else { -1 }
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_pick(mode, start_ptr, start_len, suggest_ptr, suggest_len, tag) -> i32
    // Open a file dialog and get the answer back as `Event::Picked`.
    //
    //   mode 0 = open an existing file, 1 = choose a save target
    //   start   = directory to open in ("" → the user's home)
    //   suggest = pre-filled filename, save mode only
    //   tag     = returned unchanged in the event (the roundtrip is async,
    //             so an app with several dialogs tells them apart by it)
    //
    // The picker module is named by `sys/config/picker` (default `pick`),
    // NEVER by the caller: the whole point is that the dialog is a piece
    // of trusted UI the requester cannot substitute. So this is RENDER-
    // gated, not EXECUTE-gated — asking for a dialog must not require the
    // right to launch arbitrary modules, or an app would need MORE
    // authority to pick a file than to write one.
    //
    // The requester needs no READ to browse: the picker does the listing
    // in its own sandbox and hands back a single path.
    //
    //   0  → dialog opened
    //   -1 → cap denied / no window / bad args / picker module missing
    //   -2 → this app already has a dialog open
    linker.func_wrap("env", "npk_pick",
        |caller: Caller<'_, HostState>, mode: i32, start_ptr: i32, start_len: i32,
         suggest_ptr: i32, suggest_len: i32, tag: i32| -> i32 {
            let cap_id = caller.data().cap_id;
            if capability::check_global(&cap_id, capability::Rights::RENDER).is_err() {
                return -1;
            }
            if mode != 0 && mode != 1 { return -1; }

            // Only a windowed app can receive the reply event.
            let requester = caller.data().widget_window_id;
            if requester == 0 { return -1; }
            if crate::shade::widgets::has_open_pick(requester) { return -2; }

            let start = if start_len > 0 {
                read_wasm_str(&caller, start_ptr, start_len).unwrap_or_default()
            } else {
                String::new()
            };
            let suggest = if suggest_len > 0 {
                read_wasm_str(&caller, suggest_ptr, suggest_len).unwrap_or_default()
            } else {
                String::new()
            };
            // A start dir is a hint, not authority — the picker re-resolves
            // it and the user can navigate anywhere regardless.
            let start = if start.trim().is_empty() || start.contains("..") {
                crate::intent::home_dir()
            } else {
                start
            };
            // The suggestion is a bare filename; a path here would let a
            // caller pre-aim the save at a directory the user never saw.
            let suggest = if suggest.contains('/') || suggest.contains("..") {
                String::new()
            } else {
                suggest
            };

            let module = picker_module_name();
            let path = alloc::format!("sys/wasm/{}", module);
            let bytes = match crate::npkfs::fetch(&path) {
                Ok((b, _)) => b,
                Err(_) => {
                    kprintln!("[npk] npk_pick: picker module `{}` not installed", module);
                    return -1;
                }
            };
            let rights = capability::widget_rights_from_wasm(&bytes);
            let module_cap = match capability::create_module_cap(rights, Some(600_000)) {
                Ok(id) => id,
                Err(_) => return -1,
            };

            // Wire the request as the launch argument:
            //   "<open|save>\0<start-dir>\0<suggested-name>"
            let arg = alloc::format!("{}\0{}\0{}",
                if mode == 1 { "save" } else { "open" }, start, suggest);

            // Floating + centred, like the launcher — a dialog must not
            // re-tile the workspace behind it.
            let win = match crate::shade::with_compositor(|c| {
                let id = c.create_widget_window(&module);
                c.set_overlay(id, PICKER_W, PICKER_H);
                id
            }) {
                Some(id) => id,
                None => return -1,
            };
            crate::shade::widgets::register_pick(win.0, requester, tag as u32, cap_id, mode == 1);
            crate::shade::focus_window(win);
            crate::shade::request_render();

            if spawn_on_worker_inner(bytes, module_cap, 255, &module, false, win.0, Some(arg)) {
                0
            } else {
                // Undo the half-open session, else the requester can never
                // ask again (has_open_pick would keep saying "one is up").
                if let Some(s) = crate::shade::widgets::take_pick(win.0) {
                    crate::shade::widgets::finish_pick(s, String::new());
                }
                -1
            }
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_pick_result(path_ptr, path_len) -> 0 / -1
    // Report the picked path back to whoever opened this dialog. An empty
    // path means the user cancelled.
    //
    // Authorisation is structural: the caller's window id must be one the
    // kernel itself registered as a picker in `npk_pick`. An ordinary app
    // calling this finds no session and gets -1, so it cannot forge a
    // "the user chose this file" claim for another app.
    linker.func_wrap("env", "npk_pick_result",
        |caller: Caller<'_, HostState>, path_ptr: i32, path_len: i32| -> i32 {
            let me = caller.data().widget_window_id;
            if me == 0 { return -1; }
            let session = match crate::shade::widgets::take_pick(me) {
                Some(s) => s,
                None => return -1,
            };
            let path = if path_len > 0 {
                read_wasm_str(&caller, path_ptr, path_len).unwrap_or_default()
            } else {
                String::new()
            };
            crate::shade::widgets::finish_pick(session, path);
            // Hand focus back to the app that asked, so the user carries on
            // where they left off instead of on a closing dialog.
            crate::shade::focus_window(crate::shade::window::WindowId(session.requester));
            crate::shade::request_render();
            0
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_window_set_close_guard(on) -> 0 / -1
    // Ask to be consulted before this window closes. A guarded window gets
    // `Event::CloseRequest` on Mod+Q / the title-bar X instead of vanishing,
    // so an app with unsaved work can prompt.
    //
    // Not a veto: asking again, or staying silent for a few seconds, closes
    // it anyway. Apps that don't opt in are unaffected.
    linker.func_wrap("env", "npk_window_set_close_guard",
        |caller: Caller<'_, HostState>, on: i32| -> i32 {
            let cap_id = caller.data().cap_id;
            if capability::check_global(&cap_id, capability::Rights::RENDER).is_err() {
                return -1;
            }
            let wid = caller.data().widget_window_id;
            if wid == 0 { return -1; }
            crate::shade::widgets::set_close_guard(wid, on != 0);
            0
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_pick_mkdir(path_ptr, path_len) -> 0 / -1
    // Create a directory on behalf of an open file dialog.
    //
    // This exists so the picker can offer "New folder" WITHOUT holding
    // WRITE. Giving it WRITE would hand the module that browses every
    // file the right to overwrite them too — the one thing the portal is
    // built to avoid. So the capability is this single verb instead:
    // create a directory, nothing else. No writing files, no deleting,
    // no renaming.
    //
    // Authorised exactly like `npk_pick_result` — the caller's window
    // must be one the kernel itself registered as a picker. `sys/` stays
    // off limits regardless.
    linker.func_wrap("env", "npk_pick_mkdir",
        |caller: Caller<'_, HostState>, path_ptr: i32, path_len: i32| -> i32 {
            let me = caller.data().widget_window_id;
            if me == 0 || !crate::shade::widgets::is_open_pick(me) { return -1; }
            let path = match read_wasm_str(&caller, path_ptr, path_len) {
                Some(s) => s,
                None => return -1,
            };
            let clean = path.trim().trim_matches('/');
            if clean.is_empty() || clean.contains("..") { return -1; }
            if is_trust_critical_path(clean) || clean == "sys" || clean.starts_with("sys/") {
                kprintln!("[npk] npk_pick_mkdir DENIED (sys is off limits)");
                return -1;
            }
            match crate::npkfs::fs::mkdir(clean) {
                Ok(()) => 0,
                Err(_) => -1,
            }
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_scene_commit(ptr, len) -> i32
    // Phase 10 widget pipeline: WASM app hands the kernel a version-
    // prefixed postcard-serialized Widget tree. Compositor does the
    // rest (version check, deserialize, layout, raster, per-window
    // scene store, shade render). Requires RENDER right.
    //
    // Return protocol mirrors shade::widgets::scene_commit:
    //   >0 → new widget window created, id returned (caller should
    //        treat return value as opaque)
    //   0  → reused existing widget window
    //   -1 → version mismatch / cap denied / bad payload
    //   -2 → postcard decode failure
    //   -3 → shade couldn't allocate a window
    linker.func_wrap("env", "npk_scene_commit",
        |mut caller: Caller<'_, HostState>, ptr: i32, len: i32| -> i32 {
            let cap_id = caller.data().cap_id;
            if capability::check_global(&cap_id, capability::Rights::RENDER).is_err() {
                kprintln!("[npk] WASM: npk_scene_commit DENIED (no RENDER)");
                return -1;
            }
            // Remember which capability owns this window, so a later grant
            // (loft opening a file in an already-running editor) can find it.
            let owner_wid = caller.data().widget_window_id;
            if owner_wid != 0 { crate::shade::widgets::set_window_cap(owner_wid, cap_id); }

            let (bytes_start, bytes_end) = {
                let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                    Some(m) => m,
                    None => return -1,
                };
                let data = mem.data(&caller);
                let start = ptr as usize;
                let end = start.saturating_add(len as usize).min(data.len());
                if start >= end { return -1; }
                (start, end)
            };

            // Extract the payload into a heap copy before re-borrowing
            // caller mutably. This is 200–600 bytes for typical trees.
            let payload: alloc::vec::Vec<u8> = {
                let mem = caller.get_export("memory")
                    .and_then(|e| e.into_memory())
                    .expect("memory export vanished mid-call");
                mem.data(&caller)[bytes_start..bytes_end].to_vec()
            };

            let mut prev_window = caller.data().widget_window_id;

            // First commit from a module that was spawned as a terminal:
            // promote that terminal to a widget in place so the app only
            // owns one window (not a terminal + a widget side-by-side).
            if prev_window == 0 {
                let terminal_idx = caller.data().terminal_idx;
                if terminal_idx != 255 {
                    if let Some(promoted) = crate::shade::with_compositor(|c|
                        c.promote_terminal_to_widget(terminal_idx)
                    ).flatten() {
                        caller.data_mut().widget_window_id = promoted.0;
                        caller.data_mut().terminal_idx = 255;
                        prev_window = promoted.0;
                    }
                }
            }

            let module_name = caller.data().module_name.clone();
            let result = crate::shade::widgets::scene_commit(&payload, prev_window, &module_name);

            // Positive return = newly allocated window id → store so
            // subsequent commits from this app reuse the same slot.
            if result > 0 && caller.data().widget_window_id == 0 {
                caller.data_mut().widget_window_id = result as u32;
            }
            // Collapse "new-window id" into success for the callee —
            // the WASM ABI contract is that any non-negative return
            // means "commit accepted". Negatives still propagate.
            if result < 0 { result } else { 0 }
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_canvas_commit(canvas_id, ptr, len, width, height) -> 0 / -1
    // P10.10 escape hatch: upload a raw BGRA32 bitmap into the app's
    // `Widget::Canvas` with the matching id. CANVAS-gated. The app must
    // already own a widget window (commit a scene first) — the bitmap is
    // keyed by (window_id, canvas_id); the render walker blits it
    // contain-fit into the canvas rect on the next rasterise.
    linker.func_wrap("env", "npk_canvas_commit",
        |caller: Caller<'_, HostState>, canvas_id: i32, ptr: i32, len: i32,
         width: i32, height: i32| -> i32 {
            let cap_id = caller.data().cap_id;
            if capability::check_global(&cap_id, capability::Rights::CANVAS).is_err() {
                return -1;
            }
            let wid = caller.data().widget_window_id;
            if wid == 0 || width <= 0 || height <= 0 { return -1; }
            let pixel_bytes = (width as usize) * (height as usize) * 4;
            if len as usize != pixel_bytes { return -1; }
            let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(m) => m,
                None => return -1,
            };
            let data = mem.data(&caller);
            let start = ptr as usize;
            let end = start + pixel_bytes;
            if end > data.len() { return -1; }
            let px = data[start..end].to_vec();
            if !crate::shade::widgets::canvas::commit(
                wid, canvas_id as u32, width as u32, height as u32, px) {
                return -1;
            }
            crate::shade::widgets::rerender_window(wid);
            crate::shade::request_render();
            0
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_screen_size() -> (width << 16) | height, or 0 on error.
    // Allowed for RENDER (overlay sizing) OR CAPTURE (screenshot tool
    // sizing its capture buffer — it has no RENDER in full-screen mode).
    // (Screens are well under 65535 px/side.)
    linker.func_wrap("env", "npk_screen_size",
        |caller: Caller<'_, HostState>| -> i32 {
            let cap_id = caller.data().cap_id;
            let ok = capability::check_global(&cap_id, capability::Rights::RENDER).is_ok()
                || capability::check_global(&cap_id, capability::Rights::CAPTURE).is_ok();
            if !ok { return 0; }
            let info = crate::framebuffer::get_info();
            (((info.width & 0xFFFF) << 16) | (info.height & 0xFFFF)) as i32
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_ticks() -> milliseconds since boot (monotonic), or -1.
    //
    // A clock, not a calendar: no wall time, no timezone, nothing that
    // identifies the machine — so it needs no capability, like the theme
    // query. Resolution is the 100 Hz timer, i.e. 10 ms steps; enough to
    // attribute phases of a page load, not enough to time a single glyph.
    //
    // Stage 1 needs this anyway for `setTimeout`/`requestAnimationFrame`
    // (docs/spec/BROWSER.md §10 lists `now_ms` in the Platform surface).
    linker.func_wrap("env", "npk_ticks",
        |_caller: Caller<'_, HostState>| -> i64 {
            (crate::interrupts::ticks() as i64).saturating_mul(10)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_now_us() -> microseconds since boot, from the TSC, or 0.
    //
    // Same "clock, not calendar" argument as npk_ticks, so equally ungated — but
    // fine enough to time ONE pass of a driver loop, which 10 ms steps cannot.
    // That resolution is the whole difference between "the driver is busy" and
    // "the driver is waiting": the WiFi driver's own busy counter only ever said
    // whether a pass found work, and got read as CPU load (by me).
    linker.func_wrap("env", "npk_now_us",
        |_caller: Caller<'_, HostState>| -> i64 {
            let f = crate::interrupts::tsc_freq();
            if f == 0 { return 0; }
            ((crate::interrupts::rdtsc() as u128 * 1_000_000) / f as u128) as i64
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_unix_time() -> seconds since the epoch, UTC, or 0 if the clock is
    // not readable. Ungated, like npk_ticks: the wall clock is not a secret —
    // it is on the bar and stamped into every npkFS entry. `npk_ticks` cannot
    // stand in for it, because it restarts at every boot and a cookie's
    // `Expires` is an absolute date.
    linker.func_wrap("env", "npk_unix_time",
        |_caller: Caller<'_, HostState>| -> i64 {
            crate::rtc::read_unix_time().unwrap_or(0) as i64
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_theme_token(token_id) -> RGBA u32 (0xAARRGGBB) for the ACTIVE theme
    // (light/dark aware), or 0 for an unknown token. RENDER-gated. Lets an app
    // that paints its own surface (e.g. the browser's Canvas) match the theme's
    // colours instead of hardcoding them.
    linker.func_wrap("env", "npk_theme_token",
        |caller: Caller<'_, HostState>, token_id: i32| -> i32 {
            let cap_id = caller.data().cap_id;
            if capability::check_global(&cap_id, capability::Rights::RENDER).is_err() {
                return 0;
            }
            use crate::shade::widgets::abi::Token;
            let token = match token_id {
                0 => Token::Surface,
                1 => Token::SurfaceElevated,
                2 => Token::SurfaceMuted,
                3 => Token::OnSurface,
                4 => Token::OnSurfaceMuted,
                5 => Token::OnAccent,
                6 => Token::Accent,
                7 => Token::AccentMuted,
                8 => Token::Border,
                9 => Token::Success,
                10 => Token::Warning,
                11 => Token::Danger,
                _ => return 0,
            };
            crate::shade::widgets::palette::resolve(token) as i32
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_canvas_rect(canvas_id, out_ptr) -> 0 / -1
    // Writes the canvas widget's actual laid-out rect as 4 little-endian i32
    // [x, y, w, h] into out_ptr (16 bytes), so an app can paint its canvas
    // 1:1 (no contain-fit scaling) and map click coordinates into content
    // space. RENDER-gated. Returns -1 until the canvas has been laid out once
    // (commit a scene with the Canvas first, then query on the next frame).
    linker.func_wrap("env", "npk_canvas_rect",
        |mut caller: Caller<'_, HostState>, canvas_id: i32, out_ptr: i32| -> i32 {
            let cap_id = caller.data().cap_id;
            if capability::check_global(&cap_id, capability::Rights::RENDER).is_err() {
                return -1;
            }
            let wid = caller.data().widget_window_id;
            if wid == 0 {
                return -1;
            }
            let (x, y, w, h) = match crate::shade::widgets::canvas::rect_of(wid, canvas_id as u32) {
                Some(r) => r,
                None => return -1,
            };
            let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(m) => m,
                None => return -1,
            };
            let data = mem.data_mut(&mut caller);
            let start = out_ptr as usize;
            if start + 16 > data.len() {
                return -1;
            }
            data[start..start + 4].copy_from_slice(&x.to_le_bytes());
            data[start + 4..start + 8].copy_from_slice(&y.to_le_bytes());
            data[start + 8..start + 12].copy_from_slice(&(w as i32).to_le_bytes());
            data[start + 12..start + 16].copy_from_slice(&(h as i32).to_le_bytes());
            0
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_cursor_pos() -> (x << 16) | y, or -1
    //
    // Screen coordinates, the same space `Event::MouseMove` and
    // `Event::MouseButton` report. RENDER-gated AND focus-gated: an app
    // may learn where the pointer is only while it holds focus, so a
    // background module cannot watch the mouse.
    //
    // Exists because `Event::Wheel` carries no position — an app that
    // wants to zoom towards the pointer has to ask for it.
    linker.func_wrap("env", "npk_cursor_pos",
        |caller: Caller<'_, HostState>| -> i32 {
            let cap_id = caller.data().cap_id;
            if capability::check_global(&cap_id, capability::Rights::RENDER).is_err() {
                return -1;
            }
            let wid = caller.data().widget_window_id;
            if wid == 0 { return -1; }
            if crate::shade::focused_widget_id() != Some(wid) { return -1; }
            let (x, y) = crate::shade::cursor::atomic_pos();
            if x < 0 || y < 0 || x > 0xFFFF || y > 0xFFFF { return -1; }
            (x << 16) | y
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_screen_flash() -> 0 or -1
    // CAPTURE-gated: same right as reading the screen, because this is
    // the acknowledgement for exactly that act. Paints a white wash over
    // the finished frame for ~150 ms. The caller is expected to capture
    // FIRST and flash after, so the wash can never be in the shot; the
    // compositor also draws it last, after every window.
    linker.func_wrap("env", "npk_screen_flash",
        |caller: Caller<'_, HostState>| -> i32 {
            let cap_id = caller.data().cap_id;
            if capability::check_global(&cap_id, capability::Rights::CAPTURE).is_err() {
                return -1;
            }
            // Worker cores may only set the state; core 0 ticks and paints
            // it from poll_render.
            crate::shade::with_compositor(|comp| comp.start_flash());
            crate::shade::request_render();
            0
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_capture_screen(buf_ptr, buf_max) -> bytes_written or -1
    // CAPTURE-gated (screen-scrape — only the screenshot tool holds it).
    // Copies the composited front framebuffer as tightly-packed BGRA32
    // (width*height*4) into the app buffer. The app then PNG-encodes /
    // crops it itself; the kernel only hands over the raw pixels.
    linker.func_wrap("env", "npk_capture_screen",
        |mut caller: Caller<'_, HostState>, buf_ptr: i32, buf_max: i32| -> i32 {
            let cap_id = caller.data().cap_id;
            if capability::check_global(&cap_id, capability::Rights::CAPTURE).is_err() {
                return -1;
            }
            let info = crate::framebuffer::get_info();
            let w = info.width as usize;
            let h = info.height as usize;
            let pitch = info.pitch as usize;
            if w == 0 || h == 0 { return -1; }
            let need = w * h * 4;
            if (buf_max as usize) < need { return -1; }
            if info.addr == 0 { return -1; }
            // Read the actual displayed MMIO framebuffer (not a shadow
            // buffer): it always holds the final composite (background +
            // windows + cursor) that's physically on screen. The shadow
            // double-buffer can be mid-swap when we (on a worker core)
            // read it, yielding a stale background-only frame — the
            // reason an earlier shadow capture missed all the windows.
            // Row-by-row into a tight BGRA temp (pitch may exceed w*4).
            let mut tmp = alloc::vec![0u8; need];
            let src = info.addr as *const u8;
            for y in 0..h {
                // SAFETY: the GOP framebuffer is identity-mapped and valid
                // for pitch*height bytes; we read w*4 ≤ pitch per row.
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        src.add(y * pitch),
                        tmp.as_mut_ptr().add(y * w * 4),
                        w * 4,
                    );
                }
            }
            let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(m) => m,
                None => return -1,
            };
            let data = mem.data_mut(&mut caller);
            let start = buf_ptr as usize;
            let end = start + need;
            if end > data.len() { return -1; }
            data[start..end].copy_from_slice(&tmp);
            need as i32
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_event_poll(buf_ptr, buf_max) -> i32
    // Non-blocking: pop one event from this app's widget-window
    // queue, postcard-encode it into the supplied WASM buffer.
    //   >0 → encoded byte count
    //   0  → queue empty (app should sleep / yield)
    //   -1 → no widget window, cap denied, or buffer too small
    linker.func_wrap("env", "npk_event_poll",
        |mut caller: Caller<'_, HostState>, buf_ptr: i32, buf_max: i32| -> i32 {
            let cap_id = caller.data().cap_id;
            if capability::check_global(&cap_id, capability::Rights::RENDER).is_err() {
                return -1;
            }
            let window_id = caller.data().widget_window_id;
            if window_id == 0 { return -1; }
            // -1 also covers "window was closed by shade" (e.g. Mod+Shift+Q)
            // so the app can fall out of its poll loop instead of spinning.
            if !crate::shade::widgets::widget_window_exists(window_id) { return -1; }

            let event = match crate::shade::widgets::poll_event(window_id) {
                Some(e) => e,
                None => return 0,
            };
            let encoded = match postcard::to_allocvec(&event) {
                Ok(v) => v,
                Err(_) => return -1,
            };
            if encoded.len() > buf_max as usize { return -1; }

            let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(m) => m,
                None => return -1,
            };
            let data = mem.data_mut(&mut caller);
            let start = buf_ptr as usize;
            let end = start + encoded.len();
            if end > data.len() { return -1; }
            data[start..end].copy_from_slice(&encoded);
            encoded.len() as i32
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_list_modules(buf_ptr, buf_max) -> i32
    // Writes a NUL-separated list of module names from `sys/wasm/*` into
    // the caller's buffer. Returns bytes written, or -1 on cap denied /
    // buffer too small. The trailing entry is NOT terminated — caller
    // splits on 0x00.
    //
    // RENDER-gated because only GUI apps (drun) need this today. Adjust
    // if terminal utilities ever want the same API.
    linker.func_wrap("env", "npk_list_modules",
        |mut caller: Caller<'_, HostState>, buf_ptr: i32, buf_max: i32| -> i32 {
            let cap_id = caller.data().cap_id;
            if capability::check_global(&cap_id, capability::Rights::RENDER).is_err() {
                return -1;
            }

            // v2: `sys/wasm` is a real directory. List immediate children
            // directly instead of scanning + prefix-filtering the whole tree.
            let entries = match crate::npkfs::fs::list("sys/wasm") {
                Ok(Some(v)) => v,
                Ok(None) => alloc::vec::Vec::new(),
                Err(_) => return -1,
            };

            let mut out: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
            for e in &entries {
                if !matches!(e.kind, crate::npkfs::object::EntryKind::File) { continue; }
                if e.name.ends_with(".version") { continue; }
                if !out.is_empty() { out.push(0); }
                out.extend_from_slice(e.name.as_bytes());
            }

            if out.len() > buf_max as usize { return -1; }

            let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(m) => m,
                None => return -1,
            };
            let data = mem.data_mut(&mut caller);
            let start = buf_ptr as usize;
            let end = start + out.len();
            if end > data.len() { return -1; }
            data[start..end].copy_from_slice(&out);
            out.len() as i32
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_app_meta(name_ptr, name_len, buf_ptr, buf_max) -> bytes or -1
    // Returns ONLY the `.npk.app_meta` custom-section payload of the module
    // `sys/wasm/<name>`, extracted kernel-side. Launchers (drun/dock) read an
    // app's icon/name/description with this WITHOUT fetching the whole module
    // — beak carries >2 MB of embedded fonts, and the old client-side reader
    // fetched the full wasm into a fixed 2 MB buffer, truncating beak so its
    // trailing app_meta section was lost → the app vanished from the catalog.
    // `name` is confined to a bare child of `sys/wasm/` (no path traversal).
    // RENDER-gated like npk_list_modules.
    linker.func_wrap("env", "npk_app_meta",
        |mut caller: Caller<'_, HostState>, name_ptr: i32, name_len: i32,
         buf_ptr: i32, buf_max: i32| -> i32 {
            let cap_id = caller.data().cap_id;
            if capability::check_global(&cap_id, capability::Rights::RENDER).is_err() {
                return -1;
            }
            if name_len <= 0 || name_len > 64 || buf_max <= 0 { return -1; }

            let name = match read_wasm_str(&caller, name_ptr, name_len) {
                Some(s) => s,
                None => return -1,
            };
            // Confine to sys/wasm/<bare-name>: reject any separator so a caller
            // can't traverse out of the module directory.
            if name.contains('/') { return -1; }
            let path = alloc::format!("sys/wasm/{}", name);

            let (content, _) = match crate::npkfs::fetch(&path) {
                Ok(v) => v,
                Err(_) => return -1,
            };

            let meta = match extract_wasm_custom_section(&content, ".npk.app_meta") {
                Some(m) => m,
                None => return -1,
            };

            let write_len = meta.len().min(buf_max as usize);
            let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(m) => m,
                None => return -1,
            };
            let data = mem.data_mut(&mut caller);
            let start = buf_ptr as usize;
            if start + write_len > data.len() { return -1; }
            data[start..start + write_len].copy_from_slice(&meta[..write_len]);
            write_len as i32
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_spawn_module(name_ptr, name_len) -> i32
    // Launch `sys/wasm/<name>` in a fresh terminal window and focus it.
    //
    // Modelled on `Mod+Enter` + `run <name>` — the user-expected flow
    // when drun picks a module. Terminal-kind apps (top, debug) print
    // into the new loop's terminal; widget-kind apps can convert their
    // window via `npk_window_set_overlay` from `_start`.
    //
    //   0  → spawn accepted
    //   -1 → cap denied / bad args / module not found / compositor
    //        unavailable (no free terminal slot)
    linker.func_wrap("env", "npk_spawn_module",
        |caller: Caller<'_, HostState>, name_ptr: i32, name_len: i32| -> i32 {
            let cap_id = caller.data().cap_id;
            if capability::check_global(&cap_id, capability::Rights::RENDER).is_err() {
                return -1;
            }
            if name_len <= 0 || name_len > 64 { return -1; }

            let name = {
                let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                    Some(m) => m,
                    None => return -1,
                };
                let data = mem.data(&caller);
                let start = name_ptr as usize;
                let end = start + name_len as usize;
                if end > data.len() { return -1; }
                match core::str::from_utf8(&data[start..end]) {
                    Ok(s) => alloc::string::String::from(s),
                    Err(_) => return -1,
                }
            };

            // Path validation — refuse absolute paths, traversal, prefix reuse.
            if name.contains('/') || name.contains("..") || name.is_empty() {
                return -1;
            }

            let path = alloc::format!("sys/wasm/{}", name);
            let (bytes, _hash) = match crate::npkfs::fetch(&path) {
                Ok(v) => v,
                Err(_) => return -1,
            };

            // Grant exactly the rights the app declares in its `.npk.caps`
            // section (e.g. spell asks for WRITE to save files); apps with
            // no declaration get the safe default (no WRITE). Per-app, not
            // a blanket grant.
            let rights = capability::widget_rights_from_wasm(&bytes);
            let module_cap = match capability::create_module_cap(rights, Some(600_000)) {
                Ok(id) => id,
                Err(_) => return -1,
            };

            // Create a new terminal-kind window with its own terminal
            // buffer and focus it. The widget-kind launcher that called
            // us then closes itself (`npk_close_widget`), leaving the
            // new loop + running app on screen.
            let spawned = crate::shade::with_compositor(|comp| {
                let id = comp.create_window(&name, 0, 0, 800, 600)?;
                comp.focus_window(id);
                let term_idx = comp.windows.iter()
                    .find(|w| w.id == id)
                    .map(|w| w.terminal_idx)?;
                Some((id, term_idx))
            }).flatten();

            let (win_id, term_idx) = match spawned {
                Some(v) => v,
                None => return -1,
            };

            // Fresh session prompt so the terminal isn't stuck on the
            // caller's old prompt state when the app exits.
            crate::intent::reset_session_prompt(term_idx);

            if !spawn_on_worker(bytes.to_vec(), module_cap, term_idx, &name) {
                crate::shade::with_compositor(|comp| comp.close_window(win_id));
                return -1;
            }
            crate::shade::request_render();
            0
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_run_intent(verb_ptr, verb_len) -> i32
    // Trigger a built-in system intent that isn't a WASM module — the
    // launcher path for microvm-backed apps. Currently `browser` is
    // the only verb; future apps (office, ide, …) just add a match
    // arm. Returns 0 on accepted, -1 on cap denied / unknown verb /
    // unsupported on the cooperative path.
    //
    // Safety from a worker core: vm_open under A2 (dedicated VM core)
    // is pure atomic + mutex (stash PENDING_VM → return; the
    // dedicated core picks it up via vm_core_serve and runs the
    // entire VM lifecycle on itself). Cooperative path (≤2 cores)
    // needs Core-0 BSP state for VMXON, so this rejects there.
    linker.func_wrap("env", "npk_run_intent",
        |caller: Caller<'_, HostState>, verb_ptr: i32, verb_len: i32| -> i32 {
            let cap_id = caller.data().cap_id;
            if capability::check_global(&cap_id, capability::Rights::EXECUTE).is_err() {
                return -1;
            }
            if verb_len <= 0 || verb_len > 64 { return -1; }
            let verb = {
                let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                    Some(m) => m,
                    None => return -1,
                };
                let data = mem.data(&caller);
                let start = verb_ptr as usize;
                let end = start + verb_len as usize;
                if end > data.len() { return -1; }
                match core::str::from_utf8(&data[start..end]) {
                    Ok(s) => alloc::string::String::from(s),
                    Err(_) => return -1,
                }
            };
            // Reject only on the pure cooperative Core-0 path (BSP-only),
            // so the caller falls back to typing the intent at the prompt.
            // Fiber mode (vCPU as a pool fiber) AND the dedicated-core path
            // both support launching from a worker, so allow those.
            if !crate::microvm::cpu::vm_fiber_mode()
                && crate::smp::per_core::dedicated_vm_core().is_none()
            {
                return -1;
            }
            match verb.as_str() {
                "browser" => {
                    crate::intent::launch_browser();
                    0
                }
                _ => -1,
            }
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_window_set_overlay(w, h) -> i32
    // Mark the calling app's widget window as a centred overlay of the
    // requested size. Removes the window from the tiling grid (if it
    // was part of it), re-centres it, and requests re-render.
    //
    // If the app hasn't created its widget window yet (widget_window_id
    // == 0), this call also creates the window — title is the module
    // name recorded at spawn time. First caller "wins" the window;
    // subsequent calls just reconfigure.
    //
    // Returns 0 on success, -1 on cap denied / compositor unavailable.
    linker.func_wrap("env", "npk_window_set_overlay",
        |mut caller: Caller<'_, HostState>, w: i32, h: i32| -> i32 {
            let cap_id = caller.data().cap_id;
            if capability::check_global(&cap_id, capability::Rights::RENDER).is_err() {
                return -1;
            }
            if w <= 0 || h <= 0 { return -1; }

            let mut wid = caller.data().widget_window_id;
            if wid == 0 {
                // Prefer promoting the spawning terminal to a widget so
                // the app owns a single window. Only create a fresh one
                // if no terminal backed this worker (direct-launch path).
                let terminal_idx = caller.data().terminal_idx;
                let promoted = if terminal_idx != 255 {
                    crate::shade::with_compositor(|c|
                        c.promote_terminal_to_widget(terminal_idx)
                    ).flatten()
                } else {
                    None
                };

                let new_id = match promoted {
                    Some(id) => {
                        caller.data_mut().terminal_idx = 255;
                        // Overlay path wants focus on the new widget (drun
                        // style); promotion does not focus, so fix up.
                        crate::shade::with_compositor(|comp| comp.focus_window(id));
                        id.0
                    }
                    None => {
                        let title = caller.data().module_name.clone();
                        match crate::shade::with_compositor(|comp| {
                            let id = comp.create_widget_window(
                                if title.is_empty() { "widget" } else { title.as_str() });
                            comp.focus_window(id);
                            id.0
                        }) {
                            Some(v) => v,
                            None => return -1,
                        }
                    }
                };
                caller.data_mut().widget_window_id = new_id;
                wid = new_id;
            }

            let ok = crate::shade::with_compositor(|comp| {
                let ok = comp.set_overlay(crate::shade::WindowId(wid), w as u32, h as u32);
                // The overlay path always wants this window focused — drun
                // and any other launcher style app drives keyboard from
                // here. The promote-or-create branch above already focuses,
                // but if the app calls set_overlay a second time (or after
                // some other host fn shifted focus elsewhere) we need to
                // re-claim it so keys don't end up routed at a stale window.
                if ok {
                    comp.focus_window(crate::shade::WindowId(wid));
                }
                ok
            }).unwrap_or(false);

            if ok {
                crate::shade::request_render();
                0
            } else {
                -1
            }
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_window_set_modal(modal: i32) -> i32
    // Toggle the modal flag on the calling app's widget window. While
    // any window is modal, shade-action dispatch suppresses focus-shift
    // / tiling shortcuts (see handle_action in shade/mod.rs).
    //
    // Returns 0 on success, -1 if the app has no widget window yet /
    // cap denied.
    linker.func_wrap("env", "npk_window_set_modal",
        |caller: Caller<'_, HostState>, modal: i32| -> i32 {
            let cap_id = caller.data().cap_id;
            if capability::check_global(&cap_id, capability::Rights::RENDER).is_err() {
                return -1;
            }
            let wid = caller.data().widget_window_id;
            if wid == 0 { return -1; }
            let ok = crate::shade::with_compositor(|comp|
                comp.set_modal(crate::shade::WindowId(wid), modal != 0)
            ).unwrap_or(false);
            if ok { 0 } else { -1 }
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_window_set_overlay_at(x, y, w, h) -> i32
    // Like npk_window_set_overlay but positions the overlay's top-left at
    // (x, y) instead of centring it — for corner-anchored dropdowns (e.g.
    // the volume slider under the bar). Creates/promotes + focuses the
    // caller's widget window, same as the centred overlay path.
    //
    // Returns 0 on success, -1 on cap denied / bad args / no compositor.
    linker.func_wrap("env", "npk_window_set_overlay_at",
        |mut caller: Caller<'_, HostState>, x: i32, y: i32, w: i32, h: i32| -> i32 {
            let cap_id = caller.data().cap_id;
            if capability::check_global(&cap_id, capability::Rights::RENDER).is_err() {
                return -1;
            }
            if w <= 0 || h <= 0 || x < 0 || y < 0 { return -1; }

            let mut wid = caller.data().widget_window_id;
            if wid == 0 {
                let terminal_idx = caller.data().terminal_idx;
                let promoted = if terminal_idx != 255 {
                    crate::shade::with_compositor(|c|
                        c.promote_terminal_to_widget(terminal_idx)
                    ).flatten()
                } else {
                    None
                };
                let new_id = match promoted {
                    Some(id) => {
                        caller.data_mut().terminal_idx = 255;
                        crate::shade::with_compositor(|comp| comp.focus_window(id));
                        id.0
                    }
                    None => {
                        let title = caller.data().module_name.clone();
                        match crate::shade::with_compositor(|comp| {
                            let id = comp.create_widget_window(
                                if title.is_empty() { "widget" } else { title.as_str() });
                            comp.focus_window(id);
                            id.0
                        }) {
                            Some(v) => v,
                            None => return -1,
                        }
                    }
                };
                caller.data_mut().widget_window_id = new_id;
                wid = new_id;
            }

            let ok = crate::shade::with_compositor(|comp| {
                let ok = comp.set_overlay_at(crate::shade::WindowId(wid),
                    x, y, w as u32, h as u32);
                if ok { comp.focus_window(crate::shade::WindowId(wid)); }
                ok
            }).unwrap_or(false);

            if ok { crate::shade::request_render(); 0 } else { -1 }
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_window_set_light_dismiss(on: i32) -> i32
    // Opt the caller's widget window into light-dismiss: the compositor
    // closes it when a click lands outside it (transient overlays like the
    // volume slider). Off by default, so other overlays (loft, drun) are
    // unaffected. Returns 0 on success, -1 if no widget window / cap denied.
    linker.func_wrap("env", "npk_window_set_light_dismiss",
        |caller: Caller<'_, HostState>, on: i32| -> i32 {
            let cap_id = caller.data().cap_id;
            if capability::check_global(&cap_id, capability::Rights::RENDER).is_err() {
                return -1;
            }
            let wid = caller.data().widget_window_id;
            if wid == 0 { return -1; }
            let ok = crate::shade::with_compositor(|comp|
                comp.set_light_dismiss(crate::shade::WindowId(wid), on != 0)
            ).unwrap_or(false);
            if ok { 0 } else { -1 }
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_window_set_clipboard_sink() -> i32
    // Opt the caller's widget window into Ctrl+C/X/V delivery as
    // Event::Clipboard when a focused text widget can't act on the chord
    // (copy/cut with no selection, paste into an empty single-line Input).
    // Used by file managers so the shortcuts drive file operations without
    // stealing text copy/paste from other apps. Returns 0 / -1.
    linker.func_wrap("env", "npk_window_set_clipboard_sink",
        |caller: Caller<'_, HostState>| -> i32 {
            let cap_id = caller.data().cap_id;
            if capability::check_global(&cap_id, capability::Rights::RENDER).is_err() {
                return -1;
            }
            let wid = caller.data().widget_window_id;
            if wid == 0 { return -1; }
            crate::shade::widgets::set_clipboard_sink(wid);
            0
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_window_set_dock(w, h) -> i32
    // Turn the calling app's widget window into a bottom auto-hide dock:
    // overlay (no tiling strut), never modal, never focused on reveal,
    // global across workspaces. Starts hidden; the compositor slides it
    // in when the cursor holds the bottom edge. Like set_overlay but
    // bottom-anchored instead of centred, and it does NOT grab focus.
    //
    // Returns 0 on success, -1 on cap denied / bad args / no compositor.
    linker.func_wrap("env", "npk_window_set_dock",
        |mut caller: Caller<'_, HostState>, w: i32, h: i32| -> i32 {
            let cap_id = caller.data().cap_id;
            if capability::check_global(&cap_id, capability::Rights::RENDER).is_err() {
                return -1;
            }
            if w <= 0 || h <= 0 { return -1; }

            let mut wid = caller.data().widget_window_id;
            if wid == 0 {
                // Promote the spawning terminal to a widget window if there
                // is one; otherwise create a fresh widget window. Unlike
                // the overlay path we do NOT focus it — the dock is a
                // background overlay that never owns keyboard focus.
                let terminal_idx = caller.data().terminal_idx;
                let promoted = if terminal_idx != 255 {
                    crate::shade::with_compositor(|c|
                        c.promote_terminal_to_widget(terminal_idx)
                    ).flatten()
                } else {
                    None
                };

                let new_id = match promoted {
                    Some(id) => {
                        caller.data_mut().terminal_idx = 255;
                        id.0
                    }
                    None => {
                        let title = caller.data().module_name.clone();
                        match crate::shade::with_compositor(|comp| {
                            comp.create_widget_window(
                                if title.is_empty() { "dock" } else { title.as_str() }).0
                        }) {
                            Some(v) => v,
                            None => return -1,
                        }
                    }
                };
                caller.data_mut().widget_window_id = new_id;
                wid = new_id;
            }

            let ok = crate::shade::with_compositor(|comp|
                comp.set_dock(crate::shade::WindowId(wid), w as u32, h as u32)
            ).unwrap_or(false);

            if ok {
                crate::shade::request_render();
                0
            } else {
                -1
            }
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_window_set_panel(edge, behavior, w, h) -> i32
    // Generalised edge panel (see docs/spec/PANEL.md): edge 0=Bottom 1=Top,
    // behavior 0=AutoHide overlay (dock) 1=Strut (bar). Creates/promotes
    // the caller's widget window WITHOUT grabbing focus (like the dock),
    // then hands it to the compositor's panel config. `set_dock` above is
    // now the (Bottom, AutoHide) wrapper of this.
    //
    // Returns 0 on success, -1 on cap denied / bad args / no compositor.
    linker.func_wrap("env", "npk_window_set_panel",
        |mut caller: Caller<'_, HostState>, edge: i32, behavior: i32, w: i32, h: i32| -> i32 {
            let cap_id = caller.data().cap_id;
            if capability::check_global(&cap_id, capability::Rights::RENDER).is_err() {
                return -1;
            }
            if w <= 0 || h <= 0 || edge < 0 || behavior < 0 { return -1; }

            let mut wid = caller.data().widget_window_id;
            if wid == 0 {
                let terminal_idx = caller.data().terminal_idx;
                let promoted = if terminal_idx != 255 {
                    crate::shade::with_compositor(|c|
                        c.promote_terminal_to_widget(terminal_idx)
                    ).flatten()
                } else {
                    None
                };
                let new_id = match promoted {
                    Some(id) => { caller.data_mut().terminal_idx = 255; id.0 }
                    None => {
                        let title = caller.data().module_name.clone();
                        match crate::shade::with_compositor(|comp| {
                            comp.create_widget_window(
                                if title.is_empty() { "panel" } else { title.as_str() }).0
                        }) {
                            Some(v) => v,
                            None => return -1,
                        }
                    }
                };
                caller.data_mut().widget_window_id = new_id;
                wid = new_id;
            }

            let ok = crate::shade::with_compositor(|comp|
                comp.set_panel(crate::shade::WindowId(wid),
                    edge as u8, behavior as u8, w as u32, h as u32)
            ).unwrap_or(false);

            if ok { crate::shade::request_render(); 0 } else { -1 }
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_bar_state(buf, max) -> i32
    // Live state for the bar app: "HH:MM\n<ws_count>\n<ws_active>\n<title>"
    // (clock already timezone-adjusted). Returns bytes written, -1 on
    // cap / args / buffer too small.
    linker.func_wrap("env", "npk_bar_state",
        |mut caller: Caller<'_, HostState>, buf_ptr: i32, max: i32| -> i32 {
            let cap_id = caller.data().cap_id;
            if capability::check_global(&cap_id, capability::Rights::RENDER).is_err() {
                return -1;
            }
            if max <= 0 { return -1; }

            let unix = crate::rtc::read_unix_time().unwrap_or(0);
            let tz = crate::config::timezone_offset_minutes();
            let local = unix as i64 + tz as i64 * 60;
            let secs = ((local % 86400) + 86400) % 86400;
            let (ws_count, ws_active, title) =
                crate::shade::with_compositor(|c| c.bar_info())
                    .unwrap_or((0, 0, alloc::string::String::new()));
            let s = alloc::format!("{:02}:{:02}\n{}\n{}\n{}",
                secs / 3600, (secs % 3600) / 60, ws_count, ws_active, title);

            let bytes = s.as_bytes();
            let write_len = bytes.len().min(max as usize);
            let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(m) => m,
                None => return -1,
            };
            let data = mem.data_mut(&mut caller);
            let start = buf_ptr as usize;
            if start + write_len <= data.len() {
                data[start..start + write_len].copy_from_slice(&bytes[..write_len]);
                write_len as i32
            } else {
                -1
            }
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_window_titles(buf, max) -> i32
    // One line per open app window: "<flags>\t<workspace>\t<title>", flags
    // being a decimal bitmask (1 = focused, 2 = on the active workspace).
    // Panels and overlays are excluded. The dock derives its running/active
    // indicators from this, the bar its occupied-workspace hints; the
    // kernel stays free of app names.
    linker.func_wrap("env", "npk_window_titles",
        |mut caller: Caller<'_, HostState>, buf_ptr: i32, max: i32| -> i32 {
            let cap_id = caller.data().cap_id;
            if capability::check_global(&cap_id, capability::Rights::RENDER).is_err() {
                return -1;
            }
            if max <= 0 { return -1; }
            let s = crate::shade::with_compositor(|c| c.window_lines())
                .unwrap_or_default();
            let bytes = s.as_bytes();
            let write_len = bytes.len().min(max as usize);
            let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(m) => m,
                None => return -1,
            };
            let data = mem.data_mut(&mut caller);
            let Ok(start) = usize::try_from(buf_ptr) else { return -1 };
            let Some(end) = start.checked_add(write_len) else { return -1 };
            if end > data.len() { return -1; }
            data[start..end].copy_from_slice(&bytes[..write_len]);
            write_len as i32
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_battery() -> i32 — battery state for the bar plugin. Returns -1
    // when no battery is known (desktops/QEMU → segment stays empty), else
    // (status << 8) | percent, with status 0=discharging 1=charging 2=full
    // 3=plugged-idle and percent in 0..=100. Prefers the AML driver's report
    // (aml.wasm, vendor-independent via _BST/_BIF); falls back to the
    // standardised SBS-over-SMBus path for SBS laptops.
    linker.func_wrap("env", "npk_battery",
        |caller: Caller<'_, HostState>| -> i32 {
            let cap_id = caller.data().cap_id;
            if capability::check_global(&cap_id, capability::Rights::RENDER).is_err() {
                return -1;
            }
            let cached = crate::battery::cached();
            if cached >= 0 {
                return cached;
            }
            match crate::battery::read() {
                Some(b) => ((b.status as i32) << 8) | b.percent as i32,
                None => -1,
            }
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // ── AML battery driver (aml.wasm) host-fns — all HARDWARE-gated ──────
    // npk_acpi_dsdt(buf_ptr, buf_max) -> i32: copy the DSDT (firmware AML)
    // into the caller's buffer; returns the DSDT length. If it exceeds
    // buf_max nothing is copied (caller sizes its buffer up). -1 on error.
    linker.func_wrap("env", "npk_acpi_dsdt",
        |mut caller: Caller<'_, HostState>, buf_ptr: i32, buf_max: i32| -> i32 {
            let cap_id = caller.data().cap_id;
            if capability::check_global(&cap_id, capability::Rights::HARDWARE).is_err() {
                return -1;
            }
            let Some((addr, len)) = crate::acpi::dsdt() else { return -1 };
            if len > buf_max as usize {
                return len as i32; // too small: tell the caller the needed size
            }
            // SAFETY: acpi::dsdt() mapped [addr, addr+len) for us.
            let src = unsafe { core::slice::from_raw_parts(addr as *const u8, len) };
            let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(m) => m,
                None => return -1,
            };
            let data = mem.data_mut(&mut caller);
            let start = buf_ptr as usize;
            let end = start + len;
            if end > data.len() { return -1; }
            data[start..end].copy_from_slice(src);
            len as i32
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_ec_read(addr) -> i32: read one EC-RAM byte (0..255) or -1.
    linker.func_wrap("env", "npk_ec_read",
        |caller: Caller<'_, HostState>, addr: i32| -> i32 {
            let cap_id = caller.data().cap_id;
            if capability::check_global(&cap_id, capability::Rights::HARDWARE).is_err() {
                return -1;
            }
            if !(0..=255).contains(&addr) { return -1; }
            match crate::ec::read(addr as u8) {
                Some(v) => v as i32,
                None => -1,
            }
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_ec_write(addr, val) -> i32: firmware-directed EC write (BSEL etc.).
    // 0 on success, -1 on error.
    linker.func_wrap("env", "npk_ec_write",
        |caller: Caller<'_, HostState>, addr: i32, val: i32| -> i32 {
            let cap_id = caller.data().cap_id;
            if capability::check_global(&cap_id, capability::Rights::HARDWARE).is_err() {
                return -1;
            }
            if !(0..=255).contains(&addr) || !(0..=255).contains(&val) { return -1; }
            if crate::ec::write(addr as u8, val as u8) { 0 } else { -1 }
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_battery_report(packed): the AML driver pushes the decoded battery
    // state ((status<<8)|percent, or -1 for absent) into the kernel cache
    // that npk_battery() returns.
    linker.func_wrap("env", "npk_battery_report",
        |caller: Caller<'_, HostState>, packed: i32| {
            let cap_id = caller.data().cap_id;
            if capability::check_global(&cap_id, capability::Rights::HARDWARE).is_err() {
                return;
            }
            crate::battery::report(packed);
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // ── Audio mailbox + mixer ────────────────────────────────────────────
    // Apps push PCM (S16LE / 48 kHz / stereo) into per-slot rings; the HDA
    // driver pulls a mixed stream via npk_audio_poll_mix. Ungated: audio
    // playback is not a security boundary, and the kernel holds no HDA
    // knowledge — it just shuttles + sum-mixes bytes.

    // npk_audio_open() -> slot index, or -1 if no slot free.
    linker.func_wrap("env", "npk_audio_open",
        |_caller: Caller<'_, HostState>| -> i32 { crate::audio::open() },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_audio_close(slot) -> 0.
    linker.func_wrap("env", "npk_audio_close",
        |_caller: Caller<'_, HostState>, slot: i32| -> i32 {
            if slot >= 0 { crate::audio::close(slot as usize); }
            0
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_audio_submit(slot, ptr, len) -> bytes accepted, or -1 on bad args.
    linker.func_wrap("env", "npk_audio_submit",
        |caller: Caller<'_, HostState>, slot: i32, ptr: i32, len: i32| -> i32 {
            if slot < 0 || ptr < 0 || len < 0 { return -1; }
            let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(m) => m,
                None => return -1,
            };
            let data = mem.data(&caller);
            let (start, end) = (ptr as usize, ptr as usize + len as usize);
            if end > data.len() { return -1; }
            crate::audio::submit(slot as usize, &data[start..end]) as i32
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_audio_poll_mix(ptr, max) -> bytes written (driver side).
    linker.func_wrap("env", "npk_audio_poll_mix",
        |mut caller: Caller<'_, HostState>, ptr: i32, max: i32| -> i32 {
            if ptr < 0 || max < 0 { return -1; }
            let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(m) => m,
                None => return -1,
            };
            let data = mem.data_mut(&mut caller);
            let (start, end) = (ptr as usize, ptr as usize + max as usize);
            if end > data.len() { return -1; }
            crate::audio::poll_mix(&mut data[start..end]) as i32
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_audio_set_volume(pct) -> 0; npk_audio_get_volume() -> 0..=100.
    linker.func_wrap("env", "npk_audio_set_volume",
        |_caller: Caller<'_, HostState>, pct: i32| -> i32 {
            if pct < 0 { return -1; }
            crate::audio::set_volume(pct.min(100) as u8);
            0
        },
    ).map_err(|_| WasmError::HostFunctionError)?;
    linker.func_wrap("env", "npk_audio_get_volume",
        |_caller: Caller<'_, HostState>| -> i32 { crate::audio::get_volume() as i32 },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_workspace_switch(n) -> i32 — switch to workspace n (bar clicks).
    linker.func_wrap("env", "npk_workspace_switch",
        |caller: Caller<'_, HostState>, n: i32| -> i32 {
            let cap_id = caller.data().cap_id;
            if capability::check_global(&cap_id, capability::Rights::RENDER).is_err() {
                return -1;
            }
            if !(0..=255).contains(&n) { return -1; }
            crate::shade::with_compositor(|c| c.switch_workspace(n as u8));
            crate::shade::request_render();
            0
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_power() -> i32 — ACPI S5 power-off (bar power button). Does not
    // return on success.
    linker.func_wrap("env", "npk_power",
        |caller: Caller<'_, HostState>| -> i32 {
            let cap_id = caller.data().cap_id;
            if capability::check_global(&cap_id, capability::Rights::RENDER).is_err() {
                return -1;
            }
            crate::acpi::power_off();
            0
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_fs_list(prefix_ptr, prefix_len, out_ptr, out_cap, recursive) -> i32
    // Enumerate npkFS keys under `prefix`. If recursive=0, only direct
    // children are returned (keys that contain no '/' after the prefix,
    // plus the unique directory bucket names that do). If recursive=1,
    // every key under the prefix is emitted verbatim.
    //
    // Wire format of the output buffer — one entry per line, separated
    // by '\n' (no trailing newline after the last):
    //   <name>\0<size_le_u64:8>\0<is_dir_u8>\0<mtime_le_u64:8>
    // - <name> is relative to `prefix` (prefix itself + trailing slash
    //   stripped). For a synthetic directory entry (first path component
    //   encountered in recursive scan), size=0 and is_dir=1.
    // - Size is little-endian 8 bytes. is_dir is 0 or 1.
    // - mtime is UTC seconds since the Unix epoch (LE u64). Zero means
    //   "unknown" (RTC was unreadable when this entry was created).
    //   Synthetic directory entries from recursive descent inherit
    //   mtime=0; only stored TreeEntry instances carry real values.
    //
    // Returns bytes written, 0 if prefix is empty, -1 on cap / args /
    // truncation (buffer too small to fit the full listing).
    linker.func_wrap("env", "npk_fs_list",
        |mut caller: Caller<'_, HostState>, prefix_ptr: i32, prefix_len: i32,
         out_ptr: i32, out_cap: i32, recursive: i32| -> i32 {
            let cap_id = caller.data().cap_id;
            if capability::check_global(&cap_id, capability::Rights::READ).is_err() {
                return -1;
            }
            if prefix_len < 0 || out_cap <= 0 { return -1; }

            let prefix = if prefix_len == 0 {
                alloc::string::String::new()
            } else {
                match read_wasm_str(&caller, prefix_ptr, prefix_len) {
                    Some(s) => s,
                    None => return -1,
                }
            };

            // v2: directories are real Tree objects, listings come straight
            // from them — no scan + prefix-filter pass.
            let mut out: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
            let prefix_for_list = prefix.trim_matches('/');

            if recursive == 0 {
                // Non-recursive: one directory's immediate children.
                let entries = match crate::npkfs::fs::list(prefix_for_list) {
                    Ok(Some(v)) => v,
                    Ok(None) => alloc::vec::Vec::new(),
                    Err(_) => return -1,
                };
                for e in &entries {
                    let is_dir = matches!(e.kind, crate::npkfs::object::EntryKind::Dir);
                    append_entry(&mut out, &e.name, e.size, is_dir, e.mtime);
                }
            } else {
                // Recursive: DFS the subtree, emit relative paths.
                fn dfs(
                    base: &str, rel: alloc::string::String,
                    out: &mut alloc::vec::Vec<u8>,
                ) -> Result<(), ()> {
                    let abs = if rel.is_empty() {
                        alloc::string::String::from(base)
                    } else if base.is_empty() {
                        rel.clone()
                    } else {
                        alloc::format!("{}/{}", base, rel)
                    };
                    let entries = match crate::npkfs::fs::list(&abs) {
                        Ok(Some(v)) => v,
                        Ok(None) => return Ok(()),
                        Err(_) => return Err(()),
                    };
                    for e in &entries {
                        let child_rel = if rel.is_empty() {
                            e.name.clone()
                        } else {
                            alloc::format!("{}/{}", rel, e.name)
                        };
                        match e.kind {
                            crate::npkfs::object::EntryKind::File => {
                                append_entry(out, &child_rel, e.size, false, e.mtime);
                            }
                            crate::npkfs::object::EntryKind::Dir => {
                                append_entry(out, &child_rel, 0, true, e.mtime);
                                dfs(base, child_rel, out)?;
                            }
                        }
                    }
                    Ok(())
                }
                if dfs(prefix_for_list, alloc::string::String::new(), &mut out).is_err() {
                    return -1;
                }
            }

            if out.len() > out_cap as usize { return -1; }

            let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(m) => m,
                None => return -1,
            };
            let data = mem.data_mut(&mut caller);
            let start = out_ptr as usize;
            let end = start + out.len();
            if end > data.len() { return -1; }
            data[start..end].copy_from_slice(&out);
            out.len() as i32
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_fs_stat(name_ptr, name_len, out_ptr) -> i32
    // Write 17 bytes into out_ptr:
    //   size_le_u64 (8) + is_dir_u8 (1) + mtime_le_u64 (8).
    // Returns 17 on success, 0 if no entry, -1 on cap / args.
    // mtime is UTC seconds since the Unix epoch — zero means unknown.
    linker.func_wrap("env", "npk_fs_stat",
        |mut caller: Caller<'_, HostState>, name_ptr: i32, name_len: i32,
         out_ptr: i32| -> i32 {
            let cap_id = caller.data().cap_id;
            if capability::check_global(&cap_id, capability::Rights::READ).is_err() {
                return -1;
            }
            let name = match read_wasm_str(&caller, name_ptr, name_len) {
                Some(s) => s,
                None => return -1,
            };

            let (size, is_dir, mtime) = match crate::npkfs::fs::stat(&name) {
                Ok(Some(s)) => {
                    let is_dir = matches!(s.kind, crate::npkfs::object::EntryKind::Dir);
                    (s.size, if is_dir { 1u8 } else { 0u8 }, s.mtime)
                }
                Ok(None) => return 0,
                Err(_) => return -1,
            };

            let mut buf = [0u8; 17];
            buf[0..8].copy_from_slice(&size.to_le_bytes());
            buf[8] = is_dir;
            buf[9..17].copy_from_slice(&mtime.to_le_bytes());
            let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(m) => m,
                None => return -1,
            };
            let data = mem.data_mut(&mut caller);
            let start = out_ptr as usize;
            if start + 17 > data.len() { return -1; }
            data[start..start + 17].copy_from_slice(&buf);
            17
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_fs_delete(name_ptr, name_len) -> i32
    // Delete a single npkFS key. WRITE-gated. Returns 0 on success,
    // -1 on cap / not found / fs error.
    linker.func_wrap("env", "npk_fs_delete",
        |caller: Caller<'_, HostState>, name_ptr: i32, name_len: i32| -> i32 {
            let cap_id = caller.data().cap_id;
            if capability::check_global(&cap_id, capability::Rights::WRITE).is_err() {
                return -1;
            }
            let name = match read_wasm_str(&caller, name_ptr, name_len) {
                Some(s) => s,
                None => return -1,
            };
            // Apps may not delete modules or trust anchors — see
            // is_trust_critical_path.
            if is_trust_critical_path(&name) {
                kprintln!("[npk] WASM: npk_fs_delete DENIED ({} is read-only to apps)", name);
                return -1;
            }
            match crate::npkfs::delete(&name) {
                Ok(_) => 0,
                Err(_) => -1,
            }
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_fs_rename(old_ptr, old_len, new_ptr, new_len) -> i32
    // Move/rename a single npkFS key (files and whole directories).
    // Content-addressed, so even a directory move is O(1). WRITE-gated;
    // neither path may touch the module store. Returns 0 / -1.
    linker.func_wrap("env", "npk_fs_rename",
        |caller: Caller<'_, HostState>, old_ptr: i32, old_len: i32,
         new_ptr: i32, new_len: i32| -> i32 {
            let cap_id = caller.data().cap_id;
            if capability::check_global(&cap_id, capability::Rights::WRITE).is_err() {
                return -1;
            }
            let old = match read_wasm_str(&caller, old_ptr, old_len) {
                Some(s) => s,
                None => return -1,
            };
            let new = match read_wasm_str(&caller, new_ptr, new_len) {
                Some(s) => s,
                None => return -1,
            };
            // Neither source nor destination may be module or trust store —
            // renaming in would plant an unverified module or anchor.
            if is_trust_critical_path(&old) || is_trust_critical_path(&new) {
                kprintln!("[npk] WASM: npk_fs_rename DENIED (module/trust store is read-only to apps)");
                return -1;
            }
            match crate::npkfs::rename(&old, &new) {
                Ok(_) => 0,
                Err(_) => -1,
            }
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_fs_copy(old_ptr, old_len, new_ptr, new_len) -> i32
    // Copy a single npkFS key (files and whole directories). Shares the
    // source's content hash — no data duplication. WRITE-gated; neither
    // path may touch the module store. Returns 0 / -1.
    linker.func_wrap("env", "npk_fs_copy",
        |caller: Caller<'_, HostState>, old_ptr: i32, old_len: i32,
         new_ptr: i32, new_len: i32| -> i32 {
            let cap_id = caller.data().cap_id;
            if capability::check_global(&cap_id, capability::Rights::WRITE).is_err() {
                return -1;
            }
            let old = match read_wasm_str(&caller, old_ptr, old_len) {
                Some(s) => s,
                None => return -1,
            };
            let new = match read_wasm_str(&caller, new_ptr, new_len) {
                Some(s) => s,
                None => return -1,
            };
            if is_trust_critical_path(&old) || is_trust_critical_path(&new) {
                kprintln!("[npk] WASM: npk_fs_copy DENIED (module/trust store is read-only to apps)");
                return -1;
            }
            match crate::npkfs::copy(&old, &new) {
                Ok(_) => 0,
                Err(_) => -1,
            }
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_close_widget() -> i32
    // Close the calling app's own widget window. The worker then falls
    // out of its `_start` loop by its own logic; this host fn only tears
    // down the window + scene + event queue. Returns 0 on success,
    // -1 if the app doesn't own a widget window.
    linker.func_wrap("env", "npk_close_widget",
        |caller: Caller<'_, HostState>| -> i32 {
            let cap_id = caller.data().cap_id;
            if capability::check_global(&cap_id, capability::Rights::RENDER).is_err() {
                return -1;
            }
            let wid = caller.data().widget_window_id;
            if wid == 0 { return -1; }
            crate::shade::with_compositor(|comp| {
                comp.close_window(crate::shade::WindowId(wid));
            });
            crate::shade::request_render();
            0
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
        |caller: Caller<'_, HostState>, ptr: i32, len: i32,
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
        |caller: Caller<'_, HostState>, ptr: i32| -> i32 {
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

                // 19 → raw TSC reading (monotonic, high-resolution).
                // Combine with key=10 (tsc_mhz) to convert ticks → time.
                // 64-bit TSC fits in i64 (sign bit unused for ~150 years
                // at 2 GHz), so the cast is safe.
                19 => unsafe { core::arch::x86_64::_rdtsc() as i64 },

                // ── Process tracking (keys 20-29) → process table ──
                // 20: count, 21: pid_at_index, 22-29: query by PID
                20..=29 => crate::process::sys_info(key),

                // ── Bench probes (keys 30-34) → cached on first call ──
                // 30: BLAKE3 MB/s, 31: AES-GCM enc MB/s,
                // 32: AES-GCM dec(in-place) MB/s,
                // 33: raw blkdev write MB/s, 34: raw blkdev read MB/s.
                // First call across any of these triggers ~100 ms of
                // measurement; results live in BENCH_CACHE until reboot.
                30..=34 => bench_sys_info(key),

                // ── fsck self-check (key 40) → read-only integrity scan ──
                // Runs on every call (NOT cached), logs a full report to
                // serial, and returns the total problem count (0 = clean,
                // -1 = scan error). testdisk calls this at the END of its run
                // so corruption surfaces in-flight — a reboot would brick the
                // mount before we could ever see it.
                40 => fsck_sys_info(),

                _ => -1,
            }
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_sleep(ms) -> 0 — sleep for N milliseconds.
    // Stage 2b: PARK this app's fiber + yield the worker core back to the
    // per-core scheduler, which runs the core's other ready fibers while we
    // sleep. So dock+bar+loft+spell multiplex over a couple of workers
    // instead of each pinning a core (or nesting via the old next_task
    // helper, which froze the dock — see docs/plan/SCHEDULER_FIBERS.md). The fiber is
    // resumed once the deadline passes.
    linker.func_wrap("env", "npk_sleep",
        |_caller: Caller<'_, HostState>, ms: i32| -> i32 {
            if ms <= 0 || ms > 60000 { return -1; }

            // The normal path: we run inside a fiber → yield to the scheduler.
            if crate::smp::fiber::yield_sleep(ms as u64) {
                return 0;
            }

            // Fallback: not inside a fiber (degenerate no-worker host, or a
            // one-shot wasm on Core 0) → HLT-idle until the deadline. NO
            // core-stealing helper (that was the nesting hazard).
            let freq = crate::interrupts::tsc_freq();
            let target = crate::interrupts::rdtsc() + (ms as u64) * (freq / 1000);
            let cid = crate::smp::per_core::current_core_id();
            while crate::interrupts::rdtsc() < target {
                let rflags: u64;
                unsafe { core::arch::asm!("pushfq; pop {}", out(reg) rflags); }
                let t0 = crate::interrupts::rdtsc();
                if rflags & (1 << 9) != 0 {
                    // SAFETY: IF=1 already → HLT wakes on the timer IRQ.
                    unsafe { core::arch::asm!("hlt"); }
                } else {
                    // SAFETY: enable for the HLT, then restore IF=0.
                    unsafe { core::arch::asm!("sti; hlt; cli"); }
                }
                crate::smp::per_core::record_halt(
                    cid, crate::interrupts::rdtsc().saturating_sub(t0));
                crate::smp::per_core::record_wake(
                    cid, crate::smp::per_core::WAKE_NPK_SLEEP);
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
                // Don't busy-spin, and don't run the old next_task helper
                // here: stealing a fiber task and calling its func directly
                // would bypass the fiber scheduler and re-introduce the
                // nesting freeze (docs/plan/SCHEDULER_FIBERS.md). Just HLT until the
                // next interrupt; the per-core 100 Hz timer wakes us to
                // re-check the key buffer + deadline (≤10 ms latency).
                // IF-preserving. (Only `wifi` uses npk_input_wait today; the
                // panels use npk_event_poll + npk_sleep, which yields.)
                let rflags: u64;
                unsafe { core::arch::asm!("pushfq; pop {}", out(reg) rflags); }
                let t0 = crate::interrupts::rdtsc();
                if rflags & (1 << 9) != 0 {
                    // SAFETY: IF=1 already → HLT wakes on the timer IRQ.
                    unsafe { core::arch::asm!("hlt"); }
                } else {
                    // SAFETY: enable for the HLT, then restore IF=0.
                    unsafe { core::arch::asm!("sti; hlt; cli"); }
                }
                crate::smp::per_core::record_halt(
                    core_id, crate::interrupts::rdtsc().saturating_sub(t0));
                crate::smp::per_core::record_wake(
                    core_id, crate::smp::per_core::WAKE_NPK_SLEEP);
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

    // ── Terminal Stream Sink (for remote debug mirroring) ─────────

    // npk_self_terminal() -> terminal_idx of this WASM task
    linker.func_wrap("env", "npk_self_terminal",
        |caller: Caller<'_, HostState>| -> i32 {
            caller.data().terminal_idx as i32
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_stream_open(idx) -> 0 ok, -1 error
    linker.func_wrap("env", "npk_stream_open",
        |_caller: Caller<'_, HostState>, idx: i32| -> i32 {
            // -1 = the everything-sink: every write, whichever terminal it was
            // routed to. A remote console bound to one index goes silent as soon
            // as output is redirected elsewhere.
            if idx < 0 {
                return if crate::shade::terminal::stream_open_global() { 0 } else { -1 };
            }
            if crate::shade::terminal::stream_open(idx as usize) { 0 } else { -1 }
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_stream_read(idx, buf_ptr, buf_len) -> bytes read (>=0) or -1 on error
    linker.func_wrap("env", "npk_stream_read",
        |mut caller: Caller<'_, HostState>, idx: i32, buf_ptr: i32, buf_len: i32| -> i32 {
            if buf_len <= 0 { return 0; }
            let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(m) => m, None => return -1,
            };
            let data = mem.data_mut(&mut caller);
            let start = buf_ptr as usize;
            let end = start.saturating_add(buf_len as usize);
            if end > data.len() { return -1; }
            if idx < 0 {
                crate::shade::terminal::stream_read_global(&mut data[start..end]) as i32
            } else {
                crate::shade::terminal::stream_read(idx as usize, &mut data[start..end]) as i32
            }
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_stream_close(idx) -> 0
    linker.func_wrap("env", "npk_stream_close",
        |_caller: Caller<'_, HostState>, idx: i32| -> i32 {
            if idx >= 0 { crate::shade::terminal::stream_close(idx as usize); }
            else { crate::shade::terminal::stream_close_global(); }
            0
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_key_inject(byte) -> 0
    // Injects a raw byte into the global keyboard buffer. Routes to the
    // currently-focused window's intent session. Used by debug.wasm.
    linker.func_wrap("env", "npk_key_inject",
        |_caller: Caller<'_, HostState>, byte: i32| -> i32 {
            crate::keyboard::inject_byte((byte & 0xFF) as u8);
            0
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // ── TCP Socket Host Functions (debug shell + future apps) ────

    // npk_tcp_connect(ip_packed, port) -> handle (>=0) or -1 on error.
    // ip_packed = (a << 24) | (b << 16) | (c << 8) | d.
    //
    // NON-BLOCKING: returns as soon as the handshake is started. Ask
    // `npk_tcp_status` until it answers; `npk_tcp_send` refuses until then.
    // It used to block up to 10 s, and a module IS a fiber — so a failing
    // `debug` froze every other fiber on its worker core for those 10 s,
    // the WiFi driver among them. Its card went unpolled (64 RX buffers =
    // milliseconds), and the link died with the command.
    linker.func_wrap("env", "npk_tcp_connect",
        |_caller: Caller<'_, HostState>, ip_packed: i32, port: i32| -> i32 {
            let ip = [
                ((ip_packed >> 24) & 0xFF) as u8,
                ((ip_packed >> 16) & 0xFF) as u8,
                ((ip_packed >> 8) & 0xFF) as u8,
                (ip_packed & 0xFF) as u8,
            ];
            if port <= 0 || port > 65535 { return -1; }
            match crate::net::tcp::connect_start(ip, port as u16) {
                Ok(h) => h as i32,
                Err(_) => -1,
            }
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_tcp_status(handle) -> 1 established, 0 still handshaking, -1 failed.
    // The polling half of the non-blocking connect above. The module sleeps
    // between calls; Core 0's run loop drives the stack meanwhile.
    linker.func_wrap("env", "npk_tcp_status",
        |_caller: Caller<'_, HostState>, handle: i32| -> i32 {
            if handle < 0 { return -1; }
            crate::net::tcp::connect_status(handle as usize)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_tcp_send(handle, buf_ptr, buf_len) -> 0 ok, -2 retry later, -1 error
    linker.func_wrap("env", "npk_tcp_send",
        |caller: Caller<'_, HostState>, handle: i32, buf_ptr: i32, buf_len: i32| -> i32 {
            if handle < 0 || buf_len <= 0 { return -1; }
            let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(m) => m, None => return -1,
            };
            let data = mem.data(&caller);
            let start = buf_ptr as usize;
            let end = start.saturating_add(buf_len as usize);
            if end > data.len() { return -1; }
            match crate::net::tcp::send(handle as usize, &data[start..end]) {
                Ok(_) => 0,
                // Backpressure, not a failure: too much is still unacked.
                // A module that treats this as fatal drops a live connection.
                Err(crate::net::tcp::TcpError::WouldBlock) => -2,
                Err(_) => -1,
            }
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_tcp_recv(handle, buf_ptr, buf_max) -> bytes read (0 = none available), -1 on error
    linker.func_wrap("env", "npk_tcp_recv",
        |mut caller: Caller<'_, HostState>, handle: i32, buf_ptr: i32, buf_max: i32| -> i32 {
            if handle < 0 || buf_max <= 0 { return -1; }
            let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(m) => m, None => return -1,
            };
            let data = mem.data_mut(&mut caller);
            let start = buf_ptr as usize;
            let end = start.saturating_add(buf_max as usize);
            if end > data.len() { return -1; }
            match crate::net::tcp::recv(handle as usize, &mut data[start..end]) {
                Ok(n) => n as i32,
                Err(_) => -1,
            }
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_tcp_close(handle) -> 0. Sends the FIN and returns; the graceful
    // wait is the kernel's job, not a module's — it spun up to 2 s here,
    // and 2 s of a frozen worker core is the WiFi driver not draining.
    linker.func_wrap("env", "npk_tcp_close",
        |_caller: Caller<'_, HostState>, handle: i32| -> i32 {
            if handle >= 0 { let _ = crate::net::tcp::close_nowait(handle as usize); }
            0
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_debug_target_ip() -> packed IP (0 if unset)
    linker.func_wrap("env", "npk_debug_target_ip",
        |_caller: Caller<'_, HostState>| -> i32 {
            get_debug_target().0 as i32
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_debug_target_port() -> port (0 if unset)
    linker.func_wrap("env", "npk_debug_target_port",
        |_caller: Caller<'_, HostState>| -> i32 {
            get_debug_target().1 as i32
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // ── Hardware Driver Host Functions ────────────────────────────

    // npk_pci_bind(vendor_id, device_id) -> 0=ok, -1=not found, -2=denied
    linker.func_wrap("env", "npk_pci_bind",
        |mut caller: Caller<'_, HostState>, vendor: i32, device: i32| -> i32 {
            let vid = vendor as u16;
            let did = device as u16;
            let dev = match pci::find_device(vid, did) {
                Some(d) => d,
                None => return -1,
            };
            let cap_id = caller.data().cap_id;
            let a = dev.addr;
            if capability::check_pci_device(&cap_id, capability::Rights::EXECUTE, a.bus, a.device, a.function).is_err()
                && capability::check_global(&cap_id, capability::Rights::EXECUTE).is_err() {
                kprintln!("[npk] WASM: npk_pci_bind DENIED {:04x}:{:04x}", vid, did);
                return -2;
            }
            caller.data_mut().hw = Some(HwDriverState {
                pci_addr: dev.addr,
                vendor_id: vid,
                device_id: did,
                mmio_maps: Vec::new(),
                dma_allocs: Vec::new(),
                bus_master_enabled: false,
                registered_as_netdev: false,
            });
            kprintln!("[npk] WASM driver bound to {:02x}:{:02x}.{} [{:04x}:{:04x}]",
                a.bus, a.device, a.function, vid, did);
            0
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_pci_bind_class(class, subclass) -> 0=ok, -1=not found, -2=denied
    linker.func_wrap("env", "npk_pci_bind_class",
        |mut caller: Caller<'_, HostState>, class: i32, subclass: i32| -> i32 {
            let cls = class as u8;
            let sub = subclass as u8;
            let dev = match pci::find_by_class(cls, sub) {
                Some(d) => d,
                None => return -1,
            };
            let cap_id = caller.data().cap_id;
            let a = dev.addr;
            if capability::check_pci_device(&cap_id, capability::Rights::EXECUTE, a.bus, a.device, a.function).is_err()
                && capability::check_global(&cap_id, capability::Rights::EXECUTE).is_err() {
                kprintln!("[npk] WASM: npk_pci_bind_class DENIED {:02x}:{:02x}", cls, sub);
                return -2;
            }
            kprintln!("[npk] WASM driver bound to {:02x}:{:02x}.{} [{:04x}:{:04x}]",
                a.bus, a.device, a.function, dev.vendor_id, dev.device_id);
            caller.data_mut().hw = Some(HwDriverState {
                pci_addr: dev.addr,
                vendor_id: dev.vendor_id,
                device_id: dev.device_id,
                mmio_maps: Vec::new(),
                dma_allocs: Vec::new(),
                bus_master_enabled: false,
                registered_as_netdev: false,
            });
            0
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_pci_read_config(offset) -> u32 value or -1
    linker.func_wrap("env", "npk_pci_read_config",
        |caller: Caller<'_, HostState>, offset: i32| -> i32 {
            let hw = match caller.data().hw.as_ref() {
                Some(h) => h,
                None => return -1,
            };
            if offset < 0 || offset > 255 { return -1; }
            pci::read32(hw.pci_addr, offset as u8) as i32
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_pci_write_config(offset, value) -> 0 or -1
    linker.func_wrap("env", "npk_pci_write_config",
        |caller: Caller<'_, HostState>, offset: i32, value: i32| -> i32 {
            let hw = match caller.data().hw.as_ref() {
                Some(h) => h,
                None => return -1,
            };
            if offset < 0 || offset > 255 { return -1; }
            pci::write32(hw.pci_addr, offset as u8, value as u32);
            0
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_pci_enable_bus_master() -> 0 or -1
    linker.func_wrap("env", "npk_pci_enable_bus_master",
        |mut caller: Caller<'_, HostState>| -> i32 {
            let hw = match caller.data_mut().hw.as_mut() {
                Some(h) => h,
                None => return -1,
            };
            pci::enable_bus_master(hw.pci_addr);
            // Also enable memory space
            let cmd = pci::read32(hw.pci_addr, 0x04);
            pci::write32(hw.pci_addr, 0x04, cmd | 0x06);
            hw.bus_master_enabled = true;
            0
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // ── Device-interrupt ABI (MSI-X → LAPIC → fiber wake) ───────────────
    //
    // Lets a WASM driver go IRQ-driven instead of `npk_sleep`-polling: bind a
    // device, `npk_irq_register` its MSI-X entry once, then loop
    //   since = npk_irq_arm(vec); <enable/submit device work>;
    //   npk_irq_wait(vec, since, timeout); <service>
    // The driver's fiber parks until the device fires; the IRQ wakes its core.
    // The driver still enables the device's own interrupt source via its MMIO
    // (e.g. a queue's IRQ-enable) using the existing npk_mmio_* fns.

    // npk_irq_register(entry) -> LAPIC vector (>=0), or -1. Programs the bound
    // device's MSI-X table `entry` to deliver to this driver's core.
    linker.func_wrap("env", "npk_irq_register",
        |caller: Caller<'_, HostState>, entry: i32| -> i32 {
            let hw = match caller.data().hw.as_ref() { Some(h) => h, None => return -1 };
            if !(0..2048).contains(&entry) { return -1; }
            match crate::irq::register(hw.pci_addr, entry as u16) {
                Some(v) => v as i32,
                None => -1,
            }
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_irq_arm(vector) -> fired-count snapshot, or -1 on a bad vector. Call
    // BEFORE submitting/enabling the device work that triggers the IRQ; pass
    // the result to npk_irq_wait. Also routes the IRQ to the calling core.
    linker.func_wrap("env", "npk_irq_arm",
        |_caller: Caller<'_, HostState>, vector: i32| -> i64 {
            let base = crate::interrupts::DEVICE_IRQ_VEC_BASE as i32;
            let count = crate::interrupts::DEVICE_IRQ_VEC_COUNT as i32;
            if vector < base || vector >= base + count { return -1; }
            crate::irq::arm(vector as u8) as i64
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_irq_wait(vector, since, timeout_ms) -> 1 fired, 0 timeout, -1 bad arg.
    // Parks the driver's fiber until the device IRQ advances the fired-count
    // past `since`, or `timeout_ms` elapses (defaults to 1000 if <= 0).
    linker.func_wrap("env", "npk_irq_wait",
        |_caller: Caller<'_, HostState>, vector: i32, since: i64, timeout_ms: i32| -> i32 {
            let base = crate::interrupts::DEVICE_IRQ_VEC_BASE as i32;
            let count = crate::interrupts::DEVICE_IRQ_VEC_COUNT as i32;
            if vector < base || vector >= base + count || since < 0 { return -1; }
            let t = if timeout_ms <= 0 { 1000 } else { timeout_ms as u64 };
            if crate::irq::wait(vector as u8, since as u64, t) { 1 } else { 0 }
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_mmio_map_bar(bar_index, page_count) -> handle or -1
    //
    // Sizes the BAR first and clamps `pages` to the actual BAR size. This
    // prevents drivers from mapping past the end of a BAR into whatever PCI
    // address space follows (usually another device's BAR), which would
    // silently corrupt that device or generate UR responses.
    linker.func_wrap("env", "npk_mmio_map_bar",
        |mut caller: Caller<'_, HostState>, bar_idx: i32, pages: i32| -> i32 {
            let hw = match caller.data_mut().hw.as_mut() {
                Some(h) => h,
                None => return -1,
            };
            if bar_idx < 0 || bar_idx > 5 || pages <= 0 || pages > 256 { return -1; }
            if hw.mmio_maps.len() >= MAX_MMIO_MAPS { return -1; }

            let bar_offset = 0x10 + (bar_idx as u8) * 4;
            let bar_raw = pci::read32(hw.pci_addr, bar_offset);
            let is_64bit = bar_raw & 0x04 != 0;
            let mut bar_base = if is_64bit {
                pci::read_bar64(hw.pci_addr, bar_offset)
            } else {
                (bar_raw & 0xFFFF_FFF0) as u64
            };

            // If BAR is unassigned (UEFI didn't configure it), assign it now.
            // assign_bar_mmio sizes the BAR internally; we just need the base.
            if bar_base == 0 && bar_raw & 0x01 == 0 {
                bar_base = pci::assign_bar_mmio(hw.pci_addr, bar_offset);
                if bar_base == 0 { return -1; }
            }
            if bar_base == 0 { return -1; }

            // Size the BAR: disable memory, write 0xFFFFFFFF, read back, restore.
            // Safe at this point because the driver hasn't started using the
            // BAR yet (mmio_map_bar is the first access after pci_bind).
            let cmd = pci::read32(hw.pci_addr, 0x04);
            pci::write32(hw.pci_addr, 0x04, cmd & !0x02);
            let saved_lo = pci::read32(hw.pci_addr, bar_offset);
            pci::write32(hw.pci_addr, bar_offset, 0xFFFF_FFFF);
            let size_lo = pci::read32(hw.pci_addr, bar_offset);
            pci::write32(hw.pci_addr, bar_offset, saved_lo);
            let bar_size = (!((size_lo & !0xF) as u64)).wrapping_add(1) & 0xFFFF_FFFF;
            pci::write32(hw.pci_addr, 0x04, cmd);

            let max_pages = (bar_size as usize) / 4096;
            let requested = pages as usize;
            let page_count = if requested > max_pages { max_pages } else { requested };

            for i in 0..page_count {
                let addr = bar_base + (i * 4096) as u64;
                // SAFETY: identity-mapped MMIO region for bound PCI device BAR.
                // map_page splits huge pages to set NO_CACHE for MMIO access.
                if let Err(e) = crate::paging::map_page(
                    addr, addr,
                    crate::paging::PageFlags::PRESENT
                        | crate::paging::PageFlags::WRITABLE
                        | crate::paging::PageFlags::NO_CACHE,
                ) {
                    kprintln!("[npk] WASM MMIO map {:#x}: {}", addr, e);
                }
            }
            let handle = hw.mmio_maps.len();
            hw.mmio_maps.push((bar_base, page_count));
            kprintln!("[npk] WASM driver: MMIO BAR{} mapped at {:#x} — BAR size {:#x}, requested {} pages, mapped {} pages",
                bar_idx, bar_base, bar_size, requested, page_count);
            handle as i32
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_mmio_read32(handle, offset) -> u32
    linker.func_wrap("env", "npk_mmio_read32",
        |caller: Caller<'_, HostState>, handle: i32, offset: i32| -> i32 {
            let hw = match caller.data().hw.as_ref() {
                Some(h) => h,
                None => return -1,
            };
            let h = handle as usize;
            if h >= hw.mmio_maps.len() { return -1; }
            let (base, pages) = hw.mmio_maps[h];
            let off = offset as usize;
            if off + 4 > pages * 4096 { return -1; }
            // SAFETY: validated MMIO region within mapped BAR
            unsafe { core::ptr::read_volatile((base + off as u64) as *const u32) as i32 }
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_mmio_write32(handle, offset, value) -> 0 or -1
    linker.func_wrap("env", "npk_mmio_write32",
        |caller: Caller<'_, HostState>, handle: i32, offset: i32, value: i32| -> i32 {
            let hw = match caller.data().hw.as_ref() {
                Some(h) => h,
                None => return -1,
            };
            let h = handle as usize;
            if h >= hw.mmio_maps.len() { return -1; }
            let (base, pages) = hw.mmio_maps[h];
            let off = offset as usize;
            if off + 4 > pages * 4096 { return -1; }
            // SAFETY: validated MMIO region within mapped BAR
            unsafe { core::ptr::write_volatile((base + off as u64) as *mut u32, value as u32) }
            0
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_mmio_read16(handle, offset) -> u16 as i32
    linker.func_wrap("env", "npk_mmio_read16",
        |caller: Caller<'_, HostState>, handle: i32, offset: i32| -> i32 {
            let hw = match caller.data().hw.as_ref() {
                Some(h) => h,
                None => return -1,
            };
            let h = handle as usize;
            if h >= hw.mmio_maps.len() { return -1; }
            let (base, pages) = hw.mmio_maps[h];
            let off = offset as usize;
            if off + 2 > pages * 4096 || off & 0x1 != 0 { return -1; }
            // SAFETY: validated MMIO region within mapped BAR, 2-byte aligned
            unsafe { core::ptr::read_volatile((base + off as u64) as *const u16) as i32 }
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_mmio_write16(handle, offset, value) -> 0 or -1
    // True 16-bit MMIO write — required for split registers like RX/TX BD IDX
    // (HOST_IDX[15:0] + HW_IDX[31:16]). A 32-bit RMW would clobber HW_IDX.
    linker.func_wrap("env", "npk_mmio_write16",
        |caller: Caller<'_, HostState>, handle: i32, offset: i32, value: i32| -> i32 {
            let hw = match caller.data().hw.as_ref() {
                Some(h) => h,
                None => return -1,
            };
            let h = handle as usize;
            if h >= hw.mmio_maps.len() { return -1; }
            let (base, pages) = hw.mmio_maps[h];
            let off = offset as usize;
            if off + 2 > pages * 4096 || off & 0x1 != 0 { return -1; }
            // SAFETY: validated MMIO region within mapped BAR, 2-byte aligned
            unsafe { core::ptr::write_volatile((base + off as u64) as *mut u16, value as u16) }
            0
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_mmio_read64(handle, offset) -> i64
    linker.func_wrap("env", "npk_mmio_read64",
        |caller: Caller<'_, HostState>, handle: i32, offset: i32| -> i64 {
            let hw = match caller.data().hw.as_ref() {
                Some(h) => h,
                None => return -1,
            };
            let h = handle as usize;
            if h >= hw.mmio_maps.len() { return -1; }
            let (base, pages) = hw.mmio_maps[h];
            let off = offset as usize;
            if off + 8 > pages * 4096 { return -1; }
            // SAFETY: validated MMIO region within mapped BAR
            let lo = unsafe { core::ptr::read_volatile((base + off as u64) as *const u32) } as u64;
            let hi = unsafe { core::ptr::read_volatile((base + off as u64 + 4) as *const u32) } as u64;
            (hi << 32 | lo) as i64
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_mmio_write64(handle, offset, value) -> 0 or -1
    linker.func_wrap("env", "npk_mmio_write64",
        |caller: Caller<'_, HostState>, handle: i32, offset: i32, value: i64| -> i32 {
            let hw = match caller.data().hw.as_ref() {
                Some(h) => h,
                None => return -1,
            };
            let h = handle as usize;
            if h >= hw.mmio_maps.len() { return -1; }
            let (base, pages) = hw.mmio_maps[h];
            let off = offset as usize;
            if off + 8 > pages * 4096 { return -1; }
            let v = value as u64;
            // SAFETY: validated MMIO region within mapped BAR
            unsafe {
                core::ptr::write_volatile((base + off as u64) as *mut u32, v as u32);
                core::ptr::write_volatile((base + off as u64 + 4) as *mut u32, (v >> 32) as u32);
            }
            0
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_dma_alloc(page_count) -> handle or -1
    linker.func_wrap("env", "npk_dma_alloc",
        |mut caller: Caller<'_, HostState>, pages: i32| -> i32 {
            let hw = match caller.data_mut().hw.as_mut() {
                Some(h) => h,
                None => return -1,
            };
            if pages <= 0 || pages as usize > MAX_DMA_PAGES_PER_CALL { return -1; }
            let page_count = pages as usize;
            if hw.dma_allocs.len() >= MAX_DMA_ALLOCS { return -1; }
            let total: usize = hw.dma_allocs.iter().map(|(_, p)| *p).sum();
            if total + page_count > MAX_DMA_PAGES { return -1; }

            // DMA buffers MUST be below 4GB — PCIe TX BD has 32-bit address field
            let phys = match crate::memory::allocate_contiguous_below(page_count, 0x1_0000_0000) {
                Some(p) => p,
                None => return -1,
            };
            // SAFETY: zeroing freshly allocated DMA memory
            unsafe { core::ptr::write_bytes(phys as *mut u8, 0, page_count * 4096) }
            let handle = hw.dma_allocs.len();
            hw.dma_allocs.push((phys, page_count));
            handle as i32
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_dma_phys_addr(handle) -> physical address as i64
    linker.func_wrap("env", "npk_dma_phys_addr",
        |caller: Caller<'_, HostState>, handle: i32| -> i64 {
            let hw = match caller.data().hw.as_ref() {
                Some(h) => h,
                None => return -1,
            };
            let h = handle as usize;
            if h >= hw.dma_allocs.len() { return -1; }
            hw.dma_allocs[h].0 as i64
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_dma_read(handle, dma_offset, wasm_ptr, len) -> 0 or -1
    linker.func_wrap("env", "npk_dma_read",
        |mut caller: Caller<'_, HostState>, handle: i32, dma_off: i32,
         wasm_ptr: i32, len: i32| -> i32 {
            let (phys, pages) = {
                let hw = match caller.data().hw.as_ref() {
                    Some(h) => h,
                    None => return -1,
                };
                let h = handle as usize;
                if h >= hw.dma_allocs.len() { return -1; }
                hw.dma_allocs[h]
            };
            let off = dma_off as usize;
            let length = len as usize;
            if off + length > pages * 4096 { return -1; }

            let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(m) => m,
                None => return -1,
            };
            let data = mem.data_mut(&mut caller);
            let dst = wasm_ptr as usize;
            if dst + length > data.len() { return -1; }
            // SAFETY: copying from validated DMA buffer to WASM linear memory
            unsafe {
                core::ptr::copy_nonoverlapping(
                    (phys + off as u64) as *const u8,
                    data[dst..].as_mut_ptr(),
                    length,
                );
            }
            0
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_dma_write(handle, dma_offset, wasm_ptr, len) -> 0 or -1
    linker.func_wrap("env", "npk_dma_write",
        |caller: Caller<'_, HostState>, handle: i32, dma_off: i32,
         wasm_ptr: i32, len: i32| -> i32 {
            let (phys, pages) = {
                let hw = match caller.data().hw.as_ref() {
                    Some(h) => h,
                    None => return -1,
                };
                let h = handle as usize;
                if h >= hw.dma_allocs.len() { return -1; }
                hw.dma_allocs[h]
            };
            let off = dma_off as usize;
            let length = len as usize;
            if off + length > pages * 4096 { return -1; }

            let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(m) => m,
                None => return -1,
            };
            let data = mem.data(&caller);
            let src = wasm_ptr as usize;
            if src + length > data.len() { return -1; }
            // SAFETY: copying from WASM linear memory to validated DMA buffer
            unsafe {
                core::ptr::copy_nonoverlapping(
                    data[src..].as_ptr(),
                    (phys + off as u64) as *mut u8,
                    length,
                );
            }
            0
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_dma_read32(handle, offset) -> u32
    linker.func_wrap("env", "npk_dma_read32",
        |caller: Caller<'_, HostState>, handle: i32, offset: i32| -> i32 {
            let hw = match caller.data().hw.as_ref() {
                Some(h) => h,
                None => return -1,
            };
            let h = handle as usize;
            if h >= hw.dma_allocs.len() { return -1; }
            let (phys, pages) = hw.dma_allocs[h];
            let off = offset as usize;
            if off + 4 > pages * 4096 { return -1; }
            // SAFETY: reading from validated DMA buffer
            unsafe { core::ptr::read_volatile((phys + off as u64) as *const u32) as i32 }
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_dma_write32(handle, offset, value) -> 0 or -1
    linker.func_wrap("env", "npk_dma_write32",
        |caller: Caller<'_, HostState>, handle: i32, offset: i32, value: i32| -> i32 {
            let hw = match caller.data().hw.as_ref() {
                Some(h) => h,
                None => return -1,
            };
            let h = handle as usize;
            if h >= hw.dma_allocs.len() { return -1; }
            let (phys, pages) = hw.dma_allocs[h];
            let off = offset as usize;
            if off + 4 > pages * 4096 { return -1; }
            // SAFETY: writing to validated DMA buffer
            unsafe { core::ptr::write_volatile((phys + off as u64) as *mut u32, value as u32) }
            0
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_memory_fence() -> 0
    linker.func_wrap("env", "npk_memory_fence",
        |_caller: Caller<'_, HostState>| -> i32 {
            core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
            0
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_netdev_register(mac_ptr) -> 0 or -1
    linker.func_wrap("env", "npk_netdev_register",
        |mut caller: Caller<'_, HostState>, mac_ptr: i32| -> i32 {
            let hw = match caller.data_mut().hw.as_mut() {
                Some(h) => h,
                None => return -1,
            };
            if hw.registered_as_netdev { return -1; } // already registered

            let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(m) => m,
                None => return -1,
            };
            let data = mem.data(&caller);
            let start = mac_ptr as usize;
            if start + 6 > data.len() { return -1; }
            let mut mac = [0u8; 6];
            mac.copy_from_slice(&data[start..start + 6]);

            crate::netdev::register_wasm_nic(mac);
            // Re-borrow after register call
            if let Some(h) = caller.data_mut().hw.as_mut() {
                h.registered_as_netdev = true;
            }
            kprintln!("[npk] WASM driver registered as NIC: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]);
            0
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // ── WiFi-class control channel (docs/spec/WIFI_CLASS_ABI.md) ───────────────────
    // A kernel-mediated mailbox pair routing opaque control messages between
    // the vendor driver (wifi_*.wasm) and the supplicant (wifid.wasm). The
    // kernel carries bytes only — no WPA / vendor knowledge. Manager side is
    // NETCTL-gated (only wifid, which declares it in .npk.caps); driver side
    // is gated by being a bound driver (hw state present), like the other
    // device host-fns.

    // npk_wifi_send_cmd(buf_ptr, len) -> 0 / -1 — manager enqueues a command.
    linker.func_wrap("env", "npk_wifi_send_cmd",
        |caller: Caller<'_, HostState>, buf_ptr: i32, len: i32| -> i32 {
            let cap_id = caller.data().cap_id;
            if capability::check_global(&cap_id, capability::Rights::NETCTL).is_err() {
                return -1;
            }
            match read_wasm_bytes(&caller, buf_ptr, len) {
                Some(msg) if crate::wifi::send_cmd(&msg) => 0,
                _ => -1,
            }
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_wifi_poll_event(buf_ptr, max) -> len / -1 — manager dequeues an event.
    linker.func_wrap("env", "npk_wifi_poll_event",
        |mut caller: Caller<'_, HostState>, buf_ptr: i32, max: i32| -> i32 {
            let cap_id = caller.data().cap_id;
            if capability::check_global(&cap_id, capability::Rights::NETCTL).is_err() {
                return -1;
            }
            wifi_poll_into(&mut caller, buf_ptr, max, crate::wifi::poll_event)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_wifi_poll_cmd(buf_ptr, max) -> len / -1 — driver dequeues a command.
    linker.func_wrap("env", "npk_wifi_poll_cmd",
        |mut caller: Caller<'_, HostState>, buf_ptr: i32, max: i32| -> i32 {
            if caller.data().hw.is_none() { return -1; }
            wifi_poll_into(&mut caller, buf_ptr, max, crate::wifi::poll_cmd)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_wifi_send_event(buf_ptr, len) -> 0 / -1 — driver enqueues an event.
    linker.func_wrap("env", "npk_wifi_send_event",
        |caller: Caller<'_, HostState>, buf_ptr: i32, len: i32| -> i32 {
            if caller.data().hw.is_none() { return -1; }
            match read_wasm_bytes(&caller, buf_ptr, len) {
                Some(msg) if crate::wifi::send_event(&msg) => 0,
                _ => -1,
            }
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_driver_report(buf_ptr, len) -> 0 / -1 — a bound driver publishes a
    // short plain-text status snapshot, read back with the `wlan` intent. The
    // kernel stores the bytes and a timestamp and never parses them: what is
    // worth reporting is device knowledge, and that stays in the driver.
    linker.func_wrap("env", "npk_driver_report",
        |caller: Caller<'_, HostState>, buf_ptr: i32, len: i32| -> i32 {
            if caller.data().hw.is_none() { return -1; }
            if len <= 0 || len as usize > crate::drivers::report::REPORT_MAX { return -1; }
            match read_wasm_str(&caller, buf_ptr, len) {
                Some(s) => {
                    let name = caller.data().module_name.clone();
                    crate::drivers::report::store(&name, &s);
                    0
                }
                None => -1,
            }
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // ── WiFi/NIC data path (driver ↔ kernel IP stack via the netdev mailbox) ─

    // npk_netdev_submit_rx(buf_ptr, len) -> 0 / -1 — driver hands a received
    // Ethernet frame to the kernel network stack.
    linker.func_wrap("env", "npk_netdev_submit_rx",
        |caller: Caller<'_, HostState>, buf_ptr: i32, len: i32| -> i32 {
            if caller.data().hw.is_none() { return -1; }
            match read_wasm_bytes(&caller, buf_ptr, len) {
                Some(frame) => { crate::netdev::wasm_nic_submit_rx(&frame); 0 }
                None => -1,
            }
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_netdev_rx_deliver(buf_ptr, len) -> 0 / -1 — driver delivers a received
    // frame STRAIGHT into the IP stack from its own fiber (the NAPI topology:
    // drain → stack in one context, no relay-ring + Core-0 hop). Falls back to
    // the ring internally if Core 0 holds the drain guard. Preferred over
    // npk_netdev_submit_rx for the hot path.
    linker.func_wrap("env", "npk_netdev_rx_deliver",
        |caller: Caller<'_, HostState>, buf_ptr: i32, len: i32| -> i32 {
            if caller.data().hw.is_none() { return -1; }
            match read_wasm_bytes(&caller, buf_ptr, len) {
                Some(frame) => { crate::net::wasm_deliver_rx(&frame); 0 }
                None => -1,
            }
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_netdev_poll_tx(buf_ptr, max) -> len / -1 — driver fetches the next
    // frame the kernel wants transmitted (-1 when none / buffer too small).
    linker.func_wrap("env", "npk_netdev_poll_tx",
        |mut caller: Caller<'_, HostState>, buf_ptr: i32, max: i32| -> i32 {
            if caller.data().hw.is_none() { return -1; }
            let mut frame = [0u8; crate::netdev::MTU];
            let len = match crate::netdev::wasm_nic_poll_tx(&mut frame) {
                Some(n) => n,
                None => return -1,
            };
            if max < 0 || (max as usize) < len { return -1; }
            write_wasm_bytes(&mut caller, buf_ptr, &frame[..len])
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_netdev_set_link(up) -> 0 — the single-flag form. Kept for drivers
    // that know only "usable / not usable"; it reports `up` as carrier with no
    // dormant phase.
    linker.func_wrap("env", "npk_netdev_set_link",
        |caller: Caller<'_, HostState>, up: i32| -> i32 {
            if caller.data().hw.is_none() { return -1; }
            crate::netdev::set_wasm_nic_link(up != 0);
            0
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_netdev_set_link_state(carrier, dormant) -> 0 — the RFC 2863 pair
    // Linux keeps (`rfc2863_policy`): `carrier` = the association exists,
    // `dormant` = it exists but is not usable yet (WPA not done). operstate
    // is UP only when carrier && !dormant. One flag for all three meanings
    // made every authorization phase look like the link had gone away.
    linker.func_wrap("env", "npk_netdev_set_link_state",
        |caller: Caller<'_, HostState>, carrier: i32, dormant: i32| -> i32 {
            if caller.data().hw.is_none() { return -1; }
            crate::netdev::set_wasm_nic_link_state(carrier != 0, dormant != 0);
            0
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    Ok(())
}

/// Read `len` bytes from the caller's linear memory at `ptr` into a Vec.
fn read_wasm_bytes(caller: &Caller<'_, HostState>, ptr: i32, len: i32) -> Option<alloc::vec::Vec<u8>> {
    if ptr < 0 || len <= 0 { return None; }
    let mem = caller.get_export("memory").and_then(|e| e.into_memory())?;
    let data = mem.data(caller);
    let start = ptr as usize;
    let end = start.checked_add(len as usize)?;
    if end > data.len() { return None; }
    Some(data[start..end].to_vec())
}

/// Write `bytes` into the caller's linear memory at `ptr`; returns the length
/// written, or -1 if the region is out of bounds.
fn write_wasm_bytes(caller: &mut Caller<'_, HostState>, ptr: i32, bytes: &[u8]) -> i32 {
    if ptr < 0 { return -1; }
    let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
        Some(m) => m,
        None => return -1,
    };
    let data = mem.data_mut(caller);
    let start = ptr as usize;
    let end = match start.checked_add(bytes.len()) {
        Some(e) => e,
        None => return -1,
    };
    if end > data.len() { return -1; }
    data[start..end].copy_from_slice(bytes);
    bytes.len() as i32
}

/// Shared body for the two poll fns: pull one message from `dequeue` into the
/// caller's buffer (bounded by `max`), returning its length or -1.
fn wifi_poll_into(
    caller: &mut Caller<'_, HostState>,
    buf_ptr: i32,
    max: i32,
    dequeue: fn(&mut [u8]) -> Option<usize>,
) -> i32 {
    if max <= 0 { return -1; }
    let cap = (max as usize).min(crate::wifi::WIFI_MSG_MAX);
    let mut tmp = [0u8; crate::wifi::WIFI_MSG_MAX];
    let len = match dequeue(&mut tmp[..cap]) {
        Some(n) => n,
        None => return -1,
    };
    write_wasm_bytes(caller, buf_ptr, &tmp[..len])
}

/// Free all hardware resources allocated by a WASM driver module.
fn cleanup_hw_state(state: &mut HostState) {
    // A dead driver's snapshot must not read as live numbers.
    crate::drivers::report::clear(&state.module_name);
    if let Some(hw) = state.hw.take() {
        let mut total_pages = 0usize;
        for &(phys, pages) in &hw.dma_allocs {
            crate::memory::deallocate_contiguous(phys, pages);
            total_pages += pages;
        }
        if hw.registered_as_netdev {
            crate::netdev::unregister_wasm_nic();
            // The WiFi driver owns the control channel's device side — clear it
            // so a re-launch doesn't inherit stale commands/events.
            crate::wifi::reset();
        }
        if !hw.dma_allocs.is_empty() || hw.registered_as_netdev {
            kprintln!("[npk] driver cleanup: freed {} DMA buffers ({} pages)",
                hw.dma_allocs.len(), total_pages);
        }
    }
}

/// True if `name` targets the module store (`sys/wasm/…`) or the trust
/// store (`sys/certs/…`) — the two directories where a write is a
/// privilege escalation rather than a file operation.
///
/// WASM apps must NOT write or delete in the module store: it holds the
/// executable modules plus
/// their `.npk.caps` declarations, and modules are NOT re-verified at
/// launch — so an app with WRITE that could overwrite a module (or plant
/// a new one with caps=ALL) would escalate to arbitrary rights. The
/// install/update intents reach npkFS directly (root), not through these
/// host fns, so they are unaffected. The paths layer rejects `.`/`..`
/// segments, so after trimming slashes a literal `sys/wasm/` prefix is
/// the only way to actually land in the module store.
///
/// The trust store is the same class of hole with a different blast
/// radius: a file written under `sys/certs/` becomes a root CA the whole
/// system honours, so an app that could write there could mint itself an
/// anchor and silently authenticate any server it likes. Both directories
/// are read-only to apps for the same reason — writing them is a
/// privilege escalation, not a file operation.
fn is_trust_critical_path(name: &str) -> bool {
    let c = name.trim_matches('/');
    if c == "sys/wasm" || c.starts_with("sys/wasm/") {
        return true;
    }
    // Prefix match on a path SEGMENT — a plain `starts_with` would also
    // catch a sibling like `sys/certsomething`, and missing the trailing
    // separator is how this class of guard usually leaks.
    let certs = crate::tls::certstore::STORE_DIR;
    c == certs || (c.starts_with(certs) && c.as_bytes().get(certs.len()) == Some(&b'/'))
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

/// Extract a WASM custom section's payload by name, walking the module header.
/// Kernel-side counterpart to the reader that used to live in the SDK's
/// app_catalog — here the whole (possibly multi-MB) module is available, so a
/// section at the tail (like `.npk.app_meta`) is always found regardless of
/// module size.
fn extract_wasm_custom_section<'a>(wasm: &'a [u8], target: &str) -> Option<&'a [u8]> {
    if wasm.len() < 8 || &wasm[0..4] != b"\0asm" || wasm[4..8] != [1, 0, 0, 0] {
        return None;
    }
    let mut cur = &wasm[8..];
    while !cur.is_empty() {
        let section_id = cur[0];
        cur = &cur[1..];
        let (size, consumed) = read_wasm_leb128_u32(cur)?;
        cur = &cur[consumed..];
        if size as usize > cur.len() { return None; }
        let (payload, rest) = cur.split_at(size as usize);
        cur = rest;
        if section_id != 0 { continue; } // custom sections only
        let (name_len, nconsumed) = match read_wasm_leb128_u32(payload) {
            Some(p) => p,
            None => continue,
        };
        let name_end = nconsumed + name_len as usize;
        if name_end > payload.len() { continue; }
        if &payload[nconsumed..name_end] == target.as_bytes() {
            return Some(&payload[name_end..]);
        }
    }
    None
}

fn read_wasm_leb128_u32(buf: &[u8]) -> Option<(u32, usize)> {
    let mut result: u32 = 0;
    let mut shift: u32 = 0;
    for (i, &b) in buf.iter().enumerate() {
        if shift >= 32 { return None; }
        let payload = (b & 0x7F) as u32;
        if shift == 28 && (payload & !0x0F) != 0 { return None; }
        result |= payload << shift;
        if (b & 0x80) == 0 {
            return Some((result, i + 1));
        }
        shift += 7;
    }
    None
}

/// True if the calling WASM app owns the currently-focused widget window.
/// The clipboard focus-gate: only the focused app may touch the clipboard,
/// so a background app cannot snoop or poison it.
fn app_is_focused(caller: &Caller<'_, HostState>) -> bool {
    let wid = caller.data().widget_window_id;
    wid != 0 && crate::shade::focused_widget_id() == Some(wid)
}

fn map_exec_error(e: wasmi::Error) -> WasmError {
    let msg = alloc::format!("{}", e);
    if msg.contains("fuel") { WasmError::FuelExhausted } else { WasmError::ExecutionFailed }
}

/// Serialize one npk_fs_list entry into `out`.
/// Format: name\0size_le_u64\0is_dir_u8\0mtime_le_u64, entries separated by '\n'.
/// `mtime` is UTC seconds since the Unix epoch — zero means unknown.
fn append_entry(out: &mut alloc::vec::Vec<u8>, name: &str, size: u64, is_dir: bool, mtime: u64) {
    if !out.is_empty() { out.push(b'\n'); }
    out.extend_from_slice(name.as_bytes());
    out.push(0);
    out.extend_from_slice(&size.to_le_bytes());
    out.push(0);
    out.push(if is_dir { 1 } else { 0 });
    out.push(0);
    out.extend_from_slice(&mtime.to_le_bytes());
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

