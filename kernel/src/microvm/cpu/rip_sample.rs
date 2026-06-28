//! Guest-RIP statistical profiler — finds WHAT a spinning vCPU executes.
//!
//! Diagnosis tool for the "one host core pegged at 100% while the others idle"
//! symptom (the speedtest ~250 Mbit cap). A busy guest vCPU runs guest code
//! without trapping, so VM-exit-site sampling is blind to it; we need an
//! unbiased program-counter sample of the RUNNING guest.
//!
//! Sampling point — SVM (the AMD QEMU dev box): every `EXIT_INTR` is a host
//! physical interrupt (the per-core host timer at ~100 Hz, or a device IRQ)
//! that preempted a *running* guest. The VMCB save area then holds the exact
//! guest RIP at the moment of preemption — a clean PC sample. An *idle* vCPU
//! halts (`EXIT_HLT`, a different exit) so it is never sampled: the histogram
//! concentrates on whatever is actually burning CPU. (VMX/bare-metal Intel
//! would use the VMX-preemption timer; not wired yet — QEMU/AMD reproduces the
//! cap, so SVM sampling suffices for the diagnosis.)
//!
//! Aggregation: a Space-Saving heavy-hitter table (48 slots, 64-byte RIP
//! buckets) survives true hot spots under bounded memory, plus a per-vCPU
//! sample count so we see WHICH vCPU spins. Dumped every ~5 s, then reset for an
//! independent next window. Resolve the raw hex RIPs offline against the guest
//! `System.map` (`~/.cache/nopeekos/linux-src/linux-6.18.26/System.map`).
//!
//! Gated by `DEBUG`; `record`/`maybe_dump` compile to an early return when off.

use crate::kprintln;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

/// Master switch. Set false (and rebuild) to strip the probe once the spinning
/// core is identified. `record()` runs on EVERY exit (~30k/s) + the ~5 s dump
/// (~13 kprintln lines) BLOCKS its core ~60ms on the UART THRE spin — on for
/// diagnosis (csd visibility), strip before shipping.
const DEBUG: bool = true;

const SLOTS: usize = 48;
/// 64-byte RIP buckets: clusters a hot loop's instructions into one entry so
/// the 48 slots aren't fragmented across a function's individual addresses.
const BUCKET_SHIFT: u64 = 6;
const MAX_VCPU: usize = 8;
/// Dump cadence in seconds.
const WINDOW_SECS: u64 = 5;

struct Hist {
    rip: [u64; SLOTS],
    cnt: [u64; SLOTS],
    vcpu: [u64; MAX_VCPU],
    total: u64,
}

impl Hist {
    const fn new() -> Self {
        Hist { rip: [0; SLOTS], cnt: [0; SLOTS], vcpu: [0; MAX_VCPU], total: 0 }
    }
    fn clear(&mut self) {
        *self = Hist::new();
    }
}

static H: Mutex<Hist> = Mutex::new(Hist::new());
/// Lock-free cadence gate so `maybe_dump` doesn't lock on every VM-exit.
static LAST_DUMP_TSC: AtomicU64 = AtomicU64::new(0);

/// PV-TLB-flush ceiling probe: of all cross-vCPU IPI targets (TLB shootdowns
/// etc.), how many were PREEMPTED (idle/parked, not running guest) at send time.
/// That fraction is exactly what KVM_FEATURE_PV_TLB_FLUSH could skip the IPI +
/// csd_lock_wait for — high → PV pays off, low → vCPUs are all-busy and the
/// lever is vCPU count instead. Measured host-side, no guest changes needed.
static IPI_TARGETS: AtomicU64 = AtomicU64::new(0);
static IPI_TARGETS_PREEMPTED: AtomicU64 = AtomicU64::new(0);

/// Record one cross-vCPU IPI target and whether it was preempted at send time.
pub fn note_ipi_target(preempted: bool) {
    if !DEBUG {
        return;
    }
    IPI_TARGETS.fetch_add(1, Ordering::Relaxed);
    if preempted {
        IPI_TARGETS_PREEMPTED.fetch_add(1, Ordering::Relaxed);
    }
}

