# `kernel/src/intent/mod.rs` @ 5e0102684

## L1-4 · `mod auth;`

```
//! Intent Loop
//!
//! Not a shell. Takes intents, not commands.
//! Every intent requires a valid capability token.
```

## L13-14 · `pub(crate) mod reach;`

```
/// Die Reichweitenregel — haengt an nichts und wird deshalb host-seitig
/// gefahren (siehe die Datei selbst).
```

## L35-38 · `static CANCEL_REQUESTED: core::sync::atomic::AtomicBool =`

```
/// Cooperative cancel for long-running foreground intents (downloads, OTA).
/// Set from the keyboard IRQ on Ctrl+C (terminal SIGINT semantics — the loop
/// has no copy concept; widget-app copy, when it exists, gates by focus first),
/// polled by the recv busy-loops. Cleared at the start of each cancellable op.
```

## L41 · `pub fn request_cancel() {`

```
/// IRQ-safe: request cancellation of the running foreground intent.
```

## L45 · `pub fn cancel_requested() -> bool {`

```
/// Has the user pressed Ctrl+C since the last `clear_cancel`?
```

## L49 · `pub fn clear_cancel() {`

```
/// Arm cancellation afresh — call before a cancellable operation.
```

## L54-55 · `const CONFIRM_TIMEOUT_TICKS: u64 = 60 * 100; // 60 s at 100 Hz`

```
/// Give up waiting after this long and answer "no", so a prompt can never
/// wedge the shell — whatever an intent asks, the machine comes back.
```

## L56 · `const CONFIRM_TIMEOUT_TICKS: u64 = 60 * 100; // 60 s at 100 Hz`

```
// 60 s at 100 Hz
```

## L58-64 · `pub fn confirm(question: &str) -> bool {`

```
/// Ask a yes/no question and block until it is answered. Default is NO —
/// Enter, Esc, Ctrl+C and the timeout all decline; only an explicit `y` agrees.
///
/// An intent runs *inside* the loop's read cycle, so there is no session to
/// hand the question back to: it drives the keyboard itself. That means it
/// cannot lean on the machinery the loop normally provides — see the two
/// comments below, both of which cost a hang before they were understood.
```

## L67-70 · `let on_screen = crate::shade::is_active()`

```
// Paint the question, then wait. Deliberately NOT `poll_render` in the
// wait loop: that pumps mouse events too, and a click on a window's X
// would free the session this intent is running inside. Nothing else
// changes on screen while we wait, so one frame is enough.
```

## L83-87 · `crate::xhci::poll_events();`

```
// Drain the USB event ring ourselves instead of relying on the timer
// IRQ: an intent may run in a fiber under the cooperative IF=0
// invariant, where nothing arrives on its own and a bare `hlt` never
// wakes. `poll_events` is the main-loop-context drain and is a no-op
// when the IRQ already did the work.
```

## L91-93 · `let serial = serial::SERIAL.try_lock()?;`

```
// Serial-only mode (no compositor): the answer arrives on COM1.
// `try_lock` — never spin on the lock a print holds; we would be
// the one blocking the writer.
```

## L104-105 · `if crate::keyboard::read_key() == Some(b'[') {`

```
// An arrow key arrives as ESC '[' 'A' — swallow the rest of
// the sequence instead of reading it as "Escape, decline".
```

## L116-119 · `if !crate::smp::fiber::yield_sleep(2) {`

```
// Hand the core to peer fibers if we are one; otherwise spin. NOT
// `hlt`: under IF=0 it would halt the core for good, and the timer
// that should wake it is the very thing that is off. A prompt waits
// on a human, so the spin is bounded by the timeout above.
```

## L129-131 · `const HIST_MAX: usize = 200;`

```
// -- Command history --
/// Lines a session keeps for Up/Down. The persistent log has its own,
/// larger ring; this is only what one window walks.
```

## L135 · `lines: alloc::vec::Vec<String>,`

```
/// Oldest first.
```

## L137 · `cursor: usize,`

```
/// Index into `lines`; `== lines.len()` means "on the fresh line".
```

## L142-143 · `fn new() -> Self {`

```
/// Seeded from the persistent log, so a fresh window (and the first
/// window after a reboot) opens with what was typed before.
```

## L175 · `return None; // back to empty line`

```
// back to empty line
```

## L190 · `pub struct IntentSession {`

```
// ── IntentSession (per-window heap state) ───────────────────
```

## L192-193 · `pub struct IntentSession {`

```
/// Per-window session: input state, command history.
/// Heap-allocated, one per window, owned by Core 0.
```

## L195 · `pub input_buf: [u8; INPUT_BUF_SIZE],`

```
/// Input buffer (what the user is typing).
```

## L197 · `pub pos: usize,`

```
/// Number of valid bytes in input_buf.
```

## L199 · `pub cursor: usize,`

```
/// Cursor position within input (0..=pos).
```

## L201 · `pub history: History,`

```
/// Per-session command history.
```

## L203 · `pub prompt_len: usize,`

```
/// Prompt length (for rewrite_input offset).
```

## L205 · `pub terminal_idx: u8,`

```
/// Which terminal buffer this session owns.
```

## L221 · `pub fn reset_input(&mut self) {`

```
/// Reset input state (after Enter or new prompt).
```

## L228-229 · `static mut SESSIONS: BTreeMap<u8, Box<IntentSession>> = BTreeMap::new();`

```
/// Per-window sessions, indexed by terminal_idx.
// SAFETY: only accessed from Core 0 (event dispatcher owns all session state)
```

## L232 · `fn sessions_ptr() -> *mut BTreeMap<u8, Box<IntentSession>> {`

```
/// Raw pointer access to SESSIONS (avoids static_mut_refs lint).
```

## L237 · `pub fn create_session(terminal_idx: u8) {`

```
/// Create a new session for the given terminal (idempotent).
```

## L239 · `unsafe {`

```
// SAFETY: Core 0 only
```

## L244-247 · `CWDS.lock().entry(terminal_idx).or_insert_with(home_dir);`

```
// New sessions always land in the user's home directory. Inheriting
// from the focused terminal's cwd was confusing — opening a fresh
// window from deep inside a project shouldn't drop you back into
// the same hole.
```

## L251-252 · `pub fn reset_session_prompt(terminal_idx: u8) {`

```
/// Reset session prompt after terminal was freshly allocated (cleared).
/// Forces run_loop to print a fresh prompt with full render.
```

## L254 · `unsafe {`

```
// SAFETY: Core 0 only
```

## L263 · `pub fn destroy_session(terminal_idx: u8) {`

```
/// Destroy the session for the given terminal.
```

## L265 · `unsafe { (*sessions_ptr()).remove(&terminal_idx); }`

```
// SAFETY: Core 0 only
```

## L270 · `fn session_mut(terminal_idx: u8) -> Option<&'static mut IntentSession> {`

```
/// Get a mutable reference to a session. Core 0 only.
```

## L272 · `unsafe { (*sessions_ptr()).get_mut(&terminal_idx).map(|b| &mut **b) }`

```
// SAFETY: Core 0 only, no aliasing (one terminal active at a time)
```

## L276-278 · `fn session_exists(terminal_idx: u8) -> bool {`

```
/// Does a session for `terminal_idx` still exist? Lets the read loop detect
/// that a mouse-driven window close (close_window → destroy_session) freed the
/// session it holds, WITHOUT dereferencing the dangling reference. Core 0 only.
```

