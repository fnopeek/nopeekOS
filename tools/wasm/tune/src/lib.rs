//! tune — the audio player for nopeekOS.
//!
//! Layout (top → bottom):
//!   toolbar  — note icon · title / artist · spacer · "1:23 / 4:07"
//!   progress — segmented bar, one click = one seek
//!   transport— skip back · play/pause · skip forward · master volume
//!   playlist — every audio file in the folder, current one highlighted
//!   footer   — npkFS path · "MP3 · 192 kbps · 44.1 kHz · stereo"
//!
//! The player itself knows no formats. It pulls f32 frames out of a
//! [`source::Source`], hands them to [`sink::Sink`] (resample → 48 kHz S16
//! stereo → kernel audio mailbox), and the HDA driver takes it from there.
//! Adding FLAC or Opus later means adding a `Source`, not touching this
//! file — see `src/source.rs`.
//!
//! Decoding costs about 6 % of one core on the device (measured under the
//! kernel's wasmi, 44.1 kHz / 128 kbps), so the loop below decodes ahead in
//! small steps between event polls rather than in one burst.

#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use nopeek_widgets::prefab;
use nopeek_widgets::style::{Padding, Radius, Spacing};
use nopeek_widgets::*;

mod host;
mod mp3;
mod resample;
mod sink;
mod source;
mod wav;

use source::{Source, MAX_BLOCK_SAMPLES};

#[unsafe(link_section = ".npk.app_meta")]
#[used]
static APP_META_BYTES: [u8; include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin")).len()]
    = *include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin"));

// Read files + draw. The audio mailbox is ungated — playback is not a
// security boundary, and the kernel holds no format knowledge to protect.
#[unsafe(link_section = ".npk.caps")]
#[used]
static NPK_CAPS: [u8; 1] = [caps::READ | caps::RENDER];

fn log(msg: &str) { host::log(msg); }

// ── Buffers ───────────────────────────────────────────────────────────
const EVENT_BUF_SIZE: usize = 4 * 1024;
static mut EVENT_BUF: [u8; EVENT_BUF_SIZE] = [0; EVENT_BUF_SIZE];

const LIST_BUF_SIZE: usize = 64 * 1024;
static mut LIST_BUF: [u8; LIST_BUF_SIZE] = [0; LIST_BUF_SIZE];

const HOME_CAP: usize = 256;
static mut HOME_BUF: [u8; HOME_CAP] = [0; HOME_CAP];

const PAYLOAD_CAP: usize = 1024;
static mut PAYLOAD_BUF: [u8; PAYLOAD_CAP] = [0; PAYLOAD_CAP];

/// One decoded block, interleaved f32 at the source rate.
static mut BLOCK: [f32; MAX_BLOCK_SAMPLES] = [0.0; MAX_BLOCK_SAMPLES];

fn copy_payload(s: &str) -> usize {
    let n = s.len().min(PAYLOAD_CAP);
    let dst = &raw mut PAYLOAD_BUF as *mut u8;
    unsafe { core::ptr::copy_nonoverlapping(s.as_ptr(), dst, n); }
    n
}

fn payload_str(len: usize) -> &'static str {
    let ptr = &raw const PAYLOAD_BUF as *const u8;
    let slice = unsafe { core::slice::from_raw_parts(ptr, len) };
    core::str::from_utf8(slice).unwrap_or("")
}

enum PollResult { Event(Event), Empty, WindowGone }

