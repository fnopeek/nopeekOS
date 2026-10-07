//! WASM Runtime
//!
//! Sandboxed execution via wasmi interpreter.
//! Every host function is capability-gated.
//! Modules loaded from npkFS execute with delegated capabilities —
//! no ambient authority, no access beyond what was explicitly granted.

use alloc::string::String;
use alloc::vec::Vec;
use wasmi::{Config, Engine, Linker, Module, Store, Val};
use spin::Mutex;
use crate::{kprint, kprintln, capability};
use crate::capability::CapId;
use crate::drivers::pci;

pub(crate) mod host_core;
pub(crate) mod forge_glue;

// ── Which engine runs a module ────────────────────────────────────────
//
// One persistent flag instead of a choice per spawn path: autostart,
// drivers, services and one-shot runs all enter elsewhere, and a module
// must run on the same engine it uses in normal operation. The intent shell
// is native, so a prompt stays reachable even if a module fails.
static FORGE_DEFAULT: AtomicBool = AtomicBool::new(false);

/// Read the engine choice from the config. Call after `config::load()`.
///
/// Without an entry forge is the default. `wasm.engine=wasmi` switches back
/// (set via `forge default off`, which works even if no module starts).
pub fn load_engine_default() {
    let on = crate::config::get("wasm.engine").as_deref() != Some("wasmi");
    FORGE_DEFAULT.store(on, AtOrd::Release);
    kprintln!("[npk] WASM: {}", if on { "forge" } else { "wasmi (configured)" });
}

/// The engine used when the caller does not say otherwise.
pub fn forge_is_default() -> bool {
    FORGE_DEFAULT.load(AtOrd::Acquire)
}

/// Switch the default engine and persist it.
pub fn set_engine_default(forge: bool) {
    FORGE_DEFAULT.store(forge, AtOrd::Release);
    crate::config::set("wasm.engine", if forge { "forge" } else { "wasmi" });
}

pub struct WasmResult {
    pub output: String,
}

/// Hardware driver state for WASM modules that access PCI devices.
pub(crate) struct HwDriverState {
    /// Whether this state belongs to a PCI device. A driver using
    /// `npk_mmio_map_phys` may drive non-PCI hardware (e.g. the DesignWare
    /// I2C in the AMD FCH); then `pci_addr` is meaningless and every
    /// config-space access must be refused instead of landing on 00:00.0.
    is_pci: bool,
    pci_addr: pci::PciAddr,
    #[allow(dead_code)] // populated for future audit/debug, not yet read
    vendor_id: u16,
    #[allow(dead_code)]
    device_id: u16,
    mmio_maps: Vec<(u64, usize)>,   // handle -> (base_virt, page_count)
    dma_allocs: Vec<(u64, usize)>,  // handle -> (phys_addr, page_count)
    bus_master_enabled: bool,
    registered_as_netdev: bool,
    /// The device-IRQ vector this driver registered, 0 = none. A driver
    /// may arm and wait on this vector only — any vector of the pool would
    /// let a module re-route another driver's interrupt to its own core.
    irq_vector: u8,
    /// `irq::fired_count` of that vector when `npk_wait` last reported it.
    irq_seen: u64,
}

const MAX_MMIO_MAPS: usize = 4;
/// Per-module DMA allocation slots. A WiFi driver needs many one-page
/// receive buffers plus its rings (Linux allocates 2048 for the AX200). Each
/// slot is a (phys, pages) pair, so the ceiling is bookkeeping, not memory.
const MAX_DMA_ALLOCS: usize = 1024;
const MAX_DMA_PAGES: usize = 2048; // 8MB total (iwlwifi FW sections ~1.3MB)
const MAX_DMA_PAGES_PER_CALL: usize = 1024; // 4MB; a single FW section can exceed 256KB

/// Linear memory a wasmi instance may hold. Fixed, unlike forge's rule:
/// wasmi's memories live on the kernel heap, which is a bounded region and
/// not the frame pool `forge_rt::may_map` watches.
const WASMI_MEMORY_BYTES: usize = 1024 * 1024 * 1024;

/// wasmi's caps: linear memory up to `WASMI_MEMORY_BYTES` (initial size
/// included), tables up to a million entries, one memory. A refused growth
/// fails the instruction instead of exhausting the kernel heap.
fn guest_limits() -> wasmi::StoreLimits {
    wasmi::StoreLimitsBuilder::new()
        .memory_size(WASMI_MEMORY_BYTES)
        .table_elements(1 << 20)
        .memories(1)
        .build()
}

