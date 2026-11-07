//! Screen capture into a guest (`npk.capture`).
//!
//! The guest's screen-cast portal asks to share the screen; the host picker
//! (the `pick` module in screen mode, driven by real input only) is the
//! consent. While the session runs, the composed screen is scaled by the
//! GPU module into buffers the guest allocated, whenever the screen changed
//! and the guest has handed a buffer back, and the guest is told which
//! buffer holds the frame. Frames only flow to the guest; the guest only
//! names buffers.
//!
//! A guest buffer becomes a write target only if it is a blob of this
//! guest's own virtio-gpu device, laid out linearly, not named twice, and
//! the stream's rows fit inside it at the pitch the guest gave. The first
//! frame of a session is also composed on the CPU and compared bit for bit
//! (as `gpu_scene` does for the desktop); a difference ends the session.
//! Without a GPU module composing scenes a request is refused: there is no
//! CPU path yet.
//!
//! Everything a session took goes back when it ends: its buffers are
//! forgotten by gpudev, which unbinds them (and takes back the IOMMU
//! grant) after the last frame written into them.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

use crate::framebuffer::FbInfo;
use crate::gpu::gpudev;
use crate::microvm::devices::virtio_console_pci as console;
use crate::shade::buffers::{ForeignView, PixelBuffer};
use crate::shade::scene::{self, Fit, InPlace, Item, Rect, Scene, Source, Sources, Yuv};

/// Buffers a guest may name per session.
const MAX_SLOTS: usize = 8;
/// GPU frames in a row that failed before the session ends.
const FAILURE_LIMIT: u32 = 4;
/// Refused or dropped requests that say why in the log.
const LOGGED: u64 = 32;
/// Differing pixels the verification lists.
const DIFFS_SHOWN: usize = 8;

// Why a session ended (`CAP_ENDED`).
pub const END_CANCELLED: u8 = 1;
pub const END_GUEST: u8 = 2;
pub const END_HOST: u8 = 3;
pub const END_NO_GPU: u8 = 4;
pub const END_NO_PICKER: u8 = 5;
pub const END_REPLACED: u8 = 6;
pub const END_VERIFY: u8 = 7;
pub const END_GPU_FAILED: u8 = 8;

// Why a buffer was refused (`CAP_REFUSED`).
pub const REFUSE_NOT_STARTED: u8 = 1;
pub const REFUSE_SLOT: u8 = 2;
pub const REFUSE_UNKNOWN: u8 = 3;
pub const REFUSE_SIZE: u8 = 4;
pub const REFUSE_PITCH: u8 = 5;
pub const REFUSE_TOO_SMALL: u8 = 6;
pub const REFUSE_LAYOUT: u8 = 7;
pub const REFUSE_IN_USE: u8 = 8;
pub const REFUSE_UNBOUND: u8 = 9;

/// `CAP_FRAME` flag: the frame is the known test frame (its checksum is in
/// the host's log).
const FRAME_TEST: u8 = 1;

