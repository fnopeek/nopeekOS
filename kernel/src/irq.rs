//! Device-interrupt subsystem: MSI-X routing → LAPIC vector → fiber wake.
//!
//! The host kernel has no IOAPIC and the legacy 8259 PIC is fully masked
//! (re-init traps via SMI on HP/Insyde firmware — see `interrupts::init`).
//! So real-hardware device interrupts are routed via **MSI-X**: we program
//! a device's MSI-X table entry to deliver to a chosen LAPIC vector with a
//! chosen destination APIC ID. MSI-X writes go straight to the LAPIC
//! (message address `0xFEE0_0000 | apic<<12`), bypassing the PIC/IOAPIC
//! entirely, so the HP firmware quirk never bites.
//!
//! The ISR (in `interrupts.rs`) does the minimum: bump a per-vector atomic
//! fired-count + LAPIC EOI. A driver fiber parks via `wait()` until the
//! count advances (or a timeout). Crucially we target the device's MSI-X at
//! the APIC of the core running the driver fiber, so the interrupt itself
//! wakes that core out of HLT → the worker loop re-runs the scheduler →
//! the parked fiber resumes. No polling, no IPI: the IRQ is the wake.
//!
//! This closes the fiber scheduler's open "event-wake" hole and is the
//! foundation every poll-based HW driver (NVMe/NIC/audio_hda/xHCI) migrates
//! onto. First beneficiary: NVMe completion (HW-validated on the Intel H10).
//!
//! ## Driver contract (host + WASM)
//!
//! Run the driver as a **resident fiber** (pinned to its core). Once, after
//! binding the device:
//! ```text
//!   let vec = irq::register(dev, entry);   // host  — or npk_irq_register(entry) in WASM
//! ```
//! Then loop, servicing on the SAME fiber:
//! ```text
//!   loop {
//!       let since = irq::arm(vec);         // snapshot + route the IRQ to THIS core
//!       // enable / submit the device work that will raise the IRQ
//!       irq::wait(vec, since, timeout_ms); // park until it fires (or timeout)
//!       // service (drain the ring / read the completion)
//!   }
//! ```
//! **Rules that keep it correct + general:**
//! - `arm()` BEFORE the device submit closes the lost-wakeup window (an IRQ that
//!   races the park still advances the count past the snapshot).
//! - `arm()` re-routes the MSI-X dest to the calling core, so the driver may run
//!   on any core / migrate; the IRQ always wakes the core that's about to wait.
//! - **Never hold a lock across `wait()`** — a same-core fiber spinning on it
//!   would deadlock the parked waiter. Snapshot/submit under the lock, drop it,
//!   then `arm`/`wait`, then re-acquire to service.
//!
//! WASM drivers use the mirror host-fns `npk_irq_register` / `npk_irq_arm` /
//! `npk_irq_wait` (see `wasm.rs`). The device must expose an MSI-X capability
//! (NIC / NVMe / AX200 / modern virtio do; legacy MSI-only devices are TODO).

use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use spin::Mutex;

use crate::drivers::pci::{self, PciAddr};
use crate::interrupts::{DEVICE_IRQ_VEC_BASE, DEVICE_IRQ_VEC_COUNT};

/// Per-vector registration: which device/MSI-X-entry a vector drives, and the
/// LAPIC the IRQ is currently pointed at. Lets `arm()` re-route the IRQ to the
/// core that is about to wait on it — so a device's interrupt always wakes the
/// right core out of HLT regardless of which fiber/core services it.
#[derive(Clone, Copy)]
struct IrqReg {
    dev: PciAddr,
    entry: u16,
    last_dest: u32, // APIC ID the MSI-X entry currently targets
}
static IRQ_REG: Mutex<[Option<IrqReg>; 256]> = Mutex::new([None; 256]);

/// Per-vector fired count, bumped by the ISR. Indexed by IDT vector (full
/// 256 so the ISR indexes without a bounds branch). A driver snapshots the
/// count before submitting a command, then waits for it to advance.
static IRQ_FIRED: [AtomicU64; 256] = [const { AtomicU64::new(0) }; 256];

/// Bump the fired count for `vector`. Called ONLY from the device ISR.
/// Release so a parked fiber observing the advance also sees the device
/// data the IRQ signalled.
#[inline]
pub fn note_fired(vector: u8) {
    IRQ_FIRED[vector as usize].fetch_add(1, Ordering::Release);
}

/// Current fired count for `vector`. Acquire pairs with `note_fired`.
#[inline]
pub fn fired_count(vector: u8) -> u64 {
    IRQ_FIRED[vector as usize].load(Ordering::Acquire)
}

