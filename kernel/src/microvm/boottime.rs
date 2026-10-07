//! Launch timeline of a microvm: host milestones, the first guest console
//! line matching each known guest milestone, and the first presented
//! virtio-gpu frames. All times are milliseconds since `start()`.
//!
//! Every guest console line is printed with its time, so the full log is the
//! timeline; the `[boottime]` lines are the milestones out of it. `report()`
//! adds what the console itself cost: one VM exit per guest byte and the host
//! time spent printing guest lines.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use crate::interrupts::{rdtsc, tsc_freq};
use crate::kprintln;

static T0: AtomicU64 = AtomicU64::new(0);
static ACTIVE: AtomicBool = AtomicBool::new(false);
static SERIAL_BYTES: AtomicU64 = AtomicU64::new(0);
static SERIAL_LINES: AtomicU32 = AtomicU32::new(0);
static PRINT_TSC: AtomicU64 = AtomicU64::new(0);
static FLUSHES: AtomicU32 = AtomicU32::new(0);
static GUEST_SEEN: AtomicU32 = AtomicU32::new(0);

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

/// Close the timeline with what the console cost. Called on guest exit.
pub fn report() {
    if !ACTIVE.swap(false, Ordering::AcqRel) {
        return;
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
