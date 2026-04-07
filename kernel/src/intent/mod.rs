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
mod wasm;

use crate::capability::{self, CapId, Vault, Rights};
use crate::{kprint, kprintln, serial};
use alloc::string::String;
use spin::Mutex;

const INPUT_BUF_SIZE: usize = 512;

// -- Command history --
const HIST_MAX: usize = 32;
const HIST_LINE: usize = 256;

struct History {
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

static HISTORY: Mutex<History> = Mutex::new(History {
    lines: [[0; HIST_LINE]; HIST_MAX],
    lens: [0; HIST_MAX],
    count: 0,
    cursor: 0,
});

/// Current working directory (prefix for relative paths).
static CWD: Mutex<String> = Mutex::new(String::new());

/// Set the working directory.
pub fn set_cwd(path: &str) {
    let mut cwd = CWD.lock();
    cwd.clear();
    let clean = path.trim_matches('/');
    cwd.push_str(clean);
}

/// Get the working directory.
fn get_cwd() -> String {
    CWD.lock().clone()
}

/// Get the home directory from config.
fn home_dir() -> String {
    match crate::config::get("name") {
        Some(name) => alloc::format!("home/{}", name),
        None => String::from("home"),
    }
}

/// Resolve a name relative to cwd.
/// - Absolute (starts with /): strip leading / and use as-is
/// - ".." : go up one level
/// - Relative: prepend cwd
fn resolve_path(name: &str) -> String {
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

/// Ensure all parent directories exist for a given path (create .dir markers).
fn ensure_parents(path: &str) {
    let mut current = String::new();
    for part in path.split('/') {
        if !current.is_empty() { current.push('/'); }
        current.push_str(part);
        let marker = alloc::format!("{}/.dir", current);
        if !crate::npkfs::exists(&marker) {
            let _ = crate::npkfs::store(&marker, b"", capability::CAP_NULL);
        }
    }
}

/// Read a line from serial/keyboard with tab-completion, history, and network polling.
fn read_line_with_tab(buf: &mut [u8], vault: &'static Mutex<Vault>, session_id: CapId) -> usize {
    let mut pos = 0;      // total bytes in buffer
    let mut cursor = 0;   // cursor position within buffer (can be < pos)
    let mut esc: u8 = 0; // 0=normal, 1=got ESC, 2=got ESC[
    let mut esc_mod = false; // Was mod key held when ESC was received?

    HISTORY.lock().reset_cursor();

    loop {
        // Poll network while waiting
        crate::net::poll();
        // Check for incoming npk-shell connections
        crate::shell::check_and_serve(vault, session_id);

        // Poll USB mouse events (batch: process all pending, render once)
        if let Some(evt) = crate::xhci::poll_mouse() {
            let mut last_evt = evt;
            while let Some(next) = crate::xhci::poll_mouse() {
                // Update mouse state without rendering for intermediate events
                crate::shade::with_compositor(|comp| comp.handle_mouse(&last_evt));
                last_evt = next;
            }
            // Only render for the final event
            crate::shade::handle_mouse(&last_evt);
        }

        // Check for shade compositor actions (Mod+key)
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
                    crate::shade::terminal::save_input_with_cursor(&buf[..pos], pos, cursor);
                    crate::shade::handle_action(action);
                    let (p, c) = crate::shade::terminal::restore_input_with_cursor(buf);
                    pos = p;
                    cursor = c;
                }
                ShadeAction::NewWindow => {
                    crate::shade::terminal::save_input_with_cursor(&buf[..pos], pos, cursor);
                    crate::shade::handle_action(action);
                    pos = 0;
                    cursor = 0;
                }
                _ => {
                    crate::shade::handle_action(action);
                }
            }
            continue;
        }

        // Check both serial and PS/2 keyboard for input
        let byte = if let Some(key) = crate::keyboard::read_key() {
            key
        } else {
            let serial = serial::SERIAL.lock();
            if !serial.has_data() {
                drop(serial);
                core::hint::spin_loop();
                continue;
            }
            let b = serial.read_byte();
            drop(serial);
            b
        };

        // Intercept shade keybindings (Mod+key) before intent loop
        if crate::shade::input::try_keybind(byte) {
            continue;
        }

