//! System intents: status, time, help, about, caps, audit, halt, set/get/config

use crate::kprintln;
use crate::capability::{self, Vault};

pub fn intent_status(vault: &Vault) {
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
    if let Some(cap) = crate::blkdev::capacity() {
        let mb = (cap * 512) / (1024 * 1024);
        let dev = if crate::nvme::is_available() { "NVMe" } else { "virtio-blk" };
        kprintln!("  Block device:  {} MB ({} sectors, {})", mb, cap, dev);
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

pub fn intent_caps(vault: &Vault) {
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

pub fn intent_audit() {
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

pub fn intent_time() {
    if crate::net::ntp::unix_time().is_none() {
        kprintln!("[npk] Syncing time...");
        crate::net::ntp::sync_via_dns("pool.ntp.org");
    }
    match crate::net::ntp::unix_time() {
        Some(t) => kprintln!("{}", crate::net::ntp::format_time(t)),
        None => kprintln!("[npk] Time unavailable. No RTC or network."),
    }
}

pub fn intent_help_topic(topic: &str) {
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
            kprintln!("  shell                 Start encrypted remote shell (port 4444)");
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
            kprintln!("  Security:  lock · passwd · caps · audit · shell");
            kprintln!("  Config:    set · get · config");
            kprintln!("  Disk:      disk read · disk write");
            kprintln!();
            kprintln!("  help <topic>  for details (storage, content, network, exec, security, config, disk)");
            kprintln!();
        }
    }
}

pub fn intent_about() {
    kprintln!();
    kprintln!("  nopeekOS – AI-native Operating System");
    kprintln!("  ──────────────────────────────────────");
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

pub fn intent_philosophy() {
    kprintln!();
    kprintln!("  What remains when you remove fifty years of assumptions?");
    kprintln!();
    kprintln!("  A capability vault, a WASM sandbox,");
    kprintln!("  an intent loop, and a human view.");
    kprintln!("  Everything else is generated.");
    kprintln!();
}

pub fn intent_echo(args: &str) { kprintln!("{}", args); }

pub fn intent_think(args: &str) {
    kprintln!();
    kprintln!("  [Intent: think]");
    kprintln!("  Question: {}", args);
    kprintln!();
    kprintln!("  AI reasoning not yet available.");
    kprintln!("  This will route to the neurofabric layer (Phase 7+).");
    kprintln!();
}

pub fn intent_set(args: &str) {
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

pub fn intent_get(args: &str) {
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

pub fn intent_config() {
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

pub fn intent_halt() -> ! {
    kprintln!();
    kprintln!("[npk] Shutting down...");
    kprintln!("[npk] Goodbye.");
    kprintln!();
    unsafe {
        core::arch::asm!("out dx, al", in("dx") 0xf4u16, in("al") 0u8);
        loop { core::arch::asm!("cli; hlt"); }
    }
}
