# `kernel/src/wasm.rs` @ 5e0102684

## L1-6 · `use alloc::string::String;`

```
//! WASM Runtime
//!
//! Sandboxed execution via wasmi interpreter.
//! Every host function is capability-gated.
//! Modules loaded from npkFS execute with delegated capabilities —
//! no ambient authority, no access beyond what was explicitly granted.
```

## L19-30 · `static FORGE_DEFAULT: AtomicBool = AtomicBool::new(false);`

```
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
```

## L33-42 · `pub fn load_engine_default() {`

```
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
```

## L49 · `pub fn forge_is_default() -> bool {`

```
/// Welcher Motor faehrt, wenn der Aufrufer nichts anderes sagt.
```

## L54 · `pub fn set_engine_default(forge: bool) {`

```
/// Umlegen und merken.
```

## L64 · `struct HwDriverState {`

```
/// Hardware driver state for WASM modules that access PCI devices.
```

## L66-72 · `is_pci: bool,`

```
/// Haengt dieser Zustand an einem PCI-Geraet?
///
/// Seit es `npk_mmio_map_phys` gibt, kann ein Treiber Hardware fahren,
/// die NICHT auf PCI liegt — der Designware-I2C im AMD-FCH zum
/// Beispiel. Dann ist `pci_addr` ohne Bedeutung, und jeder Ruf, der
/// damit in den Konfigurationsraum greift, muss abgelehnt werden statt
/// auf 00:00.0 zu landen.
```

## L75 · `#[allow(dead_code)] // populated for future audit/debug, not yet read`

```
// populated for future audit/debug, not yet read
```

## L79 · `mmio_maps: Vec<(u64, usize)>,   // handle -> (base_virt, page_count)`

```
// handle -> (base_virt, page_count)
```

## L80 · `dma_allocs: Vec<(u64, usize)>,  // handle -> (phys_addr, page_count)`

```
// handle -> (phys_addr, page_count)
```

## L83-85 · `irq_vector: u8,`

```
/// The device-IRQ vector this driver registered, 0 = none. A driver
/// may arm and wait on THIS vector only — any vector of the pool would
/// let a module re-route another driver's interrupt to its own core.
```

## L87 · `irq_seen: u64,`

```
/// `irq::fired_count` of that vector when `npk_wait` last reported it.
```

## L92-96 · `const MAX_DMA_ALLOCS: usize = 1024;`

```
/// Per-module DMA allocation slots. Was 128, which the AX200 driver hit with
/// 64 one-page receive buffers plus its rings — and 64 buffers is 12 ms of
/// headroom at 64 Mbit, against a `worker_idle_hlt` that parks the core for up
/// to 10 ms between drains. Linux allocates 2048 for this chip. Each slot is a
/// (phys, pages) pair, so the ceiling is bookkeeping, not memory.
```

## L98 · `const MAX_DMA_PAGES: usize = 2048; // 8MB total (iwlwifi FW sections ~1.3MB)`

```
// 8MB total (iwlwifi FW sections ~1.3MB)
```

## L99 · `const MAX_DMA_PAGES_PER_CALL: usize = 1024; // 4MB; a single FW section can exceed 256KB`

```
// 4MB; a single FW section can exceed 256KB
```

## L104 · `direct_output: bool,`

```
/// When true, npk_print writes directly to terminal instead of buffering
```

## L106 · `terminal_idx: u8,`

```
/// Terminal index for direct output (255 = use active terminal via kprint)
```

## L108 · `core_id: usize,`

```
/// Core ID this WASM app is running on (for CPU usage tracking)
```

## L110 · `pid: u32,`

```
/// Process ID in the process table
```

## L112 · `hw: Option<HwDriverState>,`

```
/// Hardware driver state (only set for driver modules)
```

## L114-120 · `pub(crate) net_reach: crate::intent::http::Reach,`

```
/// Die Reichweite des Dokuments, das dieses Modul gerade anzeigt.
///
/// **Vorgabe `Public`, und das ist die strenge Wahl.** Ein Modul, das
/// nie etwas sagt, gilt als oeffentliche Seite und kommt damit nicht ins
/// private Netz. Nur wer `npk_net_context` ruft und dabei eine private
/// Adresse nennt, bekommt mehr — und auch das erst, nachdem der Kernel
/// die Adresse selbst AUFGELOEST hat.
```

## L122-125 · `widget_window_id: u32,`

```
/// Shade window id owned by this WASM app for widget rendering.
/// 0 = no widget window yet (first scene_commit allocates one).
/// Phase 10: set when the app calls npk_scene_commit, reused on
/// subsequent commits so the same window is updated in place.
```

## L127-129 · `module_name: String,`

```
/// Module name, used as the window title when the app's first
/// scene_commit (or npk_window_set_overlay) creates its widget
/// window. Cloned from the WASM job at worker entry.
```

## L131-132 · `launch_arg: Option<String>,`

```
/// Optional launch argument (e.g. a file path to open) the app reads
/// via `npk_launch_arg`. None = launched without an argument.
```

## L134-136 · `http_final_url: Option<String>,`

```
/// URL the last `npk_http_request` body actually came from, after
/// redirects. Read back via `npk_http_final_url` — a browser needs it
/// as the document base URL for relative sub-resources.
```

## L138-140 · `http_content_type: Option<String>,`

```
/// The last `npk_http_request`'s `Content-Type`. Read back via
/// `npk_http_content_type` — a browser cannot decode a document
/// without it, and guessing the charset wrong costs the WHOLE page.
```

## L142-145 · `http_last_error: Option<String>,`

```
/// Why the last `npk_http_request` failed, as `kind\tmessage`. Read
/// back via `npk_http_last_error`. Without it every failure reaches the
/// caller as a bare -1, which is how an untrusted certificate ended up
/// rendering as a blank page.
```

## L147-150 · `http_reply_headers: Option<String>,`

```
/// The last `npk_http_send` response's header block and status. Read back
/// via `npk_http_response_headers` / `npk_http_status`. A browser needs
/// headers the body cannot carry — `Set-Cookie` above all, which repeats
/// and so could never be a single-value getter.
```

## L153-158 · `pub(crate) wasi: Option<alloc::boxed::Box<crate::wasi::WasiCtx>>,`

```
/// A `wasi_snapshot_preview1` grant, or None.
///
/// The namespace is linked for every module, but every function in
/// it bounces on `ENOTCAPABLE` unless this is `Some`. So "can this
/// program see a filesystem" is decided once, here, by whoever
/// spawned it — not by what the program chooses to import.
```

## L164 · `const INTERACTIVE_FUEL: u64 = u64::MAX / 2;`

```
/// Fuel budget for interactive apps and drivers — effectively unlimited.
```

## L167-168 · `const PICKER_W: u32 = 760;`

```
/// Default dialog size for `npk_pick`. `set_overlay` clamps it to the
/// screen, so these are an upper bound, not a requirement.
```

## L172-175 · `fn picker_module_name() -> String {`

```
/// Module that serves `npk_pick`, from `sys/config/picker` (default
/// `pick`). Read here rather than hardcoded so the dialog is replaceable,
/// but never taken from the caller — a requester that could name its own
/// picker could show the user a fake dialog and answer it itself.
```

## L192 · `const MAX_WASM_JOBS: usize = 16;`

```
// ── Worker-Core WASM Jobs ──────────────────────────────────────
```

## L194-197 · `const MAX_WASM_JOBS: usize = 16;`

```
// Concurrent in-flight WASM spawn slots. A worker takes its slot out of
// the array as soon as it starts, so this caps *pending* (not-yet-started)
// spawns. 4 was too low once dock + bar + loft + iris are all resident —
// a transient one-shot (a screenshot) couldn't even get queued.
```

## L206-207 · `widget_window_id: u32,`

```
/// Pre-allocated widget window id for widget-kind apps. 0 = app
/// will get a window on its first npk_scene_commit (classic path).
```

## L209-210 · `launch_arg: Option<String>,`

```
/// Optional launch argument (e.g. a file path to open), readable by
/// the app via `npk_launch_arg`. Set by `npk_open`.
```

## L212-213 · `use_forge: bool,`

```
/// Run this one under forge instead of the interpreter. Per job, not
/// global: ein Fehler in der Bruecke legt so nicht jede App um.
```

## L220 · `static JOB_DONE: [core::sync::atomic::AtomicBool; MAX_WASM_JOBS] =`

```
/// Per-job completion flag (set by worker, read by BSP)
```

## L224-227 · `use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering as AtOrd};`

```
// ── Per-App Key Buffers (Core 0 writes, worker reads) ─────────
//
// Each terminal has its own SPSC ring buffer. Core 0 pushes keys
// based on which window is focused. Apps read via npk_input_wait.
```

## L233-240 · `const TERM_IDX_ACTIVE: u8 = 255;`

```
/// `terminal_idx` sentinel for "whatever terminal is focused". Spawn paths that
/// have no window of their own pass it (drivers from autostart, sandboxed
/// runs). It MUST be excluded before treating the index as a slot: 255 is
/// smaller than MAX_APP_BUFS, so it used to pass the bounds check, land in
/// `write_idx(255)` — a slot that is never allocated — and be dropped without
/// a trace. Every line an autostarted driver printed went there. Four kernel
/// lines and nothing from the driver is what that looks like from the outside,
/// and it cost an evening.
```

