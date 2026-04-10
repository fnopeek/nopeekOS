//! Wallpaper intent — set, clear, list, random wallpaper.
//!
//! Images are stored in npkFS under home/<user>/wallpapers/.
//! The wallpaper WASM module (sys/wasm/wallpaper) decodes PNG→BGRA.
//! Without the WASM module, raw BGRA data is used directly.

use crate::{kprintln, kprint};
use alloc::string::String;
use alloc::vec;
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
        "demo" => generate_demo_wallpapers(),
        "" => {
            kprintln!("[npk] Usage: wallpaper <set|clear|list|random|demo>");
            kprintln!("[npk]   wallpaper set <name>     Set wallpaper from npkFS");
            kprintln!("[npk]   wallpaper clear          Revert to aurora");
            kprintln!("[npk]   wallpaper list           List available wallpapers");
            kprintln!("[npk]   wallpaper random         Set random wallpaper");
        }
        other => {
            // Treat as `wallpaper set <name>` shortcut
            set_wallpaper(other);
        }
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
        Ok((d, h)) => (resolved, d),
        Err(_) => match crate::npkfs::fetch(&wp_path) {
            Ok((d, h)) => (wp_path, d),
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

/// Generate 3 demo wallpapers with different color schemes and set one randomly.
fn generate_demo_wallpapers() {
    let (fb_w, fb_h) = crate::framebuffer::get_resolution();
    if fb_w == 0 || fb_h == 0 {
        kprintln!("[npk] No framebuffer available");
        return;
    }

    // Quarter resolution — gradients scale perfectly, 2MB vs 32MB per image
    let w: u32 = fb_w / 4;
    let h: u32 = fb_h / 4;

    ensure_wallpaper_dir();
    let wp_dir = wallpaper_dir();

    struct DemoTheme {
        name: &'static str,
        // Corner colors: top-left, top-right, bottom-left, bottom-right (RGB)
        tl: (u8, u8, u8),
        tr: (u8, u8, u8),
        bl: (u8, u8, u8),
        br: (u8, u8, u8),
    }

    let themes = [
        DemoTheme {
            name: "ocean",
            tl: (2, 3, 15),        // near black
            tr: (5, 30, 60),       // dark navy
            bl: (10, 60, 120),     // deep ocean
            br: (60, 200, 240),    // bright cyan
        },
        DemoTheme {
            name: "sunset",
            tl: (10, 2, 15),       // near black
            tr: (80, 10, 30),      // dark crimson
            bl: (160, 40, 20),     // deep orange
            br: (255, 160, 80),    // bright amber
        },
        DemoTheme {
            name: "forest",
            tl: (2, 8, 3),         // near black
            tr: (8, 40, 15),       // dark forest
            bl: (15, 80, 30),      // deep emerald
            br: (80, 220, 100),    // bright green
        },
        DemoTheme {
            name: "aurora",
            tl: (5, 2, 18),        // near black
            tr: (30, 10, 80),      // dark indigo
            bl: (80, 20, 160),     // deep purple
            br: (180, 100, 255),   // bright violet
        },
    ];

    // Header: 8 bytes (width u32 LE + height u32 LE) + BGRA pixels
    let pixel_count = (w * h) as usize;
    let data_size = 8 + pixel_count * 4;

    for theme in &themes {
        kprint!("[npk] Generating {}... ", theme.name);

        let mut data = vec![0u8; data_size];
        // Write dimensions header
        data[0..4].copy_from_slice(&w.to_le_bytes());
        data[4..8].copy_from_slice(&h.to_le_bytes());

        // Generate bilinear gradient with subtle noise
        for y in 0..h {
            // Keep USB input alive during generation (~32M pixels)
            if y % 256 == 0 { crate::xhci::poll_events(); }
            let fy = y as u32 * 1000 / h;
            for x in 0..w {
                let fx = x as u32 * 1000 / w;

                // Bilinear interpolation of 4 corners
                let r = bilinear(theme.tl.0, theme.tr.0, theme.bl.0, theme.br.0, fx, fy);
                let g = bilinear(theme.tl.1, theme.tr.1, theme.bl.1, theme.br.1, fx, fy);
                let b = bilinear(theme.tl.2, theme.tr.2, theme.bl.2, theme.br.2, fx, fy);

                // Add subtle diagonal streaks (like aurora but simpler)
                let diag = ((x as i32 * 600 + y as i32 * 800) / 1000) as u32;
                let wave = sine_lut((diag * 5) % 1024);
                let r = (r as i32 + wave * 8 / 256).clamp(0, 255) as u8;
                let g = (g as i32 + wave * 6 / 256).clamp(0, 255) as u8;
                let b = (b as i32 + wave * 10 / 256).clamp(0, 255) as u8;

                // Radial glow toward bottom-right (reinforces dark→bright gradient)
                let dx = (fx as i32 - 700).abs();
                let dy = (fy as i32 - 650).abs();
                let glow = 180i32.saturating_sub((dx * dx + dy * dy) / 600).max(0);
                let r = (r as i32 + glow / 5).min(255) as u8;
                let g = (g as i32 + glow / 4).min(255) as u8;
                let b = (b as i32 + glow / 3).min(255) as u8;

                let off = 8 + ((y * w + x) as usize) * 4;
                data[off]     = b; // B
                data[off + 1] = g; // G
                data[off + 2] = r; // R
                data[off + 3] = 255; // A
            }
        }

        let store_name = alloc::format!("{}/{}", wp_dir, theme.name);
        match crate::npkfs::store(&store_name, &data, crate::capability::CAP_NULL) {
            Ok(_) => kprintln!("OK ({}x{})", w, h),
            Err(e) => { kprintln!("failed: {:?}", e); continue; }
        }
        // Flush output to screen
        if crate::shade::is_active() {
            crate::shade::render_frame();
        }
    }

    kprintln!("[npk] {} demo wallpapers generated.", themes.len());
    if crate::shade::is_active() {
        crate::shade::render_frame();
    }
    kprintln!("[npk] Setting random wallpaper...");
    random_wallpaper();
}

/// Bilinear interpolation of 4 corner values.
fn bilinear(tl: u8, tr: u8, bl: u8, br: u8, fx: u32, fy: u32) -> u8 {
    let top = (tl as u32 * (1000 - fx) + tr as u32 * fx) / 1000;
    let bot = (bl as u32 * (1000 - fx) + br as u32 * fx) / 1000;
    ((top * (1000 - fy) + bot * fy) / 1000) as u8
}

/// Simple sine lookup (0..1023 → -128..127).
fn sine_lut(x: u32) -> i32 {
    // Quarter-wave table (64 entries), mirrored + negated for full wave
    const TABLE: [i8; 64] = [
        0, 3, 6, 9, 12, 16, 19, 22, 25, 28, 31, 34, 37, 40, 43, 46,
        49, 51, 54, 57, 60, 62, 65, 67, 70, 72, 75, 77, 79, 81, 83, 85,
        87, 89, 91, 93, 94, 96, 97, 99, 100, 101, 102, 104, 105, 105, 106, 107,
        108, 108, 109, 109, 110, 110, 110, 110, 111, 111, 111, 111, 111, 111, 111, 111,
    ];
    let idx = (x % 1024) as usize;
    let quarter = idx / 256;
    let pos = (idx % 256) * 64 / 256;
    let val = match quarter {
        0 => TABLE[pos] as i32,
        1 => TABLE[63 - pos] as i32,
        2 => -(TABLE[pos] as i32),
        _ => -(TABLE[63 - pos] as i32),
    };
    val
}

/// Clear wallpaper, revert to aurora.
fn clear_wallpaper() {
    crate::gui::background::clear_wallpaper();
    crate::shade::force_redraw();
    crate::config::set("wallpaper", "");
    kprintln!("[npk] Wallpaper cleared, aurora restored.");
}
