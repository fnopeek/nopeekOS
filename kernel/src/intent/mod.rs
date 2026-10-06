//! Intent Loop
//!
//! Not a shell. Takes intents, not commands.
//! Every intent requires a valid capability token.

mod auth;
mod cert;
pub(crate) mod fetch;
mod forge;
mod fs;
pub(crate) mod gzip;
pub mod history;
/// Reachability rule; it depends on nothing, so it is also tested on the host.
pub(crate) mod reach;
pub(crate) mod http;
pub(crate) mod http2;
mod net;
pub mod system;
mod update;
pub mod install;
mod python;
mod wallpaper;
mod wasm;

use crate::capability::{CapId, Vault, Rights};
use crate::{kprint, kprintln, serial};
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use spin::Mutex;

const INPUT_BUF_SIZE: usize = 512;

/// Cooperative cancel for long-running foreground intents (downloads, OTA).
/// Set from the keyboard IRQ on Ctrl+C (terminal SIGINT semantics — the loop
/// has no copy concept; widget-app copy, when it exists, gates by focus first),
/// polled by the recv busy-loops. Cleared at the start of each cancellable op.
static CANCEL_REQUESTED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);
/// IRQ-safe: request cancellation of the running foreground intent.
pub fn request_cancel() {
    CANCEL_REQUESTED.store(true, core::sync::atomic::Ordering::Release);
}
/// Has the user pressed Ctrl+C since the last `clear_cancel`?
pub fn cancel_requested() -> bool {
    CANCEL_REQUESTED.load(core::sync::atomic::Ordering::Acquire)
}
/// Arm cancellation afresh — call before a cancellable operation.
pub fn clear_cancel() {
    CANCEL_REQUESTED.store(false, core::sync::atomic::Ordering::Release);
}

/// Give up waiting after this long and answer "no", so a prompt can never
/// wedge the shell — whatever an intent asks, the machine comes back.
const CONFIRM_TIMEOUT_TICKS: u64 = 60 * 100; // 60 s at 100 Hz

/// Ask a yes/no question and block until it is answered. Default is no:
/// Enter, Esc, Ctrl+C and the timeout all decline; only an explicit `y` agrees.
///
/// An intent runs inside the loop's read cycle, so there is no session to
/// hand the question back to: it drives the keyboard itself and cannot lean
/// on the loop's machinery (see the two comments below).
/// Prompts (`confirm`, `read_secret`) waiting for a key right now. While an
/// intent runs on a worker, the shell loop on Core 0 throws typed keys away;
/// it must not do that to the answer the intent is waiting for.
static PROMPTS_WAITING: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// Held while a prompt waits; counts it in `PROMPTS_WAITING`.
struct PromptGuard;

impl PromptGuard {
    fn new() -> Self {
        PROMPTS_WAITING.fetch_add(1, core::sync::atomic::Ordering::AcqRel);
        PromptGuard
    }
}

impl Drop for PromptGuard {
    fn drop(&mut self) {
        PROMPTS_WAITING.fetch_sub(1, core::sync::atomic::Ordering::AcqRel);
    }
}

pub fn confirm(question: &str) -> bool {
    let _waiting = PromptGuard::new();
    kprint!("[npk] {} [y/N] ", question);
    // Paint the question, then wait. Deliberately not `poll_render` in the
    // wait loop: that pumps mouse events too, and a click on a window's X
    // would free the session this intent is running inside. Nothing else
    // changes on screen while we wait, so one frame is enough.
    let on_screen = crate::shade::is_active()
        && crate::smp::per_core::current_core_id() == 0;
    if on_screen { crate::shade::render_frame(); }

    let started = crate::interrupts::ticks();
    let answer = loop {
        if cancel_requested() { kprint!("^C"); break false; }
        if crate::interrupts::ticks().wrapping_sub(started) > CONFIRM_TIMEOUT_TICKS {
            kprint!("(timeout)");
            break false;
        }

        // Drain the USB event ring ourselves instead of relying on the timer
        // IRQ: an intent may run in a fiber under the cooperative IF=0
        // invariant, where nothing arrives on its own and a bare `hlt` never
        // wakes. `poll_events` is the main-loop-context drain and is a no-op
        // when the IRQ already did the work.
        crate::xhci::poll_events();

        let key = crate::keyboard::read_key().or_else(|| {
            // Serial-only mode (no compositor): the answer arrives on COM1.
            // `try_lock` — never spin on the lock a print holds; we would be
            // the one blocking the writer.
            let serial = serial::SERIAL.try_lock()?;
            if serial.has_data() { Some(serial.read_serial_raw()) } else { None }
        });
        if let Some(key) = key {
            match key {
                b'y' | b'Y' => { kprint!("y"); break true; }
                b'n' | b'N' => { kprint!("n"); break false; }
                b'\n' | b'\r' => break false,
                0x03 => { kprint!("^C"); break false; }
                0x1B => {
                    // An arrow key arrives as ESC '[' 'A' — swallow the rest of
                    // the sequence instead of reading it as "Escape, decline".
                    if crate::keyboard::read_key() == Some(b'[') {
                        let _ = crate::keyboard::read_key();
                        continue;
                    }
                    break false;
                }
                _ => {}
            }
        }

        // Hand the core to peer fibers if we are one; otherwise spin. Not
        // `hlt`: under IF=0 it would halt the core for good, because the timer
        // that should wake it is off. A prompt waits on a human, so the spin is
        // bounded by the timeout above.
        if !crate::smp::fiber::yield_sleep(2) {
            core::hint::spin_loop();
        }
    };
    kprintln!();
    if on_screen { crate::shade::render_frame(); }
    answer
}

/// Read a masked line (a passphrase) into `buf`, echoing `*`. Same input
/// path as `confirm`: the keyboard in the loop, COM1 without a compositor.
/// Printable ASCII only, like the login screen — a character the login
/// cannot type must not end up in a passphrase. None on Ctrl+C or timeout.
pub fn read_secret(prompt: &str, buf: &mut [u8]) -> Option<usize> {
    // A new question: a Ctrl+C from an earlier prompt must not answer it.
    // Nothing else clears the flag for an intent on Core 0.
    clear_cancel();
    let _waiting = PromptGuard::new();
    kprint!("[npk] {}", prompt);
    let on_screen = crate::shade::is_active()
        && crate::smp::per_core::current_core_id() == 0;
    if on_screen { crate::shade::render_frame(); }

    let mut len = 0usize;
    let started = crate::interrupts::ticks();
    let result = loop {
        if cancel_requested() { kprint!("^C"); break None; }
        if crate::interrupts::ticks().wrapping_sub(started) > CONFIRM_TIMEOUT_TICKS {
            kprint!("(timeout)");
            break None;
        }
        crate::xhci::poll_events();
        let key = crate::keyboard::read_key().or_else(|| {
            let serial = serial::SERIAL.try_lock()?;
            if serial.has_data() { Some(serial.read_serial_raw()) } else { None }
        });
        if let Some(key) = key {
            match key {
                b'\n' | b'\r' => break Some(len),
                0x03 => { kprint!("^C"); break None; }
                0x08 | 0x7F => {
                    if len > 0 {
                        len -= 1;
                        buf[len] = 0;
                        kprint!("\x08 \x08");
                    }
                }
                0x1B => {
                    // An arrow key arrives as ESC '[' x: swallow it.
                    if crate::keyboard::read_key() == Some(b'[') {
                        let _ = crate::keyboard::read_key();
                    }
                }
                b if (0x20..0x7F).contains(&b) && len < buf.len() => {
                    buf[len] = b;
                    len += 1;
                    kprint!("*");
                }
                _ => {}
            }
            if on_screen { crate::shade::render_frame(); }
            continue;
        }
        if !crate::smp::fiber::yield_sleep(2) {
            core::hint::spin_loop();
        }
    };
    kprintln!();
    if on_screen { crate::shade::render_frame(); }
    if result.is_none() { buf.fill(0); }
    result
}

// -- Command history --
/// Lines a session keeps for Up/Down. The persistent log has its own,
/// larger ring; this is only what one window walks.
const HIST_MAX: usize = 200;

pub(crate) struct History {
    /// Oldest first.
    lines: alloc::vec::Vec<String>,
    /// Index into `lines`; `== lines.len()` means "on the fresh line".
    cursor: usize,
}

impl History {
    /// Seeded from the persistent log, so a fresh window (and the first
    /// window after a reboot) opens with what was typed before.
    fn new() -> Self {
        let mut lines = history::snapshot();
        if lines.len() > HIST_MAX {
            let drop = lines.len() - HIST_MAX;
            lines.drain(..drop);
        }
        let cursor = lines.len();
        History { lines, cursor }
    }

    fn push(&mut self, line: &[u8]) {
        let Ok(text) = core::str::from_utf8(line) else { return };
        if text.is_empty() { return; }
        if self.lines.last().map(|l| l.as_str()) != Some(text) {
            self.lines.push(String::from(text));
            if self.lines.len() > HIST_MAX { self.lines.remove(0); }
        }
        self.cursor = self.lines.len();
        history::push(text);
    }

    fn up(&mut self) -> Option<&str> {
        if self.cursor == 0 { return None; }
        self.cursor -= 1;
        self.lines.get(self.cursor).map(|l| l.as_str())
    }

    fn down(&mut self) -> Option<&str> {
        if self.cursor >= self.lines.len() { return None; }
        self.cursor += 1;
        if self.cursor >= self.lines.len() {
            return None; // back to empty line
        }
        self.lines.get(self.cursor).map(|l| l.as_str())
    }

    fn reset_cursor(&mut self) {
        self.cursor = self.lines.len();
    }

    fn clear(&mut self) {
        self.lines.clear();
        self.cursor = 0;
    }
}

// ── IntentSession (per-window heap state) ───────────────────

/// Per-window session: input state, command history.
/// Heap-allocated, one per window, owned by Core 0.
pub struct IntentSession {
    /// Input buffer (what the user is typing).
    pub input_buf: [u8; INPUT_BUF_SIZE],
    /// Number of valid bytes in input_buf.
    pub pos: usize,
    /// Cursor position within input (0..=pos).
    pub cursor: usize,
    /// Per-session command history.
    pub history: History,
    /// Prompt length (for rewrite_input offset).
    pub prompt_len: usize,
    /// Which terminal buffer this session owns.
    pub terminal_idx: u8,
}

impl IntentSession {
    pub fn new(terminal_idx: u8) -> Self {
        Self {
            input_buf: [0u8; INPUT_BUF_SIZE],
            pos: 0,
            cursor: 0,
            history: History::new(),
            prompt_len: 0,
            terminal_idx,
        }
    }

    /// Reset input state (after Enter or new prompt).
    pub fn reset_input(&mut self) {
        self.pos = 0;
        self.cursor = 0;
    }
}

/// Per-window sessions, indexed by terminal_idx. Changed only on Core 0:
/// the shell loop holds a `&mut` into one session across its read loop, so a
/// session must not be written or freed under it from another core. Other
/// cores queue the change (`SESSION_OPS`) and Core 0 applies it at a point
/// where it holds no session.
static SESSIONS: Mutex<BTreeMap<u8, Box<IntentSession>>> = Mutex::new(BTreeMap::new());

#[derive(Clone, Copy)]
enum SessionOp {
    Create,
    ResetPrompt,
    Destroy,
    /// Every session's history; the index is ignored.
    ClearHistory,
}

/// Session changes requested off Core 0, applied by `apply_session_ops`.
static SESSION_OPS: Mutex<alloc::vec::Vec<(u8, SessionOp)>> = Mutex::new(alloc::vec::Vec::new());

fn on_core0() -> bool {
    crate::smp::per_core::current_core_id() == 0
}

fn session_op(terminal_idx: u8, op: SessionOp) {
    if on_core0() {
        apply_session_op(terminal_idx, op);
    } else {
        SESSION_OPS.lock().push((terminal_idx, op));
        wake_shell();
    }
}

fn apply_session_op(terminal_idx: u8, op: SessionOp) {
    match op {
        SessionOp::Create => {
            SESSIONS.lock().entry(terminal_idx)
                .or_insert_with(|| Box::new(IntentSession::new(terminal_idx)));
            // New sessions start in the user's home directory, not the focused terminal's cwd.
            CWDS.lock().entry(terminal_idx).or_insert_with(home_dir);
        }
        SessionOp::ResetPrompt => {
            if let Some(s) = SESSIONS.lock().get_mut(&terminal_idx) {
                s.prompt_len = 0;
                s.reset_input();
            }
        }
        SessionOp::Destroy => {
            SESSIONS.lock().remove(&terminal_idx);
            CWDS.lock().remove(&terminal_idx);
        }
        SessionOp::ClearHistory => {
            for s in SESSIONS.lock().values_mut() {
                s.history.clear();
            }
        }
    }
}

/// Apply the session changes other cores queued. Core 0, holding no session.
fn apply_session_ops() {
    let ops = core::mem::take(&mut *SESSION_OPS.lock());
    for (idx, op) in ops {
        apply_session_op(idx, op);
    }
}

/// Create a new session for the given terminal (idempotent).
pub fn create_session(terminal_idx: u8) {
    session_op(terminal_idx, SessionOp::Create);
}

/// Reset session prompt after terminal was freshly allocated (cleared).
/// Forces run_loop to print a fresh prompt with full render.
pub fn reset_session_prompt(terminal_idx: u8) {
    session_op(terminal_idx, SessionOp::ResetPrompt);
}

/// Destroy the session for the given terminal.
pub fn destroy_session(terminal_idx: u8) {
    session_op(terminal_idx, SessionOp::Destroy);
}

/// The session for `terminal_idx`. Core 0 only: sessions are freed only
/// there, so the box outlives the reference until Core 0 itself destroys it
/// (which `session_exists` then reports).
fn session_mut(terminal_idx: u8) -> Option<&'static mut IntentSession> {
    let p: *mut IntentSession = SESSIONS.lock().get_mut(&terminal_idx)?.as_mut();
    // SAFETY: the box is heap memory that stays put while the map changes;
    // only Core 0 removes entries, and the shell loop holds one session at a
    // time and re-checks `session_exists` after anything that may close it.
    Some(unsafe { &mut *p })
}

