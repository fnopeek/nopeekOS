# `kernel/src/smp/per_core.rs` @ 5e0102684

## L1-4 · `use alloc::vec::Vec;`

```
//! Per-Core State
//!
//! Tracks CPU cores discovered at boot. Dynamically sized — no hardcoded limit.
//! Scales from 1 (BSP only) to 1024+ cores.
```

## L18 · `#[allow(dead_code)]`

```
/// Sequential index (0 = BSP)
```

## L21 · `pub apic_id: u32,`

```
/// Hardware APIC ID (may not be sequential)
```

## L29 · `static SCHEDULER_READY: AtomicBool = AtomicBool::new(false);`

```
/// Set to true once scheduler is initialized and APs should start working
```

## L32 · `static HAS_MWAIT: AtomicBool = AtomicBool::new(false);`

```
/// True if CPU supports MONITOR/MWAIT (detected at boot)
```

## L35-36 · `static HAS_APERFMPERF: AtomicBool = AtomicBool::new(false);`

```
/// True if CPU supports IA32_APERF/IA32_MPERF MSRs (CPUID.06H:ECX[0]).
/// Intel since Nehalem; not present on AMD / qemu64. Guarded at every rdmsr.
```

## L39 · `static CORE_MHZ: [AtomicU32; 256] = {`

```
/// Per-core average frequency in MHz (APERF/MPERF ratio)
```

## L45 · `static CORE_USAGE: [AtomicU32; 256] = {`

```
/// Per-core CPU usage in percent (0-100)
```

## L51 · `static CORE_BUSY_TSC: [AtomicU64; 256] = {`

```
/// Per-core cumulative busy TSC cycles (only incremented during actual task work)
```

## L57-60 · `static LAST_HALT: [AtomicU64; 256] = {`

```
/// Snapshots for delta computation
/// Per-core snapshot of CORE_HALT_TSC at the last `update_core_freq`.
/// Two samples and a difference are what turns a cumulative halt counter
/// into a percentage.
```

## L74-76 · `static LAST_APERF: [AtomicU64; 256] = {`

```
/// APERF/MPERF snapshots (MSR 0xE8 / 0xE7).
/// APERF ticks at actual freq when active, MPERF at nominal TSC rate when active.
/// Ratio APERF/MPERF gives effective running frequency; MPERF/TSC gives activity fraction.
```

## L85 · `static CORE_ACTIVE: [AtomicBool; 256] = {`

```
/// Per-core active flag: true while executing a scheduler task
```

## L91-92 · `static WORK_START_TSC: [AtomicU64; 256] = {`

```
/// Per-core work-start TSC (set when task begins or resumes after wait).
/// Used for checkpoint-based busy tracking in long-running WASM apps.
```

## L98-111 · `static CORE_HALT_TSC: [AtomicU64; 256] = {`

```
// ── Idle-based instrumentation (scheduler diagnosis step 0) ────────
//
// The old `CORE_BUSY_TSC` is *self-reported*: code adds cycles it
// believes were work. It cannot tell a halted core from one spinning
// in a busy-loop — a spinner simply never accounts itself idle, so it
// looks free while pegging the host vCPU at 100%. That is exactly the
// idle-100% bug (`docs/plan/SCHEDULER_FIBERS.md`) and why `top` lies.
//
// These counters measure the opposite, directly: TSC cycles a core
// spends GENUINELY HALTED (HLT/MWAIT), recorded at every halt site.
// True busy% over a window = 100 − halted%. A spinner shows ~100%
// (never halts); a healthy idle core shows ~0%. Halt *entries* expose
// spurious-wake spin: many entries with tiny residency = the core
// keeps waking and re-arming instead of staying asleep.
```

## L113 · `static CORE_HALT_TSC: [AtomicU64; 256] = {`

```
/// Per-core cumulative TSC cycles spent halted (HLT/MWAIT).
```

## L119 · `static CORE_HALT_COUNT: [AtomicU64; 256] = {`

```
/// Per-core count of halt entries (each HLT/MWAIT execution).
```

## L125-136 · `pub fn record_halt(core_id: usize, cycles: u64) {`

```
/// Record one genuine idle halt of `cycles` TSC duration on `core_id`.
///
/// **Invariant: EVERY site that executes HLT/MWAIT must call this.** It is
/// the only signal that distinguishes "halted" from "spinning", and since
/// 0.377.0 it is also the source of the usage figure (`100 − halted%`) —
/// so a halt that stays silent does not read as idle, it reads as FULL
/// LOAD. That is what `core0_idle_tick` did: `cores` said 1 % and `top`
/// said 100 % at the same moment, because the two idle through different
/// halt sites and only one of them reported.
///
/// A deliberate spin (e.g. draining a moving pointer) must NOT call it —
/// there the core really is busy.
```

## L139-141 · `HALT_SINCE[core_id].store(0, Ordering::Relaxed);`

```
// Clear the in-progress mark BEFORE adding: a concurrent snapshot then
// at worst misses this halt once (and sees it next time), never counts
// it twice.
```

