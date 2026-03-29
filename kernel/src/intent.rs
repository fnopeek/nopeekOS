//! Intent Loop
//!
//! Not a shell. Takes intents, not commands.
//! Every intent requires a valid capability token.

use crate::capability::{self, Vault, Rights};
use crate::{kprint, kprintln};
use spin::Mutex;

const INPUT_BUF_SIZE: usize = 512;

pub fn run_loop(vault: &'static Mutex<Vault>, session_id: u128) -> ! {
    let mut input_buf = [0u8; INPUT_BUF_SIZE];

    loop {
        kprint!("npk> ");

        let serial = crate::serial::SERIAL.lock();
        let len = serial.read_line(&mut input_buf);
        drop(serial);

        if len == 0 { continue; }

        let input = match core::str::from_utf8(&input_buf[..len]) {
            Ok(s) => s.trim(),
            Err(_) => {
                kprintln!("[npk] invalid UTF-8 input");
                continue;
            }
        };

        dispatch_intent(input, vault, session_id);
    }
}

fn dispatch_intent(input: &str, vault: &'static Mutex<Vault>, session: u128) {
    if input.is_empty() { return; }

    let mut parts = input.splitn(2, ' ');
    let verb = parts.next().unwrap_or("");
    let args = parts.next().unwrap_or("");

    match verb {
        // Intents requiring READ
        "status" | "info" => {
            if require_cap(vault, session, Rights::READ, "status") {
                intent_status(&vault.lock());
            }
        }
        "caps" | "capabilities" => {
            if require_cap(vault, session, Rights::READ, "caps") {
                intent_caps(&vault.lock());
            }
        }
        "audit" => {
            if require_cap(vault, session, Rights::AUDIT, "audit") {
                intent_audit();
            }
        }

        // Intents requiring EXECUTE (WASM sandbox)
        "add" => {
            if require_cap(vault, session, Rights::EXECUTE, "add") {
                intent_wasm_add(args);
            }
        }
        "multiply" => {
            if require_cap(vault, session, Rights::EXECUTE, "multiply") {
                intent_wasm_multiply(args);
            }
        }
        "disk" | "blk" => {
            let sub = args.trim();
            if sub.is_empty() || sub == "info" {
                if require_cap(vault, session, Rights::READ, "disk") {
                    intent_disk_info();
                }
            } else if sub.starts_with("read ") || sub == "read" {
                if require_cap(vault, session, Rights::READ, "disk read") {
                    intent_disk_read(sub.strip_prefix("read").unwrap_or("").trim());
                }
            } else if sub.starts_with("write ") || sub == "write" {
                if require_cap(vault, session, Rights::WRITE, "disk write") {
                    intent_disk_write(sub.strip_prefix("write").unwrap_or("").trim());
                }
            } else {
                kprintln!("[npk] Usage: disk [info|read <sector>|write <sector> <text>]");
            }
        }

        "store" | "save" => {
            if require_cap(vault, session, Rights::WRITE, "store") {
                intent_store(args, session);
            }
        }
        "fetch" | "load" | "get" => {
            if require_cap(vault, session, Rights::READ, "fetch") {
                intent_fetch(args);
            }
        }
        "delete" | "rm" | "remove" => {
            if require_cap(vault, session, Rights::WRITE, "delete") {
                intent_delete(args);
            }
        }
        "list" | "ls" | "objects" => {
            if require_cap(vault, session, Rights::READ, "list") {
                intent_list();
            }
        }
        "fsinfo" | "fs" => {
            if require_cap(vault, session, Rights::READ, "fsinfo") {
                intent_fsinfo();
            }
        }

        "halt" | "shutdown" | "poweroff" => {
            if require_cap(vault, session, Rights::EXECUTE, "halt") {
                intent_halt();
            }
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
fn require_cap(vault: &Mutex<Vault>, cap_id: u128, rights: Rights, intent: &str) -> bool {
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
                        secs, ms, capability::short_id(new_id), capability::short_id(parent_id)),
                AuditOp::Revoke { revoker_id, target_id } =>
                    kprintln!("  [{:>4}.{:03}s] REVOKE {:08x} by {:08x}",
                        secs, ms, capability::short_id(target_id), capability::short_id(revoker_id)),
                AuditOp::Check { cap_id } =>
                    kprintln!("  [{:>4}.{:03}s] CHECK  {:08x} OK",
                        secs, ms, capability::short_id(cap_id)),
                AuditOp::Denied { reason } =>
                    kprintln!("  [{:>4}.{:03}s] DENIED {:?}",
                        secs, ms, reason),
                AuditOp::Expired { cap_id } =>
                    kprintln!("  [{:>4}.{:03}s] EXPIRED {:08x}",
                        secs, ms, capability::short_id(cap_id)),
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

fn intent_store(args: &str, cap_id: u128) {
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