fn poll_event() -> PollResult {
    let buf_ptr = &raw mut EVENT_BUF as *mut u8;
    let n = host::event_poll(buf_ptr, EVENT_BUF_SIZE);
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
// Everything the player keeps is small: the playlist, one decoder state,
// and a scene tree rebuilt from scratch each frame. The file bytes do NOT
// live here — they are claimed with `memory.grow` (see `file_arena`), so a
// four-minute song never has to fit in a fixed heap.
const HEAP_SIZE: usize = 4 * 1024 * 1024;
static mut HEAP: [u8; HEAP_SIZE] = [0; HEAP_SIZE];
static mut HEAP_POS: usize = 0;

struct BumpAllocator;
unsafe impl core::alloc::GlobalAlloc for BumpAllocator {
    unsafe fn alloc(&self, layout: core::alloc::Layout) -> *mut u8 {
        let pos = unsafe { (&raw const HEAP_POS).read() };
        let aligned = (pos + layout.align() - 1) & !(layout.align() - 1);
        if aligned + layout.size() > HEAP_SIZE { return core::ptr::null_mut(); }
        unsafe { (&raw mut HEAP_POS).write(aligned + layout.size()) };
        unsafe { (&raw mut HEAP as *mut u8).add(aligned) }
    }
    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: core::alloc::Layout) {}
}

#[global_allocator]
static ALLOCATOR: BumpAllocator = BumpAllocator;

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { log("[tune] panic!"); loop {} }

fn alloc_mark() -> usize { unsafe { (&raw const HEAP_POS).read() } }
fn alloc_reset(pos: usize) { unsafe { (&raw mut HEAP_POS).write(pos) } }

// ── File arena ────────────────────────────────────────────────────────
//
// A song is a few megabytes and the next one is a different few. A static
// array would have to be sized for the longest file anyone owns and would
// be paid for at launch, by everyone. Instead the arena is claimed with
// `memory.grow` at the size the folder listing says this file has, and
// reused for every track after that.
const WASM_PAGE: usize = 64 * 1024;
static mut ARENA_PTR: *mut u8 = core::ptr::null_mut();
static mut ARENA_CAP: usize = 0;

fn arena_reserve(want: usize) -> Option<*mut u8> {
    unsafe {
        if want <= (&raw const ARENA_CAP).read() {
            return Some((&raw const ARENA_PTR).read());
        }
        let pages = want.div_ceil(WASM_PAGE);
        let prev = core::arch::wasm32::memory_grow(0, pages);
        if prev == usize::MAX { return None; }
        // The bump allocator hands out slices of a fixed array and never
        // grows memory, so we are the only caller and each claim is one
        // contiguous run. Dropping the old one simply forgets it.
        let fresh = (prev * WASM_PAGE) as *mut u8;
        (&raw mut ARENA_PTR).write(fresh);
        (&raw mut ARENA_CAP).write(pages * WASM_PAGE);
        Some(fresh)
    }
}

/// Read a whole file into the arena. `size` comes from the folder listing;
/// a wrong guess only costs a second claim.
fn fetch_file(path: &str, size: usize) -> Option<&'static [u8]> {
    let want = size.max(64 * 1024);
    let ptr = arena_reserve(want)?;
    let n = host::fetch(path, ptr, want);
    if n <= 0 { return None; }
    Some(unsafe { core::slice::from_raw_parts(ptr as *const u8, n as usize) })
}

// ── State ─────────────────────────────────────────────────────────────

struct Track { name: String, size: u64 }


struct Tune {
    dir:     String,
    files:   Vec<Track>,
    idx:     usize,
    src:     Option<Box<dyn Source>>,
    sink:    sink::Sink,
    playing: bool,
    /// End of the decoded stream reached; the mailbox may still be draining.
    drained: bool,
    error:   Option<String>,
    /// Launched with a file to open, as opposed to launched bare.
    opened_with_file: bool,
    vol:     u8,
    /// Seconds last drawn, so the loop only re-commits when the clock moves.
    shown_s: i64,
    /// Underruns already reported, so a stutter logs once and not per tick.
    told_underruns: u32,
}

const A_PLAY_PAUSE: u32 = 1;
const A_PREV:       u32 = 2;
const A_NEXT:       u32 = 3;
const SEEK_BASE:    u32 = 100;
const SEEK_CELLS:   u32 = 40;
const VOL_BASE:     u32 = 200;
const VOL_STEPS:    u32 = 10;
const TRACK_BASE:   u32 = 1000;
/// Playlist rows drawn at once. The scene is rebuilt every second while
/// playing, so a folder of two thousand files would re-encode and re-lay-out
/// two thousand rows per second for a clock that moved by one digit. The
/// window follows the current track; skipping still walks the whole folder.
const LIST_WINDOW:  usize = 200;

