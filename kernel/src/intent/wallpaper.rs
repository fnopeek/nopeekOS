//! Wallpaper intent — set, clear, list, random wallpaper.
//!
//! Images are stored in npkFS under home/<user>/wallpapers/.
//! The wallpaper WASM module (sys/wasm/wallpaper) decodes PNG→BGRA.
//! Without the WASM module, raw BGRA data is used directly.

use crate::{kprintln, kprint};
use alloc::string::String;
use alloc::vec::Vec;

/// Set a wallpaper by name (relative to CWD or absolute).
pub fn intent_wallpaper(args: &str) {
    let mut parts = args.splitn(2, ' ');
    let sub = parts.next().unwrap_or("").trim();
    let rest = parts.next().unwrap_or("").trim();

    match sub {
        "set" => set_wallpaper(rest),
        "clear" | "off" | "none" => clear_wallpaper(),
        "list" | "ls" => list_wallpapers(),
        "random" | "rand" => random_wallpaper(),
        "demo" => generate_demo_wallpapers(rest),
        "solid" | "hex" | "color" => generate_solid_wallpaper(rest),
        "gradient" | "grad" => generate_gradient_wallpaper(rest),
        "pattern" | "pat" => generate_pattern_wallpaper(rest),
        "" => {
            kprintln!("[npk] Usage: wallpaper <set|clear|list|random|demo|solid|gradient|pattern>");
            kprintln!("[npk]   set <name>                          Set wallpaper from npkFS");
            kprintln!("[npk]   clear                               Default background");
            kprintln!("[npk]   list                                List wallpapers");
            kprintln!("[npk]   random                              Pick a random one");
            kprintln!("[npk]   demo [WxH]                          4 gradient presets");
            kprintln!("[npk]   solid <hex>                         flat colour (e.g. 2a1840)");
            kprintln!("[npk]   gradient <top> <bot>                two-colour vertical");
            kprintln!("[npk]   gradient <tl> <tr> <bl> <br>        four-corner bilinear");
            kprintln!("[npk]   pattern <kind> <fg> <bg>            kind = dots|stripes|checker|grid|noise");
        }
        other => set_wallpaper(other),
    }
}

/// Parse an optional `WxH` resolution argument. Returns native
/// framebuffer resolution when the string is empty or malformed.
fn parse_resolution(arg: &str) -> (u32, u32) {
    let (fb_w, fb_h) = crate::framebuffer::get_resolution();
    if arg.is_empty() {
        return (fb_w, fb_h);
    }
    let (w_s, h_s) = match arg.split_once('x') {
        Some(p) => p,
        None => return (fb_w, fb_h),
    };
    let w: u32 = w_s.trim().parse().unwrap_or(0);
    let h: u32 = h_s.trim().parse().unwrap_or(0);
    if w >= 16 && h >= 16 && w <= 7680 && h <= 4320 {
        (w, h)
    } else {
        kprintln!("[npk] Invalid WxH; using native {}x{}", fb_w, fb_h);
        (fb_w, fb_h)
    }
}

fn wallpaper_dir() -> String {
    let home = super::home_dir();
    alloc::format!("{}/pictures/wallpapers", home)
}

/// Ensure the wallpapers directory exists.
fn ensure_wallpaper_dir() {
    let dir = wallpaper_dir();
    super::ensure_parents(&dir);
}

/// List all wallpapers in the user's wallpapers/ directory.
fn list_wallpapers() {
    let prefix = alloc::format!("{}/", wallpaper_dir());
    let entries = match crate::npkfs::list() {
        Ok(e) => e,
        Err(_) => { kprintln!("[npk] npkFS error"); return; }
    };

    let mut count = 0;
    for (name, _, _) in &entries {
        if name.starts_with(&prefix) && !name.ends_with("/.dir") {
            let short = &name[prefix.len()..];
            if !short.contains('/') {
                kprintln!("  {}", short);
                count += 1;
            }
        }
    }

    if count == 0 {
        kprintln!("[npk] No wallpapers found in {}/", wallpaper_dir());
        kprintln!("[npk] Download one: http <host> /image.png > wallpapers/name");
    } else {
        kprintln!("[npk] {} wallpaper(s)", count);
    }
}

