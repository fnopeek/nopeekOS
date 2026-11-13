//! The x86-64 half of SMP: cores from the ACPI MADT, started by
//! INIT-SIPI-SIPI, told apart and woken through their Local APIC; the
//! per-core CPU features (MONITOR/MWAIT, APERF/MPERF, HWP, RAPL).
//!
//! Design: Core 0 = Kernel/IRQ (fixed), Cores 1..N = Worker Pool.
//! No hardcoded core limit — scales from 2 to 1024+.

core::arch::global_asm!(include_str!("trampoline.s"), options(att_syntax));

use super::{per_core, scheduler};
use crate::arch::x86_64::msr;
use crate::hw::{Mmio, PhysView};
use crate::kprintln;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use spin::Mutex;

const TRAMPOLINE_BASE: usize = 0x8000;
// 2 MB per AP, matching the BSP boot stack (linker.ld). Native intents run
// on the AP stack, not a fiber, and `update` runs the deep OTA chain there
// (TLS record buffer, storage::put, B-tree splits with 4 KB node buffers).
// There is no guard page, so the size is the only safety margin.
const AP_STACK_SIZE: usize = 2 * 1024 * 1024;

// Data area offsets within trampoline (must match trampoline.s).
// The AVX bring-up (XSETBV) sits before the GDT block.
const OFF_GDT64: usize = 0xF0;
const OFF_CR3: usize = 0x100;
const OFF_STACK: usize = 0x108;
const OFF_ENTRY: usize = 0x110;
const OFF_CORE_ID: usize = 0x118;
const OFF_RUNNING: usize = 0x11C;
const OFF_IDTR: usize = 0x120;

/// Counter incremented by each AP when it reaches Rust entry
static AP_STARTED: AtomicUsize = AtomicUsize::new(0);

unsafe extern "C" {
    static smp_trampoline_start: u8;
    static smp_trampoline_end: u8;
}

/// Initialize SMP: discover cores via MADT, boot all APs.
pub fn init() {
    let lapic = crate::interrupts::lapic_from_msr();
    let apic_base = lapic.base();
    if apic_base == 0 {
        kprintln!("[npk] smp: no Local APIC");
        return;
    }

    // Ensure APIC MMIO page is accessible
    let _ = crate::paging::map_page(
        apic_base, apic_base,
        crate::paging::PageFlags::PRESENT
            | crate::paging::PageFlags::WRITABLE
            | crate::paging::PageFlags::NO_CACHE,
    );

    let bsp_id = read_apic_id(lapic);
    register_bsp(bsp_id);

    crate::cpu_errata::apply();
    crate::spec_mitigations::apply();

    // Enable HWP (hardware frequency scaling) on BSP
    if enable_hwp() {
        per_core::update_core_freq(0);
        kprintln!("[npk] HWP: {}-{} MHz (auto-scaling)",
            per_core::min_eff_mhz(), per_core::max_turbo_mhz());
    }

    // Discover APs from ACPI MADT
    let ap_ids = parse_madt(bsp_id);
    if ap_ids.is_empty() {
        kprintln!("[npk] smp: 1 core (BSP only)");
        return;
    }

    crate::kdebug!("[npk] smp: {} cores detected (BSP + {} APs)",
        ap_ids.len() + 1, ap_ids.len());

    // Prepare trampoline at 0x8000
    setup_trampoline();

    // Boot each AP sequentially
    let mut online = 0u32;
    for (i, &ap_apic_id) in ap_ids.iter().enumerate() {
        let core_id = (i + 1) as u32;
        if boot_ap(lapic, ap_apic_id, core_id) {
            register_ap(ap_apic_id, core_id);
            online += 1;
        } else {
            kprintln!("[npk] smp: core {} (APIC {}) failed", core_id, ap_apic_id);
            mark_failed(ap_apic_id);
        }
    }

    kprintln!("[npk] smp: {}/{} APs online", online, ap_ids.len());

    if online > 0 {
        // Initialize scheduler and wake APs into their work loops
        scheduler::init(online as usize);
        // Decide the dedicated microvm core before APs enter their
        // loops (carve-out observed on first iteration).
        init_dedicated_vm_core(online as usize);
        per_core::start_scheduler();

        crate::kdebug!("[npk] smp: scheduler ready (shared inbox, tickless workers: {} timer + wake IPI)",
            if crate::interrupts::has_tsc_deadline() { "TSC-deadline" } else { "one-shot" });
        log_tsc_sync(online as usize);
        enable_deep_idle();
    }
}

/// Deep idle by default where the CPU offers it (AMD Zen on bare metal,
/// with ARAT): CStateBaseAddr+2, the usual C3. The deepest port belongs to
/// ACPI `_CST` (Linux `acpi_idle`). Not implemented: reading `_CST` via acpi.
fn enable_deep_idle() {
    let Some(base) = amd_cstate_base() else { return };
    match crate::interrupts::set_deep_idle(base + 2, 200) {
        Ok(()) => kprintln!("[npk] idle: deep C-state via I/O 0x{:x} (CStateBaseAddr+2) for halts >= 200 us",
            base + 2),
        Err(e) => kprintln!("[npk] idle: staying in C1 — {}", e),
    }
}