impl Tune {
    fn new() -> Tune {
        let mut t = Tune {
            dir: String::new(),
            files: Vec::new(),
            idx: 0,
            src: None,
            sink: match sink::Sink::open() {
                Some(s) => s,
                None => { log("[tune] no free audio slot"); sink::Sink::dead() }
            },
            playing: false,
            drained: false,
            error: None,
            opened_with_file: false,
            vol: host::get_volume().clamp(0, 100) as u8,
            shown_s: -1,
            told_underruns: 0,
        };

        let mut argbuf = [0u8; 512];
        let n = host::launch_arg(argbuf.as_mut_ptr(), argbuf.len());
        if n > 0 {
            if let Ok(path) = core::str::from_utf8(&argbuf[..n as usize]) {
                t.point_at(path);
                t.opened_with_file = true;
                return t;
            }
        }
        // No argument: the music folder, if the user has one.
        let home = read_home_dir();
        t.dir = alloc::format!("{}/music", home);
        t.refresh();
        if t.files.is_empty() {
            t.dir = home;
            t.refresh();
        }
        t
    }

    fn point_at(&mut self, path: &str) {
        let (dir, file) = split_path(path);
        self.dir = dir.to_string();
        self.refresh();
        self.idx = self.files.iter().position(|f| f.name == file).unwrap_or(0);
    }

    fn refresh(&mut self) {
        self.files = list_audio(&self.dir);
        if self.idx >= self.files.len() { self.idx = 0; }
    }

    fn full_path(&self) -> Option<String> {
        let f = self.files.get(self.idx)?;
        Some(alloc::format!("{}/{}", self.dir, f.name))
    }

    /// Open the current track. `play` decides whether it starts: opening a
    /// file (loft double-click, `run tune <file>`) means "play this", while
    /// launching the player bare means "here is the folder" — a window that
    /// starts making noise because it was opened is a rude window.
    fn load(&mut self, play: bool) {
        self.src = None;
        self.drained = false;
        self.error = None;
        let path = match self.full_path() { Some(p) => p, None => return };
        let size = self.files[self.idx].size as usize;
        let bytes = match fetch_file(&path, size) {
            Some(b) => b,
            None => { self.error = Some("cannot read file".to_string()); return; }
        };
        if !self.sink.ok() {
            self.error = Some("no free audio slot".to_string());
            return;
        }
        match source::open(bytes) {
            Some(s) => {
                self.sink.restart(s.info().rate, host::ticks(), 0);
                self.src = Some(s);
                self.playing = play;
            }
            None => self.error = Some("unsupported format".to_string()),
        }
    }

    fn info_rate(&self) -> u32 { self.src.as_ref().map(|s| s.info().rate).unwrap_or(48_000) }

    /// Play position in ms — what the speaker has reached, not what the
    /// decoder has read.
    fn position_ms(&self) -> u64 {
        self.sink.played_frames() * 1000 / sink::MIX_RATE as u64
    }

    fn duration_ms(&self) -> u64 {
        self.src.as_ref().map(|s| s.info().duration_ms()).unwrap_or(0)
    }

    /// Decode ahead until the mailbox holds `TARGET_LEAD_MS`. Runs between
    /// event polls, so each visit does a little and returns.
    fn pump(&mut self) {
        if !self.playing || self.src.is_none() { return; }
        if !self.sink.flush() { return; }   // ring still full from last time
        let channels = self.src.as_ref().map(|s| s.info().channels as usize).unwrap_or(2);
        let block = unsafe { &mut *(&raw mut BLOCK) };
        while self.sink.lead_ms() < sink::TARGET_LEAD_MS {
            let n = match self.src.as_mut() {
                Some(s) => s.next_block(block),
                None => 0,
            };
            if n == 0 {
                self.drained = true;
                break;
            }
            self.sink.push(block, n, channels);
            if !self.sink.flush() { break; }
        }
    }