/// Get all wallpaper names.
fn get_wallpaper_names() -> Vec<String> {
    let prefix = alloc::format!("{}/", wallpaper_dir());
    let entries = match crate::npkfs::list() {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    let mut names = Vec::new();
    for (name, _, _) in &entries {
        if name.starts_with(&prefix) && !name.ends_with("/.dir") {
            let short = &name[prefix.len()..];
            if !short.contains('/') {
                names.push(name.clone());
            }
        }
    }
    names
}

/// Set wallpaper from a specific file in npkFS.
fn set_wallpaper(name: &str) {
    if name.is_empty() {
        kprintln!("[npk] Usage: wallpaper set <name>");
        return;
    }

    let resolved = super::resolve_path(name);

    // Try resolved path first, then wallpapers/ directory as fallback
    let wp_path = alloc::format!("{}/{}", wallpaper_dir(), name);
    let (final_path, data) = match crate::npkfs::fetch(&resolved) {
        Ok((d, _h)) => (resolved, d),
        Err(_) => match crate::npkfs::fetch(&wp_path) {
            Ok((d, _h)) => (wp_path, d),
            Err(_) => {
                kprintln!("[npk] Wallpaper '{}' not found.", name);
                return;
            }
        }
    };

    kprint!("[npk] Loading {}... ", final_path);

    apply_wallpaper_data(&final_path, &data);
}

/// Apply raw image data as wallpaper.
/// Tries WASM PNG decoder first, falls back to raw BGRA.
fn apply_wallpaper_data(name: &str, data: &[u8]) {
    // Check for PNG magic: \x89PNG\r\n\x1a\n
    let is_png = data.len() > 8 && data[..8] == [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];

    if is_png {
        // Try WASM PNG decoder module
        if decode_with_wasm(name, data) {
            return;
        }
        kprintln!("[npk] PNG decoder not installed. Use: install wallpaper");
        kprintln!("[npk] Or store raw BGRA pixel data.");
        return;
    }

    // Raw BGRA data — need to know dimensions
    // Convention: first 8 bytes = width (u32 LE) + height (u32 LE), then pixels
    if data.len() < 8 {
        kprintln!("too small");
        return;
    }

    let w = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    let h = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
    let pixel_data = &data[8..];
    let expected = (w as usize) * (h as usize) * 4;

    if pixel_data.len() < expected || w == 0 || h == 0 || w > 8192 || h > 8192 {
        kprintln!("invalid dimensions ({}x{}, {} bytes)", w, h, pixel_data.len());
        return;
    }

    let info = crate::framebuffer::get_info();
    crate::gui::background::set_wallpaper(pixel_data, w, h, &info);
    crate::shade::force_redraw();

    // Save active wallpaper to config
    crate::config::set("wallpaper", name);

    kprintln!("OK ({}x{}, theme applied)", w, h);
}

/// Decode PNG via WASM module and set as wallpaper.
fn decode_with_wasm(name: &str, _data: &[u8]) -> bool {
    // Check if wallpaper WASM module is installed
    let (wasm_bytes, _) = match crate::npkfs::fetch("sys/wasm/wallpaper") {
        Ok(v) => v,
        Err(_) => return false,
    };

    // Store the target filename for the module to read via npk_fetch
    let _ = crate::npkfs::store(".npk-wallpaper-target", name.as_bytes(),
        crate::capability::CAP_NULL);

    // Delegate capability: READ (fetch image) + WRITE (set wallpaper) + EXECUTE
    let module_cap = match crate::capability::create_module_cap(
        crate::capability::Rights::READ | crate::capability::Rights::WRITE | crate::capability::Rights::EXECUTE,
        Some(30_000), // 5 minutes (PNG decode can be slow)
    ) {
        Ok(id) => id,
        Err(_) => return false,
    };

    // Run the WASM module (_start reads .npk-wallpaper-target, decodes, calls npk_set_wallpaper)
    match crate::wasm::execute_sandboxed(&wasm_bytes, "_start", &[], module_cap) {
        Ok(_) => {
            crate::config::set("wallpaper", name);
            // Clean up temp file
            let _ = crate::npkfs::delete(".npk-wallpaper-target");
            true
        }
        Err(e) => {
            crate::kprintln!("[npk] WASM decode error: {}", e);
            let _ = crate::npkfs::delete(".npk-wallpaper-target");
            false
        }
    }
}

/// Set a random wallpaper from the user's collection.
pub fn random_wallpaper() {
    let names = get_wallpaper_names();
    if names.is_empty() {
        return; // Silent — no wallpapers, keep aurora
    }

    let idx = (crate::csprng::random_u64() % names.len() as u64) as usize;
    let chosen = &names[idx];

    let (data, _) = match crate::npkfs::fetch(chosen) {
        Ok(d) => d,
        Err(_) => return,
    };

    apply_wallpaper_data(chosen, &data);
}

/// Run the wallpaper WASM module with an `@<mode>:…` target string.
/// Module handles all pixel math + stores under wp_dir + calls
/// npk_set_wallpaper itself for modes that apply immediately.
fn delegate_to_wallpaper(target: String, w: u32, h: u32, label: &str) {
    let (wasm_bytes, _) = match crate::npkfs::fetch("sys/wasm/wallpaper") {
        Ok(v) => v,
        Err(_) => {
            kprintln!("[npk] wallpaper WASM module not installed");
            return;
        }
    };

    let _ = crate::npkfs::store(".npk-wallpaper-target", target.as_bytes(),
        crate::capability::CAP_NULL);

    let module_cap = match crate::capability::create_module_cap(
        crate::capability::Rights::READ
            | crate::capability::Rights::WRITE
            | crate::capability::Rights::EXECUTE,
        Some(60_000),
    ) {
        Ok(id) => id,
        Err(_) => {
            kprintln!("[npk] failed to create module capability");
            let _ = crate::npkfs::delete(".npk-wallpaper-target");
            return;
        }
    };

    // ~40 instructions per pixel × 4 themes worst case + constant overhead.
    let fuel = (w as u64) * (h as u64) * 4 * 40 + 50_000_000;
    kprintln!("[npk] {} at {}x{}...", label, w, h);
    match crate::wasm::execute_sandboxed_with_fuel(&wasm_bytes, "_start", &[], module_cap, fuel) {
        Ok(_) => {}
        Err(e) => kprintln!("[npk] wallpaper module failed: {}", e),
    }
    let _ = crate::npkfs::delete(".npk-wallpaper-target");

    if crate::shade::is_active() {
        crate::shade::render_frame();
    }
}

fn generate_demo_wallpapers(res_arg: &str) {
    let (w, h) = parse_resolution(res_arg);
    if w == 0 || h == 0 { kprintln!("[npk] No framebuffer available"); return; }
    ensure_wallpaper_dir();
    let target = alloc::format!("@demos:{}x{}:{}", w, h, wallpaper_dir());
    delegate_to_wallpaper(target, w, h, "Generating demo wallpapers");
    random_wallpaper();
}

fn generate_solid_wallpaper(arg: &str) {
    let hex = arg.trim();
    if hex.is_empty() {
        kprintln!("[npk] Usage: wallpaper solid <rrggbb>");
        return;
    }
    let (w, h) = crate::framebuffer::get_resolution();
    if w == 0 || h == 0 { kprintln!("[npk] No framebuffer available"); return; }
    ensure_wallpaper_dir();
    let target = alloc::format!("@solid:{}:{}x{}:{}", hex, w, h, wallpaper_dir());
    delegate_to_wallpaper(target, w, h, "Generating solid wallpaper");
}

fn generate_pattern_wallpaper(arg: &str) {
    let parts: Vec<&str> = arg.split_whitespace().collect();
    if parts.len() != 3 {
        kprintln!("[npk] Usage: wallpaper pattern <dots|stripes|checker|grid|noise> <fg> <bg>");
        return;
    }
    let name = parts[0];
    let fg = parts[1];
    let bg = parts[2];
    let (w, h) = crate::framebuffer::get_resolution();
    if w == 0 || h == 0 { kprintln!("[npk] No framebuffer available"); return; }
    ensure_wallpaper_dir();
    let target = alloc::format!("@pattern:{}:{}:{}:{}x{}:{}", name, fg, bg, w, h, wallpaper_dir());
    delegate_to_wallpaper(target, w, h, "Generating pattern wallpaper");
}

fn generate_gradient_wallpaper(arg: &str) {
    let parts: Vec<&str> = arg.split_whitespace().collect();
    let (w, h) = crate::framebuffer::get_resolution();
    if w == 0 || h == 0 { kprintln!("[npk] No framebuffer available"); return; }
    ensure_wallpaper_dir();
    let wp_dir = wallpaper_dir();

    let target = match parts.len() {
        2 => alloc::format!("@gradient2:{}:{}:{}x{}:{}", parts[0], parts[1], w, h, wp_dir),
        4 => alloc::format!("@gradient4:{}:{}:{}:{}:{}x{}:{}",
            parts[0], parts[1], parts[2], parts[3], w, h, wp_dir),
        _ => {
            kprintln!("[npk] Usage: wallpaper gradient <tl> <tr> <bl> <br>   or   <top> <bottom>");
            return;
        }
    };
    delegate_to_wallpaper(target, w, h, "Generating gradient wallpaper");
}

/// Clear wallpaper, revert to default background.
fn clear_wallpaper() {
    crate::gui::background::clear_wallpaper();
    crate::shade::force_redraw();
    crate::config::set("wallpaper", "");
    kprintln!("[npk] Wallpaper cleared, aurora restored.");
}
