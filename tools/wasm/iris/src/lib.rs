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

use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use nopeek_widgets::app_meta::IconRef;
use nopeek_widgets::i18n;
use nopeek_widgets::prefab;
use nopeek_widgets::style::Spacing;
use nopeek_widgets::*;

#[unsafe(link_section = ".npk.app_meta")]
#[used]
static APP_META_BYTES: [u8; include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin")).len()]
    = *include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin"));

// Read images + render + upload pixels. No WRITE, no EXEC.
#[unsafe(link_section = ".npk.caps")]
#[used]
static NPK_CAPS: [u8; 1] = [caps::READ | caps::CANVAS | caps::RENDER];

// Host functions are WASM imports from the `env` module, resolved by the
// kernel at instantiation. Naming the module explicitly is what makes them
// imports rather than ordinary undefined C symbols, which rust-lld rejects.
#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn npk_scene_commit(ptr: i32, len: i32) -> i32;
    fn npk_event_poll(ptr: i32, max: i32) -> i32;
    fn npk_fetch(name_ptr: i32, name_len: i32, buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_fs_list(prefix_ptr: i32, prefix_len: i32, out_ptr: i32, out_cap: i32, recursive: i32) -> i32;
    fn npk_home_dir(buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_launch_arg(buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_canvas_commit(canvas_id: i32, ptr: i32, len: i32, w: i32, h: i32) -> i32;
    fn npk_canvas_rect(canvas_id: i32, out_ptr: i32) -> i32;
    fn npk_cursor_pos() -> i32;
    fn npk_close_widget() -> i32;
    fn npk_ticks() -> i64;
    fn npk_log_serial(ptr: i32, len: i32);
    fn npk_sleep(ms: i32) -> i32;
}

fn log(msg: &str) {
    unsafe { npk_log_serial(msg.as_ptr() as i32, msg.len() as i32); }
}

fn now_ms() -> i64 { unsafe { npk_ticks() } }

/// Log "<label>: <ms> ms". Permanent, not scaffolding: the decode runs
/// under a WASM interpreter, so knowing whether the seconds go into
/// inflate, un-filtering or the colour swap is the difference between
/// fixing the slow thing and rewriting the fast one. 10 ms resolution.
fn log_ms(label: &str, ms: i64) {
    let mut b = String::new();
    b.push_str("[iris] ");
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
const HEAP_SIZE: usize = 160 * 1024 * 1024;
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

// ── Decoded-bitmap cache ──────────────────────────────────────────────
//
// Decoding is the whole cost of showing an image (~1.3 s for a 1080p
// PNG under the interpreter), so a picture already decoded must never be
// decoded twice. The cache lives OUTSIDE the bump heap — `alloc_reset`
// would otherwise pull it out from under us every event.
//
// **Budgeted in bytes, not in pictures.** A count that fits 1080p (8 MB
// each) buys 300 MB at 4K (33 MB each); the same mistake a per-stylesheet
// cap made in beak. With a byte budget the depth adapts by itself.
//
// **And grown on demand, not reserved.** A `static` array would be part
// of the module's linear memory and therefore allocated and zeroed at
// launch — half a gigabyte for a folder holding three pictures. Instead
// the arena is claimed with `memory.grow` as pictures actually arrive,
// and only up to what this folder can use: nine images at the size the
// first decode turned out to be. Three small pictures cost a few dozen
// megabytes and never more.
//
// The growth is contiguous because we are the only caller of
// `memory.grow` in this module — the bump allocator hands out slices of
// a fixed array and never asks the runtime for more.
//
// The arena is a ring: allocations run forward, wrap when the tail is
// too short, and evict whatever they land on. In sequential browsing
// that means the pictures furthest behind you go first, which is exactly
// the right eviction order and costs no compaction.
const CACHE_MAX: usize = 320 * 1024 * 1024;
const WASM_PAGE: usize = 64 * 1024;

static mut CACHE_PTR: *mut u8 = core::ptr::null_mut();
static mut CACHE_CAP: usize = 0;
static mut CACHE_POS: usize = 0;

/// Claim linear memory until the arena holds at least `want` bytes (never
/// past `CACHE_MAX`). Returns the capacity actually available.
fn arena_reserve(want: usize) -> usize {
    unsafe {
        let cap = *(&raw const CACHE_CAP);
        let want = want.min(CACHE_MAX);
        if want <= cap { return cap; }
        let pages = (want - cap).div_ceil(WASM_PAGE);
        let prev = core::arch::wasm32::memory_grow(0, pages);
        if prev == usize::MAX { return cap; }   // runtime refused; keep what we have
        let fresh = (prev * WASM_PAGE) as *mut u8;
        if cap == 0 {
            *(&raw mut CACHE_PTR) = fresh;
        } else if fresh != (*(&raw const CACHE_PTR)).add(cap) {
            // Someone else claimed memory in between, so the arena is no
            // longer one run. Nothing else in this module does that today;
            // if it ever starts to, drop what we hold and re-base rather
            // than address across the gap.
            cache_clear();
            *(&raw mut CACHE_PTR) = fresh;
            *(&raw mut CACHE_CAP) = pages * WASM_PAGE;
            return pages * WASM_PAGE;
        }
        let grown = cap + pages * WASM_PAGE;
        *(&raw mut CACHE_CAP) = grown;
        grown
    }
}

#[derive(Clone, Copy)]
struct CacheEntry { idx: usize, off: usize, len: usize, w: u32, h: u32 }

const CACHE_SLOTS: usize = 48;
static mut ENTRIES: [Option<CacheEntry>; CACHE_SLOTS] = [None; CACHE_SLOTS];

/// Pictures that did not make it into the cache — too big for the budget,
/// or undecodable. Without this list prefetching would pick the same
/// target every round and re-decode it forever: a hundred percent of a
/// core, spent on an image that can never land.
const SKIP_SLOTS: usize = 8;
static mut SKIP: [Option<usize>; SKIP_SLOTS] = [None; SKIP_SLOTS];
static mut SKIP_POS: usize = 0;

fn skip_mark(idx: usize) {
    unsafe {
        let pos = *(&raw const SKIP_POS);
        (&mut *(&raw mut SKIP))[pos] = Some(idx);
        *(&raw mut SKIP_POS) = (pos + 1) % SKIP_SLOTS;
    }
}

fn skip_holds(idx: usize) -> bool {
    unsafe { (&*(&raw const SKIP)).iter().flatten().any(|&i| i == idx) }
}

/// Everything cached is dropped when the folder changes — the entries are
/// keyed by position in `files`, and that meaning changes with the list.
fn cache_clear() {
    unsafe {
        *(&raw mut CACHE_POS) = 0;
        for e in (&mut *(&raw mut ENTRIES)).iter_mut() { *e = None; }
        for s in (&mut *(&raw mut SKIP)).iter_mut() { *s = None; }
        *(&raw mut SKIP_POS) = 0;
    }
}

fn cache_get(idx: usize) -> Option<(&'static [u8], u32, u32)> {
    unsafe {
        let base = *(&raw const CACHE_PTR);
        if base.is_null() { return None; }
        let entries = &*(&raw const ENTRIES);
        let e = entries.iter().flatten().find(|e| e.idx == idx)?;
        Some((core::slice::from_raw_parts(base.add(e.off), e.len), e.w, e.h))
    }
}

/// Store a decoded bitmap, growing the arena if this folder can use the
/// room. `wanted` is how many pictures are worth holding — the prefetch
/// window, capped by how many the folder actually has, so a two-picture
/// folder never claims space for nine.
///
/// Skips pictures too big to share the arena: one of those would evict
/// everything else on every navigation.
fn cache_put(idx: usize, bgra: &[u8], w: u32, h: u32, wanted: usize) {
    let len = bgra.len();
    if len == 0 || len > CACHE_MAX / 2 { return; }
    let cap = arena_reserve(len.saturating_mul(wanted.max(1)));
    if cap < len { return; }
    unsafe {
        let mut start = *(&raw const CACHE_POS);
        if start + len > cap { start = 0; }   // wrap; tail is wasted
        let end = start + len;
        let entries = &mut *(&raw mut ENTRIES);
        // Anything the new bytes land on is gone.
        for slot in entries.iter_mut() {
            if let Some(e) = slot {
                if e.off < end && start < e.off + e.len { *slot = None; }
            }
        }
        let dst = *(&raw const CACHE_PTR);
        core::ptr::copy_nonoverlapping(bgra.as_ptr(), dst.add(start), len);
        let entry = CacheEntry { idx, off: start, len, w, h };
        // Prefer a free slot; if the table is full the oldest region is
        // the one nearest the write head, so drop the first entry.
        match entries.iter_mut().find(|s| s.is_none()) {
            Some(slot) => *slot = Some(entry),
            None => entries[0] = Some(entry),
        }
        *(&raw mut CACHE_POS) = end;
    }
}

// ── Strings ───────────────────────────────────────────────────────────
struct Strings {
    menu_file:   &'static str,
    menu_view:   &'static str,
    menu_help:   &'static str,
    open:        &'static str,
    close:       &'static str,
    next:        &'static str,
    previous:    &'static str,
    zoom_in:     &'static str,
    zoom_out:    &'static str,
    zoom_fit:    &'static str,
    about:       &'static str,
    no_images:   &'static str,
    loading:     &'static str,
    no_image:    &'static str,
    unsupported: &'static str,
}

const EN: Strings = Strings {
    menu_file: "File", menu_view: "View", menu_help: "Help",
    open: "Open…", close: "Close",
    next: "Next", previous: "Previous",
    zoom_in: "Zoom in", zoom_out: "Zoom out", zoom_fit: "Fit to window",
    about: "About Iris",
    no_images: "No images in this folder",
    loading: "Loading…",
    no_image: "No image",
    unsupported: "Unsupported format",
};

const DE: Strings = Strings {
    menu_file: "Datei", menu_view: "Ansicht", menu_help: "Hilfe",
    open: "Öffnen…", close: "Schließen",
    next: "Weiter", previous: "Zurück",
    zoom_in: "Vergrößern", zoom_out: "Verkleinern", zoom_fit: "Einpassen",
    about: "Über Iris",
    no_images: "Keine Bilder in diesem Ordner",
    loading: "Lädt…",
    no_image: "Kein Bild",
    unsupported: "Format nicht unterstützt",
};

fn s() -> &'static Strings {
    match i18n::lang() { i18n::Lang::De => &DE, _ => &EN }
}

// ── Actions / anchors ─────────────────────────────────────────────────
const ACT_MENU_FILE:    u32 = 1;
const ACT_MENU_VIEW:    u32 = 2;
const ACT_MENU_HELP:    u32 = 3;
const ACT_MENU_DISMISS: u32 = 4;
const ACT_FILE_OPEN:    u32 = 5;
const ACT_FILE_CLOSE:   u32 = 6;
const ACT_NEXT:         u32 = 7;
const ACT_PREV:         u32 = 8;
const ACT_HELP_ABOUT:   u32 = 9;
const ACT_ZOOM_IN:      u32 = 10;
const ACT_ZOOM_OUT:     u32 = 11;
const ACT_ZOOM_FIT:     u32 = 12;
/// Picking a file from the Open list: base + index.
const ACT_OPEN_FILE_BASE: u32 = 1000;

const NODE_MENU_FILE: u32 = 100;
const NODE_MENU_VIEW: u32 = 101;
const NODE_MENU_HELP: u32 = 102;

#[derive(Clone, Copy, PartialEq)]
enum OpenMenu { File, View, Help }

// ── State ─────────────────────────────────────────────────────────────
struct Iris {
    dir:    String,        // folder being browsed (npkFS path, no trailing /)
    files:  Vec<String>,   // image file names in `dir`, sorted
    idx:    usize,
    w:      u32,           // current image dims (0 = none / failed)
    h:      u32,
    failed: bool,          // decode failed for the current file
    // Chrome state. Copy-only, so it may be mutated after the alloc mark.
    menu:   Option<OpenMenu>,
    picking: bool,         // File → Open… list is showing
    /// Zoom as Q8.8 relative to contain-fit: 256 = the whole image in the
    /// window. The compositor scales the bitmap it already holds, so a
    /// zoom step costs no decode and no upload.
    zoom:   u16,
    /// Pan in px from centred. Clamped by the compositor to the overhang.
    pan:    (i32, i32),
    /// Where the primary button went down, while it is still down.
    press:  Option<(i32, i32)>,
    /// The pointer moved far enough since the press to call it a drag.
    dragged: bool,
    /// Last navigation direction — prefetch follows it first.
    forward: bool,
    /// A decode is about to run. Committed as a scene BEFORE the decode
    /// starts, so the footer says what is happening during the seconds
    /// the picture takes — the window is otherwise silent.
    loading: bool,
    /// A widget consumed this click, so the raw press that follows it is
    /// not ours. The compositor sends BOTH for one physical click: first
    /// `Action(id)` for the button/menu that was hit, then the raw
    /// `MouseButton` for position-sensitive apps. Without this the
    /// toolbar's ◀ would page back on the Action and forward again on the
    /// release, and a menu would close in the same click that opened it.
    swallow_press: bool,
}

const ZOOM_FIT: u16 = 256;
const ZOOM_MIN: u16 = 64;      // 25 %
const ZOOM_MAX: u16 = 4096;    // 16×
/// One wheel notch — a bit under 1.25×, so a few clicks feel linear.
const ZOOM_STEP_NUM: u32 = 5;
const ZOOM_STEP_DEN: u32 = 4;
/// Movement (px, Manhattan) below which a press+release is still a click
/// and not a drag. Generous enough that a shaky hand still pages.
const DRAG_SLOP: i32 = 4;

/// How far ahead and behind to decode. The byte budget decides how many
/// of these actually fit — at 4K only the nearest one or two will.
const PREFETCH_DEPTH: usize = 4;
/// Empty polls (16 ms each) before prefetching starts. Decoding is a
/// solid second of CPU, so it must never race the user: while clicks are
/// still arriving, answering them wins.
const QUIET_POLLS: u32 = 20;

fn zoom_in(z: u16) -> u16 {
    ((z as u32 * ZOOM_STEP_NUM / ZOOM_STEP_DEN) as u16).min(ZOOM_MAX)
}
fn zoom_out(z: u16) -> u16 {
    ((z as u32 * ZOOM_STEP_DEN / ZOOM_STEP_NUM) as u16).max(ZOOM_MIN)
}

impl Iris {
    fn new() -> Self {
        let mut iris = Iris {
            dir: String::new(), files: Vec::new(), idx: 0,
            w: 0, h: 0, failed: false,
            menu: None, picking: false, zoom: ZOOM_FIT,
            pan: (0, 0), press: None, dragged: false, forward: true, loading: false,
            swallow_press: false,
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
        // Entries are keyed by position in `files`; a new listing gives
        // those positions a different meaning.
        cache_clear();
        self.files = list_images(&self.dir);
        if self.idx >= self.files.len() { self.idx = 0; }
    }

    fn full_path(&self) -> Option<String> {
        let f = self.files.get(self.idx)?;
        Some(alloc::format!("{}/{}", self.dir, f))
    }

    /// Fetch + decode the current image and upload it to the canvas.
    /// Mutates only Copy fields (w/h/failed) + the kernel canvas store —
    /// nothing heap-persistent — so it's safe to run after the per-frame
    /// alloc mark (its big decode buffers are transient, freed next reset).
    /// Show the current image. A cache hit is the whole point: the only
    /// work left is handing the bitmap to the compositor.
    fn load(&mut self) {
        self.w = 0; self.h = 0; self.failed = false;
        if let Some((px, w, h)) = cache_get(self.idx) {
            let t = now_ms();
            let rc = unsafe {
                npk_canvas_commit(CANVAS_ID, px.as_ptr() as i32, px.len() as i32,
                    w as i32, h as i32)
            };
            log_ms("=== cached -> displayed", now_ms() - t);
            if rc < 0 { self.failed = true; } else { self.w = w; self.h = h; }
            return;
        }
        self.decode_into_view();
    }

    /// Decode `idx` off disk, show it, and keep it for next time.
    fn decode_into_view(&mut self) {
        let path = match self.full_path() { Some(p) => p, None => return };
        let t_start = now_ms();
        let bytes = match fetch_file(&path) {
            Some(b) => b,
            None => { self.failed = true; log("[iris] fetch failed"); return; }
        };
        let t_fetched = now_ms();
        log_ms("fetch", t_fetched - t_start);
        // Nothing is shown until the picture is whole. Handing over each
        // band as it landed did work — first pixels after a tenth of the
        // time — but watching an image wipe in over two seconds reads
        // worse than a moment of quiet, and while browsing it replaced a
        // finished picture with a half-black one. The decoder stays
        // resumable (it costs nothing and is checked bit-identical); it
        // simply keeps its intermediate states to itself.
        match decode_png(bytes) {
            Some((bgra, w, h)) => {
                let t_decoded = now_ms();
                let rc = unsafe {
                    npk_canvas_commit(CANVAS_ID, bgra.as_ptr() as i32,
                        bgra.len() as i32, w as i32, h as i32)
                };
                log_ms("canvas commit", now_ms() - t_decoded);
                // The number that matters: click → pixels on screen.
                log_ms("=== open -> displayed", now_ms() - t_start);
                if rc < 0 { self.failed = true; log("[iris] canvas_commit rejected"); }
                else {
                    self.w = w; self.h = h;
                    cache_put(self.idx, &bgra, w, h, self.cache_slots());
                }
            }
            None => { self.failed = true; log("[iris] decode failed"); }
        }
    }

    /// How many pictures are worth holding: the prefetch window in both
    /// directions plus the current one, but never more than the folder
    /// has. Three pictures in a folder must not claim room for nine.
    fn cache_slots(&self) -> usize {
        (2 * PREFETCH_DEPTH + 1).min(self.files.len().max(1))
    }

    /// The next neighbour worth decoding ahead, nearest first and in the
    /// direction of travel — so a fast click in the way you were already
    /// going is the case that is covered first.
    fn prefetch_target(&self) -> Option<usize> {
        let n = self.files.len();
        if n < 2 { return None; }
        let ahead = self.forward;
        for d in 1..=PREFETCH_DEPTH {
            for first in [ahead, !ahead] {
                let idx = if first {
                    (self.idx + d) % n
                } else {
                    (self.idx + n - (d % n)) % n
                };
                if idx == self.idx || skip_holds(idx) { continue; }
                if cache_get(idx).is_none() { return Some(idx); }
            }
        }
        None
    }

    /// Decode ONE neighbour into the cache. Never touches the view, so a
    /// half-finished round of prefetching leaves nothing behind.
    fn prefetch_one(&mut self, idx: usize) {
        let Some(name) = self.files.get(idx) else { skip_mark(idx); return };
        let path = alloc::format!("{}/{}", self.dir, name);
        let Some(bytes) = fetch_file(&path) else { skip_mark(idx); return };
        let t = now_ms();
        if let Some((bgra, w, h)) = decode_png(bytes) {
            cache_put(idx, &bgra, w, h, self.cache_slots());
            log_ms("prefetch", now_ms() - t);
        }
        // Did not land (undecodable, or larger than the budget allows) —
        // remember that, or we would try it again every round forever.
        if cache_get(idx).is_none() { skip_mark(idx); }
    }

    // A new image always starts fit to the window — carrying a 4× zoom
    // into the next picture leaves you staring at somebody's corner.
    fn next(&mut self) {
        if self.files.len() < 2 { return; }
        self.idx = (self.idx + 1) % self.files.len();
        self.forward = true;
        self.reset_view();
    }
    fn prev(&mut self) {
        if self.files.len() < 2 { return; }
        self.idx = (self.idx + self.files.len() - 1) % self.files.len();
        self.forward = false;
        self.reset_view();
    }

    fn reset_view(&mut self) {
        self.zoom = ZOOM_FIT;
        self.pan = (0, 0);
    }

    /// Zooming keeps the pan; the compositor re-clamps it to the new
    /// overhang, so zooming out simply pulls the image back to centre.
    fn set_zoom(&mut self, z: u16) {
        self.zoom = z;
        if z == ZOOM_FIT { self.pan = (0, 0); }
    }

    /// Zoom anchored on the pointer: whatever pixel sits under the cursor
    /// stays under it. Without this, zooming in on a detail means zooming
    /// to the middle and then dragging the detail back — twice the work
    /// for the thing you actually wanted.
    ///
    /// A point `p` of the image (in fit-units from the canvas centre) is
    /// drawn at offset `s = p·z/256 + pan`. Holding `s` fixed at the
    /// cursor offset `c` while z0 → z1 gives `pan1 = c − (c − pan0)·z1/z0`.
    /// Falls back to centred zoom when the pointer is elsewhere.
    fn zoom_at_cursor(&mut self, z1: u16) {
        let z0 = self.zoom;
        self.zoom = z1;
        if z1 == ZOOM_FIT { self.pan = (0, 0); return; }
        let (Some((cx, cy)), Some((rx, ry, rw, rh))) = (cursor_pos(), canvas_rect()) else {
            return;
        };
        if cx < rx || cy < ry || cx >= rx + rw || cy >= ry + rh { return; }
        let c = ((cx - (rx + rw / 2)) as i64, (cy - (ry + rh / 2)) as i64);
        let (z0, z1) = (z0 as i64, z1 as i64);
        let px = c.0 - (c.0 - self.pan.0 as i64) * z1 / z0;
        let py = c.1 - (c.1 - self.pan.1 as i64) * z1 / z0;
        self.pan = (px as i32, py as i32);
    }
}

enum Outcome { Idle, Render, Reload, Exit }

fn handle(iris: &mut Iris, ev: Event, payload: &str) -> Outcome {
    match ev {
        // Escape closes an open menu first, the window only when none is.
        Event::Key(KeyCode::Escape) => {
            if iris.menu.is_some() || iris.picking {
                iris.menu = None; iris.picking = false; Outcome::Render
            } else { Outcome::Exit }
        }
        Event::Key(KeyCode::Right) | Event::Key(KeyCode::Down) => { iris.next(); Outcome::Reload }
        Event::Key(KeyCode::Left)  | Event::Key(KeyCode::Up)   => { iris.prev(); Outcome::Reload }
        // Keyboard zoom, the usual trio.
        Event::Key(KeyCode::Char(b'+')) | Event::Key(KeyCode::Char(b'=')) => {
            iris.set_zoom(zoom_in(iris.zoom)); Outcome::Render
        }
        Event::Key(KeyCode::Char(b'-')) => { iris.set_zoom(zoom_out(iris.zoom)); Outcome::Render }
        Event::Key(KeyCode::Char(b'0')) => { iris.reset_view(); Outcome::Render }
        // Wheel zoom. `dy` is the compositor's scroll step, sign only —
        // up (negative) enlarges, like every other viewer.
        Event::Wheel { dy } => {
            iris.zoom_at_cursor(if dy < 0 { zoom_in(iris.zoom) } else { zoom_out(iris.zoom) });
            Outcome::Render
        }
        // A widget (menu label, menu item, toolbar button) took this
        // click. Swallow the raw press that the compositor sends right
        // after it, or the same physical click would act twice.
        Event::Action(ActionId(id)) => {
            iris.swallow_press = true;
            handle_action(iris, id)
        }
        // Press just arms a possible drag — what it MEANT is decided on
        // release, because the same button both pans and pages. That is
        // how every image viewer resolves this: a drag moves the picture,
        // a click without movement is still a click.
        Event::MouseButton { button: MouseButton::Left, down: true, x, y } => {
            if iris.swallow_press {
                iris.swallow_press = false;
                return Outcome::Idle;
            }
            // Belt and braces: a click with a dropdown open is the
            // dismissing click and belongs to the Popover, not to us.
            if iris.menu.is_some() || iris.picking { return Outcome::Idle; }
            iris.press = Some((x, y));
            iris.dragged = false;
            Outcome::Idle
        }
        Event::MouseButton { button: MouseButton::Left, down: false, .. } => {
            let was_click = iris.press.is_some() && !iris.dragged;
            iris.press = None;
            if was_click { iris.next(); Outcome::Reload } else { Outcome::Idle }
        }
        // Motion only arrives while the button is held (the compositor
        // forwards drags, never hover), so this IS the pan.
        Event::MouseMove { x, y } => {
            let Some((px, py)) = iris.press else { return Outcome::Idle };
            let (dx, dy) = (x - px, y - py);
            if !iris.dragged && dx.abs() + dy.abs() <= DRAG_SLOP {
                return Outcome::Idle;   // still a click, not yet a drag
            }
            iris.dragged = true;
            iris.press = Some((x, y));
            if iris.zoom == ZOOM_FIT { return Outcome::Idle; } // nothing to pan
            iris.pan = (iris.pan.0 + dx, iris.pan.1 + dy);
            Outcome::Render
        }
        Event::MouseButton { button: MouseButton::Right, down: true, .. } => { iris.prev(); Outcome::Reload }
        // Another image opened while running (loft / singleton routing).
        Event::Open(_) => { iris.point_at(payload); Outcome::Reload }
        _ => Outcome::Idle,
    }
}

fn handle_action(iris: &mut Iris, id: u32) -> Outcome {
    // Picking a file out of the Open list.
    if id >= ACT_OPEN_FILE_BASE {
        let i = (id - ACT_OPEN_FILE_BASE) as usize;
        iris.picking = false;
        if i < iris.files.len() { iris.idx = i; return Outcome::Reload; }
        return Outcome::Render;
    }
    match id {
        // A menu label toggles its own dropdown and closes any other.
        ACT_MENU_FILE => { iris.picking = false; iris.menu = toggle(iris.menu, OpenMenu::File); Outcome::Render }
        ACT_MENU_VIEW => { iris.picking = false; iris.menu = toggle(iris.menu, OpenMenu::View); Outcome::Render }
        ACT_MENU_HELP => { iris.picking = false; iris.menu = toggle(iris.menu, OpenMenu::Help); Outcome::Render }
        ACT_MENU_DISMISS => { iris.menu = None; iris.picking = false; Outcome::Render }
        ACT_FILE_OPEN => { iris.menu = None; iris.refresh(); iris.picking = true; Outcome::Render }
        ACT_FILE_CLOSE => Outcome::Exit,
        ACT_NEXT => { iris.menu = None; iris.next(); Outcome::Reload }
        ACT_PREV => { iris.menu = None; iris.prev(); Outcome::Reload }
        ACT_ZOOM_IN  => { iris.menu = None; iris.set_zoom(zoom_in(iris.zoom)); Outcome::Render }
        ACT_ZOOM_OUT => { iris.menu = None; iris.set_zoom(zoom_out(iris.zoom)); Outcome::Render }
        ACT_ZOOM_FIT => { iris.menu = None; iris.reset_view(); Outcome::Render }
        ACT_HELP_ABOUT => { iris.menu = None; log("[iris] image viewer"); Outcome::Render }
        _ => Outcome::Idle,
    }
}

fn toggle(cur: Option<OpenMenu>, want: OpenMenu) -> Option<OpenMenu> {
    if cur == Some(want) { None } else { Some(want) }
}

// ── Scene ─────────────────────────────────────────────────────────────
//
// Same three bands as loft / spell / beak: menu bar · toolbar · body ·
// footer, all built from prefabs so the chrome tracks the design system
// instead of drifting on its own hardcoded paddings.

/// Toolbar chrome sizing — beak's `toolbar_button` (docs/spec/UI_REFRESH.md §5).
const NAV_BTN: u16 = 28;
const NAV_BTN_RADIUS: u8 = 7;

/// Navigation button: bare at rest, `SurfaceHover` under the cursor,
/// accent tint while pressed. Disabled-looking (faint, no handler) when
/// there is nowhere to go.
fn nav_button(icon: IconId, action: u32, enabled: bool) -> Widget {
    let mut mods: Vec<Modifier> = alloc::vec![
        Modifier::MinWidth(NAV_BTN),
        Modifier::MinHeight(NAV_BTN),
        Modifier::Rounded(NAV_BTN_RADIUS),
    ];
    if enabled {
        mods.push(Modifier::OnClick(ActionId(action)));
        mods.push(Modifier::Hover(alloc::vec![
            Modifier::Background(Token::SurfaceHover),
            Modifier::Rounded(NAV_BTN_RADIUS),
        ]));
        mods.push(Modifier::Active(alloc::vec![
            Modifier::Background(Token::AccentMuted),
            Modifier::Tint(Token::Accent),
            Modifier::Rounded(NAV_BTN_RADIUS),
        ]));
    } else {
        mods.push(Modifier::Tint(Token::OnSurfaceFaint));
    }
    prefab::center_box(Widget::Icon { id: icon, size: 16, modifiers: Vec::new() }, mods)
}

fn render_menu_bar() -> Widget {
    prefab::menu_bar_with_icon(
        IconId::Image,
        &[
            (s().menu_file.to_string(), ActionId(ACT_MENU_FILE)),
            (s().menu_view.to_string(), ActionId(ACT_MENU_VIEW)),
            (s().menu_help.to_string(), ActionId(ACT_MENU_HELP)),
        ],
        &[NodeId(NODE_MENU_FILE), NodeId(NODE_MENU_VIEW), NodeId(NODE_MENU_HELP)],
    )
}

fn render_toolbar(iris: &Iris) -> Widget {
    let many = iris.files.len() > 1;
    let title = iris.files.get(iris.idx).cloned()
        .unwrap_or_else(|| s().no_image.to_string());
    prefab::toolbar(alloc::vec![
        nav_button(IconId::ArrowLeft, ACT_PREV, many),
        nav_button(IconId::ArrowRight, ACT_NEXT, many),
        Widget::Text {
            content: title,
            style: TextStyle::Body,
            modifiers: alloc::vec![Modifier::Flex(1)],
        },
        prefab::text_badge(if iris.files.is_empty() {
            "0/0".to_string()
        } else {
            alloc::format!("{}/{}", iris.idx + 1, iris.files.len())
        }),
    ])
}

/// Footer: full npkFS path left, dimensions (or the failure reason) right
/// — loft's `prefab::footer` split.
fn render_footer(iris: &Iris) -> Widget {
    let path = iris.full_path().unwrap_or_default();
    let right = if iris.loading {
        s().loading.to_string()
    } else if iris.files.is_empty() {
        s().no_images.to_string()
    } else if iris.failed {
        s().unsupported.to_string()
    } else if iris.zoom == ZOOM_FIT {
        alloc::format!("{}×{}", iris.w, iris.h)
    } else {
        // Percent of the fitted size — what the wheel actually changed.
        alloc::format!("{}×{} · {} %", iris.w, iris.h, iris.zoom as u32 * 100 / 256)
    };
    prefab::footer(&path, &right)
}

fn render_dropdown(iris: &Iris, kind: OpenMenu) -> (u32, Widget) {
    match kind {
        OpenMenu::File => (
            NODE_MENU_FILE,
            prefab::popover_menu(&[
                (s().open.to_string(), ActionId(ACT_FILE_OPEN)),
                (s().close.to_string(), ActionId(ACT_FILE_CLOSE)),
            ], None),
        ),
        OpenMenu::View => (
            NODE_MENU_VIEW,
            prefab::popover_menu(&[
                (s().next.to_string(), ActionId(ACT_NEXT)),
                (s().previous.to_string(), ActionId(ACT_PREV)),
                (s().zoom_in.to_string(), ActionId(ACT_ZOOM_IN)),
                (s().zoom_out.to_string(), ActionId(ACT_ZOOM_OUT)),
                (s().zoom_fit.to_string(), ActionId(ACT_ZOOM_FIT)),
            ], None),
        ),
        OpenMenu::Help => (
            NODE_MENU_HELP,
            prefab::popover_menu(&[
                (s().about.to_string(), ActionId(ACT_HELP_ABOUT)),
            ], None),
        ),
    }
}

/// File → Open…: every image in the folder, current one check-marked.
fn render_open_list(iris: &Iris) -> Widget {
    let content = if iris.files.is_empty() {
        prefab::popover_menu(&[
            (s().no_images.to_string(), ActionId(ACT_MENU_DISMISS)),
        ], None)
    } else {
        let items: Vec<(String, ActionId)> = iris.files.iter().enumerate()
            .map(|(i, f)| (f.clone(), ActionId(ACT_OPEN_FILE_BASE + i as u32)))
            .collect();
        prefab::popover_menu(&items, Some(iris.idx))
    };
    Widget::Popover {
        anchor:     NodeId(NODE_MENU_FILE),
        child:      Box::new(content),
        on_dismiss: ActionId(ACT_MENU_DISMISS),
        modifiers:  Vec::new(),
    }
}

fn render(iris: &Iris) -> Widget {
    let mut canvas_mods = alloc::vec![Modifier::Flex(1), Modifier::Background(Token::Page)];
    // The compositor scales the bitmap it already holds — no re-decode,
    // no re-upload, so a wheel notch is a repaint and nothing more.
    if iris.zoom != ZOOM_FIT {
        canvas_mods.push(Modifier::Scale(iris.zoom));
        if iris.pan != (0, 0) {
            canvas_mods.push(Modifier::CanvasOffset { x: iris.pan.0, y: iris.pan.1 });
        }
    }
    let body = Widget::Canvas {
        id: CanvasId(CANVAS_ID as u32),
        width: 320,
        height: 240,
        modifiers: canvas_mods,
    };

    let mut children: Vec<Widget> = alloc::vec![
        render_menu_bar(),
        Widget::Divider,
        render_toolbar(iris),
        Widget::Divider,
        body,
        Widget::Divider,
        render_footer(iris),
    ];

    // The Open list replaces the File dropdown at the same anchor.
    if iris.picking {
        children.push(render_open_list(iris));
    } else if let Some(kind) = iris.menu {
        let (anchor, content) = render_dropdown(iris, kind);
        children.push(Widget::Popover {
            anchor:     NodeId(anchor),
            child:      Box::new(content),
            on_dismiss: ActionId(ACT_MENU_DISMISS),
            modifiers:  Vec::new(),
        });
    }

    Widget::Column {
        children,
        spacing:   Spacing::None.as_u16(),
        align:     Align::Stretch,
        modifiers: alloc::vec![Modifier::Background(Token::Surface)],
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
    // The window appears immediately and says what it is busy with; the
    // first picture has nothing cached behind it and takes the full
    // decode, which is the longest wait iris ever shows anyone.
    iris.loading = true;
    commit_scene(&iris); // creates the window (assigns widget_window_id)
    iris.load();         // window exists now → canvas_commit works
    iris.loading = false;
    commit_scene(&iris); // refresh chrome with dims

    // Consecutive empty polls — the quiet period that gates prefetching.
    let mut quiet: u32 = 0;

    loop {
        match poll_event() {
            PollResult::Event(ev) => {
                quiet = 0;
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
                    Outcome::Reload => {
                        // Say "loading" first, then do the work: a decode
                        // blocks this loop for seconds and nothing else
                        // would tell the user anything.
                        iris.loading = true;
                        commit_scene(&iris);
                        iris.load();
                        iris.loading = false;
                        commit_scene(&iris);
                    }
                    Outcome::Exit => { close_self(); return; }
                }
            }
            PollResult::Empty => {
                // Decode a neighbour once the user has stopped for a
                // moment — ONE per round, then straight back to polling,
                // so a click never waits behind more than a single image.
                if quiet >= QUIET_POLLS {
                    if let Some(idx) = iris.prefetch_target() {
                        alloc_reset(mark);
                        iris.prefetch_one(idx);
                        alloc_reset(mark);   // decode buffers are transient
                        continue;
                    }
                }
                quiet = quiet.saturating_add(1);
                unsafe { let _ = npk_sleep(16); }
            }
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

/// Pointer position in screen coordinates — the same space the mouse
/// events report. `None` while we don't have focus (the kernel refuses).
fn cursor_pos() -> Option<(i32, i32)> {
    let packed = unsafe { npk_cursor_pos() };
    if packed < 0 { return None; }
    Some((packed >> 16, packed & 0xFFFF))
}

/// The canvas widget's laid-out rect (screen coords). `None` until the
/// first scene with a Canvas has been laid out.
fn canvas_rect() -> Option<(i32, i32, i32, i32)> {
    let mut buf = [0u8; 16];
    if unsafe { npk_canvas_rect(CANVAS_ID, buf.as_mut_ptr() as i32) } < 0 { return None; }
    let g = |i: usize| i32::from_le_bytes([buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]);
    Some((g(0), g(4), g(8), g(12)))
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
    for e in nopeek_widgets::fs::list_entries(slice) {
        if e.is_dir { continue; }
        if is_image(e.name) { out.push(e.name.to_string()); }
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
    decode_png_cb(data, |_, _, _| {})
}

/// Decode with progress. `on_rows` is handed the (still incomplete) BGRA
/// buffer every time another band of scanlines is finished, so a viewer
/// can put pixels on screen long before the picture is done.
///
/// Why bands and not a low-resolution preview: a PNG holds no smaller
/// version of itself, and the pixels cannot be sampled — the whole file
/// is ONE deflate stream, and every scanline's filter refers to the one
/// above it. Row 500 is unreachable except through rows 0..499. What the
/// format does give us is that the stream arrives in order, so the top of
/// the picture is genuinely ready while the bottom is still compressed.
/// We were simply throwing that away until the last byte arrived.
fn decode_png_cb<F>(data: &[u8], mut on_rows: F) -> Option<(Vec<u8>, u32, u32)>
where
    F: FnMut(&[u8], u32, u32),
{
    let t_enter = now_ms();
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
    let t_parsed = now_ms();
    log_ms("png parse", t_parsed - t_enter);

    let row_bytes = 1 + stride;
    let rows = height as usize;
    let pixel_count = (width * height) as usize;

    let mut decompressed = alloc::vec![0u8; rows * row_bytes];
    let mut unfiltered = alloc::vec![0u8; rows * stride];
    // Opaque black underneath, so the part that has not arrived yet reads
    // as a neutral band rather than as transparent garbage.
    // No alpha pre-fill: the conversion writes all four bytes of every
    // pixel. Pre-filling was needed while half-decoded pictures went on
    // screen; now that nothing partial is shown it was two million loop
    // iterations of pure waste per image — and it sat inside the phase we
    // have spent all afternoon trying to speed up.
    let mut bgra = alloc::vec![0u8; pixel_count * 4];

    use miniz_oxide::inflate::core::{decompress, inflate_flags, DecompressorOxide};
    use miniz_oxide::inflate::TINFLStatus;

    let mut state = DecompressorOxide::new();
    let mut in_pos = 0usize;
    let mut out_pos = 0usize;
    let mut rows_done = 0usize;
    let mut t_inflate: i64 = 0;
    let mut t_rest: i64 = 0;

    // Feed the compressed stream in slices so finished scanlines can be
    // handed on while the rest is still packed. Twelve bands is a
    // compromise: fine enough that the first pixels show up in about a
    // tenth of the total time, coarse enough that the extra full-buffer
    // uploads stay in the noise.
    let bands = 12usize;
    let chunk = (idat_data.len() / bands).max(64 * 1024);

    loop {
        let t0 = now_ms();
        let end = (in_pos + chunk).min(idat_data.len());
        let last_slice = end >= idat_data.len();
        let mut flags = inflate_flags::TINFL_FLAG_PARSE_ZLIB_HEADER
            | inflate_flags::TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF;
        if !last_slice { flags |= inflate_flags::TINFL_FLAG_HAS_MORE_INPUT; }

        let (status, used_in, used_out) =
            decompress(&mut state, &idat_data[in_pos..end], &mut decompressed, out_pos, flags);
        in_pos += used_in;
        out_pos += used_out;
        t_inflate += now_ms() - t0;

        let t1 = now_ms();
        let ready = (out_pos / row_bytes).min(rows);
        if ready > rows_done {
            unfilter_rows(&decompressed, &mut unfiltered, rows_done, ready, stride, channels);
            rows_to_bgra(&unfiltered, &mut bgra, rows_done, ready, width as usize, channels);
            rows_done = ready;
            on_rows(&bgra, width, height);
        }
        t_rest += now_ms() - t1;

        match status {
            TINFLStatus::Done => break,
            TINFLStatus::NeedsMoreInput | TINFLStatus::HasMoreOutput => {
                if last_slice && used_in == 0 && used_out == 0 { break; }
            }
            _ => return None,
        }
        if rows_done >= rows { break; }
    }

    log_ms("inflate", t_inflate);
    log_ms("unfilter + bgra", t_rest);
    if rows_done < rows { return None; }
    Some((bgra, width, height))
}

/// Un-filter scanlines `[from, to)` in place. Resumable: everything it
/// needs about earlier rows is already in `unfiltered`.
fn unfilter_rows(decompressed: &[u8], unfiltered: &mut [u8],
                 from: usize, to: usize, stride: usize, channels: usize) {
    for y in from..to {
        let src_offset = y * (1 + stride);
        let filter_type = decompressed[src_offset];
        let row_start = src_offset + 1;
        let dst = y * stride;
        let src = &decompressed[row_start..row_start + stride];
        // Split so the row above is an immutable slice and the current row
        // a mutable one: the predictors then read from a plain slice
        // instead of re-indexing the whole image buffer.
        let (done, rest) = unfiltered.split_at_mut(dst);
        let above: Option<&[u8]> = if y == 0 { None } else { Some(&done[dst - stride..]) };
        unfilter_row(filter_type, &mut rest[..stride], src, above, channels);
    }
}

/// Convert scanlines `[from, to)` to BGRA. One 32-bit store per pixel
/// instead of four byte stores; BGRA in memory is little-endian
/// B | G<<8 | R<<16 | A<<24.
fn rows_to_bgra(unfiltered: &[u8], bgra: &mut [u8],
                from: usize, to: usize, width: usize, channels: usize) {
    let src_stride = width * channels;
    let dst_stride = width * 4;
    for y in from..to {
        let s = &unfiltered[y * src_stride..(y + 1) * src_stride];
        let d = &mut bgra[y * dst_stride..(y + 1) * dst_stride];
        if channels == 4 {
            for (d, s) in d.chunks_exact_mut(4).zip(s.chunks_exact(4)) {
                let v = (s[2] as u32)
                    | ((s[1] as u32) << 8)
                    | ((s[0] as u32) << 16)
                    | ((s[3] as u32) << 24);
                d.copy_from_slice(&v.to_le_bytes());
            }
        } else {
            for (d, s) in d.chunks_exact_mut(4).zip(s.chunks_exact(3)) {
                let v = (s[2] as u32)
                    | ((s[1] as u32) << 8)
                    | ((s[0] as u32) << 16)
                    | 0xFF00_0000;
                d.copy_from_slice(&v.to_le_bytes());
            }
        }
    }
}

// ── PNG un-filtering, one row at a time ───────────────────────────────
//
// The filter type is constant per scanline, so it is resolved once per
// row, not once per byte. Inside a row the two left-hand predictors (`a`
// = the pixel just written, `c` = the one above it) are carried in
// locals instead of being read back out of the image buffer — only `b`
// is an actual load. The channel count is a const parameter so the
// per-channel loop unrolls and the array indices become constants.
//
// This is the hot loop of the whole viewer: real PNGs use Paeth and
// Average for nearly every row (measured on our own wallpapers: 886 of
// 1080 rows Paeth, 166 Average, none unfiltered), so the naive version
// spent ~1.5 s per image here.

fn unfilter_row(filter: u8, cur: &mut [u8], src: &[u8], above: Option<&[u8]>, channels: usize) {
    // color_type is validated as 2 (RGB) or 6 (RGBA) before we get here.
    if channels == 3 {
        unfilter_row_n::<3>(filter, cur, src, above)
    } else {
        unfilter_row_n::<4>(filter, cur, src, above)
    }
}

fn unfilter_row_n<const C: usize>(filter: u8, cur: &mut [u8], src: &[u8], above: Option<&[u8]>) {
    match (filter, above) {
        // None — and every predictor on the first row where "above"
        // reads as zero and the left one is absent.
        (0, _) | (2, None) => cur.copy_from_slice(src),

        // Sub: left neighbour. Paeth on the first row is the same thing,
        // because paeth(a, 0, 0) == a.
        (1, _) | (4, None) => {
            let mut a = [0u8; C];
            for (out, inp) in cur.chunks_exact_mut(C).zip(src.chunks_exact(C)) {
                for k in 0..C {
                    let v = inp[k].wrapping_add(a[k]);
                    out[k] = v;
                    a[k] = v;
                }
            }
        }

        // Up: the byte directly above. No left-hand state at all.
        (2, Some(up)) => {
            for ((o, s), b) in cur.iter_mut().zip(src.iter()).zip(up.iter()) {
                *o = s.wrapping_add(*b);
            }
        }

        // Average: mean of left and above (floor).
        (3, up) => {
            let mut a = [0u8; C];
            match up {
                Some(up) => {
                    for ((out, upx), inp) in cur.chunks_exact_mut(C)
                        .zip(up.chunks_exact(C))
                        .zip(src.chunks_exact(C))
                    {
                        for k in 0..C {
                            let v = inp[k]
                                .wrapping_add(((a[k] as u16 + upx[k] as u16) / 2) as u8);
                            out[k] = v;
                            a[k] = v;
                        }
                    }
                }
                None => {
                    for (out, inp) in cur.chunks_exact_mut(C).zip(src.chunks_exact(C)) {
                        for k in 0..C {
                            let v = inp[k].wrapping_add((a[k] / 2) as u8);
                            out[k] = v;
                            a[k] = v;
                        }
                    }
                }
            }
        }

        // Paeth: left / above / above-left predictor.
        (4, Some(up)) => {
            let mut a = [0u8; C];
            let mut c = [0u8; C];
            for ((out, upx), inp) in cur.chunks_exact_mut(C)
                .zip(up.chunks_exact(C))
                .zip(src.chunks_exact(C))
            {
                for k in 0..C {
                    let b = upx[k];
                    let v = inp[k].wrapping_add(paeth(a[k], b, c[k]));
                    out[k] = v;
                    a[k] = v;
                    c[k] = b;
                }
            }
        }

        // Unknown filter byte — treat the row as unfiltered rather than
        // failing the whole image.
        _ => cur.copy_from_slice(src),
    }
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
