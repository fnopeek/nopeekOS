//! Intent Loop
//!
//! Not a shell. Takes intents, not commands.
//! Every intent requires a valid capability token.

mod auth;
mod fs;
pub(crate) mod http;
mod net;
mod system;
mod update;
pub mod install;
mod wallpaper;
mod wasm;

use crate::capability::{CapId, Vault, Rights};
use crate::{kprint, kprintln, serial};
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use spin::Mutex;

const INPUT_BUF_SIZE: usize = 512;

// -- Command history --
const HIST_MAX: usize = 32;
const HIST_LINE: usize = 256;

pub(crate) struct History {
    lines: [[u8; HIST_LINE]; HIST_MAX],
    lens: [usize; HIST_MAX],
    count: usize,
    cursor: usize,
}

impl History {
    fn push(&mut self, line: &[u8]) {
        if line.is_empty() { return; }
        let len = line.len().min(HIST_LINE);
        // Skip duplicate of last entry
        if self.count > 0 {
            let last = (self.count - 1) % HIST_MAX;
            if self.lens[last] == len && self.lines[last][..len] == line[..len] {
                self.cursor = self.count;
                return;
            }
        }
        let idx = self.count % HIST_MAX;
        self.lines[idx][..len].copy_from_slice(&line[..len]);
        self.lens[idx] = len;
        self.count += 1;
        self.cursor = self.count;
    }

    fn up(&mut self) -> Option<(&[u8], usize)> {
        if self.count == 0 || self.cursor == 0 { return None; }
        let start = if self.count > HIST_MAX { self.count - HIST_MAX } else { 0 };
        if self.cursor <= start { return None; }
        self.cursor -= 1;
        let idx = self.cursor % HIST_MAX;
        Some((&self.lines[idx], self.lens[idx]))
    }

    fn down(&mut self) -> Option<(&[u8], usize)> {
        if self.cursor >= self.count { return None; }
        self.cursor += 1;
        if self.cursor >= self.count {
            return None; // back to empty line
        }
        let idx = self.cursor % HIST_MAX;
        Some((&self.lines[idx], self.lens[idx]))
    }