        // Handle ANSI escape sequences (ESC [ A/B/C/D)
        if esc == 1 {
            esc = if byte == 0x5b { 2 } else { 0 };
            continue;
        }
        if esc == 2 {
            esc = 0;
            // Check shade arrow keybinds (uses saved mod state from ESC time)
            if esc_mod && crate::shade::input::try_arrow_keybind(byte) {
                continue;
            }
            match byte {
                b'A' => {
                    // Arrow up — previous history entry
                    let mut hist = HISTORY.lock();
                    if let Some((line, len)) = hist.up() {
                        let len = len.min(buf.len());
                        if !crate::shade::is_active() {
                            for _ in 0..pos { kprint!("\x08 \x08"); }
                        }
                        buf[..len].copy_from_slice(&line[..len]);
                        pos = len;
                        cursor = len;
                        if !crate::shade::is_active() {
                            if let Ok(s) = core::str::from_utf8(&buf[..pos]) {
                                kprint!("{}", s);
                            }
                        }
                    }
                }
                b'B' => {
                    // Arrow down — next history entry
                    let mut hist = HISTORY.lock();
                    if !crate::shade::is_active() {
                        for _ in 0..pos { kprint!("\x08 \x08"); }
                    }
                    if let Some((line, len)) = hist.down() {
                        let len = len.min(buf.len());
                        buf[..len].copy_from_slice(&line[..len]);
                        pos = len;
                        cursor = len;
                        if !crate::shade::is_active() {
                            if let Ok(s) = core::str::from_utf8(&buf[..pos]) {
                                kprint!("{}", s);
                            }
                        }
                    } else {
                        pos = 0;
                        cursor = 0;
                    }
                }
                b'C' => {
                    // Arrow right — move cursor right
                    if cursor < pos { cursor += 1; }
                }
                b'D' => {
                    // Arrow left — move cursor left
                    if cursor > 0 { cursor -= 1; }
                }
                b'H' => {
                    // Home — cursor to start of input
                    cursor = 0;
                }
                b'F' => {
                    // End — cursor to end of input
                    cursor = pos;
                }
                0x7E => {} // consume trailing ~ from PgUp/PgDn sequences
                _ => {}
            }
            if crate::shade::is_active() {
                crate::shade::terminal::set_cursor_pos(
                    crate::shade::terminal::current_line_len().saturating_sub(pos - cursor));
                crate::shade::render_input_line();
            }
            continue;
        }

        match byte {
            0x1b => {
                esc = 1;
                // Capture mod state NOW (before next USB report clears it)
                esc_mod = crate::shade::input::is_mod_active();
            }
            b'\r' | b'\n' => {
                cursor = pos; // move cursor to end before newline
                kprint!("\n");
                HISTORY.lock().push(&buf[..pos]);
                return pos;
            }
            0x08 | 0x7F => {
                // Backspace — delete char left of cursor
                if cursor > 0 {
                    for i in cursor..pos {
                        buf[i - 1] = buf[i];
                    }
                    pos -= 1;
                    cursor -= 1;
                    if crate::shade::is_active() {
                        crate::shade::terminal::rewrite_input(&buf, pos);
                        crate::shade::terminal::set_cursor_pos(
                            crate::shade::terminal::current_line_len().saturating_sub(pos - cursor));
                        crate::shade::render_input_line();
                    } else {
                        kprint!("\x08 \x08");
                    }
                }
            }
            0x09 => {
                // Tab — attempt completion
                if let Ok(input) = core::str::from_utf8(&buf[..pos]) {
                    if let Some(completion) = tab_complete(input) {
                        for b in completion.as_bytes() {
                            if pos < buf.len() {
                                buf[pos] = *b;
                                pos += 1;
                            }
                        }
                        kprint!("{}", completion);
                    }
                }
            }
            b if b >= 0x20 && b < 0x7F => {
                if pos < buf.len() - 1 {
                    // Insert at cursor position (shift right)
                    if cursor < pos {
                        for i in (cursor..pos).rev() {
                            buf[i + 1] = buf[i];
                        }
                    }
                    buf[cursor] = b;
                    pos += 1;
                    cursor += 1;
                    crate::shade::terminal::scroll_reset();
                    if crate::shade::is_active() {
                        crate::shade::terminal::rewrite_input(&buf, pos);
                        crate::shade::terminal::set_cursor_pos(
                            crate::shade::terminal::current_line_len().saturating_sub(pos - cursor));
                        crate::shade::render_input_line();
                    } else {
                        kprint!("{}", b as char);
                    }
                }
            }
            _ => {}
        }
    }
}

