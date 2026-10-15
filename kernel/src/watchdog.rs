//! Hard-lockup watchdog with a report that survives a warm reset (Linux
//! kernel/watchdog_perf.c and fs/pstore/ram.c).
//!
//! Every core counts its unhalted cycles in a PMU counter that overflows
//! about once a second of busy time and raises an NMI, which arrives even
//! with interrupts masked. A core makes progress when it dispatches a fiber,
//! halts, enters its guest or refuels compiled wasm (`touch`); a core that
//! takes `LIMIT` overflow NMIs without progress is locked up. Linux counts
//! hrtimer interrupts for the same purpose; cores here are tickless, so the
//! scheduler's own steps are the heartbeat. An idle core does not count
//! cycles and is never suspected.
//!
//! On a lockup the core writes its state into a RAM region the frame
//! allocator never hands out, stops every other core with an NMI that makes
//! it record its own state, flushes the region to DRAM and resets the
//! machine warm (FADT reset register, then port 0xCF9 = 6), which leaves DRAM
//! contents in place. The next boot finds the report there, prints it and
//! clears it. Nothing on that path takes a lock or allocates.
//!
//! The region is the top `REGION` bytes of the highest conventional-memory
//! range in the UEFI map that lies above 4 GiB and inside the identity map:
//! firmware and the boot stub allocate low first, so that range is the one
//! a warm reset is least likely to overwrite. Without such a range the
//! report goes to COM1 and the screen and the machine halts instead.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering};

use crate::hw::msr;
use crate::kprintln;

const CORES: usize = 256;
/// Slots in the report; a core above that is not recorded.
const SLOTS: usize = 32;
const SLOT_BYTES: usize = 512;
const HEADER_BYTES: usize = 256;
const REGION: u64 = 32 * 1024;
const MAGIC: u64 = u64::from_le_bytes(*b"NPKLOCK1");
const VERSION: u32 = 1;
/// Return addresses kept per core from its stack.
const STACK_WORDS: usize = 40;
/// Overflow NMIs without progress that make a lockup (about 10-30 s).
const DEFAULT_LIMIT: u32 = 20;
/// Two overflow NMIs closer than this count as a strike; this many strikes
/// in a row turn the watchdog off on that core (a counter that misbehaves
/// must not storm the core with NMIs).
const STORM_GAP_DIV: u64 = 20;
const STORM_STRIKES: u32 = 10;
/// Identity-mapped RAM, as `paging` maps it.
const IDENTITY_TOP: u64 = 64 << 30;
const EFI_CONVENTIONAL_MEMORY: u32 = 7;

const PMU_NONE: u8 = 0;
const PMU_INTEL: u8 = 1;
const PMU_AMD_LEGACY: u8 = 2;
const PMU_AMD_CORE: u8 = 3;

const IA32_PMC0: u32 = 0xC1;
const IA32_PERFEVTSEL0: u32 = 0x186;
const IA32_PERF_GLOBAL_CTRL: u32 = 0x38F;
const IA32_PERF_GLOBAL_OVF_CTRL: u32 = 0x390;
const AMD_PERFEVTSEL0: u32 = 0xC001_0000;
const AMD_PERFCTR0: u32 = 0xC001_0004;
const AMD_CORE_CTL0: u32 = 0xC001_0200;
const AMD_CORE_CTR0: u32 = 0xC001_0201;
const AMD_GLOBAL_CTL: u32 = 0xC000_0301;
const AMD_GLOBAL_STATUS_CLR: u32 = 0xC000_0302;
/// PerfEvtSel: USR, OS, INT, EN.
const EVTSEL_FLAGS: u64 = (1 << 16) | (1 << 17) | (1 << 20) | (1 << 22);
/// Intel UnHalted Core Cycles (event 0x3C, umask 0).
const INTEL_CYCLES: u64 = 0x3C;
/// AMD PMCx076, cycles not in halt.
const AMD_CYCLES: u64 = 0x76;
/// LAPIC LVT performance counter: delivery mode NMI, unmasked.
const LAPIC_LVTPC: u32 = 0x340;
const LVT_NMI: u32 = 0b100 << 8;