## L280 · `unsafe { (*sessions_ptr()).contains_key(&terminal_idx) }`

```
// SAFETY: Core 0 only
```

## L284-285 · `static CWDS: Mutex<BTreeMap<u8, String>> = Mutex::new(BTreeMap::new());`

```
/// Per-terminal CWD (accessible from all cores via Mutex).
/// Separate from IntentSession because workers need read access (resolve_path).
```

## L288 · `fn get_cwd() -> String {`

```
/// Current working directory for the active terminal (or worker redirect).
```

## L294 · `pub fn set_cwd(path: &str) {`

```
/// Set CWD for the current terminal.
```

## L318-319 · `fn current_terminal() -> u8 {`

```
/// Determine which terminal the current core is operating on.
/// Workers: output redirect terminal. Core 0: active terminal.
```

## L328-332 · `use core::sync::atomic::{AtomicBool, AtomicPtr, Ordering as AtOrd};`

```
// ── Intent Job System (dispatch intents to worker cores) ─────
//
// Heavy intents (http, update, install, etc.) are spawned as tasks
// on worker cores. Core 0 returns to the event loop immediately.
// Output is redirected to the intent's terminal via CORE_OUTPUT.
```

## L349 · `const MAX_TERMS: usize = 256;`

```
/// Maximum terminals (must match shade::terminal::MAX_TERMINALS).
```

## L352 · `static INTENT_RUNNING: [AtomicBool; MAX_TERMS] = {`

```
/// Per-terminal flag: true if an intent is running on a worker.
```

## L358 · `static VAULT_REF: AtomicPtr<Mutex<Vault>> = AtomicPtr::new(core::ptr::null_mut());`

```
/// Global vault reference (set once in run_loop, used by workers).
```

## L361 · `pub fn has_running_intent(terminal_idx: u8) -> bool {`

```
/// Check if a terminal has an intent running on a worker.
```

## L368 · `fn spawn_intent_on_worker(input: &str, terminal_idx: u8, session_id: CapId) -> bool {`

```
/// Spawn an intent on a worker core. Returns true if dispatched.
```

## L394 · `fn intent_worker_task(arg: u64) {`

```
/// Worker-core entry: executes an intent, writes output to the terminal.
```

## L413 · `let verb = input.splitn(2, ' ').next().unwrap_or("?");`

```
// Extract verb for process name
```

## L417 · `let pid = crate::process::spawn(verb, crate::process::KIND_INTENT,`

```
// Register in process table
```

## L422 · `crate::shade::terminal::set_output_redirect(job.terminal_idx);`

```
// Redirect kprint output to this terminal
```

## L425 · `let vault_ptr = VAULT_REF.load(AtOrd::Acquire);`

```
// Get vault reference
```

## L428 · `let vault: &'static Mutex<Vault> = unsafe { &*vault_ptr };`

```
// SAFETY: vault_ptr is a &'static Mutex<Vault> set in run_loop
```

## L433 · `let elapsed = crate::interrupts::rdtsc().saturating_sub(start_tsc);`

```
// Track CPU time + deregister process
```

## L438 · `crate::shade::terminal::clear_output_redirect();`

```
// Clear redirect + mark done (Core 0 prints the prompt when it detects completion)
```

## L445 · `fn is_core0_intent(verb: &str) -> bool {`

```
/// Check if an intent should run on Core 0 (needs interactive input or compositor).
```

## L450-452 · `"power" | "watt" | "watts" |`

```
// `power` liest kernLOKALE MSRs (RAPL) und haelt Core 0
// selbst an — auf einem Worker gemessen waere beides
// die falsche Zahl.
```

## L454-459 · `"microvm" | "browser")`

```
// microvm + browser: VMX state (CR4.VMXE,
// IA32_FEATURE_CONTROL lock-bit, TSS, GDT-with-TR-
// slot) is BSP-only — worker cores would VMfail
// with error 8 (invalid host-state) because their
// TR is null. `browser` is just `microvm linux`
// under a user-friendly name, same constraint.
```

## L464 · `pub(crate) fn home_dir() -> String {`

```
/// Get the home directory from config.
```

## L472-475 · `pub(crate) fn resolve_path(name: &str) -> String {`

```
/// Resolve a name relative to cwd.
/// - Absolute (starts with /): strip leading / and use as-is
/// - ".." : go up one level
/// - Relative: prepend cwd
```

## L480 · `let full = if name.starts_with('/') {`

```
// Build full path: absolute (starts with /) or relative (prepend cwd)
```

## L489 · `let mut parts: alloc::vec::Vec<&str> = alloc::vec::Vec::new();`

```
// Normalize: resolve . and .. components
```

## L493 · `"" | "." => {} // skip empty and current-dir`

```
// skip empty and current-dir
```

## L502-504 · `pub(crate) fn parse_ip_pub(s: &str) -> Option<[u8; 4]> { parse_ip(s) }`

```
/// Dieselbe Funktion, fuer die TLS-Schicht sichtbar: `lanpin` muss
/// unterscheiden koennen, ob ein Host eine LITERALE Adresse ist oder ein
/// Name — bei einem Namen gilt die Freigabe nicht.
```

## L517-518 · `pub(crate) fn ensure_parents(path: &str) {`

```
/// Ensure every directory along `path` exists. v2 has real Tree
/// objects so this is just an `mkdir -p`; no `.dir` marker files.
```

## L523-524 · `fn sync_session_to_terminal(session: &IntentSession) {`

```
/// Sync session state to terminal.rs saved input (for cursor restore on focus change).
/// Temporarily switches to the session's terminal to save to the correct slot.
```

## L537-539 · `fn read_line_with_tab(session: &mut IntentSession, vault: &'static Mutex<Vault>,`

```
/// Read a line from serial/keyboard with tab-completion, history, and network polling.
/// All input state lives in the session (input_buf, pos, cursor, esc, history).
/// Returns number of bytes read, or 0 if focus changed / mode switched.
```

## L543-544 · `let term_idx = session.terminal_idx;`

```
// Plain copy of our terminal so we can check the session is still alive
// after re-entrant compositor calls without touching a freed `session`.
```

## L548 · `if crate::shade::is_active() {`

```
// Detect focus change (mouse click, shade action, WASM switch)
```

## L550-554 · `if crate::shade::focused_widget_id().is_some() {`

```
// Phase 10: focus moved to a widget-kind window — return so
// run_loop enters the widget-focused input branch. Without
// this bailout we'd keep consuming keys as shell-line
// history/edit events and never forward them to the
// focused widget app (e.g. drun).
```

## L559-561 · `if crate::shade::focused_surface_id().is_some() {`

```
// Same for a Surface (microvm) window — bail so run_loop
// enters the Surface-focused branch instead of leaving us
// wedged reading input for a window with no session.
```

## L574 · `if ft != session.terminal_idx {`

```
// Focus changed to a different terminal — return so run_loop can switch sessions
```

## L581-588 · `let console_gen = crate::serial::write_gen();`

```
// Poll network while waiting. Snapshot the console-write
// generation around the async pumps: if any of them emitted
// output (guest logs, net, debug-mirror) the user's input
// line is now corrupted/prompt-less, so redraw `path> input`.
// Edge-triggered (only on actual output) and general — not
// microvm-specific. Keyboard echo isn't counted: it happens
// later in this loop, after this check, and the snapshot is
// re-taken at the next iteration's top.
```

## L591-595 · `crate::net::tick_link_and_reconfigure();`