/// Does a session for `terminal_idx` still exist? Lets the read loop detect
/// that a mouse-driven window close (close_window → destroy_session) freed the
/// session it holds, WITHOUT dereferencing the dangling reference. Core 0 only.
fn session_exists(terminal_idx: u8) -> bool {
    SESSIONS.lock().contains_key(&terminal_idx)
}

/// Per-terminal CWD (accessible from all cores via Mutex).
/// Separate from IntentSession because workers need read access (resolve_path).
static CWDS: Mutex<BTreeMap<u8, String>> = Mutex::new(BTreeMap::new());

/// Current working directory for the active terminal (or worker redirect).
fn get_cwd() -> String {
    let term = current_terminal();
    CWDS.lock().get(&term).cloned().unwrap_or_default()
}

/// Set CWD for the current terminal.
pub fn set_cwd(path: &str) {
    let term = current_terminal();
    let mut cwds = CWDS.lock();
    let mut clean = String::new();
    let trimmed = path.trim_matches('/');
    if !trimmed.is_empty() {
        for part in trimmed.split('/') {
            if part == "." { continue; }
            if part == ".." {
                if let Some(idx) = clean.rfind('/') {
                    clean.truncate(idx);
                } else {
                    clean.clear();
                }
                continue;
            }
            if !clean.is_empty() { clean.push('/'); }
            clean.push_str(part);
        }
    }
    cwds.insert(term, clean);
}

/// Determine which terminal the current core is operating on.
/// Workers: output redirect terminal. Core 0: active terminal.
fn current_terminal() -> u8 {
    if let Some(redirect) = crate::shade::terminal::output_redirect_terminal() {
        redirect
    } else {
        crate::shade::terminal::active_idx()
    }
}

// ── Intent Job System (dispatch intents to worker cores) ─────
//
// Heavy intents (http, update, install, etc.) are spawned as tasks
// on worker cores. Core 0 returns to the event loop immediately.
// Output is redirected to the intent's terminal via CORE_OUTPUT.

use core::sync::atomic::{AtomicBool, AtomicPtr, Ordering as AtOrd};

const MAX_INTENT_JOBS: usize = 4;

struct IntentJob {
    command: [u8; INPUT_BUF_SIZE],
    command_len: usize,
    terminal_idx: u8,
    session_id: CapId,
}

static INTENT_JOBS: Mutex<[Option<IntentJob>; MAX_INTENT_JOBS]> = Mutex::new([
    None, None, None, None,
]);

/// Maximum terminals (must match shade::terminal::MAX_TERMINALS).
const MAX_TERMS: usize = 256;

/// Per-terminal flag: true if an intent is running on a worker.
static INTENT_RUNNING: [AtomicBool; MAX_TERMS] = {
    const FALSE: AtomicBool = AtomicBool::new(false);
    [FALSE; MAX_TERMS]
};

/// Global vault reference (set once in run_loop, used by workers).
static VAULT_REF: AtomicPtr<Mutex<Vault>> = AtomicPtr::new(core::ptr::null_mut());

/// Check if a terminal has an intent running on a worker.
pub fn has_running_intent(terminal_idx: u8) -> bool {
    let idx = terminal_idx as usize;
    if idx >= MAX_TERMS { return false; }
    INTENT_RUNNING[idx].load(AtOrd::Acquire)
}

/// Spawn an intent on a worker core. Returns true if dispatched.
fn spawn_intent_on_worker(input: &str, terminal_idx: u8, session_id: CapId) -> bool {
    let mut jobs = INTENT_JOBS.lock();
    let slot = match jobs.iter().position(|j| j.is_none()) {
        Some(i) => i,
        None => return false,
    };

    let mut command = [0u8; INPUT_BUF_SIZE];
    let len = input.len().min(INPUT_BUF_SIZE);
    command[..len].copy_from_slice(&input.as_bytes()[..len]);

    jobs[slot] = Some(IntentJob { command, command_len: len, terminal_idx, session_id });
    drop(jobs);

    INTENT_RUNNING[terminal_idx as usize].store(true, AtOrd::Release);

    crate::smp::scheduler::spawn(
        "intent",
        intent_worker_task,
        slot as u64,
    );

    true
}

/// Worker-core entry: executes an intent, writes output to the terminal.
fn intent_worker_task(arg: u64) {
    let slot = arg as usize;
    let job = {
        let mut jobs = INTENT_JOBS.lock();
        if slot >= MAX_INTENT_JOBS { return; }
        jobs[slot].take()
    };
    let job = match job { Some(j) => j, None => return };

    let input = match core::str::from_utf8(&job.command[..job.command_len]) {
        Ok(s) => s.trim(),
        Err(_) => {
            INTENT_RUNNING[job.terminal_idx as usize].store(false, AtOrd::Release);
            wake_shell();
            return;
        }
    };

    // Extract verb for process name
    let verb = input.splitn(2, ' ').next().unwrap_or("?");
    let core_id = crate::smp::per_core::current_core_id();

    // Register in process table
    let pid = crate::process::spawn(verb, crate::process::KIND_INTENT,
                                     job.terminal_idx, core_id as u8);
    let start_tsc = crate::interrupts::rdtsc();

    // Redirect kprint output to this terminal
    crate::shade::terminal::set_output_redirect(job.terminal_idx);

    // Get vault reference
    let vault_ptr = VAULT_REF.load(AtOrd::Acquire);
    if !vault_ptr.is_null() {
        // SAFETY: vault_ptr is a &'static Mutex<Vault> set in run_loop
        let vault: &'static Mutex<Vault> = unsafe { &*vault_ptr };
        dispatch_intent(input, vault, job.session_id);
    }

    // Track CPU time + deregister process
    let elapsed = crate::interrupts::rdtsc().saturating_sub(start_tsc);
    crate::process::add_busy_tsc(pid, elapsed);
    crate::process::exit(pid);

    // Clear redirect + mark done (Core 0 prints the prompt when it detects completion)
    crate::shade::terminal::clear_output_redirect();
    INTENT_RUNNING[job.terminal_idx as usize].store(false, AtOrd::Release);
            wake_shell();
    crate::shade::terminal::mark_dirty();
}

/// Check if an intent should run on Core 0 (needs interactive input or compositor).
fn is_core0_intent(verb: &str) -> bool {
    matches!(verb, "lock" | "passwd" | "password" | "passphrase" |
                   "clear" | "cls" | "shade" | "shell" | "npk-shell" |
                   "cd" | "pwd" | "top" | "htop" | "cores" | "cpu" | "history" | "gpu" |
                   // `power` reads core-local MSRs (RAPL) and halts Core 0
                   // itself; on a worker both would give the wrong figure.
                   "power" | "watt" | "watts" |
                   // microvm + browser: VMX state (CR4.VMXE,
                   // IA32_FEATURE_CONTROL lock-bit, TSS, GDT-with-TR-
                   // slot) is BSP-only — worker cores would VMfail
                   // with error 8 (invalid host-state) because their
                   // TR is null. `browser` is just `microvm linux`
                   // under a user-friendly name, same constraint.
                   "microvm" | "browser")
}


/// Get the home directory from config.
pub(crate) fn home_dir() -> String {
    match crate::config::get("name") {
        Some(name) => alloc::format!("home/{}", name),
        None => String::from("home"),
    }
}

/// Resolve a name relative to cwd.
/// - Absolute (starts with /): strip leading / and use as-is
/// - ".." : go up one level
/// - Relative: prepend cwd
pub(crate) fn resolve_path(name: &str) -> String {
    let name = name.trim();
    let cwd = get_cwd();

    // Build full path: absolute (starts with /) or relative (prepend cwd)
    let full = if name.starts_with('/') {
        String::from(name.trim_start_matches('/'))
    } else if cwd.is_empty() {
        String::from(name)
    } else {
        alloc::format!("{}/{}", cwd, name)
    };

    // Normalize: resolve . and .. components
    let mut parts: alloc::vec::Vec<&str> = alloc::vec::Vec::new();
    for component in full.split('/') {
        match component {
            "" | "." => {} // skip empty and current-dir
            ".." => { parts.pop(); }
            c => parts.push(c),
        }
    }

    parts.join("/")
}

/// Exposed for the TLS layer: `lanpin` must tell a literal address
/// from a name, because a pin never applies to a name.
pub(crate) fn parse_ip_pub(s: &str) -> Option<[u8; 4]> { parse_ip(s) }

fn parse_ip(s: &str) -> Option<[u8; 4]> {
    let parts: alloc::vec::Vec<&str> = s.split('.').collect();
    if parts.len() != 4 { return None; }
    let mut ip = [0u8; 4];
    for (i, p) in parts.iter().enumerate() {
        ip[i] = p.parse().ok()?;
    }
    Some(ip)
}

/// Ensure every directory along `path` exists (`mkdir -p`).
pub(crate) fn ensure_parents(path: &str) {
    let _ = crate::npkfs::fs::ensure_dirs(path);
}

/// Sync session state to terminal.rs saved input (for cursor restore on focus change).
/// Temporarily switches to the session's terminal to save to the correct slot.
/// Bytes of the UTF-8 sequence that starts with `b` (1 for ASCII and for a
/// byte that cannot start one).
fn utf8_len(b: u8) -> usize {
    match b {
        0xC2..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF4 => 4,
        _ => 1,
    }
}

/// Start of the character after the one at `i` in UTF-8 `buf`.
fn next_char_at(buf: &[u8], i: usize) -> usize {
    let mut j = (i + 1).min(buf.len());
    while j < buf.len() && buf[j] & 0xC0 == 0x80 { j += 1; }
    j
}

/// Start of the character before `i` in UTF-8 `buf`.
fn prev_char_at(buf: &[u8], i: usize) -> usize {
    let mut j = i.min(buf.len()).saturating_sub(1);
    while j > 0 && buf[j] & 0xC0 == 0x80 { j -= 1; }
    j
}

fn sync_session_to_terminal(session: &IntentSession) {
    let current = crate::shade::terminal::active_idx();
    if current != session.terminal_idx {
        crate::shade::terminal::set_active_terminal(session.terminal_idx);
    }
    crate::shade::terminal::save_input_with_cursor(
        &session.input_buf[..session.pos], session.pos, session.cursor);
    if current != session.terminal_idx {
        crate::shade::terminal::set_active_terminal(current);
    }
}