/// Progress of each core (`touch`).
static PROGRESS: [AtomicU32; CORES] = [const { AtomicU32::new(0) }; CORES];
static SEEN: [AtomicU32; CORES] = [const { AtomicU32::new(0) }; CORES];
static STALE: [AtomicU32; CORES] = [const { AtomicU32::new(0) }; CORES];
static NMIS: [AtomicU64; CORES] = [const { AtomicU64::new(0) }; CORES];
static LAST_NMI: [AtomicU64; CORES] = [const { AtomicU64::new(0) }; CORES];
static STRIKES: [AtomicU32; CORES] = [const { AtomicU32::new(0) }; CORES];
static ARMED: [AtomicBool; CORES] = [const { AtomicBool::new(false) }; CORES];

static ENABLED: AtomicBool = AtomicBool::new(true);
static DETECTED: AtomicBool = AtomicBool::new(false);
static PMU: AtomicU8 = AtomicU8::new(PMU_NONE);
static PMU_V2: AtomicBool = AtomicBool::new(false);
static PERIOD: AtomicU64 = AtomicU64::new(0);
static WIDTH: AtomicU32 = AtomicU32::new(48);
static LIMIT: AtomicU32 = AtomicU32::new(DEFAULT_LIMIT);
static DUMPING: AtomicBool = AtomicBool::new(false);
/// Physical address of the report region; 0 = none.
static PSTORE: AtomicU64 = AtomicU64::new(0);

/// The report found at boot, kept for `lockup`.
static mut SAVED: [u8; HEADER_BYTES + SLOTS * SLOT_BYTES] = [0; HEADER_BYTES + SLOTS * SLOT_BYTES];
static SAVED_VALID: AtomicBool = AtomicBool::new(false);
/// Where the report is written when there is no region: printed, then halt.
static mut FALLBACK: [u8; HEADER_BYTES + SLOTS * SLOT_BYTES] = [0; HEADER_BYTES + SLOTS * SLOT_BYTES];
static FILLED: AtomicUsize = AtomicUsize::new(0);

unsafe extern "C" {
    static __image_base: u8;
    static __text_start: u8;
    static __text_end: u8;
}

/// The address this image is linked at (linker.ld); report addresses are
/// given against it, for `addr2line -e nopeekos-kernel`.
const LINK_BASE: u64 = 0x1000_0000;

fn image_base() -> u64 {
    // SAFETY: a linker symbol; only its address is taken.
    unsafe { &__image_base as *const u8 as u64 }
}

fn text_range() -> (u64, u64) {
    // SAFETY: linker symbols; only their addresses are taken.
    unsafe { (&__text_start as *const u8 as u64, &__text_end as *const u8 as u64) }
}

/// Core `cid` made progress.
#[inline]
pub fn touch(cid: usize) {
    if cid < CORES {
        PROGRESS[cid].fetch_add(1, Ordering::Relaxed);
    }
}

/// The calling core made progress.
#[inline]
pub fn touch_here() {
    touch(crate::smp::per_core::current_core_id());
}

fn cpuid(leaf: u32, sub: u32) -> (u32, u32, u32, u32) {
    let r = core::arch::x86_64::__cpuid_count(leaf, sub);
    (r.eax, r.ebx, r.ecx, r.edx)
}