pub(crate) struct HostState {
    /// wasmi's resource limits for this instance (`guest_limits`). forge
    /// enforces the same caps in `forge_rt` and ignores this field.
    limits: wasmi::StoreLimits,
    output: String,
    pub(crate) cap_id: CapId,
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
    /// Network reach of the document this module currently shows.
    ///
    /// Defaults to `Public`, the strict choice: a module that never says
    /// otherwise cannot reach the private network. Only `npk_net_context`
    /// with a private address widens it, and only after the kernel has
    /// resolved that address itself.
    pub(crate) net_reach: crate::intent::http::Reach,
    /// Shade window id owned by this WASM app for widget rendering.
    /// 0 = no widget window yet (first scene_commit allocates one).
    /// Set when the app calls npk_scene_commit, reused on subsequent
    /// commits so the same window is updated in place.
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
    /// without it, and guessing the charset wrong breaks the whole page.
    http_content_type: Option<String>,
    /// Why the last `npk_http_request` failed, as `kind\tmessage`. Read
    /// back via `npk_http_last_error`. Without it every failure reaches the
    /// caller as a bare -1.
    http_last_error: Option<String>,
    /// The last `npk_http_send` response's header block and status. Read back
    /// via `npk_http_response_headers` / `npk_http_status`. A browser needs
    /// headers the body cannot carry — `Set-Cookie` above all, which repeats
    /// and so could never be a single-value getter.
    http_reply_headers: Option<String>,
    http_status: u16,
    /// A `wasi_snapshot_preview1` grant, or None.
    ///
    /// The namespace is linked for every module, but every function in
    /// it bounces on `ENOTCAPABLE` unless this is `Some`. So "can this
    /// program see a filesystem" is decided once, here, by whoever
    /// spawned it — not by what the program chooses to import.
    pub(crate) wasi: Option<alloc::boxed::Box<crate::wasi::WasiCtx>>,
    /// Raw TCP connections this module opened. The connection table is
    /// shared with the kernel's own clients, so a handle is honoured only
    /// if it is listed here; all of them close when the module ends.
    pub(crate) tcp_handles: Vec<usize>,
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
// the array as soon as it starts, so this caps pending (not-yet-started)
// spawns; it must leave room for a one-shot while resident apps start.
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
    /// Run this one under forge instead of the interpreter. Per job, not
    /// global, so a bug in the bridge does not take down every app.
    use_forge: bool,
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

use core::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering as AtOrd};

const APP_KEY_BUF_SIZE: usize = 32;
const MAX_APP_BUFS: usize = 256;
/// `terminal_idx` sentinel for "whatever terminal is focused". Spawn paths that
/// have no window of their own pass it (drivers from autostart, sandboxed
/// runs). It must be excluded before treating the index as a slot: 255 is
/// smaller than MAX_APP_BUFS, so it passes the bounds check and would land in
/// `write_idx(255)`, a slot that is never allocated, and output would be
/// dropped silently.
const TERM_IDX_ACTIVE: u8 = 255;

/// Single-producer (Core 0) / single-consumer (the app's fiber) key ring.
struct KeyRing {
    buf: [AtomicU8; APP_KEY_BUF_SIZE],
    head: AtomicUsize,
    tail: AtomicUsize,
}

static APP_KEY_BUFS: [KeyRing; MAX_APP_BUFS] = [const {
    KeyRing {
        buf: [const { AtomicU8::new(0) }; APP_KEY_BUF_SIZE],
        head: AtomicUsize::new(0),
        tail: AtomicUsize::new(0),
    }
}; MAX_APP_BUFS];

/// The key ring of a real terminal. `TERM_IDX_ACTIVE` has none: every module
/// spawned without a terminal shares that index.
fn key_ring(terminal_idx: u8) -> Option<&'static KeyRing> {
    if terminal_idx == TERM_IDX_ACTIVE { return None; }
    APP_KEY_BUFS.get(terminal_idx as usize)
}

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
    let Some(ring) = key_ring(terminal_idx) else { return };
    let h = ring.head.load(AtOrd::Relaxed);
    let next = (h + 1) % APP_KEY_BUF_SIZE;
    if next != ring.tail.load(AtOrd::Acquire) {
        ring.buf[h].store(key, AtOrd::Relaxed);
        ring.head.store(next, AtOrd::Release);
    }
    let w = APP_KEY_WAKER[terminal_idx as usize].load(AtOrd::Acquire);
    if w != crate::smp::fiber::NO_WAKER {
        crate::smp::fiber::signal(w, crate::smp::fiber::SIG_EVENT);
    }
}

