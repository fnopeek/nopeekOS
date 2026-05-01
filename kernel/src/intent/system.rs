//! System intents: status, time, help, about, caps, audit, halt, set/get/config

use crate::kprintln;
use crate::capability::{self, Vault};

pub fn intent_status(vault: &Vault) {
    let (active_caps, max_caps) = vault.stats();
    let (free_frames, free_mb) = crate::memory::stats();
    let uptime = crate::interrupts::uptime_secs();
    let audit_count = crate::audit::total_count();

    kprintln!();
    kprintln!("  nopeekOS v{} – AI-native Operating System", env!("CARGO_PKG_VERSION"));
    kprintln!("  ──────────────────────────────────────────");
    kprintln!("  Uptime:        {}m {}s", uptime / 60, uptime % 60);
    kprintln!("  Phase:         2 (Capability Enforcement)");
    let cores = crate::smp::per_core::core_count();
    let wakeup = if crate::smp::per_core::has_mwait() { "MWAIT" } else { "HLT" };
    kprintln!("  CPU:           x86_64, {} cores (work-stealing, {})", cores, wakeup);
    let (heap_used, heap_total) = crate::heap::stats();
    let (huge_pages, small_pages) = crate::paging::stats();
    kprintln!("  Memory:        {} MB free ({} frames)", free_mb, free_frames);
    kprintln!("  Heap:          {} KB / {} MB", heap_used / 1024, heap_total / (1024 * 1024));
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
    if let Some(mac) = crate::netdev::mac() {
        let ip = crate::net::arp::our_ip();
        kprintln!("  Network:       {}.{}.{}.{} ({:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x})",
            ip[0], ip[1], ip[2], ip[3], mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]);
    } else {
        kprintln!("  Network:       none");
    }
    if let Some((_, free, objects, generation)) = crate::npkfs::stats() {
        kprintln!("  npkFS:         {} objects, {} free blocks (gen {})", objects, free, generation);
    } else {
        kprintln!("  npkFS:         not mounted");
    }
    kprintln!();
}

pub fn intent_history() {
    super::print_active_history();
}

pub fn intent_uptime() {
    let secs = crate::interrupts::uptime_secs();
    let days = secs / 86400;
    let hours = (secs % 86400) / 3600;
    let mins = (secs % 3600) / 60;
    let s = secs % 60;
    if days > 0 {
        kprintln!("up {}d {}h {}m {}s", days, hours, mins, s);
    } else if hours > 0 {
        kprintln!("up {}h {}m {}s", hours, mins, s);
    } else {
        kprintln!("up {}m {}s", mins, s);
    }
}