/// Which PMU the watchdog can use, once (`DETECTED`).
fn detect() {
    if DETECTED.swap(true, Ordering::AcqRel) {
        return;
    }
    let (max, b, c, d) = cpuid(0, 0);
    let mut vendor = [0u8; 12];
    vendor[..4].copy_from_slice(&b.to_le_bytes());
    vendor[4..8].copy_from_slice(&d.to_le_bytes());
    vendor[8..].copy_from_slice(&c.to_le_bytes());
    let hypervisor = cpuid(1, 0).2 & (1 << 31) != 0;
    let tsc = crate::interrupts::tsc_freq().max(1_000_000_000);
    if &vendor == b"GenuineIntel" && max >= 0xA {
        let (a, ebx, _, _) = cpuid(0xA, 0);
        let version = a & 0xFF;
        let counters = (a >> 8) & 0xFF;
        let width = (a >> 16) & 0xFF;
        let events = (a >> 24) & 0xFF;
        // EBX bit 0 set = the core-cycles event is not available.
        if version >= 1 && counters >= 1 && width >= 32 && events >= 1 && ebx & 1 == 0 {
            PMU_V2.store(version >= 2, Ordering::Relaxed);
            WIDTH.store(width, Ordering::Relaxed);
            // A write to IA32_PMC0 sets 32 bits, sign-extended.
            PERIOD.store(tsc.min((1 << 31) - 1), Ordering::Relaxed);
            PMU.store(PMU_INTEL, Ordering::Release);
        }
    } else if &vendor == b"AuthenticAMD" || &vendor == b"HygonGenuine" {
        let ext = cpuid(0x8000_0000, 0).0;
        let core_ctrs = ext >= 0x8000_0001 && cpuid(0x8000_0001, 0).2 & (1 << 23) != 0;
        // A hypervisor without a virtual PMU refuses the legacy MSRs.
        let pmu = if core_ctrs { PMU_AMD_CORE } else if !hypervisor { PMU_AMD_LEGACY } else { PMU_NONE };
        if pmu != PMU_NONE {
            PMU_V2.store(ext >= 0x8000_0022 && cpuid(0x8000_0022, 0).0 & 1 != 0, Ordering::Relaxed);
            WIDTH.store(48, Ordering::Relaxed);
            PERIOD.store(tsc, Ordering::Relaxed);
            PMU.store(pmu, Ordering::Release);
        }
    }
    match PMU.load(Ordering::Acquire) {
        PMU_NONE => kprintln!("[npk] watchdog: no usable PMU counter, hard-lockup detection off"),
        p => crate::kdebug!("[npk] watchdog: {} counter, NMI every {} M busy cycles, lockup after {}",
            pmu_name(p), PERIOD.load(Ordering::Relaxed) / 1_000_000, LIMIT.load(Ordering::Relaxed)),
    }
}

fn pmu_name(p: u8) -> &'static str {
    match p {
        PMU_INTEL => "Intel architectural",
        PMU_AMD_LEGACY => "AMD legacy",
        PMU_AMD_CORE => "AMD core",
        _ => "none",
    }
}

fn msrs(p: u8) -> (u32, u32) {
    match p {
        PMU_INTEL => (IA32_PERFEVTSEL0, IA32_PMC0),
        PMU_AMD_LEGACY => (AMD_PERFEVTSEL0, AMD_PERFCTR0),
        _ => (AMD_CORE_CTL0, AMD_CORE_CTR0),
    }
}

fn width_mask() -> u64 {
    let w = WIDTH.load(Ordering::Relaxed).min(63);
    (1u64 << w) - 1
}

/// Load the counter with -PERIOD, clear its overflow and unmask the LVT
/// entry, which the NMI delivery masked.
fn rearm(p: u8) {
    let (_, ctr) = msrs(p);
    let start = 0u64.wrapping_sub(PERIOD.load(Ordering::Relaxed)) & width_mask();
    // SAFETY: the counter MSR of the PMU `detect` found; writing it only
    // sets the count.
    unsafe { msr::write(ctr, start) };
    if PMU_V2.load(Ordering::Relaxed) {
        let clr = if p == PMU_INTEL { IA32_PERF_GLOBAL_OVF_CTRL } else { AMD_GLOBAL_STATUS_CLR };
        // SAFETY: the overflow-clear MSR of a version-2 PMU; bit 0 clears
        // counter 0's overflow and nothing else.
        unsafe { msr::write(clr, 1) };
    }
    crate::interrupts::lapic().w32(LAPIC_LVTPC, LVT_NMI);
}

