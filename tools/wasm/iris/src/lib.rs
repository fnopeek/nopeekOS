//! iris — minimal image viewer for nopeekOS, in loft's visual language.
//!
//! Layout (top → bottom):
//!   toolbar — icon · file name · spacer · "i/m · W×H"
//!   body    — Widget::Canvas (the decoded image, contain-fit)
//!   footer  — full npkFS path
//!
//! Navigation: left-click = next image, right-click = previous (also
//! ←/→ arrow keys). The image set is every `.png` in the file's folder,
//! sorted by name. Launched with a file argument (loft double-click) or
//! routed an `Event::Open` when already running (singleton).
//!
//! Display uses the P10.10 canvas escape hatch: iris decodes PNG → BGRA
//! in WASM and uploads it with `npk_canvas_commit`; the compositor blits
//! it contain-fit into the `Widget::Canvas` rect. iris never touches the
//! framebuffer — it only holds the CANVAS capability.

#![no_std]

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use nopeek_widgets::app_meta::IconRef;
use nopeek_widgets::*;

#[unsafe(link_section = ".npk.app_meta")]
#[used]
static APP_META_BYTES: [u8; include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin")).len()]
    = *include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin"));

// Read images + render + upload pixels. No WRITE, no EXEC.
#[unsafe(link_section = ".npk.caps")]
#[used]
static NPK_CAPS: [u8; 1] = [caps::READ | caps::CANVAS | caps::RENDER];

unsafe extern "C" {
    fn npk_scene_commit(ptr: i32, len: i32) -> i32;
    fn npk_event_poll(ptr: i32, max: i32) -> i32;
    fn npk_fetch(name_ptr: i32, name_len: i32, buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_fs_list(prefix_ptr: i32, prefix_len: i32, out_ptr: i32, out_cap: i32, recursive: i32) -> i32;
    fn npk_home_dir(buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_launch_arg(buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_canvas_commit(canvas_id: i32, ptr: i32, len: i32, w: i32, h: i32) -> i32;
    fn npk_close_widget() -> i32;
    fn npk_log_serial(ptr: i32, len: i32);
    fn npk_sleep(ms: i32) -> i32;
}

fn log(msg: &str) {
    unsafe { npk_log_serial(msg.as_ptr() as i32, msg.len() as i32); }
}

fn commit(bytes: &[u8]) -> i32 {
    unsafe { npk_scene_commit(bytes.as_ptr() as i32, bytes.len() as i32) }
}

fn close_self() { unsafe { let _ = npk_close_widget(); } }

const CANVAS_ID: i32 = 1;

// ── Buffers ───────────────────────────────────────────────────────────
const EVENT_BUF_SIZE: usize = 8 * 1024;
static mut EVENT_BUF: [u8; EVENT_BUF_SIZE] = [0; EVENT_BUF_SIZE];

// Compressed PNG file scratch (a screen-sized PNG is a few MB).
const FETCH_BUF_SIZE: usize = 32 * 1024 * 1024;
static mut FETCH_BUF: [u8; FETCH_BUF_SIZE] = [0; FETCH_BUF_SIZE];

const LIST_BUF_SIZE: usize = 64 * 1024;
static mut LIST_BUF: [u8; LIST_BUF_SIZE] = [0; LIST_BUF_SIZE];

const HOME_CAP: usize = 256;
static mut HOME_BUF: [u8; HOME_CAP] = [0; HOME_CAP];

// Event payloads (Open path) live on the bump heap above the persistent
// mark; alloc_reset before handling frees them, so copy into this static
// first (use-after-free lesson, see spell).
const PAYLOAD_CAP: usize = 4 * 1024;
static mut PAYLOAD_BUF: [u8; PAYLOAD_CAP] = [0; PAYLOAD_CAP];

fn copy_payload(s: &str) -> usize {
    let n = s.len().min(PAYLOAD_CAP);
    let dst = core::ptr::addr_of_mut!(PAYLOAD_BUF) as *mut u8;
    unsafe { core::ptr::copy_nonoverlapping(s.as_ptr(), dst, n); }
    n
}

fn payload_str(len: usize) -> &'static str {
    let ptr = core::ptr::addr_of!(PAYLOAD_BUF) as *const u8;
    let slice = unsafe { core::slice::from_raw_parts(ptr, len) };
    core::str::from_utf8(slice).unwrap_or("")
}

enum PollResult { Event(Event), Empty, WindowGone }

fn poll_event() -> PollResult {
    let buf_ptr = core::ptr::addr_of_mut!(EVENT_BUF) as *mut u8;
    let n = unsafe { npk_event_poll(buf_ptr as i32, EVENT_BUF_SIZE as i32) };
    if n < 0 { return PollResult::WindowGone; }
    if n == 0 { return PollResult::Empty; }
    let slice = unsafe { core::slice::from_raw_parts(buf_ptr as *const u8, n as usize) };
    match postcard::from_bytes::<Event>(slice) {
        Ok(ev) => PollResult::Event(ev),
        Err(_) => PollResult::Empty,
    }
}

// ── Bump allocator ────────────────────────────────────────────────────
//
// A 4K PNG decode needs ~3× its raw size live at once (decompressed +
// unfiltered + BGRA). 256 MB matches wallpaper's headroom; everything
// above `persistent_mark` (the per-frame scene + the decode buffers) is
// freed each loop iteration so navigation doesn't leak.
const HEAP_SIZE: usize = 256 * 1024 * 1024;
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
fn panic(_: &core::panic::PanicInfo) -> ! { log("[iris] panic!"); loop {} }

fn alloc_mark() -> usize { unsafe { core::ptr::addr_of!(HEAP_POS).read() } }
fn alloc_reset(pos: usize) { unsafe { core::ptr::addr_of_mut!(HEAP_POS).write(pos); } }

// ── State ─────────────────────────────────────────────────────────────
struct Iris {
    dir:    String,        // folder being browsed (npkFS path, no trailing /)
    files:  Vec<String>,   // image file names in `dir`, sorted
    idx:    usize,
    name:   String,        // current file name (basename)
    w:      u32,           // current image dims (0 = none / failed)
    h:      u32,
    failed: bool,          // decode failed for the current file
}

impl Iris {
    fn new() -> Self {
        let mut iris = Iris {
            dir: String::new(), files: Vec::new(), idx: 0,
            name: String::new(), w: 0, h: 0, failed: false,
        };
        // Launched to open a specific file?
        let mut argbuf = [0u8; 512];
        let n = unsafe { npk_launch_arg(argbuf.as_mut_ptr() as i32, argbuf.len() as i32) };
        if n > 0 {
            if let Ok(path) = core::str::from_utf8(&argbuf[..n as usize]) {
                iris.point_at(path);
                return iris;
            }
        }
        // No argument → default to the screenshots folder.
        let home = read_home_dir();
        iris.dir = alloc::format!("{}/pictures/printscreens", home);
        iris.refresh();
        iris
    }

    /// Set the folder + selection from a full file path.
    fn point_at(&mut self, path: &str) {
        let (dir, file) = split_path(path);
        self.dir = dir.to_string();
        self.refresh();
        self.idx = self.files.iter().position(|f| f == file).unwrap_or(0);
    }

    /// Re-list `dir` for image files.
    fn refresh(&mut self) {
        self.files = list_images(&self.dir);
        if self.idx >= self.files.len() { self.idx = 0; }
    }

    fn full_path(&self) -> Option<String> {
        let f = self.files.get(self.idx)?;
        Some(alloc::format!("{}/{}", self.dir, f))
    }

    /// Fetch + decode the current image and upload it to the canvas.
    fn load(&mut self) {
        self.w = 0; self.h = 0; self.failed = false; self.name.clear();
        let path = match self.full_path() { Some(p) => p, None => return };
        if let Some(f) = self.files.get(self.idx) { self.name.push_str(f); }
        let bytes = match fetch_file(&path) {
            Some(b) => b,
            None => { self.failed = true; log("[iris] fetch failed"); return; }
        };
        match decode_png(bytes) {
            Some((bgra, w, h)) => {
                let rc = unsafe {
                    npk_canvas_commit(CANVAS_ID, bgra.as_ptr() as i32,
                        bgra.len() as i32, w as i32, h as i32)
                };
                if rc < 0 { self.failed = true; log("[iris] canvas_commit rejected"); }
                else { self.w = w; self.h = h; }
            }
            None => { self.failed = true; log("[iris] decode failed"); }
        }
    }

    fn next(&mut self) {
        if self.files.len() < 2 { return; }
        self.idx = (self.idx + 1) % self.files.len();
    }
    fn prev(&mut self) {
        if self.files.len() < 2 { return; }
        self.idx = (self.idx + self.files.len() - 1) % self.files.len();
    }
}

enum Outcome { Idle, Render, Reload, Exit }

fn handle(iris: &mut Iris, ev: Event, payload: &str) -> Outcome {
    match ev {
        Event::Key(KeyCode::Escape) => Outcome::Exit,
        Event::Key(KeyCode::Right) | Event::Key(KeyCode::Down) => { iris.next(); Outcome::Reload }
        Event::Key(KeyCode::Left)  | Event::Key(KeyCode::Up)   => { iris.prev(); Outcome::Reload }
        // Left-click = next, right-click = previous.
        Event::MouseButton { button: MouseButton::Left,  down: true, .. } => { iris.next(); Outcome::Reload }
        Event::MouseButton { button: MouseButton::Right, down: true, .. } => { iris.prev(); Outcome::Reload }
        // Another image opened while running (loft / singleton routing).
        Event::Open(_) => { iris.point_at(payload); Outcome::Reload }
        _ => Outcome::Idle,
    }
}

// ── Scene ─────────────────────────────────────────────────────────────
fn render(iris: &Iris) -> Widget {
    let title = if iris.name.is_empty() { "Kein Bild".to_string() } else { iris.name.clone() };
    let meta = if iris.files.is_empty() {
        "—".to_string()
    } else if iris.failed {
        alloc::format!("{}/{} · Format nicht unterstützt", iris.idx + 1, iris.files.len())
    } else {
        alloc::format!("{}/{} · {}×{}", iris.idx + 1, iris.files.len(), iris.w, iris.h)
    };

    let toolbar = Widget::Row {
        spacing: 4,
        align: Align::Center,
        modifiers: alloc::vec![
            Modifier::Background(Token::SurfaceElevated),
            Modifier::Padding(8),
        ],
        children: alloc::vec![
            Widget::Icon { id: IconId::Image, size: 18, modifiers: alloc::vec![Modifier::Tint(Token::Accent)] },
            Widget::Text { content: title, style: TextStyle::Body, modifiers: alloc::vec![Modifier::Padding(6)] },
            Widget::Spacer { flex: 1 },
            Widget::Text { content: meta, style: TextStyle::Muted, modifiers: Vec::new() },
        ],
    };

    let body = Widget::Canvas {
        id: CanvasId(CANVAS_ID as u32),
        width: 320,
        height: 240,
        modifiers: alloc::vec![Modifier::Flex(1)],
    };

    let footer_path = iris.full_path().unwrap_or_default();
    let footer = Widget::Row {
        spacing: 0,
        align: Align::Center,
        modifiers: alloc::vec![Modifier::Padding(6)],
        children: alloc::vec![
            Widget::Text { content: footer_path, style: TextStyle::Muted, modifiers: Vec::new() },
        ],
    };

    Widget::Column {
        spacing: 0,
        align: Align::Stretch,
        modifiers: alloc::vec![Modifier::Background(Token::Surface)],
        children: alloc::vec![toolbar, Widget::Divider, body, Widget::Divider, footer],
    }
}

fn commit_scene(iris: &Iris) {
    let tree = render(iris);
    match wire::encode(&tree) {
        Ok(bytes) => { if commit(&bytes) < 0 { log("[iris] commit failed"); } }
        Err(_) => log("[iris] encode failed"),
    }
}

// ── Entry ─────────────────────────────────────────────────────────────
#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    let mut iris = Iris::new();
    let mut mark = alloc_mark();
    commit_scene(&iris); // creates the window (assigns widget_window_id)
    iris.load();         // window exists now → canvas_commit works
    commit_scene(&iris); // refresh chrome with dims

    loop {
        match poll_event() {
            PollResult::Event(ev) => {
                let plen = match &ev {
                    Event::Open(s) => copy_payload(s),
                    _ => 0,
                };
                alloc_reset(mark);
                let outcome = handle(&mut iris, ev, payload_str(plen));
                mark = alloc_mark(); // small state mutations persist
                match outcome {
                    Outcome::Idle => {}
                    Outcome::Render => commit_scene(&iris),
                    Outcome::Reload => { iris.load(); commit_scene(&iris); }
                    Outcome::Exit => { close_self(); return; }
                }
            }
            PollResult::Empty => { unsafe { let _ = npk_sleep(16); } }
            PollResult::WindowGone => return,
        }
    }
}

// ── npkFS helpers ─────────────────────────────────────────────────────
fn read_home_dir() -> String {
    let buf_ptr = core::ptr::addr_of_mut!(HOME_BUF) as *mut u8;
    let n = unsafe { npk_home_dir(buf_ptr as i32, HOME_CAP as i32) };
    if n <= 0 { return "home".to_string(); }
    let slice = unsafe { core::slice::from_raw_parts(buf_ptr as *const u8, n as usize) };
    core::str::from_utf8(slice).unwrap_or("home").to_string()
}

fn fetch_file(path: &str) -> Option<&'static [u8]> {
    let buf_ptr = core::ptr::addr_of_mut!(FETCH_BUF) as *mut u8;
    let n = unsafe {
        npk_fetch(path.as_ptr() as i32, path.len() as i32, buf_ptr as i32, FETCH_BUF_SIZE as i32)
    };
    if n <= 0 { return None; }
    Some(unsafe { core::slice::from_raw_parts(buf_ptr as *const u8, n as usize) })
}

/// List `.png` files (non-recursive) in `dir`, sorted by name.
fn list_images(dir: &str) -> Vec<String> {
    let buf_ptr = core::ptr::addr_of_mut!(LIST_BUF) as *mut u8;
    let n = unsafe {
        npk_fs_list(dir.as_ptr() as i32, dir.len() as i32, buf_ptr as i32, LIST_BUF_SIZE as i32, 0)
    };
    let mut out: Vec<String> = Vec::new();
    if n <= 0 { return out; }
    let slice = unsafe { core::slice::from_raw_parts(buf_ptr as *const u8, n as usize) };
    for line in slice.split(|&b| b == b'\n') {
        let nul = match line.iter().position(|&b| b == 0) { Some(p) => p, None => continue };
        let name = match core::str::from_utf8(&line[..nul]) { Ok(s) => s, Err(_) => continue };
        let rest = &line[nul + 1..];
        if rest.len() >= 10 && rest[9] != 0 { continue; } // skip directories
        if is_image(name) { out.push(name.to_string()); }
    }
    out.sort();
    out
}

fn is_image(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".png")
}