/// A buffer as the guest names it (`CAP_BUFFER`).
pub struct GuestBuffer {
    pub slot: u8,
    pub resource: u32,
    pub offset: u32,
    pub stride: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Phase {
    Idle,
    /// The picker is up (or about to be).
    Asking,
    Running,
}

struct Slot {
    view: ForeignView,
    resource: u32,
    /// The host may write it: the guest handed it over (or never took it).
    host: bool,
}

struct Session {
    phase: Phase,
    /// The guest's largest acceptable stream size, 0 = any.
    max: (u32, u32),
    /// The stream size, fixed when the user chose.
    width: u32,
    height: u32,
    /// The picker's window while asking, 0 before it is open.
    picker: u32,
    slots: [Option<Slot>; MAX_SLOTS],
    seq: u32,
    verified: bool,
    failures: u32,
    /// This session's test frame checksum is in the log.
    test_logged: bool,
}

static SESSION: Mutex<Session> = Mutex::new(Session {
    phase: Phase::Idle, max: (0, 0), width: 0, height: 0, picker: 0,
    slots: [const { None }; MAX_SLOTS], seq: 0, verified: false, failures: 0, test_logged: false,
});

/// Every frame is the known test frame (`gpu capture test`, until `off`).
static TEST_MODE: AtomicBool = AtomicBool::new(false);
/// The screen changed since the last frame sent.
static DIRTY: AtomicBool = AtomicBool::new(false);
/// A START waits for core 0 to open the picker.
static ASK: AtomicBool = AtomicBool::new(false);
/// A picker window core 0 has to close (the session it asked for is gone).
static CLOSE_PICKER: AtomicU32 = AtomicU32::new(0);
/// Serials of buffers a session made gpudev bind, until gpudev let go.
static BOUND: Mutex<Vec<u64>> = Mutex::new(Vec::new());

static FRAMES: AtomicU64 = AtomicU64::new(0);
static FAILED: AtomicU64 = AtomicU64::new(0);
static REFUSED: AtomicU64 = AtomicU64::new(0);
static DROPPED: AtomicU64 = AtomicU64::new(0);
static SESSIONS: AtomicU64 = AtomicU64::new(0);

/// The compositor composed something: a running session sends a frame.
pub fn note_damage() {
    DIRTY.store(true, Ordering::Release);
}

/// vCPU: the guest asks to share the screen.
pub fn guest_start(max_w: u32, max_h: u32) {
    let mut s = SESSION.lock();
    match s.phase {
        Phase::Asking => {
            refused_log(format_args!("a second START while the picker is up, ignored"));
            return;
        }
        Phase::Running => end(&mut s, Some(END_REPLACED)),
        Phase::Idle => {}
    }
    if !gpudev::scenes_offered() {
        crate::kprintln!("[capture] the guest asks to share the screen: refused, no GPU module composes scenes");
        console::send_capture(console::CAP_ENDED, &[END_NO_GPU]);
        return;
    }
    s.phase = Phase::Asking;
    s.max = (max_w, max_h);
    s.picker = 0;
    drop(s);
    crate::kprintln!("[capture] the guest asks to share the screen (at most {}x{}), asking the user",
        max_w, max_h);
    ASK.store(true, Ordering::Release);
    crate::intent::wake_shell();
}

/// vCPU: the guest names a buffer for the stream.
pub fn guest_buffer(b: GuestBuffer) {
    let slot = b.slot;
    let r = accept(b);
    match r {
        Ok(()) => {
            console::send_capture(console::CAP_ACCEPTED, &[slot]);
        }
        Err(why) => {
            REFUSED.fetch_add(1, Ordering::Relaxed);
            refused_log(format_args!("buffer {} refused: {}", slot, refusal_name(why)));
            console::send_capture(console::CAP_REFUSED, &[slot, why]);
        }
    }
}

fn accept(b: GuestBuffer) -> Result<(), u8> {
    // The blob is looked up before the session lock: releasing a blob holds
    // the device lock and then takes the session (`forget_blob`).
    let blob = crate::microvm::devices::gpu_backend::lock().guest_blob(b.resource);
    let mut s = SESSION.lock();
    if s.phase != Phase::Running {
        return Err(REFUSE_NOT_STARTED);
    }
    let i = b.slot as usize;
    if i >= MAX_SLOTS {
        return Err(REFUSE_SLOT);
    }
    if b.width != s.width || b.height != s.height {
        return Err(REFUSE_SIZE);
    }
    if b.stride % 4 != 0 || (b.stride as u64) < b.width as u64 * 4 || b.offset % 4 != 0 {
        return Err(REFUSE_PITCH);
    }
    let pixels = blob.ok_or(REFUSE_UNKNOWN)?;
    let view = ForeignView { pixels, offset: b.offset, pitch: b.stride, width: b.width, height: b.height };
    if !view.fits() {
        return Err(REFUSE_TOO_SMALL);
    }
    if view.pixels.gpu_only() {
        return Err(REFUSE_LAYOUT);
    }
    let serial = view.pixels.serial;
    if s.slots.iter().enumerate().any(|(k, o)| k != i && o.as_ref().is_some_and(|o| o.view.pixels.serial == serial)) {
        return Err(REFUSE_IN_USE);
    }
    if let Some(old) = s.slots[i].take() {
        forget(old);
    }
    if gpudev::bo_for_pages(&view).is_none() {
        return Err(REFUSE_UNBOUND);
    }
    BOUND.lock().push(serial);
    s.slots[i] = Some(Slot { view, resource: b.resource, host: true });
    Ok(())
}

/// vCPU: the guest hands buffer `slot` back for writing.
pub fn guest_queue(slot: u8) {
    let mut s = SESSION.lock();
    let ok = s.phase == Phase::Running
        && s.slots.get_mut(slot as usize).and_then(|o| o.as_mut()).is_some_and(|o| {
            o.host = true;
            true
        });
    drop(s);
    if !ok {
        DROPPED.fetch_add(1, Ordering::Relaxed);
        refused_log(format_args!("QUEUE of buffer {} without a session or buffer, dropped", slot));
    }
}

/// vCPU: the guest ends the session.
pub fn guest_stop() {
    let mut s = SESSION.lock();
    if s.phase != Phase::Idle {
        end(&mut s, Some(END_GUEST));
    }
}

/// The guest's channel or the VM is gone: the session ends unannounced.
pub fn guest_gone() {
    let mut s = SESSION.lock();
    if s.phase != Phase::Idle {
        end(&mut s, None);
    }
}

/// The host ends the session (`gpu capture stop`).
pub fn host_stop() -> bool {
    let mut s = SESSION.lock();
    if s.phase == Phase::Idle {
        return false;
    }
    end(&mut s, Some(END_HOST));
    true
}

/// `gpu capture test [off]`: every frame is the known test frame (its
/// checksum logged once per session), or the screen again.
pub fn set_test(on: bool) {
    TEST_MODE.store(on, Ordering::Release);
    DIRTY.store(true, Ordering::Release);
}

/// A frame the guest named a capture-port message too short for its type.
pub fn note_dropped(msg_type: u8, len: usize) {
    DROPPED.fetch_add(1, Ordering::Relaxed);
    refused_log(format_args!("message {:#04x} with {} payload bytes, too short, dropped", msg_type, len));
}

/// The guest gives the blob with `serial` back (under the GPU device lock;
/// the caller makes gpudev forget it).
pub fn forget_blob(serial: u64) {
    let mut s = SESSION.lock();
    for slot in s.slots.iter_mut() {
        if slot.as_ref().is_some_and(|o| o.view.pixels.serial == serial) {
            *slot = None;
        }
    }
}

/// The user answered the picker (`pick`, screen mode).
pub fn picked(picker: u32, chosen: bool) {
    let mut s = SESSION.lock();
    // 0: the answer came before `open_picker` noted the window.
    if s.phase != Phase::Asking || (s.picker != 0 && s.picker != picker) {
        return;
    }
    // The picker is closing itself.
    s.picker = 0;
    if !chosen {
        crate::kprintln!("[capture] the user did not share the screen");
        end(&mut s, Some(END_CANCELLED));
        return;
    }
    let fb = crate::framebuffer::get_info();
    let (w, h) = stream_size(fb.width, fb.height, s.max);
    s.phase = Phase::Running;
    s.width = w;
    s.height = h;
    s.seq = 0;
    s.verified = false;
    s.failures = 0;
    s.test_logged = false;
    drop(s);
    SESSIONS.fetch_add(1, Ordering::Relaxed);
    DIRTY.store(true, Ordering::Release);
    crate::kprintln!("[capture] the user shares the screen: {}x{} as {}x{}", fb.width, fb.height, w, h);
    crate::audit::record(&crate::audit::GUEST, crate::audit::Kind::DeviceOn, "screen share");
    let mut p = [0u8; 4];
    p[0..2].copy_from_slice(&(w as u16).to_le_bytes());
    p[2..4].copy_from_slice(&(h as u16).to_le_bytes());
    console::send_capture(console::CAP_STARTED, &p);
}

/// The screen `sw` x `sh` scaled down to fit `max` (0 = no limit), aspect
/// kept, even sizes for the guest's encoder.
fn stream_size(sw: u32, sh: u32, max: (u32, u32)) -> (u32, u32) {
    let (mw, mh) = (if max.0 == 0 { sw } else { max.0.min(sw) }, if max.1 == 0 { sh } else { max.1.min(sh) });
    let (w, h) = if mw as u64 * sh as u64 <= mh as u64 * sw as u64 {
        (mw, (sh as u64 * mw as u64 / sw.max(1) as u64) as u32)
    } else {
        ((sw as u64 * mh as u64 / sh.max(1) as u64) as u32, mh)
    };
    ((w & !1).max(2), (h & !1).max(2))
}

/// Core 0, from the VM poll: open or close the picker, and send a frame when
/// the screen changed and the guest has a buffer free.
pub fn sync() {
    let close = CLOSE_PICKER.swap(0, Ordering::AcqRel);
    if close != 0 {
        crate::shade::close_window(crate::shade::window::WindowId(close));
    }
    if ASK.swap(false, Ordering::AcqRel) {
        open_picker();
    }
    if DIRTY.load(Ordering::Acquire) && buffer_free() {
        capture_frame();
    }
    reap_bound();
}

fn open_picker() {
    let requester = crate::shade::focused_surface_id()
        .filter(|&w| crate::microvm::cpu::output_of_window(w).is_some())
        .unwrap_or_else(|| crate::microvm::cpu::output_window(0));
    let app = crate::microvm::app::current().name;
    let win = crate::wasm::host_core::open_screen_pick(requester, app);
    let mut s = SESSION.lock();
    if s.phase != Phase::Asking {
        // The guest gave up meanwhile.
        if let Some(w) = win {
            CLOSE_PICKER.store(w, Ordering::Release);
        }
        return;
    }
    match win {
        Some(w) => s.picker = w,
        None => {
            crate::kprintln!("[capture] the picker could not be opened, the request is refused");
            end(&mut s, Some(END_NO_PICKER));
        }
    }
}

/// End the session: every buffer goes back, the picker closes, the guest is
/// told why (unless it is gone).
fn end(s: &mut Session, reason: Option<u8>) {
    if s.phase == Phase::Asking && s.picker != 0 {
        CLOSE_PICKER.store(s.picker, Ordering::Release);
    }
    let was = s.phase;
    if was == Phase::Running {
        crate::audit::record(&crate::audit::GUEST, crate::audit::Kind::DeviceOff, "screen share");
    }
    s.phase = Phase::Idle;
    s.picker = 0;
    let mut n = 0;
    for slot in s.slots.iter_mut() {
        if let Some(o) = slot.take() {
            forget(o);
            n += 1;
        }
    }
    crate::kprintln!("[capture] session ended ({}), {} buffer(s) given back, {} frame(s) sent in all",
        reason.map_or("the guest is gone", end_name), n, FRAMES.load(Ordering::Relaxed));
    if let Some(r) = reason
        && was != Phase::Idle
    {
        console::send_capture(console::CAP_ENDED, &[r]);
    }
}

fn forget(o: Slot) {
    gpudev::forget_pages(o.view.pixels.serial);
}

/// Drop the serials gpudev has let go of.
fn reap_bound() {
    let mut b = BOUND.lock();
    if !b.is_empty() {
        b.retain(|&serial| !gpudev::pages_released(serial));
    }
}

/// The composed screen as a capture's only source.
struct Screen<'a> {
    bo: Option<u32>,
    px: &'a [u32],
    width: u32,
    height: u32,
}