/// The fiber of the app reading each terminal's key buffer, registered when
/// it waits. A key wakes it at once instead of on the next poll.
static APP_KEY_WAKER: [core::sync::atomic::AtomicU32; MAX_APP_BUFS] =
    [const { core::sync::atomic::AtomicU32::new(crate::smp::fiber::NO_WAKER) }; MAX_APP_BUFS];

pub(crate) fn set_key_waker(terminal_idx: u8, w: crate::smp::fiber::Waker) {
    if let Some(slot) = APP_KEY_WAKER.get(terminal_idx as usize) {
        slot.store(w, AtOrd::Release);
    }
}

/// Is a key waiting in this terminal's app buffer?
pub(crate) fn has_app_key(terminal_idx: u8) -> bool {
    key_ring(terminal_idx)
        .is_some_and(|r| r.head.load(AtOrd::Acquire) != r.tail.load(AtOrd::Relaxed))
}

/// Pop a key from an app's input buffer. Called from worker core.
fn pop_app_key(terminal_idx: u8) -> Option<u8> {
    let ring = key_ring(terminal_idx)?;
    let t = ring.tail.load(AtOrd::Relaxed);
    if t == ring.head.load(AtOrd::Acquire) { return None; }
    let key = ring.buf[t].load(AtOrd::Relaxed);
    ring.tail.store((t + 1) % APP_KEY_BUF_SIZE, AtOrd::Release);
    Some(key)
}

/// Clear an app's key buffer. Called when spawning a new app.
fn clear_app_key_buf(terminal_idx: u8) {
    let Some(ring) = key_ring(terminal_idx) else { return };
    ring.head.store(0, AtOrd::Relaxed);
    ring.tail.store(0, AtOrd::Relaxed);
    APP_KEY_WAKER[terminal_idx as usize].store(crate::smp::fiber::NO_WAKER, AtOrd::Release);
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

/// Like [`spawn_on_worker`], but with a launch argument; the path a terminal
/// launch of a windowed app takes. The blocking path (`execute_inner`) runs
/// with `pid: 0`, and `fetch::begin_one` refuses async fetches without a
/// process.
pub fn spawn_on_worker_with_arg(
    wasm_bytes: Vec<u8>, cap_id: CapId, terminal_idx: u8, module_name: &str,
    launch_arg: Option<String>,
) -> bool {
    spawn_on_worker_inner(wasm_bytes, cap_id, terminal_idx, module_name, true, 0, launch_arg)
}

/// Spawn a WASM module as a background task. Unlike spawn_on_worker, this does
/// not set APP_RUNNING for the terminal — the intent shell keeps receiving keys
/// and the window continues to function normally. Used by debug.wasm.
pub fn spawn_on_worker_background(wasm_bytes: Vec<u8>, cap_id: CapId, terminal_idx: u8, module_name: &str) -> bool {
    spawn_on_worker_inner(wasm_bytes, cap_id, terminal_idx, module_name, false, 0, None)
}

/// Spawn a widget-kind WASM app. The caller pre-allocates a
/// widget window and passes its id — the worker sets `widget_window_id`
/// in HostState so the first `npk_scene_commit` targets it directly.
/// Does not allocate a terminal or set APP_RUNNING — widget apps use
/// `npk_event_poll` for input, not the per-terminal key buffer.
pub fn spawn_widget_app(wasm_bytes: Vec<u8>, cap_id: CapId, module_name: &str, widget_wid: u32) -> bool {
    spawn_on_worker_inner(wasm_bytes, cap_id, 255, module_name, false, widget_wid, None)
}

/// Like `spawn_on_worker`, but under forge: same job queue, same window.
pub fn spawn_on_worker_forge(wasm_bytes: Vec<u8>, cap_id: CapId, terminal_idx: u8, module_name: &str) -> bool {
    spawn_on_worker_inner_engine(wasm_bytes, cap_id, terminal_idx, module_name, true, 0, None, true)
}

fn spawn_on_worker_inner(
    wasm_bytes: Vec<u8>, cap_id: CapId, terminal_idx: u8, module_name: &str,
    foreground: bool, widget_wid: u32, launch_arg: Option<String>,
) -> bool {
    // Autostart, drivers, services and widget apps all pass here and run on
    // the default engine like everything else.
    let e = forge_is_default();
    spawn_on_worker_inner_engine(
        wasm_bytes, cap_id, terminal_idx, module_name, foreground, widget_wid, launch_arg, e)
}

#[allow(clippy::too_many_arguments)]
fn spawn_on_worker_inner_engine(
    wasm_bytes: Vec<u8>, cap_id: CapId, terminal_idx: u8, module_name: &str,
    foreground: bool, widget_wid: u32, launch_arg: Option<String>, use_forge: bool,
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
        widget_window_id: widget_wid, launch_arg, use_forge,
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
    if job.use_forge {
        forge_worker_task(slot, job);
        return;
    }
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
        limits: guest_limits(),
        output: String::new(),
        cap_id: job.cap_id,
        net_reach: crate::intent::http::Reach::Public,
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
        wasi: None,
        tcp_handles: Vec::new(),
    });
    store.limiter(|s| &mut s.limits);
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
    cleanup_instance_state(store.data_mut());

    // Drop this instance's per-path grants. They were handed out for one
    // pick and must not outlive the app that got them — a later instance
    // reusing the capability id would otherwise inherit a file it never
    // asked for.
    capability::revoke_path_grants(&store.data().cap_id);
    capability::release_module_cap(&store.data().cap_id);

    // Update final memory usage
    if let Some(mem) = instance.get_memory(&store, "memory") {
        crate::process::set_memory(pid, mem.data_size(&store) as u32);
    }

    // Deregister process + clear app marker + signal completion
    crate::process::exit(pid);
    if (terminal_idx as usize) < MAX_APP_BUFS {
        APP_RUNNING[terminal_idx as usize].store(false, AtOrd::Release);
        crate::intent::wake_shell();
    }
    JOB_DONE[slot].store(true, core::sync::atomic::Ordering::Release);
}