/// Next free device-IRQ vector slot. Vectors are never freed (a driver lives
/// for the boot); the pool (`DEVICE_IRQ_VEC_COUNT`) is sized for all expected
/// HW drivers.
static NEXT_SLOT: AtomicUsize = AtomicUsize::new(0);

/// Allocate a fresh LAPIC vector from the device-IRQ pool, or None if
/// exhausted. The matching IDT entry is already installed (`interrupts::init`).
pub fn alloc_vector() -> Option<u8> {
    let s = NEXT_SLOT.fetch_add(1, Ordering::Relaxed);
    if s < DEVICE_IRQ_VEC_COUNT {
        Some(DEVICE_IRQ_VEC_BASE + s as u8)
    } else {
        None
    }
}

/// Snapshot the fired count for `vector` BEFORE submitting the device
/// command (ringing the doorbell). Pass the returned token to `wait`. This
/// closes the lost-wakeup window: an IRQ that fires between submit and park
/// still advances the count past the snapshot, so `wait` returns at once.
///
/// Also routes the IRQ to the CURRENT core (where the caller will `wait`), so
/// the device's interrupt wakes this core out of HLT → its scheduler resumes
/// the parked fiber with ~no latency. Reprograms the MSI-X dest only when the
/// waiting core changed — a no-op for a driver that always services on its own
/// pinned fiber; cheap (one MMIO write) for one that moves between cores.
pub fn arm(vector: u8) -> u64 {
    route_to_current(vector);
    fired_count(vector)
}

/// Route an already-registered device IRQ to the CURRENT core. For the
/// "wake this core" usage where no fiber calls `arm`/`wait` — e.g. the host
/// NIC RX-IRQ, which targets the vCPU core so RX arrival wakes the vCPU to
/// pump. No-op if `vector` isn't registered or already targets this core.
pub fn route_to_current(vector: u8) {
    if vector == 0 {
        return;
    }
    let apic = crate::interrupts::current_apic_id();
    let mut reg = IRQ_REG.lock();
    if let Some(r) = reg[vector as usize].as_mut() {
        if r.last_dest != apic {
            pci::msix_set_dest(r.dev, r.entry, apic);
            r.last_dest = apic;
        }
    }
}

/// Route an already-registered device IRQ to `core`, which need not be the core
/// running this code. The host NIC's RX interrupt belongs on whichever core
/// DRAINS the card; while a microvm owns the tap that is Core 0, and Core 0
/// idles in `hlt` — so without this the card's arrival interrupt wakes a core
/// that isn't looking, and the drain falls back to the 100 Hz timer.
/// No-op if `vector` isn't registered, or `core` is unknown.
pub fn route_to_core(vector: u8, core: usize) {
    if vector == 0 {
        return;
    }
    let apic = {
        let cores = crate::smp::per_core::CORES.lock();
        match cores.get(core) {
            Some(c) => c.apic_id,
            None => return,
        }
    };
    let mut reg = IRQ_REG.lock();
    if let Some(r) = reg[vector as usize].as_mut() {
        if r.last_dest != apic {
            pci::msix_set_dest(r.dev, r.entry, apic);
            r.last_dest = apic;
        }
    }
}

/// Park the current fiber until `vector` fires (its count moves past `since`)
/// or `timeout_ms` elapses. Returns true if the IRQ fired, false on timeout.
/// MUST be called from inside a fiber (returns false otherwise — the caller
/// should fall back to polling).
pub fn wait(vector: u8, since: u64, timeout_ms: u64) -> bool {
    crate::smp::fiber::irq_wait(vector, since, timeout_ms)
}

/// Allocate a vector and program `dev`'s MSI-X table `entry` to deliver it to
/// the CURRENT core's LAPIC. Call this from the driver fiber so the IRQ wakes
/// exactly the core that will service it. Returns the vector, or None if the
/// device has no usable MSI-X capability or the vector pool is exhausted.
/// One-shot guard for the VT-d interrupt-remapping check below.
static IR_HANDLED: AtomicBool = AtomicBool::new(false);