    fn toggle(&mut self) {
        if self.src.is_none() { self.load(true); return; }
        self.playing = !self.playing;
        if self.playing {
            // Without this the first tick after a pause charges the whole
            // paused stretch to the speaker and reports a phantom underrun.
            self.sink.resume(host::ticks());
            return;
        }
        // Pausing drops the buffered lead, so sound stops where the display
        // says it stopped instead of playing on for another half second —
        // and the decoder is rewound to exactly there, so resuming repeats
        // nothing. Seeking already does all of that.
        let at = self.position_ms();
        self.seek_to_ms(at);
    }

    fn seek_to_ms(&mut self, ms: u64) {
        let rate = self.info_rate() as u64;
        let landed = match self.src.as_mut() {
            Some(s) => s.seek(ms * rate / 1000),
            None => return,
        };
        let at48 = landed * sink::MIX_RATE as u64 / rate.max(1);
        self.sink.restart(self.info_rate(), host::ticks(), at48);
        self.drained = false;
    }

    fn skip(&mut self, delta: i64) {
        if self.files.is_empty() { return; }
        let n = self.files.len() as i64;
        let next = (self.idx as i64 + delta).rem_euclid(n);
        self.idx = next as usize;
        self.load(true);
    }

    fn set_volume(&mut self, v: u8) {
        self.vol = v.min(100);
        host::set_volume(self.vol as i32);
    }
}

// ── Rendering ─────────────────────────────────────────────────────────

fn fmt_time(ms: u64) -> String {
    let total = ms / 1000;
    alloc::format!("{}:{:02}", total / 60, total % 60)
}

