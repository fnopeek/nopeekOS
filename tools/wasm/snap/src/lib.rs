//! snap — screenshot tool for nopeekOS.
//!
//! One-shot, no window (full-screen mode). Launched by the bar's camera
//! button via `npk_launch("snap", "full" | "region")`:
//!   - "full"   → capture the whole composited screen, save as PNG.
//!   - "region" → not implemented yet; falls back to a full capture.
//!
//! The kernel only hands over raw BGRA pixels (`npk_capture_screen`,
//! CAPTURE-gated); snap does the PNG encode + save itself. Files land in
//! `home/<user>/pictures/printscreens/screenshot-NNN.png` (the folder is
//! created on first write — npkFS upsert writes parents).

#![no_std]

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use nopeek_widgets::app_meta::IconRef;

#[unsafe(link_section = ".npk.app_meta")]
#[used]
static APP_META_BYTES: [u8; include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin")).len()]
    = *include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin"));

// Read (home dir + list) + write (save PNG) + capture (read the screen).
// No RENDER/CANVAS: full-screen mode shows no window. A region mode
// with a frozen overlay would need CANVAS|RENDER.
#[unsafe(link_section = ".npk.caps")]
#[used]
static NPK_CAPS: [u8; 1] = [nopeek_widgets::caps::READ | nopeek_widgets::caps::WRITE | nopeek_widgets::caps::CAPTURE];

use nopeek_widgets::host;

// The two calls only snap makes; the shared ones are in
// `nopeek_widgets::host`.
#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn npk_capture_screen(buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_screen_flash() -> i32;
}

/// Copy the composited screen as BGRA into `buf`; the byte count or a
/// negative refusal.
fn capture_screen(buf: &mut [u8]) -> i32 {
    // SAFETY: FFI; the range is borrowed for the call and validated by the
    // kernel.
    unsafe { npk_capture_screen(buf.as_mut_ptr() as i32, buf.len() as i32) }
}

fn screen_flash() {
    // SAFETY: FFI without pointers.
    unsafe { npk_screen_flash() };
}

fn log(msg: &str) { host::log_serial(msg); }

fn now_ms() -> i64 { host::ticks_ms() as i64 }

/// Log "<label>: <ms> ms". Permanent, not scaffolding: this runs under a
/// WASM interpreter, so which phase a save spends its seconds in is the
/// difference between fixing the slow thing and rewriting the fast one.
/// Resolution is the 100 Hz timer, i.e. 10 ms steps.
fn log_ms(label: &str, ms: i64) {
    let mut b = String::new();
    b.push_str("[snap] ");
    b.push_str(label);
    b.push_str(": ");
    push_i64(&mut b, ms);
    b.push_str(" ms");
    log(&b);
}

fn push_i64(out: &mut String, mut v: i64) {
    if v < 0 { out.push('-'); v = -v; }
    let mut digits = [0u8; 20];
    let mut n = 0;
    loop {
        digits[n] = b'0' + (v % 10) as u8;
        v /= 10;
        n += 1;
        if v == 0 { break; }
    }
    while n > 0 { n -= 1; out.push(digits[n] as char); }
}

// ── Buffers ───────────────────────────────────────────────────────────
const LIST_BUF_SIZE: usize = 64 * 1024;
const HOME_CAP: usize = 256;

// The SDK's growing heap: it frees, and it takes memory from the runtime as
// it is needed instead of reserving it at launch.
#[global_allocator]
static ALLOCATOR: nopeek_widgets::heap::Allocator = nopeek_widgets::heap::new();

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { log("[snap] panic!"); core::arch::wasm32::unreachable() }

// ── Entry ─────────────────────────────────────────────────────────────
#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    // Mode (unused — region falls back to full).
    let mut argbuf = [0u8; 32];
    let n = host::launch_arg(&mut argbuf).unwrap_or(0);
    let _mode = if n > 0 { core::str::from_utf8(&argbuf[..n]).unwrap_or("full") } else { "full" };

    let (w, h) = host::screen_size();
    if (w, h) == (0, 0) { log("[snap] screen_size failed"); return; }
    let (w, h) = (w as usize, h as usize);
    if w == 0 || h == 0 { log("[snap] zero screen"); return; }

    let t_start = now_ms();

    // Capture the composited screen as BGRA.
    let need = w * h * 4;
    let mut bgra: Vec<u8> = alloc::vec![0u8; need];
    let got = capture_screen(&mut bgra);
    if got as usize != need { log("[snap] capture failed"); return; }
    let t_captured = now_ms();
    log_ms("capture", t_captured - t_start);

    // Shutter blink — after the capture, so the white is never in the
    // shot, and before the encode, so the acknowledgement is immediate
    // rather than a second later when the file lands.
    screen_flash();

    // Encode PNG (RGB, screenshots have no meaningful alpha).
    let png = encode_png_rgb(&bgra, w as u32, h as u32);
    let t_encoded = now_ms();

    // Save under the printscreens folder (created on first write).
    let home = read_home_dir();
    let dir = alloc::format!("{}/pictures/printscreens", home);
    let name = next_name(&dir);
    let path = alloc::format!("{}/{}", dir, name);
    let saved = host::store(&path, &png);
    let t_stored = now_ms();
    log_ms("store", t_stored - t_encoded);
    // The number that matters: button press → file on disk.
    log_ms("=== capture -> saved", t_stored - t_start);
    if !saved { log("[snap] save failed"); } else { log("[snap] saved screenshot"); }
}

