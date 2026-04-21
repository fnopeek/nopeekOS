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
        "" => {
            kprintln!("[npk] Usage: wallpaper <set|clear|list|random|demo>");
            kprintln!("[npk]   wallpaper set <name>     Set wallpaper from npkFS");
            kprintln!("[npk]   wallpaper clear          Revert to aurora");
            kprintln!("[npk]   wallpaper list           List available wallpapers");
            kprintln!("[npk]   wallpaper random         Set random wallpaper");
            kprintln!("[npk]   wallpaper demo [WxH]     Generate gradients (default: native res)");
        }
        other => {
            // Treat as `wallpaper set <name>` shortcut
            set_wallpaper(other);
        }
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
    alloc::format!("{}/wallpapers", home)
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

/// Generate demo gradient wallpapers by delegating to the wallpaper
/// WASM module. The kernel owns no pixel math here — it ensures the
/// target directory, hands the module a `@demos:<W>x<H>:<wp_dir>`
/// instruction via .npk-wallpaper-target, then picks a random result.
///
/// `res_arg` is the user-supplied argument after `wallpaper demo` —
/// either empty (→ native framebuffer resolution) or `WxH`.
fn generate_demo_wallpapers(res_arg: &str) {
    let (w, h) = parse_resolution(res_arg);
    if w == 0 || h == 0 {
        kprintln!("[npk] No framebuffer available");
        return;
    }

    let (wasm_bytes, _) = match crate::npkfs::fetch("sys/wasm/wallpaper") {
        Ok(v) => v,
        Err(_) => {
            kprintln!("[npk] wallpaper WASM module not installed");
            return;
        }
    };

    ensure_wallpaper_dir();
    let wp_dir = wallpaper_dir();

    let target = alloc::format!("@demos:{}x{}:{}", w, h, wp_dir);
    let _ = crate::npkfs::store(".npk-wallpaper-target", target.as_bytes(),
        crate::capability::CAP_NULL);

    let module_cap = match crate::capability::create_module_cap(
        crate::capability::Rights::READ
            | crate::capability::Rights::WRITE
            | crate::capability::Rights::EXECUTE,
        Some(60_000), // generate at 4K can take a few seconds
    ) {
        Ok(id) => id,
        Err(_) => {
            kprintln!("[npk] failed to create module capability");
            let _ = crate::npkfs::delete(".npk-wallpaper-target");
            return;
        }
    };

    kprintln!("[npk] Generating demo wallpapers at {}x{}...", w, h);
    match crate::wasm::execute_sandboxed(&wasm_bytes, "_start", &[], module_cap) {
        Ok(_) => kprintln!("[npk] {} themes generated.", 4),
        Err(e) => kprintln!("[npk] generate failed: {}", e),
    }
    let _ = crate::npkfs::delete(".npk-wallpaper-target");

    if crate::shade::is_active() {
        crate::shade::render_frame();
    }
    kprintln!("[npk] Setting random wallpaper...");
    random_wallpaper();
}

/// Clear wallpaper, revert to aurora.
fn clear_wallpaper() {
    crate::gui::background::clear_wallpaper();
    crate::shade::force_redraw();
    crate::config::set("wallpaper", "");
    kprintln!("[npk] Wallpaper cleared, aurora restored.");
}