impl Sources for Screen<'_> {
    fn with_source(&self, _src: Source, f: &mut dyn FnMut(&[u32], u32, u32)) {
        f(self.px, self.width, self.height);
    }

    fn in_place(&self, _src: Source) -> Option<InPlace> {
        Some(InPlace { bo: self.bo, key: self.px.as_ptr() as u64, width: self.width, height: self.height })
    }
}

/// The one-item scene of a capture: the whole screen scaled into the target.
fn capture_scene(w: u32, h: u32) -> Scene {
    Scene { items: alloc::vec![Item::Content {
        src: Source::Surface(u32::MAX), dst: Rect::new(0, 0, w, h), radius: 0, fit: Fit::Scale, yuv: Yuv::None }] }
}

/// A session runs and the guest has handed a buffer over.
fn buffer_free() -> bool {
    let s = SESSION.lock();
    s.phase == Phase::Running && s.slots.iter().any(|o| o.as_ref().is_some_and(|o| o.host))
}

/// Send one frame if the guest has a buffer free. The scene is handed over
/// under the session lock, so a buffer the guest gives back meanwhile is
/// unbound only after the scene that writes it; the wait is outside it.
fn capture_frame() {
    let done = crate::framebuffer::with_fb(|fb| {
        let info = *fb.info();
        let mut s = SESSION.lock();
        if s.phase != Phase::Running {
            DIRTY.store(false, Ordering::Release);
            return None;
        }
        let i = s.slots.iter().position(|o| o.as_ref().is_some_and(|o| o.host))?;
        DIRTY.store(false, Ordering::Release);
        let view = s.slots[i].as_ref()?.view.clone();
        let test = TEST_MODE.load(Ordering::Acquire);
        let log_crc = test && !core::mem::replace(&mut s.test_logged, true);
        let (w, h, verify) = (s.width, s.height, !s.verified);
        let pattern = if test { test_pattern(info.width, info.height) } else { PixelBuffer::empty() };
        let screen = (!test).then(|| fb.front_ptr());
        let job = submit(&pattern, screen, &info, &view, w, h);
        drop(s);
        let r = job.and_then(|seq| settle(seq, &pattern, screen, &info, &view, w, h, verify, log_crc));
        Some((i, view.pixels.serial, test, r))
    });
    let Some(Some((i, serial, test, r))) = done else { return };
    let mut s = SESSION.lock();
    let same = s.phase == Phase::Running
        && s.slots[i].as_ref().is_some_and(|o| o.view.pixels.serial == serial);
    if !same {
        // The session or the buffer went while the frame was made.
        return;
    }
    match r {
        Ok(()) => {
            s.verified = true;
            s.failures = 0;
            s.seq = s.seq.wrapping_add(1);
            let seq = s.seq;
            if let Some(o) = s.slots[i].as_mut() {
                o.host = false;
            }
            drop(s);
            FRAMES.fetch_add(1, Ordering::Relaxed);
            let mut p = [0u8; 8];
            p[0] = i as u8;
            p[1] = if test { FRAME_TEST } else { 0 };
            p[4..8].copy_from_slice(&seq.to_le_bytes());
            console::send_capture(console::CAP_FRAME, &p);
        }
        Err(Failure::Differs) => end(&mut s, Some(END_VERIFY)),
        Err(Failure::Gpu(why) | Failure::NotHanded(why)) => {
            DIRTY.store(true, Ordering::Release);
            if FAILED.fetch_add(1, Ordering::Relaxed) < LOGGED {
                crate::kprintln!("[capture] frame not sent: {}", why);
            }
            s.failures += 1;
            if s.failures >= FAILURE_LIMIT {
                end(&mut s, Some(END_GPU_FAILED));
            }
        }
    }
}

