//! volume — a centred overlay volume slider, launched by the bar.
//!
//! drun-style: declares itself a modal overlay (`npk_window_set_overlay` +
//! `npk_window_set_modal`), renders a clickable level track, adjusts the
//! kernel master volume, and closes on Esc / the X / picking nothing. No
//! kernel support beyond the generic overlay host fns — the bar just
//! `launch`es this module on a speaker click.

#![no_std]

extern crate alloc;

use alloc::string::ToString;
use alloc::vec::Vec;

use nopeek_widgets::style::{Padding, Radius, Spacing};
use nopeek_widgets::*;

#[unsafe(link_section = ".npk.app_meta")]
#[used]
static APP_META_BYTES: [u8; include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin")).len()]
    = *include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin"));

// Host functions are WASM imports from the `env` module, resolved by the
// kernel at instantiation. Naming the module explicitly is what makes them
// imports rather than ordinary undefined C symbols, which rust-lld rejects.
#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn npk_scene_commit(ptr: i32, len: i32) -> i32;
    fn npk_event_poll(ptr: i32, max: i32) -> i32;
    fn npk_screen_size() -> i32;
    fn npk_window_set_overlay_at(x: i32, y: i32, w: i32, h: i32) -> i32;
    fn npk_window_set_light_dismiss(on: i32) -> i32;
    fn npk_close_widget() -> i32;
    fn npk_audio_get_volume() -> i32;
    fn npk_audio_set_volume(pct: i32) -> i32;
    fn npk_log_serial(ptr: i32, len: i32);
    fn npk_sleep(ms: i32) -> i32;
}

fn log(msg: &str) {
    unsafe { npk_log_serial(msg.as_ptr() as i32, msg.len() as i32); }
}

fn commit(bytes: &[u8]) -> i32 {
    unsafe { npk_scene_commit(bytes.as_ptr() as i32, bytes.len() as i32) }
}

const EVENT_BUF_SIZE: usize = 64;
static mut EVENT_BUF: [u8; EVENT_BUF_SIZE] = [0; EVENT_BUF_SIZE];

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

// ── Bump allocator ───────────────────────────────────────────────────
// No heap state survives between frames (the slider reads its level from
// the statics below), so every commit resets to zero and rebuilds.
const HEAP_SIZE: usize = 128 * 1024;
static mut HEAP: [u8; HEAP_SIZE] = [0; HEAP_SIZE];
static mut HEAP_POS: usize = 0;

struct BumpAllocator;
unsafe impl core::alloc::GlobalAlloc for BumpAllocator {
    unsafe fn alloc(&self, layout: core::alloc::Layout) -> *mut u8 {
        let pos_ptr = core::ptr::addr_of_mut!(HEAP_POS);
        let current = unsafe { pos_ptr.read() };
        let aligned = (current + layout.align() - 1) & !(layout.align() - 1);
        if aligned + layout.size() > HEAP_SIZE { return core::ptr::null_mut(); }
        unsafe { pos_ptr.write(aligned + layout.size()); }
        unsafe { (core::ptr::addr_of_mut!(HEAP) as *mut u8).add(aligned) }
    }
    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: core::alloc::Layout) {}
}

#[global_allocator]
static ALLOCATOR: BumpAllocator = BumpAllocator;

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    log("[volume] panic!");
    loop {}
}

// ── State + actions ──────────────────────────────────────────────────
const MUTE: u32  = 2;
const VOL_SET_BASE: u32 = 100;
const VOL_STEPS: u32 = 20;          // 5 %-steps

// Overlay size (px). Sized to the header + a 20-cell track.
const W: i32 = 340;
const H: i32 = 92;

static mut VOL: u8 = 50;
static mut PRE_MUTE: u8 = 50;       // level restored on un-mute

