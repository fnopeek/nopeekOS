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
    vendor_id: u16,
    device_id: u16,
    mmio_maps: Vec<(u64, usize)>,   // handle -> (base_virt, page_count)
    dma_allocs: Vec<(u64, usize)>,  // handle -> (phys_addr, page_count)
    bus_master_enabled: bool,
    registered_as_netdev: bool,
}

const MAX_MMIO_MAPS: usize = 4;
const MAX_DMA_ALLOCS: usize = 64;
const MAX_DMA_PAGES: usize = 256; // 1MB total

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
}

static ENGINE: Mutex<Option<Engine>> = Mutex::new(None);

/// Default fuel budget per module execution (~10M instructions)
const DEFAULT_FUEL: u64 = 10_000_000;

/// Fuel budget for interactive apps and drivers — effectively unlimited.
const INTERACTIVE_FUEL: u64 = u64::MAX / 2;

// ── Worker-Core WASM Jobs ──────────────────────────────────────

const MAX_WASM_JOBS: usize = 4;

struct WasmJob {
    bytes: Vec<u8>,
    cap_id: CapId,
    terminal_idx: u8,
    name: [u8; 32],
    name_len: u8,
    /// Pre-allocated widget window id for widget-kind apps. 0 = app
    /// will get a window on its first npk_scene_commit (classic path).
    widget_window_id: u32,
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

/// Target IP/port for the debug reverse-mirror module. Packed as
/// `(ip as u64) << 16 | port as u64`. Set by the `debug` intent dispatcher
/// before spawning debug.wasm. 0 = unset.
static DEBUG_TARGET: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

pub fn set_debug_target(ip_packed: u32, port: u16) {
    let v = ((ip_packed as u64) << 16) | (port as u64);
    DEBUG_TARGET.store(v, AtOrd::Release);
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
    spawn_on_worker_inner(wasm_bytes, cap_id, terminal_idx, module_name, true, 0)
}

/// Spawn a WASM module as a background task. Unlike spawn_on_worker, this does
/// NOT set APP_RUNNING for the terminal — the intent shell keeps receiving keys
/// and the window continues to function normally. Used by debug.wasm.
pub fn spawn_on_worker_background(wasm_bytes: Vec<u8>, cap_id: CapId, terminal_idx: u8, module_name: &str) -> bool {
    spawn_on_worker_inner(wasm_bytes, cap_id, terminal_idx, module_name, false, 0)
}

/// Spawn a widget-kind WASM app (Phase 10). The caller pre-allocates a
/// widget window and passes its id — the worker sets `widget_window_id`
/// in HostState so the first `npk_scene_commit` targets it directly.
/// Does NOT allocate a terminal or set APP_RUNNING — widget apps use
/// `npk_event_poll` for input, not the per-terminal key buffer.
pub fn spawn_widget_app(wasm_bytes: Vec<u8>, cap_id: CapId, module_name: &str, widget_wid: u32) -> bool {
    spawn_on_worker_inner(wasm_bytes, cap_id, 255, module_name, false, widget_wid)
}

fn spawn_on_worker_inner(
    wasm_bytes: Vec<u8>, cap_id: CapId, terminal_idx: u8, module_name: &str,
    foreground: bool, widget_wid: u32,
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
        widget_window_id: widget_wid,
    });
    drop(jobs);

    // Clear per-app input buffer + mark terminal as having an app (foreground only)
    if foreground {
        clear_app_key_buf(terminal_idx);
        if (terminal_idx as usize) < MAX_APP_BUFS {
            APP_RUNNING[terminal_idx as usize].store(true, AtOrd::Release);
        }
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
        hw: None,
        widget_window_id: job.widget_window_id,
        module_name: String::from(name_str),
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
    execute_inner(wasm_bytes, func_name, args, capability::CAP_NULL, DEFAULT_FUEL)
}

/// Execute a WASM module loaded from npkFS with capability-gated host functions.
/// The module receives a delegated capability token. Runs with the default
/// fuel budget (DEFAULT_FUEL, ~10M instructions).
pub fn execute_sandboxed(
    wasm_bytes: &[u8], func_name: &str, args: &[Val], cap_id: CapId,
) -> Result<WasmResult, WasmError> {
    execute_inner(wasm_bytes, func_name, args, cap_id, DEFAULT_FUEL)
}