/// Record one guest-RIP sample taken at an `EXIT_INTR` preemption of vCPU
/// `vcpu`. Space-Saving: hit an existing bucket, else evict the least-frequent
/// slot inheriting its count (so genuine heavy hitters can't be displaced by a
/// stream of cold one-offs).
pub fn record(rip: u64, vcpu: u8) {
    if !DEBUG {
        return;
    }
    let bucket = (rip >> BUCKET_SHIFT) << BUCKET_SHIFT;
    let mut h = H.lock();
    h.total += 1;
    if (vcpu as usize) < MAX_VCPU {
        h.vcpu[vcpu as usize] += 1;
    }
    let mut min_i = 0usize;
    let mut min_c = u64::MAX;
    for i in 0..SLOTS {
        if h.rip[i] == bucket && h.cnt[i] != 0 {
            h.cnt[i] += 1;
            return;
        }
        if h.cnt[i] < min_c {
            min_c = h.cnt[i];
            min_i = i;
        }
    }
    h.rip[min_i] = bucket;
    h.cnt[min_i] = min_c.wrapping_add(1);
}

/// Cheap to call on every exit: only locks + dumps once per `WINDOW_SECS`.
pub fn maybe_dump() {
    if !DEBUG {
        return;
    }
    let mhz = (crate::interrupts::tsc_freq() / 1_000_000).max(1);
    let window = WINDOW_SECS * 1_000_000 * mhz;
    let now = crate::interrupts::rdtsc();
    let last = LAST_DUMP_TSC.load(Ordering::Relaxed);
    if last == 0 {
        // First call this run: arm the window, don't dump an empty table.
        let _ = LAST_DUMP_TSC.compare_exchange(0, now, Ordering::Relaxed, Ordering::Relaxed);
        return;
    }
    if now.wrapping_sub(last) < window {
        return;
    }
    if LAST_DUMP_TSC
        .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
        .is_err()
    {
        return; // another core is dumping this window
    }
    dump_and_reset(now.wrapping_sub(last) / mhz);
}

fn dump_and_reset(window_us: u64) {
    let mut h = H.lock();
    if h.total == 0 {
        return;
    }
    let secs = (window_us / 1_000_000).max(1);

    // Per-vCPU sample split — which vCPU is hot. The dominant one's guest code
    // is what the RIP histogram below is mostly showing.
    kprintln!(
        "[ripsample] {} samples / {}s ({}/s) | per-vCPU: v0={} v1={} v2={} v3={} v4={} v5={} v6={} v7={}",
        h.total, secs, h.total / secs,
        h.vcpu[0], h.vcpu[1], h.vcpu[2], h.vcpu[3],
        h.vcpu[4], h.vcpu[5], h.vcpu[6], h.vcpu[7],
    );

    // PV-TLB-flush ceiling: % of cross-vCPU IPI targets that were preempted
    // (idle/parked) at send time — the upper bound on what PV could skip.
    let tgt = IPI_TARGETS.swap(0, Ordering::Relaxed);
    let pre = IPI_TARGETS_PREEMPTED.swap(0, Ordering::Relaxed);
    if tgt > 0 {
        kprintln!(
            "[ripsample]   ipi-targets {}/s | preempted {}% ({}/{}) = PV-TLB-flush skip ceiling",
            tgt / secs,
            pre * 100 / tgt,
            pre,
            tgt,
        );
    }

    // Top buckets by count (simple selection over 48 slots).
    let total = h.total;
    let mut used: [bool; SLOTS] = [false; SLOTS];
    for _ in 0..12 {
        let mut best_i = usize::MAX;
        let mut best_c = 0u64;
        for i in 0..SLOTS {
            if !used[i] && h.cnt[i] > best_c {
                best_c = h.cnt[i];
                best_i = i;
            }
        }
        if best_i == usize::MAX || best_c == 0 {
            break;
        }
        used[best_i] = true;
        kprintln!(
            "[ripsample]   {:>3}.{}%  rip={:#018x}  (n={})",
            best_c * 100 / total,
            (best_c * 1000 / total) % 10,
            h.rip[best_i],
            best_c,
        );
    }
    h.clear();
}

/// Reset at VM teardown so the next launch profiles from scratch.
pub fn reset() {
    if !DEBUG {
        return;
    }
    H.lock().clear();
    LAST_DUMP_TSC.store(0, Ordering::Relaxed);
}
