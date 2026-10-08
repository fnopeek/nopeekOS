//! Launch timeline of a microvm: host milestones, the first guest console
//! line matching each known guest milestone, and the first presented
//! virtio-gpu frames. All times are milliseconds since `start()`.
//!
//! Once the app is exec'd, the frame on screen becomes the baseline, and the
//! first frame that differs from it in most rows is the app's window: a mark
//! that does not depend on what the page loads afterwards.
//!
//! Every guest console line is printed with its time, so the full log is the
//! timeline; the `[boottime]` lines are the milestones out of it. `report()`
//! adds what the console itself cost: one VM exit per guest byte and the host
//! time spent printing guest lines.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use alloc::vec::Vec;
use spin::Mutex;

use crate::interrupts::{rdtsc, tsc_freq};
use crate::kprintln;

static T0: AtomicU64 = AtomicU64::new(0);
static ACTIVE: AtomicBool = AtomicBool::new(false);
static SERIAL_BYTES: AtomicU64 = AtomicU64::new(0);
static SERIAL_LINES: AtomicU32 = AtomicU32::new(0);
static PRINT_TSC: AtomicU64 = AtomicU64::new(0);
static FLUSHES: AtomicU32 = AtomicU32::new(0);
static GUEST_SEEN: AtomicU32 = AtomicU32::new(0);

/// Window detection: off, waiting for app exec, comparing frames, done.
const WIN_OFF: u32 = 0;
const WIN_TRACK: u32 = 1;
const WIN_ARMED: u32 = 2;
const WIN_DONE: u32 = 3;
static WIN_STATE: AtomicU32 = AtomicU32::new(WIN_OFF);
/// Row hashes of the last frame, and of the frame on screen at app exec.
static ROWS: Mutex<(Vec<u64>, Vec<u64>)> = Mutex::new((Vec::new(), Vec::new()));
/// Share of rows, in percent, that must differ from the baseline.
const WINDOW_ROWS_PCT: usize = 90;
/// Hash every n-th pixel of a row; enough to see a window appear.
const ROW_SAMPLE_STEP: usize = 4;
/// Guest console line after which the next full repaint is the app.
const APP_EXEC_LABEL: &str = "session: app exec";

/// First guest console line containing the needle → milestone label.
const GUEST_MARKS: &[(&str, &str)] = &[
    ("Linux version", "guest kernel: first line"),
    ("Run /init as init process", "guest kernel: exec /init"),
    ("[microvm-init] PID-1 up", "pid1: up"),
    ("switched to squashfs bundle root", "pid1: sqfs root"),
    ("cage present, starting Wayland", "pid1: session script"),
    ("[moz-disk] /dev/vda present", "session: udev settled"),
    ("[9p] npkhome mounted", "session: mounts done"),
    ("[wl] cage start", "session: cage start"),
    ("[wl] librewolf exec", "session: app exec"),
    ("browser exited", "session: browser exited"),
];

/// Presented frames that get a milestone line.
const FLUSH_MARKS: &[u32] = &[1, 10, 50, 200];

fn ms_since(t: u64) -> u64 {
    let hz = tsc_freq();
    if hz == 0 {
        return 0;
    }
    rdtsc().saturating_sub(t) * 1000 / hz
}

fn now_ms() -> u64 {
    ms_since(T0.load(Ordering::Relaxed))
}

/// Begin a timeline. Called once per launch, before any npkFS fetch.
pub fn start() {
    SERIAL_BYTES.store(0, Ordering::Relaxed);
    SERIAL_LINES.store(0, Ordering::Relaxed);
    PRINT_TSC.store(0, Ordering::Relaxed);
    FLUSHES.store(0, Ordering::Relaxed);
    GUEST_SEEN.store(0, Ordering::Relaxed);
    {
        let mut rows = ROWS.lock();
        rows.0.clear();
        rows.1.clear();
    }
    WIN_STATE.store(WIN_TRACK, Ordering::Relaxed);
    T0.store(rdtsc(), Ordering::Relaxed);
    ACTIVE.store(true, Ordering::Release);
    kprintln!("[boottime] +0 ms launch");
}

/// Host-side milestone.
pub fn mark(label: &str) {
    if ACTIVE.load(Ordering::Acquire) {
        kprintln!("[boottime] +{} ms {}", now_ms(), label);
    }
}