enum Failure {
    NotHanded(&'static str),
    Gpu(&'static str),
    Differs,
}

/// The source pixels: the screen at `screen` (a shadow, `pitch * height`
/// bytes), or `pattern` when that is None.
fn source_px<'a>(pattern: &'a PixelBuffer, screen: Option<*mut u8>, info: &FbInfo) -> &'a [u32] {
    match screen {
        // SAFETY: a shadow of `pitch * height` bytes, identity-mapped, kept
        // by the framebuffer console whose lock the caller holds; nothing
        // composes into it meanwhile, and it is only read.
        Some(p) => unsafe {
            core::slice::from_raw_parts(p as *const u32, info.pitch as usize / 4 * info.height as usize)
        },
        None => pattern,
    }
}

/// Hand the capture scene to the module: the source scaled whole into
/// `view`. Its sequence number.
fn submit(pattern: &PixelBuffer, screen: Option<*mut u8>, info: &FbInfo, view: &ForeignView, w: u32, h: u32)
          -> Result<u32, Failure> {
    let src_bo = match screen {
        Some(p) => gpudev::shadow_bo(p as u64),
        None => gpudev::bo_for(pattern.phys(), pattern.len() as u64 * 4, pattern.width(), pattern.height(),
            pattern.width() * 4),
    };
    let src_bo = src_bo.ok_or(Failure::NotHanded("the screen is not a buffer the module has"))?;
    let src = Screen { bo: Some(src_bo), px: source_px(pattern, screen, info), width: info.width, height: info.height };
    let tinfo = FbInfo { addr: 0, pitch: view.pitch, width: w, height: h, bpp: 32 };
    let _pin = gpudev::ScenePin::new();
    let tid = gpudev::bo_for_pages(view).ok_or(Failure::NotHanded("the guest buffer could not be bound"))?;
    let (payload, srcs) = crate::shade::gpu_scene::encode(&capture_scene(w, h), &src, &tinfo, &[Rect::new(0, 0, w, h)])
        .ok_or(Failure::NotHanded("the scene could not be encoded"))?;
    gpudev::submit_scene(tid, payload, &srcs).ok_or(Failure::NotHanded("the module took no scene"))
}