## L249 · `static APP_RUNNING: [AtomicBool; MAX_APP_BUFS] = {`

```
/// Per-terminal flag: true if a WASM app is running in this terminal.
```

## L255-257 · `static DEBUG_TARGET: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);`

```
/// Target IP/port for the debug reverse-mirror module. Packed as
/// `(ip as u64) << 16 | port as u64`. Set by the `debug` intent dispatcher
/// before spawning debug.wasm. 0 = unset.
```

## L332 · `pub fn push_app_key(terminal_idx: u8, key: u8) {`

```
/// Push a key to an app's input buffer. Called from Core 0.
```

## L336 · `let (buf, head, tail) = unsafe { &mut APP_KEY_BUFS[idx] };`

```
// SAFETY: single producer (Core 0), idx bounds checked
```

## L350-351 · `static APP_KEY_WAKER: [core::sync::atomic::AtomicU32; MAX_APP_BUFS] =`

```
/// The fiber of the app reading each terminal's key buffer, registered when
/// it waits. A key wakes it at once (`top` used to look every 10 ms).
```

## L361 · `pub(crate) fn has_app_key(terminal_idx: u8) -> bool {`

```
/// Is a key waiting in this terminal's app buffer?
```

## L365 · `let (_, head, tail) = unsafe { &APP_KEY_BUFS[idx] };`

```
// SAFETY: read-only look at the two indices, idx bounds checked.
```

## L370 · `fn pop_app_key(terminal_idx: u8) -> Option<u8> {`

```
/// Pop a key from an app's input buffer. Called from worker core.
```

## L374 · `let (buf, head, tail) = unsafe { &APP_KEY_BUFS[idx] };`

```
// SAFETY: single consumer (worker core), idx bounds checked
```

## L383 · `fn clear_app_key_buf(terminal_idx: u8) {`

```
/// Clear an app's key buffer. Called when spawning a new app.
```

## L393 · `pub fn has_wasm_app(terminal_idx: u8) -> bool {`

```
/// Check if the given terminal has a running WASM app.
```

## L400-401 · `pub fn spawn_on_worker(wasm_bytes: Vec<u8>, cap_id: CapId, terminal_idx: u8, module_name: &str) -> bool {`

```
/// Spawn a WASM module on a worker core. Returns immediately.
/// The app gets its own window and terminal.
```

## L406-416 · `pub fn spawn_on_worker_with_arg(`

```
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
```

## L428-432 · `pub fn spawn_widget_app(wasm_bytes: Vec<u8>, cap_id: CapId, module_name: &str, widget_wid: u32) -> bool {`

```
/// Spawn a widget-kind WASM app (Phase 10). The caller pre-allocates a
/// widget window and passes its id — the worker sets `widget_window_id`
/// in HostState so the first `npk_scene_commit` targets it directly.
/// Does NOT allocate a terminal or set APP_RUNNING — widget apps use
/// `npk_event_poll` for input, not the per-terminal key buffer.
```

## L437-438 · `pub fn spawn_on_worker_forge(wasm_bytes: Vec<u8>, cap_id: CapId, terminal_idx: u8, module_name: &str) -> bool {`

```
/// Wie `spawn_on_worker`, aber unter forge. Derselbe Weg, dieselbe Jobqueue,
/// dasselbe Fenster — nur der Motor ist ein anderer.
```

## L447-449 · `let e = forge_is_default();`

```
// Kein verdrahtetes `false` mehr: hier kommen Autostart, Treiber, Dienste
// und Widget-Apps alle durch, und sie sollen denselben Motor fahren wie
// alles andere.
```

## L477 · `if foreground {`

```
// Clear per-app input buffer + mark terminal as having an app (foreground only)
```

## L485-487 · `crate::smp::scheduler::spawn_fiber(`

```
// Run the app on a fiber (own stack) so it can yield at npk_sleep /
// npk_event_wait instead of pinning its worker core — see smp::fiber
// + docs/plan/SCHEDULER_FIBERS.md. Native intents still use plain `spawn`.
```

## L496 · `fn wasm_worker_task(arg: u64) {`

```
/// Worker-core entry: runs WASM module, signals completion.
```

## L514 · `let engine = match ENGINE.lock().as_ref().cloned() {`

```
// Clone engine (Arc internally, cheap)
```

## L527 · `let name_str = core::str::from_utf8(&job.name[..job.name_len as usize]).unwrap_or("?");`

```
// Register process in process table
```

## L568 · `if let Some(mem) = instance.get_memory(&store, "memory") {`

```
// Track WASM linear memory size
```

## L584 · `cleanup_instance_state(store.data_mut());`

```
// Cleanup hardware resources before process exit
```

## L587-590 · `capability::revoke_path_grants(&store.data().cap_id);`

```
// Drop this instance's per-path grants. They were handed out for one
// pick and must not outlive the app that got them — a later instance
// reusing the capability id would otherwise inherit a file it never
// asked for.
```

## L593 · `if let Some(mem) = instance.get_memory(&store, "memory") {`

```
// Update final memory usage
```

## L598 · `crate::process::exit(pid);`

```
// Deregister process + clear app marker + signal completion
```

## L607-610 · `fn forge_worker_task(slot: usize, job: WasmJob) {`

```
/// Derselbe Job, unter forge. Bewusst NEBEN `wasm_worker_task` und nicht
/// hinein: der Interpreterpfad bleibt Zeile fuer Zeile, wie er war, solange
/// dieser hier nicht gemessen ist. Der Preis ist ein doppelter Nachlauf von
/// zehn Zeilen — billiger als ein Umbau an dem Weg, an dem jede App haengt.
```

## L617-618 · `let done = |pid: u32, terminal_idx: u8, slot: usize| {`

```
// Ab hier muss jeder Ausgang aufraeumen, sonst bleibt ein Prozess stehen
// und das Terminal nimmt keine Tasten mehr an.
```

## L647-648 · `let mut hs = HostState {`

```
// Der Zustand gehoert hier UNS — unter wasmi haelt ihn der Store. Er darf
// sich nicht bewegen, solange die Instanz seinen Zeiger im vmctx hat.
```

## L671-672 · `let (frames, mb) = crate::mm::memory::stats();`

```
// Vier Dinge koennen hier scheitern, und die Meldung sagte keins davon.
// Der haeufigste Grund sind fehlende Rahmen — also stehen sie da.
```

## L679-680 · `let open = inst.unresolved_imports();`

```
// Ein Import auf dem Trap-Stumpf wuerde beim ersten Aufruf stehenbleiben.
// Das jetzt sagen ist besser als es spaeter als Absturz zu lesen.
```

## L696-697 · `cleanup_instance_state(&mut hs);`

```
// Wie im Interpreterpfad: Hardware zurueck, Pfadrechte weg. Beide
// arbeiten schon auf `&mut HostState`, also gilt hier dasselbe.
```

## L712-714 · `pub fn execute_sandboxed_with_fuel(`

```
/// Execute a WASM module with an explicit fuel budget. Use for trusted
/// first-party modules whose work is deterministically bounded by input
/// parameters (e.g. wallpaper generation sized by resolution).
```

## L721-727 · `pub fn execute_sandboxed_with_arg(`

```
/// Wie [`execute_sandboxed_with_fuel`], aber mit einem STARTARGUMENT.
///
/// Das ist dieselbe Zeichenkette, die `npk_open`/`npk_launch` einem Modul
/// mitgeben und die es mit `npk_launch_arg` abholt — nur kam sie bisher
/// ausschliesslich von einer anderen App. Von der Shell aus gab es keinen
/// Weg: `beak https://…` startete beak ohne die Adresse, obwohl beak sie
/// beim Start liest und ansteuert.
```

## L735-736 · `#[allow(dead_code)]`

```
/// Execute a WASM module in interactive mode (live display).
/// npk_print writes directly to terminal. Used for long-running apps (top).
```

## L748-751 · `let engine = {`

```
// Clone the engine (cheap Arc bump) and drop the ENGINE lock so two
// one-shot decodes (run/wallpaper) can run concurrently. The resident
// + `execute` paths already do this; only this one held the lock over
// the whole instantiate+call.
```

## L815-821 · `pub fn execute_wasi(`

```
/// Run a `wasm32-wasip1` module: instantiate, call `_start`, return its
/// exit status.
///
/// Separate from `execute_inner` because a wasi module is a different
/// animal: it has no `npk_*` entry point to name, it ends by trapping
/// out of `proc_exit` rather than returning, and a non-zero exit is a
/// result to report — not a kernel-side failure.
```

## L833-834 · `let t_c = crate::interrupts::ticks();`

```
// Dieselbe Teilung wie im forge-Pfad, sonst sind die Spalten nicht
// vergleichbar: auch wasmi bereitet das Modul einmal auf.
```

## L877-878 · `Err(e) => match e.kind().as_i32_exit_status() {`

```
// `proc_exit` leaves through a trap carrying the status. That is
// the normal way a wasi program finishes, including a clean one.
```

