//! The x86-64 half of device interrupts: MSI-X routing → LAPIC vector.
//!
//! The legacy 8259 PIC is fully masked (re-init traps via SMI on HP/Insyde
//! firmware — see `interrupts::init`). Device interrupts are routed via
//! MSI-X (or plain MSI): a device's table entry delivers a chosen LAPIC
//! vector to a chosen destination APIC ID. MSI writes go straight to the
//! LAPIC (message address `0xFEE0_0000 | apic<<12`), bypassing the PIC.
//! Level lines that only exist on an I/O APIC go through `register_gsi`.
//!
//! The ISR (in `interrupts.rs`) does the minimum: bump a per-vector atomic
//! fired-count + LAPIC EOI. The device's MSI-X targets the APIC of the
//! core running the driver fiber, so the interrupt itself wakes that core
//! out of HLT. No polling, no IPI: the IRQ is the wake.
//!
//! Vectors are per core, as Linux' `irq_matrix`: each core has the whole
//! device range, and a driver holds an interrupt number (`Irq`) that stays
//! put while `arm` moves its vector to another core.
//!
//! MSI-X when the device has it (NIC / NVMe / modern virtio), plain MSI
//! with one vector otherwise (RTL8822CE).

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use spin::Mutex;

use super::{desc, note_fired, Desc, Irq};
use crate::drivers::pci::{self, PciAddr};
use crate::interrupts::{DEVICE_IRQ_VEC_BASE, DEVICE_IRQ_VEC_COUNT};

/// Cores the matrix serves: an xAPIC id has 8 bits.
const CORES: usize = 256;
/// Every device vector of every core can carry one interrupt.
pub const NR_IRQS: usize = DEVICE_IRQ_VEC_COUNT * CORES;
/// Words of the per-core vector bitmap.
const WORDS: usize = DEVICE_IRQ_VEC_COUNT.div_ceil(64);

/// Per core, the interrupt each device vector delivers (0: none). The ISR's
/// lookup, as Linux' per-CPU `vector_irq`.
static VECTOR_IRQ: [[AtomicU16; DEVICE_IRQ_VEC_COUNT]; CORES] =
    [const { [const { AtomicU16::new(0) }; DEVICE_IRQ_VEC_COUNT] }; CORES];

/// Where an interrupt comes from and where it is delivered.
#[derive(Clone, Copy)]
struct Reg {
    dev: PciAddr,
    /// MSI-X table entry, `MSI` or `GSI`.
    entry: u16,
    core: usize,
    vector: u8,
    /// The core and vector before the last move. Kept until the next move,
    /// so a message already on its way to the old target still finds this
    /// interrupt, as Linux' deferred `__vector_cleanup`.
    prev: Option<(usize, u8)>,
}

/// `Reg::entry` of a device driven by plain MSI instead of MSI-X.
const MSI: u16 = u16::MAX;
/// `Reg::entry` of a line routed through an I/O APIC.
const GSI: u16 = u16::MAX - 1;

/// The vector matrix, as Linux' `irq_matrix`.
struct State {
    regs: BTreeMap<Irq, Reg>,
    /// Device vectors in use, per core.
    used: Vec<[u64; WORDS]>,
}

static STATE: Mutex<State> = Mutex::new(State { regs: BTreeMap::new(), used: Vec::new() });

impl State {
    /// A free device vector on `core`, `want` if that one is free: keeping
    /// the vector across a move changes only the destination.
    fn take_vector(&mut self, core: usize, want: Option<u8>, irq: Irq) -> Option<u8> {
        if core >= CORES {
            return None;
        }
        if self.used.len() <= core {
            self.used.resize(core + 1, [0; WORDS]);
        }
        let used = &mut self.used[core];
        let free = |used: &[u64; WORDS], s: usize| used[s / 64] & (1 << (s % 64)) == 0;
        let slot = want
            .map(|v| v.wrapping_sub(DEVICE_IRQ_VEC_BASE) as usize)
            .filter(|&s| s < DEVICE_IRQ_VEC_COUNT && free(used, s))
            .or_else(|| (0..DEVICE_IRQ_VEC_COUNT).find(|&s| free(used, s)))?;
        used[slot / 64] |= 1 << (slot % 64);
        VECTOR_IRQ[core][slot].store(irq, Ordering::Release);
        Some(DEVICE_IRQ_VEC_BASE + slot as u8)
    }

