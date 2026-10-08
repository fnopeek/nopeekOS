//! Launch timeline of a microvm: host milestones, the first guest console
//! line matching each known guest milestone, and the first presented
//! virtio-gpu frames. All times are milliseconds since `start()`.
//!
//! After the app is exec'd, the first frame that is mostly not black is the
//! app's window: the guest console and the compositor's empty output are
//! black, a browser window is not, even in a dark theme. The mark does not
//! depend on what the page loads afterwards.
//!
//! Every guest console line is printed with its time, so the full log is the
//! timeline; the `[boottime]` lines are the milestones out of it. `report()`
//! adds what the console itself cost: one VM exit per guest byte and the host
//! time spent printing guest lines.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use spin::Mutex;

use crate::interrupts::{rdtsc, tsc_freq};
use crate::microvm::cpu::{VMEXIT_BUCKETS, VMEXIT_LABELS, VT_BUCKETS};
use crate::kprintln;

static T0: AtomicU64 = AtomicU64::new(0);
static ACTIVE: AtomicBool = AtomicBool::new(false);
static SERIAL_BYTES: AtomicU64 = AtomicU64::new(0);
static SERIAL_LINES: AtomicU32 = AtomicU32::new(0);
static PRINT_TSC: AtomicU64 = AtomicU64::new(0);
static FLUSHES: AtomicU32 = AtomicU32::new(0);
static GUEST_SEEN: AtomicU32 = AtomicU32::new(0);

/// Window detection: not yet armed, waiting for the window, done.
const WIN_IDLE: u32 = 0;
const WIN_ARMED: u32 = 1;
const WIN_DONE: u32 = 2;
static WIN_STATE: AtomicU32 = AtomicU32::new(WIN_IDLE);
/// Share of sampled pixels, in percent, that must not be black.
const WINDOW_LIT_PCT: usize = 50;
/// vCPU time split and exit counts at app exec, diffed at window up.
static APP_SNAP: Mutex<Option<AppSnap>> = Mutex::new(None);
type AppSnap = (u64, [u64; VT_BUCKETS], [u64; VMEXIT_BUCKETS], ([u64; 5], u64));
/// Sample every n-th pixel of every n-th row.
const SAMPLE_STEP: usize = 4;
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
    WIN_STATE.store(WIN_IDLE, Ordering::Relaxed);
    *APP_SNAP.lock() = None;
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
                let (hist, sum, _) = crate::microvm::cpu::wake_snapshot();
                *APP_SNAP.lock() = Some((
                    rdtsc(),
                    crate::microvm::cpu::vcpu_time_snapshot(),
                    crate::microvm::cpu::vm_exit_snapshot(),
                    (hist, sum),
                ));
                WIN_STATE.store(WIN_ARMED, Ordering::Release);
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

/// The pixels of a presented frame. After app exec, marks the first frame
/// whose sampled pixels are mostly not black.
pub fn gpu_pixels(pixels: &[u8], width: u32, height: u32) {
    if !ACTIVE.load(Ordering::Acquire) || WIN_STATE.load(Ordering::Acquire) != WIN_ARMED {
        return;
    }
    let stride = width as usize * 4;
    if stride == 0 {
        return;
    }
    let (mut lit, mut total) = (0usize, 0usize);
    for row in pixels.chunks_exact(stride).take(height as usize).step_by(SAMPLE_STEP) {
        for px in row.chunks_exact(4).step_by(SAMPLE_STEP) {
            total += 1;
            if px[0] | px[1] | px[2] != 0 {
                lit += 1;
            }
        }
    }
    if total > 0 && lit * 100 >= total * WINDOW_LIT_PCT {
        WIN_STATE.store(WIN_DONE, Ordering::Release);
        kprintln!(
            "[boottime] +{} ms app: window up ({}% of pixels lit)",
            now_ms(),
            lit * 100 / total,
        );
        report_app_phase();
    }
}

/// Where the vCPUs' time went between app exec and window up, summed over
/// all vCPUs: inside the guest, halted, waiting for the device lock, and the
/// rest of exit handling; and the exits by kind.
fn report_app_phase() {
    let Some((t0, vt0, ex0, (wh0, ws0))) = APP_SNAP.lock().take() else { return };
    let hz = tsc_freq().max(1);
    let ms = |c: u64| c * 1000 / hz;
    let vt = crate::microvm::cpu::vcpu_time_snapshot();
    let ex = crate::microvm::cpu::vm_exit_snapshot();
    let d = |i: usize| vt[i].saturating_sub(vt0[i]);
    let guest = d(crate::microvm::cpu::VT_GUEST);
    let outside = d(crate::microvm::cpu::VT_OUTSIDE);
    let halted = d(crate::microvm::cpu::VT_HALTED);
    let devlock = d(crate::microvm::cpu::VT_DEVLOCK);
    let handling = outside.saturating_sub(halted).saturating_sub(devlock);
    let mut exits = alloc::string::String::new();
    for i in 0..VMEXIT_BUCKETS {
        let _ = core::fmt::Write::write_fmt(
            &mut exits,
            format_args!(" {} {}", VMEXIT_LABELS[i], ex[i].saturating_sub(ex0[i])),
        );
    }
    kprintln!(
        "[boottime] app phase {} ms, vCPU time: guest {} ms, halted {} ms, \
device lock {} ms, exit handling {} ms | exits:{}",
        ms(rdtsc().saturating_sub(t0)), ms(guest), ms(halted), ms(devlock), ms(handling), exits,
    );
    let (wh, ws, wmax) = crate::microvm::cpu::wake_snapshot();
    let n: u64 = (0..5).map(|i| wh[i].saturating_sub(wh0[i])).sum();
    let us = |c: u64| c * 1_000_000 / hz;
    let b = |i: usize| wh[i].saturating_sub(wh0[i]);
    kprintln!(
        "[boottime] app phase interrupt delivery: {} posted, avg {} us, max {} us | \
<10us {} <50us {} <200us {} <1ms {} >=1ms {}",
        n, if n > 0 { us(ws.saturating_sub(ws0)) / n } else { 0 }, us(wmax),
        b(0), b(1), b(2), b(3), b(4),
    );
}

/// Close the timeline with what the console cost. Called on guest exit.
pub fn report() {
    if !ACTIVE.swap(false, Ordering::AcqRel) {
        return;
    }
    WIN_STATE.store(WIN_IDLE, Ordering::Relaxed);
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