## L147-155 · `static HALT_SINCE: [AtomicU64; 256] = [const { AtomicU64::new(0) }; 256];`

```
/// TSC at which the halt now in progress began, 0 if the core is not
/// halted in `interrupts::halt_until`.
///
/// **Without it a sleeping core reads as SPINNING.** A halt is booked when
/// it ENDS. While every worker woke 100×/s that was always inside the
/// measuring window; since workers are tickless (0.411) a core with nothing
/// to do sleeps for seconds, no halt ends in the window, and `cores`
/// reported 100 % busy with 0 halts/s — on the hardware, for every empty
/// core at once.
```

## L158 · `pub fn halt_begin(core_id: usize, t0: u64) {`

```
/// Mark `core_id` as halted from TSC `t0` on (cleared by `record_halt`).
```

## L161-164 · `if HAS_APERFMPERF.load(Ordering::Relaxed) {`

```
// The core's running cycles up to here, for a reader on another
// core (`update_core_freq`): APERF/MPERF are core-local MSRs and
// stand still while the core is halted, so this snapshot is exact
// until the core runs again.
```

## L174 · `static RUN_APERF: [AtomicU64; 256] = [const { AtomicU64::new(0) }; 256];`

```
/// APERF/MPERF as of the core's last halt entry (see `halt_begin`).
```

## L179 · `unsafe {`

```
// SAFETY: only called when CPUID.06H:ECX[0] says MSRs 0xE7/0xE8 exist.
```

## L190 · `fn halted_tsc(core_id: usize) -> u64 {`

```
/// Cumulative halted TSC for `core_id`, INCLUDING a halt still in progress.
```

## L201-202 · `pub fn halt_snapshot(core_id: usize) -> (u64, u64) {`

```
/// Snapshot (cumulative halted TSC, halt-entry count) for `core_id`.
/// Sample twice and diff to get true busy% + halt rate over a window.
```

## L208-217 · `pub const WAKE_CAUSES: usize = 4;`

```
// ── Per-source wake attribution (1 kHz spurious-wake diagnosis) ──────
//
// HALTS/s says a core wakes ~1000×/s but not WHY. These counters record
// the CAUSE at each wake: an ISR bumps its vector class as it runs (an
// ISR that runs while the core was halted IS the wake that returned the
// HLT). The key diagnostic is UNATTRIBUTED = HALTS − Σcauses on Core 0:
// if its HALTS/s far exceeds its attributed ISRs, the HLT is returning
// with NO guest ISR — KVM resuming the vCPU on a host-side event (host
// HZ tick) past the emulated HLT. That is a QEMU/KVM artifact, not a
// bare-metal idle bug. On real HW every wake must attribute to a cause.
```

## L219 · `pub const WAKE_TIMER: usize = 0;        // 100 Hz PIT / APIC timer ISR (Core 0)`

```
// 100 Hz PIT / APIC timer ISR (Core 0)
```

## L220 · `pub const WAKE_KEYBOARD: usize = 1;     // input IRQ: i8042 or xHCI (Core 0)`

```
// input IRQ: i8042 or xHCI (Core 0)
```

## L221 · `pub const WAKE_HLT_FALLBACK: usize = 2; // worker idle sti;hlt;cli returned`

```
// worker idle sti;hlt;cli returned
```

## L222 · `pub const WAKE_NPK_SLEEP: usize = 3;    // npk_sleep HLT returned`

```
// npk_sleep HLT returned
```

## L224 · `pub const WAKE_LABELS: [&str; WAKE_CAUSES] = ["timer", "input", "hlt-fb", "npk-sleep"];`

```
/// Short labels for the `cores` breakdown, indexed by cause.
```

## L233-236 · `static TIMER_FIRES: [AtomicU64; 256] = [const { AtomicU64::new(0) }; 256];`

```
/// Record one wake of `cause` on `core_id`. Cheap + lock-free — safe to
/// call from interrupt context (unlike `current_core_id`, which locks).
/// Deadline-timer health per core: one-shot fires, and the worst lateness of
/// a halt that had a deadline (wake TSC − deadline), window peak.
```

## L248 · `pub fn timer_snapshot(core_id: usize) -> (u64, u64) {`

```
/// (fires, late_max_tsc) — late_max is swap-reset so `cores` reads the window peak.
```

## L255-257 · `static DRIVER_CORES: AtomicU64 = AtomicU64::new(0);`

```
/// Cores a hardware driver lives on: a fiber there bound a device or waits
/// on a device interrupt. A vCPU beside it would hold it off for a whole
/// slice — the touchpad went dead that way. Sticky: drivers stay put.
```

## L273 · `pub fn wake_snapshot(core_id: usize) -> [u64; WAKE_CAUSES] {`

```
/// Snapshot all wake-cause counters for `core_id`.
```

## L283 · `pub fn is_active(core_id: usize) -> bool {`

```
/// Whether `core_id` is currently inside a scheduler task (CORE_ACTIVE).
```

## L289 · `static MAX_TURBO_MHZ: AtomicU32 = AtomicU32::new(0);`