/// Start the watchdog on the calling core. Each core calls it once its
/// LAPIC timer is set up.
pub fn init_core() {
    detect();
    let p = PMU.load(Ordering::Acquire);
    let cid = crate::smp::per_core::current_core_id();
    if p == PMU_NONE || cid >= CORES || !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let (sel, _) = msrs(p);
    let event = if p == PMU_INTEL { INTEL_CYCLES } else { AMD_CYCLES };
    // SAFETY: the event-select MSR of the PMU `detect` found; 0 stops the
    // counter while it is loaded.
    unsafe { msr::write(sel, 0) };
    rearm(p);
    if PMU_V2.load(Ordering::Relaxed) {
        let ctl = if p == PMU_INTEL { IA32_PERF_GLOBAL_CTRL } else { AMD_GLOBAL_CTL };
        // SAFETY: the global control of a version-2 PMU; bit 0 enables
        // counter 0, the other bits are kept.
        unsafe { msr::write(ctl, msr::read(ctl) | 1) };
    }
    SEEN[cid].store(PROGRESS[cid].load(Ordering::Relaxed), Ordering::Relaxed);
    STALE[cid].store(0, Ordering::Relaxed);
    ARMED[cid].store(true, Ordering::Release);
    // SAFETY: as above; this starts counting unhalted cycles with an
    // interrupt (the LVT's NMI) on overflow.
    unsafe { msr::write(sel, event | EVTSEL_FLAGS) };
}

/// Stop the counter on the calling core.
fn disarm(p: u8, cid: usize) {
    ARMED[cid].store(false, Ordering::Release);
    let (sel, _) = msrs(p);
    // SAFETY: the event-select MSR of the detected PMU; 0 stops it.
    unsafe { msr::write(sel, 0) };
    crate::interrupts::lapic().w32(LAPIC_LVTPC, LVT_NMI | (1 << 16));
}

/// What the interrupted code was doing, for the report.
#[derive(Clone, Copy)]
pub struct Frame {
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
}

/// The NMI handler's part. True when the NMI was the watchdog's (counter
/// overflow); a core asked to record its state for a report never returns.
pub fn nmi(frame: Frame) -> bool {
    let cid = crate::smp::per_core::current_core_id();
    if DUMPING.load(Ordering::Acquire) {
        record(cid, frame, false);
        crate::fatal::halt();
    }
    if cid >= CORES || !ARMED[cid].load(Ordering::Acquire) {
        return false;
    }
    let p = PMU.load(Ordering::Relaxed);
    let (_, ctr) = msrs(p);
    // SAFETY: the counter MSR of the detected PMU; reading has no effect.
    let count = unsafe { msr::read(ctr) } & width_mask();
    // Armed at -PERIOD it counts up with its top bit set until it wraps.
    if count >> (WIDTH.load(Ordering::Relaxed) - 1) & 1 != 0 {
        return false;
    }
    rearm(p);
    if !ENABLED.load(Ordering::Relaxed) {
        disarm(p, cid);
        return true;
    }
    let now = crate::interrupts::rdtsc();
    let gap = crate::interrupts::tsc_freq() / STORM_GAP_DIV;
    if now.wrapping_sub(LAST_NMI[cid].swap(now, Ordering::Relaxed)) < gap {
        if STRIKES[cid].fetch_add(1, Ordering::Relaxed) + 1 >= STORM_STRIKES {
            disarm(p, cid);
            return true;
        }
    } else {
        STRIKES[cid].store(0, Ordering::Relaxed);
    }
    NMIS[cid].fetch_add(1, Ordering::Relaxed);
    let seen = PROGRESS[cid].load(Ordering::Relaxed);
    if seen != SEEN[cid].swap(seen, Ordering::Relaxed) {
        STALE[cid].store(0, Ordering::Relaxed);
        return true;
    }
    if STALE[cid].fetch_add(1, Ordering::Relaxed) + 1 < LIMIT.load(Ordering::Relaxed) {
        return true;
    }
    lockup(cid, frame)
}

fn region() -> *mut u8 {
    match PSTORE.load(Ordering::Acquire) {
        0 => core::ptr::addr_of_mut!(FALLBACK) as *mut u8,
        a => a as *mut u8,
    }
}

fn put64(base: *mut u8, off: usize, v: u64) {
    // SAFETY: `base` is the report region (pstore or `FALLBACK`), at least
    // `HEADER_BYTES + SLOTS * SLOT_BYTES` long; callers stay inside it.
    unsafe { core::ptr::write_volatile(base.add(off) as *mut u64, v) };
}

fn get64(base: *const u8, off: usize) -> u64 {
    // SAFETY: as `put64`, for reads.
    unsafe { core::ptr::read_volatile(base.add(off) as *const u64) }
}