/// Tab-completion: find matching paths for the last word in the input.
fn tab_complete(input: &str) -> Option<String> {
    let last_space = input.rfind(' ').map(|i| i + 1).unwrap_or(0);
    let partial = &input[last_space..];

    // Resolve what's typed so far to an absolute prefix
    // "" or ends with / → list contents of current dir
    // "te" in home/florian → search for "home/florian/te"
    let search = if partial.is_empty() || partial.ends_with('/') {
        let base = if partial.is_empty() { get_cwd() } else { resolve_path(partial.trim_end_matches('/')) };
        if base.is_empty() { String::new() } else { alloc::format!("{}/", base) }
    } else {
        resolve_path(partial)
    };

    let entries = match crate::npkfs::list() {
        Ok(e) => e,
        Err(_) => return None,
    };

    // Find all names that start with our search prefix
    // Collapse to immediate children (files or first dir component)
    let mut matches: alloc::vec::Vec<String> = alloc::vec::Vec::new();
    for (name, _, _) in &entries {
        if name.starts_with(".npk-") { continue; }
        if name.ends_with("/.dir") {
            let dir = &name[..name.len() - 5];
            if dir.starts_with(search.as_str()) {
                let rest = &dir[search.len()..];
                if rest.is_empty() {
                    // Exact match: the dir itself (e.g. search="home/florian/test", dir="home/florian/test")
                    let full = alloc::format!("{}/", dir);
                    if !matches.contains(&full) { matches.push(full); }
                } else {
                    // Immediate child dir
                    let child = if let Some(idx) = rest.find('/') { &rest[..idx] } else { rest };
                    if !child.is_empty() {
                        let full = alloc::format!("{}{}/", search, child);
                        if !matches.contains(&full) { matches.push(full); }
                    }
                }
            }
            continue;
        }
        if name.starts_with(search.as_str()) {
            let rest = &name[search.len()..];
            if let Some(idx) = rest.find('/') {
                let full = alloc::format!("{}{}/", search, &rest[..idx]);
                if !matches.contains(&full) { matches.push(full); }
            } else {
                let full = String::from(name.as_str());
                if !matches.contains(&full) { matches.push(full); }
            }
        }
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
    let user = crate::config::get("name");
    let cwd = get_cwd();
    let user_str = user.as_deref().unwrap_or("npk");
    if cwd.is_empty() {
        kprint!("{}@npk /> {}", user_str, input);
    } else {
        kprint!("{}@npk {}> {}", user_str, cwd, input);
    }

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
    let mut input_buf = [0u8; INPUT_BUF_SIZE];

    loop {
        {
            let user = crate::config::get("name");
            let cwd = get_cwd();
            let user_str = user.as_deref().unwrap_or("npk");
            if cwd.is_empty() {
                kprint!("{}@npk /> ", user_str);
            } else {
                kprint!("{}@npk {}> ", user_str, cwd);
            }
            // Show prompt in shade window immediately
            if crate::shade::is_active() {
                crate::shade::render_input_line();
            }
        }

        let len = read_line_with_tab(&mut input_buf, vault, session_id);

        if len == 0 { continue; }

        let input = match core::str::from_utf8(&input_buf[..len]) {
            Ok(s) => s.trim(),
            Err(_) => {
                kprintln!("[npk] invalid UTF-8 input");
                continue;
            }
        };

        if input == "lock" {
            auth::intent_lock();
            continue;
        }

        dispatch_intent(input, vault, session_id);

        // Re-render shade compositor to show new output
        if crate::shade::is_active() {
            crate::shade::render_frame();
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
        "uname" | "version" | "kernel" => {
            system::intent_uname(args);
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

        // Intents requiring EXECUTE (WASM sandbox)
        "add" => {
            if require_cap(vault, &session, Rights::EXECUTE, "add") {
                wasm::intent_wasm_add(args);
            }
        }
        "multiply" => {
            if require_cap(vault, &session, Rights::EXECUTE, "multiply") {
                wasm::intent_wasm_multiply(args);
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
            kprintln!("[npk] Unknown intent: '{}'", input);
            kprintln!("[npk] Try 'help' for available intents.");
        }
    }
}

/// Check capability before executing an intent. Returns true if allowed.
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

    let dir_marker = alloc::format!("{}/.dir", resolved);

    // Check: either .dir marker exists, or objects with this prefix exist
    let exists = crate::npkfs::exists(&dir_marker) || {
        let prefix = alloc::format!("{}/", resolved);
        crate::npkfs::list().map(|entries| {
            entries.iter().any(|(n, _, _)| n.starts_with(prefix.as_str()))
        }).unwrap_or(false)
    };

    if exists {
        set_cwd(&resolved);
    } else {
        kprintln!("[npk] '{}': not found", target);
    }
}

/// Re-export public API for main.rs
pub use wasm::bootstrap_wasm;

/// Create initial directory structure and set cwd to home.
pub fn setup_home() {
    let home = home_dir();
    ensure_parents(&home);
    set_cwd(&home);
}

/// Expose CWD for npk-shell.
pub fn get_cwd_for_shell() -> String {
    get_cwd()
}

/// Execute an intent from npk-shell (dispatch without the loop).
pub fn dispatch_for_shell(input: &str, vault: &'static Mutex<Vault>, session_id: CapId) {
    dispatch_intent(input, vault, session_id);
}