```
// Also while waiting for a line. run_loop calls this at the top of its
// iteration, but this reader blocks until Enter — so a link that comes
// up while the cursor sits at the prompt (WiFi finishing its handshake
// seconds after boot) was noticed only once the user typed something.
// Self-throttled to ~1 Hz, so this costs nothing per keystroke.
```

## L597-602 · `if crate::microvm::vm_active() {`

```
// A running microvm is the foreground task — give it a real
// time budget per outer iteration. One 3 ms slice amortised
// against net::poll + the Shade composite + a 10 ms hlt was
// ~3 ms guest per tens-of-ms host → the ~5 s frame cadence
// (with multi-tens-of-second freezes) that destabilised the
// browser. 8 slices ≈ 24 ms guest, then input/render/spin.
```

## L609-616 · `if !crate::shade::is_active()`

```
// Only redraw when the user has actually typed something:
// async output then split their in-progress command (the
// reported bug). With an empty input there is nothing to
// protect — redrawing an empty `path> ` between every
// streamed line is pure noise (observed during guest boot).
// Serial console only: in shade the live input line is redrawn via
// rewrite_input, and this kprint would (a) duplicate the prompt and
// (b) go to the primary debug sink, not the focused loop.
```

## L625 · `if crate::shade::with_compositor(|comp| comp.tick_animation()).unwrap_or(false) {`

```
// Tick swap animation
```

## L629-630 · `crate::shade::tick_focus_glow();`

```
// This loop idles in `hlt` without ever reaching poll_render, so
// the focus flash needs its tick here too.
```

## L637-641 · `if !session_exists(term_idx) { return None; }`

```
// A click on the window's X button closes it inside handle_mouse
// (close_window → destroy_session), freeing THIS `session`. The
// ShadeAction (Mod+Q) close path below is guarded by sync+return, but
// the mouse path is not — bail before any further deref. run_loop
// re-acquires a valid session on the next pass (None arm).
```

## L648 · `if let Some(action) = crate::shade::input::poll_action() {`

```
// Shade compositor actions (Mod+key)
```

## L669 · `return None; // run_loop creates session for new window`

```
// run_loop creates session for new window
```

## L672-678 · `sync_session_to_terminal(session);`

```
// Both can destroy / invalidate the current session's
// terminal OR move focus to a widget window while we
// hold `&mut session` into the SESSIONS map. Sync
// session state to the terminal buffer first, run
// the action, then bail so run_loop re-acquires the
// (possibly new / absent / widget-focused) session
// cleanly — no dangling refs, no stale focus.
```

## L690-692 · `let event = if let Some(evt) = crate::keyboard::read_event() {`

```
// Read keyboard input as KeyEvent (or serial fallback).
// When shade is active, USB keyboard is the only input — serial is
// skipped entirely to avoid cross-consumption races between sources.
```

## L696-700 · `if !crate::microvm::vm_active() {`

```
// Shade mode — normally idle until the next IRQ. But a
// running microvm is the foreground task: do NOT sleep,
// spin straight back to feed it more slices. hlt here was
// ~one 3 ms slice per timer tick → the ~5 s cadence that
// starved + crashed the browser. Idle (no VM) still hlts.
```

## L709 · `core0_wait();`

```
// Serial mode: the tick wakes us to look again.
```

## L713-715 · `let b = serial.read_serial_raw();`

```
// Use raw serial read — read_byte() has a legacy loop that also
// polls the USB keyboard, which would race with read_event() above
// and cause the two input sources to steal each other's keys.
```

## L718 · `match b {`

```
// Serial: basic byte-to-KeyEvent (no modifier capture)
```

## L728 · `if crate::shade::input::try_keybind_event(&event) {`

```
// Shade keybindings (Mod+key) — consumes the event if matched
```

## L735-738 · `if crate::shade::is_active() {`

```
// Shift+arrow → extend a terminal text selection (mirrors the widget
// editor); Ctrl+Shift+C then copies it. Any other key drops a stale
// selection so the next Shift+arrow re-anchors at the caret. Only in
// shade mode — serial has no selectable grid.
```

## L820-823 · `crate::shade::terminal::set_output_redirect(session.terminal_idx);`

```
// Route the commit newline to THIS loop (like the prompt +
// command output), not the primary debug sink — otherwise
// with active != primary the '\n' lands in the primary loop
// and prompts stack on one line in the focused loop.
```

## L847 · `continue; // skip cursor update below`

```
// skip cursor update below
```

## L870 · `continue; // skip cursor update below`

```
// skip cursor update below
```

## L895 · `continue; // skip cursor update below`

```
// skip cursor update below
```

## L899 · `if crate::shade::is_active() {`

```
// Update cursor position for navigation keys (Up/Down/Left/Right/Home/End)
```

## L909-912 · `fn tab_complete(input: &str) -> Option<String> {`

```
/// Tab-completion: find matching paths for the last word in the input.
///
/// v2: list immediate children of the implied parent directory and
/// filter by the partial leaf name. No more recursive flat-walk.
```

## L917-921 · `let (parent_abs, leaf_prefix): (String, String) = if partial.is_empty() {`

```
// Split `partial` into (parent_dir_to_list, leaf_prefix_to_match).
//   ""        → cwd, no prefix
//   "te"      → cwd, prefix "te"
//   "docs/"   → docs/, no prefix
//   "docs/n"  → docs/, prefix "n"
```

## L938-939 · `let search = if parent_abs.is_empty() {`

```
// Search prefix used by the legacy display logic below: the part
// of the partial path the user has already committed to.
```

## L959 · `let typed_resolved = if partial.is_empty() || partial.ends_with('/') {`

```
// Calculate how much the user already typed as resolved path
```

## L974 · `let common = common_prefix(&matches);`

```
// Multiple matches — try common prefix extension
```

## L980 · `kprint!("\n");`

```
// Show options
```

## L989 · `let cwd = get_cwd();`

```
// Re-print prompt + current input
```

## L1014-1022 · `#[inline]`

```
/// Idle Core 0 until the next interrupt instead of busy-spinning between
/// poll cycles. While a focused app (widget / intent / wasm) runs on a
/// worker, Core 0 only needs to wake to forward input, drive the cursor /
/// dock, and detect completion. The per-core 100 Hz LAPIC timer (v0.186)
/// guarantees a ≤10 ms wake (plain HLT = C1, so the timer keeps ticking —
/// the old "timer stalls in deep C-states" worry was a cpu-pm/MWAIT issue,
/// gone since v0.186), and keyboard / mouse / net IRQs wake it immediately.
/// The old `for 0..5000 { spin_loop() }` cadence pegged Core 0 at 100%
/// whenever any window was focused.
```

## L1025-1033 · `if crate::keyboard::mouse_active_within(40) {`

```
// PS/2 mouse is POLLED (IRQ12 masked on UEFI machines), so HLT-ing until
// the 100 Hz timer caps the pointer sample + cursor-redraw rate at ~100 Hz
// with pipeline latency → laggy mouse whenever a window is focused (the
// run loop reaches here every iteration). While the pointer is actively
// moving, DON'T HLT — return so the loop spins, draining the mouse at its
// full rate + redrawing the cursor promptly. The instant motion stops
// (no PS/2 byte for ~40 ms) we fall through to HLT again → idle power is
// unchanged. No-op on USB/IRQ-mouse hosts (mouse_active_within stays
// false → always HLT, as before).
```

## L1035-1036 · `return;`