/// Read a line from serial/keyboard with tab-completion, history, and network polling.
/// All input state lives in the session (input_buf, pos, cursor, esc, history).
/// Returns number of bytes read, or 0 if focus changed / mode switched.
fn read_line_with_tab(session: &mut IntentSession, vault: &'static Mutex<Vault>,
                      session_id: CapId) -> Option<usize> {
    session.history.reset_cursor();
    // Plain copy of our terminal so we can check the session is still alive
    // after re-entrant compositor calls without touching a freed `session`.
    let term_idx = session.terminal_idx;

    loop {
        // Detect focus change (mouse click, shade action, WASM switch)
        if crate::shade::is_active() {
            // Focus moved to a widget-kind window: return so run_loop
            // enters the widget-focused input branch. Otherwise keys would
            // be consumed as shell-line edits and never reach the focused
            // widget app.
            if crate::shade::focused_widget_id().is_some() {
                sync_session_to_terminal(session);
                return None;
            }
            // Same for a Surface (microvm) window — bail so run_loop
            // enters the Surface-focused branch instead of leaving us
            // wedged reading input for a window with no session.
            if crate::shade::focused_surface_id().is_some() {
                sync_session_to_terminal(session);
                return None;
            }

            let ft = crate::shade::terminal::active_idx();

            if crate::wasm::has_wasm_app(ft) {
                sync_session_to_terminal(session);
                return None;
            }

            // Focus changed to a different terminal — return so run_loop can switch sessions
            if ft != session.terminal_idx {
                sync_session_to_terminal(session);
                return None;
            }
        }

        // Poll network while waiting. Snapshot the console-write
        // generation around the async pumps: if any of them emitted
        // output (guest logs, net, debug-mirror) the user's input
        // line is now corrupted/prompt-less, so redraw `path> input`.
        // Edge-triggered (only on actual output) and general — not
        // microvm-specific. Keyboard echo isn't counted: it happens
        // later in this loop, after this check, and the snapshot is
        // re-taken at the next iteration's top.
        let console_gen = crate::serial::write_gen();
        crate::net::poll();
        // Also while waiting for a line: this reader blocks until Enter, and
        // a link that comes up meanwhile (e.g. WiFi finishing its handshake)
        // must be noticed without a keystroke. Self-throttled to ~1 Hz.
        crate::net::tick_link_and_reconfigure();
        // A running microvm is the foreground task: give it a real time
        // budget per outer iteration (8 slices, ~24 ms guest), then
        // input/render/spin. One slice per iteration starves the guest.
        if crate::microvm::vm_active() {
            for _ in 0..8 { crate::microvm::vm_poll_slice(); }
        } else {
            crate::microvm::vm_poll_slice();
        }
        crate::shell::check_and_serve(vault, session_id);
        // Only redraw when the user has typed something: async output would
        // otherwise split the in-progress command. With empty input, redrawing
        // `path> ` between streamed lines is just noise.
        // Serial console only: in shade the live input line is redrawn via
        // rewrite_input, and this kprint would (a) duplicate the prompt and
        // (b) go to the primary debug sink, not the focused loop.
        if !crate::shade::is_active()
            && session.pos > 0 && crate::serial::write_gen() != console_gen {
            let cwd = get_cwd();
            let path = if cwd.is_empty() { "/" } else { cwd.as_str() };
            let inp = core::str::from_utf8(&session.input_buf[..session.pos]).unwrap_or("");
            kprint!("{}> {}", path, inp);
        }

        // Tick swap animation
        if crate::shade::with_compositor(|comp| comp.tick_animation()).unwrap_or(false) {
            crate::shade::render_frame();
        } else {
            // This loop idles in `hlt` without ever reaching poll_render, so
            // the focus flash needs its tick here too.
            crate::shade::tick_focus_glow();
        }

        while let Some(evt) = crate::xhci::poll_mouse() {
            crate::shade::handle_mouse(&evt);
        }
        // A click on the window's X button closes it inside handle_mouse
        // (close_window → destroy_session), freeing this `session`. The
        // ShadeAction (Mod+Q) close path below is guarded by sync+return, the
        // mouse path is not, so bail before any further deref. run_loop
        // re-acquires a valid session on the next pass (None arm).
        if !session_exists(term_idx) { return None; }
        // Session changes queued by other cores apply at the top of
        // run_loop, where no session is held.
        if !SESSION_OPS.lock().is_empty() {
            sync_session_to_terminal(session);
            return None;
        }

        if crate::shade::take_deferred_render() {
            crate::shade::render_frame();
        }

        // Shade compositor actions (Mod+key)
        if let Some(action) = crate::shade::input::poll_action() {
            use crate::shade::input::ShadeAction;
            match action {
                ShadeAction::FocusLeft | ShadeAction::FocusRight |
                ShadeAction::FocusUp | ShadeAction::FocusDown |
                ShadeAction::SwapLeft | ShadeAction::SwapRight |
                ShadeAction::SwapUp | ShadeAction::SwapDown |
                ShadeAction::ResizeLeft | ShadeAction::ResizeRight |
                ShadeAction::ResizeUp | ShadeAction::ResizeDown |
                ShadeAction::Workspace(_) | ShadeAction::MoveToWorkspace(_) => {
                    sync_session_to_terminal(session);
                    crate::shade::handle_action(action);
                    let new_term = crate::shade::terminal::active_idx();
                    if new_term != session.terminal_idx || crate::wasm::has_wasm_app(new_term) {
                        return None;
                    }
                }
                ShadeAction::NewWindow => {
                    sync_session_to_terminal(session);
                    crate::shade::handle_action(action);
                    return None; // run_loop creates session for new window
                }
                ShadeAction::CloseWindow | ShadeAction::SpawnLauncher => {
                    // Both can destroy / invalidate the current session's
                    // terminal or move focus to a widget window while we
                    // hold `&mut session` into the SESSIONS map. Sync
                    // session state to the terminal buffer first, run
                    // the action, then bail so run_loop re-acquires the
                    // (possibly new / absent / widget-focused) session
                    // cleanly: no dangling refs, no stale focus.
                    sync_session_to_terminal(session);
                    crate::shade::handle_action(action);
                    return None;
                }
                _ => {
                    crate::shade::handle_action(action);
                }
            }
            continue;
        }

        // Read keyboard input as KeyEvent (or serial fallback).
        // When shade is active, USB keyboard is the only input — serial is
        // skipped entirely to avoid cross-consumption races between sources.
        let event = if let Some(evt) = crate::keyboard::read_event() {
            evt
        } else if crate::shade::is_active() {
            // Shade mode: normally idle until the next IRQ. A running
            // microvm is the foreground task, so do not sleep: spin
            // straight back to feed it more slices (a hlt here allows
            // only one slice per timer tick). Without a VM we still halt.
            if !crate::microvm::vm_active() {
                core0_wait();
            }
            continue;
        } else {
            let serial = serial::SERIAL.lock();
            if !serial.has_data() {
                drop(serial);
                // Serial mode: the tick wakes us to look again.
                core0_wait();
                continue;
            }
            // Use raw serial read — read_byte() has a legacy loop that also
            // polls the USB keyboard, which would race with read_event() above
            // and cause the two input sources to steal each other's keys.
            let b = serial.read_serial_raw();
            drop(serial);
            // Serial: basic byte-to-KeyEvent (no modifier capture)
            match b {
                b'\r' | b'\n' => crate::input::KeyEvent::special(crate::input::KeyCode::Enter, crate::input::Modifiers::NONE),
                0x08 | 0x7F => crate::input::KeyEvent::special(crate::input::KeyCode::Backspace, crate::input::Modifiers::NONE),
                b'\t' => crate::input::KeyEvent::special(crate::input::KeyCode::Tab, crate::input::Modifiers::NONE),
                0x1B => crate::input::KeyEvent::special(crate::input::KeyCode::Escape, crate::input::Modifiers::NONE),
                c => crate::input::KeyEvent::char(c, crate::input::Modifiers::NONE),
            }
        };

        // Shade keybindings (Mod+key) — consumes the event if matched
        if crate::shade::input::try_keybind_event(&event) {
            continue;
        }

        use crate::input::KeyCode;

        // Shift+arrow → extend a terminal text selection (mirrors the widget
        // editor); Ctrl+Shift+C then copies it. Any other key drops a stale
        // selection so the next Shift+arrow re-anchors at the caret. Only in
        // shade mode — serial has no selectable grid.
        if crate::shade::is_active() {
            let dir = if event.modifiers.shift {
                match event.key {
                    KeyCode::Left  => Some(crate::shade::terminal::SelDir::Left),
                    KeyCode::Right => Some(crate::shade::terminal::SelDir::Right),
                    KeyCode::Up    => Some(crate::shade::terminal::SelDir::Up),
                    KeyCode::Down  => Some(crate::shade::terminal::SelDir::Down),
                    _ => None,
                }
            } else { None };
            if let Some(d) = dir {
                if crate::shade::terminal::selection_key(session.terminal_idx as usize, d) {
                    crate::shade::refresh_focused_terminal();
                }
                continue;
            } else if crate::shade::terminal::clear_selection(session.terminal_idx as usize) {
                crate::shade::refresh_focused_terminal();
            }
        }

        match event.key {
            KeyCode::Up => {
                if let Some(line) = session.history.up() {
                    let len = line.len().min(session.input_buf.len());
                    if !crate::shade::is_active() {
                        for _ in 0..session.pos { kprint!("\x08 \x08"); }
                    }
                    session.input_buf[..len].copy_from_slice(&line.as_bytes()[..len]);
                    session.pos = len;
                    session.cursor = len;
                    if crate::shade::is_active() {
                        crate::shade::terminal::rewrite_input(&session.input_buf, session.pos);
                    } else if let Ok(s) = core::str::from_utf8(&session.input_buf[..session.pos]) {
                        kprint!("{}", s);
                    }
                }
            }
            KeyCode::Down => {
                if !crate::shade::is_active() {
                    for _ in 0..session.pos { kprint!("\x08 \x08"); }
                }
                if let Some(line) = session.history.down() {
                    let len = line.len().min(session.input_buf.len());
                    session.input_buf[..len].copy_from_slice(&line.as_bytes()[..len]);
                    session.pos = len;
                    session.cursor = len;
                    if crate::shade::is_active() {
                        crate::shade::terminal::rewrite_input(&session.input_buf, session.pos);
                    } else if let Ok(s) = core::str::from_utf8(&session.input_buf[..session.pos]) {
                        kprint!("{}", s);
                    }
                } else {
                    session.pos = 0;
                    session.cursor = 0;
                    if crate::shade::is_active() {
                        crate::shade::terminal::rewrite_input(&session.input_buf, 0);
                    }
                }
            }
            KeyCode::Right => {
                if session.cursor < session.pos {
                    session.cursor = next_char_at(&session.input_buf[..session.pos], session.cursor);
                }
            }
            KeyCode::Left => {
                if session.cursor > 0 {
                    session.cursor = prev_char_at(&session.input_buf[..session.pos], session.cursor);
                }
            }
            KeyCode::Home => { session.cursor = 0; }
            KeyCode::End => { session.cursor = session.pos; }
            KeyCode::PageUp | KeyCode::PageDown | KeyCode::Insert => {}
            KeyCode::Delete => {
                if session.cursor < session.pos {
                    let end = next_char_at(&session.input_buf[..session.pos], session.cursor);
                    session.input_buf.copy_within(end..session.pos, session.cursor);
                    session.pos -= end - session.cursor;
                    if crate::shade::is_active() {
                        crate::shade::terminal::rewrite_input(&session.input_buf, session.pos);
                    }
                }
            }
            KeyCode::Enter => {
                session.cursor = session.pos;
                // Route the commit newline to this loop (like the prompt +
                // command output), not the primary debug sink; otherwise
                // with active != primary the '\n' lands in the primary loop
                // and prompts stack on one line in the focused loop.
                crate::shade::terminal::set_output_redirect(session.terminal_idx);
                kprint!("\n");
                crate::shade::terminal::clear_output_redirect();
                session.history.push(&session.input_buf[..session.pos]);
                return Some(session.pos);
            }
            KeyCode::Backspace => {
                if session.cursor > 0 {
                    let start = prev_char_at(&session.input_buf[..session.pos], session.cursor);
                    session.input_buf.copy_within(session.cursor..session.pos, start);
                    session.pos -= session.cursor - start;
                    session.cursor = start;
                    if crate::shade::is_active() {
                        crate::shade::terminal::rewrite_input(&session.input_buf, session.pos);
                        crate::shade::terminal::set_cursor_pos(
                            crate::shade::terminal::current_line_len()
                                .saturating_sub(session.pos - session.cursor));
                        crate::shade::render_input_line();
                    } else {
                        kprint!("\x08 \x08");
                    }
                }
                continue; // skip cursor update below
            }
            KeyCode::Tab => {
                if let Ok(input) = core::str::from_utf8(&session.input_buf[..session.pos]) {
                    if let Some(completion) = tab_complete(input) {
                        for cb in completion.as_bytes() {
                            if session.pos < session.input_buf.len() {
                                session.input_buf[session.pos] = *cb;
                                session.pos += 1;
                            }
                        }
                        session.cursor = session.pos;
                        if crate::shade::is_active() {
                            crate::shade::terminal::rewrite_input(&session.input_buf, session.pos);
                            crate::shade::terminal::set_cursor_pos(
                                crate::shade::terminal::current_line_len()
                                    .saturating_sub(session.pos - session.cursor));
                            crate::shade::render_input_line();
                        } else {
                            kprint!("{}", completion);
                        }
                    }
                }
                continue; // skip cursor update below
            }
            KeyCode::Escape => { continue; }
            KeyCode::F(_) => { continue; }
            KeyCode::Char(b) => {
                // A non-ASCII character arrives as its UTF-8 bytes, back to
                // back in the key ring (`keyboard::push_char`); take the rest
                // of the sequence and insert it as one character.
                let mut seq = [b, 0, 0, 0];
                let n = utf8_len(b);
                let mut got = 1;
                while got < n {
                    match crate::keyboard::read_key() {
                        Some(c) if c & 0xC0 == 0x80 => { seq[got] = c; got += 1; }
                        _ => break,
                    }
                }
                let ch = &seq[..got];
                let printable = match core::str::from_utf8(ch) {
                    Ok(t) => t.chars().all(|c| !c.is_control()),
                    Err(_) => false,
                };
                if printable && session.pos + got < session.input_buf.len() {
                    session.input_buf.copy_within(session.cursor..session.pos, session.cursor + got);
                    session.input_buf[session.cursor..session.cursor + got].copy_from_slice(ch);
                    session.pos += got;
                    session.cursor += got;
                    crate::shade::terminal::scroll_reset();
                    if crate::shade::is_active() {
                        crate::shade::terminal::rewrite_input(&session.input_buf, session.pos);
                        crate::shade::terminal::set_cursor_pos(
                            crate::shade::terminal::current_line_len()
                                .saturating_sub(session.pos - session.cursor));
                        crate::shade::render_input_line();
                    } else if let Ok(t) = core::str::from_utf8(ch) {
                        kprint!("{}", t);
                    }
                }
                continue; // skip cursor update below
            }
        }

        // Update cursor position for navigation keys (arrows, Home, End)
        if crate::shade::is_active() {
            crate::shade::terminal::set_cursor_pos(
                crate::shade::terminal::current_line_len()
                    .saturating_sub(session.pos - session.cursor));
            crate::shade::render_input_line();
        }
    }
}

/// Tab-completion: find matching paths for the last word in the input.
///
/// Lists the immediate children of the implied parent directory and
/// filters them by the partial leaf name.
fn tab_complete(input: &str) -> Option<String> {
    let last_space = input.rfind(' ').map(|i| i + 1).unwrap_or(0);
    let partial = &input[last_space..];

    // Split `partial` into (parent_dir_to_list, leaf_prefix_to_match).
    //   ""        → cwd, no prefix
    //   "te"      → cwd, prefix "te"
    //   "docs/"   → docs/, no prefix
    //   "docs/n"  → docs/, prefix "n"
    let (parent_abs, leaf_prefix): (String, String) = if partial.is_empty() {
        (get_cwd(), String::new())
    } else if partial.ends_with('/') {
        (resolve_path(partial.trim_end_matches('/')), String::new())
    } else if let Some(idx) = partial.rfind('/') {
        (resolve_path(&partial[..idx]), String::from(&partial[idx + 1..]))
    } else {
        (get_cwd(), String::from(partial))
    };

    use crate::npkfs::object::EntryKind;
    let entries = match crate::npkfs::fs::list(&parent_abs) {
        Ok(Some(v)) => v,
        Ok(None) | Err(_) => return None,
    };

    // Search prefix used by the legacy display logic below: the part
    // of the partial path the user has already committed to.
    let search = if parent_abs.is_empty() {
        String::new()
    } else {
        alloc::format!("{}/", parent_abs)
    };

    let mut matches: alloc::vec::Vec<String> = alloc::vec::Vec::new();
    for e in &entries {
        if !e.name.starts_with(&leaf_prefix) { continue; }
        if e.name.starts_with(".npk-") { continue; }
        let full = match e.kind {
            EntryKind::Dir => alloc::format!("{}{}/", search, e.name),
            EntryKind::File => alloc::format!("{}{}", search, e.name),
        };
        if !matches.contains(&full) { matches.push(full); }
    }

    if matches.is_empty() { return None; }

    // Calculate how much the user already typed as resolved path
    let typed_resolved = if partial.is_empty() || partial.ends_with('/') {
        search.clone()
    } else {
        resolve_path(partial)
    };

    if matches.len() == 1 {
        let full = &matches[0];
        if full.len() > typed_resolved.len() {
            return Some(String::from(&full[typed_resolved.len()..]));
        }
        return None;
    }

    // Multiple matches — try common prefix extension
    let common = common_prefix(&matches);
    if common.len() > typed_resolved.len() {
        return Some(String::from(&common[typed_resolved.len()..]));
    }

    // Show options
    kprint!("\n");
    let display_base = if let Some(idx) = search.rfind('/') { &search[..idx + 1] } else { "" };
    for m in &matches {
        let rel = m.strip_prefix(display_base).unwrap_or(m);
        kprint!("  {}  ", rel);
    }
    kprint!("\n");

    // Re-print prompt + current input
    let cwd = get_cwd();
    let path = if cwd.is_empty() { "/" } else { cwd.as_str() };
    kprint!("{}> {}", path, input);

    None
}

