//! snap — screenshot tool for nopeekOS.
//!
//! One-shot, no window (full-screen mode). Launched by the bar's camera
//! button via `npk_launch("snap", "full" | "region")`:
//!   - "full"   → capture the whole composited screen, save as PNG.
//!   - "region" → (slice ③) freeze + rubber-band select; for now falls
//!                back to a full capture.
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
// No RENDER/CANVAS yet — full-screen mode shows no window. Region mode
// (slice ③) will add CANVAS|RENDER for the frozen overlay.
#[unsafe(link_section = ".npk.caps")]
#[used]
static NPK_CAPS: [u8; 1] = [nopeek_widgets::caps::READ | nopeek_widgets::caps::WRITE | nopeek_widgets::caps::CAPTURE];

// Host functions are WASM imports from the `env` module, resolved by the
// kernel at instantiation. Naming the module explicitly is what makes them
// imports rather than ordinary undefined C symbols, which rust-lld rejects.
#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn npk_launch_arg(buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_screen_size() -> i32;
    fn npk_capture_screen(buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_store(name_ptr: i32, name_len: i32, data_ptr: i32, data_len: i32) -> i32;
    fn npk_fs_list(prefix_ptr: i32, prefix_len: i32, out_ptr: i32, out_cap: i32, recursive: i32) -> i32;
    fn npk_home_dir(buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_log_serial(ptr: i32, len: i32);
}

fn log(msg: &str) {
    unsafe { npk_log_serial(msg.as_ptr() as i32, msg.len() as i32); }
}

// ── Buffers ───────────────────────────────────────────────────────────
const LIST_BUF_SIZE: usize = 64 * 1024;
static mut LIST_BUF: [u8; LIST_BUF_SIZE] = [0; LIST_BUF_SIZE];

const HOME_CAP: usize = 256;
static mut HOME_BUF: [u8; HOME_CAP] = [0; HOME_CAP];

// ── Bump allocator (192 MB — a 4K capture is 33 MB BGRA + ~25 MB RGB
//    scanlines + the deflate output, all live at encode time). ──
const HEAP_SIZE: usize = 192 * 1024 * 1024;
static mut HEAP: [u8; HEAP_SIZE] = [0; HEAP_SIZE];
static mut HEAP_POS: usize = 0;

struct BumpAllocator;
unsafe impl core::alloc::GlobalAlloc for BumpAllocator {
    unsafe fn alloc(&self, layout: core::alloc::Layout) -> *mut u8 {
        let pos = core::ptr::addr_of!(HEAP_POS).read();
        let align = layout.align();
        let aligned = (pos + align - 1) & !(align - 1);
        let new_pos = aligned + layout.size();
        if new_pos > HEAP_SIZE { return core::ptr::null_mut(); }
        core::ptr::addr_of_mut!(HEAP_POS).write(new_pos);
        core::ptr::addr_of_mut!(HEAP).cast::<u8>().add(aligned)
    }
    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: core::alloc::Layout) {}
}
#[global_allocator]
static ALLOCATOR: BumpAllocator = BumpAllocator;

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { log("[snap] panic!"); loop {} }

