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

pub(crate) mod host_core;
pub(crate) mod forge_glue;

// ── Welcher Motor faehrt ein Modul ────────────────────────────────────
//
// Die Wahl gehoert NICHT an jeden Startweg. Autostart, Treiber, Dienste und
// Einmallaeufe kommen alle irgendwo anders herein — und `dock`, `bar`,
// `audio_hda`, `wifid` startet ueberhaupt niemand von Hand. Ein Modul von
// Hand zu starten pruefte ausserdem einen ANDEREN Pfad als den, auf dem es im
// Betrieb hochkommt: andere Capabilities, andere Fensterbehandlung.
//
// Deshalb eine Fahne, die einen Neustart uebersteht. Umlegen, neu starten,
// und der ganze Desktop laeuft unter forge — oder eben nicht, und man sieht
// es sofort. Die Intent-Shell selbst ist nativ, also bleibt ein Prompt
// erreichbar, auch wenn ein Modul kippt.
static FORGE_DEFAULT: AtomicBool = AtomicBool::new(false);

/// Aus der Konfiguration lesen. Nach `config::load()` aufrufen.
///
/// **Ohne Eintrag gilt forge.** Am 2026-09-01 lief das ganze System einmal
/// damit durch — alle 21 Module auf ihren echten Startwegen, Autostart und
/// Treiber eingeschlossen. Ein frisch installiertes System soll den Compiler
/// bekommen, ohne dass jemand einen Schalter kennt.
///
/// `wasm.engine=wasmi` in der Konfiguration schaltet zurueck; der Weg dahin
/// ist `forge default off`, und er funktioniert auch dann noch, wenn kein
/// einziges Modul startet — die Intent-Shell ist nativ.
pub fn load_engine_default() {
    let on = crate::config::get("wasm.engine").as_deref() != Some("wasmi");
    FORGE_DEFAULT.store(on, AtOrd::Release);
    kprintln!("[npk] WASM: {}", if on { "forge" } else { "wasmi (per Konfiguration)" });
}

/// Welcher Motor faehrt, wenn der Aufrufer nichts anderes sagt.
pub fn forge_is_default() -> bool {
    FORGE_DEFAULT.load(AtOrd::Acquire)
}

/// Umlegen und merken.
pub fn set_engine_default(forge: bool) {
    FORGE_DEFAULT.store(forge, AtOrd::Release);
    crate::config::set("wasm.engine", if forge { "forge" } else { "wasmi" });
}

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

pub(crate) struct HostState {
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
    /// Die Reichweite des Dokuments, das dieses Modul gerade anzeigt.
    ///
    /// **Vorgabe `Public`, und das ist die strenge Wahl.** Ein Modul, das
    /// nie etwas sagt, gilt als oeffentliche Seite und kommt damit nicht ins
    /// private Netz. Nur wer `npk_net_context` ruft und dabei eine private
    /// Adresse nennt, bekommt mehr — und auch das erst, nachdem der Kernel
    /// die Adresse selbst AUFGELOEST hat.
    pub(crate) net_reach: crate::intent::http::Reach,
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
    /// A `wasi_snapshot_preview1` grant, or None.
    ///
    /// The namespace is linked for every module, but every function in
    /// it bounces on `ENOTCAPABLE` unless this is `Some`. So "can this
    /// program see a filesystem" is decided once, here, by whoever
    /// spawned it — not by what the program chooses to import.
    pub(crate) wasi: Option<alloc::boxed::Box<crate::wasi::WasiCtx>>,
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
    /// Run this one under forge instead of the interpreter. Per job, not
    /// global: ein Fehler in der Bruecke legt so nicht jede App um.
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

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering as AtOrd};

const APP_KEY_BUF_SIZE: usize = 32;
const MAX_APP_BUFS: usize = 256;
/// `terminal_idx` sentinel for "whatever terminal is focused". Spawn paths that
/// have no window of their own pass it (drivers from autostart, sandboxed
/// runs). It MUST be excluded before treating the index as a slot: 255 is
/// smaller than MAX_APP_BUFS, so it used to pass the bounds check, land in
/// `write_idx(255)` — a slot that is never allocated — and be dropped without
/// a trace. Every line an autostarted driver printed went there. Four kernel
/// lines and nothing from the driver is what that looks like from the outside,
/// and it cost an evening.
const TERM_IDX_ACTIVE: u8 = 255;

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
/// Wie [`spawn_on_worker`], aber mit einem STARTARGUMENT — der Weg, den der
/// Terminal-Start einer Fensteranwendung nimmt.
///
/// Warum ueberhaupt: der blockierende Ausfuehrungsweg (`execute_inner`) setzt
/// `pid: 0`, und ohne Prozessnummer lehnt `fetch::begin_one` jeden
/// asynchronen Abruf ab ("async fetch needs a process"). Vom Prompt aus
/// konnte beak damit zwar aufgehen, aber nie eine Seite laden — vom Dock
/// aus ging es, weil der Klickweg schon immer hier vorbeikam.
pub fn spawn_on_worker_with_arg(
    wasm_bytes: Vec<u8>, cap_id: CapId, terminal_idx: u8, module_name: &str,
    launch_arg: Option<String>,
) -> bool {
    spawn_on_worker_inner(wasm_bytes, cap_id, terminal_idx, module_name, true, 0, launch_arg)
}

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

/// Wie `spawn_on_worker`, aber unter forge. Derselbe Weg, dieselbe Jobqueue,
/// dasselbe Fenster — nur der Motor ist ein anderer.
pub fn spawn_on_worker_forge(wasm_bytes: Vec<u8>, cap_id: CapId, terminal_idx: u8, module_name: &str) -> bool {
    spawn_on_worker_inner_engine(wasm_bytes, cap_id, terminal_idx, module_name, true, 0, None, true)
}