/// Execute a WASM module with an explicit fuel budget. Use for trusted
/// first-party modules whose work is deterministically bounded by input
/// parameters (e.g. wallpaper generation sized by resolution). Do NOT
/// use for unbundled / untrusted modules — that's what DEFAULT_FUEL
/// guards against.
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
        hw: None,
        widget_window_id: 0,
        module_name: String::new(),
    });
    store.set_fuel(fuel).map_err(|_| WasmError::ExecutionFailed)?;

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

            let result = crate::shade::widgets::scene_commit(&payload, prev_window);

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

            let entries = match crate::npkfs::list() {
                Ok(v) => v,
                Err(_) => return -1,
            };

            // Build the NUL-separated list. Strip the "sys/wasm/" prefix.
            let prefix = "sys/wasm/";
            let mut out: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
            for (name, _size, _hash) in &entries {
                if let Some(stripped) = name.strip_prefix(prefix) {
                    // Skip sub-directory-like entries (shouldn't occur,
                    // but defensive — names with '/' aren't valid modules).
                    if stripped.contains('/') || stripped.is_empty() { continue; }
                    // `.version` files are sidecars written by the
                    // installer — not standalone modules.
                    if stripped.ends_with(".version") { continue; }
                    if !out.is_empty() { out.push(0); }
                    out.extend_from_slice(stripped.as_bytes());
                }
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

            let module_cap = match capability::create_module_cap(
                capability::Rights::READ
                    | capability::Rights::EXECUTE
                    | capability::Rights::RENDER,
                Some(600_000),
            ) {
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

            let ok = crate::shade::with_compositor(|comp|
                comp.set_overlay(crate::shade::WindowId(wid), w as u32, h as u32)
            ).unwrap_or(false);

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

    // npk_fs_list(prefix_ptr, prefix_len, out_ptr, out_cap, recursive) -> i32
    // Enumerate npkFS keys under `prefix`. If recursive=0, only direct
    // children are returned (keys that contain no '/' after the prefix,
    // plus the unique directory bucket names that do). If recursive=1,
    // every key under the prefix is emitted verbatim.
    //
    // Wire format of the output buffer — one entry per line, separated
    // by '\n' (no trailing newline after the last):
    //   <name>\0<size_le_u64:8>\0<is_dir_u8>
    // - <name> is relative to `prefix` (prefix itself + trailing slash
    //   stripped). For a synthetic directory entry (first path component
    //   encountered in recursive scan), size=0 and is_dir=1.
    // - Size is little-endian 8 bytes. is_dir is 0 or 1.
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

            let entries = match crate::npkfs::list() {
                Ok(v) => v,
                Err(_) => return -1,
            };

            // Build the output buffer. For non-recursive, synthesize
            // directory entries from the first unique path segment.
            let norm_prefix = if prefix.is_empty() || prefix.ends_with('/') {
                prefix.clone()
            } else {
                alloc::format!("{}/", prefix)
            };

            let mut seen_dirs: alloc::vec::Vec<alloc::string::String> = alloc::vec::Vec::new();
            let mut out: alloc::vec::Vec<u8> = alloc::vec::Vec::new();

            for (name, size, _hash) in &entries {
                if !norm_prefix.is_empty() && !name.starts_with(&norm_prefix) {
                    continue;
                }
                // `.dir` markers are internal — surface them as directory
                // entries using the parent component name instead.
                let rel = &name[norm_prefix.len()..];
                if rel.is_empty() { continue; }

                if recursive != 0 {
                    // Skip .dir markers entirely in recursive mode — they
                    // are bookkeeping, not content.
                    if rel.ends_with("/.dir") || rel == ".dir" { continue; }
                    append_entry(&mut out, rel, *size, false);
                    continue;
                }

                // Non-recursive: collapse sub-paths into directory entries.
                match rel.find('/') {
                    None => {
                        if rel == ".dir" { continue; }
                        append_entry(&mut out, rel, *size, false);
                    }
                    Some(i) => {
                        let dir_name = &rel[..i];
                        if !seen_dirs.iter().any(|d| d == dir_name) {
                            seen_dirs.push(dir_name.into());
                            append_entry(&mut out, dir_name, 0, true);
                        }
                    }
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
    // Write 9 bytes into out_ptr: size_le_u64 (8) + is_dir_u8 (1).
    // Directory = a `.dir` marker exists at `<name>/.dir`.
    // Returns 9 on success, 0 if neither key nor directory exists, -1 on cap / args.
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

            let (size, is_dir) = if crate::npkfs::exists(&alloc::format!("{}/.dir", name)) {
                (0u64, 1u8)
            } else {
                match crate::npkfs::fetch(&name) {
                    Ok((data, _)) => (data.len() as u64, 0u8),
                    Err(_) => return 0,
                }
            };

            let mut buf = [0u8; 9];
            buf[0..8].copy_from_slice(&size.to_le_bytes());
            buf[8] = is_dir;
            let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(m) => m,
                None => return -1,
            };
            let data = mem.data_mut(&mut caller);
            let start = out_ptr as usize;
            if start + 9 > data.len() { return -1; }
            data[start..start + 9].copy_from_slice(&buf);
            9
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
            match crate::npkfs::delete(&name) {
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

                // ── Process tracking (keys 20-29) → process table ──
                // 20: count, 21: pid_at_index, 22-29: query by PID
                20..=29 => crate::process::sys_info(key),
                _ => -1,
            }
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_sleep(ms) -> 0 — sleep for N milliseconds.
    // HLT would deliver real idle power, but worker cores don't have
    // their own APIC timer yet (Phase 9 feature), so HLT stays asleep
    // until an IPI — keyboard IRQs fire on BSP and never wake the
    // worker, so drun stopped receiving keys. Back to pause-spin
    // until per-core timer lands.
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
            if idx < 0 { return -1; }
            if crate::shade::terminal::stream_open(idx as usize) { 0 } else { -1 }
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_stream_read(idx, buf_ptr, buf_len) -> bytes read (>=0) or -1 on error
    linker.func_wrap("env", "npk_stream_read",
        |mut caller: Caller<'_, HostState>, idx: i32, buf_ptr: i32, buf_len: i32| -> i32 {
            if idx < 0 || buf_len <= 0 { return 0; }
            let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(m) => m, None => return -1,
            };
            let data = mem.data_mut(&mut caller);
            let start = buf_ptr as usize;
            let end = start.saturating_add(buf_len as usize);
            if end > data.len() { return -1; }
            crate::shade::terminal::stream_read(idx as usize, &mut data[start..end]) as i32
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_stream_close(idx) -> 0
    linker.func_wrap("env", "npk_stream_close",
        |_caller: Caller<'_, HostState>, idx: i32| -> i32 {
            if idx >= 0 { crate::shade::terminal::stream_close(idx as usize); }
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
    // ip_packed = (a << 24) | (b << 16) | (c << 8) | d. Blocks until
    // ESTABLISHED or 10s timeout. Runs on worker core only.
    linker.func_wrap("env", "npk_tcp_connect",
        |_caller: Caller<'_, HostState>, ip_packed: i32, port: i32| -> i32 {
            let ip = [
                ((ip_packed >> 24) & 0xFF) as u8,
                ((ip_packed >> 16) & 0xFF) as u8,
                ((ip_packed >> 8) & 0xFF) as u8,
                (ip_packed & 0xFF) as u8,
            ];
            if port <= 0 || port > 65535 { return -1; }
            match crate::net::tcp::connect(ip, port as u16) {
                Ok(h) => h as i32,
                Err(_) => -1,
            }
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_tcp_send(handle, buf_ptr, buf_len) -> 0 on success, -1 on error
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

    // npk_tcp_close(handle) -> 0
    linker.func_wrap("env", "npk_tcp_close",
        |_caller: Caller<'_, HostState>, handle: i32| -> i32 {
            if handle >= 0 { let _ = crate::net::tcp::close(handle as usize); }
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
            if pages <= 0 || pages > 64 { return -1; }
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

    Ok(())
}

/// Free all hardware resources allocated by a WASM driver module.
fn cleanup_hw_state(state: &mut HostState) {
    if let Some(hw) = state.hw.take() {
        let mut total_pages = 0usize;
        for &(phys, pages) in &hw.dma_allocs {
            crate::memory::deallocate_contiguous(phys, pages);
            total_pages += pages;
        }
        if hw.registered_as_netdev {
            crate::netdev::unregister_wasm_nic();
        }
        if !hw.dma_allocs.is_empty() || hw.registered_as_netdev {
            kprintln!("[npk] driver cleanup: freed {} DMA buffers ({} pages)",
                hw.dma_allocs.len(), total_pages);
        }
    }
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

/// Serialize one npk_fs_list entry into `out`.
/// Format: name\0size_le_u64\0is_dir_u8, entries separated by '\n'.
fn append_entry(out: &mut alloc::vec::Vec<u8>, name: &str, size: u64, is_dir: bool) {
    if !out.is_empty() { out.push(b'\n'); }
    out.extend_from_slice(name.as_bytes());
    out.push(0);
    out.extend_from_slice(&size.to_le_bytes());
    out.push(0);
    out.push(if is_dir { 1 } else { 0 });
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