pub fn intent_gpu(args: &str) {
    match args.trim() {
        "dump" | "regs" => {
            crate::gpu::dump_native();
        }
        "test-pll" | "test" => {
            // Test PLL re-lock with firmware values (will kill display!)
            kprintln!("[npk] WARNING: This will disable the display!");
            kprintln!("[npk] Log will be saved after test.");

            let _pre_log = crate::serial::stop_capture();
            crate::serial::start_capture();

            crate::gpu::test_pll();

            let log = crate::serial::stop_capture();
            crate::serial::start_capture();
            let log_name = crate::gpu::next_log_name();
            let _ = crate::npkfs::store(&log_name, log.as_bytes(), [0u8; 32]);
            kprintln!("[npk] Log saved: {}", log_name);
        }
        "init" | "activate" => {
            if crate::gpu::is_native() {
                kprintln!("[npk] GPU: native driver already active ({})", crate::gpu::driver_name());
                return;
            }
            if !crate::gpu::native_detected() {
                kprintln!("[npk] GPU: no native GPU detected");
                return;
            }

            // Capture serial output during init (survives black screen)
            // Stop normal capture, start fresh for GPU init
            let _pre_log = crate::serial::stop_capture();
            crate::serial::start_capture();

            kprintln!("[npk] GPU: activating native driver...");
            let result = crate::gpu::activate_native();

            // Save GPU init log to npkFS (readable after reboot)
            let gpu_log = crate::serial::stop_capture();
            // Restore pre-existing capture
            crate::serial::start_capture();

            // Store log in npkFS (unencrypted, no cap needed — use zero cap)
            let log_data = alloc::format!("{}\n--- GPU INIT RESULT: {:?} ---\n", gpu_log,
                result.as_ref().map(|fb| alloc::format!("OK {}x{}", fb.width, fb.height))
                    .unwrap_or_else(|e| alloc::format!("{:?}", e)));
            let log_name = crate::gpu::next_log_name();
            let _ = crate::npkfs::store(&log_name, log_data.as_bytes(), [0u8; 32]);
            kprintln!("[npk] Log saved: {}", log_name);

            match result {
                Ok(fb) => {
                    crate::framebuffer::init_from_gpu();
                    kprintln!("[npk] GPU: {}x{} active", fb.width, fb.height);
                }
                Err(e) => {
                    kprintln!("[npk] GPU: activation failed: {:?}", e);
                    kprintln!("[npk] GOP framebuffer unchanged");
                    kprintln!("[npk] Check log with 'list gpu'");
                }
            }
        }
        "4k" | "4k30" | "4k60" => {
            if !crate::gpu::is_native() {
                kprintln!("[npk] GPU: native driver not active (run 'gpu init' first)");
                return;
            }

            let hz: u8 = if args.trim() == "4k60" { 60 } else { 30 };

            let _pre_log = crate::serial::stop_capture();
            crate::serial::start_capture();

            kprintln!("[npk] GPU: switching to 4K@{}Hz...", hz);
            let result = crate::gpu::set_mode(3840, 2160, hz);

            let gpu_log = crate::serial::stop_capture();
            crate::serial::start_capture();

            let log_data = alloc::format!("{}\n--- GPU MODE RESULT: {:?} ---\n", gpu_log,
                result.as_ref().map(|fb| alloc::format!("OK {}x{}", fb.width, fb.height))
                    .unwrap_or_else(|e| alloc::format!("{:?}", e)));
            let log_name = crate::gpu::next_log_name();
            let _ = crate::npkfs::store(&log_name, log_data.as_bytes(), [0u8; 32]);
            kprintln!("[npk] Log saved: {}", log_name);

            // Always reinit console — display hardware is already at new mode
            // even if pipe re-enable timed out
            crate::framebuffer::init_from_gpu();
            match result {
                Ok(fb) => {
                    kprintln!("[npk] GPU: {}x{} active", fb.width, fb.height);
                }
                Err(e) => {
                    kprintln!("[npk] GPU: mode switch partial: {:?} (display may work)", e);
                    kprintln!("[npk] Check log with 'list gpu'");
                }
            }
        }
        "blit init" => {
            if !crate::gpu::is_native() {
                kprintln!("[npk] GPU: native driver not active (run 'gpu init' first)");
                return;
            }
            if crate::gpu::supports_blit() {
                kprintln!("[npk] BCS: already initialized");
                return;
            }
            kprintln!("[npk] Initializing BCS blitter engine...");
            if crate::gpu::init_blit_engine() {
                kprintln!("[npk] BCS: blitter engine ready");
                // Map shadow buffers into GGTT for GPU blit
                if let Some((phys_a, phys_b, pages)) = crate::framebuffer::shadow_phys_info() {
                    if pages > 0 {
                        crate::gpu::map_shadows_for_blit(phys_a, phys_b, pages);
                        let (ga, gb) = crate::gpu::shadow_ggtt();
                        crate::framebuffer::set_shadow_ggtt(ga, gb);
                        kprintln!("[npk] BCS: shadow A GGTT={:#x}, shadow B GGTT={:#x}", ga, gb);
                    }
                }
            } else {
                kprintln!("[npk] BCS: init failed");
            }
        }
        "blit test" => {
            if !crate::gpu::supports_blit() {
                kprintln!("[npk] BCS: not initialized (run 'gpu blit init')");
                return;
            }
            crate::gpu::test_blit();
        }
        "blit status" | "blit" => {
            kprintln!("  BCS:      {}", if crate::gpu::supports_blit() { "ready" } else { "not initialized" });
            let (ga, gb) = crate::gpu::shadow_ggtt();
            if ga != 0 {
                kprintln!("  Shadow A: GGTT {:#x}", ga);
                kprintln!("  Shadow B: GGTT {:#x}", gb);
            }
            let fb_ggtt = crate::gpu::fb_ggtt_offset();
            if fb_ggtt != 0 {
                kprintln!("  FB GGTT:  {:#x}", fb_ggtt);
            }
        }
        "status" | "" => {
            kprintln!("  Driver:   {}", crate::gpu::driver_name());
            kprintln!("  Native:   {}", if crate::gpu::is_native() { "yes" } else { "no (GOP)" });
            if let Some(fb) = crate::gpu::framebuffer_info() {
                kprintln!("  Mode:     {}x{} {}bpp", fb.width, fb.height, fb.bpp);
                kprintln!("  FB addr:  {:#x}", fb.addr);
                kprintln!("  Pitch:    {} bytes ({} KB/line)", fb.pitch, fb.pitch / 1024);
                let fb_mb = (fb.pitch as u64 * fb.height as u64) / (1024 * 1024);
                kprintln!("  FB size:  {} MB", fb_mb);
            }
            let hz = crate::gpu::current_hz();
            if hz > 0 {
                kprintln!("  Refresh:  {}Hz", hz);
            }
            kprintln!("  VSync:    {}", if crate::gpu::supports_flip() { "planned (PLANE_SURF double-buffer)" } else { "no (GOP)" });
            kprintln!("  Flip:     {}", if crate::gpu::supports_flip() { "hardware (PLANE_SURF)" } else { "CPU blit" });

            // BCS blitter status
            let bcs_ok = crate::gpu::supports_blit();
            kprintln!("  BCS:      {}", if bcs_ok { "active" } else { "off (probe failed)" });
            if bcs_ok {
                let fb_ggtt = crate::gpu::fb_ggtt_offset();
                let (ga, gb) = crate::gpu::shadow_ggtt();
                let front = crate::framebuffer::front_ggtt();
                kprintln!("  FB GGTT:  {:#x}", fb_ggtt);
                kprintln!("  Shadow A: GGTT {:#x}", ga);
                kprintln!("  Shadow B: GGTT {:#x}", gb);
                kprintln!("  Front:    GGTT {:#x} ({})",
                    front, if front == ga { "A" } else if front == gb { "B" } else { "?" });
                kprintln!("  Blit:     GPU (XY_FAST_COPY_BLT)");
                let mouse = crate::xhci::mouse_available();
                kprintln!("  Cursor:   {}", if mouse { "GPU-composited (save-under)" } else { "none" });
            } else {
                kprintln!("  Blit:     CPU (memcpy)");
                let mouse = crate::xhci::mouse_available();
                kprintln!("  Cursor:   {}", if mouse { "MMIO overlay" } else { "none" });
            }

            // BCS register dump (always, for debug)
            if crate::gpu::is_native() {
                kprintln!("  --- BCS regs ---");
                crate::gpu::dump_bcs_regs();
            }

            // Shadow buffer info
            if let Some((pa, pb, pages)) = crate::framebuffer::shadow_phys_info() {
                kprintln!("  Shadow:   {} pages ({} MB) x2", pages, pages * 4 / 1024);
                kprintln!("  Phys A:   {:#x}", pa);
                kprintln!("  Phys B:   {:#x}", pb);
            }

            if let Some(name) = crate::gpu::native_gpu_name() {
                if !crate::gpu::is_native() {
                    kprintln!("  Pending:  {} (use 'gpu init')", name);
                }
            }
            let modes = crate::gpu::supported_modes();
            if !modes.is_empty() {
                kprintln!("  Modes:");
                for m in &modes {
                    kprintln!("    {}x{} @ {}Hz", m.width, m.height, m.hz);
                }
            }
        }
        _ => {
            kprintln!("Usage: gpu [status|init|4k|blit init|blit test|blit status]");
        }
    }
}