/// Measure every AP's TSC against core 0 (wake round trip, ±rt/2) once the
/// APs are parked. Firmware may leave core 0 behind; since the kernel
/// compares TSC stamps across cores, a uniform offset is corrected once by
/// writing IA32_TSC on core 0. Linux instead marks the TSC unstable without
/// TSC_ADJUST (AMD) — not an option for a tickless kernel built on the TSC.
fn log_tsc_sync(online: usize) {
    let t0 = crate::time::counter();
    let settle = crate::time::counter_hz() / 20; // 50 ms for the APs to park
    while crate::time::counter().wrapping_sub(t0) < settle {
        core::hint::spin_loop();
    }
    let per_us = (crate::time::counter_hz() / 1_000_000).max(1) as i64;
    let (lo, hi, n, best) = measure_offsets(online);
    if n == 0 {
        kprintln!("[npk] tsc: sync not measured (no AP answered)");
        return;
    }
    crate::kdebug!("[npk] tsc: {} APs vs core 0: min {:+} us, max {:+} us",
        n, lo / per_us, hi / per_us);

    // Correct only a real, uniform offset: the APs must agree with each
    // other (else there is no single clock to join), and the offset must be
    // well above the measurement's own error.
    let (off, rt) = best;
    let agree = (hi - lo) <= 100 * per_us;
    if !agree || off.unsigned_abs() <= 100 * per_us as u64 || off < 0 {
        if !agree {
            kprintln!("[npk] tsc: APs disagree among themselves — left as is");
        }
        return;
    }
    // The offset was taken against the midpoint of the round trip, so it
    // carries ±rt/2; the smallest round trip is the best estimate.
    let _ = rt;
    crate::arch::without_interrupts(|| {
        let now = crate::time::counter().wrapping_add(off as u64);
        // SAFETY: IA32_TSC (MSR 0x10) is architectural on x86_64 and
        // writable at CPL 0; it moves only this core's (core 0's) counter.
        // Interrupts are off, so nothing on this core sees the jump halfway.
        unsafe { crate::arch::x86_64::msr::write(0x10, now) };
    });
    let (lo2, hi2, _, _) = measure_offsets(online);
    kprintln!("[npk] tsc: core 0 moved {:+} us to join the APs — now min {:+} us, max {:+} us",
        off / per_us, lo2 / per_us, hi2 / per_us);
}

/// (min, max, answered, (offset, round trip) with the smallest round trip),
/// all in TSC cycles, of every AP against core 0.
fn measure_offsets(online: usize) -> (i64, i64, usize, (i64, u64)) {
    let (mut lo, mut hi, mut n) = (i64::MAX, i64::MIN, 0usize);
    let mut best = (0i64, u64::MAX);
    for c in 1..=online {
        let mut got = None;
        for _ in 0..1000 {
            got = per_core::tsc_offset(c);
            if got.is_some() { break; }
            core::hint::spin_loop();
        }
        if let Some((off, rt)) = got {
            lo = lo.min(off);
            hi = hi.max(off);
            if rt < best.1 { best = (off, rt); }
            n += 1;
        }
    }
    (lo, hi, n, best)
}

/// Read this core's APIC ID from the Local APIC register
fn read_apic_id(lapic: Mmio) -> u32 {
    lapic.r32(crate::interrupts::LAPIC_ID) >> 24
}

/// The GDT an AP starts long mode with: null, code (0x08), data (0x10),
/// byte for byte as `boot.s`. In the image, so below 4 GiB.
static AP_BOOT_GDT: [u64; 3] = [
    0,
    0x00AF_9B00_0000_FFFF, // accessed bit preset: the CPU never writes here
    0x00CF_9300_0000_FFFF,
];

/// Copy trampoline to 0x8000 and fill in shared data (CR3, GDT, IDT, entry)
fn setup_trampoline() {
    // SAFETY: runs on the boot core before any AP is started, so nothing else
    // touches the trampoline page. 0x8000 is below 1 MiB, identity-mapped and
    // reserved for this; every write below stays inside the trampoline image
    // (offsets from `trampoline.s`), and the asm only reads CR3 and the IDTR.
    unsafe {
        let start = &smp_trampoline_start as *const u8;
        let end = &smp_trampoline_end as *const u8;
        let size = end as usize - start as usize;

        // SAFETY: 0x8000 is in first 1MB (reserved, identity-mapped, not used by kernel)
        core::ptr::copy_nonoverlapping(start, TRAMPOLINE_BASE as *mut u8, size);

        // CR3 — BSP's page table root. The trampoline loads it in 32-bit
        // protected mode (only a 32-bit mov), so the PML4 must lie below
        // 4 GiB; `paging::init` takes it from the frame allocator, which can
        // hand out any frame, hence the assert.
        let cr3: u64;
        core::arch::asm!("mov {}, cr3", out(reg) cr3);
        assert!(cr3 < 0x1_0000_0000, "PML4 above 4GB — AP trampoline cannot load it");
        *((TRAMPOLINE_BASE + OFF_CR3) as *mut u32) = cr3 as u32;

        // GDT64 pointer. The trampoline loads it with a 32-bit lgdt, so it
        // must lie below 4 GiB: the image does, the heap (where each core's
        // own GDT + TSS live) need not. It only has to carry the code and
        // data segments until `smp_ap_entry` installs the core's own.
        let gdt_base = core::ptr::addr_of!(AP_BOOT_GDT) as u64;
        let gdt_limit = (core::mem::size_of::<[u64; 3]>() - 1) as u16;
        assert!(gdt_base < 0x1_0000_0000, "GDT above 4GB — AP trampoline cannot load it");
        *((TRAMPOLINE_BASE + OFF_GDT64) as *mut u16) = gdt_limit;
        // The base follows the 2-byte limit (descriptor-table register
        // layout), so it is only 2-byte aligned.
        core::ptr::write_unaligned((TRAMPOLINE_BASE + OFF_GDT64 + 2) as *mut u32, gdt_base as u32);

        // IDTR — copy BSP's IDT register (SIDT stores 10 bytes)
        let mut idtr = [0u8; 10];
        core::arch::asm!("sidt [{}]", in(reg) idtr.as_mut_ptr());
        core::ptr::copy_nonoverlapping(
            idtr.as_ptr(),
            (TRAMPOLINE_BASE + OFF_IDTR) as *mut u8,
            10,
        );

        // Rust AP entry point
        *((TRAMPOLINE_BASE + OFF_ENTRY) as *mut u64) =
            smp_ap_entry as *const () as u64;
    }
}