fn spawn_on_worker_inner(
    wasm_bytes: Vec<u8>, cap_id: CapId, terminal_idx: u8, module_name: &str,
    foreground: bool, widget_wid: u32, launch_arg: Option<String>,
) -> bool {
    // Kein verdrahtetes `false` mehr: hier kommen Autostart, Treiber, Dienste
    // und Widget-Apps alle durch, und sie sollen denselben Motor fahren wie
    // alles andere.
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
    cleanup_instance_state(store.data_mut());

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

/// Derselbe Job, unter forge. Bewusst NEBEN `wasm_worker_task` und nicht
/// hinein: der Interpreterpfad bleibt Zeile fuer Zeile, wie er war, solange
/// dieser hier nicht gemessen ist. Der Preis ist ein doppelter Nachlauf von
/// zehn Zeilen — billiger als ein Umbau an dem Weg, an dem jede App haengt.
fn forge_worker_task(slot: usize, job: WasmJob) {
    let terminal_idx = job.terminal_idx;
    let core_id = crate::smp::per_core::current_core_id();
    let name_str = core::str::from_utf8(&job.name[..job.name_len as usize]).unwrap_or("?");
    let pid = crate::process::spawn(name_str, crate::process::KIND_WASM, terminal_idx, core_id as u8);

    // Ab hier muss jeder Ausgang aufraeumen, sonst bleibt ein Prozess stehen
    // und das Terminal nimmt keine Tasten mehr an.
    let done = |pid: u32, terminal_idx: u8, slot: usize| {
        crate::process::exit(pid);
        if (terminal_idx as usize) < MAX_APP_BUFS {
            APP_RUNNING[terminal_idx as usize].store(false, AtOrd::Release);
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

    // Der Zustand gehoert hier UNS — unter wasmi haelt ihn der Store. Er darf
    // sich nicht bewegen, solange die Instanz seinen Zeiger im vmctx hat.
    let mut hs = HostState {
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
    };

    let host = forge_glue::NpkHost(&raw mut hs);
    let Some(mut inst) = crate::forge_rt::Instance::new_with_host(&m, &host) else {
        kprintln!("[npk] forge: {} — Instanz liess sich nicht bauen", name_str);
        done(pid, terminal_idx, slot);
        return;
    };
    // Ein Import auf dem Trap-Stumpf wuerde beim ersten Aufruf stehenbleiben.
    // Das jetzt sagen ist besser als es spaeter als Absturz zu lesen.
    let open = inst.unresolved_imports();
    if open > 0 {
        kprintln!("[npk] forge: {} — {} Importe unaufgeloest, das Modul wird stehenbleiben",
            name_str, open);
    }
    kprintln!("[npk] forge: {} uebersetzt in {} ms ({} B x86)", name_str, compile_ms, m.code.len());

    inst.set_fuel(INTERACTIVE_FUEL as i64);
    crate::process::set_memory(pid, inst.memory_size() as u32);

    let (_ret, trap) = inst.call(off, 0, 0, 0);
    if trap != forge_core::trap::NONE {
        kprintln!("[npk] forge: {} endete mit {}", name_str, forge_core::trap::name(trap));
    }

    // Wie im Interpreterpfad: Hardware zurueck, Pfadrechte weg. Beide
    // arbeiten schon auf `&mut HostState`, also gilt hier dasselbe.
    cleanup_instance_state(&mut hs);
    capability::revoke_path_grants(&hs.cap_id);
    crate::process::set_memory(pid, inst.memory_size() as u32);
    done(pid, terminal_idx, slot);
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
    execute_inner(wasm_bytes, func_name, args, cap_id, fuel, None)
}

/// Wie [`execute_sandboxed_with_fuel`], aber mit einem STARTARGUMENT.
///
/// Das ist dieselbe Zeichenkette, die `npk_open`/`npk_launch` einem Modul
/// mitgeben und die es mit `npk_launch_arg` abholt — nur kam sie bisher
/// ausschliesslich von einer anderen App. Von der Shell aus gab es keinen
/// Weg: `beak https://…` startete beak ohne die Adresse, obwohl beak sie
/// beim Start liest und ansteuert.
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
    // Dieselbe Teilung wie im forge-Pfad, sonst sind die Spalten nicht
    // vergleichbar: auch wasmi bereitet das Modul einmal auf.
    let t_c = crate::interrupts::ticks();
    let module = Module::new(&engine, wasm_bytes)
        .map_err(|_| WasmError::InvalidModule)?;
    kprintln!("[npk]   wasmi: vorbereiten {} ms",
        crate::interrupts::ticks().saturating_sub(t_c) * 10);

    let mut store = Store::new(&engine, HostState {
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
    });
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

/// Der Einmallauf unter forge — wallpaper und `run <mod> <func> <args>`.
///
/// forges Eintritt nimmt drei `u32`; alles darueber oder mit anderen Typen
/// bleibt beim Interpreter, und zwar SICHTBAR statt still. Der haeufige Fall
/// (`"_start"` ohne Argumente, wie wallpaper ihn ruft) geht durch.
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
    Some(match trap {
        forge_core::trap::NONE => Ok(WasmResult { output: hs.output }),
        forge_core::trap::OUT_OF_FUEL => Err(WasmError::FuelExhausted),
        other => {
            kprintln!("[npk] forge: {} endete mit {}", func_name, forge_core::trap::name(other));
            Err(WasmError::ExecutionFailed)
        }
    })
}

/// Wie `execute_wasi`, aber unter forge. Bewusst daneben und nicht darin: der
/// Interpreterpfad bleibt, wie er ist, solange dieser hier nicht gemessen ist.
///
/// Der Unterschied ist genau einer — wie das Programm sich verabschiedet.
/// Unter wasmi kommt `proc_exit` als `Err` mit Status zurueck; unter forge
/// rollt es ueber `host_trap` ab und hinterlegt den Status vorher im
/// wasi-Zustand. Beide enden im selben Zustand, also liest der Rueckweg hier
/// dasselbe.
pub fn execute_wasi_forge(
    wasm_bytes: &[u8],
    cap_id: CapId,
    fuel: u64,
    ctx: alloc::boxed::Box<crate::wasi::WasiCtx>,
    terminal_idx: u8,
) -> Result<i32, WasmError> {
    // Eine Gesamtzahl verbirgt, welche Haelfte sich bewegt hat. Bei forge ist
    // die eine Haelfte das Uebersetzen des ganzen Moduls — bei python 7,44 MB,
    // und das faellt bei JEDEM Start an, solange der Codeblob nicht liegt.
    let t_c = crate::interrupts::ticks();
    let m = forge_core::compile(wasm_bytes).map_err(|_| WasmError::InvalidModule)?;
    let compile_ms = crate::interrupts::ticks().saturating_sub(t_c) * 10;
    let entry = m.plan.exports.iter().find(|(n, _)| n == "_start").map(|(_, i)| *i)
        .and_then(|i| m.offset_of(i))
        .ok_or(WasmError::FunctionNotFound)?;

    // Der Zustand gehoert hier UNS — unter wasmi haelt ihn der Store. Er darf
    // sich nicht bewegen, solange die Instanz seinen Zeiger im vmctx hat.
    let mut hs = HostState {
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
        // Sauber aus `_start` zurueck: ein wasi-Programm tut das selten, aber
        // es ist erlaubt und bedeutet Status 0.
        forge_core::trap::NONE => Ok(crate::wasi::exit_status(&hs).unwrap_or(0)),
        // Der normale Abgang: `proc_exit` hat den Status vorher hinterlegt.
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

    // npk_print(ptr, len) — write to output buffer or directly to terminal
    // Where an app's output goes, in one place — npk_print and the
    // wasi fd_write path must not drift apart.
    linker.func_wrap("env", "npk_print",
        |mut caller: Caller<'_, HostState>, ptr: i32, len: i32| {
            let Some(m) = caller.get_export("memory").and_then(|e| e.into_memory())
                else { return };
            let (mem, ctx) = m.data_and_store_mut(&mut caller);
            host_core::npk_print(mem, ctx, ptr, len)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_log(ptr, len) — write to serial console (no cap needed, output only)
    linker.func_wrap("env", "npk_log",
        |mut caller: Caller<'_, HostState>, ptr: i32, len: i32| {
            let Some(m) = caller.get_export("memory").and_then(|e| e.into_memory())
                else { return };
            let (mem, ctx) = m.data_and_store_mut(&mut caller);
            host_core::npk_log(mem, ctx, ptr, len)
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
        |mut caller: Caller<'_, HostState>, ptr: i32, len: i32| {
            let Some(m) = caller.get_export("memory").and_then(|e| e.into_memory())
                else { return };
            let (mem, ctx) = m.data_and_store_mut(&mut caller);
            host_core::npk_log_serial(mem, ctx, ptr, len)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_fetch(name_ptr, name_len, buf_ptr, buf_max) -> bytes or -1
    linker.func_wrap("env", "npk_fetch",
        |mut caller: Caller<'_, HostState>, name_ptr: i32, name_len: i32, buf_ptr: i32, buf_max: i32| -> i32 {
            let Some(m) = caller.get_export("memory").and_then(|e| e.into_memory())
                else { return -1 };
            let (mem, ctx) = m.data_and_store_mut(&mut caller);
            host_core::npk_fetch(mem, ctx, name_ptr, name_len, buf_ptr, buf_max)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_http_request(url_ptr, url_len, buf_ptr, buf_max) -> bytes or -1
    // Outbound HTTPS GET for the native browser (beak). Parses the URL,
    // fetches the body (following redirects) via the same TLS path OTA
    // uses, and copies up to buf_max bytes into the caller's buffer.
    // NET-gated — distinct from npkFS READ and from WiFi NETCTL.
    linker.func_wrap("env", "npk_http_request",
        |mut caller: Caller<'_, HostState>, url_ptr: i32, url_len: i32, buf_ptr: i32, buf_max: i32| -> i32 {
            let Some(m) = caller.get_export("memory").and_then(|e| e.into_memory())
                else { return -1 };
            let (mem, ctx) = m.data_and_store_mut(&mut caller);
            host_core::npk_http_request(mem, ctx, url_ptr, url_len, buf_ptr, buf_max)
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
        |mut caller: Caller<'_, HostState>, method_ptr: i32, method_len: i32, url_ptr: i32, url_len: i32, hdrs_ptr: i32, hdrs_len: i32, body_ptr: i32, body_len: i32, buf_ptr: i32, buf_max: i32| -> i32 {
            let Some(m) = caller.get_export("memory").and_then(|e| e.into_memory())
                else { return -1 };
            let (mem, ctx) = m.data_and_store_mut(&mut caller);
            host_core::npk_http_send(mem, ctx, method_ptr, method_len, url_ptr, url_len, hdrs_ptr, hdrs_len, body_ptr, body_len, buf_ptr, buf_max)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_http_response_headers(buf_ptr, buf_max) -> len, or -1
    // The last npk_http_send response's header block, minus the status line.
    // `Set-Cookie` repeats, so this is a raw block rather than a getter per
    // name. NET-gated.
    linker.func_wrap("env", "npk_http_response_headers",
        |mut caller: Caller<'_, HostState>, buf_ptr: i32, buf_max: i32| -> i32 {
            let Some(m) = caller.get_export("memory").and_then(|e| e.into_memory())
                else { return -1 };
            let (mem, ctx) = m.data_and_store_mut(&mut caller);
            host_core::npk_http_response_headers(mem, ctx, buf_ptr, buf_max)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_http_status() -> status, or 0
    // The last npk_http_send response's HTTP status. NET-gated.
    linker.func_wrap("env", "npk_http_status",
        |mut caller: Caller<'_, HostState>| -> i32 {
            host_core::npk_http_status(caller.data_mut())
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
        |mut caller: Caller<'_, HostState>, urls_ptr: i32, urls_len: i32, out_ptr: i32, out_max: i32, lens_ptr: i32, lens_max: i32| -> i32 {
            let Some(m) = caller.get_export("memory").and_then(|e| e.into_memory())
                else { return -1 };
            let (mem, ctx) = m.data_and_store_mut(&mut caller);
            host_core::npk_http_request_many(mem, ctx, urls_ptr, urls_len, out_ptr, out_max, lens_ptr, lens_max)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // ── Fetching without standing still ────────────────────────────────
    //
    // The two requests above, split into "start it" and "collect it". A
    // module that calls npk_http_send is INSIDE the host call until the
    // exchange ends — it cannot paint, cannot read a key, and its peer
    // fibers do not run. These five let it keep its loop: begin -> handle,
    // poll between frames, take when the answer is there. The wait itself
    // happens on a worker fiber on another core (intent::fetch).
    //
    // NET-gated, same capability as the synchronous pair.

    // npk_net_context(url_ptr, url_len) -> 0, or -1
    //
    // Der Browser sagt, WELCHES Dokument er gerade anzeigt. Der Kernel loest
    // die Adresse SELBST auf und merkt sich nur die Klasse (oeffentlich /
    // privat / lokal) — nie den Namen, denn ein Name kann beim zweiten
    // Aufloesen woandershin zeigen.
    //
    // Ohne diesen Aufruf gilt das Modul als oeffentliche Seite. Das ist die
    // strenge Vorgabe und deshalb sicher zu vergessen.
    linker.func_wrap("env", "npk_net_context",
        |mut caller: Caller<'_, HostState>, url_ptr: i32, url_len: i32| -> i32 {
            let Some(m) = caller.get_export("memory").and_then(|e| e.into_memory())
                else { return -1 };
            let (mem, ctx) = m.data_and_store_mut(&mut caller);
            host_core::npk_net_context(mem, ctx, url_ptr, url_len)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_http_begin(method_ptr, method_len, url_ptr, url_len,
    //                hdrs_ptr, hdrs_len, body_ptr, body_len, buf_max)
    //   -> handle >= 1, or -1 (reason via npk_http_last_error)
    linker.func_wrap("env", "npk_http_begin",
        |mut caller: Caller<'_, HostState>, method_ptr: i32, method_len: i32, url_ptr: i32, url_len: i32, hdrs_ptr: i32, hdrs_len: i32, body_ptr: i32, body_len: i32, buf_max: i32| -> i32 {
            let Some(m) = caller.get_export("memory").and_then(|e| e.into_memory())
                else { return -1 };
            let (mem, ctx) = m.data_and_store_mut(&mut caller);
            host_core::npk_http_begin(mem, ctx, method_ptr, method_len, url_ptr, url_len, hdrs_ptr, hdrs_len, body_ptr, body_len, buf_max)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_http_begin_many(urls_ptr, urls_len, out_max) -> handle >= 1, or -1
    linker.func_wrap("env", "npk_http_begin_many",
        |mut caller: Caller<'_, HostState>, urls_ptr: i32, urls_len: i32, out_max: i32| -> i32 {
            let Some(m) = caller.get_export("memory").and_then(|e| e.into_memory())
                else { return -1 };
            let (mem, ctx) = m.data_and_store_mut(&mut caller);
            host_core::npk_http_begin_many(mem, ctx, urls_ptr, urls_len, out_max)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_http_poll(handle) -> 1 answer waiting, 0 running, -1 failed,
    //                          -2 no such handle
    linker.func_wrap("env", "npk_http_poll",
        |mut caller: Caller<'_, HostState>, handle: i32| -> i32 {
            host_core::npk_http_poll(caller.data_mut(), handle)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_http_take(handle, buf_ptr, buf_max) -> bytes, -1 failed,
    //                                            -2 unknown, -3 still running
    // Frees the job and fills the same five getters npk_http_send fills.
    linker.func_wrap("env", "npk_http_take",
        |mut caller: Caller<'_, HostState>, handle: i32, buf_ptr: i32, buf_max: i32| -> i32 {
            let Some(m) = caller.get_export("memory").and_then(|e| e.into_memory())
                else { return -1 };
            let (mem, ctx) = m.data_and_store_mut(&mut caller);
            host_core::npk_http_take(mem, ctx, handle, buf_ptr, buf_max)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_http_take_many(handle, out_ptr, out_max, lens_ptr, lens_max)
    //   -> count, -1 failed, -2 unknown, -3 still running
    linker.func_wrap("env", "npk_http_take_many",
        |mut caller: Caller<'_, HostState>, handle: i32, out_ptr: i32, out_max: i32, lens_ptr: i32, lens_max: i32| -> i32 {
            let Some(m) = caller.get_export("memory").and_then(|e| e.into_memory())
                else { return -1 };
            let (mem, ctx) = m.data_and_store_mut(&mut caller);
            host_core::npk_http_take_many(mem, ctx, handle, out_ptr, out_max, lens_ptr, lens_max)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_http_cancel(handle) -> 0. Idempotent.
    linker.func_wrap("env", "npk_http_cancel",
        |mut caller: Caller<'_, HostState>, handle: i32| -> i32 {
            host_core::npk_http_cancel(caller.data_mut(), handle)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_http_final_url(buf_ptr, buf_max) -> len, or -1
    // The URL the last npk_http_request's body actually came from, after
    // redirects. A browser resolves relative sub-resources against this
    // (the document base URL) — resolving against the *requested* URL
    // instead makes every sub-resource repeat the redirect. NET-gated.
    linker.func_wrap("env", "npk_http_final_url",
        |mut caller: Caller<'_, HostState>, buf_ptr: i32, buf_max: i32| -> i32 {
            let Some(m) = caller.get_export("memory").and_then(|e| e.into_memory())
                else { return -1 };
            let (mem, ctx) = m.data_and_store_mut(&mut caller);
            host_core::npk_http_final_url(mem, ctx, buf_ptr, buf_max)
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
            let Some(m) = caller.get_export("memory").and_then(|e| e.into_memory())
                else { return -1 };
            let (mem, ctx) = m.data_and_store_mut(&mut caller);
            host_core::npk_http_content_type(mem, ctx, buf_ptr, buf_max)
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
            let Some(m) = caller.get_export("memory").and_then(|e| e.into_memory())
                else { return -1 };
            let (mem, ctx) = m.data_and_store_mut(&mut caller);
            host_core::npk_http_last_error(mem, ctx, buf_ptr, buf_max)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_store(name_ptr, name_len, data_ptr, data_len) -> 0 or -1
    linker.func_wrap("env", "npk_store",
        |mut caller: Caller<'_, HostState>, name_ptr: i32, name_len: i32, data_ptr: i32, data_len: i32| -> i32 {
            let Some(m) = caller.get_export("memory").and_then(|e| e.into_memory())
                else { return -1 };
            let (mem, ctx) = m.data_and_store_mut(&mut caller);
            host_core::npk_store(mem, ctx, name_ptr, name_len, data_ptr, data_len)
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
            let Some(m) = caller.get_export("memory").and_then(|e| e.into_memory())
                else { return -1 };
            let (mem, ctx) = m.data_and_store_mut(&mut caller);
            host_core::npk_home_dir(mem, ctx, buf_ptr, buf_max)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_fs_usage() -> i64
    // Filesystem fill level as (used_mib << 32) | total_mib, or -1 when
    // nothing is mounted. Feeds the file browser's capacity meter.
    // READ-gated — it says how much of the disk is in use.
    linker.func_wrap("env", "npk_fs_usage",
        |mut caller: Caller<'_, HostState>| -> i64 {
            host_core::npk_fs_usage(caller.data_mut())
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
            let Some(m) = caller.get_export("memory").and_then(|e| e.into_memory())
                else { return -1 };
            let (mem, ctx) = m.data_and_store_mut(&mut caller);
            host_core::npk_locale(mem, ctx, buf_ptr, buf_max)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_launch_arg(buf_ptr, buf_max) -> i32
    // Read the launch argument the app was started with (e.g. a file
    // path passed by npk_open). Returns bytes written, 0 if none, -1 on
    // error. Apps call this once at startup.
    linker.func_wrap("env", "npk_launch_arg",
        |mut caller: Caller<'_, HostState>, buf_ptr: i32, buf_max: i32| -> i32 {
            let Some(m) = caller.get_export("memory").and_then(|e| e.into_memory())
                else { return -1 };
            let (mem, ctx) = m.data_and_store_mut(&mut caller);
            host_core::npk_launch_arg(mem, ctx, buf_ptr, buf_max)
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
        |mut caller: Caller<'_, HostState>, ptr: i32, len: i32| -> i32 {
            let Some(m) = caller.get_export("memory").and_then(|e| e.into_memory())
                else { return -1 };
            let (mem, ctx) = m.data_and_store_mut(&mut caller);
            host_core::npk_clipboard_set(mem, ctx, ptr, len)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_clipboard_len() -> i32
    // Byte length of the current clipboard text (0 if empty). Lets an app
    // size its buffer before npk_clipboard_get. Focus-gated like the rest.
    linker.func_wrap("env", "npk_clipboard_len",
        |mut caller: Caller<'_, HostState>| -> i32 {
            host_core::npk_clipboard_len(caller.data_mut())
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_clipboard_get(ptr, max) -> i32
    // Write up to `max` clipboard bytes into the guest buffer. Returns the
    // FULL text length (so the app can detect truncation and re-query with
    // a bigger buffer), 0 if empty, or -1 (denied / not focused / bad ptr).
    linker.func_wrap("env", "npk_clipboard_get",
        |mut caller: Caller<'_, HostState>, ptr: i32, max: i32| -> i32 {
            let Some(m) = caller.get_export("memory").and_then(|e| e.into_memory())
                else { return -1 };
            let (mem, ctx) = m.data_and_store_mut(&mut caller);
            host_core::npk_clipboard_get(mem, ctx, ptr, max)
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
        |mut caller: Caller<'_, HostState>, app_ptr: i32, app_len: i32, arg_ptr: i32, arg_len: i32| -> i32 {
            let Some(m) = caller.get_export("memory").and_then(|e| e.into_memory())
                else { return -1 };
            let (mem, ctx) = m.data_and_store_mut(&mut caller);
            host_core::npk_open(mem, ctx, app_ptr, app_len, arg_ptr, arg_len)
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
        |mut caller: Caller<'_, HostState>, app_ptr: i32, app_len: i32, arg_ptr: i32, arg_len: i32| -> i32 {
            let Some(m) = caller.get_export("memory").and_then(|e| e.into_memory())
                else { return -1 };
            let (mem, ctx) = m.data_and_store_mut(&mut caller);
            host_core::npk_launch(mem, ctx, app_ptr, app_len, arg_ptr, arg_len)
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
        |mut caller: Caller<'_, HostState>, mode: i32, start_ptr: i32, start_len: i32, suggest_ptr: i32, suggest_len: i32, tag: i32| -> i32 {
            let Some(m) = caller.get_export("memory").and_then(|e| e.into_memory())
                else { return -1 };
            let (mem, ctx) = m.data_and_store_mut(&mut caller);
            host_core::npk_pick(mem, ctx, mode, start_ptr, start_len, suggest_ptr, suggest_len, tag)
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
        |mut caller: Caller<'_, HostState>, path_ptr: i32, path_len: i32| -> i32 {
            let Some(m) = caller.get_export("memory").and_then(|e| e.into_memory())
                else { return -1 };
            let (mem, ctx) = m.data_and_store_mut(&mut caller);
            host_core::npk_pick_result(mem, ctx, path_ptr, path_len)
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
        |mut caller: Caller<'_, HostState>, on: i32| -> i32 {
            host_core::npk_window_set_close_guard(caller.data_mut(), on)
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
        |mut caller: Caller<'_, HostState>, path_ptr: i32, path_len: i32| -> i32 {
            let Some(m) = caller.get_export("memory").and_then(|e| e.into_memory())
                else { return -1 };
            let (mem, ctx) = m.data_and_store_mut(&mut caller);
            host_core::npk_pick_mkdir(mem, ctx, path_ptr, path_len)
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
            let Some(m) = caller.get_export("memory").and_then(|e| e.into_memory())
                else { return -1 };
            let (mem, ctx) = m.data_and_store_mut(&mut caller);
            host_core::npk_scene_commit(mem, ctx, ptr, len)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_canvas_commit(canvas_id, ptr, len, width, height) -> 0 / -1
    // P10.10 escape hatch: upload a raw BGRA32 bitmap into the app's
    // `Widget::Canvas` with the matching id. CANVAS-gated. The app must
    // already own a widget window (commit a scene first) — the bitmap is
    // keyed by (window_id, canvas_id); the render walker blits it
    // contain-fit into the canvas rect on the next rasterise.
    linker.func_wrap("env", "npk_canvas_commit",
        |mut caller: Caller<'_, HostState>, canvas_id: i32, ptr: i32, len: i32, width: i32, height: i32| -> i32 {
            let Some(m) = caller.get_export("memory").and_then(|e| e.into_memory())
                else { return -1 };
            let (mem, ctx) = m.data_and_store_mut(&mut caller);
            host_core::npk_canvas_commit(mem, ctx, canvas_id, ptr, len, width, height)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_screen_size() -> (width << 16) | height, or 0 on error.
    // Allowed for RENDER (overlay sizing) OR CAPTURE (screenshot tool
    // sizing its capture buffer — it has no RENDER in full-screen mode).
    // (Screens are well under 65535 px/side.)
    linker.func_wrap("env", "npk_screen_size",
        |mut caller: Caller<'_, HostState>| -> i32 {
            host_core::npk_screen_size(caller.data_mut())
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
        |mut caller: Caller<'_, HostState>| -> i64 {
            host_core::npk_ticks(caller.data_mut())
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
        |mut caller: Caller<'_, HostState>| -> i64 {
            host_core::npk_now_us(caller.data_mut())
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_unix_time() -> seconds since the epoch, UTC, or 0 if the clock is
    // not readable. Ungated, like npk_ticks: the wall clock is not a secret —
    // it is on the bar and stamped into every npkFS entry. `npk_ticks` cannot
    // stand in for it, because it restarts at every boot and a cookie's
    // `Expires` is an absolute date.
    linker.func_wrap("env", "npk_unix_time",
        |mut caller: Caller<'_, HostState>| -> i64 {
            host_core::npk_unix_time(caller.data_mut())
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_theme_token(token_id) -> RGBA u32 (0xAARRGGBB) for the ACTIVE theme
    // (light/dark aware), or 0 for an unknown token. RENDER-gated. Lets an app
    // that paints its own surface (e.g. the browser's Canvas) match the theme's
    // colours instead of hardcoding them.
    linker.func_wrap("env", "npk_theme_token",
        |mut caller: Caller<'_, HostState>, token_id: i32| -> i32 {
            host_core::npk_theme_token(caller.data_mut(), token_id)
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
            let Some(m) = caller.get_export("memory").and_then(|e| e.into_memory())
                else { return -1 };
            let (mem, ctx) = m.data_and_store_mut(&mut caller);
            host_core::npk_canvas_rect(mem, ctx, canvas_id, out_ptr)
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
        |mut caller: Caller<'_, HostState>| -> i32 {
            host_core::npk_cursor_pos(caller.data_mut())
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_screen_flash() -> 0 or -1
    // CAPTURE-gated: same right as reading the screen, because this is
    // the acknowledgement for exactly that act. Paints a white wash over
    // the finished frame for ~150 ms. The caller is expected to capture
    // FIRST and flash after, so the wash can never be in the shot; the
    // compositor also draws it last, after every window.
    linker.func_wrap("env", "npk_screen_flash",
        |mut caller: Caller<'_, HostState>| -> i32 {
            host_core::npk_screen_flash(caller.data_mut())
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_capture_screen(buf_ptr, buf_max) -> bytes_written or -1
    // CAPTURE-gated (screen-scrape — only the screenshot tool holds it).
    // Copies the composited front framebuffer as tightly-packed BGRA32
    // (width*height*4) into the app buffer. The app then PNG-encodes /
    // crops it itself; the kernel only hands over the raw pixels.
    linker.func_wrap("env", "npk_capture_screen",
        |mut caller: Caller<'_, HostState>, buf_ptr: i32, buf_max: i32| -> i32 {
            let Some(m) = caller.get_export("memory").and_then(|e| e.into_memory())
                else { return -1 };
            let (mem, ctx) = m.data_and_store_mut(&mut caller);
            host_core::npk_capture_screen(mem, ctx, buf_ptr, buf_max)
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
            let Some(m) = caller.get_export("memory").and_then(|e| e.into_memory())
                else { return -1 };
            let (mem, ctx) = m.data_and_store_mut(&mut caller);
            host_core::npk_event_poll(mem, ctx, buf_ptr, buf_max)
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
            let Some(m) = caller.get_export("memory").and_then(|e| e.into_memory())
                else { return -1 };
            let (mem, ctx) = m.data_and_store_mut(&mut caller);
            host_core::npk_list_modules(mem, ctx, buf_ptr, buf_max)
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
        |mut caller: Caller<'_, HostState>, name_ptr: i32, name_len: i32, buf_ptr: i32, buf_max: i32| -> i32 {
            let Some(m) = caller.get_export("memory").and_then(|e| e.into_memory())
                else { return -1 };
            let (mem, ctx) = m.data_and_store_mut(&mut caller);
            host_core::npk_app_meta(mem, ctx, name_ptr, name_len, buf_ptr, buf_max)
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
        |mut caller: Caller<'_, HostState>, name_ptr: i32, name_len: i32| -> i32 {
            let Some(m) = caller.get_export("memory").and_then(|e| e.into_memory())
                else { return -1 };
            let (mem, ctx) = m.data_and_store_mut(&mut caller);
            host_core::npk_spawn_module(mem, ctx, name_ptr, name_len)
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
        |mut caller: Caller<'_, HostState>, verb_ptr: i32, verb_len: i32| -> i32 {
            let Some(m) = caller.get_export("memory").and_then(|e| e.into_memory())
                else { return -1 };
            let (mem, ctx) = m.data_and_store_mut(&mut caller);
            host_core::npk_run_intent(mem, ctx, verb_ptr, verb_len)
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
            host_core::npk_window_set_overlay(caller.data_mut(), w, h)
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
        |mut caller: Caller<'_, HostState>, modal: i32| -> i32 {
            host_core::npk_window_set_modal(caller.data_mut(), modal)
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
            host_core::npk_window_set_overlay_at(caller.data_mut(), x, y, w, h)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_window_set_light_dismiss(on: i32) -> i32
    // Opt the caller's widget window into light-dismiss: the compositor
    // closes it when a click lands outside it (transient overlays like the
    // volume slider). Off by default, so other overlays (loft, drun) are
    // unaffected. Returns 0 on success, -1 if no widget window / cap denied.
    linker.func_wrap("env", "npk_window_set_light_dismiss",
        |mut caller: Caller<'_, HostState>, on: i32| -> i32 {
            host_core::npk_window_set_light_dismiss(caller.data_mut(), on)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_window_set_clipboard_sink() -> i32
    // Opt the caller's widget window into Ctrl+C/X/V delivery as
    // Event::Clipboard when a focused text widget can't act on the chord
    // (copy/cut with no selection, paste into an empty single-line Input).
    // Used by file managers so the shortcuts drive file operations without
    // stealing text copy/paste from other apps. Returns 0 / -1.
    linker.func_wrap("env", "npk_window_set_clipboard_sink",
        |mut caller: Caller<'_, HostState>| -> i32 {
            host_core::npk_window_set_clipboard_sink(caller.data_mut())
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
            host_core::npk_window_set_dock(caller.data_mut(), w, h)
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
            host_core::npk_window_set_panel(caller.data_mut(), edge, behavior, w, h)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_bar_state(buf, max) -> i32
    // Live state for the bar app: "HH:MM\n<ws_count>\n<ws_active>\n<title>"
    // (clock already timezone-adjusted). Returns bytes written, -1 on
    // cap / args / buffer too small.
    linker.func_wrap("env", "npk_bar_state",
        |mut caller: Caller<'_, HostState>, buf_ptr: i32, max: i32| -> i32 {
            let Some(m) = caller.get_export("memory").and_then(|e| e.into_memory())
                else { return -1 };
            let (mem, ctx) = m.data_and_store_mut(&mut caller);
            host_core::npk_bar_state(mem, ctx, buf_ptr, max)
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
            let Some(m) = caller.get_export("memory").and_then(|e| e.into_memory())
                else { return -1 };
            let (mem, ctx) = m.data_and_store_mut(&mut caller);
            host_core::npk_window_titles(mem, ctx, buf_ptr, max)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_battery() -> i32 — battery state for the bar plugin. Returns -1
    // when no battery is known (desktops/QEMU → segment stays empty), else
    // (status << 8) | percent, with status 0=discharging 1=charging 2=full
    // 3=plugged-idle and percent in 0..=100. Prefers the AML driver's report
    // (aml.wasm, vendor-independent via _BST/_BIF); falls back to the
    // standardised SBS-over-SMBus path for SBS laptops.
    linker.func_wrap("env", "npk_battery",
        |mut caller: Caller<'_, HostState>| -> i32 {
            host_core::npk_battery(caller.data_mut())
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // ── AML battery driver (aml.wasm) host-fns — all HARDWARE-gated ──────
    // npk_acpi_dsdt(buf_ptr, buf_max) -> i32: copy the DSDT (firmware AML)
    // into the caller's buffer; returns the DSDT length. If it exceeds
    // buf_max nothing is copied (caller sizes its buffer up). -1 on error.
    linker.func_wrap("env", "npk_acpi_dsdt",
        |mut caller: Caller<'_, HostState>, buf_ptr: i32, buf_max: i32| -> i32 {
            let Some(m) = caller.get_export("memory").and_then(|e| e.into_memory())
                else { return -1 };
            let (mem, ctx) = m.data_and_store_mut(&mut caller);
            host_core::npk_acpi_dsdt(mem, ctx, buf_ptr, buf_max)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_ec_read(addr) -> i32: read one EC-RAM byte (0..255) or -1.
    linker.func_wrap("env", "npk_ec_read",
        |mut caller: Caller<'_, HostState>, addr: i32| -> i32 {
            host_core::npk_ec_read(caller.data_mut(), addr)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_ec_write(addr, val) -> i32: firmware-directed EC write (BSEL etc.).
    // 0 on success, -1 on error.
    linker.func_wrap("env", "npk_ec_write",
        |mut caller: Caller<'_, HostState>, addr: i32, val: i32| -> i32 {
            host_core::npk_ec_write(caller.data_mut(), addr, val)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_battery_report(packed): the AML driver pushes the decoded battery
    // state ((status<<8)|percent, or -1 for absent) into the kernel cache
    // that npk_battery() returns.
    linker.func_wrap("env", "npk_battery_report",
        |mut caller: Caller<'_, HostState>, packed: i32| {
            host_core::npk_battery_report(caller.data_mut(), packed)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // ── Audio mailbox + mixer ────────────────────────────────────────────
    // Apps push PCM (S16LE / 48 kHz / stereo) into per-slot rings; the HDA
    // driver pulls a mixed stream via npk_audio_poll_mix. Ungated: audio
    // playback is not a security boundary, and the kernel holds no HDA
    // knowledge — it just shuttles + sum-mixes bytes.

    // npk_audio_open() -> slot index, or -1 if no slot free.
    linker.func_wrap("env", "npk_audio_open",
        |mut caller: Caller<'_, HostState>| -> i32 {
            host_core::npk_audio_open(caller.data_mut())
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_audio_close(slot) -> 0.
    linker.func_wrap("env", "npk_audio_close",
        |mut caller: Caller<'_, HostState>, slot: i32| -> i32 {
            host_core::npk_audio_close(caller.data_mut(), slot)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_audio_submit(slot, ptr, len) -> bytes accepted, or -1 on bad args.
    linker.func_wrap("env", "npk_audio_submit",
        |mut caller: Caller<'_, HostState>, slot: i32, ptr: i32, len: i32| -> i32 {
            let Some(m) = caller.get_export("memory").and_then(|e| e.into_memory())
                else { return -1 };
            let (mem, ctx) = m.data_and_store_mut(&mut caller);
            host_core::npk_audio_submit(mem, ctx, slot, ptr, len)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_audio_poll_mix(ptr, max) -> bytes written (driver side).
    linker.func_wrap("env", "npk_audio_poll_mix",
        |mut caller: Caller<'_, HostState>, ptr: i32, max: i32| -> i32 {
            let Some(m) = caller.get_export("memory").and_then(|e| e.into_memory())
                else { return -1 };
            let (mem, ctx) = m.data_and_store_mut(&mut caller);
            host_core::npk_audio_poll_mix(mem, ctx, ptr, max)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_audio_set_volume(pct) -> 0; npk_audio_get_volume() -> 0..=100.
    linker.func_wrap("env", "npk_audio_set_volume",
        |mut caller: Caller<'_, HostState>, pct: i32| -> i32 {
            host_core::npk_audio_set_volume(caller.data_mut(), pct)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;
    linker.func_wrap("env", "npk_audio_get_volume",
        |mut caller: Caller<'_, HostState>| -> i32 {
            host_core::npk_audio_get_volume(caller.data_mut())
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_workspace_switch(n) -> i32 — switch to workspace n (bar clicks).
    linker.func_wrap("env", "npk_workspace_switch",
        |mut caller: Caller<'_, HostState>, n: i32| -> i32 {
            host_core::npk_workspace_switch(caller.data_mut(), n)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_power() -> i32 — ACPI S5 power-off (bar power button). Does not
    // return on success.
    linker.func_wrap("env", "npk_power",
        |mut caller: Caller<'_, HostState>| -> i32 {
            host_core::npk_power(caller.data_mut())
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
        |mut caller: Caller<'_, HostState>, prefix_ptr: i32, prefix_len: i32, out_ptr: i32, out_cap: i32, recursive: i32| -> i32 {
            let Some(m) = caller.get_export("memory").and_then(|e| e.into_memory())
                else { return -1 };
            let (mem, ctx) = m.data_and_store_mut(&mut caller);
            host_core::npk_fs_list(mem, ctx, prefix_ptr, prefix_len, out_ptr, out_cap, recursive)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_fs_stat(name_ptr, name_len, out_ptr) -> i32
    // Write 17 bytes into out_ptr:
    //   size_le_u64 (8) + is_dir_u8 (1) + mtime_le_u64 (8).
    // Returns 17 on success, 0 if no entry, -1 on cap / args.
    // mtime is UTC seconds since the Unix epoch — zero means unknown.
    linker.func_wrap("env", "npk_fs_stat",
        |mut caller: Caller<'_, HostState>, name_ptr: i32, name_len: i32, out_ptr: i32| -> i32 {
            let Some(m) = caller.get_export("memory").and_then(|e| e.into_memory())
                else { return -1 };
            let (mem, ctx) = m.data_and_store_mut(&mut caller);
            host_core::npk_fs_stat(mem, ctx, name_ptr, name_len, out_ptr)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_fs_delete(name_ptr, name_len) -> i32
    // Delete a single npkFS key. WRITE-gated. Returns 0 on success,
    // -1 on cap / not found / fs error.
    linker.func_wrap("env", "npk_fs_delete",
        |mut caller: Caller<'_, HostState>, name_ptr: i32, name_len: i32| -> i32 {
            let Some(m) = caller.get_export("memory").and_then(|e| e.into_memory())
                else { return -1 };
            let (mem, ctx) = m.data_and_store_mut(&mut caller);
            host_core::npk_fs_delete(mem, ctx, name_ptr, name_len)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_fs_rename(old_ptr, old_len, new_ptr, new_len) -> i32
    // Move/rename a single npkFS key (files and whole directories).
    // Content-addressed, so even a directory move is O(1). WRITE-gated;
    // neither path may touch the module store. Returns 0 / -1.
    linker.func_wrap("env", "npk_fs_rename",
        |mut caller: Caller<'_, HostState>, old_ptr: i32, old_len: i32, new_ptr: i32, new_len: i32| -> i32 {
            let Some(m) = caller.get_export("memory").and_then(|e| e.into_memory())
                else { return -1 };
            let (mem, ctx) = m.data_and_store_mut(&mut caller);
            host_core::npk_fs_rename(mem, ctx, old_ptr, old_len, new_ptr, new_len)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_fs_copy(old_ptr, old_len, new_ptr, new_len) -> i32
    // Copy a single npkFS key (files and whole directories). Shares the
    // source's content hash — no data duplication. WRITE-gated; neither
    // path may touch the module store. Returns 0 / -1.
    linker.func_wrap("env", "npk_fs_copy",
        |mut caller: Caller<'_, HostState>, old_ptr: i32, old_len: i32, new_ptr: i32, new_len: i32| -> i32 {
            let Some(m) = caller.get_export("memory").and_then(|e| e.into_memory())
                else { return -1 };
            let (mem, ctx) = m.data_and_store_mut(&mut caller);
            host_core::npk_fs_copy(mem, ctx, old_ptr, old_len, new_ptr, new_len)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_close_widget() -> i32
    // Close the calling app's own widget window. The worker then falls
    // out of its `_start` loop by its own logic; this host fn only tears
    // down the window + scene + event queue. Returns 0 on success,
    // -1 if the app doesn't own a widget window.
    linker.func_wrap("env", "npk_close_widget",
        |mut caller: Caller<'_, HostState>| -> i32 {
            host_core::npk_close_widget(caller.data_mut())
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_get_fb_size() -> (width << 16) | height
    linker.func_wrap("env", "npk_get_fb_size",
        |mut caller: Caller<'_, HostState>| -> i64 {
            host_core::npk_get_fb_size(caller.data_mut())
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_set_wallpaper(ptr, len, width, height) -> 0 or -1
    // Receives raw BGRA pixel data, sets it as the compositor wallpaper.
    linker.func_wrap("env", "npk_set_wallpaper",
        |mut caller: Caller<'_, HostState>, ptr: i32, len: i32, width: i32, height: i32| -> i32 {
            let Some(m) = caller.get_export("memory").and_then(|e| e.into_memory())
                else { return -1 };
            let (mem, ctx) = m.data_and_store_mut(&mut caller);
            host_core::npk_set_wallpaper(mem, ctx, ptr, len, width, height)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_set_theme(ptr) -> 0 or -1
    // Receives 16 u32 colors (64 bytes), sets as theme palette.
    linker.func_wrap("env", "npk_set_theme",
        |mut caller: Caller<'_, HostState>, ptr: i32| -> i32 {
            let Some(m) = caller.get_export("memory").and_then(|e| e.into_memory())
                else { return -1 };
            let (mem, ctx) = m.data_and_store_mut(&mut caller);
            host_core::npk_set_theme(mem, ctx, ptr)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_sys_info(key) -> i64 — system information for apps (e.g. top)
    // Keys: 0=cores, 1=uptime_secs, 2=free_mb, 3=heap_used, 4=heap_total,
    //        5=tasks_spawned, 6=tasks_completed, 7=steals, 8=workers,
    //        9=has_mwait, 10=tsc_mhz, 11=queue_len(core N, pass core in high bits)
    linker.func_wrap("env", "npk_sys_info",
        |mut caller: Caller<'_, HostState>, key: i32| -> i64 {
            host_core::npk_sys_info(caller.data_mut(), key)
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
        |mut caller: Caller<'_, HostState>, ms: i32| -> i32 {
            host_core::npk_sleep(caller.data_mut(), ms)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_input_poll() -> key or -1 — non-blocking read from per-app buffer
    linker.func_wrap("env", "npk_input_poll",
        |mut caller: Caller<'_, HostState>| -> i32 {
            host_core::npk_input_poll(caller.data_mut())
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_input_wait(timeout_ms) -> key or -1 — blocking wait with timeout
    // Spins on worker core checking per-app key buffer + TSC deadline.
    // Flushes busy-TSC and marks core idle during wait for accurate CPU usage.
    linker.func_wrap("env", "npk_input_wait",
        |mut caller: Caller<'_, HostState>, timeout_ms: i32| -> i32 {
            host_core::npk_input_wait(caller.data_mut(), timeout_ms)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_clear() — clear the app's terminal
    linker.func_wrap("env", "npk_clear",
        |mut caller: Caller<'_, HostState>| {
            host_core::npk_clear(caller.data_mut())
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // ── Terminal Stream Sink (for remote debug mirroring) ─────────

    // npk_self_terminal() -> terminal_idx of this WASM task
    linker.func_wrap("env", "npk_self_terminal",
        |mut caller: Caller<'_, HostState>| -> i32 {
            host_core::npk_self_terminal(caller.data_mut())
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_stream_open(idx) -> 0 ok, -1 error
    linker.func_wrap("env", "npk_stream_open",
        |mut caller: Caller<'_, HostState>, idx: i32| -> i32 {
            host_core::npk_stream_open(caller.data_mut(), idx)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_stream_read(idx, buf_ptr, buf_len) -> bytes read (>=0) or -1 on error
    linker.func_wrap("env", "npk_stream_read",
        |mut caller: Caller<'_, HostState>, idx: i32, buf_ptr: i32, buf_len: i32| -> i32 {
            let Some(m) = caller.get_export("memory").and_then(|e| e.into_memory())
                else { return -1 };
            let (mem, ctx) = m.data_and_store_mut(&mut caller);
            host_core::npk_stream_read(mem, ctx, idx, buf_ptr, buf_len)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_stream_close(idx) -> 0
    linker.func_wrap("env", "npk_stream_close",
        |mut caller: Caller<'_, HostState>, idx: i32| -> i32 {
            host_core::npk_stream_close(caller.data_mut(), idx)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_key_inject(byte) -> 0
    // Injects a raw byte into the global keyboard buffer. Routes to the
    // currently-focused window's intent session. Used by debug.wasm.
    linker.func_wrap("env", "npk_key_inject",
        |mut caller: Caller<'_, HostState>, byte: i32| -> i32 {
            host_core::npk_key_inject(caller.data_mut(), byte)
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
        |mut caller: Caller<'_, HostState>, ip_packed: i32, port: i32| -> i32 {
            host_core::npk_tcp_connect(caller.data_mut(), ip_packed, port)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_tcp_status(handle) -> 1 established, 0 still handshaking, -1 failed.
    // The polling half of the non-blocking connect above. The module sleeps
    // between calls; Core 0's run loop drives the stack meanwhile.
    linker.func_wrap("env", "npk_tcp_status",
        |mut caller: Caller<'_, HostState>, handle: i32| -> i32 {
            host_core::npk_tcp_status(caller.data_mut(), handle)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_tcp_send(handle, buf_ptr, buf_len) -> 0 ok, -2 retry later, -1 error
    linker.func_wrap("env", "npk_tcp_send",
        |mut caller: Caller<'_, HostState>, handle: i32, buf_ptr: i32, buf_len: i32| -> i32 {
            let Some(m) = caller.get_export("memory").and_then(|e| e.into_memory())
                else { return -1 };
            let (mem, ctx) = m.data_and_store_mut(&mut caller);
            host_core::npk_tcp_send(mem, ctx, handle, buf_ptr, buf_len)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_tcp_recv(handle, buf_ptr, buf_max) -> bytes read (0 = none available), -1 on error
    linker.func_wrap("env", "npk_tcp_recv",
        |mut caller: Caller<'_, HostState>, handle: i32, buf_ptr: i32, buf_max: i32| -> i32 {
            let Some(m) = caller.get_export("memory").and_then(|e| e.into_memory())
                else { return -1 };
            let (mem, ctx) = m.data_and_store_mut(&mut caller);
            host_core::npk_tcp_recv(mem, ctx, handle, buf_ptr, buf_max)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_tcp_close(handle) -> 0. Sends the FIN and returns; the graceful
    // wait is the kernel's job, not a module's — it spun up to 2 s here,
    // and 2 s of a frozen worker core is the WiFi driver not draining.
    linker.func_wrap("env", "npk_tcp_close",
        |mut caller: Caller<'_, HostState>, handle: i32| -> i32 {
            host_core::npk_tcp_close(caller.data_mut(), handle)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_debug_target_ip() -> packed IP (0 if unset)
    linker.func_wrap("env", "npk_debug_target_ip",
        |mut caller: Caller<'_, HostState>| -> i32 {
            host_core::npk_debug_target_ip(caller.data_mut())
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_debug_target_port() -> port (0 if unset)
    linker.func_wrap("env", "npk_debug_target_port",
        |mut caller: Caller<'_, HostState>| -> i32 {
            host_core::npk_debug_target_port(caller.data_mut())
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // ── Hardware Driver Host Functions ────────────────────────────

    // npk_pci_bind(vendor_id, device_id) -> 0=ok, -1=not found, -2=denied
    linker.func_wrap("env", "npk_pci_bind",
        |mut caller: Caller<'_, HostState>, vendor: i32, device: i32| -> i32 {
            host_core::npk_pci_bind(caller.data_mut(), vendor, device)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_pci_bind_class(class, subclass) -> 0=ok, -1=not found, -2=denied
    linker.func_wrap("env", "npk_pci_bind_class",
        |mut caller: Caller<'_, HostState>, class: i32, subclass: i32| -> i32 {
            host_core::npk_pci_bind_class(caller.data_mut(), class, subclass)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_pci_read_config(offset) -> u32 value or -1
    linker.func_wrap("env", "npk_pci_read_config",
        |mut caller: Caller<'_, HostState>, offset: i32| -> i32 {
            host_core::npk_pci_read_config(caller.data_mut(), offset)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_pci_write_config(offset, value) -> 0 or -1
    linker.func_wrap("env", "npk_pci_write_config",
        |mut caller: Caller<'_, HostState>, offset: i32, value: i32| -> i32 {
            host_core::npk_pci_write_config(caller.data_mut(), offset, value)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_pci_enable_bus_master() -> 0 or -1
    linker.func_wrap("env", "npk_pci_enable_bus_master",
        |mut caller: Caller<'_, HostState>| -> i32 {
            host_core::npk_pci_enable_bus_master(caller.data_mut())
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
        |mut caller: Caller<'_, HostState>, entry: i32| -> i32 {
            host_core::npk_irq_register(caller.data_mut(), entry)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_irq_arm(vector) -> fired-count snapshot, or -1 on a bad vector. Call
    // BEFORE submitting/enabling the device work that triggers the IRQ; pass
    // the result to npk_irq_wait. Also routes the IRQ to the calling core.
    linker.func_wrap("env", "npk_irq_arm",
        |mut caller: Caller<'_, HostState>, vector: i32| -> i64 {
            host_core::npk_irq_arm(caller.data_mut(), vector)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_irq_wait(vector, since, timeout_ms) -> 1 fired, 0 timeout, -1 bad arg.
    // Parks the driver's fiber until the device IRQ advances the fired-count
    // past `since`, or `timeout_ms` elapses (defaults to 1000 if <= 0).
    linker.func_wrap("env", "npk_irq_wait",
        |mut caller: Caller<'_, HostState>, vector: i32, since: i64, timeout_ms: i32| -> i32 {
            host_core::npk_irq_wait(caller.data_mut(), vector, since, timeout_ms)
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
            host_core::npk_mmio_map_bar(caller.data_mut(), bar_idx, pages)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_mmio_read32(handle, offset) -> u32
    linker.func_wrap("env", "npk_mmio_read32",
        |mut caller: Caller<'_, HostState>, handle: i32, offset: i32| -> i32 {
            host_core::npk_mmio_read32(caller.data_mut(), handle, offset)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_mmio_write32(handle, offset, value) -> 0 or -1
    linker.func_wrap("env", "npk_mmio_write32",
        |mut caller: Caller<'_, HostState>, handle: i32, offset: i32, value: i32| -> i32 {
            host_core::npk_mmio_write32(caller.data_mut(), handle, offset, value)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_mmio_read16(handle, offset) -> u16 as i32
    linker.func_wrap("env", "npk_mmio_read16",
        |mut caller: Caller<'_, HostState>, handle: i32, offset: i32| -> i32 {
            host_core::npk_mmio_read16(caller.data_mut(), handle, offset)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_mmio_write16(handle, offset, value) -> 0 or -1
    // True 16-bit MMIO write — required for split registers like RX/TX BD IDX
    // (HOST_IDX[15:0] + HW_IDX[31:16]). A 32-bit RMW would clobber HW_IDX.
    linker.func_wrap("env", "npk_mmio_write16",
        |mut caller: Caller<'_, HostState>, handle: i32, offset: i32, value: i32| -> i32 {
            host_core::npk_mmio_write16(caller.data_mut(), handle, offset, value)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_mmio_read64(handle, offset) -> i64
    linker.func_wrap("env", "npk_mmio_read64",
        |mut caller: Caller<'_, HostState>, handle: i32, offset: i32| -> i64 {
            host_core::npk_mmio_read64(caller.data_mut(), handle, offset)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_mmio_write64(handle, offset, value) -> 0 or -1
    linker.func_wrap("env", "npk_mmio_write64",
        |mut caller: Caller<'_, HostState>, handle: i32, offset: i32, value: i64| -> i32 {
            host_core::npk_mmio_write64(caller.data_mut(), handle, offset, value)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_dma_alloc(page_count) -> handle or -1
    linker.func_wrap("env", "npk_dma_alloc",
        |mut caller: Caller<'_, HostState>, pages: i32| -> i32 {
            host_core::npk_dma_alloc(caller.data_mut(), pages)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_dma_phys_addr(handle) -> physical address as i64
    linker.func_wrap("env", "npk_dma_phys_addr",
        |mut caller: Caller<'_, HostState>, handle: i32| -> i64 {
            host_core::npk_dma_phys_addr(caller.data_mut(), handle)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_dma_read(handle, dma_offset, wasm_ptr, len) -> 0 or -1
    linker.func_wrap("env", "npk_dma_read",
        |mut caller: Caller<'_, HostState>, handle: i32, dma_off: i32, wasm_ptr: i32, len: i32| -> i32 {
            let Some(m) = caller.get_export("memory").and_then(|e| e.into_memory())
                else { return -1 };
            let (mem, ctx) = m.data_and_store_mut(&mut caller);
            host_core::npk_dma_read(mem, ctx, handle, dma_off, wasm_ptr, len)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_dma_write(handle, dma_offset, wasm_ptr, len) -> 0 or -1
    linker.func_wrap("env", "npk_dma_write",
        |mut caller: Caller<'_, HostState>, handle: i32, dma_off: i32, wasm_ptr: i32, len: i32| -> i32 {
            let Some(m) = caller.get_export("memory").and_then(|e| e.into_memory())
                else { return -1 };
            let (mem, ctx) = m.data_and_store_mut(&mut caller);
            host_core::npk_dma_write(mem, ctx, handle, dma_off, wasm_ptr, len)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_dma_read32(handle, offset) -> u32
    linker.func_wrap("env", "npk_dma_read32",
        |mut caller: Caller<'_, HostState>, handle: i32, offset: i32| -> i32 {
            host_core::npk_dma_read32(caller.data_mut(), handle, offset)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_dma_write32(handle, offset, value) -> 0 or -1
    linker.func_wrap("env", "npk_dma_write32",
        |mut caller: Caller<'_, HostState>, handle: i32, offset: i32, value: i32| -> i32 {
            host_core::npk_dma_write32(caller.data_mut(), handle, offset, value)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_memory_fence() -> 0
    linker.func_wrap("env", "npk_memory_fence",
        |mut caller: Caller<'_, HostState>| -> i32 {
            host_core::npk_memory_fence(caller.data_mut())
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_netdev_register(mac_ptr) -> 0 or -1
    linker.func_wrap("env", "npk_netdev_register",
        |mut caller: Caller<'_, HostState>, mac_ptr: i32| -> i32 {
            let Some(m) = caller.get_export("memory").and_then(|e| e.into_memory())
                else { return -1 };
            let (mem, ctx) = m.data_and_store_mut(&mut caller);
            host_core::npk_netdev_register(mem, ctx, mac_ptr)
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
        |mut caller: Caller<'_, HostState>, buf_ptr: i32, len: i32| -> i32 {
            let Some(m) = caller.get_export("memory").and_then(|e| e.into_memory())
                else { return -1 };
            let (mem, ctx) = m.data_and_store_mut(&mut caller);
            host_core::npk_wifi_send_cmd(mem, ctx, buf_ptr, len)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_wifi_poll_event(buf_ptr, max) -> len / -1 — manager dequeues an event.
    linker.func_wrap("env", "npk_wifi_poll_event",
        |mut caller: Caller<'_, HostState>, buf_ptr: i32, max: i32| -> i32 {
            let Some(m) = caller.get_export("memory").and_then(|e| e.into_memory())
                else { return -1 };
            let (mem, ctx) = m.data_and_store_mut(&mut caller);
            host_core::npk_wifi_poll_event(mem, ctx, buf_ptr, max)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_wifi_poll_cmd(buf_ptr, max) -> len / -1 — driver dequeues a command.
    linker.func_wrap("env", "npk_wifi_poll_cmd",
        |mut caller: Caller<'_, HostState>, buf_ptr: i32, max: i32| -> i32 {
            let Some(m) = caller.get_export("memory").and_then(|e| e.into_memory())
                else { return -1 };
            let (mem, ctx) = m.data_and_store_mut(&mut caller);
            host_core::npk_wifi_poll_cmd(mem, ctx, buf_ptr, max)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_wifi_send_event(buf_ptr, len) -> 0 / -1 — driver enqueues an event.
    linker.func_wrap("env", "npk_wifi_send_event",
        |mut caller: Caller<'_, HostState>, buf_ptr: i32, len: i32| -> i32 {
            let Some(m) = caller.get_export("memory").and_then(|e| e.into_memory())
                else { return -1 };
            let (mem, ctx) = m.data_and_store_mut(&mut caller);
            host_core::npk_wifi_send_event(mem, ctx, buf_ptr, len)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_driver_report(buf_ptr, len) -> 0 / -1 — a bound driver publishes a
    // short plain-text status snapshot, read back with the `wlan` intent. The
    // kernel stores the bytes and a timestamp and never parses them: what is
    // worth reporting is device knowledge, and that stays in the driver.
    linker.func_wrap("env", "npk_driver_report",
        |mut caller: Caller<'_, HostState>, buf_ptr: i32, len: i32| -> i32 {
            let Some(m) = caller.get_export("memory").and_then(|e| e.into_memory())
                else { return -1 };
            let (mem, ctx) = m.data_and_store_mut(&mut caller);
            host_core::npk_driver_report(mem, ctx, buf_ptr, len)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // ── WiFi/NIC data path (driver ↔ kernel IP stack via the netdev mailbox) ─

    // npk_netdev_submit_rx(buf_ptr, len) -> 0 / -1 — driver hands a received
    // Ethernet frame to the kernel network stack.
    linker.func_wrap("env", "npk_netdev_submit_rx",
        |mut caller: Caller<'_, HostState>, buf_ptr: i32, len: i32| -> i32 {
            let Some(m) = caller.get_export("memory").and_then(|e| e.into_memory())
                else { return -1 };
            let (mem, ctx) = m.data_and_store_mut(&mut caller);
            host_core::npk_netdev_submit_rx(mem, ctx, buf_ptr, len)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_netdev_rx_deliver(buf_ptr, len) -> 0 / -1 — driver delivers a received
    // frame STRAIGHT into the IP stack from its own fiber (the NAPI topology:
    // drain → stack in one context, no relay-ring + Core-0 hop). Falls back to
    // the ring internally if Core 0 holds the drain guard. Preferred over
    // npk_netdev_submit_rx for the hot path.
    linker.func_wrap("env", "npk_netdev_rx_deliver",
        |mut caller: Caller<'_, HostState>, buf_ptr: i32, len: i32| -> i32 {
            let Some(m) = caller.get_export("memory").and_then(|e| e.into_memory())
                else { return -1 };
            let (mem, ctx) = m.data_and_store_mut(&mut caller);
            host_core::npk_netdev_rx_deliver(mem, ctx, buf_ptr, len)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_netdev_poll_tx(buf_ptr, max) -> len / -1 — driver fetches the next
    // frame the kernel wants transmitted (-1 when none / buffer too small).
    linker.func_wrap("env", "npk_netdev_poll_tx",
        |mut caller: Caller<'_, HostState>, buf_ptr: i32, max: i32| -> i32 {
            let Some(m) = caller.get_export("memory").and_then(|e| e.into_memory())
                else { return -1 };
            let (mem, ctx) = m.data_and_store_mut(&mut caller);
            host_core::npk_netdev_poll_tx(mem, ctx, buf_ptr, max)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_netdev_set_link(up) -> 0 — the single-flag form. Kept for drivers
    // that know only "usable / not usable"; it reports `up` as carrier with no
    // dormant phase.
    linker.func_wrap("env", "npk_netdev_set_link",
        |mut caller: Caller<'_, HostState>, up: i32| -> i32 {
            host_core::npk_netdev_set_link(caller.data_mut(), up)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    // npk_netdev_set_link_state(carrier, dormant) -> 0 — the RFC 2863 pair
    // Linux keeps (`rfc2863_policy`): `carrier` = the association exists,
    // `dormant` = it exists but is not usable yet (WPA not done). operstate
    // is UP only when carrier && !dormant. One flag for all three meanings
    // made every authorization phase look like the link had gone away.
    linker.func_wrap("env", "npk_netdev_set_link_state",
        |mut caller: Caller<'_, HostState>, carrier: i32, dormant: i32| -> i32 {
            host_core::npk_netdev_set_link_state(caller.data_mut(), carrier, dormant)
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    Ok(())
}

/// Free everything an instance leaves behind: its driver's hardware, and any
/// fetch it started and never collected.
fn cleanup_instance_state(state: &mut HostState) {
    // A dead driver's snapshot must not read as live numbers.
    crate::drivers::report::clear(&state.module_name);
    // A browser closed mid-load would otherwise hold its slots — and its
    // megabytes of reserved answer — until the next boot.
    crate::intent::fetch::release_owner(state.pid);
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