    fn put_vector(&mut self, core: usize, vector: u8) {
        let slot = vector.wrapping_sub(DEVICE_IRQ_VEC_BASE) as usize;
        if core >= CORES || slot >= DEVICE_IRQ_VEC_COUNT {
            return;
        }
        VECTOR_IRQ[core][slot].store(0, Ordering::Release);
        if let Some(used) = self.used.get_mut(core) {
            used[slot / 64] &= !(1 << (slot % 64));
        }
    }

    /// A number and a vector on the current core for a new interrupt.
    fn alloc(&mut self, dev: PciAddr, entry: u16) -> Option<(Irq, u8, u32)> {
        let core = crate::smp::per_core::current_core_id();
        let dest = crate::interrupts::current_apic_id();
        let irq = super::alloc_irq()?;
        let Some(vector) = self.take_vector(core, None, irq) else {
            super::free_irq(irq);
            crate::kprintln!("[npk] irq: all {} device vectors of core {} in use",
                DEVICE_IRQ_VEC_COUNT, core);
            return None;
        };
        self.regs.insert(irq, Reg { dev, entry, core, vector, prev: None });
        Some((irq, vector, dest))
    }
}

/// Route I/O APIC input `gsi` to a fresh interrupt on the current core,
/// masked. The driver unmasks it by `arm`ing it before its first `wait`.
/// None if no vector is free or no I/O APIC serves the GSI.
pub fn register_gsi(gsi: u32, level: bool, active_low: bool) -> Option<Irq> {
    if !crate::ioapic::is_free(gsi) {
        return None;
    }
    let none = PciAddr { bus: 0, device: 0, function: 0 };
    let (irq, vector, dest) = STATE.lock().alloc(none, GSI)?;
    let d = desc(irq)?;
    d.hwirq.store(gsi, Ordering::Relaxed);
    if !crate::ioapic::route(gsi, vector, dest, level, active_low) {
        super::release(irq);
        return None;
    }
    d.oneshot.store(level, Ordering::Release);
    crate::kprintln!("[npk] irq: GSI {} ({}, active-{}) as irq {} on vector {:#x}",
        gsi, if level { "level" } else { "edge" }, if active_low { "low" } else { "high" },
        irq, vector);
    Some(irq)
}

/// Interrupt-context part of a device vector, before the LAPIC EOI: find
/// the interrupt this core's `vector` carries, mask a level line (see
/// `Desc::oneshot`), tell the waiter.
pub fn isr(vector: u8) {
    let core = crate::smp::per_core::current_core_id();
    let slot = vector.wrapping_sub(DEVICE_IRQ_VEC_BASE) as usize;
    let irq = VECTOR_IRQ.get(core).and_then(|row| row.get(slot)).map_or(0, |a| a.load(Ordering::Acquire));
    let Some(d) = desc(irq) else { return };
    if d.oneshot.load(Ordering::Acquire) {
        crate::ioapic::mask(d.hwirq.load(Ordering::Relaxed));
    }
    note_fired(d);
}

/// The routing half of `irq::release`: vectors freed, an I/O APIC line
/// masked and released. False if `irq` was not registered here.
pub fn release_source(irq: Irq, d: &Desc) -> bool {
    let mut st = STATE.lock();
    let Some(r) = st.regs.remove(&irq) else { return false };
    st.put_vector(r.core, r.vector);
    if let Some((c, v)) = r.prev {
        st.put_vector(c, v);
    }
    if r.entry == GSI {
        crate::ioapic::release(d.hwirq.load(Ordering::Relaxed));
    }
    true
}

