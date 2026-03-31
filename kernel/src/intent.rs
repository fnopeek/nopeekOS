//! Intent Loop
//!
//! Not a shell. Takes intents, not commands.
//! Every intent requires a valid capability token.

use crate::capability::{self, CapId, Vault, Rights};
use crate::{kprint, kprintln, crypto, serial};
use alloc::string::String;
use spin::Mutex;

const INPUT_BUF_SIZE: usize = 512;

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

/// Read a line from serial with tab-completion and network polling.
fn read_line_with_tab(buf: &mut [u8]) -> usize {
    let mut pos = 0;

    loop {
        // Poll network while waiting
        crate::net::poll();
        let serial = serial::SERIAL.lock();
        if !serial.has_data() {
            drop(serial);
            unsafe { core::arch::asm!("hlt"); }
            continue;
        }
        let byte = serial.read_byte();
        drop(serial);

        match byte {
            b'\r' | b'\n' => {
                kprint!("\n");
                return pos;
            }
            0x08 | 0x7F => {
                // Backspace
                if pos > 0 {
                    pos -= 1;
                    kprint!("\x08 \x08");
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
                if pos < buf.len() {
                    buf[pos] = b;
                    pos += 1;
                    kprint!("{}", b as char);
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
        }

        let len = read_line_with_tab(&mut input_buf);

        if len == 0 { continue; }

        let input = match core::str::from_utf8(&input_buf[..len]) {
            Ok(s) => s.trim(),
            Err(_) => {
                kprintln!("[npk] invalid UTF-8 input");
                continue;
            }
        };

        if input == "lock" {
            intent_lock();
            continue;
        }

        dispatch_intent(input, vault, session_id);
    }
}

fn intent_lock() {
    kprintln!("[npk] System locked. All capabilities suspended.");
    kprintln!("[npk] Enter passphrase to unlock.");
    crypto::clear_master_key();

    let salt = crate::npkfs::install_salt().unwrap_or([0u8; 16]);
    let mut attempts: u32 = 0;

    loop {
        if attempts > 0 {
            let delay_secs = 1u64 << attempts.min(5);
            kprintln!("[npk] Wait {} seconds...", delay_secs);
            let start = crate::interrupts::ticks();
            let delay_ticks = delay_secs * 100;
            while crate::interrupts::ticks() - start < delay_ticks {
                unsafe { core::arch::asm!("hlt"); }
            }
        }

        kprint!("[npk] Passphrase: ");
        let mut buf = [0u8; 128];
        let len = { serial::SERIAL.lock().read_line_masked(&mut buf) };
        if len == 0 { continue; }

        let key = crypto::derive_master_key(&buf[..len], &salt);
        for b in buf.iter_mut() { *b = 0; }

        crypto::set_master_key(key);

        match crate::npkfs::fetch(".npk-keycheck") {
            Ok((data, _)) if &data[..] == b"nopeekOS.keycheck.v1.valid" => {
                crate::config::load();
                if let Some(name) = crate::config::get("name") {
                    kprintln!("[npk] Welcome back, {}.", name);
                } else {
                    kprintln!("[npk] Unlocked.");
                }
                return;
            }
            _ => {
                crypto::clear_master_key();
                kprintln!("[npk] Wrong passphrase.");
                attempts += 1;
                if attempts >= 10 {
                    kprintln!("[npk] Too many failed attempts. System halted.");
                    loop { unsafe { core::arch::asm!("cli; hlt"); } }
                }
            }
        }
    }
}

fn intent_passwd() {
    let salt = crate::npkfs::install_salt().unwrap_or([0u8; 16]);

    // Verify current passphrase
    kprint!("[npk] Current passphrase: ");
    let mut buf = [0u8; 128];
    let len = { serial::SERIAL.lock().read_line_masked(&mut buf) };
    if len == 0 {
        kprintln!("[npk] Cancelled.");
        return;
    }

    let old_key = crypto::derive_master_key(&buf[..len], &salt);
    for b in buf.iter_mut() { *b = 0; }

    // Temporarily set old key to verify
    let saved_key = crypto::get_master_key();
    crypto::set_master_key(old_key);

    match crate::npkfs::fetch(".npk-keycheck") {
        Ok((data, _)) if &data[..] == b"nopeekOS.keycheck.v1.valid" => {}
        _ => {
            // Restore original key
            if let Some(k) = saved_key { crypto::set_master_key(k); }
            kprintln!("[npk] Wrong passphrase. Aborted.");
            return;
        }
    }

    // Delete old keycheck (still encrypted with old key)
    let _ = crate::npkfs::delete(".npk-keycheck");

    // Get new passphrase
    let new_key = loop {
        kprint!("[npk] New passphrase: ");
        let mut buf1 = [0u8; 128];
        let len1 = { serial::SERIAL.lock().read_line_masked(&mut buf1) };
        if len1 < 8 {
            kprintln!("[npk] Too short. Minimum 8 characters.");
            continue;
        }

        kprint!("[npk] Confirm passphrase: ");
        let mut buf2 = [0u8; 128];
        let len2 = { serial::SERIAL.lock().read_line_masked(&mut buf2) };

        if len1 != len2 || buf1[..len1] != buf2[..len2] {
            kprintln!("[npk] Passphrases do not match. Try again.");
            for b in buf1.iter_mut() { *b = 0; }
            for b in buf2.iter_mut() { *b = 0; }
            continue;
        }

        let key = crypto::derive_master_key(&buf1[..len1], &salt);
        for b in buf1.iter_mut() { *b = 0; }
        for b in buf2.iter_mut() { *b = 0; }
        break key;
    };

    // Set new key and re-encrypt keycheck
    crypto::set_master_key(new_key);
    match crate::npkfs::store(".npk-keycheck", b"nopeekOS.keycheck.v1.valid", capability::CAP_NULL) {
        Ok(_) => kprintln!("[npk] Passphrase changed successfully."),
        Err(e) => kprintln!("[npk] ERROR: Could not store new keycheck: {}", e),
    }

    kprintln!("[npk] NOTE: Existing objects remain encrypted with the old key.");
    kprintln!("[npk]       They will be re-encrypted on next fetch+store cycle.");
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
                intent_status(&vault.lock());
            }
        }
        "caps" | "capabilities" => {
            if require_cap(vault, &session, Rights::READ, "caps") {
                intent_caps(&vault.lock());
            }
        }
        "audit" => {
            if require_cap(vault, &session, Rights::AUDIT, "audit") {
                intent_audit();
            }
        }

        // Intents requiring EXECUTE (WASM sandbox)
        "add" => {
            if require_cap(vault, &session, Rights::EXECUTE, "add") {
                intent_wasm_add(args);
            }
        }
        "multiply" => {
            if require_cap(vault, &session, Rights::EXECUTE, "multiply") {
                intent_wasm_multiply(args);
            }
        }
        "disk" | "blk" => {
            let sub = args.trim();
            if sub.is_empty() || sub == "info" {
                if require_cap(vault, &session, Rights::READ, "disk") {
                    intent_disk_info();
                }
            } else if sub.starts_with("read ") || sub == "read" {
                if require_cap(vault, &session, Rights::READ, "disk read") {
                    intent_disk_read(sub.strip_prefix("read").unwrap_or("").trim());
                }
            } else if sub.starts_with("write ") || sub == "write" {
                if require_cap(vault, &session, Rights::WRITE, "disk write") {
                    intent_disk_write(sub.strip_prefix("write").unwrap_or("").trim());
                }
            } else {
                kprintln!("[npk] Usage: disk [info|read <sector>|write <sector> <text>]");
            }
        }

        "store" | "save" => {
            if require_cap(vault, &session, Rights::WRITE, "store") {
                intent_store(args, session);
            }
        }
        "fetch" | "load" => {
            if require_cap(vault, &session, Rights::READ, "fetch") {
                intent_fetch(args);
            }
        }
        "cat" | "show" | "print" | "type" => {
            if require_cap(vault, &session, Rights::READ, "cat") {
                intent_cat(args);
            }
        }
        "grep" | "search" | "find" => {
            if require_cap(vault, &session, Rights::READ, "grep") {
                intent_grep(args);
            }
        }
        "head" => {
            if require_cap(vault, &session, Rights::READ, "head") {
                intent_head(args);
            }
        }
        "wc" | "count" => {
            if require_cap(vault, &session, Rights::READ, "wc") {
                intent_wc(args);
            }
        }
        "hexdump" | "hex" | "xxd" => {
            if require_cap(vault, &session, Rights::READ, "hexdump") {
                intent_hexdump(args);
            }
        }

        "delete" | "rm" | "remove" => {
            if require_cap(vault, &session, Rights::WRITE, "delete") {
                intent_delete(args);
            }
        }
        "mkdir" => {
            if require_cap(vault, &session, Rights::WRITE, "mkdir") {
                intent_mkdir(args);
            }
        }
        "rmdir" => {
            if require_cap(vault, &session, Rights::WRITE, "rmdir") {
                intent_rmdir(args);
            }
        }
        "list" | "ls" | "objects" => {
            if require_cap(vault, &session, Rights::READ, "list") {
                intent_list(args);
            }
        }
        "fsinfo" | "fs" => {
            if require_cap(vault, &session, Rights::READ, "fsinfo") {
                intent_fsinfo();
            }
        }

        "resolve" | "dns" => {
            if require_cap(vault, &session, Rights::READ, "resolve") {
                intent_resolve(args);
            }
        }
        "time" | "clock" | "date" => {
            if require_cap(vault, &session, Rights::READ, "time") {
                intent_time();
            }
        }
        "traceroute" | "trace" => {
            if require_cap(vault, &session, Rights::EXECUTE, "traceroute") {
                intent_traceroute(args);
            }
        }
        "netstat" | "connections" => {
            if require_cap(vault, &session, Rights::READ, "netstat") {
                intent_netstat();
            }
        }
        "http" | "curl" | "wget" => {
            if require_cap(vault, &session, Rights::EXECUTE, "http") {
                intent_http(args);
            }
        }
        "https" => {
            if require_cap(vault, &session, Rights::EXECUTE, "https") {
                intent_https(args);
            }
        }
        "ping" => {
            if require_cap(vault, &session, Rights::EXECUTE, "ping") {
                intent_ping(args);
            }
        }
        "net" | "ifconfig" => {
            if require_cap(vault, &session, Rights::READ, "net") {
                intent_net_info();
            }
        }

        "run" | "exec" => {
            if require_cap(vault, &session, Rights::EXECUTE, "run") {
                intent_run(args);
            }
        }

        "halt" | "shutdown" | "poweroff" => {
            if require_cap(vault, &session, Rights::EXECUTE, "halt") {
                intent_halt();
            }
        }

        "passwd" | "password" | "passphrase" => {
            intent_passwd();
        }

        "set" => {
            if require_cap(vault, &session, Rights::WRITE, "set") {
                intent_set(args);
            }
        }
        "get" => {
            if require_cap(vault, &session, Rights::READ, "get") {
                intent_get(args);
            }
        }
        "config" | "settings" => {
            if require_cap(vault, &session, Rights::READ, "config") {
                intent_config();
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
            // ANSI escape: clear screen + cursor home
            kprint!("\x1B[2J\x1B[H");
        }

        // Unrestricted intents (informational)
        "help" | "?" => intent_help_topic(args.trim()),
        "echo" => intent_echo(args),
        "think" => intent_think(args),
        "about" => intent_about(),
        "philosophy" => intent_philosophy(),

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

fn intent_status(vault: &Vault) {
    let (active_caps, max_caps) = vault.stats();
    let (free_frames, free_mb) = crate::memory::stats();
    let uptime = crate::interrupts::uptime_secs();
    let audit_count = crate::audit::total_count();

    kprintln!();
    kprintln!("  nopeekOS v0.1.0 – AI-native Operating System");
    kprintln!("  ──────────────────────────────────────────");
    kprintln!("  Uptime:        {}m {}s", uptime / 60, uptime % 60);
    kprintln!("  Phase:         2 (Capability Enforcement)");
    kprintln!("  Architecture:  x86_64");
    let (huge_pages, small_pages) = crate::paging::stats();
    kprintln!("  Memory:        {} MB free ({} frames)", free_mb, free_frames);
    kprintln!("  Paging:        {} x 2MB + {} x 4KB, NX enabled", huge_pages, small_pages);
    kprintln!("  Capabilities:  {}/{} active", active_caps, max_caps);
    kprintln!("  Audit log:     {} events", audit_count);
    kprintln!("  WASM Runtime:  wasmi (interpreter)");
    if let Some(cap) = crate::virtio_blk::capacity() {
        let mb = (cap * crate::virtio_blk::SECTOR_SIZE as u64) / (1024 * 1024);
        kprintln!("  Block device:  {} MB ({} sectors, virtio-blk)", mb, cap);
    } else {
        kprintln!("  Block device:  none");
    }
    if let Some(mac) = crate::virtio_net::mac() {
        let ip = crate::net::arp::our_ip();
        kprintln!("  Network:       {}.{}.{}.{} ({:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x})",
            ip[0], ip[1], ip[2], ip[3], mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]);
    } else {
        kprintln!("  Network:       none");
    }
    if let Some((_, free, objects, gen)) = crate::npkfs::stats() {
        kprintln!("  npkFS:         {} objects, {} free blocks (gen {})", objects, free, gen);
    } else {
        kprintln!("  npkFS:         not mounted");
    }
    kprintln!();
}

fn intent_caps(vault: &Vault) {
    let (active, max) = vault.stats();
    kprintln!();
    kprintln!("  Capability Vault");
    kprintln!("  ────────────────");
    kprintln!("  Active tokens:  {}", active);
    kprintln!("  Max capacity:   {}", max);
    kprintln!("  Token IDs:      256-bit random (ChaCha20 CSPRNG)");
    kprintln!();
    kprintln!("  Security model: Deny by Default");
    kprintln!("  No ambient authority. No root user. No sudo.");
    kprintln!("  Every action requires an explicit capability token.");
    kprintln!();
}

fn intent_audit() {
    use crate::audit::{self, AuditOp};

    let entries = audit::recent(10);
    let total = audit::total_count();

    kprintln!();
    kprintln!("  Audit Log ({} total events, showing last {})", total, entries.len());
    kprintln!("  ─────────────────────────────────────────────");

    if entries.is_empty() {
        kprintln!("  (no events recorded)");
    } else {
        for entry in &entries {
            let secs = entry.tick / 100;
            let ms = (entry.tick % 100) * 10;
            match entry.op {
                AuditOp::Create { parent_id, new_id } =>
                    kprintln!("  [{:>4}.{:03}s] CREATE {:08x} from {:08x}",
                        secs, ms, capability::short_id(&new_id), capability::short_id(&parent_id)),
                AuditOp::Revoke { revoker_id, target_id } =>
                    kprintln!("  [{:>4}.{:03}s] REVOKE {:08x} by {:08x}",
                        secs, ms, capability::short_id(&target_id), capability::short_id(&revoker_id)),
                AuditOp::Check { cap_id } =>
                    kprintln!("  [{:>4}.{:03}s] CHECK  {:08x} OK",
                        secs, ms, capability::short_id(&cap_id)),
                AuditOp::Denied { reason } =>
                    kprintln!("  [{:>4}.{:03}s] DENIED {:?}",
                        secs, ms, reason),
                AuditOp::Expired { cap_id } =>
                    kprintln!("  [{:>4}.{:03}s] EXPIRED {:08x}",
                        secs, ms, capability::short_id(&cap_id)),
            }
        }
    }
    kprintln!();
}

fn intent_help_topic(topic: &str) {
    match topic {
        "storage" | "store" | "fs" => {
            kprintln!();
            kprintln!("  Storage");
            kprintln!("  ───────");
            kprintln!("  store <name> <data>   Save object to content store");
            kprintln!("  fetch <name>          Load and display object");
            kprintln!("  delete <name>         Remove object");
            kprintln!("  list                  List all objects with hashes");
            kprintln!("  fsinfo                Disk usage and block stats");
            kprintln!();
        }
        "content" | "tools" | "cat" | "grep" => {
            kprintln!();
            kprintln!("  Content Tools");
            kprintln!("  ─────────────");
            kprintln!("  cat <name>              Display object contents");
            kprintln!("  grep <pattern> <name>   Search lines (case-insensitive)");
            kprintln!("  head <name> [n]         Show first n lines (default 10)");
            kprintln!("  wc <name>               Count lines, words, bytes");
            kprintln!("  hexdump <name> [n]      Hex dump (default 256 bytes)");
            kprintln!();
            kprintln!("  Redirect: cat mypage > copy   grep html mypage > matches");
            kprintln!();
        }
        "network" | "net" | "http" | "https" => {
            kprintln!();
            kprintln!("  Network");
            kprintln!("  ───────");
            kprintln!("  ping <host>              ICMP ping (IP or hostname)");
            kprintln!("  resolve <host>           DNS lookup");
            kprintln!("  traceroute <host>        Network path trace");
            kprintln!("  netstat                  Active connections");
            kprintln!("  net                      Interface info");
            kprintln!();
            kprintln!("  http  <host> [path]      HTTP GET (plaintext)");
            kprintln!("  https <host> [path]      HTTPS GET (TLS 1.3)");
            kprintln!("    Flags:  -h headers only  -b body only  -s silent");
            kprintln!("    Store:  https example.com / > mypage");
            kprintln!();
        }
        "exec" | "wasm" | "run" => {
            kprintln!();
            kprintln!("  Execution");
            kprintln!("  ─────────");
            kprintln!("  run <module> [args]   Execute WASM module from store");
            kprintln!("  add <a> <b>           Add two numbers [WASM]");
            kprintln!("  multiply <a> <b>      Multiply two numbers [WASM]");
            kprintln!();
        }
        "security" | "lock" | "caps" => {
            kprintln!();
            kprintln!("  Security");
            kprintln!("  ────────");
            kprintln!("  lock                  Lock system (clear keys)");
            kprintln!("  passwd                Change passphrase");
            kprintln!("  caps                  Show capability vault");
            kprintln!("  audit                 Security event log");
            kprintln!();
        }
        "config" | "set" | "settings" => {
            kprintln!();
            kprintln!("  Configuration");
            kprintln!("  ─────────────");
            kprintln!("  set <key> <value>     Set config value");
            kprintln!("  get <key>             Get config value");
            kprintln!("  config                Show all settings");
            kprintln!();
            kprintln!("  Keys: timezone (+2), keyboard (de_CH), lang (de)");
            kprintln!();
        }
        "disk" | "blk" => {
            kprintln!();
            kprintln!("  Disk");
            kprintln!("  ────");
            kprintln!("  disk                  Disk info");
            kprintln!("  disk read <sector>    Raw sector hex dump");
            kprintln!("  disk write <s> <txt>  Write text to sector");
            kprintln!();
        }
        _ => {
            // Main overview
            kprintln!();
            kprintln!("  nopeekOS");
            kprintln!("  ════════");
            kprintln!();
            kprintln!("  System:    status · time · about · clear · halt");
            kprintln!("  Storage:   store · fetch · delete · list · fsinfo");
            kprintln!("  Content:   cat · grep · head · wc · hexdump");
            kprintln!("  Network:   ping · resolve · http · https · traceroute · netstat");
            kprintln!("  Exec:      run · add · multiply");
            kprintln!("  Security:  lock · passwd · caps · audit");
            kprintln!("  Config:    set · get · config");
            kprintln!("  Disk:      disk read · disk write");
            kprintln!();
            kprintln!("  help <topic>  for details (storage, content, network, exec, security, config, disk)");
            kprintln!();
        }
    }
}

fn parse_two_ints(args: &str) -> Option<(i32, i32)> {
    let mut parts = args.trim().splitn(2, ' ');
    let a = parts.next()?.trim().parse::<i32>().ok()?;
    let b = parts.next()?.trim().parse::<i32>().ok()?;
    Some((a, b))
}

fn intent_wasm_add(args: &str) {
    use crate::wasm;
    let (a, b) = match parse_two_ints(args) {
        Some(v) => v,
        None => { kprintln!("[npk] Usage: add <a> <b>"); return; }
    };

    match wasm::execute(wasm::MODULE_ADD, "add", &[wasm::val_i32(a), wasm::val_i32(b)]) {
        Ok(result) => kprintln!("{}", result.output),
        Err(e) => kprintln!("[npk] WASM error: {}", e),
    }
}

fn intent_wasm_multiply(args: &str) {
    use crate::wasm;
    let (a, b) = match parse_two_ints(args) {
        Some(v) => v,
        None => { kprintln!("[npk] Usage: multiply <a> <b>"); return; }
    };

    match wasm::execute(wasm::MODULE_MULTIPLY, "multiply", &[wasm::val_i32(a), wasm::val_i32(b)]) {
        Ok(result) => kprintln!("{}", result.output),
        Err(e) => kprintln!("[npk] WASM error: {}", e),
    }
}

fn intent_echo(args: &str) { kprintln!("{}", args); }

fn intent_think(args: &str) {
    kprintln!();
    kprintln!("  [Intent: think]");
    kprintln!("  Question: {}", args);
    kprintln!();
    kprintln!("  AI reasoning not yet available.");
    kprintln!("  This will route to the neurofabric layer (Phase 7+).");
    kprintln!();
}

fn intent_about() {
    kprintln!();
    kprintln!("  nopeekOS – AI-native Operating System");
    kprintln!("  ──────────────────────────────────");
    kprintln!();
    kprintln!("  Not a Unix clone. Not POSIX. No legacy.");
    kprintln!("  Built for AI as the operator, humans as the conductor.");
    kprintln!();
    kprintln!("  Capabilities, not permissions. Intents, not commands.");
    kprintln!("  Content-addressed, not paths. Runtime-generated, not installed.");
    kprintln!();
    kprintln!("  Created in Luzern, Switzerland.");
    kprintln!();
}

fn intent_philosophy() {
    kprintln!();
    kprintln!("  What remains when you remove fifty years of assumptions?");
    kprintln!();
    kprintln!("  A capability vault, a WASM sandbox,");
    kprintln!("  an intent loop, and a human view.");
    kprintln!("  Everything else is generated.");
    kprintln!();
}

fn intent_store(args: &str, cap_id: CapId) {
    let mut parts = args.splitn(2, ' ');
    let name = match parts.next() {
        Some(n) if !n.is_empty() => n,
        _ => { kprintln!("[npk] Usage: store <name> <data>"); return; }
    };
    let data = parts.next().unwrap_or("");
    if data.is_empty() {
        kprintln!("[npk] Usage: store <name> <data>");
        return;
    }

    let path = resolve_path(name);
    // Auto-create parent directories
    if let Some(idx) = path.rfind('/') {
        ensure_parents(&path[..idx]);
    }
    match crate::npkfs::upsert(&path, data.as_bytes(), cap_id) {
        Ok(hash) => {
            kprint!("[npk] Stored '{}' ({} bytes, hash: ", path, data.len());
            for b in &hash[..4] { kprint!("{:02x}", b); }
            kprintln!("...)");
        }
        Err(e) => kprintln!("[npk] Store error: {}", e),
    }
}

fn intent_fetch(args: &str) {
    let name = args.trim();
    if name.is_empty() {
        kprintln!("[npk] Usage: fetch <name>");
        return;
    }
    let path = resolve_path(name);

    match crate::npkfs::fetch(&path) {
        Ok((data, _hash)) => {
            match core::str::from_utf8(&data) {
                Ok(s) => kprintln!("{}", s),
                Err(_) => {
                    kprintln!("[npk] ({} bytes, binary)", data.len());
                }
            }
        }
        Err(e) => kprintln!("[npk] Fetch error: {}", e),
    }
}

/// Helper: fetch an npkFS object and return its data.
/// Name is resolved relative to cwd.
fn fetch_object(name: &str) -> Option<alloc::vec::Vec<u8>> {
    let path = resolve_path(name);
    match crate::npkfs::fetch(&path) {
        Ok((data, _)) => Some(data),
        Err(e) => { kprintln!("[npk] '{}': {}", name, e); None }
    }
}

/// Parse "args > target" redirect syntax. Returns (args, Option<store_name>).
fn parse_redirect(args: &str) -> (&str, Option<&str>) {
    if let Some(idx) = args.rfind('>') {
        let target = args[idx + 1..].trim();
        let rest = args[..idx].trim();
        if target.is_empty() { (args, None) } else { (rest, Some(target)) }
    } else {
        (args, None)
    }
}

fn intent_cat(args: &str) {
    let (args, redirect) = parse_redirect(args);
    let name = args.trim();
    if name.is_empty() {
        kprintln!("[npk] Usage: cat <name> [> target]");
        return;
    }

    let data = match fetch_object(name) {
        Some(d) => d,
        None => return,
    };

    if let Some(target) = redirect {
        let target_path = resolve_path(target);
        match crate::npkfs::upsert(&target_path, &data, capability::CAP_NULL) {
            Ok(_) => kprintln!("[npk] Copied -> '{}' ({} bytes)", target_path, data.len()),
            Err(e) => kprintln!("[npk] Store error: {}", e),
        }
        return;
    }

    match core::str::from_utf8(&data) {
        Ok(text) => kprintln!("{}", text),
        Err(_) => kprintln!("[npk] ({} bytes, binary — use 'hexdump {}')", data.len(), name),
    }
}

fn intent_grep(args: &str) {
    let (args, redirect) = parse_redirect(args);
    let args = args.trim();

    let (pattern, name) = match args.split_once(' ') {
        Some((p, n)) => (p.trim(), n.trim()),
        None => {
            kprintln!("[npk] Usage: grep <pattern> <name> [> target]");
            return;
        }
    };

    let data = match fetch_object(name) {
        Some(d) => d,
        None => return,
    };

    let text = match core::str::from_utf8(&data) {
        Ok(t) => t,
        Err(_) => { kprintln!("[npk] '{}' is binary", name); return; }
    };

    let pattern_lower = alloc::string::String::from(pattern).to_ascii_lowercase();
    let mut matches = alloc::vec::Vec::new();
    let mut match_count = 0u32;

    for (i, line) in text.lines().enumerate() {
        let line_lower = alloc::string::String::from(line).to_ascii_lowercase();
        if line_lower.contains(pattern_lower.as_str()) {
            match_count += 1;
            if redirect.is_some() {
                matches.extend_from_slice(line.as_bytes());
                matches.push(b'\n');
            } else {
                kprintln!("  {:4}: {}", i + 1, line);
            }
        }
    }

    if let Some(target) = redirect {
        let target_path = resolve_path(target);
        match crate::npkfs::upsert(&target_path, &matches, capability::CAP_NULL) {
            Ok(_) => kprintln!("[npk] {} matches -> '{}'", match_count, target_path),
            Err(e) => kprintln!("[npk] Store error: {}", e),
        }
    } else if match_count == 0 {
        kprintln!("[npk] No matches for '{}' in '{}'", pattern, name);
    } else {
        kprintln!("[npk] {} matches", match_count);
    }
}

fn intent_head(args: &str) {
    let args = args.trim();
    let (name, count) = if let Some((n, c)) = args.rsplit_once(' ') {
        match c.parse::<usize>() {
            Ok(num) => (n.trim(), num),
            Err(_) => (args, 10),
        }
    } else {
        (args, 10)
    };

    if name.is_empty() {
        kprintln!("[npk] Usage: head <name> [lines]");
        return;
    }

    let data = match fetch_object(name) {
        Some(d) => d,
        None => return,
    };

    match core::str::from_utf8(&data) {
        Ok(text) => {
            for line in text.lines().take(count) {
                kprintln!("{}", line);
            }
        }
        Err(_) => kprintln!("[npk] '{}' is binary", name),
    }
}

fn intent_wc(args: &str) {
    let name = args.trim();
    if name.is_empty() {
        kprintln!("[npk] Usage: wc <name>");
        return;
    }

    let data = match fetch_object(name) {
        Some(d) => d,
        None => return,
    };

    let bytes = data.len();
    match core::str::from_utf8(&data) {
        Ok(text) => {
            let lines = text.lines().count();
            let words = text.split_whitespace().count();
            kprintln!("  {} lines  {} words  {} bytes  {}", lines, words, bytes, name);
        }
        Err(_) => {
            kprintln!("  {} bytes (binary)  {}", bytes, name);
        }
    }
}

fn intent_hexdump(args: &str) {
    let args = args.trim();
    // Optional limit: hexdump name 128
    let (name, limit) = if let Some((n, l)) = args.rsplit_once(' ') {
        match l.parse::<usize>() {
            Ok(num) => (n.trim(), num),
            Err(_) => (args, 256),
        }
    } else {
        (args, 256)
    };

    if name.is_empty() {
        kprintln!("[npk] Usage: hexdump <name> [bytes]");
        return;
    }

    let data = match fetch_object(name) {
        Some(d) => d,
        None => return,
    };

    let show = data.len().min(limit);
    for (i, chunk) in data[..show].chunks(16).enumerate() {
        kprint!("  {:04x}  ", i * 16);
        for (j, &b) in chunk.iter().enumerate() {
            kprint!("{:02x} ", b);
            if j == 7 { kprint!(" "); }
        }
        // Padding for short lines
        for j in chunk.len()..16 {
            kprint!("   ");
            if j == 7 { kprint!(" "); }
        }
        kprint!(" |");
        for &b in chunk {
            if b >= 0x20 && b <= 0x7E {
                kprint!("{}", b as char);
            } else {
                kprint!(".");
            }
        }
        kprintln!("|");
    }

    if show < data.len() {
        kprintln!("[npk] ({} of {} bytes shown)", show, data.len());
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

fn intent_mkdir(args: &str) {
    let dir = args.trim().trim_end_matches('/');
    if dir.is_empty() {
        kprintln!("[npk] Usage: mkdir <path>");
        return;
    }
    let resolved = resolve_path(dir);
    let marker = alloc::format!("{}/.dir", resolved);
    if crate::npkfs::exists(&marker) {
        kprintln!("[npk] Directory '{}' already exists", resolved);
        return;
    }
    ensure_parents(&resolved);
    kprintln!("[npk] Created '{}'", resolved);
}

fn intent_rmdir(args: &str) {
    let dir = args.trim().trim_end_matches('/');
    if dir.is_empty() || dir == "." {
        kprintln!("[npk] Usage: rmdir <path>");
        return;
    }
    let resolved = resolve_path(dir);
    let cwd = get_cwd();
    if resolved == cwd || cwd.starts_with(&alloc::format!("{}/", resolved)) {
        kprintln!("[npk] Cannot remove current working directory");
        return;
    }

    let prefix = alloc::format!("{}/", resolved);
    if let Ok(entries) = crate::npkfs::list() {
        let has_content = entries.iter().any(|(n, _, _)| {
            n.starts_with(prefix.as_str()) && !n.ends_with("/.dir")
        });
        if has_content {
            kprintln!("[npk] Directory '{}' is not empty", resolved);
            return;
        }
    }

    let marker = alloc::format!("{}/.dir", resolved);
    match crate::npkfs::delete(&marker) {
        Ok(()) => kprintln!("[npk] Removed '{}'", resolved),
        Err(_) => kprintln!("[npk] Directory '{}' not found", resolved),
    }
}

fn intent_delete(args: &str) {
    let name = args.trim();
    if name.is_empty() {
        kprintln!("[npk] Usage: delete <name>");
        return;
    }
    let path = resolve_path(name);

    match crate::npkfs::delete(&path) {
        Ok(()) => kprintln!("[npk] Deleted '{}'", path),
        Err(e) => kprintln!("[npk] Delete error: {}", e),
    }
}

fn intent_list(args: &str) {
    let filter = args.trim();
    // Use explicit arg, or cwd, or root
    let resolved = if !filter.is_empty() {
        resolve_path(filter)
    } else {
        get_cwd()
    };
    let prefix = if !resolved.is_empty() {
        let mut p = resolved;
        if !p.ends_with('/') { p.push('/'); }
        Some(p)
    } else {
        None
    };

    match crate::npkfs::list() {
        Ok(entries) => {
            // Filter and collect visible entries (hide .npk-* and .dir markers)
            let visible: alloc::vec::Vec<_> = entries.iter()
                .filter(|(name, _, _)| {
                    if name.starts_with(".npk-") { return false; }
                    if name.ends_with("/.dir") { return false; }
                    if let Some(ref pfx) = prefix {
                        return name.starts_with(pfx.as_str());
                    }
                    true
                })
                .collect();

            // Collect known dirs (from .dir markers and from object prefixes)
            let mut dirs: alloc::vec::Vec<alloc::string::String> = alloc::vec::Vec::new();
            for (name, _, _) in &entries {
                // .dir markers define empty dirs (register all levels)
                if let Some(dir) = name.strip_suffix("/.dir") {
                    let mut remaining = dir;
                    loop {
                        let d = alloc::string::String::from(remaining);
                        if !dirs.contains(&d) { dirs.push(d); }
                        match remaining.rfind('/') {
                            Some(idx) => remaining = &remaining[..idx],
                            None => break,
                        }
                    }
                }
                // Objects with / define implicit dirs (all levels)
                let mut remaining = name.as_str();
                while let Some(idx) = remaining.rfind('/') {
                    remaining = &remaining[..idx];
                    let d = alloc::string::String::from(remaining);
                    if !dirs.contains(&d) { dirs.push(d); }
                }
            }
            dirs.sort();

            kprintln!();

            if visible.is_empty() && dirs.is_empty() {
                kprintln!("  (empty)");
            } else {
                // Determine the current "depth" we're listing
                let prefix_str = prefix.as_deref().unwrap_or("");

                // Show subdirectories at this level
                let mut shown_dirs: alloc::vec::Vec<&str> = alloc::vec::Vec::new();
                for dir in &dirs {
                    let rel = if !prefix_str.is_empty() {
                        match dir.strip_prefix(prefix_str) {
                            Some(r) => r,
                            None => continue,
                        }
                    } else {
                        dir.as_str()
                    };
                    // Only show immediate children (no nested /, no self-reference)
                    if rel.is_empty() || rel == "." || rel.contains('/') { continue; }
                    if shown_dirs.contains(&rel) { continue; }
                    shown_dirs.push(rel);

                    // Count objects in this dir
                    let dir_prefix = alloc::format!("{}/", dir);
                    let count = entries.iter()
                        .filter(|(n, _, _)| n.starts_with(dir_prefix.as_str()) && !n.ends_with("/.dir") && !n.starts_with(".npk-"))
                        .count();
                    if count == 0 {
                        kprintln!("  {}/", rel);
                    } else {
                        kprintln!("  {}/  ({} objects)", rel, count);
                    }
                }

                // Show files at this level (no / after prefix)
                for (name, size, hash) in &visible {
                    let rel = if !prefix_str.is_empty() {
                        match name.strip_prefix(prefix_str) {
                            Some(r) => r,
                            None => continue,
                        }
                    } else {
                        name.as_str()
                    };
                    // Only show files at this level (not in subdirs)
                    if rel.contains('/') { continue; }

                    kprint!("  {:<24} ", rel);
                    if *size >= 1024 {
                        kprint!("{:>6} KB  ", size / 1024);
                    } else {
                        kprint!("{:>6} B   ", size);
                    }
                    for b in &hash[..4] { kprint!("{:02x}", b); }
                    kprintln!();
                }
            }

            if let Some((_, free, count, _)) = crate::npkfs::stats() {
                kprintln!();
                kprintln!("  {} objects, {} free blocks", count, free);
            }
            kprintln!();
        }
        Err(e) => kprintln!("[npk] List error: {}", e),
    }
}

fn intent_run(args: &str) {
    use crate::{wasm, npkfs, capability};
    use wasmi::Val;

    let mut parts = args.trim().splitn(2, ' ');
    let module_name = match parts.next() {
        Some(n) if !n.is_empty() => n,
        _ => { kprintln!("[npk] Usage: run <module> [args...]"); return; }
    };
    let arg_str = parts.next().unwrap_or("");

    // Load module from npkFS: try cwd-relative, then sys/wasm/
    let resolved = resolve_path(module_name);
    let sys_path = alloc::format!("sys/wasm/{}", module_name);
    let (wasm_bytes, hash) = match npkfs::fetch(&resolved) {
        Ok(v) => v,
        Err(_) => match npkfs::fetch(&sys_path) {
            Ok(v) => v,
            Err(e) => { kprintln!("[npk] Module '{}': {}", module_name, e); return; }
        }
    };

    // BLAKE3 integrity verified by npkfs::fetch

    // Delegate a capability for this module: READ + EXECUTE, 60s TTL
    let module_cap = match capability::create_module_cap(
        capability::Rights::READ | capability::Rights::EXECUTE,
        Some(6000), // 60 seconds at 100Hz
    ) {
        Ok(id) => id,
        Err(e) => { kprintln!("[npk] Cap delegation failed: {}", e); return; }
    };

    kprint!("[npk] Running '{}' (hash: ", module_name);
    for b in &hash[..4] { kprint!("{:02x}", b); }
    kprintln!("..., cap: {:08x})", capability::short_id(&module_cap));

    // Parse args as i32 values
    let args_vec: alloc::vec::Vec<Val> = arg_str.split_whitespace()
        .filter_map(|s| s.parse::<i32>().ok())
        .map(|v| Val::I32(v))
        .collect();

    // Determine function name: if no args, try _start; otherwise use module name
    let func_name = if args_vec.is_empty() { "_start" } else { module_name };

    match wasm::execute_sandboxed(&wasm_bytes, func_name, &args_vec, module_cap) {
        Ok(result) => {
            if !result.output.is_empty() {
                kprintln!("{}", result.output);
            }
        }
        Err(e) => kprintln!("[npk] Execution error: {}", e),
    }
}

/// Store built-in WASM modules to npkFS on first boot.
pub fn bootstrap_wasm() {
    use crate::{wasm, npkfs};

    if !npkfs::is_mounted() { return; }

    let modules: &[(&str, &[u8])] = &[
        ("sys/wasm/hello", wasm::MODULE_HELLO),
        ("sys/wasm/fib", wasm::MODULE_FIB),
        ("sys/wasm/add", wasm::MODULE_ADD),
        ("sys/wasm/multiply", wasm::MODULE_MULTIPLY),
    ];

    let mut stored = 0;
    for (name, data) in modules {
        if npkfs::fetch(name).is_err() {
            if npkfs::store(name, data, capability::CAP_NULL).is_ok() {
                stored += 1;
            }
        }
    }
    if stored > 0 {
        kprintln!("[npk] Bootstrap: stored {} WASM modules", stored);
    }
}

/// Create initial directory structure and set cwd to home.
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

pub fn setup_home() {
    let home = home_dir();
    ensure_parents(&home);
    set_cwd(&home);
}

fn intent_traceroute(args: &str) {
    let target = args.trim();
    if target.is_empty() {
        kprintln!("[npk] Usage: traceroute <ip or hostname>");
        return;
    }

    let ip = if let Some(ip) = parse_ip(target) {
        ip
    } else {
        match crate::net::dns::resolve(target) {
            Some(ip) => {
                kprintln!("[npk] {} -> {}.{}.{}.{}", target, ip[0], ip[1], ip[2], ip[3]);
                ip
            }
            None => { kprintln!("[npk] Could not resolve '{}'", target); return; }
        }
    };

    // ARP resolve gateway
    crate::net::arp::request([10, 0, 2, 2]);
    for _ in 0..50_000 { crate::net::poll(); core::hint::spin_loop(); }

    kprintln!("[npk] Traceroute to {}.{}.{}.{} (max 20 hops)", ip[0], ip[1], ip[2], ip[3]);

    for ttl in 1..=20u8 {
        crate::net::icmp::ping_ttl(ip, ttl as u16, ttl);

        let t0 = crate::interrupts::ticks();
        let mut _found = false;

        loop {
            crate::net::poll();

            if let Some(from) = crate::net::icmp::ttl_expired_from() {
                kprintln!("  {:>2}  {}.{}.{}.{}", ttl, from[0], from[1], from[2], from[3]);
                _found = true;
                break;
            }
            if crate::net::icmp::ping_received() {
                kprintln!("  {:>2}  {}.{}.{}.{} (destination)", ttl, ip[0], ip[1], ip[2], ip[3]);
                return; // reached destination
            }
            if crate::interrupts::ticks() - t0 > 100 { // 1s per hop
                kprintln!("  {:>2}  *", ttl);
                _found = true;
                break;
            }
            core::hint::spin_loop();
        }
    }
}

fn intent_netstat() {
    let conns = crate::net::tcp::list_connections();
    kprintln!();
    kprintln!("  Active TCP Connections");
    kprintln!("  ─────────────────────");
    if conns.is_empty() {
        kprintln!("  (none)");
    } else {
        kprintln!("  {:>6}  {:>21}  {}", "Local", "Remote", "State");
        for (lport, rip, rport, state) in &conns {
            kprintln!("  {:>6}  {}.{}.{}.{}:{:<5}  {}",
                lport, rip[0], rip[1], rip[2], rip[3], rport, state);
        }
    }
    kprintln!();
}

fn intent_http(args: &str) {
    do_http_request(args, false);
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

fn intent_resolve(args: &str) {
    let name = args.trim();
    if name.is_empty() {
        kprintln!("[npk] Usage: resolve <hostname>");
        return;
    }
    match crate::net::dns::resolve(name) {
        Some(ip) => kprintln!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]),
        None => kprintln!("[npk] Could not resolve '{}'", name),
    }
}

fn intent_time() {
    if crate::net::ntp::unix_time().is_none() {
        kprintln!("[npk] Syncing time...");
        crate::net::ntp::sync_via_dns("pool.ntp.org");
    }
    match crate::net::ntp::unix_time() {
        Some(t) => kprintln!("{}", crate::net::ntp::format_time(t)),
        None => kprintln!("[npk] Time unavailable. No RTC or network."),
    }
}

fn intent_ping(args: &str) {
    let host = args.trim();
    if host.is_empty() {
        kprintln!("[npk] Usage: ping <host or ip>");
        return;
    }

    let ip = if let Some(ip) = parse_ip(host) {
        ip
    } else {
        match crate::net::dns::resolve(host) {
            Some(ip) => {
                kprintln!("[npk] {} -> {}.{}.{}.{}", host, ip[0], ip[1], ip[2], ip[3]);
                ip
            }
            None => {
                kprintln!("[npk] Could not resolve '{}'", host);
                return;
            }
        }
    };

    // Send ARP first to resolve gateway
    crate::net::arp::request([10, 0, 2, 2]);
    // Brief poll to get ARP reply
    for _ in 0..100_000 {
        crate::net::poll();
        core::hint::spin_loop();
    }

    crate::net::icmp::ping(ip, 1);

    // Poll for reply
    let t0 = crate::interrupts::ticks();
    loop {
        crate::net::poll();
        if crate::net::icmp::ping_received() {
            break;
        }
        let elapsed = crate::interrupts::ticks() - t0;
        if elapsed > 300 {
            kprintln!("[npk] Ping timeout");
            break;
        }
        core::hint::spin_loop();
    }
}

fn intent_net_info() {
    if let Some(mac) = crate::virtio_net::mac() {
        let ip = crate::net::arp::our_ip();
        kprintln!();
        kprintln!("  Network (virtio-net)");
        kprintln!("  ───────────────────");
        kprintln!("  MAC:     {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]);
        kprintln!("  IPv4:    {}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]);
        kprintln!("  Gateway: 10.0.2.2 (QEMU user-mode)");
        kprintln!("  Status:  online");
        kprintln!();
    } else {
        kprintln!("[npk] Network not available");
    }
}

fn intent_fsinfo() {
    match crate::npkfs::stats() {
        Some((total, free, objects, gen)) => {
            let total_mb = total * crate::npkfs::BLOCK_SIZE as u64 / (1024 * 1024);
            let free_mb = free * crate::npkfs::BLOCK_SIZE as u64 / (1024 * 1024);
            let used = total - free;
            kprintln!();
            kprintln!("  npkFS – Content-Addressed Object Store");
            kprintln!("  ──────────────────────────────────────");
            kprintln!("  Total:       {} blocks ({} MB)", total, total_mb);
            kprintln!("  Used:        {} blocks", used);
            kprintln!("  Free:        {} blocks ({} MB)", free, free_mb);
            kprintln!("  Objects:     {}", objects);
            kprintln!("  Generation:  {}", gen);
            kprintln!("  Hash:        BLAKE3");
            kprintln!("  TRIM:        {}", if crate::virtio_blk::has_discard() { "active" } else { "unavailable" });
            kprintln!();
        }
        None => kprintln!("[npk] Filesystem not mounted"),
    }
}

fn intent_disk_info() {
    use crate::virtio_blk;
    match virtio_blk::capacity() {
        Some(cap) => {
            let mb = (cap * virtio_blk::SECTOR_SIZE as u64) / (1024 * 1024);
            let blocks = virtio_blk::block_count().unwrap_or(0);
            kprintln!();
            kprintln!("  Block Device (virtio-blk)");
            kprintln!("  ────────────────────────");
            kprintln!("  Capacity:  {} sectors / {} blocks ({} MB)", cap, blocks, mb);
            kprintln!("  Sector:    {} bytes", virtio_blk::SECTOR_SIZE);
            kprintln!("  Block:     {} bytes", virtio_blk::BLOCK_SIZE);
            kprintln!("  TRIM:      {}", if virtio_blk::has_discard() { "supported" } else { "not available" });
            kprintln!("  Status:    online");
            kprintln!();
        }
        None => kprintln!("[npk] No block device available"),
    }
}

fn intent_disk_read(args: &str) {
    use crate::virtio_blk;

    let sector: u64 = match args.parse() {
        Ok(n) => n,
        Err(_) => { kprintln!("[npk] Usage: disk read <sector>"); return; }
    };

    let mut buf = [0u8; virtio_blk::SECTOR_SIZE];
    match virtio_blk::read_sector(sector, &mut buf) {
        Ok(()) => {
            kprintln!();
            kprintln!("  Sector {}:", sector);
            hex_dump(&buf);
            kprintln!();
        }
        Err(e) => kprintln!("[npk] Read error: {}", e),
    }
}

fn intent_disk_write(args: &str) {
    use crate::virtio_blk;

    let mut parts = args.splitn(2, ' ');
    let sector: u64 = match parts.next().and_then(|s| s.parse().ok()) {
        Some(n) => n,
        None => { kprintln!("[npk] Usage: disk write <sector> <text>"); return; }
    };
    let text = parts.next().unwrap_or("");
    if text.is_empty() {
        kprintln!("[npk] Usage: disk write <sector> <text>");
        return;
    }

    let mut buf = [0u8; virtio_blk::SECTOR_SIZE];
    let len = text.len().min(virtio_blk::SECTOR_SIZE);
    buf[..len].copy_from_slice(&text.as_bytes()[..len]);

    match virtio_blk::write_sector(sector, &buf) {
        Ok(()) => kprintln!("[npk] Wrote {} bytes to sector {}", len, sector),
        Err(e) => kprintln!("[npk] Write error: {}", e),
    }
}

fn hex_dump(buf: &[u8; 512]) {
    let mut prev = [0xFFu8; 16]; // impossible first value
    let mut collapsed = false;

    for row in 0..32 {
        let off = row * 16;
        let line = &buf[off..off + 16];

        if row > 0 && line == &prev[..] {
            if !collapsed {
                kprintln!("  *");
                collapsed = true;
            }
            continue;
        }
        collapsed = false;
        prev.copy_from_slice(line);

        kprint!("  {:04x}: ", off);
        for (j, &b) in line.iter().enumerate() {
            kprint!("{:02x} ", b);
            if j == 7 { kprint!(" "); }
        }
        kprint!(" |");
        for &b in line {
            if b >= 0x20 && b < 0x7F {
                kprint!("{}", b as char);
            } else {
                kprint!(".");
            }
        }
        kprintln!("|");
    }
}

fn intent_https(args: &str) {
    do_http_request(args, true);
}

const HTTP_MAX_RESPONSE: usize = 128 * 1024; // 128 KB

/// Flags parsed from HTTP/HTTPS arguments.
struct HttpFlags {
    headers_only: bool,  // -h: show only headers
    body_only: bool,     // -b: show only body
    silent: bool,        // -s: no status output
}

/// Parse flags from anywhere in the args, return flags + cleaned args.
fn parse_http_args(args: &str) -> (HttpFlags, alloc::string::String) {
    let mut flags = HttpFlags { headers_only: false, body_only: false, silent: false };
    let mut cleaned = alloc::string::String::new();

    for part in args.split_whitespace() {
        match part {
            "-h" => flags.headers_only = true,
            "-b" => flags.body_only = true,
            "-s" => flags.silent = true,
            _ => {
                if !cleaned.is_empty() { cleaned.push(' '); }
                cleaned.push_str(part);
            }
        }
    }

    (flags, cleaned)
}

fn do_http_request(args: &str, use_tls: bool) {
    let proto = if use_tls { "https" } else { "http" };
    let (flags, url) = parse_http_args(args);
    let url = url.as_str();

    if url.is_empty() {
        kprintln!("[npk] Usage: {} [-h|-b|-s] <host> [path] [> name]", proto);
        kprintln!("[npk]   -h  Headers only");
        kprintln!("[npk]   -b  Body only (no headers)");
        kprintln!("[npk]   -s  Silent (no status messages)");
        return;
    }

    // Parse "host path" or "host/path"
    let (host, path) = if let Some(idx) = url.find(' ') {
        (&url[..idx], url[idx + 1..].trim())
    } else if let Some(idx) = url.find('/') {
        (&url[..idx], &url[idx..])
    } else {
        (url, "/")
    };
    let host = host.trim();

    // Check for "> name" store redirect
    let store_as = if let Some(idx) = path.find('>') {
        let name = path[idx + 1..].trim();
        if name.is_empty() { None } else { Some(alloc::string::String::from(name)) }
    } else {
        None
    };
    let path = if let Some(idx) = path.find('>') { path[..idx].trim() } else { path };
    let path = if path.is_empty() { "/" } else { path };

    // Resolve hostname
    let ip = if let Some(ip) = parse_ip(host) {
        ip
    } else {
        match crate::net::dns::resolve(host) {
            Some(ip) => {
                if !flags.silent {
                    kprintln!("[npk] {} -> {}.{}.{}.{}", host, ip[0], ip[1], ip[2], ip[3]);
                }
                ip
            }
            None => {
                kprintln!("[npk] Could not resolve '{}'", host);
                return;
            }
        }
    };

    // ARP resolve gateway
    crate::net::arp::request([10, 0, 2, 2]);
    for _ in 0..50_000 { crate::net::poll(); core::hint::spin_loop(); }

    let port = if use_tls { 443u16 } else { 80 };
    if !flags.silent {
        kprintln!("[npk] Connecting to {}.{}.{}.{}:{}...", ip[0], ip[1], ip[2], ip[3], port);
    }

    let handle = match crate::net::tcp::connect(ip, port) {
        Ok(h) => h,
        Err(e) => { kprintln!("[npk] TCP error: {}", e); return; }
    };

    // TLS handshake (if HTTPS)
    let mut tls_session = if use_tls {
        if !flags.silent {
            kprintln!("[npk] TLS 1.3 handshake with '{}'...", host);
        }
        match crate::tls::tls_connect(handle, host) {
            Ok(s) => {
                if !flags.silent {
                    kprintln!("[npk] TLS established (ChaCha20-Poly1305)");
                }
                Some(s)
            }
            Err(e) => {
                kprintln!("[npk] TLS error: {}", e);
                let _ = crate::net::tcp::close(handle);
                return;
            }
        }
    } else {
        None
    };

    // Send HTTP GET
    let http_ver = if use_tls { "1.1" } else { "1.0" };
    let request = alloc::format!(
        "GET {} HTTP/{}\r\nHost: {}\r\nUser-Agent: nopeekOS/0.1\r\nAccept: */*\r\nConnection: close\r\n\r\n",
        path, http_ver, host
    );

    let send_ok = if let Some(ref mut sess) = tls_session {
        crate::tls::tls_send(sess, request.as_bytes()).is_ok()
    } else {
        crate::net::tcp::send(handle, request.as_bytes()).is_ok()
    };
    if !send_ok {
        kprintln!("[npk] Send error");
        if let Some(ref mut sess) = tls_session { let _ = crate::tls::tls_close(sess); }
        else { let _ = crate::net::tcp::close(handle); }
        return;
    }

    // Receive response
    let mut response = alloc::vec::Vec::new();
    let mut buf = [0u8; 4096];

    if let Some(ref mut sess) = tls_session {
        let mut empty_count = 0;
        loop {
            match crate::tls::tls_recv(sess, &mut buf) {
                Ok(0) => {
                    empty_count += 1;
                    if empty_count > 5 && response.is_empty() { break; }
                    if empty_count > 2 && !response.is_empty() { break; }
                }
                Ok(n) => { response.extend_from_slice(&buf[..n]); empty_count = 0; }
                Err(_) => break,
            }
            if response.len() > HTTP_MAX_RESPONSE { break; }
        }
        let _ = crate::tls::tls_close(sess);
    } else {
        loop {
            match crate::net::tcp::recv_blocking(handle, &mut buf, 500) {
                Ok(0) => break,
                Ok(n) => response.extend_from_slice(&buf[..n]),
                Err(_) => break,
            }
            if response.len() > HTTP_MAX_RESPONSE { break; }
        }
        let _ = crate::net::tcp::close(handle);
    }

    if response.is_empty() {
        kprintln!("[npk] No response received");
        return;
    }

    // Find header/body boundary
    let header_end = response.windows(4)
        .position(|w| w == b"\r\n\r\n")
        .unwrap_or(response.len());
    let body_start = if header_end < response.len() { header_end + 4 } else { response.len() };

    if let Some(name) = store_as {
        let store_path = resolve_path(&name);
        let body = &response[body_start..];
        match crate::npkfs::upsert(&store_path, body, capability::CAP_NULL) {
            Ok(hash) => {
                kprint!("[npk] Stored '{}' ({} bytes, hash: ", store_path, body.len());
                for b in &hash[..4] { kprint!("{:02x}", b); }
                kprintln!("...)");
            }
            Err(e) => kprintln!("[npk] Store error: {}", e),
        }
        return;
    }

    // Display based on flags
    if flags.headers_only {
        if let Ok(hdrs) = core::str::from_utf8(&response[..header_end]) {
            kprintln!("{}", hdrs);
        }
    } else if flags.body_only {
        print_response_data(&response[body_start..]);
    } else {
        // Full response: headers + body
        print_response_data(&response);
    }

    if response.len() >= HTTP_MAX_RESPONSE {
        kprintln!("\n[npk] (truncated at {} KB)", HTTP_MAX_RESPONSE / 1024);
    }
}

fn print_response_data(data: &[u8]) {
    match core::str::from_utf8(data) {
        Ok(text) => kprintln!("{}", text),
        Err(_) => kprintln!("[npk] ({} bytes, binary)", data.len()),
    }
}

fn intent_set(args: &str) {
    let args = args.trim();
    if let Some((key, value)) = args.split_once(' ') {
        let key = key.trim();
        let value = value.trim();
        if key.is_empty() || value.is_empty() {
            kprintln!("[npk] Usage: set <key> <value>");
            return;
        }
        crate::config::set(key, value);
        kprintln!("[npk] {} = {}", key, value);
    } else {
        kprintln!("[npk] Usage: set <key> <value>");
        kprintln!("[npk] Keys: timezone, keyboard, lang");
        kprintln!("[npk] Example: set timezone +2");
    }
}

fn intent_get(args: &str) {
    let key = args.trim();
    if key.is_empty() {
        kprintln!("[npk] Usage: get <key>");
        return;
    }
    match crate::config::get(key) {
        Some(val) => kprintln!("{} = {}", key, val),
        None => kprintln!("[npk] '{}' not set", key),
    }
}

fn intent_config() {
    let entries = crate::config::list();
    if entries.is_empty() {
        kprintln!("[npk] No configuration set.");
        kprintln!("[npk] Use 'set <key> <value>' to configure.");
    } else {
        kprintln!();
        for (k, v) in &entries {
            kprintln!("  {} = {}", k, v);
        }
        kprintln!();
    }
    kprintln!("[npk] Available keys:");
    for (key, desc) in crate::config::KNOWN_KEYS {
        kprintln!("  {:12} {}", key, desc);
    }
}

fn intent_halt() -> ! {
    kprintln!();
    kprintln!("[npk] Shutting down...");
    kprintln!("[npk] Goodbye.");
    kprintln!();
    unsafe {
        core::arch::asm!("out dx, al", in("dx") 0xf4u16, in("al") 0u8);
        loop { core::arch::asm!("cli; hlt"); }
    }
}