// ── Entry ─────────────────────────────────────────────────────────────
#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    // Mode (unused for now — region falls back to full until slice ③).
    let mut argbuf = [0u8; 32];
    let n = unsafe { npk_launch_arg(argbuf.as_mut_ptr() as i32, argbuf.len() as i32) };
    let _mode = if n > 0 { core::str::from_utf8(&argbuf[..n as usize]).unwrap_or("full") } else { "full" };

    // Screen size: packed (w << 16) | h.
    let packed = unsafe { npk_screen_size() };
    if packed <= 0 { log("[snap] screen_size failed"); return; }
    let w = ((packed as u32) >> 16) as usize;
    let h = ((packed as u32) & 0xFFFF) as usize;
    if w == 0 || h == 0 { log("[snap] zero screen"); return; }

    // Capture the composited screen as BGRA.
    let need = w * h * 4;
    let mut bgra: Vec<u8> = alloc::vec![0u8; need];
    let got = unsafe { npk_capture_screen(bgra.as_mut_ptr() as i32, need as i32) };
    if got as usize != need { log("[snap] capture failed"); return; }

    // Encode PNG (RGB, screenshots have no meaningful alpha).
    let png = encode_png_rgb(&bgra, w as u32, h as u32);

    // Save under the printscreens folder (created on first write).
    let home = read_home_dir();
    let dir = alloc::format!("{}/pictures/printscreens", home);
    let name = next_name(&dir);
    let path = alloc::format!("{}/{}", dir, name);
    let rc = unsafe {
        npk_store(path.as_ptr() as i32, path.len() as i32, png.as_ptr() as i32, png.len() as i32)
    };
    if rc < 0 { log("[snap] save failed"); } else { log("[snap] saved screenshot"); }
}

// ── npkFS helpers ─────────────────────────────────────────────────────
fn read_home_dir() -> String {
    let buf_ptr = core::ptr::addr_of_mut!(HOME_BUF) as *mut u8;
    let n = unsafe { npk_home_dir(buf_ptr as i32, HOME_CAP as i32) };
    if n <= 0 { return "home".to_string(); }
    let slice = unsafe { core::slice::from_raw_parts(buf_ptr as *const u8, n as usize) };
    core::str::from_utf8(slice).unwrap_or("home").to_string()
}

/// Next free `screenshot-NNN.png` in `dir` (clock-free naming — we have
/// no time host fn, so number sequentially from the existing files).
fn next_name(dir: &str) -> String {
    let buf_ptr = core::ptr::addr_of_mut!(LIST_BUF) as *mut u8;
    let n = unsafe {
        npk_fs_list(dir.as_ptr() as i32, dir.len() as i32, buf_ptr as i32, LIST_BUF_SIZE as i32, 0)
    };
    let mut max: u32 = 0;
    if n > 0 {
        let slice = unsafe { core::slice::from_raw_parts(buf_ptr as *const u8, n as usize) };
        for line in slice.split(|&b| b == b'\n') {
            let nul = match line.iter().position(|&b| b == 0) { Some(p) => p, None => continue };
            let name = match core::str::from_utf8(&line[..nul]) { Ok(s) => s, Err(_) => continue };
            if let Some(num) = name.strip_prefix("screenshot-").and_then(|s| s.strip_suffix(".png")) {
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
    // Raw scanlines: 1 filter byte (0 = None) + W*3 RGB bytes per row.
    let mut raw: Vec<u8> = Vec::with_capacity(hc * (1 + wc * 3));
    for y in 0..hc {
        raw.push(0);
        let row = y * wc * 4;
        for x in 0..wc {
            let p = row + x * 4;
            // source is BGRA → RGB
            raw.push(bgra[p + 2]);
            raw.push(bgra[p + 1]);
            raw.push(bgra[p]);
        }
    }
    // zlib-wrapped deflate (header + deflate + adler32).
    let idat = miniz_oxide::deflate::compress_to_vec_zlib(&raw, 6);

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

// CRC-32 (IEEE, PNG) — bitwise, no table (a few chunks per image).
struct Crc { v: u32 }
impl Crc {
    fn new() -> Self { Crc { v: 0xFFFF_FFFF } }
    fn update(&mut self, data: &[u8]) {
        for &byte in data {
            self.v ^= byte as u32;
            for _ in 0..8 {
                let mask = (self.v & 1).wrapping_neg();
                self.v = (self.v >> 1) ^ (0xEDB8_8320 & mask);
            }
        }
    }
    fn finalize(self) -> u32 { self.v ^ 0xFFFF_FFFF }
}

// Keep IconRef referenced (used via the build.rs-generated AppMeta blob).
#[allow(dead_code)]
fn _keep_iconref_alive() -> Option<IconRef> { None }