// ── npkFS helpers ─────────────────────────────────────────────────────
fn read_home_dir() -> String {
    let mut buf = [0u8; HOME_CAP];
    let n = host::home_dir(&mut buf).unwrap_or(0);
    if n == 0 { return "home".to_string(); }
    core::str::from_utf8(&buf[..n]).unwrap_or("home").to_string()
}

/// Next free `screenshot-NNN.png` in `dir` (clock-free naming — we have
/// no time host fn, so number sequentially from the existing files).
fn next_name(dir: &str) -> String {
    let mut buf = alloc::vec![0u8; LIST_BUF_SIZE];
    let n = host::fs_list(dir, &mut buf, false).unwrap_or(0);
    let mut max: u32 = 0;
    if n > 0 {
        for e in nopeek_widgets::fs::list_entries(&buf[..n]) {
            if let Some(num) = e.name.strip_prefix("screenshot-").and_then(|s| s.strip_suffix(".png")) {
                if let Ok(v) = num.parse::<u32>() { if v > max { max = v; } }
            }
        }
    }
    alloc::format!("screenshot-{:03}.png", max + 1)
}

// ── PNG encoder (RGB8, single IDAT, filter 0) ─────────────────────────
fn encode_png_rgb(bgra: &[u8], w: u32, h: u32) -> Vec<u8> {
    let wc = w as usize;
    let hc = h as usize;
    let t0 = now_ms();
    // Raw scanlines: 1 filter byte (0 = None) + W*3 RGB bytes per row.
    //
    // Filter 0 for every row is deliberate on both ends: it costs the
    // encoder nothing, and it lets iris un-filter a whole row with one
    // `copy_from_slice` (a wasm `memory.copy`) instead of a per-byte
    // add. A cleverer filter would shrink the file and make both sides
    // walk the image byte by byte under the interpreter.
    //
    // Pre-zeroed + indexed writes rather than `push`: three pushes per
    // pixel is ~25 M capacity checks at 4K. `chunks_exact` drops the
    // per-access bounds checks on top.
    let row_bytes = 1 + wc * 3;
    let mut raw: Vec<u8> = alloc::vec![0u8; hc * row_bytes];
    for y in 0..hc {
        let dst = y * row_bytes + 1;             // filter byte stays 0
        let src = y * wc * 4;
        let dst_row = &mut raw[dst..dst + wc * 3];
        let src_row = &bgra[src..src + wc * 4];
        for (d, s) in dst_row.chunks_exact_mut(3).zip(src_row.chunks_exact(4)) {
            d[0] = s[2];  // B G R A → R G B
            d[1] = s[1];
            d[2] = s[0];
        }
    }
    let t_repack = now_ms();
    log_ms("repack bgra->rgb", t_repack - t0);

    // zlib-wrapped deflate (header + deflate + adler32).
    //
    // Level 1, not 6: under the interpreter, level 6 makes deflate the
    // bulk of the save time for a file that is only somewhat smaller. A
    // screenshot is a transient artefact; seconds of the user's time cost
    // more than the megabyte.
    let idat = miniz_oxide::deflate::compress_to_vec_zlib(&raw, 1);
    let t_deflate = now_ms();
    log_ms("deflate", t_deflate - t_repack);

    let mut out: Vec<u8> = Vec::with_capacity(idat.len() + 64);
    out.extend_from_slice(b"\x89PNG\r\n\x1a\n");

    // IHDR
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.push(8);  // bit depth
    ihdr.push(2);  // color type 2 = truecolour RGB
    ihdr.push(0);  // compression
    ihdr.push(0);  // filter
    ihdr.push(0);  // interlace
    write_chunk(&mut out, b"IHDR", &ihdr);
    write_chunk(&mut out, b"IDAT", &idat);
    write_chunk(&mut out, b"IEND", &[]);
    // CRC runs over every IDAT byte, so it scales with the image, not
    // with the chunk count — worth its own number.
    log_ms("chunks + crc32", now_ms() - t_deflate);
    out
}

fn write_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc = Crc::new();
    crc.update(kind);
    crc.update(data);
    out.extend_from_slice(&crc.finalize().to_be_bytes());
}

// CRC-32 (IEEE, PNG), table-driven. The chunk count is small but the
// IDAT chunk is the whole image, and the bitwise form costs 8 iterations
// per byte. The table is built at compile time, so it costs 1 KB of
// module data and no startup work.
const CRC_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0usize;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
};

struct Crc { v: u32 }
impl Crc {
    fn new() -> Self { Crc { v: 0xFFFF_FFFF } }
    fn update(&mut self, data: &[u8]) {
        for &byte in data {
            let idx = ((self.v ^ byte as u32) & 0xFF) as usize;
            self.v = CRC_TABLE[idx] ^ (self.v >> 8);
        }
    }
    fn finalize(self) -> u32 { self.v ^ 0xFFFF_FFFF }
}

// Keep IconRef referenced (used via the build.rs-generated AppMeta blob).
#[allow(dead_code)]
fn _keep_iconref_alive() -> Option<IconRef> { None }
