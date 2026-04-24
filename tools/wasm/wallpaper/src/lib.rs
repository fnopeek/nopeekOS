//! Wallpaper WASM module for nopeekOS.
//!
//! Two modes, both via `_start`:
//!   1. **Decode**: target file is a filename → fetch PNG → decode → set.
//!   2. **Generate**: target starts with `@demos:<W>x<H>:<wp_dir>` →
//!      write 4 gradient wallpapers into `<wp_dir>/<theme>`.
//!
//! The kernel picks the mode by writing `.npk-wallpaper-target`
//! accordingly. Runs inside the nopeekOS WASM sandbox (wasmi).

#![no_std]

#[unsafe(link_section = ".npk.app_meta")]
#[used]
static APP_META_BYTES: [u8; include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin")).len()]
    = *include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin"));

extern crate alloc;

use alloc::format;
use alloc::vec;
use alloc::vec::Vec;

// --- Host function bindings (provided by nopeekOS kernel) ---

unsafe extern "C" {
    fn npk_fetch(name_ptr: i32, name_len: i32, buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_store(name_ptr: i32, name_len: i32, data_ptr: i32, data_len: i32) -> i32;
    fn npk_set_wallpaper(ptr: i32, len: i32, width: i32, height: i32) -> i32;
    fn npk_log(ptr: i32, len: i32);
}

fn log(msg: &str) {
    unsafe { npk_log(msg.as_ptr() as i32, msg.len() as i32); }
}

fn fetch(name: &str, buf: &mut [u8]) -> Option<usize> {
    let result = unsafe {
        npk_fetch(name.as_ptr() as i32, name.len() as i32,
                  buf.as_mut_ptr() as i32, buf.len() as i32)
    };
    if result < 0 { None } else { Some(result as usize) }
}

fn set_wallpaper(pixels: &[u8], w: u32, h: u32) -> bool {
    let result = unsafe {
        npk_set_wallpaper(pixels.as_ptr() as i32, pixels.len() as i32,
                          w as i32, h as i32)
    };
    result == 0
}

fn store(name: &str, data: &[u8]) -> bool {
    let result = unsafe {
        npk_store(name.as_ptr() as i32, name.len() as i32,
                  data.as_ptr() as i32, data.len() as i32)
    };
    result == 0
}

// --- Simple bump allocator for WASM ---
//
// 64 MB heap covers worst-case native-resolution gradient generation:
// one reusable BGRA buffer at 3840x2160 = 32 MB + ancillary allocs.
// Bump allocator never frees — the module runs once per intent and
// exits, so the WASM linear memory is discarded afterwards anyway.
const HEAP_SIZE: usize = 64 * 1024 * 1024; // 64 MB
static mut HEAP: [u8; HEAP_SIZE] = [0; HEAP_SIZE];
static mut HEAP_POS: usize = 0;

struct BumpAllocator;

unsafe impl core::alloc::GlobalAlloc for BumpAllocator {
    unsafe fn alloc(&self, layout: core::alloc::Layout) -> *mut u8 {
        let align = layout.align();
        let size = layout.size();
        let pos_ptr = core::ptr::addr_of_mut!(HEAP_POS);
        let current = unsafe { pos_ptr.read() };
        let aligned = (current + align - 1) & !(align - 1);
        if aligned + size > HEAP_SIZE {
            return core::ptr::null_mut();
        }
        unsafe { pos_ptr.write(aligned + size); }
        let heap_ptr = core::ptr::addr_of_mut!(HEAP) as *mut u8;
        unsafe { heap_ptr.add(aligned) }
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: core::alloc::Layout) {
        // Bump allocator: no deallocation (module runs once and exits)
    }
}

#[global_allocator]
static ALLOCATOR: BumpAllocator = BumpAllocator;

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    log("[wallpaper] panic!");
    loop {}
}

// --- Entry point ---

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    // Read target from .npk-wallpaper-target — either a PNG filename
    // (decode mode) or a `@demos:<W>x<H>:<wp_dir>` string (generate mode).
    let mut name_buf = [0u8; 512];
    let name_len = match fetch(".npk-wallpaper-target", &mut name_buf) {
        Some(n) => n,
        None => { log("[wallpaper] no target file"); return; }
    };
    let target = match core::str::from_utf8(&name_buf[..name_len]) {
        Ok(s) => s,
        Err(_) => { log("[wallpaper] invalid target"); return; }
    };

    if let Some(spec) = target.strip_prefix("@demos:") {
        run_generate(spec);
    } else {
        run_decode(target);
    }
}