```
// Deliberate spin — and it MUST NOT be recorded as a halt. The
// core really is busy here, and the usage figure should say so.
```

## L1039-1040 · `core0_wait();`

```
// Park the shell fiber until input or the next tick (3c-1); the halt
// itself — and its accounting — happens in `per_core::core0_loop`.
```

## L1044-1051 · `fn maybe_idle_gc() {`

```
/// Idle auto-GC trigger, called from the shell run-loop's ~1 Hz top.
/// Reclaims orphans once `GC_PRESSURE_THRESHOLD` mutations have piled up,
/// but ONLY when the system is quiet: FS mounted, no in-flight streaming
/// write (GC would eat its chunks), and no focused microvm surface (the
/// loop is busy slicing the guest — a GC stall there would hitch it).
/// Self-throttled so the gate checks don't run on every spin. GC itself
/// is cheap now (skips Blob bodies) and resets the pressure counter, so
/// after a sweep this won't fire again until real churn rebuilds it.
```

## L1055 · `const CHECK_INTERVAL_TICKS: u64 = 300; // ~3 s @ 100 Hz`

```
// ~3 s @ 100 Hz
```

## L1072-1074 · `if crate::smp::scheduler::worker_count() == 0 {`

```
// The sweep runs on a worker (`docs/plan/CORES_AND_EVENTS.md`, stage 3):
// it walks the whole tree under ROOT_MUTEX with disk I/O, and on Core 0
// it held the shell, the cursor and every frame for as long as it took.
```

## L1088 · `_ => {}`

```
// skipped (a stream raced in) or nothing to reclaim — stay quiet.
```

## L1094-1095 · `static SHELL_WAKER: core::sync::atomic::AtomicU32 =`

```
/// The shell's fiber on Core 0 (stage 3c), signalled by the input
/// interrupts so a key wakes the shell at once instead of on its next tick.
```

## L1099 · `pub fn wake_shell() {`

```
/// Input arrived (called from the i8042 and xHCI interrupt handlers).
```

## L1107-1117 · `fn core0_wait() {`

```
/// Core 0's idle step in the shell loop: park the shell fiber until
/// something wakes it, so other Core-0 fibers run (3c-1).
///
/// **How long (stage 3e):** one frame (10 ms) while anything needs the loop
/// on its own — an animation or the dock (`shade::needs_tick`), a network
/// timer or a polled card (`net::needs_tick`), a held USB key, input that
/// no interrupt reports, serial mode, a cooperative guest. Otherwise one
/// second: the link check and the idle GC run on that. Everything else
/// wakes the shell (`wake_shell`): input, render requests, terminal output,
/// a finished intent, a microVM request. A dependency missed here shows as
/// up to a second of lag, not as a hang.
```

## L1120 · `crate::keyboard::apply_deferred();`

```
// Media keys and unmapped-key reports the PS/2 ISR deferred to here.
```

## L1122 · `if crate::xhci::needs_poll() || crate::xhci::take_missed_drain() {`

```
// Input that no interrupt reports is drained here (the tick used to).
```

## L1135 · `crate::interrupts::halt_until(Some(d), crate::smp::per_core::WAKE_HLT_FALLBACK);`

```
// Not in a fiber (never after boot): halt to the same deadline.
```

## L1141 · `VAULT_REF.store(vault as *const _ as *mut _, AtOrd::Release);`

```
// Store vault reference for worker cores
```

## L1147-1148 · `let mut wasm_esc: u8 = 0;`

```
// Session is created by compositor::create_window (not here).
// Pre-shade: serial-only output, no session needed.
```

## L1150 · `let mut wasm_esc: u8 = 0;`

```
// WASM key routing state (not per-session — persists across focus changes)
```

## L1160-1162 · `crate::net::tick_link_and_reconfigure();`

```
// ~1 Hz (self-throttled): refresh wired carrier + auto-reconfigure IP
// when the active interface changes (LAN cable pulled → WiFi takes over
// with a fresh DHCP lease, or a static config). No manual `dhcp` needed.
```

## L1165-1169 · `maybe_idle_gc();`

```
// Self-throttled: reclaim orphaned objects when enough mutations
// have accumulated and the system is quiet. Keeps an active disk
// from filling with COW/overwrite/delete orphans (npkFS is
// content-addressed → every write orphans the old version). The
// F2FS / `git gc --auto` model.
```

## L1172 · `if crate::shade::is_active() {`

```
// If focused window has a running WASM app or intent, route keys / wait.
```

## L1176-1184 · `if crate::shade::focused_surface_id().is_some() {`

```
// 12.4: Surface-kind (microvm) window focused. It has no
// terminal session (idx 255) — without this branch the
// terminal path below wedges and shade keybinds never run,
// so the window can't be closed (observed: focus the VM
// window → everything frozen). Here: keep Core 0 polling
// (so the guest keeps slicing + the surface keeps
// compositing), let shade keybinds through (Mod+Q closes
// it → VM torn down), swallow other keys for now (Phase B
// forwards them to the guest's virtio-input eventq).
```

## L1186-1194 · `while let Some(evt) = crate::xhci::poll_mouse() {`

```
// Drain the mouse ring HERE, before poll_render() —
// poll_render has its own poll_mouse() loop for the
// host cursor and poll_mouse is a consuming SPSC ring,
// so if poll_render runs first it eats every event and
// the guest never sees a click (host cursor still
// works, which is why the mouse "felt fine"). We call
// handle_mouse ourselves so the host cursor/drag/focus
// logic still runs; poll_render's own loop then finds
// an empty ring (harmless).
```

## L1196-1198 · `crate::shade::handle_mouse(&evt);`

```
// handle_mouse forwards to the guest internally
// (race-free across all poll_mouse consumers) and
// drives the host cursor/drag/focus.
```

## L1203-1211 · `let cooperative = crate::microvm::vm_active();`

```
// Cooperative path (≤2 cores): Core 0 IS the guest's CPU,
// so it must keep slicing — 8 slices (~24 ms guest) per
// composite cycle (net::poll runs inside each slice's pump,
// so L3 stays responsive). Fiber/dedicated path: the guest
// runs on a WORKER core; here vm_poll_slice is just a cheap
// reaper, so Core 0 must NOT spin — that pegged a host core
// at 100% whenever the VM window was focused, even with the
// guest idle (same class as the v0.187.11 focused-window
// spin; the Surface branch was missed then). We idle below.
```

## L1216 · `crate::microvm::vm_poll_slice(); // reaper only`

```
// reaper only
```

## L1225-1230 · `if crate::shade::input::try_keybind_event(&event) {`

```
// Mod+X keybinds (Mod+Q close, focus moves, …)
// still reach shade so the window stays
// manageable. Everything else is translated to
// real Linux evdev keys (US xkb target, Shift
// derived from the char, Ctrl/Alt wrapped) and
// pushed into the guest's virtio-input eventq.
```

## L1236-1241 · `if !cooperative {`

```
// Guest runs on a worker core → idle Core 0 until the next
// IRQ (input, or the 100 Hz timer to re-check for a new
// guest frame) instead of spinning at 100%. Keyboard/mouse
// IRQs wake us to forward input; a deferred guest frame is
// composited within ≤10 ms. Cooperative path keeps spinning
// (it drives the guest).
```

## L1248-1250 · `if let Some(widget_wid) = crate::shade::focused_widget_id() {`

```
// Phase 10: widget-kind window focused — keys go into the
// per-window widget event queue, never the terminal / WASM
// app key buf. The widget app polls them via npk_event_poll.
```

