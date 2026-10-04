# `kernel/src/microvm/cpu/rip_sample.rs` @ 5e0102684

## L1-23 · `use crate::kprintln;`

```
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
```

## L29-32 · `const DEBUG: bool = false;`

```
/// Master switch. Set false (and rebuild) to strip the probe once the spinning
/// core is identified. `record()` runs on EVERY exit (~30k/s) + the ~5 s dump
/// (~13 kprintln lines) BLOCKS its core ~60ms on the UART THRE spin — on for
/// diagnosis (csd visibility), strip before shipping.
```

## L36-37 · `const BUCKET_SHIFT: u64 = 6;`

```
/// 64-byte RIP buckets: clusters a hot loop's instructions into one entry so
/// the 48 slots aren't fragmented across a function's individual addresses.
```

## L40 · `const WINDOW_SECS: u64 = 5;`

```
/// Dump cadence in seconds.
```

## L60 · `static LAST_DUMP_TSC: AtomicU64 = AtomicU64::new(0);`

```
/// Lock-free cadence gate so `maybe_dump` doesn't lock on every VM-exit.
```

## L63-67 · `static IPI_TARGETS: AtomicU64 = AtomicU64::new(0);`

```
/// PV-TLB-flush ceiling probe: of all cross-vCPU IPI targets (TLB shootdowns
/// etc.), how many were PREEMPTED (idle/parked, not running guest) at send time.
/// That fraction is exactly what KVM_FEATURE_PV_TLB_FLUSH could skip the IPI +
/// csd_lock_wait for — high → PV pays off, low → vCPUs are all-busy and the
/// lever is vCPU count instead. Measured host-side, no guest changes needed.
```

## L71 · `pub fn note_ipi_target(preempted: bool) {`

```
/// Record one cross-vCPU IPI target and whether it was preempted at send time.
```

## L82-85 · `pub fn record(rip: u64, vcpu: u8) {`

```
/// Record one guest-RIP sample taken at an `EXIT_INTR` preemption of vCPU
/// `vcpu`. Space-Saving: hit an existing bucket, else evict the least-frequent
/// slot inheriting its count (so genuine heavy hitters can't be displaced by a
/// stream of cold one-offs).
```

## L112 · `pub fn maybe_dump() {`

```
/// Cheap to call on every exit: only locks + dumps once per `WINDOW_SECS`.
```

## L122 · `let _ = LAST_DUMP_TSC.compare_exchange(0, now, Ordering::Relaxed, Ordering::Relaxed);`

```
// First call this run: arm the window, don't dump an empty table.
```

## L133 · `return; // another core is dumping this window`

```
// another core is dumping this window
```

## L145-146 · `kprintln!(`

```
// Per-vCPU sample split — which vCPU is hot. The dominant one's guest code
// is what the RIP histogram below is mostly showing.
```

## L154-155 · `let tgt = IPI_TARGETS.swap(0, Ordering::Relaxed);`

```
// PV-TLB-flush ceiling: % of cross-vCPU IPI targets that were preempted
// (idle/parked) at send time — the upper bound on what PV could skip.
```

## L168 · `let total = h.total;`

```
// Top buckets by count (simple selection over 48 slots).
```

## L195 · `pub fn reset() {`

```
/// Reset at VM teardown so the next launch profiles from scratch.
```