/// Slot of core `cid`: valid+core, rip, cs, rflags, rsp, progress state,
/// then up to `STACK_WORDS` return addresses from the stack.
fn record(cid: usize, f: Frame, trigger: bool) {
    if cid >= SLOTS {
        return;
    }
    let base = region();
    let s = HEADER_BYTES + cid * SLOT_BYTES;
    put64(base, s + 8, f.rip);
    put64(base, s + 16, f.cs);
    put64(base, s + 24, f.rflags);
    put64(base, s + 32, f.rsp);
    let stale = STALE.get(cid).map_or(0, |x| x.load(Ordering::Relaxed)) as u64;
    put64(base, s + 40, stale | (NMIS.get(cid).map_or(0, |x| x.load(Ordering::Relaxed)) << 32));
    // Words from the stack that point into kernel text: return addresses,
    // and the odd stale one. Only the page RSP is in, which is mapped.
    let (t0, t1) = text_range();
    let mut n = 0usize;
    let rsp = f.rsp & !7;
    if (0x10_0000..IDENTITY_TOP).contains(&rsp) {
        let end = (rsp | 0xFFF) + 1;
        let mut a = rsp;
        while a < end && n < STACK_WORDS {
            // SAFETY: inside the 4 KB page that holds RSP, in the identity map.
            let w = unsafe { core::ptr::read_volatile(a as *const u64) };
            if (t0..t1).contains(&w) {
                put64(base, s + 56 + n * 8, w);
                n += 1;
            }
            a += 8;
        }
    }
    put64(base, s + 48, n as u64);
    let flags = 1u64 | if trigger { 2 } else { 0 };
    put64(base, s, flags | (cid as u64) << 32);
    FILLED.fetch_add(1, Ordering::AcqRel);
}

/// Core `cid` made no progress for `LIMIT` overflow NMIs: record every core,
/// keep the record, reset.
fn lockup(cid: usize, f: Frame) -> ! {
    if DUMPING.swap(true, Ordering::AcqRel) {
        record(cid, f, false);
        crate::fatal::halt();
    }
    // SAFETY: masking interrupts on the way down; nothing returns here.
    unsafe { core::arch::asm!("cli", options(nomem, nostack)) };
    let base = region();
    for off in (0..HEADER_BYTES + SLOTS * SLOT_BYTES).step_by(8) {
        put64(base, off, 0);
    }
    record(cid, f, true);
    crate::fatal::nmi_others();
    let hz = crate::interrupts::tsc_freq().max(1);
    let until = crate::interrupts::rdtsc() + hz / 5;
    let others = crate::smp::per_core::core_count().clamp(1, SLOTS) - 1;
    while FILLED.load(Ordering::Acquire) < others + 1 && crate::interrupts::rdtsc() < until {
        core::hint::spin_loop();
    }
    put64(base, 8, VERSION as u64 | (cid as u64) << 32);
    put64(base, 16, crate::interrupts::rdtsc());
    put64(base, 24, hz);
    put64(base, 32, image_base());
    put64(base, 40, LIMIT.load(Ordering::Relaxed) as u64 | PERIOD.load(Ordering::Relaxed) << 16);
    let ver = env!("CARGO_PKG_VERSION").as_bytes();
    let mut v = [0u8; 16];
    let n = ver.len().min(16);
    v[..n].copy_from_slice(&ver[..n]);
    put64(base, 48, u64::from_le_bytes(v[..8].try_into().unwrap()));
    put64(base, 56, u64::from_le_bytes(v[8..].try_into().unwrap()));
    put64(base, 0, MAGIC);
    let pstore = PSTORE.load(Ordering::Acquire);
    if pstore != 0 {
        crate::mm::clflush_range(pstore, REGION);
    }
    if let Some(mut w) = crate::fatal::emergency() {
        use core::fmt::Write;
        if pstore != 0 {
            let _ = writeln!(w, "hard lockup on core {}: report saved at {:#x}, resetting", cid, pstore);
        } else {
            let _ = writeln!(w, "hard lockup on core {} (no report region, halting)", cid);
            // SAFETY: `FALLBACK` is written above and no core changes it now.
            let rep = unsafe { &*core::ptr::addr_of!(FALLBACK) };
            format_report(rep, |line| { let _ = writeln!(w, "{}", line); });
            crate::fatal::halt();
        }
    }
    if pstore == 0 {
        crate::fatal::halt();
    }
    crate::acpi::reset();
    // SAFETY: port 0xCF9 (reset control): 2 arms, 6 resets the platform
    // without a power cycle, so DRAM keeps its contents.
    unsafe {
        core::arch::asm!("out dx, al", in("dx") 0xCF9u16, in("al") 0x02u8, options(nomem, nostack));
        for _ in 0..100_000u32 { core::hint::spin_loop(); }
        core::arch::asm!("out dx, al", in("dx") 0xCF9u16, in("al") 0x06u8, options(nomem, nostack));
    }
    crate::fatal::halt()
}