## L1269 · `if crate::shade::input::try_keybind_event(&event) {`

```
// Mod+X keybinds still reach shade (Mod+D, Mod+Q, etc.).
```

## L1273-1277 · `crate::shade::widgets::suppress_hover(widget_wid);`

```
// Keyboard navigation takes visual precedence over
// any stale mouse-hover state. Without this, moving
// the selection with arrows while the cursor sits
// on a different row leaves both rows highlighted.
// Mouse-move re-establishes hover on the next motion.
```

## L1279-1292 · `if event.modifiers.ctrl {`

```
// If a Widget::Input / TextArea is focused, the
// compositor owns text editing — printable /
// Backspace / Delete / arrows / Home / End / Enter
// (and, in a TextArea, Tab → indent) are intercepted,
// mutate the editor buffer, and emit Event::InputChange
// or Event::Action(on_submit). This runs BEFORE the
// Tab focus-nav below so a TextArea can claim Tab.
// Ctrl chords the text editor doesn't own belong to the
// app: Ctrl+S, Ctrl+O, … Routed BEFORE handle_input_key
// because a focused Input/TextArea otherwise swallows
// them whole — its clipboard arm consumes every
// Ctrl+<letter> and drops the ones it has no case for,
// so Ctrl+S used to do nothing at all. A/C/X/V stay with
// the editor (select-all, copy, cut, paste).
```

## L1300-1301 · `if l.is_ascii_graphic() && !matches!(l, b'a' | b'c' | b'x' | b'v') {`

```
// Any printable key, not just letters — zoom
// lives on Ctrl+plus / minus / 0 everywhere.
```

## L1318-1325 · `if crate::shade::widgets::is_clipboard_sink(widget_wid) {`

```
// Ctrl+C / X / V that no focused text widget claimed →
// deliver a semantic clipboard event to a clipboard-sink
// app (loft copies/moves the selected file). Gated on the
// opt-in so non-sink apps never see the new event variant.
// Normalize the same way handle_input_key does: PS/2 maps
// Ctrl+C to control byte 0x03 (produced only when Ctrl was
// held, so no mods check); Ctrl+X/V arrive as the letter
// (or a control byte on xHCI) and need mods.ctrl.
```

## L1346-1349 · `if matches!(event.key, crate::input::KeyCode::Tab) {`

```
// Tab / Shift+Tab move focus between focusable widgets
// when the focused widget didn't consume Tab (i.e. not
// a TextArea). Falls through (key reaches the app) if
// there are no focusable nodes.
```

## L1364-1365 · `core0_idle_tick();`

```
// Idle until the next IRQ instead of busy-spinning — a
// focused widget app otherwise pegged Core 0 at 100%.
```

## L1370 · `if has_running_intent(focused_term) {`

```
// Intent running on worker — event loop without input
```

## L1391-1395 · `core0_idle_tick();`

```
// Idle until the next IRQ (≤10 ms via the per-core 100 Hz
// timer) instead of busy-spinning — we re-poll the worker's
// completion on every wake; 10 ms latency to notice an intent
// finished is imperceptible, and net/mouse/key IRQs wake us
// sooner. Plain HLT (C1) keeps the LAPIC timer ticking.
```

## L1400 · `if crate::wasm::has_wasm_app(focused_term) {`

```
// WASM app running — route keys to app
```

## L1462-1464 · `core0_idle_tick();`

```
// Idle until the next IRQ (timer/input) — same as the
// widget / intent branches. Wakes on key/mouse immediately;
// the 100 Hz timer re-polls app completion within 10 ms.
```

## L1470 · `if from_intent {`

```
// Transition flags — need fresh prompt after WASM/intent completion
```

## L1474 · `if crate::shade::is_active() {`

```
// Flush worker output before printing prompt
```

## L1483 · `kprintln!();`

```
// WASM app exited on this terminal — need fresh prompt
```

## L1487-1488 · `if crate::shade::is_active() {`

```
// If focus switched to a different terminal, don't set need_prompt
// (that terminal's session already has its own prompt state)
```

## L1494-1500 · `if crate::shade::is_active() && !crate::shade::focused_is_terminal() {`

```
// No loop window is focused (desktop, or a non-terminal like the
// dock/a widget has focus): there is no terminal session to drive.
// Without this, keys typed on the bare desktop were read into a
// stale terminal session and EXECUTED (the output only showed on
// serial). Idle here instead — shade keybinds (Mod+Enter to open a
// loop, Mod+D launcher) + mouse still work; plain keys are dropped.
// Guard only applies in shade mode; serial-only mode has no windows.
```

## L1515-1516 · `if crate::shade::input::try_keybind(key) {`

```
// Keybinds (Mod+…) still fire; plain keys have no session to
// land in on the desktop, so they're discarded.
```

## L1527 · `let term = crate::shade::terminal::active_idx();`

```
// Get session for active terminal (create if needed)
```

## L1531 · `need_prompt = true; // new session always needs a prompt`

```
// new session always needs a prompt
```

## L1533 · `let session = unsafe {`

```
// SAFETY: Core 0 only, session exists after create above, no aliasing
```

## L1538 · `if !need_prompt && session.prompt_len == 0 {`

```
// Fresh session (created by compositor) that never had a prompt
```

## L1544 · `let shade_active = crate::shade::is_active();`

```
// Fresh prompt (after command, WASM exit, intent completion, or new session)
```

## L1553-1556 · `crate::shade::terminal::set_output_redirect(session.terminal_idx);`

```
// Route the prompt (and, below, command output) to THIS terminal
// even though the default sink is the primary loop — so prompts +
// results land in the loop the user is typing in, while background
// debug still goes to the primary. See terminal::write.
```

## L1566 · `crate::shade::render_frame();`

```
// New window needs full render to show prompt
```

## L1569 · `crate::shade::render_input_line();`

```
// Existing window — fast input line update only
```

## L1575 · `if crate::shade::is_active() {`

```
// Resuming session after focus change — sync prompt_len + cursor
```

## L1584 · `let line = read_line_with_tab(session, vault, session_id);`

```
// Read input into session
```

## L1592-1593 · `None => continue,`

```
// Focus / window change: run_loop re-acquires the session;
// keep prompt state, do NOT reprint.
```

## L1595-1598 · `Some(0) => { need_prompt = true; continue; }`

```
// Empty Enter: a (blank) line WAS entered. Treat like any
// completed command → fresh prompt next iteration. Without
// this, spamming Enter scrolled the prompt away (the
// focus-change `len==0` path swallowed it).
```

## L1619-1623 · `if crate::shade::is_active() {`

```
// Close this loop's window. Reuse the Mod+Q (CloseWindow) path:
// sync the session to the terminal buffer, close the focused
// window, then `continue` so run_loop re-acquires the now-focused
// terminal's session cleanly — never touching the &mut into
// SESSIONS that CloseWindow just freed ([[project-loop-pid-leak]]).
```

## L1629 · `kprintln!("exit: nothing to close on the serial console");`

```
// Serial console: no window to close.
```

## L1635 · `let verb = input.splitn(2, ' ').next().unwrap_or("");`

```
// Check if this intent can run on a worker core
```

## L1640 · `continue;`

```
// Worker prints prompt when done via from_intent transition
```

## L1645-1647 · `crate::shade::terminal::set_output_redirect(term);`

