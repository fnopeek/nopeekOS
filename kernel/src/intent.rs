//! Intent Loop
//!
//! Not a shell. Takes intents, not commands.
//! Every intent requires a valid capability token.

use crate::capability::{self, CapId, Vault, Rights};
use crate::{kprint, kprintln, crypto, serial};
use spin::Mutex;

const INPUT_BUF_SIZE: usize = 512;

pub fn run_loop(vault: &'static Mutex<Vault>, session_id: CapId) -> ! {
    let mut input_buf = [0u8; INPUT_BUF_SIZE];

    loop {
        kprint!("npk> ");

        // Poll network while waiting for input
        loop {
            crate::net::poll();
            let serial = crate::serial::SERIAL.lock();
            if serial.has_data() {
                drop(serial);
                break;
            }
            drop(serial);
            unsafe { core::arch::asm!("hlt"); }
        }

        let serial_port = serial::SERIAL.lock();
        let len = serial_port.read_line(&mut input_buf);
        drop(serial_port);

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
                kprintln!("[npk] Unlocked.");
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
        "fetch" | "load" | "get" => {
            if require_cap(vault, &session, Rights::READ, "fetch") {
                intent_fetch(args);
            }
        }
        "delete" | "rm" | "remove" => {
            if require_cap(vault, &session, Rights::WRITE, "delete") {
                intent_delete(args);
            }
        }
        "list" | "ls" | "objects" => {
            if require_cap(vault, &session, Rights::READ, "list") {
                intent_list();
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

        "clear" | "cls" => {
            // ANSI escape: clear screen + cursor home
            kprint!("\x1B[2J\x1B[H");
        }

        // Unrestricted intents (informational)
        "help" | "?" => intent_help(),
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
    kprintln!("  Token IDs:      128-bit random (xorshift128+)");
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

fn intent_help() {
    kprintln!();
    kprintln!("  nopeekOS Intent Interface");
    kprintln!("  ──────────────────────");
    kprintln!();
    kprintln!("  This is not a shell. You express intents, not commands.");
    kprintln!("  Every intent is checked against your capability token.");
    kprintln!();
    kprintln!("  Available intents:");
    kprintln!("    status       System overview          (requires READ)");
    kprintln!("    caps         Capability vault info    (requires READ)");
    kprintln!("    audit        Recent audit log         (requires AUDIT)");
    kprintln!("    add <a> <b>  Add two numbers [WASM]   (requires EXECUTE)");
    kprintln!("    multiply <a> <b>  Multiply [WASM]   (requires EXECUTE)");
    kprintln!("    store <n> <data>  Store object            (requires WRITE)");
    kprintln!("    fetch <name>     Fetch object            (requires READ)");
    kprintln!("    delete <name>    Delete object           (requires WRITE)");
    kprintln!("    list             List all objects        (requires READ)");
    kprintln!("    fsinfo           Filesystem info         (requires READ)");
    kprintln!("    disk              Block device info      (requires READ)");
    kprintln!("    disk read <n>     Read sector n          (requires READ)");
    kprintln!("    disk write <n> <s>  Write to sector n    (requires WRITE)");
    kprintln!("    run <module> [args]  Run WASM from npkFS  (requires EXECUTE)");
    kprintln!("    ping <host>  ICMP ping               (requires EXECUTE)");
    kprintln!("    resolve <h>  DNS lookup               (requires READ)");
    kprintln!("    http <h> [p] HTTP GET                 (requires EXECUTE)");
    kprintln!("    https <h> [p] HTTPS GET (TLS 1.3)    (requires EXECUTE)");
    kprintln!("    traceroute   Trace network path       (requires EXECUTE)");
    kprintln!("    netstat      Network connections       (requires READ)");
    kprintln!("    net          Network interface info    (requires READ)");
    kprintln!("    time         Current time              (requires READ)");
    kprintln!("    lock         Lock system (clear keys, require passphrase)");
    kprintln!("    passwd       Change passphrase");
    kprintln!("    echo <text>  Echo text");
    kprintln!("    about        About nopeekOS");
    kprintln!("    halt         Shutdown system          (requires EXECUTE)");
    kprintln!();
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

    match crate::npkfs::store(name, data.as_bytes(), cap_id) {
        Ok(hash) => {
            kprint!("[npk] Stored '{}' ({} bytes, hash: ", name, data.len());
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

    match crate::npkfs::fetch(name) {
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

fn intent_delete(args: &str) {
    let name = args.trim();
    if name.is_empty() {
        kprintln!("[npk] Usage: delete <name>");
        return;
    }

    match crate::npkfs::delete(name) {
        Ok(()) => kprintln!("[npk] Deleted '{}'", name),
        Err(e) => kprintln!("[npk] Delete error: {}", e),
    }
}

fn intent_list() {
    match crate::npkfs::list() {
        Ok(entries) => {
            kprintln!();
            if entries.is_empty() {
                kprintln!("  (no objects stored)");
            } else {
                for (name, size, hash) in &entries {
                    kprint!("  {:<20} {:>8} B  ", name, size);
                    for b in &hash[..4] { kprint!("{:02x}", b); }
                    kprintln!("...");
                }
            }
            if let Some((total, free, count, gen)) = crate::npkfs::stats() {
                kprintln!();
                kprintln!("  {} objects, {} free blocks / {} total (gen {})",
                    count, free, total, gen);
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

    // Load module from npkFS
    let (wasm_bytes, hash) = match npkfs::fetch(module_name) {
        Ok(v) => v,
        Err(e) => { kprintln!("[npk] Module '{}': {}", module_name, e); return; }
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
        ("hello", wasm::MODULE_HELLO),
        ("fib", wasm::MODULE_FIB),
        ("add", wasm::MODULE_ADD),
        ("multiply", wasm::MODULE_MULTIPLY),
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
    let url = args.trim();
    if url.is_empty() {
        kprintln!("[npk] Usage: http <host> [path]");
        return;
    }

    // Parse "host/path" or "host path"
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
                kprintln!("[npk] {} -> {}.{}.{}.{}", host, ip[0], ip[1], ip[2], ip[3]);
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

    // TCP connect
    kprintln!("[npk] Connecting to {}.{}.{}.{}:80...", ip[0], ip[1], ip[2], ip[3]);
    let handle = match crate::net::tcp::connect(ip, 80) {
        Ok(h) => h,
        Err(e) => { kprintln!("[npk] TCP error: {}", e); return; }
    };

    // Send HTTP GET
    let request = alloc::format!(
        "GET {} HTTP/1.0\r\nHost: {}\r\nUser-Agent: nopeekOS/0.1\r\nConnection: close\r\n\r\n",
        path, host
    );
    if let Err(e) = crate::net::tcp::send(handle, request.as_bytes()) {
        kprintln!("[npk] Send error: {}", e);
        let _ = crate::net::tcp::close(handle);
        return;
    }

    // Receive response
    let mut response = alloc::vec::Vec::new();
    let mut buf = [0u8; 2048];
    loop {
        match crate::net::tcp::recv_blocking(handle, &mut buf, 500) {
            Ok(0) => break,
            Ok(n) => response.extend_from_slice(&buf[..n]),
            Err(_) => break,
        }
        if response.len() > 8192 { break; } // limit output
    }

    let _ = crate::net::tcp::close(handle);

    if response.is_empty() {
        kprintln!("[npk] No response received");
        return;
    }

    // Strip HTTP headers if present (find \r\n\r\n)
    let body_start = response.windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|i| i + 4)
        .unwrap_or(0);

    if let Some(name) = store_as {
        // Store response body in npkFS
        let body = &response[body_start..];
        match crate::npkfs::store(&name, body, capability::CAP_NULL) {
            Ok(hash) => {
                kprint!("[npk] Stored '{}' ({} bytes, hash: ", name, body.len());
                for b in &hash[..4] { kprint!("{:02x}", b); }
                kprintln!("...)");
            }
            Err(e) => kprintln!("[npk] Store error: {}", e),
        }
    } else {
        // Print response body
        let body = &response[body_start..];
        match core::str::from_utf8(body) {
            Ok(s) => {
                let display = if s.len() > 2048 { &s[..2048] } else { s };
                kprintln!("{}", display);
            }
            Err(_) => kprintln!("[npk] ({} bytes, binary response)", body.len()),
        }
    }
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
    match crate::net::ntp::unix_time() {
        Some(t) => kprintln!("{}", crate::net::ntp::format_time(t)),
        None => kprintln!("[npk] Time not synced (run 'ping' first to init network)"),
    }
}

fn intent_ping(args: &str) {
    let ip_str = args.trim();
    if ip_str.is_empty() {
        kprintln!("[npk] Usage: ping <ip>");
        return;
    }

    let parts: alloc::vec::Vec<&str> = ip_str.split('.').collect();
    if parts.len() != 4 {
        kprintln!("[npk] Invalid IP format");
        return;
    }
    let mut ip = [0u8; 4];
    for (i, p) in parts.iter().enumerate() {
        ip[i] = match p.parse::<u8>() {
            Ok(v) => v,
            Err(_) => { kprintln!("[npk] Invalid IP octet"); return; }
        };
    }

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
    use crate::tls;

    let url = args.trim();
    if url.is_empty() {
        kprintln!("[npk] Usage: https <host> [path]");
        return;
    }

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
                kprintln!("[npk] {} -> {}.{}.{}.{}", host, ip[0], ip[1], ip[2], ip[3]);
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

    // TCP connect on port 443
    kprintln!("[npk] Connecting to {}.{}.{}.{}:443...", ip[0], ip[1], ip[2], ip[3]);
    let handle = match crate::net::tcp::connect(ip, 443) {
        Ok(h) => h,
        Err(e) => { kprintln!("[npk] TCP error: {}", e); return; }
    };

    // TLS 1.3 handshake
    kprintln!("[npk] TLS 1.3 handshake with '{}'...", host);
    let mut session = match tls::tls_connect(handle, host) {
        Ok(s) => {
            kprintln!("[npk] TLS established (ChaCha20-Poly1305)");
            s
        }
        Err(e) => {
            kprintln!("[npk] TLS error: {}", e);
            let _ = crate::net::tcp::close(handle);
            return;
        }
    };

    // Send HTTP/1.1 GET over TLS
    let request = alloc::format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: nopeekOS/0.1\r\nConnection: close\r\n\r\n",
        path, host
    );
    if let Err(e) = tls::tls_send(&mut session, request.as_bytes()) {
        kprintln!("[npk] TLS send error: {}", e);
        let _ = tls::tls_close(&mut session);
        return;
    }

    // Receive response
    let mut response = alloc::vec::Vec::new();
    let mut buf = [0u8; 4096];
    let mut empty_count = 0;
    loop {
        match tls::tls_recv(&mut session, &mut buf) {
            Ok(0) => {
                // Skip non-app records (NewSessionTicket, CCS)
                empty_count += 1;
                if empty_count > 5 && response.is_empty() { break; }
                if empty_count > 2 && !response.is_empty() { break; }
                continue;
            }
            Ok(n) => {
                response.extend_from_slice(&buf[..n]);
                empty_count = 0;
            }
            Err(_) => break,
        }
        if response.len() > 32768 { break; }
    }

    let _ = tls::tls_close(&mut session);

    if response.is_empty() {
        kprintln!("[npk] No response received");
        return;
    }

    // Strip HTTP headers
    let body_start = response.windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|i| i + 4)
        .unwrap_or(0);

    if let Some(name) = store_as {
        let body = &response[body_start..];
        match crate::npkfs::store(&name, body, capability::CAP_NULL) {
            Ok(hash) => {
                kprint!("[npk] Stored '{}' ({} bytes, hash: ", name, body.len());
                for b in &hash[..4] { kprint!("{:02x}", b); }
                kprintln!("...)");
            }
            Err(e) => kprintln!("[npk] Store error: {}", e),
        }
    } else {
        // Print headers + body
        if let Ok(text) = core::str::from_utf8(&response) {
            for line in text.lines().take(50) {
                kprintln!("{}", line);
            }
        } else {
            kprintln!("[npk] ({} bytes, binary response)", response.len());
        }
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