/// Pick the report region from the UEFI map, keep the report found there
/// from the last boot, and return the range for the frame allocator to keep.
pub fn pstore_init(boot_info: &crate::boot_info::BootInfo) -> Option<(u64, u64)> {
    let n = (boot_info.region_count as usize).min(boot_info.regions.len());
    let best = boot_info.regions[..n].iter()
        .filter(|r| r.uefi_type == EFI_CONVENTIONAL_MEMORY && r.page_count * 4096 >= REGION)
        .map(|r| (r.physical_start, r.physical_start + r.page_count * 4096))
        .filter(|&(s, e)| s >= 4 << 30 && e <= IDENTITY_TOP)
        .max_by_key(|&(_, e)| e)?;
    let start = (best.1 - REGION) & !0xFFF;
    let p = start as *const u8;
    if get64(p, 0) == MAGIC {
        // SAFETY: written once here, on the boot core before any other core
        // or reader exists.
        let saved = unsafe { &mut *core::ptr::addr_of_mut!(SAVED) };
        for (i, b) in saved.iter_mut().enumerate() {
            // SAFETY: inside the region, identity-mapped by the firmware.
            *b = unsafe { core::ptr::read_volatile(p.add(i)) };
        }
        SAVED_VALID.store(true, Ordering::Release);
        put64(start as *mut u8, 0, 0);
        crate::mm::clflush_range(start, 64);
    }
    PSTORE.store(start, Ordering::Release);
    Some((start, REGION))
}

/// Print the report the last boot left, if any (boot log and `lockup`).
pub fn print_saved(always: bool) {
    if !SAVED_VALID.load(Ordering::Acquire) {
        if always {
            kprintln!("[npk] lockup: no report from the last boot");
        }
        return;
    }
    // SAFETY: written only by `pstore_init`, before this can run.
    let rep = unsafe { &*core::ptr::addr_of!(SAVED) };
    format_report(rep, |line| kprintln!("{}", line));
}

fn format_report(rep: &[u8], mut out: impl FnMut(&str)) {
    use core::fmt::Write;
    let base = rep.as_ptr();
    let q = |off: usize| get64(base, off);
    let mut line = heapless_line();
    let head = q(8);
    let trigger = (head >> 32) as usize;
    let img = q(32);
    let limit = q(40) & 0xFFFF;
    let period = q(40) >> 16;
    let mut ver = [0u8; 16];
    ver[..8].copy_from_slice(&q(48).to_le_bytes());
    ver[8..].copy_from_slice(&q(56).to_le_bytes());
    let ver = core::str::from_utf8(&ver).unwrap_or("?").trim_end_matches('\0');
    let link = |a: u64| a.wrapping_sub(img).wrapping_add(LINK_BASE);
    line.clear();
    let _ = write!(line, "[npk] watchdog: the last boot (kernel {}) locked up on core {}: no progress for {} \
        overflow NMIs ({} M busy cycles each)", ver, trigger, limit, period / 1_000_000);
    out(&line);
    out("[npk]   addresses are link addresses: addr2line -f -e target/x86_64-nopeek/release/nopeekos-kernel <addr>");
    for c in 0..SLOTS {
        let s = HEADER_BYTES + c * SLOT_BYTES;
        let flags = q(s);
        if flags & 1 == 0 {
            continue;
        }
        let rflags = q(s + 24);
        let st = q(s + 40);
        line.clear();
        let _ = write!(line, "[npk]   core {}{}: rip {:#x} cs {:#x} rflags {:#x} (IF={}) rsp {:#x}, {} NMIs, {} without progress",
            c, if flags & 2 != 0 { " (stuck)" } else { "" }, link(q(s + 8)), q(s + 16), rflags,
            (rflags >> 9) & 1, q(s + 32), st >> 32, st & 0xFFFF_FFFF);
        out(&line);
        let n = (q(s + 48) as usize).min(STACK_WORDS);
        if n > 0 {
            line.clear();
            let _ = write!(line, "[npk]     stack:");
            for i in 0..n {
                let _ = write!(line, " {:#x}", link(q(s + 56 + i * 8)));
                if line.len() > 160 {
                    out(&line);
                    line.clear();
                    let _ = write!(line, "[npk]           ");
                }
            }
            out(&line);
        }
    }
}