```
// Core-0 intent output goes to the loop it was typed in (not the
// primary debug sink). Synchronous, so the redirect is safe to clear
// right after — no idle in between where background debug would leak.
```

## L1656 · `let term_idx = crate::shade::terminal::active_idx();`

```
// Don't print prompt if dispatch spawned a WASM app (e.g. top)
```

## L1664-1665 · `fn isin_fixed(x: u16, amp: i64) -> i16 {`

```
/// Integer parabolic sine: `x` is a full-circle phase (0..65536), `amp` the
/// peak amplitude. No FPU use, so it is safe to call from kernel context.
```

## L1667 · `const HALF: u32 = 1 << 15; // 32768 = pi`

```
// 32768 = pi
```

## L1670 · `let prod = (t as u64) * ((HALF - t) as u64); // <= 2^28, peak at t = 16384`

```
// <= 2^28, peak at t = 16384
```

## L1674-1676 · `fn intent_beep(args: &str) {`

```
/// `beep [hz]` — play a short test tone through the audio mailbox → HDA driver
/// → speaker. Proves the app → mailbox → driver → hardware path (M2). Needs
/// `audio_hda` running (autostart) to be audible.
```

## L1687 · `buf.push(s[0]); buf.push(s[1]); // left`

```
// left
```

## L1688 · `buf.push(s[0]); buf.push(s[1]); // right`

```
// right
```

## L1706 · `"status" | "info" => {`

```
// Intents requiring READ
```

## L1733 · `match args.split_whitespace().collect::<alloc::vec::Vec<_>>().as_slice() {`

```
// A/B switches for power measurements (see drivers/sci.rs).
```

## L1743-1745 · `let mut it = args.split_whitespace();`

```
// `dsdt send <ip> <port>` streams the raw table over TCP (exact
// bytes, no terminal-mirror ring-overflow); `dsdt full` base64-
// dumps it to the console; bare `dsdt` dumps battery fields.
```

## L1761-1765 · `let mut it = args.split_whitespace();`

```
// Parse "<ip> <port>" and set the target before spawning debug.wasm.
// No-arg dev shortcut: dial Florian's laptop on the LAN
// (192.168.178.97:22222) so reboots don't need re-typing
// every time. Saves ~10 s per cycle. Remove or move to
// sys/config/debug-target once a config-file path lands.
```

## L1850-1853 · `if require_cap(vault, &session, Rights::EXECUTE, "microvm benchvm") {`

```
// `microvm benchvm [<MB>]` — launch the microvm in pure-bridge
// throughput mode: PID-1 wgets <MB> MiB through the nat bridge
// (no cage/GPU/browser), the local netbench_server reports the
// rate. Isolates the bridge from the browser userspace.
```

## L1859-1864 · `if require_cap(vault, &session, Rights::EXECUTE, "microvm shell") {`

```
// `microvm shell [<line>]` — pre-injects <line> + '\n'
// into the UART RX FIFO before VMLAUNCH. PID-1 in the
// guest detects the pending byte via LSR.DR, drains
// RBR through iopl(3), echoes the line back through
// the same UART, then powers off. End-to-end inject-
// console round-trip (Phase 12.1.4).
```

## L1879-1882 · `if require_cap(vault, &session, Rights::EXECUTE, "browser") {`

```
// User-facing alias for `microvm linux` — launches the
// LibreWolf bundle. `microvm` stays as the dev/test surface
// (substrate self-test + bare-boot diag); `browser` is what
// the user types or what drun spawns.
```

## L2024 · `if args.trim().is_empty() {`

```
// Printing is a read; clearing rewrites a stored object.
```

## L2061-2075 · `"window" => {`

```
// `nic` — die eigenen Zaehler des USB-Netzchips, JEDERZEIT.
//
// **Sie standen nur am Ende eines gelungenen Downloads**, also
// genau dort nicht, wo man sie braucht. `rx_missed` ist der
// FIFO-Ueberlauf des Chips: steht er hoch, kamen die Rahmen an und
// wir haben sie nicht abgeholt; steht er null und `rx_pkts` ist
// klein, kam ueber die Leitung nichts. Das ist die Gabelung, an
// der jede Vermutung ueber einen langsamen Dongle anfaengt.
// `net window <KB>` / `net window auto` — der Fenster-Sweep.
//
// Bei gesaettigter Strecke ist `RTT = Fenster / Rate`. Eine einzelne
// Messung kann deshalb NICHT sagen, ob die Luft oder wir der Deckel
// sind -- beide Zahlen bewegen sich gemeinsam. Die FORM ueber
// mehrere Fenster sagt es: waechst der Durchsatz mit, waren wir es;
// bleibt er stehen und nur die RTT steigt, ist die Luft am Ende.
```

## L2106-2109 · `let (ok, short, other, last, resid) =`

```
// Die Vollzuege des xHCI-Bulk-IN. Sie beantworten die
// Frage, die der Chip-Tally offenlaesst: der Ring ist
// voll, der Chip laeuft nicht ueber — warum meldet der
// Controller trotzdem so wenig fertig?
```

## L2135-2136 · `let a = args.trim();`

```
// `set`/`unset` edit the config object, so they need WRITE — the
// report itself is a read.
```

## L2175-2177 · `kprintln!("[npk] DHCP: requesting a lease...");`

```
// Re-run DHCP on the active NIC (the boot-time configure() ran
// before a WiFi link existed). Brings up IP over `wlan` once the
// 4-way completed and it's the active interface.
```

## L2179-2181 · `if crate::net::dhcp::run_blocking(5000) {`

```
// Typed by hand, so waiting for the answer is what the user
// asked for — unlike the ~1 Hz link tick, which must never take
// the terminal away. Same state machine either way.
```

## L2185-2186 · `kprintln!("[npk] DHCP: still trying — run net in a moment");`

```
// Out of patience, not out of exchange: it keeps going on
// the link tick, so saying "failed" here would be a lie.
```

## L2271-2273 · `"cert" | "certs" => {`

```
// Reading the trust store is harmless; changing it is not. WRITE
// gates the whole intent rather than just `add`/`remove`, because
// the listing is also the thing that tells you what to remove.
```

## L2330 · `crate::shade::terminal::clear();`

```
// Shade mode: clear terminal buffer and re-render focused window
```

## L2336 · `let serial = crate::serial::SERIAL.lock();`

```
// ANSI clear to serial
```

## L2343 · `"help" | "?" => system::intent_help_topic(args.trim()),`

```
// Unrestricted intents (informational)
```

## L2351-2356 · `if crate::npkfs::exists(&alloc::format!("sys/wasm/{}", verb)) {`

```
// Implicit-run: if `<cmd>` matches a WASM module under
// sys/wasm/, execute it with `args`. Makes any installed
// app callable by name, same UX as built-in intents.
// Hardcoded dispatcher entries above (top/wallpaper/...)
// still win for apps that need special run semantics
// (interactive, background, etc.).
```

## L2369-2374 · `fn microvm_linux_info() {`

```
/// Check capability before executing an intent. Returns true if allowed.
/// `microvm linux-info` handler — fetch the bundled bzImage from
/// npkFS, parse the Linux Boot Protocol setup-header, print stats.
/// Read-only; no VM activity. The asset lands in npkFS at
/// `sys/microvm/linux-virt.bzImage` on fresh install (see
/// install_data/assets).
```

## L2397-2398 · `let version = header.version;`

```
// Reads of #[repr(packed)] fields go via local copies to avoid
// unaligned-reference UB.
```

## L2432-2448 · `pub fn launch_browser() {`