/// A level line was masked by the ISR: the driver has serviced the
/// device, so let the next assertion through.
pub fn unmask(_irq: Irq, d: &Desc) {
    crate::ioapic::unmask(d.hwirq.load(Ordering::Relaxed));
}

/// Route a registered interrupt to the current core: a vector there (the
/// same number when it is free), then the device re-pointed. For the "wake
/// this core" usage where no fiber calls `arm`/`wait`, e.g. the host NIC
/// RX interrupt that wakes the vCPU to pump. No-op if `irq` is not
/// registered or already targets this core.
pub fn route_to_current(irq: Irq) {
    let Some(d) = desc(irq) else { return };
    let core = crate::smp::per_core::current_core_id();
    let mut st = STATE.lock();
    let Some(mut r) = st.regs.get(&irq).copied() else { return };
    if r.core == core {
        return;
    }
    let Some(vector) = st.take_vector(core, Some(r.vector), irq) else {
        crate::kprintln!("[npk] irq: no free vector on core {}; irq {} stays on core {}",
            core, irq, r.core);
        return;
    };
    // The target two moves ago has had all the time in-flight messages need.
    if let Some((c, v)) = r.prev {
        st.put_vector(c, v);
    }
    let apic = crate::interrupts::current_apic_id();
    match r.entry {
        MSI => pci::msi_set_target(r.dev, vector, apic),
        GSI => crate::ioapic::set_target(d.hwirq.load(Ordering::Relaxed), vector, apic),
        e => pci::msix_set_target(r.dev, e, vector, apic),
    }
    let moved_vector = vector != r.vector;
    r.prev = Some((r.core, r.vector));
    r.core = core;
    r.vector = vector;
    st.regs.insert(irq, r);
    drop(st);
    // A new vector can lose a message sent mid-change (an unmaskable MSI);
    // one extra wake covers it, as Linux retriggers a pending one. Drivers
    // look at the device after every wake.
    if moved_vector {
        note_fired(d);
    }
}

/// One-shot guard for the VT-d interrupt-remapping check below.
static IR_HANDLED: AtomicBool = AtomicBool::new(false);

/// Make compatibility-format MSIs (our `0xFEE0_0000` messages) deliverable.
///
/// Intel VT-d interrupt remapping, when the platform/firmware enables it (e.g.
/// HP "Kernel DMA Protection"), blocks compatibility-format interrupt requests
/// — it expects remappable-format requests indexing the IR table — and silently
/// drops every MSI we emit. We don't use IR, so we disable it once and
/// compatibility MSIs reach the LAPIC. Linux instead emits remappable-format
/// MSIs. No DMAR (no VT-d) or IR already off: no-op.
fn ensure_msi_deliverable() {
    if IR_HANDLED.swap(true, Ordering::Relaxed) {
        return;
    }
    let Some(dmar) = crate::drivers::acpi::find_table(b"DMAR") else {
        crate::kdebug!("[npk] irq: no DMAR table — no VT-d, compatibility MSIs OK");
        return;
    };
    // DMAR: ACPI header (length @4 u32), Flags @37 (1 byte). Remapping
    // structures start at +48; every DRHD (type 0) is one remapping unit,
    // its register base at DRHD+8 (after type:2 len:2 flags:1 rsvd:1
    // segment:2). Intel clients have several; each is turned off, as Linux
    // walks every DRHD (`dmar_table_init`).
    crate::drivers::acpi::ensure_mapped_pub(dmar, 4096);
    // SAFETY: the DMAR header, mapped above; firmware tables do not change.
    let len = unsafe { crate::hw::PhysView::new(dmar as u64, 4096) }.u32(4u32).unwrap_or(0) as usize;
    crate::drivers::acpi::ensure_mapped_pub(dmar, len.max(48));
    // SAFETY: the whole DMAR table, mapped above. Reads are bounded by `len`.
    let t = unsafe { crate::hw::PhysView::new(dmar as u64, len.max(48) as u64) };
    let flags = t.u8(37u32).unwrap_or(0);
    let mut off = 48usize;
    let mut units = 0;
    while off + 4 <= len {
        let ty = t.u16(off).unwrap_or(0);
        let slen = t.u16(off + 2).unwrap_or(0) as usize;
        if slen == 0 {
            break;
        }
        if ty == 0 && off + 16 <= len {
            let regbase = t.u64(off + 8).unwrap_or(0);
            if regbase != 0 {
                disable_ir_unit(regbase, flags);
                units += 1;
            }
        }
        off += slen;
    }
    if units == 0 {
        crate::kprintln!("[npk] irq: DMAR present (flags={:#x}) but no DRHD regbase", flags);
    }
}

