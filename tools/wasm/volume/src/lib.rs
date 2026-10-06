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

// Read + exec + render, and the shell roles: a placed overlay and the
// system volume.
#[unsafe(link_section = ".npk.caps")]
#[used]
static NPK_CAPS: [u8; 2] = [caps::READ | caps::EXEC | caps::RENDER, caps::ext::SHELL];

use nopeek_widgets::host;

// The two overlay calls only this app makes; the shared ones are in
// `nopeek_widgets::host`.
#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn npk_window_set_overlay_at(x: i32, y: i32, w: i32, h: i32) -> i32;
    fn npk_window_set_light_dismiss(on: i32) -> i32;
}

fn set_overlay_at(x: i32, y: i32, w: i32, h: i32) {
    // SAFETY: FFI without pointers.
    unsafe { npk_window_set_overlay_at(x, y, w, h) };
}

fn set_light_dismiss(on: bool) {
    // SAFETY: FFI without pointers.
    unsafe { npk_window_set_light_dismiss(on as i32) };
}

fn log(msg: &str) { host::log_serial(msg); }

const EVENT_BUF_SIZE: usize = 64;

enum PollResult { Event(Event), Empty, WindowGone }

fn poll_event(buf: &mut [u8]) -> PollResult {
    match nopeek_widgets::events::poll(buf) {
        nopeek_widgets::events::Poll::Event(ev) => PollResult::Event(ev),
        nopeek_widgets::events::Poll::Empty => PollResult::Empty,
        nopeek_widgets::events::Poll::Gone => PollResult::WindowGone,
    }
}

// No heap state survives between frames (the slider keeps its level in
// `Volume`), so every commit resets to the start mark and rebuilds.
#[global_allocator]
static ALLOCATOR: nopeek_widgets::bump::Bump<{ 128 * 1024 }> = nopeek_widgets::bump::Bump::new();

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    log("[volume] panic!");
    core::arch::wasm32::unreachable()
}

// ── State + actions ──────────────────────────────────────────────────
const MUTE: u32  = 2;
const VOL_SET_BASE: u32 = 100;
const VOL_STEPS: u32 = 20;          // 5 %-steps

// Overlay size (px). Sized to the header + a 20-cell track.
const W: i32 = 340;
const H: i32 = 92;

struct Volume {
    vol: u8,
    /// Level restored on un-mute.
    pre_mute: u8,
    /// Allocator fill level before the first frame.
    heap_base: usize,
}

fn render(v: &Volume) -> Widget {
    let vol = v.vol as u32;
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

fn commit_tree(v: &Volume) {
    ALLOCATOR.reset(v.heap_base);   // reclaim the previous frame's tree
    let tree = render(v);
    match wire::encode(&tree) {
        Ok(bytes) => { if !host::scene_commit(&bytes) { log("[volume] commit failed"); } }
        Err(_) => log("[volume] encode failed"),
    }
}

enum Outcome { Idle, Rerender, Exit }

fn set_volume(st: &mut Volume, level: u8) {
    let v = level.min(100);
    host::audio_set_volume(v as u32);
    st.vol = v;
}

fn handle(st: &mut Volume, ev: Event) -> Outcome {
    match ev {
        Event::Key(KeyCode::Escape) => Outcome::Exit,
        Event::Action(ActionId(id)) => {
            if id == MUTE {
                let v = host::audio_volume().unwrap_or(0);
                if v > 0 {
                    st.pre_mute = v as u8;
                    set_volume(st, 0);
                } else {
                    set_volume(st, st.pre_mute.max(10));
                }
                Outcome::Rerender
            } else if id >= VOL_SET_BASE && id < VOL_SET_BASE + VOL_STEPS {
                let n = id - VOL_SET_BASE;
                set_volume(st, ((n + 1) * (100 / VOL_STEPS)) as u8);
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
    let vol = host::audio_volume().unwrap_or(0).min(100) as u8;
    let mut st = Volume {
        vol,
        pre_mute: if vol > 0 { vol } else { 50 },
        heap_base: ALLOCATOR.mark(),
    };
    // Top-right, just below the bar (≈8 px under the ~40 px strut). The
    // compositor clamps to the screen. Light-dismiss = close on a click
    // outside; Esc closes too.
    let screen_w = (host::screen_size().0 as i32).max(W + 20);
    let x = screen_w - W - 12;
    set_overlay_at(x, 48, W, H);
    set_light_dismiss(true);

    commit_tree(&st);

    let mut event_buf = [0u8; EVENT_BUF_SIZE];
    loop {
        match poll_event(&mut event_buf) {
            PollResult::Event(ev) => match handle(&mut st, ev) {
                Outcome::Idle => {}
                Outcome::Rerender => commit_tree(&st),
                Outcome::Exit => { host::close_widget(); return; }
            },
            PollResult::Empty => host::sleep_ms(16),
            PollResult::WindowGone => return,
        }
    }
}