/// Boot a single AP: allocate stack, write per-AP data, send INIT-SIPI-SIPI
fn boot_ap(lapic: Mmio, target_apic_id: u32, core_id: u32) -> bool {
    let stack_top = allocate_ap_stack();
    if stack_top == 0 {
        return false;
    }

    // Write per-AP fields to trampoline data area
    // SAFETY: trampoline at 0x8000 is set up, no AP is using it yet
    //         (we boot APs sequentially)
    unsafe {
        *((TRAMPOLINE_BASE + OFF_STACK) as *mut u64) = stack_top;
        *((TRAMPOLINE_BASE + OFF_CORE_ID) as *mut u32) = core_id;
        *((TRAMPOLINE_BASE + OFF_RUNNING) as *mut u32) = 0;

        // Ensure all writes are visible before SIPI
        core::sync::atomic::fence(Ordering::SeqCst);
    }

    let started_before = AP_STARTED.load(Ordering::Acquire);
    let vector = (TRAMPOLINE_BASE / 0x1000) as u32; // SIPI vector = page number

    // INIT IPI
    send_ipi(lapic, target_apic_id, 0x0000_4500);
    crate::time::delay_ms(10);

    // SIPI #1
    send_ipi(lapic, target_apic_id, 0x0000_4600 | vector);
    crate::time::delay_ms(1);

    // Check if AP started
    if AP_STARTED.load(Ordering::Acquire) > started_before {
        return true;
    }

    // SIPI #2 (spec says retry once)
    send_ipi(lapic, target_apic_id, 0x0000_4600 | vector);

    // Wait up to 100ms
    let timeout = crate::time::counter_hz() / 10;
    let t0 = crate::time::counter();
    while crate::time::counter() - t0 < timeout {
        if AP_STARTED.load(Ordering::Acquire) > started_before {
            return true;
        }
        core::hint::spin_loop();
    }

    false
}

/// Force the host core with sequential id `core_id` out of VMRUN by sending it
/// the vCPU-kick IPI from the calling core's LAPIC. Used by guest-SMP IPI
/// delivery so a target vCPU takes a cross-vCPU interrupt within microseconds
/// (its VMRUN #VMEXIT(INTR)s on receipt) instead of at its next natural exit
/// (~10 ms host-timer tick). No-op if the core id is unknown. The kick vector's
/// host ISR is a pure EOI — the receipt itself is the wakeup.
pub fn kick_host_core(core_id: usize) {
    kick_host_core_with(core_id, crate::interrupts::VCPU_KICK_VECTOR);
}

/// As `kick_host_core` for a vCPU with posted interrupts: in guest mode the
/// notification vector delivers the posted vectors without an exit
/// (`kvm_vcpu_trigger_posted_interrupt`).
pub fn kick_host_core_posted(core_id: usize) {
    kick_host_core_with(core_id, crate::interrupts::POSTED_INTR_VECTOR);
}

fn kick_host_core_with(core_id: usize, vector: u8) {
    // Event-wake a consumer fiber parked in `kick_wait` on the target core
    // before the IPI, so the wake is never lost. Harmless for non-net kicks
    // (only `kick_wait` fibers observe the generation).
    crate::smp::fiber::net_kick_bump(core_id);
    // `kvm_vcpu_kick`: the IPI only when the vCPU may be in the guest. A
    // parked one was woken above, and one between exit and entry sees the
    // new generation in its last look (`entry_irqs_off`). The bump is a
    // locked RMW, so it is ordered before this load.
    if !crate::microvm::cpu::in_guest(core_id) {
        return;
    }
    // Lock-free and without the APIC-base RDMSR, which is itself a VM exit
    // when nopeekOS runs virtualized: a kick is on the IPI hot path.
    let Some(apic_id) = core_apic(core_id) else { return };
    // FIXED delivery (mode 000), level assert (bit 14), physical dest.
    send_ipi(crate::interrupts::lapic(), apic_id,
        0x0000_4000 | vector as u32);
}

/// Send the worker wake IPI to the core with xAPIC id `apic_id`.
pub fn send_wake_ipi(apic_id: u32) {
    // FIXED delivery, level assert, physical destination.
    send_ipi(crate::interrupts::lapic(), apic_id,
        0x0000_4000 | crate::interrupts::WORKER_WAKE_VECTOR as u32);
}

/// Ask the core with xAPIC id `apic_id` for its APERF/MPERF
/// (`per_core::aperf_mperf_now`).
pub fn send_freq_sample_ipi(apic_id: u32) {
    send_ipi(crate::interrupts::lapic(), apic_id,
        0x0000_4000 | crate::interrupts::FREQ_SAMPLE_VECTOR as u32);
}

/// Send IPI via Local APIC ICR (wait for idle first).
///
/// IF is masked across the two ICR writes: an ISR on this core that sends
/// its own IPI in between would overwrite ICR-high, and the second write
/// here would then deliver to the ISR's target.
fn send_ipi(lapic: Mmio, target_apic_id: u32, icr_low: u32) {
    crate::arch::without_interrupts(|| send_ipi_masked(lapic, target_apic_id, icr_low))
}

fn send_ipi_masked(lapic: Mmio, target_apic_id: u32, icr_low: u32) {
    use crate::interrupts::{LAPIC_ICR_HI, LAPIC_ICR_LO};
    // Wait for delivery status = idle
    while lapic.r32(LAPIC_ICR_LO) & (1 << 12) != 0 {
        core::hint::spin_loop();
    }
    // Destination APIC ID (bits 24-31 of ICR high)
    lapic.w32(LAPIC_ICR_HI, target_apic_id << 24);
    // Command (writing ICR low triggers the IPI)
    lapic.w32(LAPIC_ICR_LO, icr_low);
}

/// Allocate the per-AP stack (`AP_STACK_SIZE`) with an unmapped guard page
/// below it (`mm::stack`), so running off its end faults instead of
/// overwriting whatever lies beneath. Returns the stack top (grows down).
/// An AP's stack lives as long as the machine, so it is never handed back.
fn allocate_ap_stack() -> u64 {
    match crate::mm::stack::KernelStack::new(AP_STACK_SIZE) {
        Some(stack) => {
            let top = stack.top();
            core::mem::forget(stack);
            top
        }
        None => 0,
    }
}

// ── ACPI MADT Parsing ──────────────────────────────────────────

