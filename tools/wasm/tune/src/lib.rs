//! tune — the audio player for nopeekOS.
//!
//! Layout (top → bottom):
//!   body — the picture (video), or title + folder playlist (audio)
//!   seek — `Widget::Slider`, edge to edge; seeks on release
//!   bar  — play/pause · −10 s · +10 s · "24:10 / 46:02" · spacer · volume
//!          (volume opens a popover with its own slider)
//!
//! The player itself knows no formats. It pulls f32 frames out of a
//! [`source::Source`], hands them to [`sink::Sink`] (resample → 48 kHz S16
//! stereo → kernel audio mailbox), and the HDA driver takes it from there.
//! Adding FLAC or Opus later means adding a `Source`, not touching this
//! file — see `src/source.rs`.
//!
//! The loop below decodes ahead in small steps between event polls rather
//! than in one burst, so input stays responsive.

#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use nopeek_widgets::prefab;
use nopeek_widgets::style::{Padding, Radius, Spacing};
use nopeek_widgets::*;

mod aac;
mod demux;
mod host;
mod mp3;
// The MP4 container, tested host-side against ffmpeg.
mod mp4;
mod video;
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
static NPK_CAPS: [u8; 1] = [caps::READ | caps::RENDER | caps::CANVAS];

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

// ── Heap ──────────────────────────────────────────────────────────────
//
// Everything the player keeps on the heap is small: the playlist, decoder
// state, and a scene tree rebuilt each frame. The file bytes do not live
// here — they are claimed with `memory.grow` (see `file_arena`), so a long
// song never has to fit in a fixed heap. The heap must free: a video frame
// is about 0.5 MB and many arrive per second.
#[global_allocator]
static ALLOCATOR: nopeek_widgets::heap::Allocator = nopeek_widgets::heap::new();

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { log("[tune] panic!"); loop {} }

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
        // The heap grows memory too, so we are not the only caller of
        // `memory.grow`. That is fine: it returns the previous page count, the
        // fresh pages behind it are ours alone, and the old arena is simply
        // abandoned.
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
    /// Volume popover open.
    vol_open: bool,
    /// Where the seek slider is being dragged to, while it is. The time
    /// readout follows it; the seek itself waits for the release.
    scrub:   Option<u16>,
    /// Was playing when the seek drag started — resume after the seek.
    scrub_resume: bool,
    /// Last scene committed for a drag step (ms). The compositor moves the
    /// thumb itself; what the app redraws mid-drag is only the readout.
    slide_drawn_at: i64,
    /// Last pointer movement over the window (ms), for hiding the bar.
    motion_at: i64,
    /// The last movement was over the bar itself: keep it up.
    over_bar: bool,
    /// Whether the scene on screen shows the bar.
    controls_shown: bool,
    /// Seconds last drawn, so the loop only re-commits when the clock moves.
    shown_s: i64,
    /// The video half, when the file has one. `None` is an ordinary audio
    /// track.
    video: Option<video::Video>,
    /// Wall-clock tick at which the current video started, so the picture
    /// time is `now - started`. The audio clock is the better one and takes
    /// over when a film has sound; a silent file has nothing to sync to.
    video_t0: i64,
    /// Video time of the last frame handed to the compositor, for the UI.
    video_ms: i64,
    /// Priming of the audio track in ms — what precedes the first audible sample.
    audio_priming_ms: i64,
    /// Still filling the queue before the clock starts.
    video_buffering: bool,
}