fn render() -> Widget {
    let vol = unsafe { VOL } as u32;
    let step = 100 / VOL_STEPS;

    // The slider track: VOL_STEPS clickable cells, filled (Accent) up to the
    // current level, muted past it. No drag (the ABI gives clicks), so the
    // cells double as the slider's discrete stops.
    let mut cells: Vec<Widget> = Vec::with_capacity(VOL_STEPS as usize);
    for i in 0..VOL_STEPS {
        let level = (i + 1) * step;
        let tok = if level <= vol { Token::Accent } else { Token::SurfaceMuted };
        cells.push(Widget::Text {
            content: " ".to_string(),     // a space gives the cell its height
            style: TextStyle::Body,
            modifiers: alloc::vec![
                Modifier::MinWidth(12),
                Modifier::Background(tok),
                Modifier::Rounded(Radius::Sm.as_u8()),
                Modifier::OnClick(ActionId(VOL_SET_BASE + i)),
            ],
        });
    }
    let track = Widget::Row {
        children: cells,
        spacing: Spacing::Xs.as_u16(),
        align: Align::Center,
        modifiers: Vec::new(),
    };

    let icon = if vol == 0 { IconId::SpeakerX }
               else if vol <= 50 { IconId::SpeakerLow }
               else { IconId::SpeakerHigh };
    // Speaker → toggle mute. Esc / click-outside close (no chrome button).
    let header = Widget::Row {
        children: alloc::vec![
            Widget::Icon { id: icon, size: 20,
                modifiers: alloc::vec![Modifier::OnClick(ActionId(MUTE))] },
            Widget::Text { content: alloc::format!("  {}%", vol),
                style: TextStyle::Body, modifiers: Vec::new() },
        ],
        spacing: Spacing::Sm.as_u16(),
        align: Align::Center,
        modifiers: Vec::new(),
    };

    Widget::Column {
        children: alloc::vec![header, track],
        spacing: Spacing::Sm.as_u16(),
        align: Align::Stretch,
        modifiers: alloc::vec![Modifier::Padding(Padding::Sm.as_u16())],
    }
}

fn commit_tree() {
    unsafe { HEAP_POS = 0; }   // reclaim the previous frame's tree
    let tree = render();
    match wire::encode(&tree) {
        Ok(bytes) => { if commit(&bytes) < 0 { log("[volume] commit failed"); } }
        Err(_) => log("[volume] encode failed"),
    }
}

enum Outcome { Idle, Rerender, Exit }

fn set_volume(level: u8) {
    let v = level.min(100);
    unsafe { let _ = npk_audio_set_volume(v as i32); VOL = v; }
}

fn handle(ev: Event) -> Outcome {
    match ev {
        Event::Key(KeyCode::Escape) => Outcome::Exit,
        Event::Action(ActionId(id)) => {
            if id == MUTE {
                let v = unsafe { npk_audio_get_volume() };
                if v > 0 {
                    unsafe { PRE_MUTE = v as u8; }
                    set_volume(0);
                } else {
                    set_volume(unsafe { PRE_MUTE }.max(10));
                }
                Outcome::Rerender
            } else if id >= VOL_SET_BASE && id < VOL_SET_BASE + VOL_STEPS {
                let n = id - VOL_SET_BASE;
                set_volume(((n + 1) * (100 / VOL_STEPS)) as u8);
                Outcome::Rerender
            } else {
                Outcome::Idle
            }
        }
        _ => Outcome::Idle,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    unsafe {
        VOL = npk_audio_get_volume().clamp(0, 100) as u8;
        PRE_MUTE = if VOL > 0 { VOL } else { 50 };
        // Top-right, just below the bar (≈8 px under the ~40 px strut). The
        // compositor clamps to the screen. Light-dismiss = close on a click
        // outside; Esc closes too.
        let packed = npk_screen_size();
        let screen_w = ((packed >> 16) & 0xFFFF).max(W + 20);
        let x = screen_w - W - 12;
        let _ = npk_window_set_overlay_at(x, 48, W, H);
        let _ = npk_window_set_light_dismiss(1);
    }

    commit_tree();

    loop {
        match poll_event() {
            PollResult::Event(ev) => match handle(ev) {
                Outcome::Idle => {}
                Outcome::Rerender => commit_tree(),
                Outcome::Exit => { unsafe { let _ = npk_close_widget(); } return; }
            },
            PollResult::Empty => { unsafe { let _ = npk_sleep(16); } }
            PollResult::WindowGone => return,
        }
    }
}