/// Parse MADT (signature "APIC") and return all AP APIC IDs
fn parse_madt(bsp_apic_id: u32) -> Vec<u32> {
    let mut ap_ids = Vec::new();

    let madt_addr = match crate::acpi::find_table(b"APIC") {
        Some(addr) => addr,
        None => {
            kprintln!("[npk] smp: MADT not found");
            return ap_ids;
        }
    };

    // SAFETY: `find_table` returns a mapped ACPI table; its header holds
    // the length.
    let header = unsafe { PhysView::new(madt_addr as u64, 8) };
    let madt_len = header.u32(4u32).unwrap_or(0) as usize;
    if madt_len < 44 || madt_len > 0x10000 {
        return ap_ids;
    }
    crate::acpi::ensure_mapped_pub(madt_addr, madt_len);
    // SAFETY: the whole table is mapped just above; firmware tables do not
    // change.
    let madt = unsafe { PhysView::new(madt_addr as u64, madt_len as u64) };

    // Walk Interrupt Controller Structures (start at offset 44)
    let mut offset = 44;
    while offset + 2 <= madt_len {
        let entry_type = madt.u8(offset).unwrap_or(0);
        let entry_len = madt.u8(offset + 1).unwrap_or(0) as usize;
        if entry_len < 2 || offset + entry_len > madt_len { break; }

        match entry_type {
            // Type 0: Processor Local APIC (8-bit APIC ID)
            0 if entry_len >= 8 => {
                let apic_id = madt.u8(offset + 3).unwrap_or(0) as u32;
                let flags = madt.u32(offset + 4).unwrap_or(0);
                // bit 0 = Enabled, bit 1 = Online Capable
                if (flags & 0x03) != 0 && apic_id != bsp_apic_id {
                    ap_ids.push(apic_id);
                }
            }
            // Type 9: Processor Local x2APIC (32-bit APIC ID, for >255 cores)
            9 if entry_len >= 16 => {
                let x2apic_id = madt.u32(offset + 4).unwrap_or(0);
                let flags = madt.u32(offset + 8).unwrap_or(0);
                if (flags & 0x03) != 0 && x2apic_id != bsp_apic_id {
                    ap_ids.push(x2apic_id);
                }
            }
            _ => {}
        }

        offset += entry_len;
    }

    ap_ids
}

// ── Per-core CPU state ─────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreState {
    Bsp,
    Online,
    Failed,
}

pub struct CoreInfo {
    /// Sequential index (0 = BSP)
    #[allow(dead_code)]
    pub id: u32,
    /// Hardware APIC ID (may not be sequential)
    pub apic_id: u32,
    pub state: CoreState,
}

pub static CORES: Mutex<Vec<CoreInfo>> = Mutex::new(Vec::new());

/// True if CPU supports MONITOR/MWAIT (detected at boot)
static HAS_MWAIT: AtomicBool = AtomicBool::new(false);

/// True if CPU supports IA32_APERF/IA32_MPERF MSRs (CPUID.06H:ECX[0]).
/// Intel since Nehalem, AMD since family 10h; absent under `-cpu qemu64`.
/// Guarded at every rdmsr.
static HAS_APERFMPERF: AtomicBool = AtomicBool::new(false);


/// APERF/MPERF as of the core's last halt entry (see `halt_begin`) or its
/// last `FREQ_SAMPLE_VECTOR` IPI.
static RUN_APERF: [AtomicU64; 256] = [const { AtomicU64::new(0) }; 256];
static RUN_MPERF: [AtomicU64; 256] = [const { AtomicU64::new(0) }; 256];
/// Samples taken per core, so a reader that asked for one sees it arrive.
static RUN_SAMPLES: [AtomicU64; 256] = [const { AtomicU64::new(0) }; 256];

/// Record the calling core's APERF/MPERF for readers on other cores.
pub fn sample_aperf_mperf(core_id: usize) {
    if core_id >= 256 || !HAS_APERFMPERF.load(Ordering::Relaxed) { return; }
    let (a, m) = read_aperf_mperf();
    RUN_APERF[core_id].store(a, Ordering::Relaxed);
    RUN_MPERF[core_id].store(m, Ordering::Relaxed);
    RUN_SAMPLES[core_id].fetch_add(1, Ordering::Release);
}

/// APERF/MPERF of every core now, (0, 0) without the MSRs. The caller reads
/// its own; every other core is asked with `FREQ_SAMPLE_VECTOR`, since a
/// core that runs without halting takes no other interrupt. A core that has
/// not answered within a millisecond keeps its last sample.
pub fn aperf_mperf_now(out: &mut [(u64, u64)]) {
    if !HAS_APERFMPERF.load(Ordering::Relaxed) {
        out.fill((0, 0));
        return;
    }
    let me = current_core_id();
    let n = out.len().min(per_core::core_count()).min(256);
    let mut before = [0u64; 256];
    for c in 0..n {
        if c != me {
            before[c] = RUN_SAMPLES[c].load(Ordering::Acquire);
            send_freq_sample_ipi(CORE_APIC[c].load(Ordering::Relaxed));
        }
    }
    let deadline = crate::time::counter() + crate::time::counter_hz() / 1000;
    for c in 0..n {
        if c == me {
            out[c] = read_aperf_mperf();
            continue;
        }
        while RUN_SAMPLES[c].load(Ordering::Acquire) == before[c]
            && crate::time::counter() < deadline
        {
            core::hint::spin_loop();
        }
        out[c] = halted_aperf_mperf(c);
    }
}

/// APERF/MPERF of `core_id` as of its last halt entry; (0, 0) without the
/// MSRs. A reader on another core gets the running cycles up to that halt.
pub fn halted_aperf_mperf(core_id: usize) -> (u64, u64) {
    if core_id >= 256 { return (0, 0); }
    (RUN_APERF[core_id].load(Ordering::Relaxed), RUN_MPERF[core_id].load(Ordering::Relaxed))
}

fn read_aperf_mperf() -> (u64, u64) {
    if !HAS_APERFMPERF.load(Ordering::Relaxed) { return (0, 0); }
    // SAFETY: CPUID.06H:ECX[0] says MSRs 0xE7/0xE8 exist (checked above);
    // reading them has no side effect.
    unsafe { (msr::read(0xE8), msr::read(0xE7)) }
}


/// Platform frequency limits (set once by enable_hwp)
static MAX_TURBO_MHZ: AtomicU32 = AtomicU32::new(0);
static MIN_EFF_MHZ: AtomicU32 = AtomicU32::new(0);