/// One byte written by the guest to COM1 (one VM exit each).
pub fn guest_byte() {
    SERIAL_BYTES.fetch_add(1, Ordering::Relaxed);
}

/// Print a completed guest console line, timed while a timeline runs.
pub fn guest_line(s: &str) {
    if !ACTIVE.load(Ordering::Acquire) {
        kprintln!("[guest] {}", s);
        return;
    }
    let t = rdtsc();
    let ms = now_ms();
    SERIAL_LINES.fetch_add(1, Ordering::Relaxed);
    kprintln!("[guest +{}] {}", ms, s);
    let seen = GUEST_SEEN.load(Ordering::Relaxed);
    for (i, (needle, label)) in GUEST_MARKS.iter().enumerate() {
        let bit = 1u32 << i;
        if seen & bit == 0 && s.contains(needle) {
            GUEST_SEEN.fetch_or(bit, Ordering::Relaxed);
            kprintln!("[boottime] +{} ms {}", ms, label);
            if *label == APP_EXEC_LABEL {
                arm_window();
            }
        }
    }
    PRINT_TSC.fetch_add(rdtsc().saturating_sub(t), Ordering::Relaxed);
}

/// A virtio-gpu flush that put a new frame on screen.
pub fn gpu_frame() {
    if !ACTIVE.load(Ordering::Acquire) {
        return;
    }
    let n = FLUSHES.fetch_add(1, Ordering::Relaxed) + 1;
    if FLUSH_MARKS.contains(&n) {
        kprintln!("[boottime] +{} ms frame #{}", now_ms(), n);
    }
}

fn arm_window() {
    let mut rows = ROWS.lock();
    let (last, base) = &mut *rows;
    base.clear();
    base.extend_from_slice(last);
    WIN_STATE.store(WIN_ARMED, Ordering::Release);
}

fn hash_rows(pixels: &[u8], width: u32, height: u32, out: &mut Vec<u64>) {
    let stride = width as usize * 4;
    out.clear();
    for row in pixels.chunks_exact(stride).take(height as usize) {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for px in row.chunks_exact(4).step_by(ROW_SAMPLE_STEP) {
            let v = u32::from_le_bytes([px[0], px[1], px[2], px[3]]) as u64;
            h = (h ^ v).wrapping_mul(0x0000_0100_0000_01b3);
        }
        out.push(h);
    }
}

/// The pixels of a presented frame. Until the app window is found, keeps
/// row hashes; after app exec, marks the first frame that repaints most
/// rows of the baseline.
pub fn gpu_pixels(pixels: &[u8], width: u32, height: u32) {
    let state = WIN_STATE.load(Ordering::Acquire);
    if !ACTIVE.load(Ordering::Acquire) || (state != WIN_TRACK && state != WIN_ARMED) {
        return;
    }
    let mut rows = ROWS.lock();
    let (last, base) = &mut *rows;
    hash_rows(pixels, width, height, last);
    if state != WIN_ARMED || last.is_empty() {
        return;
    }
    let changed = if base.len() == last.len() {
        last.iter().zip(base.iter()).filter(|(a, b)| a != b).count()
    } else {
        last.len()
    };
    if changed * 100 >= last.len() * WINDOW_ROWS_PCT {
        WIN_STATE.store(WIN_DONE, Ordering::Release);
        kprintln!(
            "[boottime] +{} ms app: window up ({}% of rows repainted)",
            now_ms(),
            changed * 100 / last.len(),
        );
    }
}

/// Close the timeline with what the console cost. Called on guest exit.
pub fn report() {
    if !ACTIVE.swap(false, Ordering::AcqRel) {
        return;
    }
    WIN_STATE.store(WIN_OFF, Ordering::Relaxed);
    {
        let mut rows = ROWS.lock();
        *rows = (Vec::new(), Vec::new());
    }
    let hz = tsc_freq().max(1);
    kprintln!(
        "[boottime] +{} ms end: {} guest console bytes (= VM exits), {} lines, {} ms host printing them, {} frames",
        now_ms(),
        SERIAL_BYTES.load(Ordering::Relaxed),
        SERIAL_LINES.load(Ordering::Relaxed),
        PRINT_TSC.load(Ordering::Relaxed) * 1000 / hz,
        FLUSHES.load(Ordering::Relaxed),
    );
}