// --- Decode mode (PNG → BGRA → set framebuffer) ---

fn run_decode(filename: &str) {
    let max_size = 6 * 1024 * 1024; // 6 MB max
    let mut img_buf = vec![0u8; max_size];
    let img_len = match fetch(filename, &mut img_buf) {
        Some(n) => n,
        None => { log("[wallpaper] failed to fetch image"); return; }
    };

    let (pixels, width, height) = match decode_png(&img_buf[..img_len]) {
        Some(v) => v,
        None => { log("[wallpaper] PNG decode failed"); return; }
    };

    if set_wallpaper(&pixels, width, height) {
        log("[wallpaper] OK");
    } else {
        log("[wallpaper] npk_set_wallpaper failed");
    }
}

// --- Generate mode (4 gradient themes into wp_dir) ---

/// Corner colors for one theme: top-left, top-right, bottom-left,
/// bottom-right (R, G, B).
struct Theme {
    name: &'static str,
    tl: (u8, u8, u8),
    tr: (u8, u8, u8),
    bl: (u8, u8, u8),
    br: (u8, u8, u8),
}

const THEMES: &[Theme] = &[
    Theme {
        name: "ocean",
        tl: (2, 3, 15),        // near black
        tr: (5, 30, 60),       // dark navy
        bl: (10, 60, 120),     // deep ocean
        br: (60, 200, 240),    // bright cyan
    },
    Theme {
        name: "sunset",
        tl: (10, 2, 15),       // near black
        tr: (80, 10, 30),      // dark crimson
        bl: (160, 40, 20),     // deep orange
        br: (255, 160, 80),    // bright amber
    },
    Theme {
        name: "forest",
        tl: (2, 8, 3),         // near black
        tr: (8, 40, 15),       // dark forest
        bl: (15, 80, 30),      // deep emerald
        br: (80, 220, 100),    // bright green
    },
    Theme {
        name: "aurora",
        tl: (5, 2, 18),        // near black
        tr: (30, 10, 80),      // dark indigo
        bl: (80, 20, 160),     // deep purple
        br: (180, 100, 255),   // bright violet
    },
];

fn run_generate(spec: &str) {
    // Parse "<W>x<H>:<wp_dir>"
    let (dims, wp_dir) = match spec.split_once(':') {
        Some(v) => v,
        None => { log("[wallpaper] bad @demos spec (missing dir)"); return; }
    };
    let (w_s, h_s) = match dims.split_once('x') {
        Some(v) => v,
        None => { log("[wallpaper] bad @demos spec (missing WxH)"); return; }
    };
    let width: u32 = match w_s.parse() {
        Ok(v) if v >= 16 && v <= 7680 => v,
        _ => { log("[wallpaper] bad width"); return; }
    };
    let height: u32 = match h_s.parse() {
        Ok(v) if v >= 16 && v <= 4320 => v,
        _ => { log("[wallpaper] bad height"); return; }
    };

    // One reusable BGRA buffer with an 8-byte (W LE + H LE) header —
    // kernel's background::set_wallpaper consumes this exact layout.
    let pixel_count = (width as usize) * (height as usize);
    let data_size = 8 + pixel_count * 4;
    let mut data = vec![0u8; data_size];
    data[0..4].copy_from_slice(&width.to_le_bytes());
    data[4..8].copy_from_slice(&height.to_le_bytes());

    for theme in THEMES {
        fill_gradient(&mut data[8..], width, height, theme);
        let path = format!("{}/{}", wp_dir, theme.name);
        if store(&path, &data) {
            log(&format!("[wallpaper] {} {}x{} OK", theme.name, width, height));
        } else {
            log(&format!("[wallpaper] {} npk_store failed", theme.name));
        }
    }
}