fn render(t: &Tune) -> Widget {
    let pos = t.position_ms();
    let dur = t.duration_ms();

    let (title, artist) = match t.src.as_ref() {
        Some(s) => {
            let i = s.info();
            let title = i.title.clone().unwrap_or_else(|| {
                t.files.get(t.idx).map(|f| strip_ext(&f.name)).unwrap_or_default()
            });
            (title, i.artist.clone().unwrap_or_default())
        }
        None => (
            t.files.get(t.idx).map(|f| strip_ext(&f.name)).unwrap_or_else(|| "no audio here".to_string()),
            t.error.clone().unwrap_or_default(),
        ),
    };

    let head = Widget::Column {
        children: alloc::vec![
            Widget::Text { content: title, style: TextStyle::Title, modifiers: Vec::new() },
            Widget::Text { content: artist, style: TextStyle::Muted, modifiers: Vec::new() },
        ],
        spacing: 0,
        align: Align::Start,
        modifiers: Vec::new(),
    };

    let toolbar = prefab::toolbar(alloc::vec![
        Widget::Icon { id: IconId::MusicNotes, size: 24, modifiers: Vec::new() },
        head,
        Widget::Spacer { flex: 1 },
        prefab::text_badge(if dur > 0 {
            alloc::format!("{} / {}", fmt_time(pos), fmt_time(dur))
        } else {
            fmt_time(pos)
        }),
    ]);

    // Segmented progress bar. The ABI gives clicks, not drags, so the
    // segments double as the seek stops — the same idiom the volume
    // overlay uses for its level.
    let filled = if dur > 0 { (pos * SEEK_CELLS as u64 / dur).min(SEEK_CELLS as u64) as u32 } else { 0 };
    let mut cells: Vec<Widget> = Vec::with_capacity(SEEK_CELLS as usize);
    for i in 0..SEEK_CELLS {
        let tok = if i < filled { Token::Accent } else { Token::SurfaceMuted };
        let mut mods = alloc::vec![
            Modifier::Flex(1),
            Modifier::MinHeight(10),
            Modifier::Background(tok),
            Modifier::Rounded(2),
        ];
        if dur > 0 { mods.push(Modifier::OnClick(ActionId(SEEK_BASE + i))); }
        cells.push(Widget::Text { content: " ".to_string(), style: TextStyle::Body, modifiers: mods });
    }
    let progress = Widget::Row {
        children: cells,
        spacing: 2,
        align: Align::Center,
        modifiers: alloc::vec![Modifier::PaddingXY { x: Padding::Sm.as_u16(), y: 0 }],
    };

    let mut vol_cells: Vec<Widget> = Vec::with_capacity(VOL_STEPS as usize);
    for i in 0..VOL_STEPS {
        let level = ((i + 1) * (100 / VOL_STEPS)) as u8;
        let tok = if level <= t.vol { Token::Accent } else { Token::SurfaceMuted };
        vol_cells.push(Widget::Text {
            content: " ".to_string(),
            style: TextStyle::Body,
            modifiers: alloc::vec![
                Modifier::MinWidth(10),
                Modifier::MinHeight(14),
                Modifier::Background(tok),
                Modifier::Rounded(Radius::Sm.as_u8()),
                Modifier::OnClick(ActionId(VOL_BASE + i)),
            ],
        });
    }

    let transport = Widget::Row {
        children: alloc::vec![
            prefab::icon_button(IconId::SkipBack, 20, Some(ActionId(A_PREV)), None),
            prefab::icon_button(
                if t.playing { IconId::Pause } else { IconId::Play },
                24, Some(ActionId(A_PLAY_PAUSE)), None),
            prefab::icon_button(IconId::SkipForward, 20, Some(ActionId(A_NEXT)), None),
            Widget::Spacer { flex: 1 },
            Widget::Icon { id: volume_icon(t.vol), size: 16, modifiers: Vec::new() },
            Widget::Row { children: vol_cells, spacing: 2, align: Align::Center, modifiers: Vec::new() },
        ],
        spacing: Spacing::Sm.as_u16(),
        align: Align::Center,
        modifiers: alloc::vec![Modifier::Padding(Padding::Sm.as_u16())],
    };

    let total = t.files.len();
    let first = t.idx.saturating_sub(LIST_WINDOW / 2).min(total.saturating_sub(LIST_WINDOW.min(total)));
    let last = (first + LIST_WINDOW).min(total);
    let mut rows: Vec<Widget> = Vec::with_capacity(last - first + 2);
    if first > 0 { rows.push(prefab::muted(&alloc::format!("… {} above", first))); }
    for (i, f) in t.files[first..last].iter().enumerate() {
        let i = first + i;
        let current = i == t.idx;
        let icon = if current && t.playing { IconId::Play } else { IconId::FileAudio };
        rows.push(prefab::nav_row(icon, &strip_ext(&f.name), current,
            Some(ActionId(TRACK_BASE + i as u32)), None));
    }
    if last < total { rows.push(prefab::muted(&alloc::format!("… {} below", total - last))); }
    let list = Widget::Scroll {
        child: Box::new(Widget::Column {
            children: rows,
            spacing: 2,
            align: Align::Stretch,
            modifiers: alloc::vec![Modifier::PaddingXY { x: Padding::Sm.as_u16(), y: 0 }],
        }),
        axis: Axis::Vertical,
        modifiers: alloc::vec![Modifier::Flex(1)],
    };

    let right = match t.src.as_ref() {
        Some(s) => {
            let i = s.info();
            let ch = if i.channels >= 2 { "stereo" } else { "mono" };
            if i.bitrate_kbps > 0 {
                alloc::format!("{} · {} kbps · {} Hz · {}", i.kind, i.bitrate_kbps, i.rate, ch)
            } else {
                alloc::format!("{} · {} Hz · {}", i.kind, i.rate, ch)
            }
        }
        None => String::new(),
    };

    Widget::Column {
        children: alloc::vec![
            toolbar,
            progress,
            transport,
            Widget::Divider,
            list,
            Widget::Divider,
            prefab::footer(&t.dir, &right),
        ],
        spacing: Spacing::Sm.as_u16(),
        align: Align::Stretch,
        modifiers: alloc::vec![Modifier::Padding(Padding::Xs.as_u16())],
    }
}