/// The same job under forge. Kept separate from `wasm_worker_task` so the
/// interpreter path stays untouched; the cost is a duplicated cleanup tail.
fn forge_worker_task(slot: usize, job: WasmJob) {
    let terminal_idx = job.terminal_idx;
    let core_id = crate::smp::per_core::current_core_id();
    let name_str = core::str::from_utf8(&job.name[..job.name_len as usize]).unwrap_or("?");
    let pid = crate::process::spawn(name_str, crate::process::KIND_WASM, terminal_idx, core_id as u8);

    // From here every exit must clean up, otherwise a process is left behind
    // and the terminal stops accepting keys.
    let done = |pid: u32, terminal_idx: u8, slot: usize| {
        crate::process::exit(pid);
        if (terminal_idx as usize) < MAX_APP_BUFS {
            APP_RUNNING[terminal_idx as usize].store(false, AtOrd::Release);
        crate::intent::wake_shell();
        }
        JOB_DONE[slot].store(true, core::sync::atomic::Ordering::Release);
    };

    let t0 = crate::interrupts::ticks();
    let m = match forge_core::compile(&job.bytes) {
        Ok(m) => m,
        Err(e) => {
            kprintln!("[npk] forge: {} liess sich nicht uebersetzen — {}", name_str, e);
            done(pid, terminal_idx, slot);
            return;
        }
    };
    let compile_ms = crate::interrupts::ticks().saturating_sub(t0) * 10;

    let entry = m.plan.exports.iter().find(|(n, _)| n == "_start").map(|(_, i)| *i)
        .and_then(|fidx| m.offset_of(fidx));
    let Some(off) = entry else {
        kprintln!("[npk] forge: {} hat kein _start", name_str);
        done(pid, terminal_idx, slot);
        return;
    };

    // Here we own the state (under wasmi the Store holds it). It must not
    // move while the instance holds its pointer in the vmctx.
    let mut hs = HostState {
        limits: guest_limits(),
        output: String::new(),
        cap_id: job.cap_id,
        net_reach: crate::intent::http::Reach::Public,
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
        wasi: None,
        tcp_handles: Vec::new(),
    };

    let host = forge_glue::NpkHost(&raw mut hs);
    let Some(mut inst) = crate::forge_rt::Instance::new_with_host(&m, &host) else {
        // The most common cause is a lack of frames, so report free memory.
        let (frames, mb) = crate::mm::memory::stats();
        kprintln!("[npk] forge: {} — Instanz liess sich nicht bauen (frei: {} Rahmen = {} MB)",
            name_str, frames, mb);
        done(pid, terminal_idx, slot);
        return;
    };
    // An import bound to the trap stub stops on first call; say so now
    // rather than have it look like a crash later.
    let open = inst.unresolved_imports();
    if open > 0 {
        kprintln!("[npk] forge: {} — {} Importe unaufgeloest, das Modul wird stehenbleiben",
            name_str, open);
    }
    crate::kdebug!("[npk] forge: {} compiled in {} ms ({} B x86)", name_str, compile_ms, m.code.len());

    inst.set_fuel(INTERACTIVE_FUEL as i64);
    crate::process::set_memory(pid, inst.memory_size() as u32);

    let (_ret, trap) = inst.call(off, 0, 0, 0);
    if trap != forge_core::trap::NONE {
        kprintln!("[npk] forge: {} endete mit {}", name_str, forge_core::trap::name(trap));
    }

    // As in the interpreter path: release hardware, revoke path grants.
    cleanup_instance_state(&mut hs);
    capability::revoke_path_grants(&hs.cap_id);
    capability::release_module_cap(&hs.cap_id);
    crate::process::set_memory(pid, inst.memory_size() as u32);
    done(pid, terminal_idx, slot);
}

