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
    kprintln!("  Content Store: in-memory (empty)");
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