fn volume_icon(v: u8) -> IconId {
    if v == 0 { IconId::SpeakerX } else if v <= 50 { IconId::SpeakerLow } else { IconId::SpeakerHigh }
}

fn commit_scene(t: &Tune) {
    match wire::encode(&render(t)) {
        Ok(bytes) => { if host::scene_commit(&bytes) < 0 { log("[tune] commit failed"); } }
        Err(_) => log("[tune] encode failed"),
    }
}

// ── Events ────────────────────────────────────────────────────────────

enum Outcome { Idle, Render, Exit }

fn handle(t: &mut Tune, ev: Event, payload: &str) -> Outcome {
    match ev {
        Event::Key(KeyCode::Escape) => Outcome::Exit,
        Event::Key(KeyCode::Char(b' ')) => { t.toggle(); Outcome::Render }
        Event::Key(KeyCode::Char(b'n')) => { t.skip(1); Outcome::Render }
        Event::Key(KeyCode::Char(b'p')) => { t.skip(-1); Outcome::Render }
        Event::Key(KeyCode::Right) => { let p = t.position_ms() + 5000; t.seek_to_ms(p); Outcome::Render }
        Event::Key(KeyCode::Left) => {
            let p = t.position_ms().saturating_sub(5000);
            t.seek_to_ms(p);
            Outcome::Render
        }
        Event::Key(KeyCode::Up) => { let v = t.vol.saturating_add(5); t.set_volume(v); Outcome::Render }
        Event::Key(KeyCode::Down) => { let v = t.vol.saturating_sub(5); t.set_volume(v); Outcome::Render }
        Event::Open(_) => {
            // Already running and asked to open another file (loft
            // double-click): switch tracks rather than spawning a twin.
            t.point_at(payload);
            t.load(true);
            Outcome::Render
        }
        Event::Action(ActionId(id)) => {
            if id == A_PLAY_PAUSE { t.toggle(); return Outcome::Render; }
            if id == A_PREV {
                // Within the first three seconds "back" means the previous
                // track; after that it means the start of this one — the
                // behaviour every physical player has.
                if t.position_ms() > 3000 { t.seek_to_ms(0); } else { t.skip(-1); }
                return Outcome::Render;
            }
            if id == A_NEXT { t.skip(1); return Outcome::Render; }
            if (SEEK_BASE..SEEK_BASE + SEEK_CELLS).contains(&id) {
                let dur = t.duration_ms();
                if dur > 0 {
                    let cell = (id - SEEK_BASE) as u64;
                    t.seek_to_ms(dur * cell / SEEK_CELLS as u64);
                }
                return Outcome::Render;
            }
            if (VOL_BASE..VOL_BASE + VOL_STEPS).contains(&id) {
                let step = 100 / VOL_STEPS;
                t.set_volume((((id - VOL_BASE) + 1) * step) as u8);
                return Outcome::Render;
            }
            if id >= TRACK_BASE {
                let i = (id - TRACK_BASE) as usize;
                if i < t.files.len() { t.idx = i; t.load(true); }
                return Outcome::Render;
            }
            Outcome::Idle
        }
        _ => Outcome::Idle,
    }
}

// ── npkFS helpers ─────────────────────────────────────────────────────

fn read_home_dir() -> String {
    let buf_ptr = &raw mut HOME_BUF as *mut u8;
    let n = host::home_dir(buf_ptr, HOME_CAP);
    if n <= 0 { return "home".to_string(); }
    let slice = unsafe { core::slice::from_raw_parts(buf_ptr as *const u8, n as usize) };
    core::str::from_utf8(slice).unwrap_or("home").to_string()
}