/// A fixed line buffer; the report is formatted where no heap may exist.
struct Line {
    buf: [u8; 256],
    len: usize,
}

fn heapless_line() -> Line {
    Line { buf: [0; 256], len: 0 }
}

impl Line {
    fn clear(&mut self) {
        self.len = 0;
    }
    fn len(&self) -> usize {
        self.len
    }
}

impl core::fmt::Write for Line {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let n = s.len().min(self.buf.len() - self.len);
        self.buf[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        Ok(())
    }
}

impl core::ops::Deref for Line {
    type Target = str;
    fn deref(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }
}

/// `watchdog`: state; `watchdog on|off`; `watchdog test [core]` spins that
/// core with interrupts masked, to prove the chain end to end.
pub fn intent(args: &str) {
    let mut it = args.split_whitespace();
    match it.next() {
        None => {
            let p = PMU.load(Ordering::Acquire);
            kprintln!("  Watchdog: {} ({} counter)", if ENABLED.load(Ordering::Relaxed) && p != PMU_NONE { "on" } else { "off" },
                pmu_name(p));
            if p != PMU_NONE {
                kprintln!("  NMI every {} M busy cycles, lockup after {} without progress",
                    PERIOD.load(Ordering::Relaxed) / 1_000_000, LIMIT.load(Ordering::Relaxed));
            }
            match PSTORE.load(Ordering::Acquire) {
                0 => kprintln!("  Report: no region above 4 GiB, a lockup is shown on COM1 and the screen"),
                a => kprintln!("  Report: {:#x} ({} KB), kept over a warm reset", a, REGION / 1024),
            }
            for c in 0..crate::smp::per_core::core_count().min(CORES) {
                kprintln!("  core {}: {} NMIs{}", c, NMIS[c].load(Ordering::Relaxed),
                    if ARMED[c].load(Ordering::Relaxed) { "" } else { ", not armed" });
            }
        }
        Some("on") => {
            ENABLED.store(true, Ordering::Relaxed);
            kprintln!("[npk] watchdog: on (cores that turned it off stay off until reboot)");
        }
        Some("off") => {
            ENABLED.store(false, Ordering::Relaxed);
            kprintln!("[npk] watchdog: off");
        }
        Some("test") => {
            let core = it.next().and_then(|s| s.parse::<usize>().ok()).unwrap_or(0);
            if PMU.load(Ordering::Acquire) == PMU_NONE {
                kprintln!("[npk] watchdog: no PMU counter, the test would only hang the core");
                return;
            }
            kprintln!("[npk] watchdog test: core {} spins with interrupts masked; expect a reset in 10-30 s", core);
            if core == 0 {
                spin_masked(0);
            } else {
                crate::smp::fiber::admit(core, spin_masked, core as u64);
            }
        }
        Some(_) => kprintln!("[npk] Usage: watchdog [on|off|test [core]]"),
    }
}

fn spin_masked(_: u64) {
    // SAFETY: a deliberate lockup for `watchdog test`; nothing resumes it.
    unsafe { core::arch::asm!("cli", options(nomem, nostack)) };
    loop {
        core::hint::spin_loop();
    }
}