pub fn intent_shade(args: &str) {
    match args.trim() {
        "init" | "start" => {
            if crate::shade::is_active() {
                kprintln!("[npk] shade: already running");
                return;
            }
            // Destroy pre-shade sessions so terminals start clean
            for i in 0..8u8 { crate::intent::destroy_session(i); }
            crate::shade::init();
            crate::shade::render_frame();
            kprintln!("[npk] shade: compositor active (Mod+Enter for first window)");
        }
        "demo" => {
            if !crate::shade::is_active() {
                crate::shade::init();
            }
            crate::shade::with_compositor(|comp| {
                comp.create_window("loop", 0, 0, 800, 600);
                if let Some(id2) = comp.create_window("editor", 0, 0, 800, 600) {
                    if let Some(win) = comp.window_mut(id2) {
                        win.bg_color = 0x00180820;
                    }
                }
                if let Some(id3) = comp.create_window("status", 0, 0, 800, 300) {
                    if let Some(win) = comp.window_mut(id3) {
                        win.bg_color = 0x00081820;
                    }
                }
            });
            crate::shade::render_frame();
            kprintln!("[npk] shade: demo mode (3 windows, master-stack layout)");
        }
        "stop" | "exit" => {
            if !crate::shade::is_active() {
                kprintln!("[npk] shade: not running");
                return;
            }
            crate::shade::stop();
            kprintln!("[npk] shade: stopped");
        }
        "ws" | "workspace" => {
            kprintln!("[npk] Usage: shade ws <1-4>");
        }
        sub if sub.starts_with("ws ") || sub.starts_with("workspace ") => {
            let num_str = sub.split_whitespace().nth(1).unwrap_or("");
            if let Ok(ws) = num_str.parse::<u8>() {
                if ws >= 1 && ws <= 4 {
                    crate::shade::with_compositor(|comp| {
                        comp.switch_workspace(ws - 1);
                    });
                    crate::shade::render_frame();
                    kprintln!("[npk] shade: workspace {}", ws);
                } else {
                    kprintln!("[npk] shade: workspace 1-4");
                }
            }
        }
        "config" => {
            kprintln!();
            kprintln!("  Shade Compositor");
            kprintln!("  ────────────────");
            for (key, default, desc) in crate::shade::default_config() {
                let current = crate::config::get(key);
                let val = current.as_deref().unwrap_or(default);
                kprintln!("  {:24} = {:8}  {}", key, val, desc);
            }
            kprintln!();
            kprintln!("  Use 'set <key> <value>' to change.");
            kprintln!();
        }
        "status" | "" => {
            if crate::shade::is_active() {
                crate::shade::with_compositor(|comp| {
                    kprintln!("  shade: active");
                    kprintln!("  screen: {}x{} scale:{}x", comp.screen_w, comp.screen_h, comp.scale);
                    kprintln!("  windows: {}", comp.window_count());
                    kprintln!("  workspace: {}/4", comp.active_workspace + 1);
                    kprintln!("  gaps: {}px  border: {}px", comp.gaps, comp.border);
                    kprintln!("  bar: {:?} ({}px)", comp.bar.position, comp.bar.height);
                });
            } else {
                kprintln!("[npk] shade: not running (use 'shade init' to start)");
            }
        }
        _ => {
            kprintln!("Usage: shade [init|demo|stop|status|config|ws <1-4>]");
        }
    }
}