pub fn init() {
    let mut config = Config::default();
    config.consume_fuel(true);
    let engine = Engine::new(&config);
    *ENGINE.lock() = Some(engine);
    crate::kdebug!("[npk] WASM runtime: wasmi v1.0 (fuel-metered)");
}

/// Execute a WASM module with an explicit fuel budget. Use for trusted
/// first-party modules whose work is deterministically bounded by input
/// parameters (e.g. wallpaper generation sized by resolution).
pub fn execute_sandboxed_with_fuel(
    wasm_bytes: &[u8], func_name: &str, args: &[Val], cap_id: CapId, fuel: u64,
) -> Result<WasmResult, WasmError> {
    execute_inner(wasm_bytes, func_name, args, cap_id, fuel, None)
}

/// Like [`execute_sandboxed_with_fuel`], but with a launch argument: the
/// same string `npk_open`/`npk_launch` pass and the module reads with
/// `npk_launch_arg`, here supplied from the shell (`beak https://…`).
pub fn execute_sandboxed_with_arg(
    wasm_bytes: &[u8], func_name: &str, args: &[Val], cap_id: CapId, fuel: u64,
    launch_arg: Option<String>,
) -> Result<WasmResult, WasmError> {
    execute_inner(wasm_bytes, func_name, args, cap_id, fuel, launch_arg)
}