## L886-890 · `fn execute_inner_forge(`

```
/// Der Einmallauf unter forge — wallpaper und `run <mod> <func> <args>`.
///
/// forges Eintritt nimmt drei `u32`; alles darueber oder mit anderen Typen
/// bleibt beim Interpreter, und zwar SICHTBAR statt still. Der haeufige Fall
/// (`"_start"` ohne Argumente, wie wallpaper ihn ruft) geht durch.
```

## L952-959 · `pub fn execute_wasi_forge(`

```
/// Wie `execute_wasi`, aber unter forge. Bewusst daneben und nicht darin: der
/// Interpreterpfad bleibt, wie er ist, solange dieser hier nicht gemessen ist.
///
/// Der Unterschied ist genau einer — wie das Programm sich verabschiedet.
/// Unter wasmi kommt `proc_exit` als `Err` mit Status zurueck; unter forge
/// rollt es ueber `host_trap` ab und hinterlegt den Status vorher im
/// wasi-Zustand. Beide enden im selben Zustand, also liest der Rueckweg hier
/// dasselbe.
```

## L967-969 · `let t_c = crate::interrupts::ticks();`

```
// Eine Gesamtzahl verbirgt, welche Haelfte sich bewegt hat. Bei forge ist
// die eine Haelfte das Uebersetzen des ganzen Moduls — bei python 7,44 MB,
// und das faellt bei JEDEM Start an, solange der Codeblob nicht liegt.
```

## L977-978 · `let mut hs = HostState {`

```
// Der Zustand gehoert hier UNS — unter wasmi haelt ihn der Store. Er darf
// sich nicht bewegen, solange die Instanz seinen Zeiger im vmctx hat.
```

## L1014-1015 · `forge_core::trap::NONE => Ok(crate::wasi::exit_status(&hs).unwrap_or(0)),`

```
// Sauber aus `_start` zurueck: ein wasi-Programm tut das selten, aber
// es ist erlaubt und bedeutet Status 0.
```

## L1017 · `forge_core::trap::EXIT => Ok(crate::wasi::exit_status(&hs).unwrap_or(0)),`

```
// Der normale Abgang: `proc_exit` hat den Status vorher hinterlegt.
```

## L1027-1029 · `pub(crate) fn emit_output(state: &mut HostState, s: &str) {`

```
/// Route a string to wherever this run's output belongs: straight to a
/// specific terminal on a worker core, to the active terminal via
/// `kprint`, or into the buffer a one-shot `run` prints at the end.
```

## L1044 · `crate::wasi::link(linker).map_err(|_| WasmError::HostFunctionError)?;`

```
// The second ABI. Inert without a grant in HostState.wasi.
```

## L1047-1049 · `linker.func_wrap("env", "npk_print",`

```
// npk_print(ptr, len) — write to output buffer or directly to terminal
// Where an app's output goes, in one place — npk_print and the
// wasi fd_write path must not drift apart.
```

## L1059 · `linker.func_wrap("env", "npk_log",`

```
// npk_log(ptr, len) — write to serial console (no cap needed, output only)
```

## L1069-1078 · `linker.func_wrap("env", "npk_log_serial",`

```
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
```

## L1088 · `linker.func_wrap("env", "npk_fetch",`

```
// npk_fetch(name_ptr, name_len, buf_ptr, buf_max) -> bytes or -1
```

## L1098-1102 · `linker.func_wrap("env", "npk_http_request",`

```
// npk_http_request(url_ptr, url_len, buf_ptr, buf_max) -> bytes or -1
// Outbound HTTPS GET for the native browser (beak). Parses the URL,
// fetches the body (following redirects) via the same TLS path OTA
// uses, and copies up to buf_max bytes into the caller's buffer.
// NET-gated — distinct from npkFS READ and from WiFi NETCTL.
```

## L1112-1129 · `linker.func_wrap("env", "npk_http_send",`

```
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
```

## L1139-1142 · `linker.func_wrap("env", "npk_http_response_headers",`

```
// npk_http_response_headers(buf_ptr, buf_max) -> len, or -1
// The last npk_http_send response's header block, minus the status line.
// `Set-Cookie` repeats, so this is a raw block rather than a getter per
// name. NET-gated.
```

## L1152-1153 · `linker.func_wrap("env", "npk_http_status",`

```
// npk_http_status() -> status, or 0
// The last npk_http_send response's HTTP status. NET-gated.
```

## L1160-1171 · `linker.func_wrap("env", "npk_http_request_many",`

```
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
```

## L1181-1190 · `linker.func_wrap("env", "npk_net_context",`

```
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
```

## L1192-1200 · `linker.func_wrap("env", "npk_net_context",`

```
// npk_net_context(url_ptr, url_len) -> 0, or -1
//
// Der Browser sagt, WELCHES Dokument er gerade anzeigt. Der Kernel loest
// die Adresse SELBST auf und merkt sich nur die Klasse (oeffentlich /
// privat / lokal) — nie den Namen, denn ein Name kann beim zweiten
// Aufloesen woandershin zeigen.
//
// Ohne diesen Aufruf gilt das Modul als oeffentliche Seite. Das ist die
// strenge Vorgabe und deshalb sicher zu vergessen.
```

## L1210-1212 · `linker.func_wrap("env", "npk_http_begin",`

```
// npk_http_begin(method_ptr, method_len, url_ptr, url_len,
//                hdrs_ptr, hdrs_len, body_ptr, body_len, buf_max)
//   -> handle >= 1, or -1 (reason via npk_http_last_error)
```

## L1222 · `linker.func_wrap("env", "npk_http_begin_many",`

```
// npk_http_begin_many(urls_ptr, urls_len, out_max) -> handle >= 1, or -1
```

## L1232-1235 · `linker.func_wrap("env", "npk_http_begin_many_hdr",`

```
// npk_http_begin_many_hdr(urls_ptr, urls_len, hdrs_ptr, hdrs_len, out_max)
// Wie oben, aber mit einer Keks-Zeile JE ADRESSE. Eigene Funktion und
// keine geaenderte Signatur: eine geaenderte waere fuer jedes bereits
// ausgelieferte Modul ein Bindefehler.
```

## L1247-1248 · `linker.func_wrap("env", "npk_http_poll",`

```
// npk_http_poll(handle) -> 1 answer waiting, 0 running, -1 failed,
//                          -2 no such handle
```

## L1255-1257 · `linker.func_wrap("env", "npk_http_take",`

```
// npk_http_take(handle, buf_ptr, buf_max) -> bytes, -1 failed,
//                                            -2 unknown, -3 still running
// Frees the job and fills the same five getters npk_http_send fills.
```

## L1267-1268 · `linker.func_wrap("env", "npk_http_take_many",`

```
// npk_http_take_many(handle, out_ptr, out_max, lens_ptr, lens_max)
//   -> count, -1 failed, -2 unknown, -3 still running
```

## L1278 · `linker.func_wrap("env", "npk_http_cancel",`

```
// npk_http_cancel(handle) -> 0. Idempotent.
```

## L1285-1295 · `linker.func_wrap("env", "npk_random_bytes",`

```
// npk_random_bytes(buf_ptr, len) -> geschriebene Bytes, oder -1
//
// Zufall aus dem CSPRNG des Kernels (ChaCha20, aus RDRAND geseedet).
// OHNE Kapabilitaet, wie `npk_unix_time`: er gibt Bytes heraus und liest
// nichts, und eine Berechtigung, die niemand je verweigert, ist keine.
//
// **Der Grund, warum es das gibt:** beak braucht
// `crypto.getRandomValues` fuer Seiten, und `Math.random` dafuer
// auszugeben waere schlimmer als die Luecke — Seitencode baut daraus
// Sitzungsmarken. Gedeckelt auf 64 KiB je Aufruf (WebCrypto 10.1.1),
// damit ein Modul den RNG-Mutex nicht beliebig lange haelt.
```

## L1305-1309 · `linker.func_wrap("env", "npk_http_final_url",`

```
// npk_http_final_url(buf_ptr, buf_max) -> len, or -1
// The URL the last npk_http_request's body actually came from, after
// redirects. A browser resolves relative sub-resources against this
// (the document base URL) — resolving against the *requested* URL
// instead makes every sub-resource repeat the redirect. NET-gated.
```

## L1319-1327 · `linker.func_wrap("env", "npk_http_content_type",`

```
// npk_http_content_type(buf_ptr, buf_max) -> len, or -1
//
// The last npk_http_request's Content-Type, verbatim (e.g.
// "text/html; charset=ISO-8859-1"). Cleared when the request failed.
//
// A document's bytes do not say what encoding they are in. Without this
// a browser can only assume UTF-8, and one byte that is not valid UTF-8
// costs it the entire page — which is exactly what made google.ch render
// blank. NET-gated, like the request it describes.
```

## L1337-1346 · `linker.func_wrap("env", "npk_http_last_error",`

```
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
```

## L1356 · `linker.func_wrap("env", "npk_store",`

```
// npk_store(name_ptr, name_len, data_ptr, data_len) -> 0 or -1
```

## L1366-1372 · `linker.func_wrap("env", "npk_home_dir",`