fn common_prefix(strings: &[String]) -> String {
    if strings.is_empty() { return String::new(); }
    let first = strings[0].as_bytes();
    let mut len = first.len();
    for s in &strings[1..] {
        let b = s.as_bytes();
        len = len.min(b.len());
        for i in 0..len {
            if first[i] != b[i] {
                len = i;
                break;
            }
        }
    }
    String::from(&strings[0][..len])
}

/// Idle Core 0 until the next interrupt instead of busy-spinning between
/// poll cycles. While a focused app (widget / intent / wasm) runs on a
/// worker, Core 0 only needs to wake to forward input, drive the cursor /
/// dock, and detect completion. The per-core 100 Hz LAPIC timer guarantees
/// a wake within 10 ms (plain HLT = C1 keeps the timer ticking), and
/// keyboard / mouse / net IRQs wake it immediately.
#[inline]
fn core0_idle_tick() {
    // The PS/2 mouse is polled (IRQ12 masked on UEFI machines), so halting
    // until the 100 Hz timer caps the pointer sample and cursor-redraw rate at
    // ~100 Hz, which feels laggy. While the pointer is moving, don't halt:
    // return so the loop spins and drains the mouse at its full rate. Once
    // motion stops (no PS/2 byte for ~40 ms) we halt again, so idle power is
    // unchanged. No-op on USB/IRQ-mouse hosts (mouse_active_within stays false).
    if crate::keyboard::mouse_active_within(40) {
        // Deliberate spin; it must not be recorded as a halt, because the
        // core really is busy and the usage figure should say so.
        return;
    }
    // Park the shell fiber until input or the next tick; the halt
    // itself, and its accounting, happens in `per_core::core0_loop`.
    core0_wait();
}

/// Idle auto-GC trigger, called from the shell run-loop's ~1 Hz top.
/// Reclaims orphans once `GC_PRESSURE_THRESHOLD` mutations have piled up,
/// but only when the system is quiet: FS mounted, no in-flight streaming
/// write (GC would eat its chunks), and no focused microvm surface (a GC
/// stall would hitch the guest). Self-throttled so the gate checks don't
/// run on every spin. GC skips Blob bodies and resets the pressure
/// counter, so after a sweep this won't fire until real churn rebuilds it.
fn maybe_idle_gc() {
    use core::sync::atomic::{AtomicU64, Ordering};
    static LAST_CHECK: AtomicU64 = AtomicU64::new(0);
    const CHECK_INTERVAL_TICKS: u64 = 300; // ~3 s @ 100 Hz

    let now = crate::interrupts::ticks();
    if now.wrapping_sub(LAST_CHECK.load(Ordering::Relaxed)) < CHECK_INTERVAL_TICKS {
        return;
    }
    LAST_CHECK.store(now, Ordering::Relaxed);

    if !crate::storage::npkfs::is_mounted() { return; }
    if crate::shade::focused_surface_id().is_some() { return; }
    if crate::storage::npkfs::fs::stream_active() { return; }
    if crate::storage::npkfs::fs::gc_pressure()
        < crate::storage::npkfs::fs::GC_PRESSURE_THRESHOLD
    {
        return;
    }

    // The sweep runs on a worker: it walks the whole tree under ROOT_MUTEX
    // with disk I/O, and on Core 0 it would stall the shell, the cursor and
    // every frame for as long as it takes.
    if crate::smp::scheduler::worker_count() == 0 {
        idle_gc_task(0);
    } else if !GC_RUNNING.swap(true, core::sync::atomic::Ordering::AcqRel) {
        crate::smp::scheduler::spawn("gc", idle_gc_task, 0);
    }
}

static GC_RUNNING: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

fn idle_gc_task(_: u64) {
    match crate::storage::npkfs::fs::gc() {
        Ok(s) if !s.skipped && s.removed > 0 =>
            kprintln!("[npk] auto-gc: reclaimed {} orphan(s), {} kept", s.removed, s.kept),
        // skipped (a stream raced in) or nothing to reclaim — stay quiet.
        _ => {}
    }
    GC_RUNNING.store(false, core::sync::atomic::Ordering::Release);
}

/// The shell's fiber on Core 0, signalled by the input interrupts so a
/// key wakes the shell at once instead of on its next tick.
static SHELL_WAKER: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(crate::smp::fiber::NO_WAKER);

/// Input arrived (called from the i8042 and xHCI interrupt handlers).
pub fn wake_shell() {
    let w = SHELL_WAKER.load(AtOrd::Acquire);
    if w != crate::smp::fiber::NO_WAKER {
        crate::smp::fiber::signal(w, crate::smp::fiber::SIG_EVENT);
    }
}

/// Core 0's idle step in the shell loop: park the shell fiber until
/// something wakes it, so other Core-0 fibers run.
///
/// How long: one frame (10 ms) while anything needs the loop on its own:
/// an animation or the dock (`shade::needs_tick`), a network timer or a
/// polled card (`net::needs_tick`), a held USB key, input that no
/// interrupt reports, serial mode, a cooperative guest. Otherwise one
/// second, which drives the link check and the idle GC. Everything else
/// wakes the shell (`wake_shell`): input, render requests, terminal output,
/// a finished intent, a microVM request. A dependency missed here shows as
/// up to a second of lag, not as a hang.
fn core0_wait() {
    let freq = crate::interrupts::tsc_freq();
    // Media keys and unmapped-key reports the PS/2 ISR deferred to here.
    crate::keyboard::apply_deferred();
    // Input that no interrupt reports is drained here.
    if crate::xhci::needs_poll() || crate::xhci::take_missed_drain() {
        crate::interrupts::without_interrupts(crate::xhci::poll_events_irq);
    }
    let busy = crate::shade::needs_tick()
        || crate::net::needs_tick()
        || crate::xhci::repeat_active()
        || crate::xhci::needs_poll()
        || crate::keyboard::needs_poll()
        || !crate::shade::is_active()
        || crate::microvm::vm_active();
    let d = crate::interrupts::rdtsc() + if busy { freq / 100 } else { freq };
    if crate::smp::fiber::wait(crate::smp::fiber::SIG_EVENT, d).is_none() {
        // Not in a fiber (never after boot): halt to the same deadline.
        crate::interrupts::halt_until(Some(d), crate::smp::per_core::WAKE_HLT_FALLBACK);
    }
}