/// Execute a WASM module in interactive mode (live display).
/// npk_print writes directly to terminal. Used for long-running apps (top).
#[allow(dead_code)]
fn execute_inner(
    wasm_bytes: &[u8], func_name: &str, args: &[Val], cap_id: CapId, fuel: u64,
    launch_arg: Option<String>,
) -> Result<WasmResult, WasmError> {
    if forge_is_default() {
        if let Some(r) = execute_inner_forge(wasm_bytes, func_name, args, cap_id, fuel,
                                             launch_arg.clone()) {
            return r;
        }
    }
    // Clone the engine (cheap Arc bump) and drop the ENGINE lock so two
    // one-shot decodes (run/wallpaper) can run concurrently.
    let engine = {
        let guard = ENGINE.lock();
        guard.as_ref().ok_or(WasmError::NotInitialized)?.clone()
    };

    let module = Module::new(&engine, wasm_bytes)
        .map_err(|_| WasmError::InvalidModule)?;

    let mut store = Store::new(&engine, HostState {
        limits: guest_limits(),
        output: String::new(),
        cap_id,
        direct_output: false,
        terminal_idx: 255,
        core_id: 0,
        pid: 0,
        hw: None,
        net_reach: crate::intent::http::Reach::Public,
        widget_window_id: 0,
        module_name: String::new(),
        launch_arg,
        http_final_url: None,
        http_content_type: None,
        http_last_error: None,
        http_reply_headers: None,
        http_status: 0,
        wasi: None,
        tcp_handles: Vec::new(),
    });
    store.limiter(|s| &mut s.limits);
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

/// Run a `wasm32-wasip1` module: instantiate, call `_start`, return its
/// exit status.
///
/// Separate from `execute_inner` because a wasi module is a different
/// animal: it has no `npk_*` entry point to name, it ends by trapping
/// out of `proc_exit` rather than returning, and a non-zero exit is a
/// result to report — not a kernel-side failure.
pub fn execute_wasi(
    wasm_bytes: &[u8],
    cap_id: CapId,
    fuel: u64,
    ctx: alloc::boxed::Box<crate::wasi::WasiCtx>,
    terminal_idx: u8,
) -> Result<i32, WasmError> {
    let engine = {
        let guard = ENGINE.lock();
        guard.as_ref().ok_or(WasmError::NotInitialized)?.clone()
    };
    // Time preparation and run separately, as the forge path does, so the
    // two engines report comparable figures.
    let t_c = crate::interrupts::ticks();
    let module = Module::new(&engine, wasm_bytes)
        .map_err(|_| WasmError::InvalidModule)?;
    kprintln!("[npk]   wasmi: vorbereiten {} ms",
        crate::interrupts::ticks().saturating_sub(t_c) * 10);

    let mut store = Store::new(&engine, HostState {
        limits: guest_limits(),
        output: String::new(),
        cap_id,
        direct_output: true,
        terminal_idx,
        core_id: 0,
        pid: 0,
        hw: None,
        net_reach: crate::intent::http::Reach::Public,
        widget_window_id: 0,
        module_name: String::new(),
        launch_arg: None,
        http_final_url: None,
        http_content_type: None,
        http_last_error: None,
        http_reply_headers: None,
        http_status: 0,
        wasi: Some(ctx),
        tcp_handles: Vec::new(),
    });
    store.limiter(|s| &mut s.limits);
    store.set_fuel(fuel).map_err(|_| WasmError::ExecutionFailed)?;

    let mut linker = <Linker<HostState>>::new(&engine);
    register_host_functions(&mut linker)?;

    let instance = linker.instantiate_and_start(&mut store, &module)
        .map_err(|_| WasmError::InstantiationFailed)?;

    let start = instance.get_typed_func::<(), ()>(&store, "_start")
        .map_err(|_| WasmError::FunctionNotFound)?;

    let t_r = crate::interrupts::ticks();
    let r = start.call(&mut store, ());
    kprintln!("[npk]   wasmi: laufen {} ms",
        crate::interrupts::ticks().saturating_sub(t_r) * 10);
    match r {
        Ok(()) => Ok(0),
        // `proc_exit` leaves through a trap carrying the status. That is
        // the normal way a wasi program finishes, including a clean one.
        Err(e) => match e.kind().as_i32_exit_status() {
            Some(code) => Ok(code),
            None => Err(map_exec_error(e)),
        },
    }
}

/// One-shot run under forge (wallpaper, `run <mod> <func> <args>`).
///
/// The forge entry takes three `u32`; anything else falls back to the
/// interpreter, with a log line. Returns `None` for that fallback.
fn execute_inner_forge(
    wasm_bytes: &[u8], func_name: &str, args: &[Val], cap_id: CapId, fuel: u64,
    launch_arg: Option<String>,
) -> Option<Result<WasmResult, WasmError>> {
    if args.len() > 3 || args.iter().any(|v| v.i32().is_none()) {
        kprintln!("[npk] forge: {} nimmt Argumente, die der Eintritt nicht kann — wasmi", func_name);
        return None;
    }
    let m = match forge_core::compile(wasm_bytes) {
        Ok(m) => m,
        Err(_) => return Some(Err(WasmError::InvalidModule)),
    };
    let entry = m.plan.exports.iter().find(|(n, _)| n == func_name).map(|(_, i)| *i)
        .and_then(|i| m.offset_of(i));
    let Some(off) = entry else {
        return Some(Err(WasmError::FunctionNotFound));
    };

    let mut hs = HostState {
        limits: guest_limits(),
        output: String::new(),
        cap_id,
        direct_output: false,
        terminal_idx: TERM_IDX_ACTIVE,
        core_id: 0,
        pid: 0,
        hw: None,
        net_reach: crate::intent::http::Reach::Public,
        widget_window_id: 0,
        module_name: String::new(),
        launch_arg,
        http_final_url: None,
        http_content_type: None,
        http_last_error: None,
        http_reply_headers: None,
        http_status: 0,
        wasi: None,
        tcp_handles: Vec::new(),
    };
    let host = forge_glue::NpkHost(&raw mut hs);
    let Some(mut inst) = crate::forge_rt::Instance::new_with_host(&m, &host) else {
        return Some(Err(WasmError::InstantiationFailed));
    };
    if inst.unresolved_imports() > 0 {
        return Some(Err(WasmError::InstantiationFailed));
    }
    inst.set_fuel(fuel.min(i64::MAX as u64) as i64);

    let a = |i: usize| args.get(i).and_then(|v| v.i32()).unwrap_or(0) as u32;
    let (_ret, trap) = inst.call(off, a(0), a(1), a(2));

    cleanup_instance_state(&mut hs);
    capability::revoke_path_grants(&hs.cap_id);
    capability::release_module_cap(&hs.cap_id);
    Some(match trap {
        forge_core::trap::NONE => Ok(WasmResult { output: hs.output }),
        forge_core::trap::OUT_OF_FUEL => Err(WasmError::FuelExhausted),
        other => {
            kprintln!("[npk] forge: {} endete mit {}", func_name, forge_core::trap::name(other));
            Err(WasmError::ExecutionFailed)
        }
    })
}

/// Like `execute_wasi`, but under forge, kept separate from the interpreter
/// path.
///
/// The one difference is how the program exits: under wasmi `proc_exit`
/// returns as an `Err` carrying the status; under forge it unwinds through
/// `host_trap` after storing the status in the wasi state, which is read
/// here.
pub fn execute_wasi_forge(
    wasm_bytes: &[u8],
    cap_id: CapId,
    fuel: u64,
    ctx: alloc::boxed::Box<crate::wasi::WasiCtx>,
    terminal_idx: u8,
) -> Result<i32, WasmError> {
    // Report compile and run time separately: compiling the whole module is
    // paid on every start while no cached code blob exists.
    let t_c = crate::interrupts::ticks();
    let m = forge_core::compile(wasm_bytes).map_err(|_| WasmError::InvalidModule)?;
    let compile_ms = crate::interrupts::ticks().saturating_sub(t_c) * 10;
    let entry = m.plan.exports.iter().find(|(n, _)| n == "_start").map(|(_, i)| *i)
        .and_then(|i| m.offset_of(i))
        .ok_or(WasmError::FunctionNotFound)?;

    // Here we own the state (under wasmi the Store holds it). It must not
    // move while the instance holds its pointer in the vmctx.
    let mut hs = HostState {
        limits: guest_limits(),
        output: String::new(),
        cap_id,
        direct_output: true,
        terminal_idx,
        core_id: 0,
        pid: 0,
        hw: None,
        net_reach: crate::intent::http::Reach::Public,
        widget_window_id: 0,
        module_name: String::new(),
        launch_arg: None,
        http_final_url: None,
        http_content_type: None,
        http_last_error: None,
        http_reply_headers: None,
        http_status: 0,
        wasi: Some(ctx),
        tcp_handles: Vec::new(),
    };
    let host = forge_glue::NpkHost(&raw mut hs);
    let mut inst = crate::forge_rt::Instance::new_with_host(&m, &host)
        .ok_or(WasmError::InstantiationFailed)?;
    let open = inst.unresolved_imports();
    if open > 0 {
        kprintln!("[npk] forge: {} Importe unaufgeloest — das Modul wird stehenbleiben", open);
        return Err(WasmError::InstantiationFailed);
    }
    inst.set_fuel(fuel.min(i64::MAX as u64) as i64);

    kprintln!("[npk]   forge: uebersetzen {} ms ({} B x86)", compile_ms, m.code.len());
    let t_r = crate::interrupts::ticks();
    let (_ret, trap) = inst.call(entry, 0, 0, 0);
    kprintln!("[npk]   forge: laufen {} ms",
        crate::interrupts::ticks().saturating_sub(t_r) * 10);
    match trap {
        // Returning from `_start` is rare for a wasi program but allowed,
        // and means status 0.
        forge_core::trap::NONE => Ok(crate::wasi::exit_status(&hs).unwrap_or(0)),
        // The normal exit: `proc_exit` has stored the status.
        forge_core::trap::EXIT => Ok(crate::wasi::exit_status(&hs).unwrap_or(0)),
        forge_core::trap::OUT_OF_FUEL => Err(WasmError::FuelExhausted),
        other => {
            kprintln!("[npk] forge: python endete mit {}", forge_core::trap::name(other));
            Err(WasmError::ExecutionFailed)
        }
    }
}

/// Route a string to wherever this run's output belongs: straight to a
/// specific terminal on a worker core, to the active terminal via
/// `kprint`, or into the buffer a one-shot `run` prints at the end.
pub(crate) fn emit_output(state: &mut HostState, s: &str) {
    if state.direct_output {
        let idx = state.terminal_idx;
        if idx != TERM_IDX_ACTIVE && (idx as usize) < MAX_APP_BUFS {
            crate::shade::terminal::write_idx(idx as usize, s);
        } else {
            kprint!("{}", s);
        }
    } else {
        state.output.push_str(s);
    }
}

fn register_host_functions(linker: &mut Linker<HostState>) -> Result<(), WasmError> {
    // The second ABI. Inert without a grant in HostState.wasi.
    crate::wasi::link(linker).map_err(|_| WasmError::HostFunctionError)?;
    forge_glue::register_wasmi(linker).map_err(|_| WasmError::HostFunctionError)
}

fn cleanup_instance_state(state: &mut HostState) {
    // A dead driver's snapshot must not read as live numbers.
    crate::drivers::report::clear(&state.module_name);
    // Otherwise an app closed mid-load holds its fetch slots and reserved
    // buffers until the next boot.
    crate::intent::fetch::release_owner(state.pid);
    for h in state.tcp_handles.drain(..) {
        let _ = crate::net::tcp::close(h);
    }
    crate::wasm::host_core::tls_release_owner(state.pid);
    crate::audio::release_owner(state.pid);
    if let Some(hw) = state.hw.take() {
        release_hw(hw);
    }
}

/// Let go of a driver's device: quiet it, give back its interrupt vector,
/// its DMA buffers and its network registration. On module exit, and when a
/// driver binds another device in place of this one.
pub(crate) fn release_hw(hw: HwDriverState) {
    {
        // Stop the device before its buffers go back: with bus mastering on
        // it keeps writing received data into whatever the frames become
        // next. Clearing the bit also stops its MSI writes. Linux does the
        // same, `pci_clear_master` before freeing coherent memory.
        if hw.is_pci {
            let cmd = crate::drivers::pci::read32(hw.pci_addr, 0x04);
            crate::drivers::pci::write32(hw.pci_addr, 0x04, cmd & !0x4);
        }
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
        if hw.irq_vector != 0 {
            crate::irq::release(hw.irq_vector);
        }
        if !hw.dma_allocs.is_empty() || hw.registered_as_netdev {
            kprintln!("[npk] driver cleanup: freed {} DMA buffers ({} pages)",
                hw.dma_allocs.len(), total_pages);
        }
    }
}

/// Root of the per-module private areas.
///
/// `priv/<module>/…` is the one place in storage that no capability opens:
/// elsewhere READ reads and WRITE writes, here the name decides, and the
/// kernel assigns the name. Not even a module with all rights can enter
/// another module's private area. This is where state that is itself a
/// credential (e.g. browser cookies) can persist without being readable by
/// every app holding READ.
///
/// The rule is symmetric (read, write, list, delete, rename, copy); a
/// boundary in one direction only is none. The file manager does not see
/// these folders, by design.
pub(crate) const PRIVATE_ROOT: &str = "priv";

/// The module that owns this path, or `None` if nobody owns it (then
/// capabilities decide as usual).
pub(crate) fn private_area_owner(name: &str) -> Option<&str> {
    // Match on segments, not prefixes: `privat/x` is not `priv/x`, but
    // `priv//beak/x` is. npkFS does not normalise (`clean_path` only trims
    // the ends), so the guard does.
    let mut segs = name.split('/').filter(|s| !s.is_empty() && *s != ".");
    if segs.next() != Some(PRIVATE_ROOT) { return None }
    // A `..` under `priv/` makes the owner the empty string, which is no
    // module name, so nobody gets in, not even the rightful owner. Fails safe.
    if name.split('/').any(|s| s == "..") { return Some("") }
    match segs.next() {
        Some(owner) => Some(owner),
        // `priv` itself belongs to nobody and may be listed; `npk_fs_list`
        // filters its entries one by one.
        None => None,
    }
}

/// May `module` touch this path? Always true outside `priv/`, where
/// capabilities decide.
pub(crate) fn private_area_allows(name: &str, module: &str) -> bool {
    match private_area_owner(name) {
        Some(owner) => owner == module,
        None => true,
    }
}

/// Is this the module's own private area? Then no capability is needed:
/// persisting one's own state is a different ability from reading the
/// machine's storage, so a module without READ/WRITE can still keep it.
///
/// The name is the authorisation, and the kernel assigns it: a module cannot
/// leave `priv/<itself>` or enter another's, whatever its `.npk.caps` says.
pub(crate) fn is_own_private(name: &str, module: &str) -> bool {
    !module.is_empty() && private_area_owner(name) == Some(module)
}

/// True if `name` targets the module store (`sys/wasm/…`) or the trust
/// store (`sys/certs/…`) — the two directories where a write is a
/// privilege escalation rather than a file operation.
///
/// Modules are not re-verified at launch, so an app that could overwrite a
/// module (or plant one with caps=ALL) would escalate to arbitrary rights.
/// A file under `sys/certs/` becomes a root CA the whole system honours.
/// The install/update intents reach npkFS directly, not through these host
/// functions. The paths layer rejects `.`/`..` segments, so after trimming
/// slashes a literal prefix is the only way to land in either store.
fn is_trust_critical_path(name: &str) -> bool {
    let c = name.trim_matches('/');
    if c == "sys/wasm" || c.starts_with("sys/wasm/") {
        return true;
    }
    // Prefix match on a path segment — a plain `starts_with` would also
    // catch a sibling like `sys/certsomething`, and missing the trailing
    // separator is how this class of guard usually leaks.
    let certs = crate::tls::certstore::STORE_DIR;
    c == certs || (c.starts_with(certs) && c.as_bytes().get(certs.len()) == Some(&b'/'))
}

/// Extract a WASM custom section's payload by name, walking the module header.
/// The whole (possibly multi-MB) module is available here, so a section at
/// the tail (like `.npk.app_meta`) is found regardless of module size.
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
fn app_is_focused(state: &HostState) -> bool {
    let wid = state.widget_window_id;
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