```
// npk_home_dir(buf_ptr, buf_max) -> i32
// Write the current user's home directory ("home/<name>", or "home"
// if unset) into the caller's buffer; returns bytes written or -1.
// Apps need this because the username lives in the single encrypted
// `.system/config` blob, not a fetchable `sys/config/name` object —
// so they can't derive their home/documents path on their own.
// READ-gated (it reveals the user identity from config).
```

## L1382-1385 · `linker.func_wrap("env", "npk_fs_usage",`

```
// npk_fs_usage() -> i64
// Filesystem fill level as (used_mib << 32) | total_mib, or -1 when
// nothing is mounted. Feeds the file browser's capacity meter.
// READ-gated — it says how much of the disk is in use.
```

## L1392-1403 · `linker.func_wrap("env", "npk_locale",`

```
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
```

## L1413-1416 · `linker.func_wrap("env", "npk_launch_arg",`

```
// npk_launch_arg(buf_ptr, buf_max) -> i32
// Read the launch argument the app was started with (e.g. a file
// path passed by npk_open). Returns bytes written, 0 if none, -1 on
// error. Apps call this once at startup.
```

## L1426-1434 · `linker.func_wrap("env", "npk_clipboard_set",`

```
// ── Clipboard (cross-app copy/paste) ──────────────────────────────
//
// A single kernel-owned selection buffer (crate::shade::clipboard).
// Gated on RENDER + focus: only the *currently focused* widget app may
// read or write it — a background app cannot snoop the clipboard, the
// same focus-ambient contract as receiving keystrokes. (When a future
// third-party app store lands, promote clipboard-read to a declared
// CLIPBOARD cap — needs a 2nd `.npk.caps` byte; the 1-byte section is
// full today.)
```

## L1436-1438 · `linker.func_wrap("env", "npk_clipboard_set",`

```
// npk_clipboard_set(ptr, len) -> i32
// Copy `len` UTF-8 bytes from guest memory into the clipboard as Text.
// Returns bytes stored, or -1 (denied / not focused / bad ptr).
```

## L1448-1450 · `linker.func_wrap("env", "npk_clipboard_len",`

```
// npk_clipboard_len() -> i32
// Byte length of the current clipboard text (0 if empty). Lets an app
// size its buffer before npk_clipboard_get. Focus-gated like the rest.
```

## L1457-1460 · `linker.func_wrap("env", "npk_clipboard_get",`

```
// npk_clipboard_get(ptr, max) -> i32
// Write up to `max` clipboard bytes into the guest buffer. Returns the
// FULL text length (so the app can detect truncation and re-query with
// a bigger buffer), 0 if empty, or -1 (denied / not focused / bad ptr).
```

## L1470-1477 · `linker.func_wrap("env", "npk_open",`

```
// npk_open(app_ptr, app_len, arg_ptr, arg_len) -> i32
// Launch widget module `app` (sys/wasm/<app>) with `arg` as its launch
// argument (read by the app via npk_launch_arg). The launched app gets
// its own per-app caps (from its `.npk.caps` section) and a fresh
// window. EXECUTE-gated (launch authority). Used by loft for file
// associations — open a file with its handler app. The kernel stays
// generic: the ext→app mapping lives in the caller (loft + config),
// never here.
```

## L1487-1493 · `linker.func_wrap("env", "npk_launch",`

```
// npk_launch(app_ptr, app_len, arg_ptr, arg_len) -> 0 / -1
// Fire-and-forget launch of sys/wasm/<app> with `arg` as its launch
// argument + per-app caps — like npk_open but WITHOUT a pre-created
// window and WITHOUT singleton routing. The window (if any) is created
// lazily on the app's first scene_commit, so a one-shot tool that
// never commits (e.g. a full-screen screenshot) never shows a window
// — and so never appears in its own capture. EXECUTE-gated.
```

## L1503-1524 · `linker.func_wrap("env", "npk_pick",`

```
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
```

## L1534-1541 · `linker.func_wrap("env", "npk_pick_result",`

```
// npk_pick_result(path_ptr, path_len) -> 0 / -1
// Report the picked path back to whoever opened this dialog. An empty
// path means the user cancelled.
//
// Authorisation is structural: the caller's window id must be one the
// kernel itself registered as a picker in `npk_pick`. An ordinary app
// calling this finds no session and gets -1, so it cannot forge a
// "the user chose this file" claim for another app.
```

## L1551-1557 · `linker.func_wrap("env", "npk_window_set_close_guard",`

```
// npk_window_set_close_guard(on) -> 0 / -1
// Ask to be consulted before this window closes. A guarded window gets
// `Event::CloseRequest` on Mod+Q / the title-bar X instead of vanishing,
// so an app with unsaved work can prompt.
//
// Not a veto: asking again, or staying silent for a few seconds, closes
// it anyway. Apps that don't opt in are unaffected.
```

## L1564-1576 · `linker.func_wrap("env", "npk_pick_mkdir",`

```
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
```

## L1586-1598 · `linker.func_wrap("env", "npk_scene_commit",`

```
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
```

## L1608-1613 · `linker.func_wrap("env", "npk_canvas_commit",`

```
// npk_canvas_commit(canvas_id, ptr, len, width, height) -> 0 / -1
// P10.10 escape hatch: upload a raw BGRA32 bitmap into the app's
// `Widget::Canvas` with the matching id. CANVAS-gated. The app must
// already own a widget window (commit a scene first) — the bitmap is
// keyed by (window_id, canvas_id); the render walker blits it
// contain-fit into the canvas rect on the next rasterise.
```

## L1623-1626 · `linker.func_wrap("env", "npk_canvas_commit_yuv",`

```
// npk_canvas_commit_yuv(canvas_id, y, u, v, ys, cs, w, h, flags) -> 0 / -1
// The same escape hatch for a planar 4:2:0 frame — a video decoder
// hands over its own planes and the blit converts, at destination
// size and natively. CANVAS-gated exactly like the BGRA form.
```

## L1638-1641 · `linker.func_wrap("env", "npk_screen_size",`

```
// npk_screen_size() -> (width << 16) | height, or 0 on error.
// Allowed for RENDER (overlay sizing) OR CAPTURE (screenshot tool
// sizing its capture buffer — it has no RENDER in full-screen mode).
// (Screens are well under 65535 px/side.)
```

## L1648-1656 · `linker.func_wrap("env", "npk_ticks",`

```
// npk_ticks() -> milliseconds since boot (monotonic), or -1.
//
// A clock, not a calendar: no wall time, no timezone, nothing that
// identifies the machine — so it needs no capability, like the theme
// query. Resolution is the 100 Hz timer, i.e. 10 ms steps; enough to
// attribute phases of a page load, not enough to time a single glyph.
//
// Stage 1 needs this anyway for `setTimeout`/`requestAnimationFrame`
// (docs/spec/BROWSER.md §10 lists `now_ms` in the Platform surface).
```

## L1663-1669 · `linker.func_wrap("env", "npk_now_us",`

```
// npk_now_us() -> microseconds since boot, from the TSC, or 0.
//
// Same "clock, not calendar" argument as npk_ticks, so equally ungated — but
// fine enough to time ONE pass of a driver loop, which 10 ms steps cannot.
// That resolution is the whole difference between "the driver is busy" and
// "the driver is waiting": the WiFi driver's own busy counter only ever said
// whether a pass found work, and got read as CPU load (by me).
```

## L1676-1680 · `linker.func_wrap("env", "npk_unix_time",`

```
// npk_unix_time() -> seconds since the epoch, UTC, or 0 if the clock is
// not readable. Ungated, like npk_ticks: the wall clock is not a secret —
// it is on the bar and stamped into every npkFS entry. `npk_ticks` cannot
// stand in for it, because it restarts at every boot and a cookie's
// `Expires` is an absolute date.
```

## L1687-1690 · `linker.func_wrap("env", "npk_theme_token",`

```
// npk_theme_token(token_id) -> RGBA u32 (0xAARRGGBB) for the ACTIVE theme
// (light/dark aware), or 0 for an unknown token. RENDER-gated. Lets an app
// that paints its own surface (e.g. the browser's Canvas) match the theme's
// colours instead of hardcoding them.
```

## L1697-1702 · `linker.func_wrap("env", "npk_canvas_rect",`

```
// npk_canvas_rect(canvas_id, out_ptr) -> 0 / -1
// Writes the canvas widget's actual laid-out rect as 4 little-endian i32
// [x, y, w, h] into out_ptr (16 bytes), so an app can paint its canvas
// 1:1 (no contain-fit scaling) and map click coordinates into content
// space. RENDER-gated. Returns -1 until the canvas has been laid out once
// (commit a scene with the Canvas first, then query on the next frame).
```

## L1712-1720 · `linker.func_wrap("env", "npk_cursor_pos",`

```
// npk_cursor_pos() -> (x << 16) | y, or -1
//
// Screen coordinates, the same space `Event::MouseMove` and
// `Event::MouseButton` report. RENDER-gated AND focus-gated: an app
// may learn where the pointer is only while it holds focus, so a
// background module cannot watch the mouse.
//
// Exists because `Event::Wheel` carries no position — an app that
// wants to zoom towards the pointer has to ask for it.
```