/// Turn interrupt remapping off on one VT-d unit.
fn disable_ir_unit(regbase: u64, flags: u8) {
    const GCMD: u64 = 0x18; // Global Command Register
    const GSTS: u64 = 0x1C; // Global Status Register
    // SAFETY: the DRHD register base is VT-d remapping-hardware MMIO.
    let Ok(page) = (unsafe { crate::hw::Mmio::map(regbase & !0xFFF, 0x1000) }) else {
        crate::kprintln!("[npk] irq: DRHD registers at {:#x} cannot be mapped", regbase);
        return;
    };
    let regs = page.sub(regbase & 0xFFF, GSTS + 4);
    let gsts = regs.r32(GSTS);
    let ires = (gsts >> 25) & 1; // Interrupt Remapping Enable Status
    crate::kprintln!(
        "[npk] irq: VT-d @ {:#x} GSTS={:#010x} IR={} TE={} QI={} dmar_flags={:#x}",
        regbase, gsts, ires, (gsts >> 31) & 1, (gsts >> 26) & 1, flags,
    );
    if ires == 1 {
        // Disable IR (Linux pattern): re-assert the persistent enables we want
        // to keep (TE bit31, QIE bit26, CFI bit23) with IRE (bit25) cleared,
        // then wait for IRES to drop. Preserving TE keeps any active DMA
        // translation intact; we only turn off interrupt remapping.
        let keep = gsts & ((1 << 31) | (1 << 26) | (1 << 23));
        regs.w32(GCMD, keep);
        let mut ok = false;
        for _ in 0..1_000_000u32 {
            let s = regs.r32(GSTS);
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

/// Allocate an interrupt and program `dev`'s MSI-X table `entry` (or its
/// plain MSI) to deliver it to the current core. Call this from the driver
/// fiber so the interrupt wakes exactly the core that will service it.
/// None if the device has no usable MSI or MSI-X, or no vector is free.
pub fn register(dev: PciAddr, entry: u16) -> Option<Irq> {
    // VT-d IR (if the platform enabled it) silently drops our compatibility-
    // format MSIs: disable it once before programming any device MSI-X.
    ensure_msi_deliverable();
    // MSI-X when the device has it; otherwise plain MSI with its one vector
    // (the RTL8822CE). `entry` only means something for MSI-X.
    let msix = pci::has_msix(dev);
    let (irq, vector, dest) = STATE.lock().alloc(dev, if msix { entry } else { MSI })?;
    let ok = if msix {
        pci::program_msix(dev, entry, vector, dest)
    } else {
        pci::program_msi(dev, vector, dest)
    };
    if !ok {
        super::release(irq);
        return None;
    }
    if !msix {
        crate::kdebug!("[npk] irq: {:02x}:{:02x}.{} plain MSI as irq {} on vector {:#x}",
            dev.bus, dev.device, dev.function, irq, vector);
    }
    Some(irq)
}