/// Wait for capture `seq`. With `verify`, the frame is also composed on the
/// CPU and compared bit for bit; with `log_crc`, the checksum of what the
/// guest receives is logged.
#[allow(clippy::too_many_arguments)]
fn settle(seq: u32, pattern: &PixelBuffer, screen: Option<*mut u8>, info: &FbInfo, view: &ForeignView, w: u32,
          h: u32, verify: bool, log_crc: bool) -> Result<(), Failure> {
    let status = gpudev::wait_scene_bound(seq);
    if status != 0 {
        return Err(Failure::Gpu(if status == -62 { "the module did not finish in time" } else { "the module failed it" }));
    }
    if !verify && !log_crc {
        return Ok(());
    }
    view.clflush_rect((0, 0, w, h));
    let mut got = PixelBuffer::new(w, h);
    if got.is_empty() {
        return Err(Failure::NotHanded("no memory to read the frame back"));
    }
    view.copy_into(&mut got, (0, 0, w, h));
    if verify {
        let px = source_px(pattern, screen, info);
        let tight;
        let px = if screen.is_some() && info.pitch != info.width * 4 {
            tight = tight_copy(px, info);
            &tight[..]
        } else {
            px
        };
        let cpu_src = Screen { bo: None, px, width: info.width, height: info.height };
        let cpu = PixelBuffer::new(w, h);
        if cpu.is_empty() {
            return Err(Failure::NotHanded("no memory to verify the frame"));
        }
        let cinfo = cpu.info();
        scene::execute(&capture_scene(w, h), &cpu_src, cpu.ptr(), &cinfo, &[Rect::new(0, 0, w, h)]);
        let mut diffs = 0u64;
        let mut shown = Vec::new();
        for (k, (c, g)) in cpu.iter().zip(got.iter()).enumerate() {
            if c != g {
                diffs += 1;
                if shown.len() < DIFFS_SHOWN {
                    shown.push((k as u32 % w, k as u32 / w, *c, *g));
                }
            }
        }
        if diffs != 0 {
            crate::kprintln!("[capture] verification: {} of {} pixels differ, the session ends", diffs,
                w as u64 * h as u64);
            for (x, y, c, g) in shown {
                crate::kprintln!("[capture]   ({}, {}) CPU {:08x} GPU {:08x}", x, y, c, g);
            }
            return Err(Failure::Differs);
        }
        crate::kprintln!("[capture] verification: the GPU frame equals the CPU frame ({}x{}, 0 pixels differ)", w, h);
    }
    if log_crc {
        let bytes: Vec<u8> = got.iter().flat_map(|p| p.to_le_bytes()).collect();
        crate::kprintln!("[capture] test frame {}x{} sent, crc32 {:08x}", w, h, crate::storage::gpt::crc32(&bytes));
    }
    Ok(())
}