```
/// `microvm linux` / `microvm shell` handler — fetch the bundled
/// bzImage from npkFS, hand it + a cmdline + `inject` bytes to
/// vmx::run_linux. The kernel writes its earlyprintk to serial 0x3F8,
/// which we trap via I/O bitmap and reflect as `[guest] <line>`
/// kprintln output. `inject` is empty for the plain `linux`
/// subcommand (idle-pause behavior); for `shell <line>` it's the
/// line + '\n' pre-loaded into the UART RX FIFO so PID-1 can echo
/// it back (Phase 12.1.4).
/// Public entry the WASM host-fn `npk_run_intent("browser")` calls so
/// drun (or any future launcher) can spawn the LibreWolf microvm
/// without going through the Core-0 shell prompt. Safe from a worker
/// core when the dedicated-VM-core path is active — `vm_open` just
/// stashes a PENDING_VM request (pure atomic + mutex); the actual
/// VMXON / VMRUN happens later on the dedicated core via
/// `vm_core_serve`. On the cooperative path (≤ 2 cores), `vm_open`
/// requires BSP state and will fail from a worker → host-fn returns
/// -1; caller must type `browser` at a Core-0 prompt instead.
```

## L2456-2464 · `const USERSPACE_PATH: &str = "sys/microvm/userspace.cpio.gz";`

```
/// Optional userspace bundle — Alpine minirootfs + busybox + (future)
/// Wayland/Mesa/LibreWolf. Built by `microvm-userspace/build.sh`,
/// distributed via OTA (`release/assets/microvm-userspace.cpio.gz`
/// for small bundles on raw.githubusercontent, or a GitHub Releases
/// asset with `url=` override in the asset manifest for the
/// Mesa-class >100 MB bundles). If present, we use it INSTEAD of
/// the minimal PID-1-only initramfs — the bundle contains our PID-1
/// at /init too, so all the existing substrate tests still run,
/// and PID-1 then exec's /bin/sh from the bundle's busybox.
```

## L2466-2521 · `use core::fmt::Write;`

```
// Linux 32-bit boot protocol cmdline.
//
// `earlycon=uart8250,io,0x3f8,115200n8`: activate a simple
// early-boot console that writes directly to legacy COM1 at
// port 0x3F8 — no UART detection, no driver init. We checked
// Alpine's vmlinuz-virt config: CONFIG_EARLY_PRINTK is NOT
// set (so `earlyprintk=` is silently ignored), but
// CONFIG_SERIAL_EARLYCON=y IS set. earlycon is what we
// want — it bypasses the 8250 detection probe (which fails
// against our minimal UART emulation) and just dumps bytes.
//
// `console=ttyS0,115200`: registers the regular 8250 driver as
// primary console once full kernel init runs. May or may not
// succeed depending on whether the 8250 detection passes.
//
// `panic=1`: halt immediately on any panic (no reboot loop).
// `nokaslr`: predictable load addresses for our hypervisor side.
// `noapic acpi=off tsc=reliable`: tell Linux to skip hardware
// probing it would otherwise crash on — no ACPI tables, no IO-APIC
// (device IRQs go through the 8259 PIC). Without these Linux times
// out / panics on probes that don't behave like real silicon.
// `nolapic` was DROPPED (guest-SMP Stage 1): the local APIC is now
// trap-and-emulated (svm::lapic, NPT-faulting page @ 0xFEE00000), so
// Linux brings it up + uses the LAPIC timer as its clockevent. This
// is the prerequisite for guest SMP (AP bringup is LAPIC INIT-SIPI).
//
// PCI is now ON (12.2 step 1) — the legacy 0xCF8/0xCFC config-
// space path is emulated in `microvm::devices::pci_bus`. Linux
// enumerates bus 0 and finds an i440FX-style host bridge plus a
// virtio-blk-pci device at slot 1. BAR MMIO + IRQ delivery come
// in 12.2.2 — for now Linux probes, finds the device, can't talk
// to its BAR yet, and shelves it.
// `tsc_early_khz=<host TSC kHz>` skips Linux's PIT-based TSC
// calibration, which deadlocks on the AMD-V backend (host CPUID
// 0x15 absent on AMD → Linux falls back to PIT calibration → our
// PIT IO emulation returns 0 → Linux loops forever waiting for
// ticks). Harmless on Intel: there CPUID 0x15 advertises the freq
// and Linux uses it directly, ignoring the cmdline hint.
// It MUST be the REAL host TSC frequency, not a hardcoded 2 GHz:
// the guest's TSC is the pass-through host TSC (~4.7 GHz on the
// 9600X). With a wrong (too-low) hint Linux computes elapsed
// time = cycles / freq with too-small freq → the guest clock
// (clocksource=tsc, sched_clock, hrtimers, clock_gettime) runs
// ~2.3× too fast while jiffies (IRQ0) stays real-time. Benign
// for a slow/short cooperative guest; FATAL on the dedicated
// core where the guest runs continuously near-native — the
// TSC↔jiffies skew accumulates fast and breaks librewolf's
// pthread/futex/cond-timedwait startup → deterministic crash in
// musl __restore_sigs at ~6 s. `tsc_freq()` is the real
// calibrated value (its 2 GHz default is itself a safe fallback).
// The guest has no RTC — its clock boots at the year 2000. Every
// HTTPS site's cert is then "not yet valid" → Firefox shows "your
// computer clock is wrong" and the load fails/crashes. Pass the
// host's real wall-clock epoch on the cmdline; PID-1's
// launch_wayland reads `nopeektime=` from /proc/cmdline and
// `date -s @<epoch>` before cage so TLS cert validation passes.
```

## L2525-2541 · `let maxcpus: u8 = if crate::microvm::cpu::smp_ap_active() {`

```
// DIAG: `quiet loglevel=3` temporarily removed — bare-metal NUC
// reports the browser hanging and we can't tell whether it's the
// guest kernel failing to boot or something later. Putting boot
// verbose back so any `[guest]` panic / wedge during early Linux
// init surfaces on the host console. Re-add the gating once
// bare-metal is validated. Performance regression on launch is
// ~150 8250-IO VM-exits, one-shot, acceptable for diag.
// Guest-SMP maxcpus. Stage 2 (GUEST_SMP, GUEST_SMP_AP off): keep
// `maxcpus=1` — the MP-table makes Linux ENUMERATE GUEST_VCPUS CPUs
// (CPU1 present-but-offline) but `maxcpus=1` stops it from ONLINING the
// AP, because an enumerated-but-never-responding AP HANGS the cpuhp
// bring-up. Stage 3b-2 (GUEST_SMP_AP on): raise to GUEST_VCPUS so Linux
// actually brings the AP up — by then INIT-SIPI spawns a responding AP
// vCPU (`request_ap_spawn`), so the wait completes instead of hanging.
// Online the AP only where the host can actually bring it up (AMD/SVM).
// Both backends now have an AP-vCPU path; `smp_ap_active()` gates it.
// The count scales with the host's worker cores (`guest_vcpus()`).
```

## L2554-2558 · `if !crate::microvm::cpu::guest_lapic_active() {`

```
// `nolapic` unless the LAPIC is actually emulated for this host. LAPIC
// emulation is SVM-only (svm/lapic.rs); on Intel/VMX there is none, so the
// guest must fall back to the PIT IRQ0 the VMX path injects — otherwise it
// programs the LAPIC timer and hangs waiting for a tick that never comes
// (cage/Wayland never starts). See cpu::guest_lapic_active().
```