/// Split "a/b/c.png" → ("a/b", "c.png"). No slash → ("", name).
fn split_path(path: &str) -> (&str, &str) {
    match path.rfind('/') {
        Some(i) => (&path[..i], &path[i + 1..]),
        None => ("", path),
    }
}

// ── PNG decoder (ported from wallpaper) ───────────────────────────────
// 8-bit RGB/RGBA, non-interlaced. Returns (BGRA, width, height).
fn decode_png(data: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
    if data.len() < 8 || &data[0..8] != b"\x89PNG\r\n\x1a\n" { return None; }

    let mut pos = 8;
    let mut width: u32 = 0;
    let mut height: u32 = 0;
    let mut bit_depth: u8 = 0;
    let mut color_type: u8 = 0;
    let mut idat_data: Vec<u8> = Vec::with_capacity(data.len());

    while pos + 12 <= data.len() {
        let chunk_len = u32::from_be_bytes([data[pos], data[pos+1], data[pos+2], data[pos+3]]) as usize;
        let chunk_type = &data[pos+4..pos+8];
        let start = pos + 8;
        let end = start + chunk_len;
        if end > data.len() { break; }
        match chunk_type {
            b"IHDR" => {
                if chunk_len < 13 { return None; }
                let d = &data[start..];
                width  = u32::from_be_bytes([d[0], d[1], d[2], d[3]]);
                height = u32::from_be_bytes([d[4], d[5], d[6], d[7]]);
                bit_depth  = d[8];
                color_type = d[9];
                if d[10] != 0 || d[11] != 0 || d[12] != 0 { return None; } // compression/filter/interlace
                if bit_depth != 8 { return None; }
                if color_type != 2 && color_type != 6 { return None; }
            }
            b"IDAT" => idat_data.extend_from_slice(&data[start..end]),
            b"IEND" => break,
            _ => {}
        }
        pos = end + 4; // +4 CRC
    }

    if width == 0 || height == 0 || idat_data.is_empty() { return None; }

    let channels: usize = match color_type { 2 => 3, 6 => 4, _ => return None };
    let stride = width as usize * channels;

    if idat_data.len() < 6 { return None; }
    let decompressed = match miniz_oxide::inflate::decompress_to_vec_zlib(&idat_data) {
        Ok(d) => d,
        Err(_) => match miniz_oxide::inflate::decompress_to_vec(&idat_data[2..]) {
            Ok(d) => d,
            Err(_) => return None,
        },
    };

    let expected = height as usize * (1 + stride);
    if decompressed.len() < expected { return None; }

    let mut unfiltered = alloc::vec![0u8; height as usize * stride];
    for y in 0..height as usize {
        let src_offset = y * (1 + stride);
        let filter_type = decompressed[src_offset];
        let row_start = src_offset + 1;
        let dst_offset = y * stride;
        for x in 0..stride {
            let raw = decompressed[row_start + x];
            let a = if x >= channels { unfiltered[dst_offset + x - channels] } else { 0 };
            let b = if y > 0 { unfiltered[dst_offset - stride + x] } else { 0 };
            let c = if x >= channels && y > 0 { unfiltered[dst_offset - stride + x - channels] } else { 0 };
            let val = match filter_type {
                0 => raw,
                1 => raw.wrapping_add(a),
                2 => raw.wrapping_add(b),
                3 => raw.wrapping_add(((a as u16 + b as u16) / 2) as u8),
                4 => raw.wrapping_add(paeth(a, b, c)),
                _ => raw,
            };
            unfiltered[dst_offset + x] = val;
        }
    }

    let pixel_count = (width * height) as usize;
    let mut bgra = alloc::vec![0u8; pixel_count * 4];
    for i in 0..pixel_count {
        let src = i * channels;
        let dst = i * 4;
        bgra[dst]     = unfiltered[src + 2];
        bgra[dst + 1] = unfiltered[src + 1];
        bgra[dst + 2] = unfiltered[src];
        bgra[dst + 3] = if channels == 4 { unfiltered[src + 3] } else { 255 };
    }
    Some((bgra, width, height))
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = a as i16 + b as i16 - c as i16;
    let pa = (p - a as i16).unsigned_abs();
    let pb = (p - b as i16).unsigned_abs();
    let pc = (p - c as i16).unsigned_abs();
    if pa <= pb && pa <= pc { a } else if pb <= pc { b } else { c }
}

// Keep IconRef referenced (used via the build.rs-generated AppMeta blob).
#[allow(dead_code)]
fn _keep_iconref_alive() -> Option<IconRef> { None }