/// The screen's rows without the pitch's padding.
fn tight_copy(px: &[u32], info: &FbInfo) -> Vec<u32> {
    let stride = info.pitch as usize / 4;
    let mut v = Vec::with_capacity(info.width as usize * info.height as usize);
    for y in 0..info.height as usize {
        v.extend_from_slice(&px[y * stride..y * stride + info.width as usize]);
    }
    v
}

/// A screen-sized frame every run makes the same: diagonal colour bands
/// with a grid, so scaling errors show.
fn test_pattern(w: u32, h: u32) -> PixelBuffer {
    let mut b = PixelBuffer::new(w, h);
    for (k, p) in b.iter_mut().enumerate() {
        let (x, y) = (k as u32 % w, k as u32 / w);
        let grid = x % 64 == 0 || y % 64 == 0;
        *p = if grid {
            0x00FF_FFFF
        } else {
            ((x * 255 / w.max(1)) << 16) | ((y * 255 / h.max(1)) << 8) | ((x + y) & 0xFF)
        };
    }
    b
}

fn refused_log(args: core::fmt::Arguments) {
    if REFUSED.load(Ordering::Relaxed) + DROPPED.load(Ordering::Relaxed) <= LOGGED {
        crate::kprintln!("[capture] {}", args);
    }
}