## L1727-1732 · `linker.func_wrap("env", "npk_screen_flash",`

```
// npk_screen_flash() -> 0 or -1
// CAPTURE-gated: same right as reading the screen, because this is
// the acknowledgement for exactly that act. Paints a white wash over
// the finished frame for ~150 ms. The caller is expected to capture
// FIRST and flash after, so the wash can never be in the shot; the
// compositor also draws it last, after every window.
```

## L1739-1743 · `linker.func_wrap("env", "npk_capture_screen",`

```
// npk_capture_screen(buf_ptr, buf_max) -> bytes_written or -1
// CAPTURE-gated (screen-scrape — only the screenshot tool holds it).
// Copies the composited front framebuffer as tightly-packed BGRA32
// (width*height*4) into the app buffer. The app then PNG-encodes /
// crops it itself; the kernel only hands over the raw pixels.
```

## L1753-1758 · `linker.func_wrap("env", "npk_event_poll",`

```
// npk_event_poll(buf_ptr, buf_max) -> i32
// Non-blocking: pop one event from this app's widget-window
// queue, postcard-encode it into the supplied WASM buffer.
//   >0 → encoded byte count
//   0  → queue empty (app should sleep / yield)
//   -1 → no widget window, cap denied, or buffer too small
```

## L1768-1775 · `linker.func_wrap("env", "npk_list_modules",`