pub fn register_bsp(apic_id: u32) {
    CORES.lock().push(CoreInfo { id: 0, apic_id, state: CoreState::Bsp });
    map_apic(apic_id, 0);
    // Detect MONITOR/MWAIT support via CPUID.01H:ECX bit 3
    let ecx: u32;
    // SAFETY: CPUID leaf 1 exists everywhere; rbx is reserved by LLVM and
    // saved by hand.
    unsafe {
        core::arch::asm!(
            "push rbx",
            "mov eax, 1",
            "cpuid",
            "mov {0:e}, ecx",
            "pop rbx",
            out(reg) ecx,
            out("eax") _,
            out("edx") _,
        );
    }
    HAS_MWAIT.store(ecx & (1 << 3) != 0, Ordering::Release);

    // Detect IA32_APERF/IA32_MPERF via CPUID.06H:ECX[0]
    // Absent on AMD and on KVM guest w/ generic `-cpu qemu64`.
    let ecx: u32;
    // SAFETY: CPUID leaf 6 exists on every CPU we boot on; rbx is saved by
    // hand.
    unsafe {
        core::arch::asm!(
            "push rbx",
            "mov eax, 6",
            "cpuid",
            "mov {0:e}, ecx",
            "pop rbx",
            out(reg) ecx,
            out("eax") _,
            out("edx") _,
        );
    }
    HAS_APERFMPERF.store(ecx & 1 != 0, Ordering::Release);
    rapl_probe();
}

// ── RAPL: AMD package and core energy counters ───────────────────────
//
// Separates the CPU package's power draw from the rest of the platform.
// Ported from Linux 6.18. Feature bit from `arch/x86/kernel/cpu/amd.c`:
//
//     /* Bit 14 indicates the Runtime Average Power Limit interface. */
//     if (c->x86_power & BIT(14)) set_cpu_cap(c, X86_FEATURE_RAPL);
//
// where `x86_power` = CPUID Fn8000_0007_EDX. MSR numbers from
// `arch/x86/include/asm/msr-index.h`, the unit field from
// `arch/x86/events/rapl.c` (`(msr_rapl_power_unit_bits >> 8) & 0x1F`).
const MSR_AMD_RAPL_POWER_UNIT: u32 = 0xC001_0299;
const MSR_AMD_CORE_ENERGY_STATUS: u32 = 0xC001_029A;
const MSR_AMD_PKG_ENERGY_STATUS: u32 = 0xC001_029B;
// Intel: the MSRs of `arch/x86/events/rapl.c`, on the models its table
// lists (`model_skl` for Tiger Lake); the core counter is PP0, all cores
// together.
const MSR_RAPL_POWER_UNIT: u32 = 0x606;
const MSR_PKG_ENERGY_STATUS: u32 = 0x611;
const MSR_PP0_ENERGY_STATUS: u32 = 0x639;
/// RAPL through the Intel MSRs (else the AMD ones).
static RAPL_INTEL: AtomicBool = AtomicBool::new(false);

/// The model of an Intel family-6 CPU on bare metal (CPUID leaf 1), else
/// None.
pub fn intel_model() -> Option<u32> {
    let (b, c, d): (u32, u32, u32);
    let (a1, c1): (u32, u32);
    // SAFETY: CPUID leaves 0 and 1 exist on every x86-64; rbx is reserved
    // by LLVM, so it is moved out by hand.
    unsafe {
        core::arch::asm!("push rbx", "xor eax, eax", "xor ecx, ecx", "cpuid", "mov {0:e}, ebx", "pop rbx",
            out(reg) b, out("ecx") c, out("edx") d, out("eax") _);
        core::arch::asm!("push rbx", "mov eax, 1", "xor ecx, ecx", "cpuid", "pop rbx",
            out("eax") a1, out("ecx") c1, out("edx") _);
    }
    // "GenuineIntel"
    if (b, d, c) != (0x756E_6547, 0x4965_6E69, 0x6C65_746E) { return None; }
    // Under a hypervisor these MSRs are its business.
    if c1 & (1 << 31) != 0 { return None; }
    if (a1 >> 8) & 0xF != 6 { return None; }
    Some(((a1 >> 4) & 0xF) | (((a1 >> 16) & 0xF) << 4))
}

/// `INTEL_TIGERLAKE_L` / `INTEL_TIGERLAKE` (intel-family.h).
fn intel_tigerlake(model: u32) -> bool {
    matches!(model, 0x8C | 0x8D)
}

/// The C-state residency counters turbostat reads on Tiger Lake: package
/// PC2, PC3, PC6, PC7, PC8, PC9, PC10, then this core's C6 and C7. They
/// count at the TSC rate.
pub const RESIDENCY_NAMES: [&str; 9] = ["PC2", "PC3", "PC6", "PC7", "PC8", "PC9", "PC10", "CC6", "CC7"];
const RESIDENCY_MSRS: [u32; 9] = [0x60D, 0x3F8, 0x3F9, 0x3FA, 0x630, 0x631, 0x632, 0x3FD, 0x3FE];

/// `MSR_PKG_CST_CONFIG_CONTROL` on a CPU with the counters: the deepest
/// package C-state the firmware allows (turbostat's `hsw_pkg_cstate_limits`
/// for Tiger Lake) and whether the register is locked.
pub fn pkg_cstate_limit() -> Option<(&'static str, bool)> {
    if !intel_model().is_some_and(intel_tigerlake) { return None; }
    // SAFETY: MSR_PKG_CST_CONFIG_CONTROL exists on every Intel core since
    // Nehalem, Tiger Lake included; reading it has no side effect.
    let v = unsafe { msr::read(0xE2) };
    const HSW: [&str; 16] = ["PC0", "PC2", "PC3", "PC6", "PC7", "PC7S", "PC8", "PC9", "unlimited",
        "reserved", "reserved", "reserved", "reserved", "reserved", "reserved", "reserved"];
    Some((HSW[(v & 0xF) as usize], v & (1 << 15) != 0))
}

/// The residency counters, on a CPU that has them.
pub fn residency() -> Option<[u64; 9]> {
    if !intel_model().is_some_and(intel_tigerlake) { return None; }
    let mut v = [0u64; 9];
    for (o, &m) in v.iter_mut().zip(RESIDENCY_MSRS.iter()) {
        // SAFETY: Tiger Lake has every one of these counters (turbostat's
        // model table); reading them has no side effect.
        *o = unsafe { msr::read(m) };
    }
    Some(v)
}