```
/// Platform frequency limits (set once by enable_hwp)
```

## L296 · `let ecx: u32;`

```
// Detect MONITOR/MWAIT support via CPUID.01H:ECX bit 3
```

## L312-313 · `let ecx: u32;`

```
// Detect IA32_APERF/IA32_MPERF via CPUID.06H:ECX[0]
// Absent on AMD and on KVM guest w/ generic `-cpu qemu64`.
```

## L331-350 · `const MSR_AMD_RAPL_POWER_UNIT: u32 = 0xC001_0299;`

```
// ── RAPL: was zieht das CPU-Package wirklich? ────────────────────────
//
// Die Frage dahinter ist nicht akademisch: Florians IdeaPad zieht im
// Leerlauf 21,4 W (aus `_BST`, deckungsgleich mit 2,5 h auf 53,5 Wh),
// dasselbe Blech unter Linux 5-8 W. Es gibt acht plausible Verdaechtige
// — C-States, Aufwachrate, PCIe-ASPM, NVMe-APST, der UC-Framebuffer, der
// USB-Dongle, WLAN, der dauerlaufende Audio-DMA — und EINE Zahl. Dieses
// Register trennt den groessten Block vom Rest: ist das Package 3 W,
// sind C-States und Tickless die falsche Baustelle.
//
// Portiert aus Linux 6.18. Das Merkmalsbit steht in
// `arch/x86/kernel/cpu/amd.c`:
//
//     /* Bit 14 indicates the Runtime Average Power Limit interface. */
//     if (c->x86_power & BIT(14)) set_cpu_cap(c, X86_FEATURE_RAPL);
//
// wobei `x86_power` = CPUID Fn8000_0007_EDX ist. Die drei MSR-Nummern
// stehen in `arch/x86/include/asm/msr-index.h`, die Lage des
// Einheitenfeldes in `arch/x86/events/rapl.c`
// (`(msr_rapl_power_unit_bits >> 8) & 0x1F`).
```

## L356-358 · `static RAPL_NJ_PER_UNIT: AtomicU32 = AtomicU32::new(0);`

```
/// Nanojoule je Zaehlschritt. Die Einheit ist 2^-ESU Joule; in nJ
/// gerechnet bleibt es eine ganze Zahl (ESU 16 -> 15258 nJ) und es
/// braucht keine Gleitkommazahl im Kernel.
```

## L361-362 · `fn rdmsr32(msr: u32) -> u32 {`

```
/// MUSS auf dem zu messenden Kern laufen (rdmsr ist kernlokal) und nur,
/// wenn `has_rapl()` gilt — sonst #GP.
```

## L365-366 · `unsafe {`

```
// SAFETY: der Rufer hat `has_rapl()` geprueft; das MSR existiert dann
// auf jedem Kern dieses Sockels.
```

## L375-376 · `unsafe {`

```
// SAFETY: CPUID 0x80000007 ist auf jedem x86-64 gueltig; rbx wird von
// LLVM reserviert, deshalb von Hand gesichert.
```

## L390 · `let esu = (rdmsr32(MSR_AMD_RAPL_POWER_UNIT) >> 8) & 0x1F;`

```
// Erst JETZT lesen — vor dem Merkmalsbit waere es ein #GP.
```

## L392-393 · `if esu == 0 || esu > 30 {`

```
// 2^-ESU Joule, in Nanojoule: 1e9 >> ESU. Ein absurdes ESU (0 oder
// >30) ergaebe Unsinn statt einer Messung — dann lieber gar keine.
```

## L403 · `pub fn rapl_nj_per_unit() -> u32 { RAPL_NJ_PER_UNIT.load(Ordering::Relaxed) }`

```
/// Nanojoule je Zaehlschritt (0 = kein RAPL).
```

## L406-407 · `pub fn rapl_pkg_raw() -> u32 {`

```
/// Roher Energiezaehler des PACKAGE. 32 Bit, laeuft um — immer die
/// Differenz zweier Abtastungen nehmen (`wrapping_sub`).
```

## L413 · `pub fn rapl_core_raw() -> u32 {`

```
/// Dasselbe fuer den KERN, auf dem dieser Aufruf laeuft.
```

## L419-422 · `pub fn rapl_mw(delta_units: u32, window_us: u64) -> u64 {`

```
/// Milliwatt aus Zaehlerdifferenz und Fenster.
///
/// mW = nJ / us — die Einheiten kuerzen sich, deshalb steht hier keine
/// Umrechnungskonstante, die man falsch setzen koennte.
```

## L442 · `pub fn core_count() -> usize {`

```
/// Total cores (BSP + online APs)
```

## L447 · `pub fn start_scheduler() {`

```
/// Signal APs to start their scheduler loops
```

## L452 · `pub const NO_DEDICATED_CORE: u32 = u32::MAX;`

```
// ── Dedicated microvm core (substrate rework A1) ───────────────────
```

## L454-455 · `pub const NO_DEDICATED_CORE: u32 = u32::MAX;`