pub fn run_loop(vault: &'static Mutex<Vault>, session_id: CapId) -> ! {
    // Store vault reference for worker cores
    VAULT_REF.store(vault as *const _ as *mut _, AtOrd::Release);
    if let Some(w) = crate::smp::fiber::current_waker() {
        SHELL_WAKER.store(w, AtOrd::Release);
    }

    // Session is created by compositor::create_window (not here).
    // Pre-shade: serial-only output, no session needed.

    // WASM key routing state (not per-session — persists across focus changes)
    let mut wasm_esc: u8 = 0;
    let mut wasm_esc_mod = false;
    let mut from_wasm = false;
    let mut wasm_term: u8 = 255;
    let mut from_intent = false;
    let mut need_prompt = true;
    let mut shade_was_active = crate::shade::is_active();

    loop {
        // ~1 Hz (self-throttled): refresh wired carrier + auto-reconfigure IP
        // when the active interface changes (LAN cable pulled → WiFi takes over
        // with a fresh DHCP lease, or a static config). No manual `dhcp` needed.
        crate::net::tick_link_and_reconfigure();

        // Self-throttled: reclaim orphaned objects when enough mutations
        // have accumulated and the system is quiet. Keeps an active disk
        // from filling with COW/overwrite/delete orphans (npkFS is
        // content-addressed → every write orphans the old version). The
        // F2FS / `git gc --auto` model.
        maybe_idle_gc();

        // If focused window has a running WASM app or intent, route keys / wait.
        if crate::shade::is_active() {
            let focused_term = crate::shade::terminal::active_idx();

            // Surface-kind (microvm) window focused. It has no
            // terminal session (idx 255), so the terminal path below
            // would wedge and shade keybinds would never run. Here:
            // keep Core 0 polling (the guest keeps slicing and the
            // surface keeps compositing), let shade keybinds through
            // (Mod+Q closes it and tears the VM down), and forward
            // other keys to the guest.
            if crate::shade::focused_surface_id().is_some() {
                // Drain the mouse ring here, before poll_render():
                // poll_mouse is a consuming SPSC ring and poll_render
                // has its own loop, so running it first would eat every
                // event and the guest would never see a click.
                // handle_mouse still drives the host cursor/drag/focus;
                // poll_render's own loop then finds an empty ring.
                while let Some(evt) = crate::xhci::poll_mouse() {
                    // handle_mouse forwards to the guest internally
                    // (race-free across all poll_mouse consumers) and
                    // drives the host cursor/drag/focus.
                    crate::shade::handle_mouse(&evt);
                }
                crate::shade::poll_render();
                crate::net::poll();
                // Cooperative path (≤2 cores): Core 0 is the guest's CPU,
                // so it must keep slicing, 8 slices (~24 ms guest) per
                // composite cycle (net::poll runs inside each slice's pump,
                // so L3 stays responsive). Fiber/dedicated path: the guest
                // runs on a worker core and vm_poll_slice is only a cheap
                // reaper, so Core 0 must not spin; it idles below.
                let cooperative = crate::microvm::vm_active();
                if cooperative {
                    for _ in 0..8 { crate::microvm::vm_poll_slice(); }
                } else {
                    crate::microvm::vm_poll_slice(); // reaper only
                }
                if crate::shade::take_deferred_render() {
                    crate::shade::render_frame();
                }
                if let Some(action) = crate::shade::input::poll_action() {
                    crate::shade::handle_action(action);
                }
                while let Some(event) = crate::keyboard::read_event() {
                    // Mod+X keybinds (Mod+Q close, focus moves, …)
                    // still reach shade so the window stays
                    // manageable. Everything else is translated to
                    // real Linux evdev keys (US xkb target, Shift
                    // derived from the char, Ctrl/Alt wrapped) and
                    // pushed into the guest's virtio-input eventq.
                    if crate::shade::input::try_keybind_event(&event) {
                        continue;
                    }
                    crate::microvm::devices::virtio_input_keymap::forward_key(&event);
                }
                // Guest runs on a worker core → idle Core 0 until the next
                // IRQ (input, or the 100 Hz timer to re-check for a new
                // guest frame) instead of spinning at 100%. Keyboard/mouse
                // IRQs wake us to forward input; a deferred guest frame is
                // composited within ≤10 ms. Cooperative path keeps spinning
                // (it drives the guest).
                if !cooperative {
                    core0_idle_tick();
                }
                continue;
            }

            // Widget-kind window focused: keys go into the
            // per-window widget event queue, never the terminal / WASM
            // app key buf. The widget app polls them via npk_event_poll.
            if let Some(widget_wid) = crate::shade::focused_widget_id() {
                crate::shade::poll_render();
                crate::net::poll();
                crate::microvm::vm_poll_slice();

                while let Some(evt) = crate::xhci::poll_mouse() {
                    crate::shade::handle_mouse(&evt);
                }

                if crate::shade::take_deferred_render() {
                    crate::shade::render_frame();
                }

                if let Some(action) = crate::shade::input::poll_action() {
                    crate::shade::handle_action(action);
                }

                while let Some(event) = crate::keyboard::read_event() {
                    // Mod+X keybinds still reach shade (Mod+D, Mod+Q, etc.).
                    if crate::shade::input::try_keybind_event(&event) {
                        continue;
                    }
                    // Keyboard navigation takes visual precedence over
                    // any stale mouse-hover state. Without this, moving
                    // the selection with arrows while the cursor sits
                    // on a different row leaves both rows highlighted.
                    // Mouse-move re-establishes hover on the next motion.
                    crate::shade::widgets::suppress_hover(widget_wid);
                    // If a Widget::Input / TextArea is focused, the
                    // compositor owns text editing: printable /
                    // Backspace / Delete / arrows / Home / End / Enter
                    // (and, in a TextArea, Tab → indent) are intercepted,
                    // mutate the editor buffer, and emit Event::InputChange
                    // or Event::Action(on_submit). This runs before the
                    // Tab focus-nav below so a TextArea can claim Tab.
                    // Ctrl chords the text editor doesn't own belong to the
                    // app: Ctrl+S, Ctrl+O, … They are routed before
                    // handle_input_key because a focused Input/TextArea
                    // would swallow them: its clipboard arm consumes every
                    // Ctrl+<letter>. A/C/X/V stay with the editor
                    // (select-all, copy, cut, paste).
                    if event.modifiers.ctrl {
                        let letter = match event.key {
                            crate::input::KeyCode::Char(b) =>
                                Some(if b < 0x20 { b | 0x60 } else { b.to_ascii_lowercase() }),
                            _ => None,
                        };
                        if let Some(l) = letter {
                            // Any printable key, not just letters — zoom
                            // lives on Ctrl+plus / minus / 0 everywhere.
                            if l.is_ascii_graphic() && !matches!(l, b'a' | b'c' | b'x' | b'v') {
                                crate::shade::widgets::push_event(
                                    widget_wid,
                                    crate::shade::widgets::abi::Event::Chord {
                                        letter: l,
                                        shift:  event.modifiers.shift,
                                        alt:    event.modifiers.alt,
                                    },
                                );
                                continue;
                            }
                        }
                    }
                    if crate::shade::widgets::handle_input_key(widget_wid, event.key, event.modifiers) {
                        continue;
                    }
                    // Ctrl+C / X / V that no focused text widget claimed →
                    // deliver a semantic clipboard event to a clipboard-sink
                    // app (loft copies/moves the selected file). Gated on the
                    // opt-in so non-sink apps never see the new event variant.
                    // Normalize the same way handle_input_key does: PS/2 maps
                    // Ctrl+C to control byte 0x03 (produced only when Ctrl was
                    // held, so no mods check); Ctrl+X/V arrive as the letter
                    // (or a control byte on xHCI) and need mods.ctrl.
                    if crate::shade::widgets::is_clipboard_sink(widget_wid) {
                        let clip_letter = match event.key {
                            crate::input::KeyCode::Char(0x03) => Some(b'c'),
                            crate::input::KeyCode::Char(b) if event.modifiers.ctrl =>
                                Some(if b < 0x20 { b | 0x60 } else { b.to_ascii_lowercase() }),
                            _ => None,
                        };
                        if let Some(kind) = clip_letter.and_then(|l| match l {
                            b'c' => Some(crate::shade::widgets::abi::ClipKind::Copy),
                            b'x' => Some(crate::shade::widgets::abi::ClipKind::Cut),
                            b'v' => Some(crate::shade::widgets::abi::ClipKind::Paste),
                            _ => None,
                        }) {
                            crate::shade::widgets::push_event(
                                widget_wid,
                                crate::shade::widgets::abi::Event::Clipboard(kind),
                            );
                            continue;
                        }
                    }
                    // Tab / Shift+Tab move focus between focusable widgets
                    // when the focused widget didn't consume Tab (i.e. not
                    // a TextArea). Falls through (key reaches the app) if
                    // there are no focusable nodes.
                    if matches!(event.key, crate::input::KeyCode::Tab) {
                        let consumed = if event.modifiers.shift {
                            crate::shade::widgets::prev_focus(widget_wid)
                        } else {
                            crate::shade::widgets::next_focus(widget_wid)
                        };
                        if consumed { continue; }
                    }
                    crate::shade::widgets::push_event(
                        widget_wid,
                        crate::shade::widgets::abi::Event::Key(event.key),
                    );
                }

                // Idle until the next IRQ instead of busy-spinning, which
                // would peg Core 0 while a widget app is focused.
                core0_idle_tick();
                continue;
            }

            // Intent running on worker — event loop without input
            if has_running_intent(focused_term) {
                from_intent = true;
                crate::shade::poll_render();
                crate::net::poll();
                crate::microvm::vm_poll_slice();

                while let Some(evt) = crate::xhci::poll_mouse() {
                    crate::shade::handle_mouse(&evt);
                }

                if crate::shade::take_deferred_render() {
                    crate::shade::render_frame();
                }

                if let Some(action) = crate::shade::input::poll_action() {
                    crate::shade::handle_action(action);
                }

                // Typed keys are dropped — except while the intent asks for
                // one (`confirm`, `read_secret`): it reads the same buffer.
                if PROMPTS_WAITING.load(core::sync::atomic::Ordering::Acquire) == 0 {
                    while let Some(_key) = crate::keyboard::read_key() {}
                }

                // Idle until the next IRQ (≤10 ms via the per-core 100 Hz
                // timer) instead of busy-spinning — we re-poll the worker's
                // completion on every wake; 10 ms latency to notice an intent
                // finished is imperceptible, and net/mouse/key IRQs wake us
                // sooner. Plain HLT (C1) keeps the LAPIC timer ticking.
                core0_idle_tick();
                continue;
            }

            // WASM app running — route keys to app
            if crate::wasm::has_wasm_app(focused_term) {
                from_wasm = true;
                wasm_term = focused_term;
                crate::shade::poll_render();
                crate::net::poll();
                crate::microvm::vm_poll_slice();

                while let Some(evt) = crate::xhci::poll_mouse() {
                    crate::shade::handle_mouse(&evt);
                }

                if crate::shade::take_deferred_render() {
                    crate::shade::render_frame();
                }

                if let Some(action) = crate::shade::input::poll_action() {
                    crate::shade::handle_action(action);
                }

                while let Some(key) = crate::keyboard::read_key() {
                    if wasm_esc == 1 {
                        wasm_esc = 0;
                        if key == b'[' {
                            wasm_esc = 2;
                            continue;
                        }
                        crate::wasm::push_app_key(focused_term, 0x1B);
                        crate::wasm::push_app_key(focused_term, key);
                        continue;
                    }

                    if wasm_esc == 2 {
                        wasm_esc = 0;
                        if wasm_esc_mod && crate::shade::input::try_arrow_keybind(key) {
                            if let Some(action) = crate::shade::input::poll_action() {
                                crate::shade::handle_action(action);
                            }
                            continue;
                        }
                        crate::wasm::push_app_key(focused_term, 0x1B);
                        crate::wasm::push_app_key(focused_term, b'[');
                        crate::wasm::push_app_key(focused_term, key);
                        continue;
                    }

                    if key == 0x1B {
                        wasm_esc = 1;
                        wasm_esc_mod = crate::shade::input::is_mod_active();
                        continue;
                    }

                    if crate::shade::input::try_keybind(key) {
                        if let Some(action) = crate::shade::input::poll_action() {
                            crate::shade::handle_action(action);
                        }
                        continue;
                    }

                    crate::wasm::push_app_key(focused_term, key);
                }

                // Idle until the next IRQ (timer/input) — same as the
                // widget / intent branches. Wakes on key/mouse immediately;
                // the 100 Hz timer re-polls app completion within 10 ms.
                core0_idle_tick();
                continue;
            }
        }

        // Transition flags — need fresh prompt after WASM/intent completion
        if from_intent {
            from_intent = false;
            need_prompt = true;
            // Flush worker output before printing prompt
            if crate::shade::is_active() {
                crate::shade::render_frame();
            }
        }
        if from_wasm {
            from_wasm = false;
            let current = crate::shade::terminal::active_idx();
            if current == wasm_term {
                // WASM app exited on this terminal — need fresh prompt
                kprintln!();
                need_prompt = true;
            }
            // If focus switched to a different terminal, don't set need_prompt
            // (that terminal's session already has its own prompt state)
            if crate::shade::is_active() {
                crate::shade::render_frame();
            }
        }

        // No loop window is focused (desktop, or a non-terminal like the
        // dock/a widget has focus): there is no terminal session to drive.
        // Without this, keys typed on the bare desktop would be read into a
        // stale terminal session and executed. Idle here instead: shade
        // keybinds (Mod+Enter to open a loop, Mod+D launcher) and the mouse
        // still work; plain keys are dropped.
        // Guard only applies in shade mode; serial-only mode has no windows.
        if crate::shade::is_active() && !crate::shade::focused_is_terminal() {
            crate::shade::poll_render();
            crate::net::poll();
            crate::microvm::vm_poll_slice();
            while let Some(evt) = crate::xhci::poll_mouse() {
                crate::shade::handle_mouse(&evt);
            }
            if crate::shade::take_deferred_render() {
                crate::shade::render_frame();
            }
            if let Some(action) = crate::shade::input::poll_action() {
                crate::shade::handle_action(action);
            }
            while let Some(key) = crate::keyboard::read_key() {
                // Keybinds (Mod+…) still fire; plain keys have no session to
                // land in on the desktop, so they're discarded.
                if crate::shade::input::try_keybind(key) {
                    if let Some(action) = crate::shade::input::poll_action() {
                        crate::shade::handle_action(action);
                    }
                }
            }
            core0_idle_tick();
            continue;
        }

        // Get session for active terminal (create if needed)
        apply_session_ops();
        let term = crate::shade::terminal::active_idx();
        let session = match session_mut(term) {
            Some(s) => s,
            None => {
                apply_session_op(term, SessionOp::Create);
                need_prompt = true; // new session always needs a prompt
                match session_mut(term) {
                    Some(s) => s,
                    None => continue,
                }
            }
        };

        // Fresh session (created by compositor) that never had a prompt
        if !need_prompt && session.prompt_len == 0 {
            need_prompt = true;
        }

        if need_prompt {
            // Fresh prompt (after command, WASM exit, intent completion, or new session)
            let shade_active = crate::shade::is_active();
            let shade_just_started = shade_active && !shade_was_active;
            shade_was_active = shade_active;
            let first_prompt = session.prompt_len == 0 || shade_just_started;
            session.reset_input();
            let cwd = get_cwd();
            let path = if cwd.is_empty() { "/" } else { cwd.as_str() };
            let p = alloc::format!("{}> ", path);
            // Route the prompt (and, below, command output) to this terminal
            // even though the default sink is the primary loop, so prompts and
            // results land in the loop the user is typing in, while background
            // debug still goes to the primary. See terminal::write.
            crate::shade::terminal::set_output_redirect(session.terminal_idx);
            kprint!("{}", p);
            crate::shade::terminal::clear_output_redirect();
            session.prompt_len = p.len();
            if crate::shade::is_active() {
                crate::shade::terminal::set_prompt_len(session.prompt_len);
                crate::shade::terminal::set_cursor_pos(
                    crate::shade::terminal::current_line_len());
                if first_prompt {
                    // New window needs full render to show prompt
                    crate::shade::render_frame();
                } else {
                    // Existing window — fast input line update only
                    crate::shade::render_input_line();
                }
            }
            need_prompt = false;
        } else {
            // Resuming session after focus change — sync prompt_len + cursor
            if crate::shade::is_active() {
                crate::shade::terminal::set_prompt_len(session.prompt_len);
                crate::shade::terminal::set_cursor_pos(
                    crate::shade::terminal::current_line_len()
                        .saturating_sub(session.pos.saturating_sub(session.cursor)));
            }
        }

        // Read input into session
        let line = read_line_with_tab(session, vault, session_id);

        if crate::shade::is_active() {
            crate::shade::render_frame();
        }

        let len = match line {
            // Focus / window change: run_loop re-acquires the session;
            // keep prompt state, do not reprint.
            None => continue,
            // Empty Enter: a blank line was entered. Treat it like any
            // completed command (fresh prompt next iteration); otherwise
            // the focus-change `len==0` path would swallow it.
            Some(0) => { need_prompt = true; continue; }
            Some(n) => n,
        };

        let input = match core::str::from_utf8(&session.input_buf[..len]) {
            Ok(s) => s.trim(),
            Err(_) => {
                kprintln!("[npk] invalid UTF-8 input");
                need_prompt = true;
                continue;
            }
        };

        if input == "lock" {
            auth::intent_lock();
            need_prompt = true;
            continue;
        }

        if input == "exit" || input == "quit" {
            // Close this loop's window. Reuse the Mod+Q (CloseWindow) path:
            // sync the session to the terminal buffer, close the focused
            // window, then `continue` so run_loop re-acquires the now-focused
            // terminal's session cleanly, never touching the &mut into
            // SESSIONS that CloseWindow just freed.
            if crate::shade::is_active() {
                sync_session_to_terminal(session);
                crate::shade::handle_action(crate::shade::input::ShadeAction::CloseWindow);
                continue;
            }
            // Serial console: no window to close.
            kprintln!("exit: nothing to close on the serial console");
            need_prompt = true;
            continue;
        }

        // Check if this intent can run on a worker core
        let verb = input.splitn(2, ' ').next().unwrap_or("");
        if !is_core0_intent(verb) && crate::shade::is_active() {
            let term_idx = crate::shade::terminal::active_idx();
            if spawn_intent_on_worker(input, term_idx, session_id) {
                // Worker prints prompt when done via from_intent transition
                continue;
            }
        }

        // Core-0 intent output goes to the loop it was typed in (not the
        // primary debug sink). Synchronous, so the redirect is safe to clear
        // right after — no idle in between where background debug would leak.
        crate::shade::terminal::set_output_redirect(term);
        dispatch_intent(input, vault, session_id);
        crate::shade::terminal::clear_output_redirect();

        if crate::shade::is_active() {
            crate::shade::render_frame();
        }

        // Don't print prompt if dispatch spawned a WASM app (e.g. top)
        let term_idx = crate::shade::terminal::active_idx();
        if !crate::wasm::has_wasm_app(term_idx) {
            need_prompt = true;
        }
    }
}

/// Integer parabolic sine: `x` is a full-circle phase (0..65536), `amp` the
/// peak amplitude. No FPU use, so it is safe to call from kernel context.
fn isin_fixed(x: u16, amp: i64) -> i16 {
    const HALF: u32 = 1 << 15; // 32768 = pi
    let xi = x as u32;
    let (sign, t) = if xi < HALF { (1i64, xi) } else { (-1i64, xi - HALF) };
    let prod = (t as u64) * ((HALF - t) as u64); // <= 2^28, peak at t = 16384
    (((prod as i64) * amp >> 28) * sign) as i16
}

/// `beep [hz]`: play a short test tone through the audio mailbox → HDA driver
/// → speaker, exercising the app → mailbox → driver → hardware path. Needs
/// `audio_hda` running (autostart) to be audible.
fn intent_beep(args: &str) {
    const SR: u32 = 48000;
    const MS: u32 = 150;
    let hz: u32 = args.trim().parse().unwrap_or(440).clamp(50, 12000);
    let frames = (SR * MS / 1000) as usize;
    let step = ((65536u32 * hz / SR) as u16).max(1);
    let mut buf = alloc::vec::Vec::with_capacity(frames * crate::audio::BYTES_PER_FRAME);
    let mut phase: u16 = 0;
    for _ in 0..frames {
        let s = isin_fixed(phase, 6000).to_le_bytes();
        buf.push(s[0]); buf.push(s[1]); // left
        buf.push(s[0]); buf.push(s[1]); // right
        phase = phase.wrapping_add(step);
    }
    if crate::audio::play_oneshot(&buf) {
        kprintln!("beep {} Hz", hz);
    } else {
        kprintln!("beep: no free audio slot (is audio_hda running?)");
    }
}