const A_PLAY_PAUSE: u32 = 1;
const A_BACK:       u32 = 2;
const A_FORWARD:    u32 = 3;
const A_SEEK:       u32 = 4;
const A_VOL_TOGGLE: u32 = 5;
const A_VOL_CLOSE:  u32 = 6;
const A_VOL:        u32 = 7;
const A_MOTION:     u32 = 8;
const A_MOTION_BAR: u32 = 9;
const A_OPEN:       u32 = 10;
/// How long the bar stays over a playing video after the pointer stops.
const CONTROLS_HIDE_MS: i64 = 2500;
/// At most this often a new scene while a slider is being dragged.
const SLIDE_REDRAW_MS: i64 = 100;
const TRACK_BASE:   u32 = 1000;
/// What the two jump buttons move, in ms.
const JUMP_MS:      u64 = 10_000;
/// Anchor of the volume popover.
const NODE_VOL:     u32 = 1;
/// Playlist rows drawn at once. The scene is rebuilt every second while
/// playing, so a folder of two thousand files would re-encode and re-lay-out
/// two thousand rows per second for a clock that moved by one digit. The
/// window follows the current track; skipping still walks the whole folder.
const LIST_WINDOW:  usize = 200;
/// The single canvas. An app may have several; a player shows one picture.
const VIDEO_CANVAS: i32 = 0;

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
            vol_open: false,
            scrub: None,
            scrub_resume: false,
            slide_drawn_at: 0,
            motion_at: 0,
            over_bar: false,
            controls_shown: true,
            shown_s: -1,
            video: None,
            video_t0: 0,
            video_ms: 0,
            audio_priming_ms: 0,
            video_buffering: false,
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
        self.files = list_media(&self.dir);
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
        self.video = None;
        self.video_ms = 0;
        self.audio_priming_ms = 0;
        self.drained = false;
        self.error = None;
        let path = match self.full_path() { Some(p) => p, None => return };
        let size = self.files[self.idx].size as usize;
        let bytes = match fetch_file(&path, size) {
            Some(b) => b,
            None => { self.error = Some("cannot read file".to_string()); return; }
        };
        // One read of the container, two streams. Chosen by content, not by
        // extension — the same rule `source::open` uses for the audio decoder.
        let d = demux::open(bytes);
        let priming = d.audio_priming;

        if let Some(v) = d.video {
            self.video_t0 = host::ticks();
            self.video_buffering = true;
            self.video = Some(v);
        }

        if let Some(src) = d.audio {
            if !self.sink.ok() {
                self.error = Some("no free audio slot".to_string());
            } else {
                // Priming of the audio track, at its own rate. The video track of the
                // same file often has a different one; playing both raw desynchronises.
                let rate = src.info().rate.max(1);
                self.audio_priming_ms = (priming as i64 * 1000) / rate as i64;
                self.sink.restart(rate, host::ticks(), 0);
                self.src = Some(src);
            }
        }

        if self.video.is_none() && self.src.is_none() {
            // The reason belongs on screen: a fragmented MP4 and an unsupported
            // codec are different answers.
            self.error = Some(match d.video_error {
                Some(video::OpenError::Fragmented) =>
                    "fragmented MP4 (moof) — not supported".to_string(),
                Some(video::OpenError::NoVideo) => "no H.264 track".to_string(),
                Some(video::OpenError::NotMp4) => "not an MP4".to_string(),
                None => "unsupported format".to_string(),
            });
            return;
        }
        self.log_format();
        self.playing = play;
    }

    /// What was opened, once per file. The player shows no format line, so
    /// the serial log says what is being played.
    fn log_format(&self) {
        let mut m = String::from("[tune] ");
        if let Some(v) = self.video.as_ref() {
            m.push_str(&alloc::format!("H.264 {}x{}", v.width, v.height));
            if v.rotation != 0 { m.push_str(&alloc::format!(" gedreht {}\u{b0}", v.rotation)); }
            m.push_str(" · ");
        }
        match self.src.as_ref() {
            Some(s) => {
                let i = s.info();
                m.push_str(i.kind);
                if i.bitrate_kbps > 0 { m.push_str(&alloc::format!(" {} kbps", i.bitrate_kbps)); }
                m.push_str(&alloc::format!(" {} Hz {} ch", i.rate, i.channels));
            }
            None => m.push_str("ohne Ton"),
        }
        log(&m);
    }

    fn info_rate(&self) -> u32 { self.src.as_ref().map(|s| s.info().rate).unwrap_or(48_000) }

    /// The playback position as the viewer sees it, in ms.
    ///
    /// With sound, audio is the clock even when a picture runs alongside: it
    /// counts what was actually heard. A silent film has nothing to follow, so
    /// the wall clock is the clock.
    fn position_ms(&self) -> u64 {
        if self.src.is_some() { return self.audio_ms().max(0) as u64; }
        if self.video.is_some() { return self.video_ms.max(0) as u64; }
        self.sink.played_frames() * 1000 / sink::MIX_RATE as u64
    }

    /// Heard time on the presentation axis: what the ring has played, minus
    /// the priming before the first audible sample.
    ///
    /// The video track computes its `pts` against its own `edit_start`, so
    /// both land on the same zero — that is lip sync.
    fn audio_ms(&self) -> i64 {
        self.sink.played_frames() as i64 * 1000 / sink::MIX_RATE as i64
            - self.audio_priming_ms
    }

    fn duration_ms(&self) -> u64 {
        if let Some(v) = self.video.as_ref() { return v.duration_ms; }
        self.src.as_ref().map(|s| s.info().duration_ms()).unwrap_or(0)
    }

    /// Advance the picture and hand it to the compositor. Called once per
    /// turn of the loop, like `pump` — decoding a frame costs milliseconds,
    /// and it belongs where the event loop can see it.
    fn video_tick(&mut self) {
        if !self.playing || self.video.is_none() { return; }
        let has_audio = self.src.is_some();
        let now = host::ticks();

        // Build up a reserve before starting. Without sound the wall clock
        // is dragged along instead of stopped, otherwise buffering time
        // would count as lag. With sound the sink holds anyway until
        // something is fed.
        if self.video_buffering {
            if !has_audio { self.video_t0 = now - self.video_ms; }
            let at = self.video_ms;
            let flags = self.video.as_ref().map(|v| v.colour_flags).unwrap_or(0);
            let v = self.video.as_mut().unwrap();
            v.decode_step(at, video::FILL_DECODES);
            // Show the first frame meanwhile: a black box while buffering
            // looks like a failure.
            if let Some(f) = v.take_due(at) {
                let (ys, cs) = video::Video::strides(f);
                host::canvas_commit_yuv(VIDEO_CANVAS, &f.y, &f.u, &f.v, ys, cs,
                                        f.width as u32, f.height as u32, flags);
            }
            if self.video.as_ref().unwrap().primed(at) { self.video_buffering = false; }
            return;
        }

        // ── Read the clock ───────────────────────────────────────────────
        let ms = self.clock_ms(has_audio, now);
        self.video_ms = ms;
        let flags = self.video.as_ref().unwrap().colour_flags;

        // Show first, then decode — both in the same turn. The due frame
        // is already on screen when decoding starts, so a long decode never
        // delays a frame and the lead stays steady.
        {
            let v = self.video.as_mut().unwrap();
            if let Some(f) = v.take_due(ms) {
                let (ys, cs) = video::Video::strides(f);
                // Commit is not free: it copies the three planes across the
                // module boundary and re-rasters the window, both inside the host
                // call and so serial to decoding.
                host::canvas_commit_yuv(VIDEO_CANVAS, &f.y, &f.u, &f.v, ys, cs,
                                        f.width as u32, f.height as u32, flags);
            }
        }

        // Re-read the clock: the commit took time, and decoding is
        // governed by the lead, which has shrunk since.
        let ms = self.clock_ms(has_audio, host::ticks());
        self.video_ms = ms;

        self.video.as_mut().unwrap().decode_step(ms, video::PLAY_DECODES);

        // Read the clock again: decoding can take long, and the UI shows
        // the current position.
        self.video_ms = self.clock_ms(has_audio, host::ticks());

        if self.video.as_ref().unwrap().ended() { self.drained = true; }
    }

    /// Decode ahead until the mailbox holds `TARGET_LEAD_MS`. Runs between
    /// event polls, so each visit does a little and returns.
    ///
    /// For a film with sound this path feeds the clock the picture follows,
    /// so it must not run dry. `drained` is therefore set here and in the
    /// picture path independently — a film ends when both are done.
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
                if self.video.is_none() { self.drained = true; }
                break;
            }
            self.sink.push(block, n, channels);
            if !self.sink.flush() { break; }
        }
    }

    /// The time the picture follows: audio if there is sound, else the wall clock.
    fn clock_ms(&self, has_audio: bool, now: i64) -> i64 {
        if has_audio {
            self.sink.played_frames_at(now) as i64 * 1000 / sink::MIX_RATE as i64
                - self.audio_priming_ms
        } else {
            now - self.video_t0
        }
    }

    fn toggle(&mut self) {
        if self.src.is_none() && self.video.is_none() { self.load(true); return; }
        self.playing = !self.playing;
        // The bar stays up briefly after each toggle, otherwise it vanishes
        // under the finger that just pressed play.
        self.motion_at = host::ticks();
        if self.playing {
            // Stopped at the end: play starts over.
            if self.drained { self.seek_to_ms(0); }
            if self.src.is_some() {
                // Without this the first tick after a pause charges the whole
                // paused stretch to the speaker and reports a phantom underrun.
                self.sink.resume(host::ticks());
            } else {
                // The wall clock kept running, so the origin moves forward
                // instead of counting the pause.
                self.video_t0 = host::ticks() - self.video_ms;
            }
            return;
        }
        // Pausing with sound drops the lead. Otherwise the ring plays on
        // for half a second while the picture stands, and would be heard
        // twice on resume. Seeking does the same.
        if self.src.is_some() {
            let at = self.position_ms();
            self.seek_to_ms(at);
        }
    }

    /// Seek to `ms` on the presentation axis — both streams.
    ///
    /// Audio decides where it really lands (it can seek to any frame), and the
    /// picture follows to its nearest sync frame before that. The other way
    /// round, audio would be at a point the picture shows nothing for yet.
    fn seek_to_ms(&mut self, ms: u64) {
        let mut at = ms as i64;

        if self.src.is_some() {
            let rate = self.info_rate().max(1) as u64;
            // Add the priming: the presentation axis starts after it, the
            // track's frames before it.
            let src_frame =
                ((ms as i64 + self.audio_priming_ms).max(0) as u64) * rate / 1000;
            let landed = match self.src.as_mut() {
                Some(s) => s.seek(src_frame),
                None => 0,
            };
            let at48 = landed * sink::MIX_RATE as u64 / rate;
            self.sink.restart(rate as u32, host::ticks(), at48);
            at = (landed as i64 * 1000 / rate as i64) - self.audio_priming_ms;
        }

        if let Some(v) = self.video.as_mut() {
            let landed = v.seek(at);
            self.video_ms = landed;
            self.video_t0 = host::ticks() - landed;
            // After a seek the queue is empty — build a reserve first, or the
            // very spot that was sought to stutters.
            self.video_buffering = true;
        }
        self.drained = false;
    }

    fn skip(&mut self, delta: i64) {
        if self.files.is_empty() { return; }
        let n = self.files.len() as i64;
        let next = (self.idx as i64 + delta).rem_euclid(n);
        self.idx = next as usize;
        self.load(true);
    }

    /// The bar is always there for audio. Over a video it gives way to the
    /// picture while it plays, and comes back when paused, when the pointer
    /// moves, or while something on it is in use.
    fn controls_visible(&self, now: i64) -> bool {
        self.video.is_none() || !self.playing || self.vol_open || self.scrub.is_some()
            || self.over_bar || now - self.motion_at < CONTROLS_HIDE_MS
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
            t.files.get(t.idx).map(|f| strip_ext(&f.name)).unwrap_or_default(),
            t.error.clone().unwrap_or_default(),
        ),
    };

    // Nothing in the folder and nothing opened: no player to show, only
    // the way to a file.
    if t.files.is_empty() && t.src.is_none() && t.video.is_none() {
        return empty_state(t);
    }

    let body = match t.video.as_ref() {
        // The picture gets the whole area.
        Some(_) => Widget::Canvas {
            id: CanvasId(VIDEO_CANVAS as u32),
            // Not the video size. `measure_intrinsic` takes these numbers
            // as the column's minimum; a 2560-wide picture would drive the
            // layout of a smaller screen instead of fitting in. The real size
            // comes from `Flex(1)` and the compositor's contain-fit, which
            // scales the stored image independently.
            width: 320,
            height: 180,
            modifiers: alloc::vec![Modifier::Flex(1), Modifier::Background(Token::Page)],
        },
        None => audio_body(t, &title, &artist),
    };

    // While dragging, the readout shows where the thumb is, not where the
    // decoder is — that is the number the hand is looking for.
    let shown = match t.scrub {
        Some(v) if dur > 0 => dur * v as u64 / SLIDER_MAX as u64,
        _ => pos,
    };
    let value = match t.scrub {
        Some(v) => v,
        None if dur > 0 => (pos * SLIDER_MAX as u64 / dur).min(SLIDER_MAX as u64) as u16,
        None => 0,
    };
    let seek = Widget::Slider {
        value,
        on_change: ActionId(A_SEEK),
        modifiers: if dur > 0 { Vec::new() } else { alloc::vec![Modifier::Disabled(Vec::new())] },
    };

    let mut time = alloc::vec![
        Widget::Text { content: fmt_time(shown), style: TextStyle::Mono, modifiers: Vec::new() },
    ];
    if dur > 0 {
        time.push(Widget::Text {
            content: alloc::format!("/ {}", fmt_time(dur)),
            style: TextStyle::Mono,
            modifiers: alloc::vec![Modifier::Tint(Token::OnSurfaceMuted)],
        });
    }

    let mut vol_btn = prefab::icon_button(volume_icon(t.vol), 16, Some(ActionId(A_VOL_TOGGLE)), None);
    if let Widget::Row { modifiers, .. } | Widget::Column { modifiers, .. } | Widget::Stack { modifiers, .. } = &mut vol_btn {
        modifiers.push(Modifier::NodeId(NodeId(NODE_VOL)));
    }

    let bar = Widget::Row {
        children: alloc::vec![
            prefab::icon_button(
                if t.playing { IconId::Pause } else { IconId::Play },
                16, Some(ActionId(A_PLAY_PAUSE)), None),
            prefab::icon_button(IconId::ArrowCounterClockwise, 16, Some(ActionId(A_BACK)), None),
            prefab::icon_button(IconId::ArrowClockwise, 16, Some(ActionId(A_FORWARD)), None),
            Widget::Row { children: time, spacing: Spacing::Xs.as_u16(), align: Align::Center, modifiers: Vec::new() },
            Widget::Spacer { flex: 1 },
            vol_btn,
        ],
        spacing: Spacing::Xs.as_u16(),
        align: Align::Center,
        modifiers: alloc::vec![Modifier::PaddingXY { x: Padding::Sm.as_u16(), y: Padding::Xs.as_u16() }],
    };

    let controls = Widget::Column {
        children: alloc::vec![seek, bar],
        spacing: 0,
        align: Align::Stretch,
        modifiers: alloc::vec![
            Modifier::Background(Token::Surface),
            Modifier::OnMotion(ActionId(A_MOTION_BAR)),
        ],
    };
    let mut children = match t.video {
        // The bar lies over the picture, so showing and hiding it does
        // not resize the picture.
        Some(_) => {
            // A click on the picture is play/pause. The hit area sits in the
            // upper layer above the bar rather than on the picture: hit testing
            // takes the first child that hits, and the picture covers the whole
            // area — the buttons would be dead beneath it.
            let mut over = alloc::vec![Widget::Column {
                children: Vec::new(),
                spacing: 0,
                align: Align::Stretch,
                modifiers: alloc::vec![Modifier::Flex(1), Modifier::OnClick(ActionId(A_PLAY_PAUSE))],
            }];
            if t.controls_shown { over.push(controls); }
            let layers = alloc::vec![body, Widget::Column {
                children: over,
                spacing: 0,
                align: Align::Stretch,
                modifiers: Vec::new(),
            }];
            alloc::vec![Widget::Stack {
                children: layers,
                modifiers: alloc::vec![Modifier::Flex(1), Modifier::OnMotion(ActionId(A_MOTION))],
            }]
        }
        None => alloc::vec![body, controls],
    };
    if t.vol_open {
        children.push(Widget::Popover {
            anchor: NodeId(NODE_VOL),
            child: Box::new(Widget::Row {
                children: alloc::vec![
                    Widget::Slider {
                        value: (t.vol as u32 * SLIDER_MAX as u32 / 100) as u16,
                        on_change: ActionId(A_VOL),
                        modifiers: alloc::vec![Modifier::MinWidth(120)],
                    },
                    Widget::Text {
                        content: alloc::format!("{}", t.vol),
                        style: TextStyle::Mono,
                        modifiers: alloc::vec![Modifier::MinWidth(24)],
                    },
                ],
                spacing: Spacing::Sm.as_u16(),
                align: Align::Center,
                modifiers: alloc::vec![
                    Modifier::Padding(Padding::Sm.as_u16()),
                    Modifier::Background(Token::SurfaceElevated),
                    Modifier::Rounded(Radius::Sm.as_u8()),
                ],
            }),
            on_dismiss: ActionId(A_VOL_CLOSE),
            modifiers: Vec::new(),
        });
    }

    Widget::Column {
        children,
        spacing: 0,
        align: Align::Stretch,
        modifiers: alloc::vec![Modifier::Flex(1)],
    }
}