/// Fill a BGRA pixel slice with the bilinear-interpolated gradient of
/// `theme`, plus a sine streak overlay and a bottom-right radial glow.
/// The pixel math mirrors the original kernel implementation.
fn fill_gradient(pixels: &mut [u8], w: u32, h: u32, t: &Theme) {
    for y in 0..h {
        let fy = y * 1000 / h;
        for x in 0..w {
            let fx = x * 1000 / w;

            let r = bilinear(t.tl.0, t.tr.0, t.bl.0, t.br.0, fx, fy);
            let g = bilinear(t.tl.1, t.tr.1, t.bl.1, t.br.1, fx, fy);
            let b = bilinear(t.tl.2, t.tr.2, t.bl.2, t.br.2, fx, fy);

            // Diagonal sine streaks (soft aurora-like bands)
            let diag = ((x as i32 * 600 + y as i32 * 800) / 1000) as u32;
            let wave = sine_lut((diag * 5) % 1024);
            let r = (r as i32 + wave * 8 / 256).clamp(0, 255) as u8;
            let g = (g as i32 + wave * 6 / 256).clamp(0, 255) as u8;
            let b = (b as i32 + wave * 10 / 256).clamp(0, 255) as u8;

            // Radial glow toward bottom-right
            let dx = (fx as i32 - 700).abs();
            let dy = (fy as i32 - 650).abs();
            let glow = 180i32.saturating_sub((dx * dx + dy * dy) / 600).max(0);
            let r = (r as i32 + glow / 5).min(255) as u8;
            let g = (g as i32 + glow / 4).min(255) as u8;
            let b = (b as i32 + glow / 3).min(255) as u8;

            let off = ((y * w + x) as usize) * 4;
            pixels[off]     = b;
            pixels[off + 1] = g;
            pixels[off + 2] = r;
            pixels[off + 3] = 255;
        }
    }
}

/// Bilinear interpolation of 4 corner values.
fn bilinear(tl: u8, tr: u8, bl: u8, br: u8, fx: u32, fy: u32) -> u8 {
    let top = (tl as u32 * (1000 - fx) + tr as u32 * fx) / 1000;
    let bot = (bl as u32 * (1000 - fx) + br as u32 * fx) / 1000;
    ((top * (1000 - fy) + bot * fy) / 1000) as u8
}

/// Quarter-wave sine lookup (0..1023 → −128..127 via mirroring).
fn sine_lut(x: u32) -> i32 {
    const TABLE: [i8; 64] = [
        0, 3, 6, 9, 12, 16, 19, 22, 25, 28, 31, 34, 37, 40, 43, 46,
        49, 51, 54, 57, 60, 62, 65, 67, 70, 72, 75, 77, 79, 81, 83, 85,
        87, 89, 91, 93, 94, 96, 97, 99, 100, 101, 102, 104, 105, 105, 106, 107,
        108, 108, 109, 109, 110, 110, 110, 110, 111, 111, 111, 111, 111, 111, 111, 111,
    ];
    let idx = (x % 1024) as usize;
    let quarter = idx / 256;
    let pos = (idx % 256) * 64 / 256;
    match quarter {
        0 => TABLE[pos] as i32,
        1 => TABLE[63 - pos] as i32,
        2 => -(TABLE[pos] as i32),
        _ => -(TABLE[63 - pos] as i32),
    }
}

// --- PNG Decoder ---