fn dispatch_intent(input: &str, vault: &'static Mutex<Vault>, session: CapId) {
    if input.is_empty() { return; }

    let mut parts = input.splitn(2, ' ');
    let verb = parts.next().unwrap_or("");
    let args = parts.next().unwrap_or("");

    match verb {
        // Intents requiring READ
        "status" | "info" => {
            if require_cap(vault, &session, Rights::READ, "status") {
                system::intent_status(&vault.lock());
            }
        }
        "top" | "htop" => {
            wasm::intent_run_interactive("top");
        }
        "python" | "py" | "python3" => {
            if require_cap(vault, &session, Rights::EXECUTE, "python") {
                python::intent_python(args, vault, session);
            }
        }
        "cores" | "cpu" => {
            system::intent_cores();
        }
        "akku" | "battery" | "bat" => {
            system::intent_battery();
        }
        "power" | "watt" | "watts" => {
            system::intent_power(args);
        }
        "ec" if args.trim_start().starts_with("watch") => {
            system::intent_ec_watch(args.trim_start().trim_start_matches("watch"));
        }
        "ec" => {
            // A/B switches for power measurements (see drivers/sci.rs).
            match args.split_whitespace().collect::<alloc::vec::Vec<_>>().as_slice() {
                ["gpe", "off"] => kprintln!("  EC-GPE aus: {}", crate::sci::set_ec_gpe(false)),
                ["gpe", "on"] => kprintln!("  EC-GPE an: {}", crate::sci::set_ec_gpe(true)),
                ["mode", "legacy"] => kprintln!("  SCI_EN jetzt {:?}", crate::sci::set_acpi_mode(false)),
                ["mode", "acpi"] => kprintln!("  SCI_EN jetzt {:?}", crate::sci::set_acpi_mode(true)),
                _ => kprintln!("  ec watch [take] [s] | ec gpe on|off | ec mode acpi|legacy"),
            }
        }
        "dsdt" => {
            // `dsdt send <ip> <port>` streams the raw table over TCP (exact
            // bytes, no terminal-mirror ring-overflow); `dsdt full` base64-
            // dumps it to the console; bare `dsdt` dumps battery fields.
            let mut it = args.split_whitespace();
            match it.next() {
                Some("send") => {
                    let ip_s = it.next().unwrap_or("");
                    let port_s = it.next().unwrap_or("");
                    match (parse_ip(ip_s), port_s.parse::<u16>()) {
                        (Some(ip), Ok(port)) if port != 0 => system::intent_dsdt_send(ip, port),
                        _ => kprintln!("[npk] usage: dsdt send <ip> <port>"),
                    }
                }
                Some("full") => system::intent_dsdt_full(),
                _ => system::intent_dsdt(),
            }
        }
        "debug" => {
            // Parse "<ip> <port>" and set the target before spawning debug.wasm.
            // Without arguments, dial the fixed development host below.
            let mut it = args.split_whitespace();
            let ip_s = it.next().unwrap_or("");
            let port_s = it.next().unwrap_or("");
            let (ip, port) = if ip_s.is_empty() && port_s.is_empty() {
                let ip = ((192u32) << 24) | ((168u32) << 16) | ((178u32) << 8) | 97u32;
                kprintln!("[npk] debug → 192.168.178.97:22222 (no args, using default)");
                (ip, 22222u16)
            } else {
                let ip = match parse_ip(ip_s) {
                    Some(a) => ((a[0] as u32) << 24) | ((a[1] as u32) << 16)
                             | ((a[2] as u32) << 8)  |  (a[3] as u32),
                    None => { kprintln!("[npk] Usage: debug <ip> <port>   (e.g. debug 192.168.1.50 22222)"); 0 }
                };
                let port: u16 = port_s.parse().unwrap_or(0);
                (ip, port)
            };
            if ip != 0 && port != 0 {
                crate::wasm::set_debug_target(ip, port);
                wasm::intent_run_background("debug");
            } else if ip != 0 {
                kprintln!("[npk] Usage: debug <ip> <port>");
            }
        }
        "uname" | "version" | "kernel" => {
            system::intent_uname(args);
        }
        "vmx" | "vt-x" => {
            if require_cap(vault, &session, Rights::READ, "vmx") {
                crate::microvm::report();
            }
        }
        "microvm" => {
            let sub = args.trim();
            if sub.is_empty() {
                kprintln!("[microvm] Usage: microvm <test|linux-info>");
                kprintln!("[microvm]   test       — real-mode HLT-loop substrate test");
                kprintln!("[microvm]   linux-info — parse bundled bzImage, print stats");
            } else if sub == "test" {
                if require_cap(vault, &session, Rights::EXECUTE, "microvm test") {
                    match crate::microvm::run_substrate_test() {
                        Ok(outcome) => {
                            let basic = (outcome.exit_reason & 0xFFFF) as u16;
                            let label = match basic {
                                12 => " (HLT)",
                                30 => " (I/O instruction)",
                                33 => " (VM-entry: invalid guest state)",
                                48 => " (EPT violation)",
                                49 => " (EPT misconfiguration)",
                                _ => "",
                            };
                            kprintln!(
                                "[microvm] substrate-test OK — VM-exit reason {}{}",
                                basic, label,
                            );
                            if basic == 30 {
                                let (port, dir_in, size) =
                                    crate::microvm::decode_io_exit_qualification(
                                        outcome.exit_qualification,
                                    );
                                let dir = if dir_in { "IN" } else { "OUT" };
                                let value = outcome.guest_rax & match size {
                                    1 => 0xFF,
                                    2 => 0xFFFF,
                                    4 => 0xFFFF_FFFF,
                                    _ => 0xFF,
                                };
                                kprintln!(
                                    "[microvm]   {} port {:#06x} size={} value={:#x}",
                                    dir, port, size, value,
                                );
                            }
                        }
                        Err(e) => kprintln!("[microvm] substrate-test FAILED: {}", e),
                    }
                }
            } else if sub == "linux-info" {
                if require_cap(vault, &session, Rights::READ, "microvm linux-info") {
                    microvm_linux_info();
                }
            } else if sub == "linux" {
                if require_cap(vault, &session, Rights::EXECUTE, "microvm linux") {
                    microvm_linux(b"", None);
                }
            } else if let Some(rest) = sub.strip_prefix("benchvm") {
                // `microvm benchvm [<MB>]` — launch the microvm in pure-bridge
                // throughput mode: PID-1 wgets <MB> MiB through the nat bridge
                // (no cage/GPU/browser), the local netbench_server reports the
                // rate. Isolates the bridge from the browser userspace.
                if require_cap(vault, &session, Rights::EXECUTE, "microvm benchvm") {
                    let mb: u32 = rest.trim().parse().unwrap_or(1000);
                    microvm_linux(b"", Some(mb));
                }
            } else if let Some(rest) = sub.strip_prefix("shell") {
                // `microvm shell [<line>]`: pre-injects <line> + '\n'
                // into the UART RX FIFO before VMLAUNCH. PID-1 in the
                // guest detects the pending byte via LSR.DR, drains
                // RBR through iopl(3), echoes the line back through
                // the same UART, then powers off. End-to-end
                // inject-console round trip.
                if require_cap(vault, &session, Rights::EXECUTE, "microvm shell") {
                    let line = rest.trim();
                    let line = if line.is_empty() { "hi" } else { line };
                    let mut buf = alloc::vec::Vec::with_capacity(line.len() + 1);
                    buf.extend_from_slice(line.as_bytes());
                    buf.push(b'\n');
                    microvm_linux(&buf, None);
                }
            } else {
                kprintln!("[microvm] unknown subcommand: '{}'", sub);
                kprintln!("[microvm] available: test, linux-info, linux, shell");
            }
        }
        "browser" => {
            // User-facing alias for `microvm linux` — launches the
            // LibreWolf bundle. `microvm` stays as the dev/test surface
            // (substrate self-test + bare-boot diag); `browser` is what
            // the user types or what drun spawns.
            if require_cap(vault, &session, Rights::EXECUTE, "browser") {
                microvm_linux(b"", None);
            }
        }
        "caps" | "capabilities" => {
            if require_cap(vault, &session, Rights::READ, "caps") {
                system::intent_caps(&vault.lock());
            }
        }
        "audit" => {
            if require_cap(vault, &session, Rights::AUDIT, "audit") {
                system::intent_audit();
            }
        }

        "disk" | "blk" => {
            let sub = args.trim();
            if sub.is_empty() || sub == "info" {
                if require_cap(vault, &session, Rights::READ, "disk") {
                    fs::intent_disk_info();
                }
            } else if sub.starts_with("read ") || sub == "read" {
                if require_cap(vault, &session, Rights::READ, "disk read") {
                    fs::intent_disk_read(sub.strip_prefix("read").unwrap_or("").trim());
                }
            } else if sub.starts_with("write ") || sub == "write" {
                if require_cap(vault, &session, Rights::WRITE, "disk write") {
                    fs::intent_disk_write(sub.strip_prefix("write").unwrap_or("").trim());
                }
            } else {
                kprintln!("[npk] Usage: disk [info|read <sector>|write <sector> <text>]");
            }
        }

        "store" | "save" => {
            if require_cap(vault, &session, Rights::WRITE, "store") {
                fs::intent_store(args, session);
            }
        }
        "fetch" | "load" => {
            if require_cap(vault, &session, Rights::READ, "fetch") {
                fs::intent_fetch(args);
            }
        }
        "cat" | "show" | "print" | "type" => {
            if require_cap(vault, &session, Rights::READ, "cat") {
                fs::intent_cat(args);
            }
        }
        "grep" | "search" | "find" => {
            if require_cap(vault, &session, Rights::READ, "grep") {
                fs::intent_grep(args);
            }
        }
        "head" => {
            if require_cap(vault, &session, Rights::READ, "head") {
                fs::intent_head(args);
            }
        }
        "wc" | "count" => {
            if require_cap(vault, &session, Rights::READ, "wc") {
                fs::intent_wc(args);
            }
        }
        "hexdump" | "hex" | "xxd" => {
            if require_cap(vault, &session, Rights::READ, "hexdump") {
                fs::intent_hexdump(args);
            }
        }

        "delete" | "rm" | "remove" => {
            if require_cap(vault, &session, Rights::WRITE, "delete") {
                fs::intent_delete(args);
            }
        }
        "mkdir" => {
            if require_cap(vault, &session, Rights::WRITE, "mkdir") {
                fs::intent_mkdir(args);
            }
        }
        "rmdir" => {
            if require_cap(vault, &session, Rights::WRITE, "rmdir") {
                fs::intent_rmdir(args);
            }
        }
        "list" | "ls" | "objects" => {
            if require_cap(vault, &session, Rights::READ, "list") {
                fs::intent_list(args);
            }
        }
        "fsinfo" | "fs" => {
            if require_cap(vault, &session, Rights::READ, "fsinfo") {
                fs::intent_fsinfo();
            }
        }

        "resolve" | "dns" => {
            if require_cap(vault, &session, Rights::READ, "resolve") {
                net::intent_resolve(args);
            }
        }
        "uptime" => {
            system::intent_uptime();
        }
        "lspci" | "pci" => {
            if require_cap(vault, &session, Rights::READ, "lspci") {
                system::intent_lspci(args);
            }
        }
        "lsusb" | "usb" => {
            if require_cap(vault, &session, Rights::READ, "lsusb") {
                system::intent_lsusb();
            }
        }
        "mouse" => {
            system::intent_mouse(args);
        }
        "beep" | "test-audio" => {
            intent_beep(args);
        }
        "volume" | "vol" => {
            let a = args.trim();
            if a.is_empty() {
                kprintln!("volume: {}%", crate::audio::get_volume());
            } else if let Ok(n) = a.parse::<u32>() {
                crate::audio::set_volume(n.min(100) as u8);
                kprintln!("volume: {}%", crate::audio::get_volume());
            } else {
                kprintln!("volume: usage: volume [0-100]");
            }
        }
        "dmesg" | "bootlog" => {
            system::intent_dmesg(args);
        }
        "gpu" => {
            system::intent_gpu(args);
        }
        "shade" => {
            system::intent_shade(args);
        }
        "history" => {
            // Printing is a read; clearing rewrites a stored object.
            if args.trim().is_empty() {
                system::intent_history(args);
            } else if require_cap(vault, &session, Rights::WRITE, "history") {
                system::intent_history(args);
            }
        }
        "time" | "clock" | "date" => {
            if require_cap(vault, &session, Rights::READ, "time") {
                system::intent_time();
            }
        }
        "traceroute" | "trace" => {
            if require_cap(vault, &session, Rights::EXECUTE, "traceroute") {
                net::intent_traceroute(args);
            }
        }
        "netstat" | "connections" => {
            if require_cap(vault, &session, Rights::READ, "netstat") {
                net::intent_netstat();
            }
        }
        "http" | "curl" | "wget" => {
            if require_cap(vault, &session, Rights::EXECUTE, "http") {
                http::intent_http(args);
            }
        }
        "https" => {
            if require_cap(vault, &session, Rights::EXECUTE, "https") {
                http::intent_https(args);
            }
        }
        "netbench" => {
            if require_cap(vault, &session, Rights::EXECUTE, "netbench") {
                http::intent_netbench(args);
            }
        }
        // `net window <KB>` / `net window auto`: force or release the receive
        // window. On a saturated path RTT = window / rate, so a single measurement
        // cannot tell whether the link or we are the limit. Sweep several windows:
        // if throughput grows with the window, we were the limit; if only the RTT
        // rises, the link is.
        "window" => {
            let a = args.trim();
            if a.is_empty() {
                let f = crate::net::tcp::rcv_window_force();
                let (w, srtt, cap) = crate::net::tcp::window_diag();
                if f > 0 {
                    kprintln!("[npk] fenster: fest {} KB", f / 1024);
                } else {
                    kprintln!("[npk] fenster: automatisch (DRS — was die Anwendung je RTT abholt)");
                }
                kprintln!("[npk]   zuletzt angeboten {} KB, Deckel {} KB, srtt {} Takte (={} ms)",
                          w / 1024, cap / 1024, srtt, srtt * 10);
            } else if a == "auto" {
                crate::net::tcp::set_rcv_window_force(0);
                kprintln!("[npk] fenster: automatisch");
            } else if let Ok(kb) = a.parse::<u32>() {
                let b = kb.saturating_mul(1024);
                crate::net::tcp::set_rcv_window_force(b);
                kprintln!("[npk] fenster: fest {} KB — gilt ab der naechsten Quittung", kb);
            } else {
                kprintln!("usage: window <KB> | window auto");
            }
        }
        // `nic`: the USB NIC's own counters, on demand. A high `rx_missed`
        // (chip FIFO overflow) means frames arrived and we did not collect them;
        // zero with a small `rx_pkts` means nothing came over the wire.
        "nic" => {
            if require_cap(vault, &session, Rights::AUDIT, "nic") {
                if crate::xhci::nic_attached() {
                    kprintln!("[npk] NIC USB link: {}",
                              crate::xhci::nic_link_speed_str());
                    crate::drivers::rtl8153::log_link_diag();
                    crate::drivers::rtl8153::dump_tally("jetzt");
                    // xHCI bulk-IN completion codes. They answer what the
                    // chip tally leaves open: why the controller reports few
                    // completions although the ring is full and the chip does
                    // not overflow.
                    let (ok, short, other, last, resid) =
                        crate::xhci::nic_take_cc();
                    let n = (ok + short + other).max(1);
                    kprintln!("[npk] xhci bulk-IN: {} ok, {} short, {} sonstige (letzter cc {}) | Rest je Vollzug {} B von {}",
                              ok, short, other, last, resid / n, 16384);
                    if args.trim() == "reset" {
                        crate::drivers::rtl8153::tally_reset();
                        kprintln!("[npk] rtl8153: Zaehler auf null");
                    }
                } else {
                    kprintln!("[npk] keine USB-NIC angesteckt");
                }
            }
        }
        "ping" => {
            if require_cap(vault, &session, Rights::EXECUTE, "ping") {
                net::intent_ping(args);
            }
        }
        "net" | "ifconfig" => {
            if require_cap(vault, &session, Rights::READ, "net") {
                net::intent_net_info();
            }
        }
        "wlan" => {
            // `set`/`unset` edit the config object, so they need WRITE — the
            // report itself is a read.
            let a = args.trim();
            if a.starts_with("set") || a.starts_with("unset") {
                if require_cap(vault, &session, Rights::WRITE, "wlan") {
                    net::intent_wlan_set(a, session);
                }
            } else if require_cap(vault, &session, Rights::READ, "wlan") {
                net::intent_wlan(args);
            }
        }

        "mtrr" | "fbinfo" | "gfx" => {
            if require_cap(vault, &session, Rights::READ, "mtrr") {
                crate::framebuffer::dump_memory_type();
            }
        }

        "usbnet" => {
            if require_cap(vault, &session, Rights::READ, "nic") {
                if crate::xhci::nic_attached() {
                    crate::xhci::nic_dump_ports();
                } else {
                    kprintln!("[npk] nic: no USB NIC attached");
                }
            }
        }
        "xhci" => {
            if require_cap(vault, &session, Rights::READ, "xhci") {
                crate::xhci::scan_all_controllers();
            }
        }
        "tbtrain" => {
            if require_cap(vault, &session, Rights::WRITE, "tbtrain") {
                crate::xhci::tb_warm_reset();
            }
        }

        "dhcp" => {
            if require_cap(vault, &session, Rights::WRITE, "dhcp") {
                // Re-run DHCP on the active NIC (the boot-time configure() ran
                // before a WiFi link existed). Brings up IP over `wlan` once the
                // 4-way completed and it's the active interface.
                kprintln!("[npk] DHCP: requesting a lease...");
                // Typed by hand, so waiting for the answer is what the user
                // asked for — unlike the ~1 Hz link tick, which must never take
                // the terminal away. Same state machine either way.
                if crate::net::dhcp::run_blocking(5000) {
                    kprintln!("[npk] DHCP: configured — run `net` for the address");
                } else if crate::net::dhcp::is_running() {
                    // Out of patience, not out of exchange: it keeps going on
                    // the link tick, so saying "failed" here would be a lie.
                    kprintln!("[npk] DHCP: still trying — run `net` in a moment");
                } else {
                    kprintln!("[npk] DHCP: failed (no offer)");
                }
            }
        }

        "run" | "exec" => {
            if require_cap(vault, &session, Rights::EXECUTE, "run") {
                wasm::intent_run(args);
            }
        }
        "forge" => {
            if require_cap(vault, &session, Rights::EXECUTE, "forge") {
                forge::intent_forge(args, vault, session);
            }
        }

        "driver" => {
            if require_cap(vault, &session, Rights::EXECUTE, "driver") {
                wasm::intent_run_driver(args);
            }
        }

        "slab" => {
            if require_cap(vault, &session, Rights::AUDIT, "slab") {
                let sub = args.trim();
                if sub == "test" {
                    crate::gpu::ggtt_slab::self_test();
                } else {
                    crate::gpu::ggtt_slab::dump_stats();
                }
            }
        }

        "gc" => {
            if require_cap(vault, &session, Rights::AUDIT, "gc") {
                match crate::storage::npkfs::fs::gc() {
                    Ok(s) if s.skipped =>
                        kprintln!("[npk] gc: skipped — streaming write in progress, try again later"),
                    Ok(s) => kprintln!("[npk] gc: kept {}, removed {}", s.kept, s.removed),
                    Err(e) => kprintln!("[npk] gc error: {:?}", e),
                }
            }
        }

        "halt" | "shutdown" | "poweroff" => {
            if require_cap(vault, &session, Rights::EXECUTE, "halt") {
                system::intent_halt();
            }
        }
        "reboot" | "restart" => {
            if require_cap(vault, &session, Rights::EXECUTE, "reboot") {
                system::intent_reboot();
            }
        }

        "update" | "upgrade" => {
            if require_cap(vault, &session, Rights::EXECUTE, "update") {
                update::intent_update(args);
            }
        }

        "install" => {
            if require_cap(vault, &session, Rights::EXECUTE, "install") {
                install::intent_install(args);
            }
        }
        "uninstall" => {
            if require_cap(vault, &session, Rights::EXECUTE, "uninstall") {
                install::intent_uninstall(args);
            }
        }
        "modules" => {
            install::intent_modules();
        }
        "assets" | "asset" => {
            install::intent_assets();
        }

        "wallpaper" | "wp" => {
            wallpaper::intent_wallpaper(args);
        }

        // Reading the trust store is harmless; changing it is not. WRITE
        // gates the whole intent rather than just `add`/`remove`, because
        // the listing is also the thing that tells you what to remove.
        "cert" | "certs" => {
            let ro = args.split_whitespace().next().unwrap_or("list");
            let needed = match ro {
                "list" | "ls" | "show" | "info" => Rights::READ,
                _ => Rights::WRITE,
            };
            if require_cap(vault, &session, needed, "cert") {
                cert::intent_cert(args);
            }
        }

        "theme" => {
            intent_theme(args);
        }

        "passwd" | "password" | "passphrase" => {
            auth::intent_passwd();
        }

        "shell" | "npk-shell" => {
            if require_cap(vault, &session, Rights::EXECUTE, "shell") {
                crate::shell::serve_one(vault, session);
            }
        }

        "set" => {
            if require_cap(vault, &session, Rights::WRITE, "set") {
                system::intent_set(args);
            }
        }
        "unset" => {
            if require_cap(vault, &session, Rights::WRITE, "unset") {
                system::intent_unset(args);
            }
        }
        "get" => {
            if require_cap(vault, &session, Rights::READ, "get") {
                system::intent_get(args);
            }
        }
        "config" | "settings" => {
            if require_cap(vault, &session, Rights::READ, "config") {
                system::intent_config();
            }
        }

        "cd" => {
            intent_cd(args);
        }
        "pwd" => {
            let cwd = get_cwd();
            if cwd.is_empty() { kprintln!("/"); } else { kprintln!("/{}", cwd); }
        }

        "clear" | "cls" => {
            if crate::shade::is_active() {
                // Shade mode: clear terminal buffer and re-render focused window
                crate::shade::terminal::clear();
                crate::shade::render_frame();
            } else {
                crate::framebuffer::clear();
            }
            // ANSI clear to serial
            let serial = crate::serial::SERIAL.lock();
            for &b in b"\x1B[2J\x1B[H" {
                serial.write_byte(b);
            }
        }

        // Unrestricted intents (informational)
        "help" | "?" => system::intent_help_topic(args.trim()),
        "echo" => system::intent_echo(args),
        "think" => system::intent_think(args),
        "about" => system::intent_about(),
        "philosophy" => system::intent_philosophy(),

        _ => {
            // Implicit-run: if `<cmd>` matches a WASM module under
            // sys/wasm/, execute it with `args`. Makes any installed
            // app callable by name, same UX as built-in intents.
            // Hardcoded dispatcher entries above (top/wallpaper/...)
            // still win for apps that need special run semantics
            // (interactive, background, etc.).
            if crate::npkfs::exists(&alloc::format!("sys/wasm/{}", verb)) {
                if require_cap(vault, &session, Rights::EXECUTE, verb) {
                    wasm::intent_run(input);
                }
            } else {
                kprintln!("[npk] Unknown intent: '{}'", input);
                kprintln!("[npk] Try 'help' for available intents.");
            }
        }
    }
}