static HAS_RAPL: AtomicBool = AtomicBool::new(false);
/// Nanojoules per counter step. The unit is 2^-ESU joules; in nJ it stays
/// an integer (ESU 16 -> 15258 nJ), so no floating point is needed.
static RAPL_NJ_PER_UNIT: AtomicU32 = AtomicU32::new(0);

#[derive(Clone, Copy)]
enum RaplMsr {
    PowerUnit,
    PkgEnergy,
    CoreEnergy,
}

/// Low dword of a RAPL MSR, 0 without RAPL. Core-local: measures the core
/// it runs on.
fn rapl_read(r: RaplMsr) -> u32 {
    if !has_rapl() { return 0; }
    let intel = RAPL_INTEL.load(Ordering::Relaxed);
    let m = match (r, intel) {
        (RaplMsr::PowerUnit, false) => MSR_AMD_RAPL_POWER_UNIT,
        (RaplMsr::PkgEnergy, false) => MSR_AMD_PKG_ENERGY_STATUS,
        (RaplMsr::CoreEnergy, false) => MSR_AMD_CORE_ENERGY_STATUS,
        (RaplMsr::PowerUnit, true) => MSR_RAPL_POWER_UNIT,
        (RaplMsr::PkgEnergy, true) => MSR_PKG_ENERGY_STATUS,
        (RaplMsr::CoreEnergy, true) => MSR_PP0_ENERGY_STATUS,
    };
    // SAFETY: `has_rapl()` (AMD: CPUID 0x80000007:EDX[14]; Intel: a model
    // of rapl.c's table) says these MSRs exist on every core of this
    // socket; reading them has no side effect.
    unsafe { msr::read(m) as u32 }
}

fn rapl_probe() {
    let edx: u32;
    // SAFETY: CPUID 0x80000007 is valid on every x86-64; rbx is reserved by
    // LLVM, so it is saved by hand.
    unsafe {
        core::arch::asm!(
            "push rbx",
            "mov eax, 0x80000007",
            "cpuid",
            "pop rbx",
            out("edx") edx,
            out("eax") _,
            out("ecx") _,
        );
    }
    let intel = intel_model().is_some_and(intel_tigerlake);
    if edx & (1 << 14) == 0 && !intel { return; }
    RAPL_INTEL.store(intel, Ordering::Release);
    HAS_RAPL.store(true, Ordering::Release);
    // Only read after the feature check; before it this would #GP.
    let esu = (rapl_read(RaplMsr::PowerUnit) >> 8) & 0x1F;
    // 2^-ESU joules in nanojoules: 1e9 >> ESU. An absurd ESU (0 or > 30)
    // would give nonsense, so report no RAPL instead.
    if esu == 0 || esu > 30 {
        HAS_RAPL.store(false, Ordering::Release);
        return;
    }
    RAPL_NJ_PER_UNIT.store((1_000_000_000u64 >> esu) as u32, Ordering::Release);
}

pub fn has_rapl() -> bool { HAS_RAPL.load(Ordering::Acquire) }

/// Nanojoules per counter step (0 = no RAPL).
pub fn rapl_nj_per_unit() -> u32 { RAPL_NJ_PER_UNIT.load(Ordering::Relaxed) }

/// Raw package energy counter. 32 bits and wraps: always take the
/// difference of two samples (`wrapping_sub`).
pub fn rapl_pkg_raw() -> u32 {
    if !has_rapl() { return 0; }
    rapl_read(RaplMsr::PkgEnergy)
}

/// The same for the core this call runs on (Intel: all cores, PP0).
pub fn rapl_core_raw() -> u32 {
    if !has_rapl() { return 0; }
    rapl_read(RaplMsr::CoreEnergy)
}

/// Milliwatts from a counter delta and a window: mW = nJ / us, so no
/// conversion constant is needed.
pub fn rapl_mw(delta_units: u32, window_us: u64) -> u64 {
    let nj = rapl_nj_per_unit() as u64;
    if nj == 0 || window_us == 0 { return 0; }
    (delta_units as u64).saturating_mul(nj) / window_us
}


pub fn register_ap(apic_id: u32, core_id: u32) {
    let mut cores = CORES.lock();
    cores.push(CoreInfo { id: core_id, apic_id, state: CoreState::Online });
    map_apic(apic_id, core_id);
    per_core::set_core_count(cores.len());
}

pub fn mark_failed(apic_id: u32) {
    if let Some(c) = CORES.lock().iter_mut().find(|c| c.apic_id == apic_id) {
        c.state = CoreState::Failed;
    }
}


/// Master switch for the dedicated-core path. `false` → the microvm
/// always stays cooperatively time-sliced on Core 0, regardless of core
/// count. `true` → the vendor and core-count gates below decide.
/// Only consulted when the vCPU does not run as a pool fiber.
pub const A2_DEDICATED_CORE_ENABLED: bool = true;

/// Minimum logical cores (incl. BSP) before dedicating one to the
/// microvm — below this, dedicating would starve the rest, so keep
/// cooperative Core-0 slicing.
const MIN_CORES_FOR_DEDICATED: usize = 3;