```
// npk_list_modules(buf_ptr, buf_max) -> i32
// Writes a NUL-separated list of module names from `sys/wasm/*` into
// the caller's buffer. Returns bytes written, or -1 on cap denied /
// buffer too small. The trailing entry is NOT terminated — caller
// splits on 0x00.
//
// RENDER-gated because only GUI apps (drun) need this today. Adjust
// if terminal utilities ever want the same API.
```

## L1785-1793 · `linker.func_wrap("env", "npk_app_meta",`

```
// npk_app_meta(name_ptr, name_len, buf_ptr, buf_max) -> bytes or -1
// Returns ONLY the `.npk.app_meta` custom-section payload of the module
// `sys/wasm/<name>`, extracted kernel-side. Launchers (drun/dock) read an
// app's icon/name/description with this WITHOUT fetching the whole module
// — beak carries >2 MB of embedded fonts, and the old client-side reader
// fetched the full wasm into a fixed 2 MB buffer, truncating beak so its
// trailing app_meta section was lost → the app vanished from the catalog.
// `name` is confined to a bare child of `sys/wasm/` (no path traversal).
// RENDER-gated like npk_list_modules.
```

## L1803-1813 · `linker.func_wrap("env", "npk_spawn_module",`

```
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
```

## L1823-1834 · `linker.func_wrap("env", "npk_run_intent",`

```
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
```

## L1844-1854 · `linker.func_wrap("env", "npk_window_set_overlay",`

```
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
```

## L1861-1867 · `linker.func_wrap("env", "npk_window_set_modal",`

```
// npk_window_set_modal(modal: i32) -> i32
// Toggle the modal flag on the calling app's widget window. While
// any window is modal, shade-action dispatch suppresses focus-shift
// / tiling shortcuts (see handle_action in shade/mod.rs).
//
// Returns 0 on success, -1 if the app has no widget window yet /
// cap denied.
```

## L1874-1880 · `linker.func_wrap("env", "npk_window_set_overlay_at",`

```
// npk_window_set_overlay_at(x, y, w, h) -> i32
// Like npk_window_set_overlay but positions the overlay's top-left at
// (x, y) instead of centring it — for corner-anchored dropdowns (e.g.
// the volume slider under the bar). Creates/promotes + focuses the
// caller's widget window, same as the centred overlay path.
//
// Returns 0 on success, -1 on cap denied / bad args / no compositor.
```

## L1887-1891 · `linker.func_wrap("env", "npk_window_set_light_dismiss",`

```
// npk_window_set_light_dismiss(on: i32) -> i32
// Opt the caller's widget window into light-dismiss: the compositor
// closes it when a click lands outside it (transient overlays like the
// volume slider). Off by default, so other overlays (loft, drun) are
// unaffected. Returns 0 on success, -1 if no widget window / cap denied.
```

## L1898-1903 · `linker.func_wrap("env", "npk_window_set_clipboard_sink",`

```
// npk_window_set_clipboard_sink() -> i32
// Opt the caller's widget window into Ctrl+C/X/V delivery as
// Event::Clipboard when a focused text widget can't act on the chord
// (copy/cut with no selection, paste into an empty single-line Input).
// Used by file managers so the shortcuts drive file operations without
// stealing text copy/paste from other apps. Returns 0 / -1.
```

## L1910-1917 · `linker.func_wrap("env", "npk_window_set_dock",`

```
// npk_window_set_dock(w, h) -> i32
// Turn the calling app's widget window into a bottom auto-hide dock:
// overlay (no tiling strut), never modal, never focused on reveal,
// global across workspaces. Starts hidden; the compositor slides it
// in when the cursor holds the bottom edge. Like set_overlay but
// bottom-anchored instead of centred, and it does NOT grab focus.
//
// Returns 0 on success, -1 on cap denied / bad args / no compositor.
```

## L1924-1931 · `linker.func_wrap("env", "npk_window_set_panel",`

```
// npk_window_set_panel(edge, behavior, w, h) -> i32
// Generalised edge panel (see docs/spec/PANEL.md): edge 0=Bottom 1=Top,
// behavior 0=AutoHide overlay (dock) 1=Strut (bar). Creates/promotes
// the caller's widget window WITHOUT grabbing focus (like the dock),
// then hands it to the compositor's panel config. `set_dock` above is
// now the (Bottom, AutoHide) wrapper of this.
//
// Returns 0 on success, -1 on cap denied / bad args / no compositor.
```

## L1938-1941 · `linker.func_wrap("env", "npk_bar_state",`

```
// npk_bar_state(buf, max) -> i32
// Live state for the bar app: "HH:MM\n<ws_count>\n<ws_active>\n<title>"
// (clock already timezone-adjusted). Returns bytes written, -1 on
// cap / args / buffer too small.
```

## L1951-1956 · `linker.func_wrap("env", "npk_window_titles",`

```
// npk_window_titles(buf, max) -> i32
// One line per open app window: "<flags>\t<workspace>\t<title>", flags
// being a decimal bitmask (1 = focused, 2 = on the active workspace).
// Panels and overlays are excluded. The dock derives its running/active
// indicators from this, the bar its occupied-workspace hints; the
// kernel stays free of app names.
```

## L1966-1971 · `linker.func_wrap("env", "npk_battery",`

```
// npk_battery() -> i32 — battery state for the bar plugin. Returns -1
// when no battery is known (desktops/QEMU → segment stays empty), else
// (status << 8) | percent, with status 0=discharging 1=charging 2=full
// 3=plugged-idle and percent in 0..=100. Prefers the AML driver's report
// (aml.wasm, vendor-independent via _BST/_BIF); falls back to the
// standardised SBS-over-SMBus path for SBS laptops.
```

## L1978-1981 · `linker.func_wrap("env", "npk_acpi_dsdt",`

```
// ── AML battery driver (aml.wasm) host-fns — all HARDWARE-gated ──────
// npk_acpi_dsdt(buf_ptr, buf_max) -> i32: copy the DSDT (firmware AML)
// into the caller's buffer; returns the DSDT length. If it exceeds
// buf_max nothing is copied (caller sizes its buffer up). -1 on error.
```

## L1991 · `linker.func_wrap("env", "npk_acpi_mem_read",`

```
// npk_acpi_mem_read(hi, lo) -> byte, or -1 (RAM / no right / unmapped)
```

## L1998-1999 · `linker.func_wrap("env", "npk_acpi_table",`

```
// npk_acpi_table(sig, index, buf_ptr, buf_max) -> len. Die n-te Tabelle
// mit dieser Signatur; `sig` sind die vier Zeichen little-endian.
```

## L2009-2011 · `linker.func_wrap("env", "npk_mmio_map_phys",`

```
// npk_mmio_map_phys(hi, lo, pages) -> handle, or -1. Fuer Hardware, die
// nicht auf PCI liegt (FCH-I2C: Touchpad). Rechte + RAM-/APIC-Verbot
// stehen in host_core.
```

## L2018 · `linker.func_wrap("env", "npk_pointer_inject",`

```
// npk_pointer_inject(dx, dy, buttons, scroll, hscroll) -> 0, oder -1 ohne Recht.
```

## L2026 · `linker.func_wrap("env", "npk_ec_query",`

```
// npk_ec_query() -> query number, or -1 when nothing is pending
```

## L2033 · `linker.func_wrap("env", "npk_ec_read",`

```
// npk_ec_read(addr) -> i32: read one EC-RAM byte (0..255) or -1.
```

## L2040-2041 · `linker.func_wrap("env", "npk_ec_write",`

```
// npk_ec_write(addr, val) -> i32: firmware-directed EC write (BSEL etc.).
// 0 on success, -1 on error.
```

## L2048-2050 · `linker.func_wrap("env", "npk_battery_report",`

```
// npk_battery_report(packed): the AML driver pushes the decoded battery
// state ((status<<8)|percent, or -1 for absent) into the kernel cache
// that npk_battery() returns.
```

## L2057-2058 · `linker.func_wrap("env", "npk_battery_detail",`

```
// npk_battery_detail(rate, remaining, full, voltage_mv, unit): the raw
// _BST/_BIF figures behind the percentage, for `battery`.
```

## L2066-2070 · `linker.func_wrap("env", "npk_audio_open",`

```
// ── Audio mailbox + mixer ────────────────────────────────────────────
// Apps push PCM (S16LE / 48 kHz / stereo) into per-slot rings; the HDA
// driver pulls a mixed stream via npk_audio_poll_mix. Ungated: audio
// playback is not a security boundary, and the kernel holds no HDA
// knowledge — it just shuttles + sum-mixes bytes.
```

## L2072 · `linker.func_wrap("env", "npk_audio_open",`

```
// npk_audio_open() -> slot index, or -1 if no slot free.
```

## L2079 · `linker.func_wrap("env", "npk_audio_close",`

```
// npk_audio_close(slot) -> 0.
```

## L2086 · `linker.func_wrap("env", "npk_audio_submit",`

```
// npk_audio_submit(slot, ptr, len) -> bytes accepted, or -1 on bad args.
```

## L2096-2097 · `linker.func_wrap("env", "npk_audio_buffered",`

```
// npk_audio_buffered(slot) -> bytes still in the ring, -1 if closed.
// The honest play clock; see host_core for why the wall clock is not one.
```

## L2104 · `linker.func_wrap("env", "npk_audio_poll_mix",`

```
// npk_audio_poll_mix(ptr, max) -> bytes written (driver side).
```

## L2114 · `linker.func_wrap("env", "npk_audio_set_volume",`

```
// npk_audio_set_volume(pct) -> 0; npk_audio_get_volume() -> 0..=100.
```

## L2126 · `linker.func_wrap("env", "npk_workspace_switch",`

```
// npk_workspace_switch(n) -> i32 — switch to workspace n (bar clicks).
```

## L2133-2134 · `linker.func_wrap("env", "npk_power",`

```
// npk_power() -> i32 — ACPI S5 power-off (bar power button). Does not
// return on success.
```

## L2141-2160 · `linker.func_wrap("env", "npk_fs_list",`

```
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
```

## L2170-2174 · `linker.func_wrap("env", "npk_fs_stat",`

```
// npk_fs_stat(name_ptr, name_len, out_ptr) -> i32
// Write 17 bytes into out_ptr:
//   size_le_u64 (8) + is_dir_u8 (1) + mtime_le_u64 (8).
// Returns 17 on success, 0 if no entry, -1 on cap / args.
// mtime is UTC seconds since the Unix epoch — zero means unknown.
```

## L2184-2186 · `linker.func_wrap("env", "npk_fs_delete",`

```
// npk_fs_delete(name_ptr, name_len) -> i32
// Delete a single npkFS key. WRITE-gated. Returns 0 on success,
// -1 on cap / not found / fs error.
```

## L2196-2199 · `linker.func_wrap("env", "npk_fs_rename",`

```
// npk_fs_rename(old_ptr, old_len, new_ptr, new_len) -> i32
// Move/rename a single npkFS key (files and whole directories).
// Content-addressed, so even a directory move is O(1). WRITE-gated;
// neither path may touch the module store. Returns 0 / -1.
```

## L2209-2212 · `linker.func_wrap("env", "npk_fs_copy",`

```
// npk_fs_copy(old_ptr, old_len, new_ptr, new_len) -> i32
// Copy a single npkFS key (files and whole directories). Shares the
// source's content hash — no data duplication. WRITE-gated; neither
// path may touch the module store. Returns 0 / -1.
```

## L2222-2226 · `linker.func_wrap("env", "npk_close_widget",`

```
// npk_close_widget() -> i32
// Close the calling app's own widget window. The worker then falls
// out of its `_start` loop by its own logic; this host fn only tears
// down the window + scene + event queue. Returns 0 on success,
// -1 if the app doesn't own a widget window.
```

## L2233 · `linker.func_wrap("env", "npk_get_fb_size",`

```
// npk_get_fb_size() -> (width << 16) | height
```

## L2240-2241 · `linker.func_wrap("env", "npk_set_wallpaper",`

```
// npk_set_wallpaper(ptr, len, width, height) -> 0 or -1
// Receives raw BGRA pixel data, sets it as the compositor wallpaper.
```

## L2251-2252 · `linker.func_wrap("env", "npk_set_theme",`

```
// npk_set_theme(ptr) -> 0 or -1
// Receives 16 u32 colors (64 bytes), sets as theme palette.
```

## L2262-2265 · `linker.func_wrap("env", "npk_sys_info",`

```
// npk_sys_info(key) -> i64 — system information for apps (e.g. top)
// Keys: 0=cores, 1=uptime_secs, 2=free_mb, 3=heap_used, 4=heap_total,
//        5=tasks_spawned, 6=tasks_completed, 7=steals, 8=workers,
//        9=has_mwait, 10=tsc_mhz, 11=queue_len(core N, pass core in high bits)
```

## L2272-2278 · `linker.func_wrap("env", "npk_sleep",`

```
// npk_sleep(ms) -> 0 — sleep for N milliseconds.
// Stage 2b: PARK this app's fiber + yield the worker core back to the
// per-core scheduler, which runs the core's other ready fibers while we
// sleep. So dock+bar+loft+spell multiplex over a couple of workers
// instead of each pinning a core (or nesting via the old next_task
// helper, which froze the dock — see docs/plan/SCHEDULER_FIBERS.md). The fiber is
// resumed once the deadline passes.
```

## L2285 · `linker.func_wrap("env", "npk_input_poll",`

```
// npk_input_poll() -> key or -1 — non-blocking read from per-app buffer
```

## L2292-2294 · `linker.func_wrap("env", "npk_input_wait",`

```
// npk_input_wait(timeout_ms) -> key or -1 — blocking wait with timeout
// Spins on worker core checking per-app key buffer + TSC deadline.
// Flushes busy-TSC and marks core idle during wait for accurate CPU usage.
```

## L2301-2302 · `linker.func_wrap("env", "npk_sci_arm",`

```
// npk_sci_arm(gpe) -> vector | -1 and npk_sci_service() -> mask: the
// ACPI SCI for the AML driver's EC (drivers/sci.rs).
```

## L2314-2315 · `linker.func_wrap("env", "npk_irq_register_gsi",`

```
// npk_irq_register_gsi(gsi, flags) -> vector | -1 — a non-PCI device's
// I/O APIC line (flags: 1 level, 2 active-low). HARDWARE-gated.
```

## L2322-2324 · `linker.func_wrap("env", "npk_wait",`

```
// npk_wait(mask, timeout_ms) -> fired bits (0 = timeout). Parks the
// app's fiber until an input event arrives (mask bit 1) or the timeout
// passes; timeout < 0 waits without one. See host_core::npk_wait.
```

## L2331 · `linker.func_wrap("env", "npk_clear",`

```
// npk_clear() — clear the app's terminal
```

## L2338 · `linker.func_wrap("env", "npk_self_terminal",`

```
// ── Terminal Stream Sink (for remote debug mirroring) ─────────
```

## L2340 · `linker.func_wrap("env", "npk_self_terminal",`

```
// npk_self_terminal() -> terminal_idx of this WASM task
```

## L2347 · `linker.func_wrap("env", "npk_stream_open",`

```
// npk_stream_open(idx) -> 0 ok, -1 error
```

## L2354 · `linker.func_wrap("env", "npk_stream_read",`

```
// npk_stream_read(idx, buf_ptr, buf_len) -> bytes read (>=0) or -1 on error
```

## L2364 · `linker.func_wrap("env", "npk_stream_close",`

```
// npk_stream_close(idx) -> 0
```

## L2371-2373 · `linker.func_wrap("env", "npk_key_inject",`

```
// npk_key_inject(byte) -> 0
// Injects a raw byte into the global keyboard buffer. Routes to the
// currently-focused window's intent session. Used by debug.wasm.
```

## L2380 · `linker.func_wrap("env", "npk_tls_connect",`

```
// ── TCP Socket Host Functions (debug shell + future apps) ────
```

## L2382-2395 · `linker.func_wrap("env", "npk_tls_connect",`

```
// npk_tcp_connect(ip_packed, port) -> handle (>=0) or -1 on error.
// ip_packed = (a << 24) | (b << 16) | (c << 8) | d.
//
// NON-BLOCKING: returns as soon as the handshake is started. Ask
// `npk_tcp_status` until it answers; `npk_tcp_send` refuses until then.
// It used to block up to 10 s, and a module IS a fiber — so a failing
// `debug` froze every other fiber on its worker core for those 10 s,
// the WiFi driver among them. Its card went unpolled (64 RX buffers =
// milliseconds), and the link died with the command.
// ── npk_tls_* ────────────────────────────────────────────────────────
//
// **In BEIDEN Wegen**, sonst laeuft es unter forge und stirbt unter dem
// Interpreter (oder umgekehrt) — der Fehler, den `feedback_the_second_
// engine_only_runs_where_the_first_one_called` beschreibt.
```

## L2433-2435 · `linker.func_wrap("env", "npk_tcp_status",`

```
// npk_tcp_status(handle) -> 1 established, 0 still handshaking, -1 failed.
// The polling half of the non-blocking connect above. The module sleeps
// between calls; Core 0's run loop drives the stack meanwhile.
```

## L2442 · `linker.func_wrap("env", "npk_tcp_send",`

```
// npk_tcp_send(handle, buf_ptr, buf_len) -> 0 ok, -2 retry later, -1 error
```

## L2452 · `linker.func_wrap("env", "npk_tcp_recv",`

```
// npk_tcp_recv(handle, buf_ptr, buf_max) -> bytes read (0 = none available), -1 on error
```

## L2462-2464 · `linker.func_wrap("env", "npk_tcp_close",`

```
// npk_tcp_close(handle) -> 0. Sends the FIN and returns; the graceful
// wait is the kernel's job, not a module's — it spun up to 2 s here,
// and 2 s of a frozen worker core is the WiFi driver not draining.
```

## L2471 · `linker.func_wrap("env", "npk_debug_target_ip",`

```
// npk_debug_target_ip() -> packed IP (0 if unset)
```

## L2478 · `linker.func_wrap("env", "npk_debug_target_port",`

```
// npk_debug_target_port() -> port (0 if unset)
```

## L2485 · `linker.func_wrap("env", "npk_pci_bind",`

```
// ── Hardware Driver Host Functions ────────────────────────────
```

## L2487 · `linker.func_wrap("env", "npk_pci_bind",`

```
// npk_pci_bind(vendor_id, device_id) -> 0=ok, -1=not found, -2=denied
```

## L2494 · `linker.func_wrap("env", "npk_pci_bind_class",`

```
// npk_pci_bind_class(class, subclass) -> 0=ok, -1=not found, -2=denied
```

## L2501 · `linker.func_wrap("env", "npk_pci_bind_class_n",`

```
// npk_pci_bind_class_n(class, subclass, index) -> 0=ok, -1=not found, -2=denied
```

## L2508 · `linker.func_wrap("env", "npk_pci_read_config",`

```
// npk_pci_read_config(offset) -> u32 value or -1
```

## L2515 · `linker.func_wrap("env", "npk_pci_write_config",`

```
// npk_pci_write_config(offset, value) -> 0 or -1
```

## L2522 · `linker.func_wrap("env", "npk_pci_enable_bus_master",`

```
// npk_pci_enable_bus_master() -> 0 or -1
```

## L2529-2537 · `linker.func_wrap("env", "npk_irq_register",`

```
// ── Device-interrupt ABI (MSI-X → LAPIC → fiber wake) ───────────────
//
// Lets a WASM driver go IRQ-driven instead of `npk_sleep`-polling: bind a
// device, `npk_irq_register` its MSI-X entry once, then loop
//   since = npk_irq_arm(vec); <enable/submit device work>;
//   npk_irq_wait(vec, since, timeout); <service>
// The driver's fiber parks until the device fires; the IRQ wakes its core.
// The driver still enables the device's own interrupt source via its MMIO
// (e.g. a queue's IRQ-enable) using the existing npk_mmio_* fns.
```

## L2539-2540 · `linker.func_wrap("env", "npk_irq_register",`

```
// npk_irq_register(entry) -> LAPIC vector (>=0), or -1. Programs the bound
// device's MSI-X table `entry` to deliver to this driver's core.
```

## L2547-2549 · `linker.func_wrap("env", "npk_irq_arm",`

```
// npk_irq_arm(vector) -> fired-count snapshot, or -1 on a bad vector. Call
// BEFORE submitting/enabling the device work that triggers the IRQ; pass
// the result to npk_irq_wait. Also routes the IRQ to the calling core.
```

## L2556-2558 · `linker.func_wrap("env", "npk_irq_wait",`

```
// npk_irq_wait(vector, since, timeout_ms) -> 1 fired, 0 timeout, -1 bad arg.
// Parks the driver's fiber until the device IRQ advances the fired-count
// past `since`, or `timeout_ms` elapses (defaults to 1000 if <= 0).
```

## L2565-2570 · `linker.func_wrap("env", "npk_mmio_map_bar",`

```
// npk_mmio_map_bar(bar_index, page_count) -> handle or -1
//
// Sizes the BAR first and clamps `pages` to the actual BAR size. This
// prevents drivers from mapping past the end of a BAR into whatever PCI
// address space follows (usually another device's BAR), which would
// silently corrupt that device or generate UR responses.
```

## L2577 · `linker.func_wrap("env", "npk_mmio_read32",`

```
// npk_mmio_read32(handle, offset) -> u32
```

## L2584 · `linker.func_wrap("env", "npk_mmio_write32",`

```
// npk_mmio_write32(handle, offset, value) -> 0 or -1
```

## L2591 · `linker.func_wrap("env", "npk_mmio_read16",`

```
// npk_mmio_read16(handle, offset) -> u16 as i32
```

## L2598-2600 · `linker.func_wrap("env", "npk_mmio_write16",`

```
// npk_mmio_write16(handle, offset, value) -> 0 or -1
// True 16-bit MMIO write — required for split registers like RX/TX BD IDX
// (HOST_IDX[15:0] + HW_IDX[31:16]). A 32-bit RMW would clobber HW_IDX.
```

## L2607-2612 · `linker.func_wrap("env", "npk_mmio_read8",`

```
// npk_mmio_read8(handle, offset) -> u8 as i32
// npk_mmio_write8(handle, offset, value) -> 0 or -1
// Echtes 8-Bit-MMIO. rtw88 (RTL8822CE) fuehrt seine Power-Sequenz als
// read8/write8-Interpreter und meint Register wie REG_SYS_FUNC_EN+1 als
// EINZELNES Byte. Ein 32-Bit-RMW beruehrt drei Nachbarbytes und ist damit
// ein anderer Vorgang — Linux waehlt die Breite absichtlich.
```

## L2625 · `linker.func_wrap("env", "npk_mmio_read64",`

```
// npk_mmio_read64(handle, offset) -> i64
```

## L2632 · `linker.func_wrap("env", "npk_mmio_write64",`

```
// npk_mmio_write64(handle, offset, value) -> 0 or -1
```

## L2639-2643 · `linker.func_wrap("env", "npk_dma_alloc_below",`

```
// npk_dma_alloc_below(page_count, limit_mb) -> handle or -1
// Der Treiber nennt die Obergrenze selbst. `allocate_contiguous_below`
// sucht von oben, also liegt eine 4-GB-Grenze immer direkt unter dem
// PCI-MMIO-Loch — auf AMD-Blech genau dort, wo TSEG/DPR jedes Geraet
// abweist, waehrend die CPU dort ungestoert liest und schreibt.
```

## L2650 · `linker.func_wrap("env", "npk_dma_alloc",`

```
// npk_dma_alloc(page_count) -> handle or -1
```

## L2657 · `linker.func_wrap("env", "npk_dma_phys_addr",`

```
// npk_dma_phys_addr(handle) -> physical address as i64
```

## L2664 · `linker.func_wrap("env", "npk_dma_read",`

```
// npk_dma_read(handle, dma_offset, wasm_ptr, len) -> 0 or -1
```

## L2674 · `linker.func_wrap("env", "npk_dma_write",`

```
// npk_dma_write(handle, dma_offset, wasm_ptr, len) -> 0 or -1
```

## L2684 · `linker.func_wrap("env", "npk_dma_read32",`

```
// npk_dma_read32(handle, offset) -> u32
```

## L2691 · `linker.func_wrap("env", "npk_dma_write32",`

```
// npk_dma_write32(handle, offset, value) -> 0 or -1
```

## L2698 · `linker.func_wrap("env", "npk_memory_fence",`

```
// npk_memory_fence() -> 0
```

## L2705 · `linker.func_wrap("env", "npk_netdev_register",`

```
// npk_netdev_register(mac_ptr) -> 0 or -1
```

## L2715-2721 · `linker.func_wrap("env", "npk_wifi_send_cmd",`

```
// ── WiFi-class control channel (docs/spec/WIFI_CLASS_ABI.md) ───────────────────
// A kernel-mediated mailbox pair routing opaque control messages between
// the vendor driver (wifi_*.wasm) and the supplicant (wifid.wasm). The
// kernel carries bytes only — no WPA / vendor knowledge. Manager side is
// NETCTL-gated (only wifid, which declares it in .npk.caps); driver side
// is gated by being a bound driver (hw state present), like the other
// device host-fns.
```

## L2723 · `linker.func_wrap("env", "npk_wifi_send_cmd",`

```
// npk_wifi_send_cmd(buf_ptr, len) -> 0 / -1 — manager enqueues a command.
```

## L2733 · `linker.func_wrap("env", "npk_wifi_poll_event",`

```
// npk_wifi_poll_event(buf_ptr, max) -> len / -1 — manager dequeues an event.
```

## L2743 · `linker.func_wrap("env", "npk_wifi_poll_cmd",`

```
// npk_wifi_poll_cmd(buf_ptr, max) -> len / -1 — driver dequeues a command.
```

## L2753 · `linker.func_wrap("env", "npk_wifi_send_event",`

```
// npk_wifi_send_event(buf_ptr, len) -> 0 / -1 — driver enqueues an event.
```

## L2763-2766 · `linker.func_wrap("env", "npk_driver_report",`

```
// npk_driver_report(buf_ptr, len) -> 0 / -1 — a bound driver publishes a
// short plain-text status snapshot, read back with the `wlan` intent. The
// kernel stores the bytes and a timestamp and never parses them: what is
// worth reporting is device knowledge, and that stays in the driver.
```

## L2776 · `linker.func_wrap("env", "npk_netdev_submit_rx",`

```
// ── WiFi/NIC data path (driver ↔ kernel IP stack via the netdev mailbox) ─
```

## L2778-2779 · `linker.func_wrap("env", "npk_netdev_submit_rx",`

```
// npk_netdev_submit_rx(buf_ptr, len) -> 0 / -1 — driver hands a received
// Ethernet frame to the kernel network stack.
```

## L2789-2793 · `linker.func_wrap("env", "npk_netdev_rx_deliver",`

```
// npk_netdev_rx_deliver(buf_ptr, len) -> 0 / -1 — driver delivers a received
// frame STRAIGHT into the IP stack from its own fiber (the NAPI topology:
// drain → stack in one context, no relay-ring + Core-0 hop). Falls back to
// the ring internally if Core 0 holds the drain guard. Preferred over
// npk_netdev_submit_rx for the hot path.
```

## L2803-2804 · `linker.func_wrap("env", "npk_netdev_poll_tx",`

```
// npk_netdev_poll_tx(buf_ptr, max) -> len / -1 — driver fetches the next
// frame the kernel wants transmitted (-1 when none / buffer too small).
```

## L2814-2816 · `linker.func_wrap("env", "npk_netdev_set_link",`

```
// npk_netdev_set_link(up) -> 0 — the single-flag form. Kept for drivers
// that know only "usable / not usable"; it reports `up` as carrier with no
// dormant phase.
```

## L2823-2827 · `linker.func_wrap("env", "npk_netdev_set_link_state",`

```
// npk_netdev_set_link_state(carrier, dormant) -> 0 — the RFC 2863 pair
// Linux keeps (`rfc2863_policy`): `carrier` = the association exists,
// `dormant` = it exists but is not usable yet (WPA not done). operstate
// is UP only when carrier && !dormant. One flag for all three meanings
// made every authorization phase look like the link had gone away.
```

## L2837-2838 · `fn cleanup_instance_state(state: &mut HostState) {`

```
/// Free everything an instance leaves behind: its driver's hardware, and any
/// fetch it started and never collected.
```

## L2840 · `crate::drivers::report::clear(&state.module_name);`

```
// A dead driver's snapshot must not read as live numbers.
```

## L2842-2843 · `crate::intent::fetch::release_owner(state.pid);`

```
// A browser closed mid-load would otherwise hold its slots — and its
// megabytes of reserved answer — until the next boot.
```

## L2853-2854 · `crate::wifi::reset();`

```
// The WiFi driver owns the control channel's device side — clear it
// so a re-launch doesn't inherit stale commands/events.
```

## L2864-2908 · `pub(crate) const PRIVATE_ROOT: &str = "priv";`

```
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
/// Das Modul, dem dieser Pfad GEHOERT — oder `None`, wenn er niemandem
/// gehoert (also allen).
///
/// **`priv/<modul>/…` ist der einzige Ort im Speicher, den eine
/// Kapabilitaet nicht aufmacht.** Ueberall sonst gilt: wer READ hat, liest;
/// wer WRITE hat, schreibt. Hier nicht — hier entscheidet der NAME, und den
/// vergibt der Kernel, nicht das Modul. Auch ein Programm mit allen Rechten
/// kommt in einen fremden privaten Bereich nicht hinein.
///
/// **Warum es das braucht.** beak muss seine Kekse ueber einen Neustart
/// retten, sonst ist jede Anmeldung eine Sitzung lang. Ein Keks ist aber
/// keine Datei wie andere: er IST die Anmeldung. In `sys/config/beak`
/// abgelegt haette ihn jede App mit READ lesen koennen — `npk_fetch` prueft
/// die Kapabilitaet und danach jeden Pfad —, und damit waere aus „Kekse
/// bleiben erhalten" ein Weg geworden, alle Sitzungen der Maschine
/// abzugreifen. Das ist keine Datei-Frage, das ist eine Rechte-Frage.
///
/// Die Regel ist bewusst symmetrisch (Lesen, Schreiben, Auflisten, Loeschen,
/// Umbenennen, Kopieren): eine Grenze, die nur eine Richtung kennt, ist
/// keine. Und sie gilt fuer JEDE App, nicht nur fuer beak — spell, tune und
/// loft haben denselben Bedarf.
///
/// Preis, und er ist beabsichtigt: der Dateimanager sieht diese Ordner
/// nicht. Ein privater Bereich, den ein anderes Programm anzeigen kann, ist
/// keiner.
```

## L2912-2916 · `let mut segs = name.split('/').filter(|s| !s.is_empty() && *s != ".");`

```
// **Ueber SEGMENTE, nicht ueber Praefixe.** `privat/x` ist nicht
// `priv/x`, `priv//beak/x` ist es sehr wohl, und das Vergessen des
// Trenners ist die Art, wie diese Sorte Wache ueblicherweise leckt
// (siehe `is_trust_critical_path` daneben). npkFS normalisiert nicht —
// `clean_path` schneidet nur die Raender —, also normalisiert die Wache.
```

## L2919-2922 · `if name.split('/').any(|s| s == "..") { return Some("") }`

```
// Ein `..` unter `priv/` ist kein Versehen. Der Besitzer wird dann zur
// leeren Zeichenkette, und die ist kein Modulname — es kommt also
// NIEMAND hinein, auch der rechtmaessige Besitzer nicht. Die Absage in
// die sichere Richtung.
```

## L2926-2928 · `None => None,`

```
// `priv` allein ist der Ordner ueber allen privaten Bereichen: er
// gehoert niemandem, und Auflisten darf man ihn. Was DARIN steht,
// filtert `npk_fs_list` Eintrag fuer Eintrag.
```

## L2933-2934 · `pub(crate) fn private_area_allows(name: &str, module: &str) -> bool {`

```
/// Darf `module` diesen Pfad anfassen? Fuer alles ausserhalb von `priv/`
/// immer ja — dort entscheiden weiter die Kapabilitaeten.
```

## L2942-2954 · `pub(crate) fn is_own_private(name: &str, module: &str) -> bool {`

```
/// Ist das der EIGENE private Bereich dieses Moduls?
///
/// **Dann braucht es keine Kapabilitaet.** Das ist der Punkt, an dem
/// „Kapabilitaeten, keine Berechtigungen" etwas Konkretes heisst: die
/// Faehigkeit, den eigenen Zustand ueber einen Neustart zu retten, ist NICHT
/// dieselbe wie die Faehigkeit, den Speicher der Maschine zu lesen. beak
/// traegt `RENDER | CANVAS | NET` und soll genau das behalten — ein Browser
/// mit Lese- und Schreibrecht auf alles ist die Sorte Programm, gegen die
/// dieses System gebaut ist. Trotzdem muss er seine Kekse behalten duerfen.
///
/// Der Name ist die Berechtigung, und den Namen vergibt der Kernel: ein
/// Modul kann `priv/<sich selbst>` nicht verlassen und den eines anderen
/// nicht betreten, ganz gleich, was in seinem `.npk.caps` steht.
```

## L2964-2966 · `let certs = crate::tls::certstore::STORE_DIR;`

```
// Prefix match on a path SEGMENT — a plain `starts_with` would also
// catch a sibling like `sys/certsomething`, and missing the trailing
// separator is how this class of guard usually leaks.
```

## L2971-2975 · `fn extract_wasm_custom_section<'a>(wasm: &'a [u8], target: &str) -> Option<&'a [u8]> {`

```
/// Extract a WASM custom section's payload by name, walking the module header.
/// Kernel-side counterpart to the reader that used to live in the SDK's
/// app_catalog — here the whole (possibly multi-MB) module is available, so a
/// section at the tail (like `.npk.app_meta`) is always found regardless of
/// module size.
```

## L2989 · `if section_id != 0 { continue; } // custom sections only`

```
// custom sections only
```

## L3019-3021 · `fn app_is_focused(state: &HostState) -> bool {`

```
/// True if the calling WASM app owns the currently-focused widget window.
/// The clipboard focus-gate: only the focused app may touch the clipboard,
/// so a background app cannot snoop or poison it.
```

## L3032-3034 · `fn append_entry(out: &mut alloc::vec::Vec<u8>, name: &str, size: u64, is_dir: bool, mtime: u64) {`

```
/// Serialize one npk_fs_list entry into `out`.
/// Format: name\0size_le_u64\0is_dir_u8\0mtime_le_u64, entries separated by '\n'.
/// `mtime` is UTC seconds since the Unix epoch — zero means unknown.
```