/// `microvm linux-info` handler: fetch the bundled bzImage from npkFS,
/// parse the Linux Boot Protocol setup header, print stats. Read-only, no
/// VM activity. The asset lands at `sys/microvm/linux-virt.bzImage` on a
/// fresh install (see install_data/assets).
fn microvm_linux_info() {
    const BZIMAGE_PATH: &str = "sys/microvm/linux-virt.bzImage";

    let bytes = match crate::npkfs::fetch(BZIMAGE_PATH) {
        Ok((b, _hash)) => b,
        Err(e) => {
            kprintln!("[microvm] cannot read {}: {:?}", BZIMAGE_PATH, e);
            kprintln!("[microvm] reinstall from USB to seed bundled assets");
            return;
        }
    };

    let header = match crate::microvm::linux::bzimage::parse_header(&bytes) {
        Ok(h) => h,
        Err(e) => {
            kprintln!("[microvm] parse failed: {}", e);
            return;
        }
    };

    let setup_size = crate::microvm::linux::bzimage::setup_section_size(&header);
    let prot_size  = crate::microvm::linux::bzimage::protected_kernel_size(&header);
    // Reads of #[repr(packed)] fields go via local copies to avoid
    // unaligned-reference UB.
    let version = header.version;
    let setup_sects = header.setup_sects;
    let syssize = header.syssize;
    let code32_start = header.code32_start;
    let init_size = header.init_size;
    let kernel_alignment = header.kernel_alignment;
    let relocatable = header.relocatable_kernel;
    let xloadflags = header.xloadflags;
    let pref_address = header.pref_address;

    kprintln!("[microvm] Linux bzImage at {}:", BZIMAGE_PATH);
    kprintln!("[microvm]   bzImage size       {} bytes ({} KB)",
              bytes.len(), bytes.len() / 1024);
    kprintln!("[microvm]   protocol           {}.{:02}",
              version >> 8, version & 0xFF);
    kprintln!("[microvm]   setup_sects        {} (= {} bytes incl. bootsector)",
              setup_sects, setup_size);
    kprintln!("[microvm]   syssize            {:#x} paragraphs (= {} KB)",
              syssize, prot_size / 1024);
    kprintln!("[microvm]   code32_start       {:#010x}", code32_start);
    kprintln!("[microvm]   init_size          {:#x} bytes ({} MB)",
              init_size, init_size / (1024 * 1024));
    kprintln!("[microvm]   kernel_alignment   {:#x}", kernel_alignment);
    kprintln!("[microvm]   relocatable        {}", relocatable);
    kprintln!("[microvm]   xloadflags         {:#06x}", xloadflags);
    kprintln!("[microvm]   pref_address       {:#018x}", pref_address);

    if (init_size as u64) > 64 * 1024 * 1024 {
        kprintln!("[microvm]   WARNING: init_size > 64 MB EPT window —");
        kprintln!("[microvm]   the launcher (12.1.1c-3b2) will need a bigger window.");
    }
}

/// Entry for the host function `npk_run_intent("browser")`, so a launcher
/// can spawn the browser microvm without the Core-0 shell prompt. Safe from
/// a worker when the dedicated VM core is active: `vm_open` only queues a
/// PENDING_VM request (atomic + mutex), and VMXON / VMRUN happen later on
/// that core via `vm_core_serve`. On the cooperative path (≤ 2 cores)
/// `vm_open` needs BSP state and fails from a worker; the host function
/// then returns -1 and the user must type `browser` at a Core-0 prompt.
pub fn launch_browser() {
    microvm_linux(b"", None);
}