```
/// Sentinel: no core is dedicated → microvm stays cooperatively
/// time-sliced on Core 0 (the validated path on ≤2-core hosts).
```

## L458-463 · `pub static DEDICATED_VM_CORE: AtomicU32 = AtomicU32::new(NO_DEDICATED_CORE);`

```
/// Worker core exclusively reserved for the microvm. It is carved out
/// of the pool: it never calls `next_task`, so no task is ever assigned
/// to it and the scheduler needs no other change.
/// A1 just parks this core (proves the carve-out + stats); A2 runs the
/// guest VMRESUME/VMRUN loop here so the guest no longer fights Shade
/// + the shell for Core 0.
```

## L466-480 · `pub const A2_DEDICATED_CORE_ENABLED: bool = true;`

```
/// A2 master switch (mirrors `guest_mem::DEMAND_ENABLED` for B3).
/// `false` → the microvm ALWAYS stays cooperatively time-sliced on
/// Core 0, regardless of core count; the A1/A2 dedicated-core code
/// stays in-tree but dormant. `true` → the core-count gate below
/// decides.
///
/// Gated OFF after the 2026-05-19 QEMU_SMP bisection: with A2 the
/// guest dies in process-agnostic musl/near-null corruption at ≈6 s
/// before any page loads; with the cooperative Core-0 path the
/// browser surfs real multi-site traffic (DuckDuckGo/Google,
/// ~74 s+). So A2 (dedicated core + per-core LAPIC timer + the new
/// inject path) is a correctness REGRESSION, not the validated step
/// the plan assumed. The cooperative path is the working baseline;
/// A2's cadence benefit is deferred until its guest-corruption root
/// cause is found and fixed. Flip to `true` only for A2 debugging.
```

## L483-487 · `const MIN_CORES_FOR_DEDICATED: usize = 3;`