    fn reset_cursor(&mut self) {
        self.cursor = self.count;
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
            history: History {
                lines: [[0; HIST_LINE]; HIST_MAX],
                lens: [0; HIST_MAX],
                count: 0,
                cursor: 0,
            },
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

/// Per-window sessions, indexed by terminal_idx.
// SAFETY: only accessed from Core 0 (event dispatcher owns all session state)
static mut SESSIONS: BTreeMap<u8, Box<IntentSession>> = BTreeMap::new();

/// Raw pointer access to SESSIONS (avoids static_mut_refs lint).
fn sessions_ptr() -> *mut BTreeMap<u8, Box<IntentSession>> {
    core::ptr::addr_of_mut!(SESSIONS)
}

/// Create a new session for the given terminal (idempotent).
pub fn create_session(terminal_idx: u8) {
    // SAFETY: Core 0 only
    unsafe {
        if (*sessions_ptr()).contains_key(&terminal_idx) { return; }
        (*sessions_ptr()).insert(terminal_idx, Box::new(IntentSession::new(terminal_idx)));
    }
    // New sessions always land in the user's home directory. Inheriting
    // from the focused terminal's cwd was confusing — opening a fresh
    // window from deep inside a project shouldn't drop you back into
    // the same hole.
    CWDS.lock().entry(terminal_idx).or_insert_with(home_dir);
}

/// Reset session prompt after terminal was freshly allocated (cleared).
/// Forces run_loop to print a fresh prompt with full render.
pub fn reset_session_prompt(terminal_idx: u8) {
    // SAFETY: Core 0 only
    unsafe {
        if let Some(s) = (*sessions_ptr()).get_mut(&terminal_idx) {
            s.prompt_len = 0;
            s.reset_input();
        }
    }
}

/// Destroy the session for the given terminal.
pub fn destroy_session(terminal_idx: u8) {
    // SAFETY: Core 0 only
    unsafe { (*sessions_ptr()).remove(&terminal_idx); }
    CWDS.lock().remove(&terminal_idx);
}

/// Get a mutable reference to a session. Core 0 only.
fn session_mut(terminal_idx: u8) -> Option<&'static mut IntentSession> {
    // SAFETY: Core 0 only, no aliasing (one terminal active at a time)
    unsafe { (*sessions_ptr()).get_mut(&terminal_idx).map(|b| &mut **b) }
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
        crate::smp::scheduler::Priority::Normal,
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
    crate::shade::terminal::mark_dirty();
}

/// Check if an intent should run on Core 0 (needs interactive input or compositor).
fn is_core0_intent(verb: &str) -> bool {
    matches!(verb, "lock" | "passwd" | "password" | "passphrase" |
                   "clear" | "cls" | "shade" | "shell" | "npk-shell" |
                   "cd" | "pwd" | "top" | "htop" | "history" | "gpu" |
                   // microvm: VMX state (CR4.VMXE, IA32_FEATURE_CONTROL
                   // lock-bit, TSS, GDT-with-TR-slot) is BSP-only —
                   // worker cores would VMfail with error 8 (invalid
                   // host-state) because their TR is null.
                   "microvm")
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

fn parse_ip(s: &str) -> Option<[u8; 4]> {
    let parts: alloc::vec::Vec<&str> = s.split('.').collect();
    if parts.len() != 4 { return None; }
    let mut ip = [0u8; 4];
    for (i, p) in parts.iter().enumerate() {
        ip[i] = p.parse().ok()?;
    }
    Some(ip)
}

/// Ensure every directory along `path` exists. v2 has real Tree
/// objects so this is just an `mkdir -p`; no `.dir` marker files.
pub(crate) fn ensure_parents(path: &str) {
    let _ = crate::npkfs::fs::ensure_dirs(path);
}

/// Sync session state to terminal.rs saved input (for cursor restore on focus change).
/// Temporarily switches to the session's terminal to save to the correct slot.
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
                      session_id: CapId) -> usize {
    session.history.reset_cursor();

    loop {
        // Detect focus change (mouse click, shade action, WASM switch)
        if crate::shade::is_active() {
            // Phase 10: focus moved to a widget-kind window — return so
            // run_loop enters the widget-focused input branch. Without
            // this bailout we'd keep consuming keys as shell-line
            // history/edit events and never forward them to the
            // focused widget app (e.g. drun).
            if crate::shade::focused_widget_id().is_some() {
                sync_session_to_terminal(session);
                return 0;
            }

            let ft = crate::shade::terminal::active_idx();

            if crate::wasm::has_wasm_app(ft) {
                sync_session_to_terminal(session);
                return 0;
            }

            // Focus changed to a different terminal — return so run_loop can switch sessions
            if ft != session.terminal_idx {
                sync_session_to_terminal(session);
                return 0;
            }
        }

        // Poll network while waiting
        crate::net::poll();
        crate::shell::check_and_serve(vault, session_id);

        // Tick swap animation
        if crate::shade::with_compositor(|comp| comp.tick_animation()).unwrap_or(false) {
            crate::shade::render_frame();
        }

        while let Some(evt) = crate::xhci::poll_mouse() {
            crate::shade::handle_mouse(&evt);
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
                        return 0;
                    }
                }
                ShadeAction::NewWindow => {
                    sync_session_to_terminal(session);
                    crate::shade::handle_action(action);
                    return 0; // run_loop creates session for new window
                }
                ShadeAction::CloseWindow | ShadeAction::SpawnLauncher => {
                    // Both can destroy / invalidate the current session's
                    // terminal OR move focus to a widget window while we
                    // hold `&mut session` into the SESSIONS map. Sync
                    // session state to the terminal buffer first, run
                    // the action, then bail so run_loop re-acquires the
                    // (possibly new / absent / widget-focused) session
                    // cleanly — no dangling refs, no stale focus.
                    sync_session_to_terminal(session);
                    crate::shade::handle_action(action);
                    return 0;
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
            // Shade mode — idle until next IRQ (keyboard / mouse / timer).
            // SAFETY: ring-0, IRQs enabled.
            unsafe { core::arch::asm!("hlt"); }
            continue;
        } else {
            let serial = serial::SERIAL.lock();
            if !serial.has_data() {
                drop(serial);
                // SAFETY: ring-0 idle — 100Hz APIC timer IRQ wakes us reliably,
                // and all input paths (keyboard, mouse, NIC) are IRQ-driven.
                unsafe { core::arch::asm!("hlt"); }
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
        match event.key {
            KeyCode::Up => {
                if let Some((line, len)) = session.history.up() {
                    let len = len.min(session.input_buf.len());
                    if !crate::shade::is_active() {
                        for _ in 0..session.pos { kprint!("\x08 \x08"); }
                    }
                    session.input_buf[..len].copy_from_slice(&line[..len]);
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
                if let Some((line, len)) = session.history.down() {
                    let len = len.min(session.input_buf.len());
                    session.input_buf[..len].copy_from_slice(&line[..len]);
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
                if session.cursor < session.pos { session.cursor += 1; }
            }
            KeyCode::Left => {
                if session.cursor > 0 { session.cursor -= 1; }
            }
            KeyCode::Home => { session.cursor = 0; }
            KeyCode::End => { session.cursor = session.pos; }
            KeyCode::PageUp | KeyCode::PageDown | KeyCode::Insert => {}
            KeyCode::Delete => {
                if session.cursor < session.pos {
                    for i in session.cursor..session.pos - 1 {
                        session.input_buf[i] = session.input_buf[i + 1];
                    }
                    session.pos -= 1;
                    if crate::shade::is_active() {
                        crate::shade::terminal::rewrite_input(&session.input_buf, session.pos);
                    }
                }
            }
            KeyCode::Enter => {
                session.cursor = session.pos;
                kprint!("\n");
                session.history.push(&session.input_buf[..session.pos]);
                return session.pos;
            }
            KeyCode::Backspace => {
                if session.cursor > 0 {
                    for i in session.cursor..session.pos {
                        session.input_buf[i - 1] = session.input_buf[i];
                    }
                    session.pos -= 1;
                    session.cursor -= 1;
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
                        kprint!("{}", completion);
                    }
                }
                continue; // skip cursor update below
            }
            KeyCode::Escape => { continue; }
            KeyCode::F(_) => { continue; }
            KeyCode::Char(b) => {
                if b >= 0x20 && b < 0x7F && session.pos < session.input_buf.len() - 1 {
                    if session.cursor < session.pos {
                        for i in (session.cursor..session.pos).rev() {
                            session.input_buf[i + 1] = session.input_buf[i];
                        }
                    }
                    session.input_buf[session.cursor] = b;
                    session.pos += 1;
                    session.cursor += 1;
                    crate::shade::terminal::scroll_reset();
                    if crate::shade::is_active() {
                        crate::shade::terminal::rewrite_input(&session.input_buf, session.pos);
                        crate::shade::terminal::set_cursor_pos(
                            crate::shade::terminal::current_line_len()
                                .saturating_sub(session.pos - session.cursor));
                        crate::shade::render_input_line();
                    } else {
                        kprint!("{}", b as char);
                    }
                }
                continue; // skip cursor update below
            }
        }

        // Update cursor position for navigation keys (Up/Down/Left/Right/Home/End)
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
/// v2: list immediate children of the implied parent directory and
/// filter by the partial leaf name. No more recursive flat-walk.
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

pub fn run_loop(vault: &'static Mutex<Vault>, session_id: CapId) -> ! {
    // Store vault reference for worker cores
    VAULT_REF.store(vault as *const _ as *mut _, AtOrd::Release);

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
        // If focused window has a running WASM app or intent, route keys / wait.
        if crate::shade::is_active() {
            let focused_term = crate::shade::terminal::active_idx();

            // Phase 10: widget-kind window focused — keys go into the
            // per-window widget event queue, never the terminal / WASM
            // app key buf. The widget app polls them via npk_event_poll.
            if let Some(widget_wid) = crate::shade::focused_widget_id() {
                crate::shade::poll_render();
                crate::net::poll();

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
                    // Tab / Shift+Tab move focus inside the widget
                    // window when there's at least one focusable
                    // widget. Falls through (key reaches the app) if
                    // no focusable nodes exist (purely decorative tree).
                    if matches!(event.key, crate::input::KeyCode::Tab) {
                        let consumed = if event.modifiers.shift {
                            crate::shade::widgets::prev_focus(widget_wid)
                        } else {
                            crate::shade::widgets::next_focus(widget_wid)
                        };
                        if consumed { continue; }
                    }
                    // Keyboard navigation takes visual precedence over
                    // any stale mouse-hover state. Without this, moving
                    // the selection with arrows while the cursor sits
                    // on a different row leaves both rows highlighted.
                    // Mouse-move re-establishes hover on the next motion.
                    crate::shade::widgets::suppress_hover(widget_wid);
                    // If a Widget::Input is focused, the compositor
                    // owns text editing — printable / Backspace /
                    // Delete / Left / Right / Home / End / Enter are
                    // intercepted, mutate the editor buffer, and emit
                    // either Event::InputChange (value changed) or
                    // Event::Action(on_submit) (Enter). Apps see only
                    // the events they declared interest in; cursor
                    // moves never round-trip.
                    if crate::shade::widgets::handle_input_key(widget_wid, event.key) {
                        continue;
                    }
                    crate::shade::widgets::push_event(
                        widget_wid,
                        crate::shade::widgets::abi::Event::Key(event.key),
                    );
                }

                // Same polling cadence as the WASM-app branch.
                for _ in 0..5_000 {
                    core::hint::spin_loop();
                }
                continue;
            }

            // Intent running on worker — event loop without input
            if has_running_intent(focused_term) {
                from_intent = true;
                crate::shade::poll_render();
                crate::net::poll();

                while let Some(evt) = crate::xhci::poll_mouse() {
                    crate::shade::handle_mouse(&evt);
                }

                if crate::shade::take_deferred_render() {
                    crate::shade::render_frame();
                }

                if let Some(action) = crate::shade::input::poll_action() {
                    crate::shade::handle_action(action);
                }

                while let Some(_key) = crate::keyboard::read_key() {}

                // While a worker task is running we poll (pause) instead of hlt.
                // Rationale: hlt + HWP EPP=192 can sink Core 0 deep enough that the
                // LAPIC timer stalls on some platforms (observed on ADL-N); without
                // a steady timer tick we'd miss the worker's completion signal.
                // Net/mouse/key IRQs still wake us, but we can't rely on them
                // during a silent HTTP stall. Pause keeps the core in C0 cheaply.
                for _ in 0..5_000 {
                    core::hint::spin_loop();
                }
                continue;
            }

            // WASM app running — route keys to app
            if crate::wasm::has_wasm_app(focused_term) {
                from_wasm = true;
                wasm_term = focused_term;
                crate::shade::poll_render();
                crate::net::poll();

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

                // Poll (pause) — same rationale as the intent-running branch:
                // while a WASM app runs on a worker, Core 0 must keep ticking to
                // forward keys and detect completion even if the LAPIC timer
                // stalls in deep C-states.
                for _ in 0..5_000 {
                    core::hint::spin_loop();
                }
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

        // Get session for active terminal (create if needed)
        let term = crate::shade::terminal::active_idx();
        if session_mut(term).is_none() {
            create_session(term);
            need_prompt = true; // new session always needs a prompt
        }
        // SAFETY: Core 0 only, session exists after create above, no aliasing
        let session = unsafe {
            &mut **((*sessions_ptr()).get_mut(&term).unwrap() as *mut Box<IntentSession>)
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
            kprint!("{}", p);
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
        let len = read_line_with_tab(session, vault, session_id);

        if crate::shade::is_active() {
            crate::shade::render_frame();
        }

        if len == 0 { continue; } // focus change — don't set need_prompt

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

        // Check if this intent can run on a worker core
        let verb = input.splitn(2, ' ').next().unwrap_or("");
        if !is_core0_intent(verb) && crate::shade::is_active() {
            let term_idx = crate::shade::terminal::active_idx();
            if spawn_intent_on_worker(input, term_idx, session_id) {
                // Worker prints prompt when done via from_intent transition
                continue;
            }
        }

        dispatch_intent(input, vault, session_id);

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
        "debug" => {
            // Parse "<ip> <port>" and set the target before spawning debug.wasm.
            // No-arg dev shortcut: dial Florian's laptop on the LAN
            // (192.168.178.97:22222) so reboots don't need re-typing
            // every time. Saves ~10 s per cycle. Remove or move to
            // sys/config/debug-target once a config-file path lands.
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
                    microvm_linux(b"");
                }
            } else if let Some(rest) = sub.strip_prefix("shell") {
                // `microvm shell [<line>]` — pre-injects <line> + '\n'
                // into the UART RX FIFO before VMLAUNCH. PID-1 in the
                // guest detects the pending byte via LSR.DR, drains
                // RBR through iopl(3), echoes the line back through
                // the same UART, then powers off. End-to-end inject-
                // console round-trip (Phase 12.1.4).
                if require_cap(vault, &session, Rights::EXECUTE, "microvm shell") {
                    let line = rest.trim();
                    let line = if line.is_empty() { "hi" } else { line };
                    let mut buf = alloc::vec::Vec::with_capacity(line.len() + 1);
                    buf.extend_from_slice(line.as_bytes());
                    buf.push(b'\n');
                    microvm_linux(&buf);
                }
            } else {
                kprintln!("[microvm] unknown subcommand: '{}'", sub);
                kprintln!("[microvm] available: test, linux-info, linux, shell");
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
        "dmesg" | "bootlog" => {
            system::intent_dmesg();
        }
        "gpu" => {
            system::intent_gpu(args);
        }
        "shade" => {
            system::intent_shade(args);
        }
        "history" => {
            system::intent_history();
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

        "run" | "exec" => {
            if require_cap(vault, &session, Rights::EXECUTE, "run") {
                wasm::intent_run(args);
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

        "wallpaper" | "wp" => {
            wallpaper::intent_wallpaper(args);
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

/// Check capability before executing an intent. Returns true if allowed.
/// `microvm linux-info` handler — fetch the bundled bzImage from
/// npkFS, parse the Linux Boot Protocol setup-header, print stats.
/// Read-only; no VM activity. The asset lands in npkFS at
/// `sys/microvm/linux-virt.bzImage` on fresh install (see
/// install_data/assets).
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

/// `microvm linux` / `microvm shell` handler — fetch the bundled
/// bzImage from npkFS, hand it + a cmdline + `inject` bytes to
/// vmx::run_linux. The kernel writes its earlyprintk to serial 0x3F8,
/// which we trap via I/O bitmap and reflect as `[guest] <line>`
/// kprintln output. `inject` is empty for the plain `linux`
/// subcommand (idle-pause behavior); for `shell <line>` it's the
/// line + '\n' pre-loaded into the UART RX FIFO so PID-1 can echo
/// it back (Phase 12.1.4).
fn microvm_linux(inject: &[u8]) {
    const BZIMAGE_PATH: &str = "sys/microvm/linux-virt.bzImage";
    const INITRAMFS_PATH: &str = "sys/microvm/initramfs.cpio.gz";
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
    // `nolapic noapic acpi=off pci=off tsc=reliable`: tell Linux to
    // skip hardware probing it would otherwise crash on. We're not
    // a real PC — no ACPI tables, no PCI bus, no functioning APIC
    // (just an EPT-mapped scratch page absorbing the MMIO accesses).
    // Without these, Linux times out / panics on probes that don't
    // behave like real silicon. With them, it boots minimally to
    // the rootfs-mount panic — the 12.1.1d milestone target.
    // `tsc_early_khz=2000000` skips Linux's PIT-based TSC calibration,
    // which deadlocks on the AMD-V backend (host CPUID 0x15 absent
    // on AMD → Linux falls back to PIT calibration → our PIT IO
    // emulation returns 0 → Linux loops forever waiting for ticks).
    // Harmless on Intel: there CPUID 0x15 advertises the freq and
    // Linux uses it directly, ignoring the cmdline hint.
    const CMDLINE: &[u8] =
        b"earlycon=uart8250,io,0x3f8,115200n8 console=ttyS0,115200 panic=1 nokaslr \
          nolapic noapic acpi=off pci=off tsc=reliable tsc_early_khz=2000000 \
          devtmpfs.mount=1";

    kprintln!("[microvm] loading bzImage from {}...", BZIMAGE_PATH);
    let bytes = match crate::npkfs::fetch(BZIMAGE_PATH) {
        Ok((b, _hash)) => b,
        Err(e) => {
            kprintln!("[microvm] cannot read {}: {:?}", BZIMAGE_PATH, e);
            kprintln!("[microvm] reinstall from USB to seed bundled assets");
            return;
        }
    };

    let initramfs = match crate::npkfs::fetch(INITRAMFS_PATH) {
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
    };

    kprintln!("[microvm] launching Linux ({} bytes, cmdline: {:?})",
              bytes.len(),
              core::str::from_utf8(CMDLINE).unwrap_or("?"));

    match crate::microvm::run_linux(&bytes, CMDLINE, initramfs.as_deref(), inject) {
        Ok(outcome) => {
            let basic = (outcome.exit_reason & 0xFFFF) as u16;
            kprintln!("[microvm] guest exited — final reason {} qual {:#x}",
                      basic, outcome.exit_qualification);
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
            crate::shade::request_render();
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
pub use wallpaper::random_wallpaper;
pub use fs::crypto_bench;

/// Create initial directory structure and set cwd to home.
///
/// Lays down the canonical user-tree on first boot so loft's sidebar
/// (`Home / Documents / Downloads / Pictures / Projects / Trash`) and
/// the wallpaper subsystem land on real `.dir`-marker-backed
/// directories instead of phantom paths. Each `ensure_parents` call
/// is idempotent — re-running setup_home on an already-populated home
/// is a no-op (no duplicate writes, no journal churn).
pub fn setup_home() {
    let home = home_dir();
    ensure_parents(&home);
    // Sidebar-aligned standard subdirs. The wallpapers dir lives
    // under `pictures/` to match `wallpaper_dir()` in
    // `intent::wallpaper`; the previous flat `wallpapers/` was a
    // dead path nothing read or wrote.
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
    // SAFETY: Core 0 only (history is a Core 0-only intent)
    // SAFETY: Core 0 only (history is a Core 0-only intent)
    let session = unsafe { (*sessions_ptr()).get(&term) };
    if let Some(s) = session {
        let hist = &s.history;
        if hist.count == 0 {
            kprintln!("(no history)");
            return;
        }
        let start = if hist.count > HIST_MAX { hist.count - HIST_MAX } else { 0 };
        for i in start..hist.count {
            let idx = i % HIST_MAX;
            if let Ok(text) = core::str::from_utf8(&hist.lines[idx][..hist.lens[idx]]) {
                kprintln!("  {:3}  {}", i + 1, text);
            }
        }
    } else {
        kprintln!("(no history)");
    }
}

/// Execute an intent from remote shell (dispatch without the loop).
#[allow(dead_code)]
pub fn dispatch_for_shell(input: &str, vault: &'static Mutex<Vault>, session_id: CapId) {
    dispatch_intent(input, vault, session_id);
}