/// `microvm linux` / `microvm shell` handler: fetch the bundled bzImage
/// from npkFS and boot it with a cmdline and `inject` bytes. The guest's
/// serial output on 0x3F8 is trapped via the I/O bitmap and reflected as
/// `[guest] <line>`. `inject` is empty for plain `linux`; for `shell <line>`
/// it is the line + '\n' preloaded into the UART RX FIFO so PID-1 can echo
/// it back.
fn microvm_linux(inject: &[u8], bench_mb: Option<u32>) {
    const BZIMAGE_PATH: &str = "sys/microvm/linux-virt.bzImage";
    const INITRAMFS_PATH: &str = "sys/microvm/initramfs.cpio.gz";
    /// Optional userspace bundle (Alpine minirootfs + busybox + browser
    /// stack), built by `microvm-userspace/build.sh` and distributed via OTA.
    /// If present it replaces the minimal PID-1-only initramfs; it carries our
    /// PID-1 at /init too, which then execs /bin/sh from the bundle's busybox.
    const USERSPACE_PATH: &str = "sys/microvm/userspace.cpio.gz";
    // Linux boot protocol cmdline.
    //
    // `earlycon=uart8250,io,0x3f8,115200n8`: early console straight to COM1,
    // without the 8250 detection probe (which fails against our minimal UART
    // emulation). Alpine's virt kernel has CONFIG_SERIAL_EARLYCON but not
    // CONFIG_EARLY_PRINTK, so `earlyprintk=` would be ignored.
    // `console=ttyS0,115200`: the regular 8250 console once full init runs.
    // `panic=1`: halt on panic, no reboot loop. `nokaslr`: predictable load
    // addresses for the hypervisor side. `acpi=off tsc=reliable`: skip probes
    // that misbehave without real silicon (there are no ACPI tables).
    //
    // `tsc_early_khz=<host TSC kHz>` skips Linux's PIT-based TSC calibration,
    // which hangs on AMD-V (no CPUID 0x15, and our PIT emulation returns 0).
    // It must be the real host TSC frequency: the guest TSC is the host TSC,
    // and a too-low hint makes the guest clock run faster than jiffies, which
    // breaks pthread/futex timed waits in the browser. Intel advertises the
    // frequency in CPUID 0x15 and Linux ignores the hint there.
    //
    // The guest has no RTC, so its clock would start in 2000 and every TLS
    // certificate would be "not yet valid". `nopeektime=<epoch>` passes the
    // host wall clock; PID-1 sets it with `date -s` before starting cage.
    use core::fmt::Write;
    let tsc_khz = crate::interrupts::tsc_freq() / 1000;
    let mut s = String::new();
    // Boot output stays verbose (no `quiet loglevel=3`) so a guest panic or
    // wedge during early init surfaces on the host console.
    //
    // maxcpus: an AP that Linux enumerates but that never responds hangs the
    // cpuhp bring-up, so APs are onlined only where the host spawns a
    // responding AP vCPU on INIT-SIPI (`smp_ap_active()`). The count scales
    // with the host's worker cores (`guest_vcpus()`).
    let maxcpus: u8 = if crate::microvm::cpu::smp_ap_active() {
        crate::microvm::cpu::guest_vcpus()
    } else {
        1
    };
    let _ = write!(
        s,
        "earlycon=uart8250,io,0x3f8,115200n8 console=ttyS0,115200 \
panic=1 nokaslr acpi=off tsc=reliable idle=halt \
tsc_early_khz={} devtmpfs.mount=1 maxcpus={}",
        tsc_khz, maxcpus,
    );
    // `nolapic` unless the LAPIC is actually emulated for this host. LAPIC
    // emulation is SVM-only (svm/lapic.rs); on Intel/VMX there is none, so the
    // guest must fall back to the PIT IRQ0 the VMX path injects — otherwise it
    // programs the LAPIC timer and hangs waiting for a tick that never comes
    // (cage/Wayland never starts). See cpu::guest_lapic_active().
    if !crate::microvm::cpu::guest_lapic_active() {
        let _ = write!(s, " nolapic");
    }
    // The I/O APIC is in the MP table only when it is emulated; otherwise the
    // guest stays on the 8259.
    if !crate::microvm::cpu::guest_ioapic_active() {
        let _ = write!(s, " noapic");
    }
    if let Some(epoch) =
        crate::rtc::read_unix_time().or_else(crate::net::ntp::unix_time)
    {
        let _ = write!(s, " nopeektime={}", epoch);
    }
    // Guest-side diagnostic probe: PID-1 forks a 1 s busybox loop that dumps the
    // guest's own view (per-vCPU busy/softirq %, socket cwnd/rtt/retrans, softnet
    // drops/squeeze) to /dev/kmsg → our console as `[gdiag]`.
    //
    // Off by default: the guest writes through the emulated 8250, one VM exit
    // per byte, and each line is a blocking UART write in the exit handler on
    // the BSP vCPU fiber that also services the virtio-net doorbell, so the
    // probe slows the network path it measures. Enable with
    // `set microvm_gdiag on`.
    if crate::config::get("microvm_gdiag")
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("on"))
    {
        let _ = write!(s, " nopeekgdiag=1");
    }
    // Diagnostic pure-bridge throughput run: PID-1 sees `nopeekbench=` and runs
    // a busybox wget loop through the nat bridge instead of cage/browser.
    if let Some(mb) = bench_mb {
        // `nopeekbenchhost=` names the bench server. The gateway is only right
        // under QEMU (slirp's server is the gateway); on real hardware it is the
        // router. So the server comes from `sys/config/benchhost` and falls back
        // to a hardcoded address (temporary).
        const BENCH_FALLBACK: [u8; 4] = [192, 168, 178, 97];
        let host = crate::config::get("benchhost")
            .and_then(|v| parse_ip(v.trim()))
            .unwrap_or(BENCH_FALLBACK);
        kprintln!("[microvm] benchvm target {}.{}.{}.{}:80 ({} MB)",
            host[0], host[1], host[2], host[3], mb);
        let _ = write!(
            s, " nopeekbenchhost={}.{}.{}.{} nopeekbench={}",
            host[0], host[1], host[2], host[3], mb,
        );
    }
    let cmdline: alloc::vec::Vec<u8> = s.into_bytes();
    let cmdline: &[u8] = &cmdline;

    kprintln!("[microvm] loading bzImage from {}...", BZIMAGE_PATH);
    let bytes = match crate::npkfs::fetch(BZIMAGE_PATH) {
        Ok((b, _hash)) => b,
        Err(e) => {
            kprintln!("[microvm] cannot read {}: {:?}", BZIMAGE_PATH, e);
            kprintln!("[microvm] reinstall from USB to seed bundled assets");
            return;
        }
    };

    // Userspace-bundle delivery, newest path first:
    //
    //  1. squashfs bundle present → boot the tiny PID-1-only initramfs.
    //     PID-1 mounts the .sqfs from /dev/vdb (the slot-5 read-only
    //     virtio-blk device reads `sys/microvm/userspace.sqfs` directly,
    //     never unpacked into RAM) and chroots into it. The bundle stays
    //     compressed on the virtual disk.
    //
    //  2. legacy cpio bundle present (no sqfs) → use it as the initramfs
    //     (embeds our PID-1 at /init). The whole tree is unpacked into
    //     tmpfs. Kept until the sqfs bundle is the only shipped form.
    //
    //  3. neither → minimal PID-1-only substrate.
    const USERSPACE_SQFS_PATH: &str = "sys/microvm/userspace.sqfs";
    let have_sqfs = crate::npkfs::exists(USERSPACE_SQFS_PATH);
    let initramfs = if have_sqfs {
        kprintln!("[microvm] sqfs bundle at {} — booting tiny PID-1 (mounts /dev/vdb)",
                  USERSPACE_SQFS_PATH);
        match crate::npkfs::fetch(INITRAMFS_PATH) {
            Ok((b, _hash)) => {
                kprintln!("[microvm] loaded initramfs ({} bytes)", b.len());
                Some(b)
            }
            Err(e) => {
                kprintln!("[microvm] no initramfs at {}: {:?} — booting without",
                          INITRAMFS_PATH, e);
                None
            }
        }
    } else {
        match crate::npkfs::fetch(USERSPACE_PATH) {
            Ok((b, _hash)) => {
                kprintln!("[microvm] loaded legacy cpio userspace bundle ({} bytes) from {}",
                          b.len(), USERSPACE_PATH);
                Some(b)
            }
            Err(_) => match crate::npkfs::fetch(INITRAMFS_PATH) {
                Ok((b, _hash)) => {
                    kprintln!("[microvm] loaded initramfs ({} bytes)", b.len());
                    Some(b)
                }
                Err(e) => {
                    kprintln!("[microvm] no initramfs at {}: {:?} — booting without",
                              INITRAMFS_PATH, e);
                    kprintln!("[microvm] reinstall from USB to seed bundled assets");
                    None
                }
            }
        }
    };

    kprintln!("[microvm] launching Linux ({} bytes, cmdline: {:?})",
              bytes.len(),
              core::str::from_utf8(cmdline).unwrap_or("?"));

    // Non-blocking: open the VM (one-time substrate + guest-image setup,
    // copies the bzImage into guest RAM so `bytes` can drop here), register
    // it, return. The Core-0 event loop then drives bounded slices via
    // `vm_poll_slice`, rendering Shade between them, instead of this call
    // blocking Core 0 until guest exit. Guest-exit logging happens in
    // vm_poll_slice when the slice that observes the exit completes.
    match crate::microvm::vm_open(&bytes, cmdline, initramfs.as_deref(), inject) {
        Ok(()) => {
            // Give the guest its own tiled Surface window (tiling
            // invariant: never fullscreen). virtio-gpu FLUSH renders
            // into it; shade composites it like any window; the
            // teardown path closes it on guest exit / window close.
            // Without a compositor (serial-only boot) we stay unbound
            // and virtio-gpu falls back to the fullscreen blit.
            match crate::shade::create_surface_window("microvm") {
                Some(wid) => {
                    crate::microvm::vm_bind_window(wid.0);
                    // Focus the guest window on launch: pointer and
                    // keyboard forwarding only run in the Surface-focused
                    // branch, so otherwise the guest gets no input until the
                    // user focuses the tile. The spawning shell stays
                    // reachable via Mod-focus / Mod+number.
                    crate::shade::focus_window(wid);
                    kprintln!("[microvm] guest running — window {} (focused)", wid.0);
                }
                None => kprintln!("[microvm] guest running (no compositor; serial only)"),
            }
        }
        Err(e) => kprintln!("[microvm] launch FAILED: {:?} (len={}, empty={})", e, e.len(), e.is_empty()),
    }
}

fn require_cap(vault: &Mutex<Vault>, cap_id: &CapId, rights: Rights, intent: &str) -> bool {
    let v = vault.lock();
    match v.check(cap_id, rights) {
        Ok(_) => true,
        Err(e) => {
            kprintln!("[npk] DENIED: '{}' requires {:?} — {}", intent, rights, e);
            false
        }
    }
}

fn intent_theme(args: &str) {
    let mode = args.trim();
    match mode {
        "" | "show" | "status" => {
            let cur = crate::config::get("theme").unwrap_or_else(|| alloc::string::String::from("auto"));
            kprintln!("[npk] theme: {}", cur);
            kprintln!("[npk] Usage: theme <dark|light|auto>");
        }
        "dark" | "light" | "auto" => {
            crate::config::set("theme", mode);
            crate::shade::widgets::refresh_all_scenes();
            // Full redraw (not just request_render) so terminal windows
            // repaint with the new Surface/OnSurface colors too — they
            // resolve the theme at render time and the input-line cache
            // must be invalidated.
            crate::shade::force_redraw();
            kprintln!("[npk] theme: {}", mode);
        }
        _ => {
            kprintln!("[npk] Usage: theme <dark|light|auto>");
        }
    }
}

fn intent_cd(args: &str) {
    let raw = args.trim();

    if raw.is_empty() || raw == "~" {
        set_cwd(&home_dir());
        return;
    }

    if raw == "/" {
        set_cwd("");
        return;
    }

    let target = raw.trim_end_matches('/');

    if target == ".." {
        let cwd = get_cwd();
        match cwd.rfind('/') {
            Some(idx) => set_cwd(&cwd[..idx]),
            None => set_cwd(""),
        }
        return;
    }

    // Resolve path and verify it exists as a directory
    let resolved = resolve_path(target);

    // Root always exists
    if resolved.is_empty() {
        set_cwd("");
        return;
    }

    use crate::npkfs::object::EntryKind;
    match crate::npkfs::fs::stat(&resolved) {
        Ok(Some(s)) if s.kind == EntryKind::Dir => set_cwd(&resolved),
        Ok(Some(_)) => kprintln!("[npk] '{}': not a directory", target),
        Ok(None)    => kprintln!("[npk] '{}': not found", target),
        Err(e)      => kprintln!("[npk] cd error: {:?}", e),
    }
}

/// Re-export public API for main.rs
pub use wallpaper::apply_startup_wallpaper;
pub use fs::crypto_bench;

/// Create initial directory structure and set cwd to home.
///
/// Lays down the standard user tree used by loft's sidebar
/// (`Home / Documents / Downloads / Pictures / Projects / Trash`) and the
/// wallpaper subsystem. Idempotent: re-running it on a populated home
/// writes nothing.
pub fn setup_home() {
    let home = home_dir();
    ensure_parents(&home);
    // Sidebar-aligned standard subdirs. The wallpapers dir lives
    // under `pictures/` to match `wallpaper_dir()` in `intent::wallpaper`.
    for sub in &["documents", "downloads", "pictures", "pictures/wallpapers", "projects", ".trash"] {
        ensure_parents(&alloc::format!("{}/{}", home, sub));
    }
    set_cwd(&home);
}

/// Expose CWD for npk-shell.
pub fn get_cwd_for_shell() -> String {
    get_cwd()
}

/// Print the active terminal's command history.
pub fn print_active_history() {
    let term = crate::shade::terminal::active_idx();
    let lines = SESSIONS.lock().get(&term).map(|s| s.history.lines.clone());
    let Some(lines) = lines.filter(|l| !l.is_empty()) else {
        kprintln!("(no history)");
        return;
    };
    for (i, line) in lines.iter().enumerate() {
        kprintln!("  {:3}  {}", i + 1, line);
    }
}

/// `history clear` — drop the stored log and every window's ring.
pub fn clear_all_history() {
    let stored = history::clear();
    session_op(0, SessionOp::ClearHistory);
    kprintln!("[npk] history cleared ({} stored)", stored);
}

/// Execute an intent from remote shell (dispatch without the loop).
#[allow(dead_code)]
pub fn dispatch_for_shell(input: &str, vault: &'static Mutex<Vault>, session_id: CapId) {
    dispatch_intent(input, vault, session_id);
}