```
/// Minimum logical cores (incl. BSP) before dedicating one to the
/// microvm — below this, dedicating would starve the rest, so keep
/// cooperative Core-0 slicing. TODO: make a `sys/config/
/// microvm_dedicated_core` tunable (same pattern as the planned
/// `hwp_epp` knob in `enable_hwp`).
```

## L490-494 · `pub fn init_dedicated_vm_core(worker_count: usize) {`

```
/// Decide the dedicated VM core from the live core count. Called once
/// right after `scheduler::init` (so WORKER_COUNT is known). Picks the
/// highest worker id (= `worker_count`, since worker ids are
/// `1..=worker_count`) so the latency-sensitive low cores keep
/// stealing; Core 0 stays the IRQ/compositor/intent core regardless.
```

## L496 · `let total = worker_count + 1; // + BSP`

```
// + BSP
```

## L498-513 · `let vendor = crate::microvm::cpu::detect_vendor();`

```
// A2 dedicated-core path was hardened on QEMU/AMD-SVM through four
// SVM-specific correctness fixes (v0.172.34/35/36/38 + v0.172.42).
// The Intel-VMX equivalents (VMCS host-state lifecycle, IDT
// vectoring info, MSR auto-save/load) are still pending audit —
// bare-metal NUC reports `[microvm] guest running` but Linux
// never produces an earlycon byte. Until VMX-A2 has its own
// correctness pass, gate by vendor: AMD keeps the dedicated path
// (validated), Intel falls back to cooperative Core-0 slicing
// (the byte-identical pre-A2 model).
//
// Vendor-detect via the existing `microvm::cpu::detect_vendor`
// helper. It uses the well-tested `vmx::host_cpuid` wrapper
// (lateout("esi") binding + nostack + preserves_flags), unlike
// v0.172.62's hand-rolled inline asm which hung QEMU/AMD on the
// first OTA-deploy. `detect_vendor` is stateless — safe to call
// before `microvm::cpu::init` has populated the static VENDOR.
```

## L518-524 · `let fiber_mode = crate::microvm::cpu::VCPU_AS_FIBER`

```
// vCPU-as-fiber (unified pool): when enabled, the guest runs as a
// normal pool fiber on a DYNAMIC core — so we do NOT statically carve
// one out here (that wasted a core whenever no VM ran, and stranded the
// app fibers already on it). See microvm::cpu / docs/plan/SCHEDULER_FIBERS.md.
// Intel parity (#3): also enable fiber mode on VMX (flag-gated), so the
// browser leaves the cooperative Core-0 path. The per-core TSS the
// worker needs for VMX host-state is installed lazily in vmx::vm_open.
```

## L561 · `pub fn dedicated_vm_core() -> Option<usize> {`

```
/// The dedicated microvm core id, or `None` if cooperative Core-0.
```

## L569 · `pub fn has_mwait() -> bool {`

```
/// Check if MONITOR/MWAIT is available
```

## L574-589 · `pub fn update_core_freq(core_id: usize) {`

```
/// Update per-core CPU usage and frequency.
///
/// Usage:     explicit busy-TSC tracking (task execution time / total time).
/// Activity:  MPERF/TSC ratio — hardware-measured fraction of wall clock the
///            core was not halted. Captures idle that scheduler tracking misses
///            (e.g. Core 0 in HLT between events).
/// Freq:      APERF/MPERF * nominal_MHz — effective frequency while running.
///            Both MSRs stop ticking during C-states, so ratio is independent
///            of idle; captures true average P-state when active.
///
/// Callable from ANY core (since 0.423): the halt counter is shared, and
/// APERF/MPERF of another core come from the snapshot it takes at every halt
/// entry. Before, only the core itself could measure — and a sleeping core
/// never does, so `top` showed a value from its last burst (3.5 GHz on a
/// core asleep for minutes). This relies on one TSC across cores
/// (`smp::init` corrects core 0 at boot).
```

## L593-598 · `let prev = LAST_TSC_CORE[core_id].load(Ordering::Relaxed);`

```
// **Evaluate only windows of at least 100 ms.** The worker loop calls
// this on every pass; two passes back to back with no halt between
// made a window of microseconds that read "100 % busy" — and that was
// the value left standing (`top` showed the bar's core at 87 % while
// `cores` measured it asleep). Until the window is long enough, keep
// accumulating: the snapshots stay where they are.
```

## L606-607 · `let has_apmp = HAS_APERFMPERF.load(Ordering::Relaxed);`

```
// APERF/MPERF are gated by CPUID.06H:ECX[0] — absent on AMD and on KVM
// guests with `-cpu qemu64`. Reading them unconditionally raises #GP.
```

## L625 · `if prev_tsc == 0 { return; } // First call — seed only`

```
// First call — seed only
```

## L634-648 · `let halt_pct = ((delta_halt as u128) * 100 / (delta_tsc as u128)).min(100) as u32;`

```
// Usage = 100 − halted%, the same definition `intent_cores` prints.
//
// It used to be CORE_BUSY_TSC / wall clock, and that is self-reported:
// code adds the cycles it BELIEVES were work. For Core 0 the timer ISR
// added one whole tick's worth (`freq / 100`) on every tick — that is
// exactly the wall clock, unconditionally, 100 times a second. Clamped
// with `.min(100)`, Core 0 could therefore never report anything but
// 99-100 %, no matter what it did. It was a declaration from the era
// when the shell loop really did spin, and it outlived the `hlt` that
// replaced the spinning.
//
// The honest counter was already there — `record_halt` at every HLT
// site — and only `cores` used it. Now every core is measured the same
// way: a spinner never halts and shows ~100 %, a core idling on the
// timer shows ~0 %.
```

## L652-654 · `if delta_mperf == 0 {`

```
// Effective running frequency: APERF/MPERF * nominal_TSC_freq.
// TSC is calibrated to nominal base; MPERF ticks at that same rate.
// A core that did not run in the window has no frequency: 0 = asleep.
```

## L659 · `let eff_mhz = ((delta_aperf as u128) * (nominal_mhz as u128)`

```
// Guard against overflow: cap aperf delta ratio implicitly via u128.
```

## L666 · `pub fn add_busy_tsc(core_id: usize, cycles: u64) {`

```
/// Record task execution time on a core (called from AP work loop).
```

## L672 · `pub fn start_work(core_id: usize) {`

```
/// Start tracking work time for a core (called when task begins or resumes).
```

## L678-679 · `pub fn flush_busy(core_id: usize) -> u64 {`

```
/// Flush accumulated work time since last start_work (called before wait/idle).
/// Returns the flushed cycles count.
```

## L689 · `pub fn set_active(core_id: usize, active: bool) {`

```
/// Set per-core active flag (true = executing work, false = waiting/idle).
```

## L696 · `pub fn core_freq_mhz(core_id: usize) -> u32 {`

```
/// Get last measured frequency in MHz for a core.
```

## L702 · `pub fn max_turbo_mhz() -> u32 { MAX_TURBO_MHZ.load(Ordering::Relaxed) }`

```
/// Max turbo frequency in MHz (from HWP capabilities).
```

## L705 · `pub fn min_eff_mhz() -> u32 { MIN_EFF_MHZ.load(Ordering::Relaxed) }`

```
/// Min efficiency frequency in MHz (from HWP capabilities).
```

## L708-709 · `pub fn core_usage(core_id: usize) -> u32 {`

```
/// Per-core CPU usage in percent (0-100): 100 − halted% over the last
/// sampling window. Measured, not self-reported.
```

## L715-718 · `static APIC_BASE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);`

```
/// Read current core's sequential ID via LAPIC.
/// Cached APIC base (MSR 0x1B). Identical on every core and never changes, so
/// reading it once avoids an `rdmsr` — a VM-exit under virtualization — on every
/// current_core_id() call (hot: poll loops, idle, core-gating).
```

## L726 · `unsafe { core::arch::asm!("rdmsr", in("ecx") 0x1Bu32, out("eax") lo, out("edx") hi); }`

```
// SAFETY: MSR 0x1B (APIC base) always readable on x86_64 ring 0.
```

## L731-732 · `let apic_id = unsafe { core::ptr::read_volatile((base + 0x20) as *const u32) } >> 24;`

```
// SAFETY: APIC MMIO is identity-mapped. One MMIO read remains (the per-core
// APIC ID register); the rdmsr VM-exit is gone.
```

## L734 · `APIC_TO_CORE[(apic_id & 0xFF) as usize].load(Relaxed) as usize`

```
// An unregistered id reads as core 0, as the old list search did.
```

## L738-744 · `static APIC_TO_CORE: [core::sync::atomic::AtomicU16; 256] =`

```
/// xAPIC id → sequential core id, filled once per core at registration.
///
/// `current_core_id` used to lock `CORES` and search the list on every call —
/// a global lock and a bouncing cache line on the hottest paths (`poll_rx_only`,
/// `pump_peers`, every fiber yield). The mapping never changes after boot, so
/// a table of atomics answers it without either. The xAPIC id register holds
/// 8 bits, hence 256 entries.
```

## L748 · `static CORE_APIC: [AtomicU32; 256] = [const { AtomicU32::new(0) }; 256];`

```
/// Sequential core id → xAPIC id, the reverse of `APIC_TO_CORE`.
```

## L758-760 · `pub fn enable_hwp() -> bool {`

```
/// Enable Hardware P-states (HWP / Speed Shift) on the current core.
/// CPU automatically scales frequency: idle → min, load → turbo.
/// Returns true if HWP was enabled successfully.
```

## L762 · `let eax: u32;`

```
// Check HWP support: CPUID.06H:EAX bit 7
```

## L778-779 · `unsafe { core::arch::asm!("wrmsr", in("ecx") 0x770u32, in("eax") 1u32, in("edx") 0u32); }`

```
// Enable HWP: MSR 0x770 (IA32_PM_ENABLE) = 1
// SAFETY: HWP supported (checked above), ring 0
```

## L782-783 · `let cap_lo: u32;`

```
// Read HWP capabilities: MSR 0x771 (IA32_HWP_CAPABILITIES)
// [7:0]=Highest, [15:8]=Guaranteed, [23:16]=Efficient, [31:24]=Lowest
```

## L785 · `unsafe { core::arch::asm!("rdmsr", in("ecx") 0x771u32, out("eax") cap_lo, out("edx") _); }`

```
// SAFETY: MSR 0x771 exists when HWP is supported
```

## L790 · `if MAX_TURBO_MHZ.load(Ordering::Relaxed) == 0 {`

```
// Store platform limits (first caller wins)
```

## L796-807 · `let hwp_req = (lowest as u32)`

```
// Configure HWP request: MSR 0x774 (IA32_HWP_REQUEST)
// [7:0]=Min, [15:8]=Max, [23:16]=Desired(0=auto), [31:24]=EPP
// EPP range: 0=perf, 128=balanced, 192=power_save, 255=max_power_save
//
// EPP=0 (max perf): CPU ramps to turbo on the first instruction of a
// burst. Right for desktop / mains-powered targets like the N100 NUC.
// Bursty workloads (crypto, I/O batches) live in the ~10-100 ms range
// — too short for the firmware's default heuristics with EPP=192 to
// notice and ramp; the CPU sat at base clock and software ChaCha20
// throughput was capped accordingly. Mobile targets that care about
// battery would set this to 128 or 192 — make it a `sys/config/hwp_epp`
// tunable when the notebook hardware lands.
```

## L811 · `unsafe { core::arch::asm!("wrmsr", in("ecx") 0x774u32, in("eax") hwp_req, in("edx") 0u32); }`

```
// SAFETY: MSR 0x774 exists when HWP is enabled
```

## L817-821 · `pub fn amd_cstate_base() -> Option<u16> {`

```
/// AMD Zen C-state base address (MSRC001_0073 CStateBaseAddr, bits 15:0):
/// an I/O read of CStateBaseAddr+n is trapped by the core as a request for
/// C-state n (ACPI `_CST` lists these ports as SystemIO entries). None off
/// AMD, before family 17h, under a hypervisor (the MSR is not emulated and
/// a read would #GP), or when the firmware left it 0 (no trapping).
```

## L827 · `unsafe {`

```
// SAFETY: CPUID leaf 1 exists everywhere; rbx is reserved by LLVM.
```

## L832 · `if ecx & (1 << 31) != 0 { return None; } // hypervisor`

```
// hypervisor
```

## L837-838 · `unsafe {`

```
// SAFETY: MSRC001_0073 is architectural on AMD family 17h+ (PPR
// Core::X86::Msr::CStateBaseAddr), gated on vendor, family and bare metal.
```

## L847-849 · `static IN_LOOP: [AtomicBool; 256] = [const { AtomicBool::new(false) }; 256];`

```
/// Worker ist in seiner Schleife angekommen. Ein AP, der nie startete,
/// darf beim Verteilen nicht als „leerster Kern" zaehlen — er nimmt nie
/// etwas, und die Arbeit bliebe liegen.
```

## L851 · `static WOKE_TSC: [AtomicU64; 256] = [const { AtomicU64::new(0) }; 256];`

```
/// TSC stamped by a core first thing after `hlt` (the `tsc_offset` probe).
```

## L860-864 · `pub fn tsc_offset(c: usize) -> Option<(i64, u64)> {`

```
/// Signed TSC offset of core `c` against the caller's TSC, in cycles, by a
/// wake-IPI round trip: the woken core stamps its own TSC first thing after
/// `hlt`; the caller's midpoint of send and receipt is the same instant ±
/// half the round trip. Returns (offset, round trip). None if `c` is not
/// halted or did not answer. Costs `c` one wake.
```

## L884 · `static IDLE: [AtomicBool; 256] = [const { AtomicBool::new(false) }; 256];`

```
/// Worker is halted in its idle path and needs an IPI to see new work.
```

## L887-890 · `pub fn wake_idle_workers() {`

```
/// New work is in the inbox: send the wake IPI to every idle worker. Each
/// one re-runs its loop; the placement rule (`least_loaded`) picks who takes
/// it, the others halt again. Spawns are rare, so waking all idle cores is
/// cheaper than guessing the right one and stranding the work on a miss.
```

## L898-900 · `pub fn wake_core(c: usize) {`

```
/// Wake worker `c` if it is halted in its idle path. The caller has already
/// published what `c` should find (a queued fiber, inbox work) — `c` sets
/// IDLE before its last look, so either it sees the work or we see IDLE.
```

## L910-914 · `pub fn core0_loop() -> ! {`

```
/// Core 0's loop after boot (stage 3c): the same fiber scheduler as a
/// worker, with the shell (`intent::run_loop`) as a fiber. Since stage 3e
/// without a periodic tick: the halt ends at the earliest fiber deadline, on
/// an interrupt, or by the wake IPI when another core signals a fiber here.
/// Core 0 still takes no inbox work.
```

## L916-917 · `crate::interrupts::make_core0_tickless();`

```
// No periodic tick from here on (stage 3e): Core 0's LAPIC timer becomes
// the one-shot deadline timer every worker has.
```

## L921 · `update_core_freq(0);`

```
// Was done by the tick every second; self-throttled to 100 ms windows.
```

## L933 · `static NATIVE_BUSY: [AtomicBool; 256] = [const { AtomicBool::new(false) }; 256];`

```
/// Ein Intent laeuft gerade auf diesem Kern (bis zum Ende, ohne abzugeben).
```

## L935 · `static NATIVE_NAME: [(AtomicUsize, AtomicUsize); 256] =`

```
/// The native task each core runs, as a `&'static str` split in two words.
```

## L939 · `pub fn native_task(c: usize) -> Option<&'static str> {`

```
/// The native task core `c` is running right now, if any (`cores`).
```

## L947-948 · `Some(unsafe { core::str::from_utf8_unchecked(core::slice::from_raw_parts(p as *const u8, l)) })`

```
// SAFETY: stored from a `&'static str` in the worker loop below; the
// pointer and length belong together and outlive everything.
```

## L952-953 · `fn core_load(c: usize) -> u64 {`

```
/// Last eines Kerns fuers Verteilen: residente Fiber, plus eins, solange
/// ein Intent ihn belegt.
```

## L958-963 · `fn least_loaded(cid: usize) -> bool {`

```
/// Gehoert `cid` zu den am wenigsten belegten Workern, die Arbeit nehmen?
///
/// Ausgenommen sind der Kern des WLAN-Treibers und der microVM-Kern: beide
/// nehmen nie etwas, und zaehlten sie mit, koennte das Minimum auf einem
/// Kern liegen, der nie zugreift. Gleichstand ist erlaubt — mehrere leere
/// Kerne duerfen gleichzeitig zugreifen, der Deque entscheidet.
```

## L973-975 · `if c == cid || Some(c) == nic || c == vm || !IN_LOOP[c].load(Ordering::Acquire)`

```
// Ein Kern mitten in einem Intent kann gerade nichts annehmen;
// zaehlte er beim Minimum mit, wartete neue Arbeit auf das Ende
// eines Downloads.
```

## L988-989 · `#[unsafe(no_mangle)]`

```
/// AP Rust entry — called by trampoline after long mode transition.
/// Interrupts are disabled (cli from trampoline). IDT is loaded.
```

## L992 · `let apic_base = super::read_apic_base();`

```
// Enable Local APIC (needed for IPI wakeup fallback)
```

## L994 · `unsafe {`

```
// SAFETY: APIC MMIO is identity-mapped, each core sees its own LAPIC
```

## L1002 · `enable_hwp();`

```
// Enable HWP on this AP (per-core frequency scaling)
```

## L1005 · `super::AP_STARTED.fetch_add(1, Ordering::Release);`

```
// Signal BSP: this AP is alive
```

## L1008 · `while !SCHEDULER_READY.load(Ordering::Acquire) {`

```
// Wait until scheduler is initialized
```

## L1013-1016 · `crate::interrupts::init_worker_timer();`

```
// This core's own LAPIC timer, one-shot to whatever deadline it waits
// for — its idle HLT needs a wake source independent of the host. Plain
// HLT VMEXITs, so KVM frees the host core (MWAIT-on-cacheline needed
// cpu-pm=on). New work wakes it by IPI (`wake_idle_workers`).
```

## L1022 · `let cid = core_id as usize;`

```
// Enter scheduler loop
```

## L1026-1034 · `if DEDICATED_VM_CORE.load(Ordering::Acquire) == cid as u32 {`

```
// Carved out of the work-stealing pool for the microvm
// (substrate rework A2). `vm_core_serve` opens the guest ON
// THIS CORE when a launch is pending and runs it continuously
// to exit (blocking the core for the guest's whole lifetime —
// that is the point: it no longer fights Shade + the shell for
// Core 0). Cheap no-op when nothing is pending → we halt and
// re-check within ~10 ms. Default
// sentinel never matches a real cid, so ≤2-core / no-AP hosts
// keep the exact old work-stealing loop.
```

## L1040 · `let d = crate::interrupts::rdtsc() + crate::interrupts::tsc_freq() / 100;`

```
// Re-check for a pending launch every 10 ms.
```

## L1046-1057 · `let nic_core = crate::netdev::wasm_nic_core() == Some(cid)`

```
// **Der Kern des WLAN-Treibers nimmt keine neue Arbeit an**, solange
// es andere Worker gibt.
//
// Ein leerer Worker sieht alle 10 ms nach neuer Arbeit; der Kern des
// Treibers wacht JEDE Millisekunde auf (`sleep_ms(1)` im Pumpweg)
// und fragt dabei zuerst hier. Er gewann deshalb praktisch jedes neue
// Intent — auch den Download, der die Rahmen des Treibers abholt.
// Ein Intent laeuft bis zum Ende und gibt den Kern nur ueber
// `pump_peers` her: Treiber und Leser liefen ABWECHSELND statt
// nebeneinander. Am Geraet (VHT80, 2026-09-22): 63 % der Laufzeit
// Stillstand auf der Luft, `rx ring dropped 255`, 255 Quittungen auf
// einmal im Sendering, Server-RTT 18 ms auf einer Strecke von 2.
```

## L1060-1068 · `let take = !nic_core && !crate::microvm::cpu::is_vm_worker_core(cid)`

```
// **Und allgemein: neue Arbeit nimmt nur ein Kern, der zu den am
// wenigsten belegten gehoert.** Dieselbe Ursache in gross: am
// Geraet (16 Kerne) lagen dock, bar, aml, audio_hda, i2c_hid, wifid
// UND der WLAN-Treiber alle auf Kern 5 — der Kern, der als erster
// einen Fiber hatte, wachte am haeufigsten auf und stahl damit auch
// alle folgenden. Fiber sind kooperativ: dreht der Treiber unter
// Last, warten Touchpad, Ton und Panels, und umgekehrt.
// Ein Kern mit vCPU oder VM-Arbeiter nimmt nichts Neues an: Fiber sind
// kooperativ, und neben einer drehenden vCPU kaeme es kaum dran.
```

## L1071-1072 · `if let Some(task) = if take { super::scheduler::next_task(cid) } else { None } {`

```
// Admit a freshly-spawned app as a fiber, or run a native intent.
// New work arrives in the shared inbox (`scheduler::spawn`).
```

## L1075-1076 · `super::fiber::admit(cid, task.func, task.arg);`

```
// App: hand to this core's fiber scheduler. It runs on its
// own stack and yields at npk_sleep so peers share the core.
```

## L1079 · `CORE_ACTIVE[cid].store(true, Ordering::Relaxed);`

```
// Native run-to-completion task (intent) — run directly.
```

## L1093-1095 · `CORE_ACTIVE[cid].store(true, Ordering::Relaxed);`

```
// Run this core's fibers round-robin: resume any whose sleep
// deadline passed, park the rest. Returns when all are parked /
// none remain — then the core halts below.
```

## L1102 · `update_core_freq(cid);`

```
// Before sleep: update usage stats (delta covers work + idle since last call)
```

## L1105-1112 · `IDLE[cid].store(true, Ordering::SeqCst);`

```
// Idle: halt until the earliest parked fiber is due, or until an
// interrupt — a device IRQ for a fiber in `irq_wait`, a kick, or
// the wake IPI for new work. No periodic tick.
//
// IDLE is published BEFORE the inbox is looked at again: a spawn
// that pushed without seeing IDLE sent no IPI, so its work must be
// seen by this re-check; one that saw IDLE sends the IPI, which
// stays pending (IF=0) and ends the halt below at once.
```

## L1118 · `continue; // became runnable — round again`

```
// became runnable — round again
```

## L1125-1128 · `let recheck = now + crate::interrupts::tsc_freq() / 100;`

```
// Work waits that this core declines — for another core. If
// that one is busy inside its fibers it takes the work only
// when it gets back to the top of its loop; look again in
// 10 ms so the work cannot be stranded behind a changed load.
```