/// Decide the dedicated VM core from the live core count. Called once
/// right after `scheduler::init` (so WORKER_COUNT is known). Picks the
/// highest worker id (= `worker_count`, since worker ids are
/// `1..=worker_count`) so the low cores keep taking work; Core 0 stays
/// the IRQ/compositor/intent core regardless.
pub fn init_dedicated_vm_core(worker_count: usize) {
    let total = worker_count + 1; // + BSP

    // The dedicated-core path is validated on AMD-SVM only; the VMX
    // equivalents (VMCS host-state lifecycle, IDT vectoring info, MSR
    // auto-save/load) are unaudited, so Intel falls back to cooperative
    // Core-0 slicing. `detect_vendor` is stateless and safe to call before
    // `microvm::cpu::init`.
    let vendor = crate::microvm::cpu::detect_vendor();
    let is_amd = matches!(vendor, crate::microvm::cpu::Vendor::Amd);
    let is_intel = matches!(vendor, crate::microvm::cpu::Vendor::Intel);

    // vCPU-as-fiber: the guest runs as a normal pool fiber on a dynamic
    // core, so no core is carved out (a carve-out wastes a core while no VM
    // runs). See microvm::cpu. On VMX this
    // is flag-gated; the per-core TSS VMX host-state needs is installed
    // lazily in vmx::vm_open.
    let fiber_mode = crate::microvm::cpu::VCPU_AS_FIBER
        && worker_count >= 1
        && (is_amd || (is_intel && crate::microvm::cpu::VMX_VCPU_AS_FIBER));
    crate::microvm::cpu::set_vm_fiber_mode(fiber_mode);
    if fiber_mode {
        crate::kdebug!(
            "[npk] smp: microvm runs as a pool fiber on a dynamic core ({} cores, no carve-out)",
            total
        );
        return;
    }

    let dedicate = A2_DEDICATED_CORE_ENABLED
        && is_amd
        && worker_count >= 1
        && total >= MIN_CORES_FOR_DEDICATED;

    if dedicate {
        per_core::DEDICATED_VM_CORE.store(worker_count as u32, Ordering::Release);
        crate::kprintln!(
            "[npk] smp: core {} dedicated to microvm ({} cores, carved out of work-stealing)",
            worker_count, total
        );
    } else if !is_amd && total >= MIN_CORES_FOR_DEDICATED {
        crate::kprintln!(
            "[npk] smp: {} core(s) — A2 disabled (vendor-gated, unvalidated on this CPU) → microvm cooperative on Core 0",
            total
        );
    } else {
        crate::kprintln!(
            "[npk] smp: {} core(s) → microvm stays cooperative on Core 0",
            total
        );
    }
}


/// Check if MONITOR/MWAIT is available
pub fn has_mwait() -> bool {
    HAS_MWAIT.load(Ordering::Relaxed)
}


/// Max turbo frequency in MHz (from HWP capabilities).
pub fn max_turbo_mhz() -> u32 { MAX_TURBO_MHZ.load(Ordering::Relaxed) }

/// Min efficiency frequency in MHz (from HWP capabilities).
pub fn min_eff_mhz() -> u32 { MIN_EFF_MHZ.load(Ordering::Relaxed) }


/// Cached APIC base (MSR 0x1B). Identical on every core and never changes, so
/// reading it once avoids an `rdmsr` — a VM-exit under virtualization — on every
/// current_core_id() call (hot: poll loops, idle, core-gating).
static APIC_BASE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Read current core's sequential ID via LAPIC.
pub fn current_core_id() -> usize {
    use core::sync::atomic::Ordering::Relaxed;
    let base = APIC_BASE.load(Relaxed);
    let lapic = if base == 0 {
        let l = crate::interrupts::lapic_from_msr();
        APIC_BASE.store(l.base(), Relaxed);
        l
    } else {
        // SAFETY: `APIC_BASE` holds only bases from
        // `interrupts::lapic_from_msr`, the xAPIC register page.
        unsafe { Mmio::from_mapped(base, 0x1000) }
    };
    // The per-core APIC ID register.
    let apic_id = lapic.r32(crate::interrupts::LAPIC_ID) >> 24;
    // An unregistered id reads as core 0.
    APIC_TO_CORE[(apic_id & 0xFF) as usize].load(Relaxed) as usize
}

/// The current core's id once the local APIC is set up; before that
/// every caller is the boot core and per-core state is not yet in use.
pub fn ready_core_id() -> Option<usize> {
    if crate::interrupts::apic_base() == 0 { return None; }
    Some(current_core_id())
}

/// xAPIC id → sequential core id, filled once per core at registration.
///
/// Lets `current_core_id` avoid a lock on the hottest paths (`poll_rx_only`,
/// `pump_peers`, every fiber yield). The mapping never changes after boot.
/// The xAPIC id register holds 8 bits, hence 256 entries.
static APIC_TO_CORE: [core::sync::atomic::AtomicU16; 256] =
    [const { core::sync::atomic::AtomicU16::new(0) }; 256];

/// Sequential core id → xAPIC id, the reverse of `APIC_TO_CORE`.
static CORE_APIC: [AtomicU32; 256] = [const { AtomicU32::new(0) }; 256];

/// The xAPIC id of core `c`, if it is a registered core. Lock-free, for IPI
/// senders on hot paths.
pub fn core_apic(c: usize) -> Option<u32> {
    (c < per_core::core_count().min(256)).then(|| CORE_APIC[c].load(Ordering::Relaxed))
}

fn map_apic(apic_id: u32, core_id: u32) {
    APIC_TO_CORE[(apic_id & 0xFF) as usize].store(core_id as u16, Ordering::Release);
    if (core_id as usize) < 256 {
        CORE_APIC[core_id as usize].store(apic_id, Ordering::Release);
    }
}


