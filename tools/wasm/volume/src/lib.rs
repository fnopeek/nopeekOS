//! volume — the master volume as a small card next to the panel's
//! speaker.
//!
//! deck opens it with `npk_launch_at`; the kernel places the overlay next
//! to the item that was clicked and closes it on a click outside
//! (docs/plan/STATUS_ITEMS.md). The speaker mutes, the slider sets the
//! level while it is dragged, Esc closes. A level changed elsewhere (keys,
//! another app) shows here as it happens.

#![no_std]

extern crate alloc;


use nopeek_widgets::prefab;
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

// The overlay calls only this app makes; the shared ones are in
// `nopeek_widgets::host`.
#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn npk_window_set_overlay(w: i32, h: i32) -> i32;
    fn npk_window_set_light_dismiss(on: i32) -> i32;
}

fn set_overlay(w: i32, h: i32) {
    // SAFETY: FFI without pointers.
    unsafe { npk_window_set_overlay(w, h) };
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

const MUTE: u32 = 2;
const LEVEL: u32 = 3;

// Card size (logical px): a 28 px button row and the card's padding.
const W: i32 = 280;
const H: i32 = 44;

// Wake masks for `host::wait` (docs/plan/CORES_AND_EVENTS.md).
const WAIT_INPUT: u32 = 1;
const WAIT_STATE: u32 = 32;

struct Volume {
    vol: u8,
    /// Level restored on un-mute.
    pre_mute: u8,
    /// Allocator fill level before the first frame.
    heap_base: usize,
}

fn render(v: &Volume) -> Widget {
    let icon = if v.vol == 0 { IconId::SpeakerX }
               else if v.vol <= 50 { IconId::SpeakerLow }
               else { IconId::SpeakerHigh };
    Widget::Row {
        children: alloc::vec![
            prefab::icon_button(icon, 16, Some(ActionId(MUTE)), None),
            Widget::Slider {
                value: (v.vol as u32 * SLIDER_MAX as u32 / 100) as u16,
                on_change: ActionId(LEVEL),
                modifiers: alloc::vec![Modifier::Flex(1)],
            },
            Widget::Text {
                content: alloc::format!("{}%", v.vol),
                style: TextStyle::Mono,
                modifiers: alloc::vec![Modifier::MinWidth(36), Modifier::Tint(Token::OnSurfaceMuted)],
            },
        ],
        spacing: Spacing::Sm.as_u16(),
        align: Align::Center,
        modifiers: alloc::vec![
            Modifier::Background(Token::SurfaceElevated),
            Modifier::Border { token: Token::Border, width: 1, radius: Radius::Lg.as_u8() },
            Modifier::Rounded(Radius::Lg.as_u8()),
            Modifier::PaddingXY { x: Padding::Sm.as_u16(), y: Padding::Sm.as_u16() },
            Modifier::MinWidth(W as u16),
            Modifier::MinHeight(H as u16),
        ],
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
        Event::Action(ActionId(MUTE)) => {
            if st.vol > 0 {
                st.pre_mute = st.vol;
                set_volume(st, 0);
            } else {
                set_volume(st, st.pre_mute.max(10));
            }
            Outcome::Rerender
        }
        // The level follows the hand; the ear is the feedback.
        Event::Slide { action: ActionId(LEVEL), value, .. } => {
            set_volume(st, (value as u32 * 100 / SLIDER_MAX as u32) as u8);
            Outcome::Rerender
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
    // Placed by the kernel next to the item deck was clicked on; started any
    // other way it is centred. Light-dismiss either way.
    set_overlay(W, H);
    set_light_dismiss(true);
    commit_tree(&st);

    let mut event_buf = [0u8; EVENT_BUF_SIZE];
    loop {
        let mut redraw = false;
        loop {
            match poll_event(&mut event_buf) {
                PollResult::Event(ev) => match handle(&mut st, ev) {
                    Outcome::Idle => {}
                    Outcome::Rerender => redraw = true,
                    Outcome::Exit => { host::close_widget(); return; }
                },
                PollResult::Empty => break,
                PollResult::WindowGone => return,
            }
        }
        // Changed elsewhere (keys, another app): show it.
        let now = host::audio_volume().unwrap_or(0).min(100) as u8;
        if now != st.vol { st.vol = now; redraw = true; }
        if redraw { commit_tree(&st); }
        host::wait(WAIT_INPUT | WAIT_STATE, -1);
    }
}