fn empty_state(t: &Tune) -> Widget {
    let mut children = alloc::vec![
        Widget::Spacer { flex: 1 },
        Widget::Icon { id: IconId::PlayCircle, size: 48, modifiers: alloc::vec![Modifier::Tint(Token::OnSurfaceMuted)] },
        Widget::Text { content: "Nothing to play".to_string(), style: TextStyle::Heading, modifiers: Vec::new() },
    ];
    if let Some(e) = t.error.as_ref() {
        children.push(Widget::Text { content: e.clone(), style: TextStyle::Muted, modifiers: Vec::new() });
    }
    children.push(prefab::button("Open file…", prefab::ButtonStyle::Primary, ActionId(A_OPEN)));
    children.push(Widget::Spacer { flex: 1 });
    Widget::Column {
        children,
        spacing: Spacing::Sm.as_u16(),
        align: Align::Center,
        modifiers: alloc::vec![Modifier::Flex(1)],
    }
}

/// Without a picture the area belongs to the folder: what is playing on
/// top, every track below it.
fn audio_body(t: &Tune, title: &str, artist: &str) -> Widget {
    let head = Widget::Column {
        children: alloc::vec![
            Widget::Text { content: title.to_string(), style: TextStyle::Title, modifiers: Vec::new() },
            Widget::Text { content: artist.to_string(), style: TextStyle::Muted, modifiers: Vec::new() },
        ],
        spacing: 0,
        align: Align::Start,
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

    Widget::Column {
        children: alloc::vec![head, Widget::Divider, list],
        spacing: Spacing::Xs.as_u16(),
        align: Align::Stretch,
        modifiers: alloc::vec![Modifier::Flex(1)],
    }
}

fn volume_icon(v: u8) -> IconId {
    if v == 0 { IconId::SpeakerX } else if v <= 50 { IconId::SpeakerLow } else { IconId::SpeakerHigh }
}

fn commit_scene(t: &mut Tune) {
    t.controls_shown = t.controls_visible(host::ticks());
    match wire::encode(&render(t)) {
        Ok(bytes) => { if host::scene_commit(&bytes) < 0 { log("[tune] commit failed"); } }
        Err(_) => log("[tune] encode failed"),
    }
}

/// The system file dialog, starting in the folder tune is looking at.
fn open_dialog(t: &Tune) {
    let start = if t.dir.is_empty() { read_home_dir() } else { t.dir.clone() };
    if host::pick_open(&start) < 0 { log("[tune] file dialog unavailable"); }
}

/// A mid-drag redraw, throttled: every scene is a full layout and raster
/// in the compositor, and a drag sends a step with every mouse packet.
fn slide_redraw(t: &mut Tune) -> Outcome {
    let now = host::ticks();
    if now - t.slide_drawn_at < SLIDE_REDRAW_MS { return Outcome::Idle; }
    t.slide_drawn_at = now;
    Outcome::Render
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
        Event::Chord { letter: b'o', shift: false, alt: false } => { open_dialog(t); Outcome::Idle }
        Event::Picked { path, .. } => {
            // Empty = cancelled.
            if path.is_empty() { return Outcome::Idle; }
            let (_, name) = split_path(&path);
            if !is_media(name) {
                t.error = Some(alloc::format!("{} is not a media file", name));
                return Outcome::Render;
            }
            t.error = None;
            t.point_at(&path);
            t.load(true);
            Outcome::Render
        }
        Event::Open(_) => {
            // Already running and asked to open another file (loft
            // double-click): switch tracks rather than spawning a twin.
            t.point_at(payload);
            t.load(true);
            Outcome::Render
        }
        // Grabbing pauses; release seeks and resumes if it was playing —
        // nothing runs away under the hand meanwhile.
        Event::Slide { action: ActionId(A_SEEK), value, done } => {
            if t.scrub.is_none() && !done {
                t.scrub_resume = t.playing;
                if t.playing { t.toggle(); }
            }
            if done {
                t.scrub = None;
                let dur = t.duration_ms();
                if dur > 0 { t.seek_to_ms(dur * value as u64 / SLIDER_MAX as u64); }
                if core::mem::take(&mut t.scrub_resume) && !t.playing { t.toggle(); }
                Outcome::Render
            } else {
                t.scrub = Some(value);
                slide_redraw(t)
            }
        }
        // Volume follows the hand: a level is cheap to set and the ear is
        // the feedback.
        Event::Slide { action: ActionId(A_VOL), value, done } => {
            t.set_volume((value as u32 * 100 / SLIDER_MAX as u32) as u8);
            if done { Outcome::Render } else { slide_redraw(t) }
        }
        Event::Action(ActionId(id)) => {
            if id == A_PLAY_PAUSE { t.toggle(); return Outcome::Render; }
            if id == A_BACK {
                let p = t.position_ms().saturating_sub(JUMP_MS);
                t.seek_to_ms(p);
                return Outcome::Render;
            }
            if id == A_FORWARD { let p = t.position_ms() + JUMP_MS; t.seek_to_ms(p); return Outcome::Render; }
            // Movement alone redraws nothing; the loop decides whether the
            // bar appears or stays because of it.
            if id == A_MOTION || id == A_MOTION_BAR {
                t.motion_at = host::ticks();
                t.over_bar = id == A_MOTION_BAR;
                return if t.controls_visible(t.motion_at) != t.controls_shown {
                    Outcome::Render
                } else {
                    Outcome::Idle
                };
            }
            if id == A_OPEN { open_dialog(t); return Outcome::Idle; }
            if id == A_VOL_TOGGLE { t.vol_open = !t.vol_open; return Outcome::Render; }
            if id == A_VOL_CLOSE { t.vol_open = false; return Outcome::Render; }
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

/// What the folder list accepts: audio and video in one place, so a new
/// format is added in one spot.
fn is_media(name: &str) -> bool {
    // `.m4a` is MP4 with an audio track and no picture — same demuxer,
    // so it belongs here.
    let lower = {
        let mut s = String::with_capacity(name.len());
        for c in name.chars() { s.push(c.to_ascii_lowercase()); }
        s
    };
    source::is_audio(name)
        || video::is_video(name)
        || [".m4a", ".aac"].iter().any(|e| lower.ends_with(e))
}

fn list_media(dir: &str) -> Vec<Track> {
    let buf_ptr = &raw mut LIST_BUF as *mut u8;
    let n = host::fs_list(dir, buf_ptr, LIST_BUF_SIZE);
    let mut out: Vec<Track> = Vec::new();
    if n <= 0 { return out; }
    let slice = unsafe { core::slice::from_raw_parts(buf_ptr as *const u8, n as usize) };
    for e in nopeek_widgets::fs::list_entries(slice) {
        if e.is_dir || !is_media(e.name) { continue; }
        out.push(Track { name: e.name.to_string(), size: e.size });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
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
    // Log the version first, so every log says which build it comes from.
    log(concat!("[tune] version ", env!("CARGO_PKG_VERSION")));
    let mut t = Tune::new();
    commit_scene(&mut t);      // window appears before the first fetch
    let autoplay = t.opened_with_file;
    t.load(autoplay);
    commit_scene(&mut t);

    loop {
        // Clock and pump run on EVERY turn, not only when the poll came up
        // empty: a stream of mouse-move events would otherwise starve the
        // decoder for as long as the hand keeps moving, and the mailbox
        // holds 600 ms.
        let now = host::ticks();
        // `playing` means "the player runs"; the sink needs "it is
        // sounding". For a silent film these differ: `submitted` stays 0,
        // the wall clock advances `played`, and the underrun condition would
        // be true on every tick.
        t.sink.tick(now, t.playing && t.src.is_some());
        t.pump();
        t.video_tick();

        match poll_event() {
            PollResult::Event(ev) => {
                let plen = match &ev { Event::Open(s) => copy_payload(s), _ => 0 };
                let outcome = handle(&mut t, ev, payload_str(plen));
                match outcome {
                    Outcome::Idle => {}
                    Outcome::Render => { commit_scene(&mut t); t.shown_s = -1; }
                    Outcome::Exit => { t.sink.close(); host::close_widget(); return; }
                }
            }
            PollResult::Empty => {
                // The track ends when the mailbox has drained, not when the
                // decoder ran out — otherwise the last second is cut off.
                if t.drained && t.playing && (t.video.is_some() || t.sink.lead_frames() == 0) {
                    t.playing = false;
                    // A film stops at the end; music continues through the
                    // folder.
                    if t.video.is_none() {
                        if t.files.len() > 1 { t.skip(1); } else { t.seek_to_ms(0); }
                    }
                    commit_scene(&mut t);
                    t.shown_s = -1;
                }
                // Redraw once a second while playing: the clock and the
                // progress bar are the only things that move.
                let secs = (t.position_ms() / 1000) as i64;
                if t.playing && secs != t.shown_s {
                    t.shown_s = secs;
                        commit_scene(&mut t);
                } else if t.controls_visible(host::ticks()) != t.controls_shown {
                    commit_scene(&mut t);
                }
                // Paused: nothing to keep up with — poll a quarter as often.
                // Playing video: once the reserve is full, sleep until the next due
                // frame (at most one tick) — a fixed sleep can push a turn past the
                // frame period and drop that frame. Below the reserve mark every
                // millisecond goes to decoding.
                let nap = match (t.playing, t.video.as_ref()) {
                    (true, Some(v)) if v.may_wait(t.video_ms) =>
                        v.next_due_in(t.video_ms).clamp(1, TICK_MS as i64) as i32,
                    (true, Some(_)) => 1,
                    (true, None)    => TICK_MS,
                    (false, _)      => TICK_MS * 4,
                };
                host::sleep(nap);
            }
            PollResult::WindowGone => { t.sink.close(); return; }
        }
    }
}