## L2562-2563 · `if !crate::microvm::cpu::guest_ioapic_active() {`

```
// The I/O APIC is in the MP table only when it is emulated; otherwise the
// guest stays on the 8259.
```

## L2572-2587 · `if crate::config::get("microvm_gdiag")`

```
// Guest-side diagnostic probe: PID-1 forks a 1 s busybox loop that dumps the
// GUEST's own view (per-vCPU busy/softirq %, socket cwnd/rtt/retrans, softnet
// drops/squeeze) to /dev/kmsg → our console as `[gdiag]`.
//
// OFF by default since 0.294.0. It was on for everyone, and it is not free:
// the guest writes it through the emulated 8250, which is ONE VM-EXIT PER
// BYTE, and each finished line leaves as a blocking UART write from inside
// that exit handler — on the BSP vCPU fiber, the same one that services the
// virtio-net doorbell. The softnet line alone is ~700 bytes a second, so the
// probe cost about 700 extra exits and ~60 ms of UART spin per second,
// forever, on the network's own critical path. A measuring tool that slows
// the thing it measures is the third time we have paid for this.
//
//     set microvm_gdiag on
//
// turns it back on for a session that actually wants the inside view.
```

## L2593-2594 · `if let Some(mb) = bench_mb {`

```
// Diagnostic pure-bridge throughput run: PID-1 sees `nopeekbench=` and runs
// a busybox wget loop through the nat bridge instead of cage/browser.
```

## L2596-2615 · `const BENCH_FALLBACK: [u8; 4] = [192, 168, 178, 97];`

```
// `nohz=off` EXONERATED (v0.226.66 HW): forced 1000→2008 Hz effective tick
// but throughput stayed a lottery and gap_max grew to 40 ms (the extra
// timer-injects added host contention). So delayed-ACK-on-tick is NOT the
// root — reverted. `nopeekbenchhost=<gw>` lets PID-1 target whatever server
// sits at our host-NIC gateway, so the SAME bench works on slirp (10.0.2.2)
// and tap/vhost (e.g. 172.30.0.1) without a rebuild — that is the tap test
// that bypasses the single-threaded slirp ceiling (build.sh QEMU_NET=tap).
// The gateway default is a QEMU habit: on slirp the bench server sits at
// 10.0.2.2, which IS the gateway, and on tap it is 172.30.0.1. On real
// hardware the gateway is the ROUTER, and the bench would wget a box that
// has never heard of it — a failure that looks exactly like the bridge
// being broken and is not. Name the server instead:
//
//     store sys/config/benchhost 192.168.178.97
//
// Falls back to the gateway, so every QEMU invocation keeps working.
// Hardcoded fallback, on request, until the bridge is fixed: the gateway
// default is a QEMU habit (slirp's server IS the gateway) and on real
// hardware it points the bench at the ROUTER, which fails for a reason
// that has nothing to do with what we are measuring. TEMPORARY.
```

## L2640-2654 · `const USERSPACE_SQFS_PATH: &str = "sys/microvm/userspace.sqfs";`

```
// Userspace-bundle delivery, newest path first:
//
//  1. squashfs bundle present → boot the TINY PID-1-only
//     initramfs. That PID-1 mounts the .sqfs from /dev/vdb (the
//     slot-5 read-only virtio-blk device loads it straight from
//     `sys/microvm/userspace.sqfs` — never unpacked into RAM) and
//     chroots into it. RAM-efficient: the bundle stays compressed
//     on the virtual disk, decompressed on read.
//
//  2. legacy cpio bundle present (no sqfs) → use it AS the
//     initramfs (embeds our PID-1 at /init). RAM-heavy: the whole
//     tree is unpacked into tmpfs. Kept for back-compat until the
//     sqfs bundle is the only shipped form.
//
//  3. neither → minimal PID-1-only substrate.
```

## L2697-2703 · `match crate::microvm::vm_open(&bytes, cmdline, initramfs.as_deref(), inject) {`

```
// 12.4 step 1b: non-blocking. Open the VM (synchronous one-time
// substrate + guest-image setup, copies the bzImage into guest
// RAM so `bytes` can drop here), register it, return. The Core-0
// event loop then drives bounded slices via `vm_poll_slice`,
// rendering Shade between them — instead of this call blocking
// Core 0 until guest exit. Guest-exit logging happens in
// vm_poll_slice when the slice that observes the exit completes.
```

## L2706-2712 · `match crate::shade::create_surface_window("microvm") {`

```
// 12.4 step A3: give the guest its own tiled Surface
// window (tiling invariant — never fullscreen). virtio-gpu
// FLUSH renders into it; shade composites it like any
// window; the teardown path closes it on guest exit /
// window close. If the compositor isn't up (serial-only
// boot) we stay unbound and virtio-gpu falls back to the
// legacy fullscreen blit.
```

## L2716-2724 · `crate::shade::focus_window(wid);`

```
// Focus the guest window on launch. forward_pointer
// _to_guest and the keyboard path only run in the
// Surface-focused branch, so without this the guest
// gets no input until the user manually focuses the
// tile (the "only works after I touch the keyboard"
// symptom). This is the productised D1' behaviour:
// launching the guest gives it focus, like any app
// window. The spawning shell is reachable again via
// Mod-focus / Mod+number.
```

## L2757-2760 · `crate::shade::force_redraw();`

```
// Full redraw (not just request_render) so terminal windows
// repaint with the new Surface/OnSurface colors too — they
// resolve the theme at render time and the input-line cache
// must be invalidated.
```

## L2794 · `let resolved = resolve_path(target);`

```
// Resolve path and verify it exists as a directory
```

## L2797 · `if resolved.is_empty() {`

```
// Root always exists
```

## L2812 · `pub use wallpaper::apply_startup_wallpaper;`

```
/// Re-export public API for main.rs
```

## L2816-2823 · `pub fn setup_home() {`

```
/// Create initial directory structure and set cwd to home.
///
/// Lays down the canonical user-tree on first boot so loft's sidebar
/// (`Home / Documents / Downloads / Pictures / Projects / Trash`) and
/// the wallpaper subsystem land on real `.dir`-marker-backed
/// directories instead of phantom paths. Each `ensure_parents` call
/// is idempotent — re-running setup_home on an already-populated home
/// is a no-op (no duplicate writes, no journal churn).
```

## L2827-2830 · `for sub in &["documents", "downloads", "pictures", "pictures/wallpapers", "projects", ".trash"] {`

```
// Sidebar-aligned standard subdirs. The wallpapers dir lives
// under `pictures/` to match `wallpaper_dir()` in
// `intent::wallpaper`; the previous flat `wallpapers/` was a
// dead path nothing read or wrote.
```

## L2837 · `pub fn get_cwd_for_shell() -> String {`

```
/// Expose CWD for npk-shell.
```

## L2842 · `pub fn print_active_history() {`

```
/// Print the active terminal's command history.
```

## L2845 · `let session = unsafe { (*sessions_ptr()).get(&term) };`

```
// SAFETY: Core 0 only (history is a Core 0-only intent)
```

## L2860 · `pub fn clear_all_history() {`

```
/// `history clear` — drop the stored log and every window's ring.
```

## L2863 · `unsafe {`

```
// SAFETY: Core 0 only (intents run on Core 0)
```

## L2872 · `#[allow(dead_code)]`

```
/// Execute an intent from remote shell (dispatch without the loop).
```