/// Enable Hardware P-states (HWP / Speed Shift) on the current core.
/// CPU automatically scales frequency: idle → min, load → turbo.
/// Returns true if HWP was enabled successfully.
pub fn enable_hwp() -> bool {
    // Check HWP support: CPUID.06H:EAX bit 7
    let eax: u32;
    // SAFETY: CPUID leaf 6 exists on every CPU we boot on; rbx is saved by
    // hand.
    unsafe {
        core::arch::asm!(
            "push rbx",
            "mov eax, 6",
            "cpuid",
            "mov {0:e}, eax",
            "pop rbx",
            out(reg) eax,
            out("ecx") _,
            out("edx") _,
        );
    }
    if eax & (1 << 7) == 0 { return false; }

    // Enable HWP: MSR 0x770 (IA32_PM_ENABLE) = 1
    // SAFETY: HWP supported (checked above); enabling it only hands
    // P-state selection to the CPU.
    unsafe { msr::write(0x770, 1) };

    // Read HWP capabilities: MSR 0x771 (IA32_HWP_CAPABILITIES)
    // [7:0]=Highest, [15:8]=Guaranteed, [23:16]=Efficient, [31:24]=Lowest
    // SAFETY: MSR 0x771 exists when HWP is supported; read-only.
    let cap_lo = unsafe { msr::read(0x771) } as u32;
    let highest = cap_lo & 0xFF;
    let lowest = (cap_lo >> 24) & 0xFF;

    // Store platform limits (first caller wins)
    if MAX_TURBO_MHZ.load(Ordering::Relaxed) == 0 {
        MAX_TURBO_MHZ.store(highest * 100, Ordering::Relaxed);
        MIN_EFF_MHZ.store(lowest * 100, Ordering::Relaxed);
    }

    // Configure HWP request: MSR 0x774 (IA32_HWP_REQUEST)
    // [7:0]=Min, [15:8]=Max, [23:16]=Desired(0=auto), [31:24]=EPP
    // EPP range: 0=perf, 128=balanced, 192=power_save, 255=max_power_save
    //
    // EPP=0 (max perf): the CPU ramps to turbo on the first instruction of a
    // burst. Bursty workloads (crypto, I/O batches) last ~10-100 ms, too
    // short for the firmware heuristics at EPP=192 to ramp up. Battery-
    // powered targets may prefer 128 or 192; not configurable yet.
    let hwp_req = (lowest as u32)
        | ((highest as u32) << 8)
        | (0u32 << 24);
    // SAFETY: MSR 0x774 exists when HWP is enabled; min/max come from the
    // CPU's own capabilities.
    unsafe { msr::write(0x774, hwp_req as u64) };

    true
}

/// AMD Zen C-state base address (MSRC001_0073 CStateBaseAddr, bits 15:0):
/// an I/O read of CStateBaseAddr+n is trapped by the core as a request for
/// C-state n (ACPI `_CST` lists these ports as SystemIO entries). None off
/// AMD, before family 17h, under a hypervisor (the MSR is not emulated and
/// a read would #GP), or when the firmware left it 0 (no trapping).
pub fn amd_cstate_base() -> Option<u16> {
    if !matches!(crate::microvm::cpu::detect_vendor(), crate::microvm::cpu::Vendor::Amd) {
        return None;
    }
    let (eax, ecx): (u32, u32);
    // SAFETY: CPUID leaf 1 exists everywhere; rbx is reserved by LLVM.
    unsafe {
        core::arch::asm!("push rbx", "mov eax, 1", "cpuid", "pop rbx",
            out("eax") eax, out("ecx") ecx, out("edx") _);
    }
    if ecx & (1 << 31) != 0 { return None; } // hypervisor
    let base_fam = (eax >> 8) & 0xF;
    let family = if base_fam == 0xF { base_fam + ((eax >> 20) & 0xFF) } else { base_fam };
    if family < 0x17 { return None; }
    // SAFETY: MSRC001_0073 is architectural on AMD family 17h+ (PPR
    // Core::X86::Msr::CStateBaseAddr), gated on vendor, family and bare
    // metal; reading it has no side effect.
    let lo = unsafe { msr::read(0xC001_0073) } as u32;
    let base = (lo & 0xFFFF) as u16;
    if base == 0 || base > 0xFFF8 { None } else { Some(base) }
}


/// Whether the core energy counter covers all cores (Intel PP0) rather
/// than the calling one (AMD).
pub fn rapl_core_is_all() -> bool { RAPL_INTEL.load(Ordering::Relaxed) }

/// APERF/MPERF of `core_id` for `per_core::update_core_freq`: read here
/// for the calling core, the last sample for any other; (0, 0) without
/// the MSRs, which are absent on KVM guests with `-cpu qemu64` and would
/// raise #GP.
pub fn aperf_mperf_of(core_id: usize) -> (u64, u64) {
    if !HAS_APERFMPERF.load(Ordering::Relaxed) {
        (0, 0)
    } else if core_id == current_core_id() {
        read_aperf_mperf()
    } else {
        (RUN_APERF[core_id].load(Ordering::Relaxed), RUN_MPERF[core_id].load(Ordering::Relaxed))
    }
}

/// AP Rust entry — called by trampoline after long mode transition.
/// Interrupts are disabled (cli from trampoline). IDT is loaded.
#[unsafe(no_mangle)]
pub extern "C" fn smp_ap_entry(core_id: u32) -> ! {
    // Own GDT + TSS before anything can fault: the shared IDT sends a
    // double fault to IST1, which only exists once this core has a TSS.
    crate::tss::init_core();

    // Enable Local APIC (needed for IPI wakeup fallback)
    // Each core sees its own LAPIC.
    let lapic = crate::interrupts::lapic_from_msr();
    let svr = lapic.r32(crate::interrupts::LAPIC_SVR);
    lapic.w32(crate::interrupts::LAPIC_SVR, svr | (1 << 8) | crate::interrupts::SPURIOUS_VECTOR as u32);

    crate::cpu_errata::apply();
    crate::spec_mitigations::apply();

    // Enable HWP on this AP (per-core frequency scaling)
    enable_hwp();

    // Signal BSP: this AP is alive
    AP_STARTED.fetch_add(1, Ordering::Release);

    per_core::worker_loop(core_id)
}

// ── What the scheduler asks of the machine ─────────────────────

/// Send the worker wake IPI to core `c`.
pub fn send_wake(c: usize) {
    if c < 256 {
        send_wake_ipi(CORE_APIC[c].load(Ordering::Relaxed));
    }
}

/// The worker's one-shot LAPIC timer (`interrupts::init_worker_timer`).
/// Plain HLT VM-exits, so KVM frees the host core (MWAIT would need
/// cpu-pm=on).
pub fn init_worker_timer() {
    crate::interrupts::init_worker_timer();
}

pub fn make_core0_tickless() {
    crate::interrupts::make_core0_tickless();
}

pub fn halt_until(deadline: Option<u64>, cause: usize) {
    crate::interrupts::halt_until(deadline, cause);
}

/// The core the WASM NIC driver runs on.
pub fn nic_core() -> Option<usize> {
    crate::netdev::wasm_nic_core()
}

pub fn is_vcpu_core(c: usize) -> bool {
    crate::microvm::cpu::is_vcpu_core(c)
}

pub fn in_guest(c: usize) -> bool {
    crate::microvm::cpu::in_guest(c)
}

pub fn vm_core_serve() {
    crate::microvm::vm_core_serve();
}