fn decode_png(data: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
    // Verify PNG signature
    if data.len() < 8 || &data[0..8] != b"\x89PNG\r\n\x1a\n" {
        return None;
    }

    let mut pos = 8;
    let mut width: u32 = 0;
    let mut height: u32 = 0;
    let mut bit_depth: u8 = 0;
    let mut color_type: u8 = 0;
    let mut idat_data: Vec<u8> = Vec::new();

    // Parse chunks
    while pos + 12 <= data.len() {
        let chunk_len = u32::from_be_bytes([data[pos], data[pos+1], data[pos+2], data[pos+3]]) as usize;
        let chunk_type = &data[pos+4..pos+8];
        let chunk_data_start = pos + 8;
        let chunk_data_end = chunk_data_start + chunk_len;

        if chunk_data_end > data.len() { break; }

        match chunk_type {
            b"IHDR" => {
                if chunk_len < 13 { return None; }
                let d = &data[chunk_data_start..];
                width = u32::from_be_bytes([d[0], d[1], d[2], d[3]]);
                height = u32::from_be_bytes([d[4], d[5], d[6], d[7]]);
                bit_depth = d[8];
                color_type = d[9];
                let compression = d[10];
                let filter = d[11];
                let interlace = d[12];

                if compression != 0 || filter != 0 || interlace != 0 {
                    log("[wallpaper] unsupported PNG format (interlaced)");
                    return None;
                }
                if bit_depth != 8 {
                    log("[wallpaper] only 8-bit PNG supported");
                    return None;
                }
                if color_type != 2 && color_type != 6 {
                    // 2 = RGB, 6 = RGBA
                    log("[wallpaper] only RGB/RGBA PNG supported");
                    return None;
                }
            }
            b"IDAT" => {
                idat_data.extend_from_slice(&data[chunk_data_start..chunk_data_end]);
            }
            b"IEND" => break,
            _ => {} // Skip unknown chunks
        }

        pos = chunk_data_end + 4; // +4 for CRC
    }

    if width == 0 || height == 0 || idat_data.is_empty() {
        return None;
    }

    // Channels per pixel
    let channels: usize = match color_type {
        2 => 3, // RGB
        6 => 4, // RGBA
        _ => return None,
    };

    let stride = width as usize * channels; // bytes per row (without filter byte)

    // Decompress IDAT (zlib = 2-byte header + deflate + 4-byte checksum)
    if idat_data.len() < 6 { return None; }
    // Skip zlib header (2 bytes), decompress deflate stream
    let deflate_data = &idat_data[2..];
    let decompressed = match miniz_oxide::inflate::decompress_to_vec_zlib(&idat_data) {
        Ok(d) => d,
        Err(_) => {
            // Try raw deflate without zlib wrapper
            match miniz_oxide::inflate::decompress_to_vec(deflate_data) {
                Ok(d) => d,
                Err(_) => { log("[wallpaper] deflate failed"); return None; }
            }
        }
    };

    let expected = height as usize * (1 + stride); // 1 filter byte per row
    if decompressed.len() < expected {
        log("[wallpaper] decompressed size mismatch");
        return None;
    }

    // Unfilter rows
    let mut unfiltered = vec![0u8; height as usize * stride];
    for y in 0..height as usize {
        let src_offset = y * (1 + stride);
        let filter_type = decompressed[src_offset];
        let row_start = src_offset + 1;
        let dst_offset = y * stride;

        for x in 0..stride {
            let raw = decompressed[row_start + x];
            let a = if x >= channels { unfiltered[dst_offset + x - channels] } else { 0 }; // left
            let b = if y > 0 { unfiltered[dst_offset - stride + x] } else { 0 }; // up
            let c = if x >= channels && y > 0 { unfiltered[dst_offset - stride + x - channels] } else { 0 }; // up-left

            let val = match filter_type {
                0 => raw,                                          // None
                1 => raw.wrapping_add(a),                         // Sub
                2 => raw.wrapping_add(b),                         // Up
                3 => raw.wrapping_add(((a as u16 + b as u16) / 2) as u8), // Average
                4 => raw.wrapping_add(paeth(a, b, c)),            // Paeth
                _ => raw,
            };
            unfiltered[dst_offset + x] = val;
        }
    }

    // Convert to BGRA (framebuffer format)
    let pixel_count = (width * height) as usize;
    let mut bgra = vec![0u8; pixel_count * 4];
    for i in 0..pixel_count {
        let src = i * channels;
        let dst = i * 4;
        bgra[dst]     = unfiltered[src + 2]; // B
        bgra[dst + 1] = unfiltered[src + 1]; // G
        bgra[dst + 2] = unfiltered[src];     // R
        bgra[dst + 3] = if channels == 4 { unfiltered[src + 3] } else { 255 }; // A
    }

    Some((bgra, width, height))
}

/// Paeth predictor (PNG filter type 4).
fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = a as i16 + b as i16 - c as i16;
    let pa = (p - a as i16).unsigned_abs();
    let pb = (p - b as i16).unsigned_abs();
    let pc = (p - c as i16).unsigned_abs();
    if pa <= pb && pa <= pc { a }
    else if pb <= pc { b }
    else { c }
}