/// Make compatibility-format MSIs (our `0xFEE0_0000` messages) deliverable.
///
/// Intel VT-d interrupt remapping, when the platform/firmware enables it (e.g.
/// HP "Kernel DMA Protection"), BLOCKS compatibility-format interrupt requests
/// — it expects remappable-format requests indexing the IR table. That silently
/// drops every device MSI we emit (table programmed perfectly, zero IRQs). We
/// don't use IR anywhere (the LAPIC timer is a *local* interrupt; every device
/// polls today), so we disable it once → compatibility MSIs reach the LAPIC.
/// Linux works on the same HW by emitting remappable-format MSIs; we take the
/// simpler route. No DMAR (no VT-d) or IR already off → no-op. Safe: nothing in
/// this OS relies on a remapped I/O interrupt.
fn ensure_msi_deliverable() {
    if IR_HANDLED.swap(true, Ordering::Relaxed) {
        return;
    }
    let Some(dmar) = crate::drivers::acpi::find_table(b"DMAR") else {
        crate::kprintln!("[npk] irq: no DMAR table — no VT-d, compatibility MSIs OK");
        return;
    };
    crate::drivers::acpi::ensure_mapped_pub(dmar, 4096);
    // DMAR: ACPI header (length @4 u32), Flags @37 (1 byte). Remapping
    // structures start at +48; walk for a DRHD (type 0). Its register base is
    // at DRHD+8 (after type:2 len:2 flags:1 rsvd:1 segment:2).
    // SAFETY: DMAR table mapped above; reads bounded by `len` (< one page).
    let len = unsafe { core::ptr::read_volatile((dmar + 4) as *const u32) } as usize;
    let flags = unsafe { core::ptr::read_volatile((dmar + 37) as *const u8) };
    let scan_end = len.min(4096);
    let mut off = 48usize;
    let mut regbase = 0u64;
    while off + 4 <= scan_end {
        let ty = unsafe { core::ptr::read_volatile((dmar + off) as *const u16) };
        let slen = unsafe { core::ptr::read_volatile((dmar + off + 2) as *const u16) } as usize;
        if slen == 0 {
            break;
        }
        if ty == 0 && off + 16 <= scan_end {
            regbase = unsafe { core::ptr::read_volatile((dmar + off + 8) as *const u64) };
            break;
        }
        off += slen;
    }
    if regbase == 0 {
        crate::kprintln!("[npk] irq: DMAR present (flags={:#x}) but no DRHD regbase", flags);
        return;
    }
    // Map the IOMMU register page (defensive, uncached).
    let page = (regbase as usize) & !0xFFF;
    let _ = crate::paging::map_page(
        page as u64,
        page as u64,
        crate::paging::PageFlags::PRESENT
            | crate::paging::PageFlags::WRITABLE
            | crate::paging::PageFlags::NO_CACHE,
    );
    const GCMD: u64 = 0x18; // Global Command Register
    const GSTS: u64 = 0x1C; // Global Status Register
    // SAFETY: VT-d remapping-hardware MMIO at the DRHD register base.
    let gsts = unsafe { core::ptr::read_volatile((regbase + GSTS) as *const u32) };
    let ires = (gsts >> 25) & 1; // Interrupt Remapping Enable Status
    crate::kprintln!(
        "[npk] irq: VT-d @ {:#x} GSTS={:#010x} IR={} TE={} QI={} dmar_flags={:#x}",
        regbase, gsts, ires, (gsts >> 31) & 1, (gsts >> 26) & 1, flags,
    );
    if ires == 1 {
        // Disable IR (Linux pattern): re-assert the persistent enables we want
        // to KEEP (TE bit31, QIE bit26, CFI bit23) with IRE (bit25) cleared,
        // then wait for IRES to drop. Preserving TE keeps any active DMA
        // translation intact; we only turn off interrupt remapping.
        let keep = gsts & ((1 << 31) | (1 << 26) | (1 << 23));
        // SAFETY: GCMD write per VT-d spec; only reached when IR is enabled.
        unsafe { core::ptr::write_volatile((regbase + GCMD) as *mut u32, keep); }
        let mut ok = false;
        for _ in 0..1_000_000u32 {
            let s = unsafe { core::ptr::read_volatile((regbase + GSTS) as *const u32) };
            if (s >> 25) & 1 == 0 {
                ok = true;
                break;
            }
            core::hint::spin_loop();
        }
        crate::kprintln!(
            "[npk] irq: VT-d interrupt remapping {} — compatibility MSIs now allowed",
            if ok { "DISABLED" } else { "disable TIMED OUT" },
        );
    }
}

pub fn register(dev: PciAddr, entry: u16) -> Option<u8> {
    // VT-d IR (if the platform enabled it) silently drops our compatibility-
    // format MSIs — disable it once before programming any device MSI-X.
    ensure_msi_deliverable();
    let vector = alloc_vector()?;
    let dest = crate::interrupts::current_apic_id();
    if pci::program_msix(dev, entry, vector, dest) {
        IRQ_REG.lock()[vector as usize] = Some(IrqReg { dev, entry, last_dest: dest });
        Some(vector)
    } else {
        None
    }
}