fn list_audio(dir: &str) -> Vec<Track> {
    let buf_ptr = &raw mut LIST_BUF as *mut u8;
    let n = host::fs_list(dir, buf_ptr, LIST_BUF_SIZE);
    let mut out: Vec<Track> = Vec::new();
    if n <= 0 { return out; }
    let slice = unsafe { core::slice::from_raw_parts(buf_ptr as *const u8, n as usize) };
    for e in nopeek_widgets::fs::list_entries(slice) {
        if e.is_dir || !source::is_audio(e.name) { continue; }
        out.push(Track { name: e.name.to_string(), size: e.size });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn push_u32(s: &mut String, mut n: u32) {
    if n == 0 { s.push('0'); return; }
    let mut buf = [0u8; 10];
    let mut i = 0;
    while n > 0 { buf[i] = b'0' + (n % 10) as u8; n /= 10; i += 1; }
    while i > 0 { i -= 1; s.push(buf[i] as char); }
}

fn split_path(path: &str) -> (&str, &str) {
    match path.rfind('/') {
        Some(i) => (&path[..i], &path[i + 1..]),
        None => ("", path),
    }
}

fn strip_ext(name: &str) -> String {
    match name.rfind('.') {
        Some(i) => name[..i].to_string(),
        None => name.to_string(),
    }
}

// ── Main loop ─────────────────────────────────────────────────────────

/// Poll cadence while playing. The tick is 10 ms, so anything smaller is a
/// lie (see the kernel's sleep granularity); anything larger eats into the
/// 600 ms lead the mailbox is holding.
const TICK_MS: i32 = 10;

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    let mut t = Tune::new();
    commit_scene(&t);      // window appears before the first fetch
    let autoplay = t.opened_with_file;
    t.load(autoplay);
    let mut mark = alloc_mark();
    commit_scene(&t);

    loop {
        // Clock and pump run on EVERY turn, not only when the poll came up
        // empty: a stream of mouse-move events would otherwise starve the
        // decoder for as long as the hand keeps moving, and the mailbox
        // holds 600 ms.
        let now = host::ticks();
        t.sink.tick(now, t.playing);
        t.pump();
        if t.sink.underruns != t.told_underruns {
            // A stutter that only shows as a click is a measurement thrown
            // away — the serial mirror gets the count. Checked here and not
            // in the redraw below, because a hard stall freezes the clock
            // and the redraw with it.
            t.told_underruns = t.sink.underruns;
            let mut m = alloc::string::String::from("[tune] mailbox ran dry, total ");
            push_u32(&mut m, t.told_underruns);
            log(&m);
        }

        match poll_event() {
            PollResult::Event(ev) => {
                let plen = match &ev { Event::Open(s) => copy_payload(s), _ => 0 };
                alloc_reset(mark);
                let outcome = handle(&mut t, ev, payload_str(plen));
                mark = alloc_mark();   // state changes (playlist, tags) persist
                match outcome {
                    Outcome::Idle => {}
                    Outcome::Render => { commit_scene(&t); t.shown_s = -1; }
                    Outcome::Exit => { t.sink.close(); host::close_widget(); return; }
                }
            }
            PollResult::Empty => {
                // The track ends when the mailbox has drained, not when the
                // decoder ran out — otherwise the last second is cut off.
                if t.drained && t.playing && t.sink.lead_frames() == 0 {
                    t.playing = false;
                    alloc_reset(mark);
                    if t.files.len() > 1 { t.skip(1); } else { t.seek_to_ms(0); }
                    mark = alloc_mark();
                    commit_scene(&t);
                    t.shown_s = -1;
                }
                // Redraw once a second while playing: the clock and the
                // progress bar are the only things that move.
                let secs = (t.position_ms() / 1000) as i64;
                if t.playing && secs != t.shown_s {
                    t.shown_s = secs;
                    alloc_reset(mark);
                    commit_scene(&t);
                }
                // Paused, there is nothing to keep up with — poll a quarter
                // as often and leave the core alone.
                host::sleep(if t.playing { TICK_MS } else { TICK_MS * 4 });
            }
            PollResult::WindowGone => { t.sink.close(); return; }
        }
    }
}