fn refusal_name(r: u8) -> &'static str {
    match r {
        REFUSE_NOT_STARTED => "no session is running",
        REFUSE_SLOT => "slot out of range",
        REFUSE_UNKNOWN => "not a blob of this guest",
        REFUSE_SIZE => "not the stream's size",
        REFUSE_PITCH => "impossible pitch or offset",
        REFUSE_TOO_SMALL => "the stream does not fit in it",
        REFUSE_LAYOUT => "laid out for the card",
        REFUSE_IN_USE => "already named in another slot",
        REFUSE_UNBOUND => "the module cannot bind it",
        _ => "?",
    }
}

fn end_name(r: u8) -> &'static str {
    match r {
        END_CANCELLED => "the user cancelled",
        END_GUEST => "the guest stopped",
        END_HOST => "stopped on the host",
        END_NO_GPU => "no GPU",
        END_NO_PICKER => "no picker",
        END_REPLACED => "a new request",
        END_VERIFY => "the verification failed",
        END_GPU_FAILED => "the GPU failed frames",
        _ => "?",
    }
}

/// What `gpu capture` reports.
pub fn report() {
    let s = SESSION.lock();
    let slots: Vec<(usize, u32, bool)> = s.slots.iter().enumerate()
        .filter_map(|(k, o)| o.as_ref().map(|o| (k, o.resource, o.host))).collect();
    crate::kprintln!("  Capture:  {:?}{}, {} session(s), {} frame(s) sent, {} failed, {} buffer(s) refused, \
                      {} message(s) dropped",
        s.phase,
        if s.phase == Phase::Running { alloc::format!(" {}x{}", s.width, s.height) } else { alloc::string::String::new() },
        SESSIONS.load(Ordering::Relaxed), FRAMES.load(Ordering::Relaxed), FAILED.load(Ordering::Relaxed),
        REFUSED.load(Ordering::Relaxed), DROPPED.load(Ordering::Relaxed));
    for (k, res, host) in slots {
        crate::kprintln!("            slot {}: resource {}, {}", k, res, if host { "the host's" } else { "the guest's" });
    }
    drop(s);
    reap_bound();
    let st = gpudev::status();
    crate::kprintln!("            {} buffer(s) still bound for capture; gpudev holds {} buffer(s), {} bytes",
        BOUND.lock().len(), st.buffers, st.buffer_bytes);
}