pub fn intent_dmesg() {
    // Stop capture, print, restart — so dmesg output itself isn't appended
    let log = crate::serial::stop_capture();
    if log.is_empty() {
        kprintln!("(no boot log captured)");
    } else {
        // Print without going through capture (direct serial + framebuffer)
        kprintln!("{}", log);
    }
    crate::serial::start_capture();
}

pub fn intent_uname(args: &str) {
    let all = args.contains("-a") || args.is_empty();
    if all {
        kprintln!("nopeekOS {} x86_64 release (rustc {})",
            env!("CARGO_PKG_VERSION"),
            rustc_version());
    } else {
        if args.contains("-s") { kprintln!("nopeekOS"); }
        if args.contains("-r") || args.contains("-v") {
            kprintln!("{}", env!("CARGO_PKG_VERSION"));
        }
        if args.contains("-m") { kprintln!("x86_64"); }
    }
}

fn rustc_version() -> &'static str {
    // Embedded at compile time via env
    option_env!("RUSTC_VERSION").unwrap_or("nightly")
}

pub fn intent_caps(vault: &Vault) {
    let (active, max) = vault.stats();
    kprintln!();
    kprintln!("  Capability Vault");
    kprintln!("  ────────────────");
    kprintln!("  Active tokens:  {}", active);
    kprintln!("  Max capacity:   {}", max);
    kprintln!("  Token IDs:      256-bit random (CSPRNG)");
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
        "shade" | "compositor" | "wm" | "display" => {
            kprintln!();
            kprintln!("  Shade Compositor");
            kprintln!("  ────────────────");
            kprintln!("  shade init             Start compositor");
            kprintln!("  shade demo             Demo with 3 tiled windows");
            kprintln!("  shade stop             Stop compositor, return to text");
            kprintln!("  shade status           Current compositor state");
            kprintln!("  shade config           Show/change compositor settings");
            kprintln!("  shade ws <1-4>         Switch workspace");
            kprintln!();
            kprintln!("  Config keys (set via 'set <key> <value>'):");
            kprintln!("    shade.gaps            Gap between windows (px, default: 8)");
            kprintln!("    shade.border          Border width (px, default: 2)");
            kprintln!("    shade.border_active   Active border color (hex)");
            kprintln!("    shade.border_inactive Inactive border color (hex)");
            kprintln!("    shade.bar_height      Status bar height (px, default: 28)");
            kprintln!("    shade.bar_position    Bar position (top/bottom)");
            kprintln!();
        }
        "wallpaper" | "wp" | "background" => {
            kprintln!();
            kprintln!("  Wallpaper");
            kprintln!("  ─────────");
            kprintln!("  wallpaper set <name>   Set wallpaper from npkFS");
            kprintln!("  wallpaper clear        Revert to aurora background");
            kprintln!("  wallpaper list         List available wallpapers");
            kprintln!("  wallpaper random       Set random wallpaper");
            kprintln!();
            kprintln!("  Wallpapers live in ~/wallpapers/");
            kprintln!("  Download: https <host> /image.png > wallpapers/name");
            kprintln!("  A random wallpaper is set on each login.");
            kprintln!("  Theme colors are extracted automatically.");
            kprintln!();
        }
        "packages" | "install" | "modules" => {
            kprintln!();
            kprintln!("  Package Manager");
            kprintln!("  ───────────────");
            kprintln!("  install <module>       Download + verify + install WASM module");
            kprintln!("  uninstall <module> [--force]  Remove module (--force for bundled)");
            kprintln!("  modules                List installed modules");
            kprintln!();
            kprintln!("  Modules are signed (ECDSA P-384) and verified.");
            kprintln!("  Source: raw.githubusercontent.com/fnopeek/nopeekOS/");
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
        "vmx" | "vt-x" | "microvm" => {
            kprintln!();
            kprintln!("  Virtualization (Phase 12 — MicroVM)");
            kprintln!("  ───────────────────────────────────");
            kprintln!("  vmx                    Probe Intel VT-x capability + report");
            kprintln!();
            kprintln!("  Phase 12 builds a per-app VT-x MicroVM for legacy Linux GUI");
            kprintln!("  apps (Browser first). Status:");
            kprintln!("    12.1.0a   probe + report                          ✓");
            kprintln!("    12.1.0b   VMXON region + CR4.VMXE round-trip      ✓");
            kprintln!("    12.1.0c   VMCS region + VMCLEAR + VMPTRLD         ✓");
            kprintln!("    12.1.0d-1 host-state VMWRITE/VMREAD + trampoline  ✓");
            kprintln!("    12.1.0d-2 guest-state + controls + VMLAUNCH       — next");
            kprintln!();
            kprintln!("  Reported fields:");
            kprintln!("    revision_id      VMCS revision (per CPU stepping)");
            kprintln!("    vmxon_region_sz  VMXON / VMCS allocation size in bytes");
            kprintln!("    ept_supported    Extended Page Tables for guest-phys → host-phys");
            kprintln!("    unrestricted     Real-mode guest without trampolining");
            kprintln!("    vpid             Tagged TLB across VM-entry/exit");
            kprintln!("    bring-up         Last result of VMXON+VMXOFF round-trip");
            kprintln!();
            kprintln!("  If 'NOT available': enable 'Intel Virtualization Technology'");
            kprintln!("  in BIOS/UEFI firmware setup.");
            kprintln!();
        }
        _ => {
            // Main overview
            kprintln!();
            kprintln!("  nopeekOS");
            kprintln!("  ════════");
            kprintln!();
            kprintln!("  System:    status · uptime · time · dmesg · about · clear · halt");
            kprintln!("  Storage:   store · fetch · delete · list · fsinfo");
            kprintln!("  Content:   cat · grep · head · wc · hexdump");
            kprintln!("  Network:   ping · resolve · http · https · traceroute · netstat");
            kprintln!("  Exec:      run · add · multiply");
            kprintln!("  Packages:  install · uninstall · modules");
            kprintln!("  Security:  lock · passwd · caps · audit · shell");
            kprintln!("  Config:    set · get · config");
            kprintln!("  Display:   gpu · shade · wallpaper");
            kprintln!("  Disk:      disk read · disk write");
            kprintln!("  Virt:      vmx");
            kprintln!();
            kprintln!("  help <topic>  for details (storage, content, network, exec, security, config, disk, shade, wallpaper, vmx)");
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

pub fn intent_reboot() -> ! {
    kprintln!();
    kprintln!("[npk] Rebooting...");
    kprintln!();
    unsafe {
        // Disable interrupts first
        core::arch::asm!("cli");

        // Method 1: ACPI reset register (if available from FADT)
        crate::acpi::reset();

        // Method 2: PCI CF9 reset (Intel chipsets)
        // Must write 0x02 first (enable reset), then 0x06 (trigger)
        core::arch::asm!("out dx, al", in("dx") 0xCF9u16, in("al") 0x02u8);
        for _ in 0..100_000u32 { core::hint::spin_loop(); }
        core::arch::asm!("out dx, al", in("dx") 0xCF9u16, in("al") 0x06u8);
        for _ in 0..1_000_000u32 { core::hint::spin_loop(); }

        // Method 3: Keyboard controller reset (port 0x64)
        core::arch::asm!("out dx, al", in("dx") 0x64u16, in("al") 0xFEu8);
        for _ in 0..1_000_000u32 { core::hint::spin_loop(); }

        // Method 4: Triple-fault (guaranteed reboot on any x86)
        let null_idt: [u8; 6] = [0; 6];
        core::arch::asm!("lidt [{}]", in(reg) &null_idt);
        core::arch::asm!("int3");

        loop { core::arch::asm!("hlt"); }
    }
}

pub fn intent_lspci(args: &str) {
    use crate::drivers::pci::{self, PciAddr};

    let verbose = args.contains("-v");
    let mut count = 0u16;

    kprintln!();
    for bus in 0u16..=255 {
        for dev in 0u8..32 {
            for func in 0u8..8 {
                let addr = PciAddr { bus: bus as u8, device: dev, function: func };
                let id = pci::read32(addr, 0x00);
                if id == 0xFFFF_FFFF || id == 0 {
                    if func == 0 { break; }
                    continue;
                }

                let vid = (id & 0xFFFF) as u16;
                let did = ((id >> 16) & 0xFFFF) as u16;
                let class_reg = pci::read32(addr, 0x08);
                let cls = ((class_reg >> 24) & 0xFF) as u8;
                let sub = ((class_reg >> 16) & 0xFF) as u8;
                let prog_if = ((class_reg >> 8) & 0xFF) as u8;
                let rev = (class_reg & 0xFF) as u8;

                let class_name = pci_class_name(cls, sub, prog_if);
                let dev_name = pci_device_name(vid, did);

                kprintln!("  {:02x}:{:02x}.{}  {:04x}:{:04x}  {}",
                    bus, dev, func, vid, did, class_name);
                if !dev_name.is_empty() {
                    kprintln!("           {}", dev_name);
                }

                if verbose {
                    let bar0 = pci::read32(addr, 0x10);
                    let irq = pci::read8(addr, 0x3C);
                    let cmd = pci::read16(addr, 0x04);
                    kprintln!("           rev {:02x}  prog-if {:02x}  IRQ {}  BAR0 {:08x}",
                        rev, prog_if, irq, bar0);
                    kprintln!("           cmd: {}{}{}",
                        if cmd & 0x04 != 0 { "bus-master " } else { "" },
                        if cmd & 0x02 != 0 { "mem " } else { "" },
                        if cmd & 0x01 != 0 { "io" } else { "" });
                }

                count += 1;

                if func == 0 && pci::read8(addr, 0x0E) & 0x80 == 0 {
                    break;
                }
            }
        }
    }
    kprintln!();
    kprintln!("  {} PCI devices found", count);
    kprintln!();
}

fn pci_class_name(cls: u8, sub: u8, prog_if: u8) -> &'static str {
    match (cls, sub, prog_if) {
        (0x00, 0x00, _) => "Legacy device",
        (0x00, 0x01, _) => "VGA-compatible",
        (0x01, 0x00, _) => "SCSI controller",
        (0x01, 0x01, _) => "IDE controller",
        (0x01, 0x06, _) => "SATA controller",
        (0x01, 0x08, 0x02) => "NVMe controller",
        (0x01, 0x08, _) => "NVM controller",
        (0x02, 0x00, _) => "Ethernet controller",
        (0x02, 0x80, _) => "Network controller",
        (0x03, 0x00, _) => "VGA controller",
        (0x03, 0x80, _) => "Display controller",
        (0x04, 0x00, _) => "Video controller",
        (0x04, 0x01, _) => "Audio controller",
        (0x04, 0x03, _) => "HD Audio controller",
        (0x06, 0x00, _) => "Host bridge",
        (0x06, 0x01, _) => "ISA bridge",
        (0x06, 0x04, _) => "PCI-to-PCI bridge",
        (0x06, 0x80, _) => "System bridge",
        (0x07, 0x00, _) => "Serial controller",
        (0x07, 0x80, _) => "Communication controller",
        (0x08, 0x00, _) => "PIC",
        (0x08, 0x01, _) => "DMA controller",
        (0x08, 0x02, _) => "Timer",
        (0x08, 0x03, _) => "RTC controller",
        (0x08, 0x80, _) => "System peripheral",
        (0x0C, 0x03, 0x00) => "UHCI USB controller",
        (0x0C, 0x03, 0x10) => "OHCI USB controller",
        (0x0C, 0x03, 0x20) => "EHCI USB controller",
        (0x0C, 0x03, 0x30) => "xHCI USB controller",
        (0x0C, 0x03, _) => "USB controller",
        (0x0C, 0x05, _) => "SMBus controller",
        (0x0D, 0x00, _) => "IrDA controller",
        (0x0D, 0x80, _) => "Wireless controller",
        (0x0E, 0x00, _) => "I2O controller",
        (0x0F, _, _) => "Satellite controller",
        (0x10, _, _) => "Crypto controller",
        (0x11, _, _) => "Signal processing",
        (0xFF, _, _) => "Unassigned",
        _ => "Unknown",
    }
}

fn pci_device_name(vendor: u16, device: u16) -> &'static str {
    match (vendor, device) {
        // Intel WiFi
        (0x8086, 0x2723) => "Intel Wi-Fi 6 AX200",
        (0x8086, 0x2725) => "Intel Wi-Fi 6E AX210",
        (0x8086, 0x4DF0) => "Intel Wi-Fi 6 AX201",
        (0x8086, 0xA0F0) => "Intel Wi-Fi 6 AX201",
        (0x8086, 0x06F0) => "Intel Wi-Fi 6 AX201",
        (0x8086, 0x34F0) => "Intel Wi-Fi 6 AX201",
        (0x8086, 0x51F0) => "Intel Wi-Fi 6E AX211",
        (0x8086, 0x51F1) => "Intel Wi-Fi 6E AX211",
        (0x8086, 0x54F0) => "Intel Wi-Fi 6E AX211",
        (0x8086, 0x7AF0) => "Intel Wi-Fi 6E AX211",
        (0x8086, 0x7E40) => "Intel Wi-Fi 7 BE200",
        (0x8086, 0xE440) => "Intel Wi-Fi 7 BE200",
        (0x8086, 0x272B) => "Intel Wi-Fi 7 BE202",
        // Intel Ethernet
        (0x8086, 0x15F3) => "Intel I225-V (2.5GbE)",
        (0x8086, 0x15F2) => "Intel I225-LM (2.5GbE)",
        (0x8086, 0x125C) => "Intel I226-V (2.5GbE)",
        (0x8086, 0x125B) => "Intel I226-LM (2.5GbE)",
        (0x8086, 0x15E3) => "Intel I219-LM",
        (0x8086, 0x0D4F) => "Intel I219-V",
        (0x8086, 0x15BE) => "Intel I219-LM",
        (0x8086, 0x15BD) => "Intel I219-V",
        // Intel GPU
        (0x8086, 0x46A6) => "Intel Alder Lake-N [UHD Graphics]",
        (0x8086, 0x46D0) => "Intel Alder Lake-N [UHD Graphics]",
        (0x8086, 0x46D1) => "Intel Alder Lake-N [UHD Graphics]",
        (0x8086, 0x46D2) => "Intel Alder Lake-N [UHD Graphics]",
        (0x8086, 0xA7A0) => "Intel Raptor Lake [UHD Graphics]",
        (0x8086, 0xA720) => "Intel Raptor Lake [UHD Graphics]",
        (0x8086, 0xA780) => "Intel Raptor Lake [UHD Graphics]",
        (0x8086, 0x4628) => "Intel Alder Lake [Iris Xe]",
        (0x8086, 0x4626) => "Intel Alder Lake [Iris Xe]",
        (0x8086, 0x46A8) => "Intel Alder Lake [Iris Xe]",
        // Intel NVMe
        (0x8086, 0xF1A8) => "Intel SSD 660p/670p",
        (0x8086, 0xF1AA) => "Intel SSD 670p",
        // Intel Host Bridge / ISA / misc
        (0x8086, 0x4617) => "Intel Alder Lake Host Bridge",
        (0x8086, 0x461C) => "Intel Alder Lake-N Host Bridge",
        (0x8086, 0x4601) => "Intel Alder Lake Host Bridge",
        (0x8086, 0x461D) => "Intel Alder Lake-N TurboBoost",
        (0x8086, 0x467E) => "Intel Alder Lake-N GNA",
        (0x8086, 0x467D) => "Intel Alder Lake-N IPU",
        (0x8086, 0x4649) => "Intel Alder Lake PCIe RP",
        (0x8086, 0x464D) => "Intel Alder Lake PCIe RP",
        (0x8086, 0x4641) => "Intel Alder Lake PCH",
        (0x8086, 0x5481) => "Intel Alder Lake-N ISA Bridge",
        (0x8086, 0x51A3) => "Intel Alder Lake-P ISA Bridge",
        (0x8086, 0x54A3) => "Intel Alder Lake-N SMBus",
        (0x8086, 0x51EF) => "Intel Alder Lake-P SMBus",
        (0x8086, 0x54A4) => "Intel Alder Lake-N SPI Controller",
        (0x8086, 0x54C4) => "Intel Alder Lake-N eSPI/SPI",
        (0x8086, 0x54EF) => "Intel Alder Lake-N Shared SRAM",
        (0x8086, 0x54E8) => "Intel Alder Lake-N Serial IO I2C #0",
        (0x8086, 0x54EA) => "Intel Alder Lake-N Serial IO I2C #2",
        (0x8086, 0x54EB) => "Intel Alder Lake-N Serial IO I2C #3",
        (0x8086, 0x54E0) => "Intel Alder Lake-N HECI/MEI",
        (0x8086, 0x54D3) => "Intel Alder Lake-N SATA AHCI",
        (0x8086, 0x51E8) => "Intel Alder Lake-P Serial IO I2C",
        // Intel HD Audio
        (0x8086, 0x54C8) => "Intel Alder Lake-N HD Audio",
        (0x8086, 0x51C8) => "Intel Alder Lake-P HD Audio",
        (0x8086, 0x51CA) => "Intel Alder Lake-P HD Audio",
        (0x8086, 0x4DC8) => "Intel Alder Lake-N HD Audio",
        // Intel PCI-to-PCI bridges (Alder Lake-N)
        (0x8086, 0x54BE) => "Intel Alder Lake-N PCIe RP #7",
        (0x8086, 0x54B0) => "Intel Alder Lake-N PCIe RP #9",
        (0x8086, 0x54B2) => "Intel Alder Lake-N PCIe RP #11",
        // Intel Thunderbolt / USB
        (0x8086, 0x461E) => "Intel Alder Lake Thunderbolt 4",
        (0x8086, 0x464E) => "Intel Alder Lake-N xHCI",
        (0x8086, 0x54ED) => "Intel Alder Lake-N PCH xHCI",
        (0x8086, 0x51ED) => "Intel Alder Lake-P xHCI",
        (0x8086, 0x4DED) => "Intel Alder Lake-N xHCI",
        // Samsung NVMe
        (0x144D, 0xA808) => "Samsung 970 EVO Plus",
        (0x144D, 0xA809) => "Samsung 980 PRO",
        (0x144D, 0xA80A) => "Samsung 990 PRO",
        // Virtio (QEMU)
        (0x1AF4, 0x1000) => "VirtIO Network (legacy)",
        (0x1AF4, 0x1041) => "VirtIO Network",
        (0x1AF4, 0x1001) => "VirtIO Block (legacy)",
        (0x1AF4, 0x1042) => "VirtIO Block",
        (0x1AF4, 0x1050) => "VirtIO GPU",
        // Realtek
        (0x10EC, 0x8168) => "Realtek RTL8111/8168",
        (0x10EC, 0x8125) => "Realtek RTL8125 (2.5GbE)",
        (0x10EC, 0xB852) => "Realtek RTL8852BE (Wi-Fi 6)",
        (0x10EC, 0xB832) => "Realtek RTL8832BE (Wi-Fi 6E)",
        (0x10EC, 0xC852) => "Realtek RTL8852CE (Wi-Fi 6E)",
        // MAXIO NVMe
        (0x1E4B, 0x1202) => "MAXIO MAP1202 NVMe SSD",
        (0x1E4B, 0x1602) => "MAXIO MAP1602 NVMe SSD",
        // QEMU/VBox
        (0x8086, 0x100E) => "Intel 82540EM (QEMU e1000)",
        (0x8086, 0x29C0) => "Intel 82G33 Host Bridge (QEMU)",
        (0x8086, 0x2918) => "Intel ICH9 LPC (QEMU)",
        (0x8086, 0x2922) => "Intel ICH9 AHCI (QEMU)",
        (0x8086, 0x2930) => "Intel ICH9 SMBus (QEMU)",
        (0x1234, 0x1111) => "QEMU/Bochs VGA",
        _ => "",
    }
}

pub fn intent_halt() -> ! {
    kprintln!();
    kprintln!("[npk] Shutting down...");
    kprintln!("[npk] Goodbye.");
    kprintln!();
    unsafe {
        // Try QEMU exit (harmless on real hardware)
        core::arch::asm!("out dx, al", in("dx") 0xf4u16, in("al") 0u8);

        // ACPI S5 power-off (port discovered from FADT at boot)
        crate::acpi::power_off();

        // Fallback: hardcoded common PM1a_CNT ports
        let slp_s5: u16 = (5 << 10) | (1 << 13);
        core::arch::asm!("out dx, ax", in("dx") 0x604u16, in("ax") slp_s5);
        core::arch::asm!("out dx, ax", in("dx") 0x1804u16, in("ax") slp_s5);

        // Last resort: triple-fault reboot
        let null_idt: [u8; 6] = [0; 6];
        core::arch::asm!("lidt [{}]", in(reg) &null_idt);
        core::arch::asm!("int3");

        loop { core::arch::asm!("cli; hlt"); }
    }
}
