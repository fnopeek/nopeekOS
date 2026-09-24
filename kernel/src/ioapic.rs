//! I/O APIC — the router for interrupts that are not MSI.
//!
//! Everything so far came in as MSI/MSI-X, which goes straight to a LAPIC.
//! What does not — the PS/2 keyboard (ISA IRQ 1), the ACPI SCI (EC and
//! battery events), a GPIO controller's line (the IdeaPad touchpads) — goes
//! through an I/O APIC: one redirection entry per input pin ("GSI") says
//! which vector, which core, edge or level, high or low.
//! `docs/plan/CORES_AND_EVENTS.md`, Stufe 3a.
//!
//! Ported from Linux `arch/x86/kernel/apic/io_apic.c` (register access,
//! entry layout, `clear_IO_APIC_pin`, `__eoi_ioapic_pin`) and
//! `arch/x86/kernel/acpi/boot.c` (MADT I/O APIC and Interrupt Source
//! Override entries, `mp_override_legacy_irq`).
//!
//! **One deliberate difference from Linux at boot:** Linux masks every pin
//! (`clear_IO_APIC`). We leave pins with SMI or ExtINT delivery as the
//! firmware set them, like Linux leaves SMI pins: the PIT still reaches
//! Core 0 through the legacy PIC on some machines, and that path may run
//! through an ExtINT pin ("virtual wire"). Stage 3e removes the PIT tick;
//! then this exception can go. Every other pin is masked until a driver
//! routes it.
//!
//! Locking: the index/data register pair must not be interleaved, and the
//! device-IRQ ISR masks level lines — so every access takes `LOCK` with
//! interrupts off on the calling core (`without_interrupts`).

use alloc::vec::Vec;
use spin::Mutex;

use crate::kprintln;

/// One I/O APIC from the MADT.
#[derive(Clone, Copy)]
struct IoApic {
    id: u8,
    base: u64,
    gsi_base: u32,
    pins: u32,
    version: u8,
}

/// MADT type 2: ISA IRQ `bus_irq` arrives on `gsi` with MPS INTI `flags`
/// (bits 1:0 polarity, 3:2 trigger; 0 = conforms to the bus).
#[derive(Clone, Copy)]
struct Override {
    bus_irq: u8,
    gsi: u32,
    flags: u16,
}

struct State {
    apics: Vec<IoApic>,
    overrides: Vec<Override>,
    /// GSIs a route has been written for — one owner each.
    routed: Vec<u32>,
}

static LOCK: Mutex<State> = Mutex::new(State { apics: Vec::new(), overrides: Vec::new(), routed: Vec::new() });

// Redirection entry, low dword (io_apic.h `struct IO_APIC_route_entry`).
const RTE_DELIVERY_SHIFT: u32 = 8; // 3 bits
const DELIVERY_SMI: u32 = 2;
const DELIVERY_EXTINT: u32 = 7;
const RTE_ACTIVE_LOW: u32 = 1 << 13;
const RTE_IRR: u32 = 1 << 14;
const RTE_LEVEL: u32 = 1 << 15;
const RTE_MASKED: u32 = 1 << 16;

fn read(a: &IoApic, reg: u32) -> u32 {
    // SAFETY: the I/O APIC's register window was mapped NO_CACHE in `init`;
    // IOREGSEL at +0, IOWIN at +0x10 (`struct io_apic`).
    unsafe {
        core::ptr::write_volatile(a.base as *mut u32, reg);
        core::ptr::read_volatile((a.base + 0x10) as *const u32)
    }
}

fn write(a: &IoApic, reg: u32, val: u32) {
    // SAFETY: as in `read`.
    unsafe {
        core::ptr::write_volatile(a.base as *mut u32, reg);
        core::ptr::write_volatile((a.base + 0x10) as *mut u32, val);
    }
}

fn read_entry(a: &IoApic, pin: u32) -> (u32, u32) {
    (read(a, 0x10 + 2 * pin), read(a, 0x11 + 2 * pin))
}

/// `__ioapic_write_entry`: the high dword first, so the entry is never live
/// with a new vector and an old destination.
fn write_entry(a: &IoApic, pin: u32, lo: u32, hi: u32) {
    write(a, 0x11 + 2 * pin, hi);
    write(a, 0x10 + 2 * pin, lo);
}

/// `__eoi_ioapic_pin`: clear a level line's Remote-IRR.
fn eoi_pin(a: &IoApic, pin: u32, vector: u8) {
    if a.version >= 0x20 {
        // SAFETY: the EOI register at +0x40 exists from version 0x20 on.
        unsafe { core::ptr::write_volatile((a.base + 0x40) as *mut u32, vector as u32) };
    } else {
        // Masked and edge for a moment, then the level entry again.
        let (lo, hi) = read_entry(a, pin);
        write_entry(a, pin, (lo | RTE_MASKED) & !RTE_LEVEL, hi);
        write_entry(a, pin, lo, hi);
    }
}

/// `clear_IO_APIC_pin`, minus ExtINT (see the module note).
fn clear_pin(a: &IoApic, pin: u32) {
    let (mut lo, hi) = read_entry(a, pin);
    let mode = (lo >> RTE_DELIVERY_SHIFT) & 7;
    if mode == DELIVERY_SMI || mode == DELIVERY_EXTINT {
        return;
    }
    if lo & RTE_MASKED == 0 {
        lo |= RTE_MASKED;
        write_entry(a, pin, lo, hi);
        lo = read_entry(a, pin).0;
    }
    if lo & RTE_IRR != 0 {
        // An explicit EOI clears Remote-IRR only in level mode.
        if lo & RTE_LEVEL == 0 {
            lo |= RTE_LEVEL;
            write_entry(a, pin, lo, hi);
        }
        eoi_pin(a, pin, (lo & 0xFF) as u8);
    }
    // `ioapic_mask_entry`: everything cleared except the mask bit.
    write_entry(a, pin, RTE_MASKED, 0);
    if read_entry(a, pin).0 & RTE_IRR != 0 {
        kprintln!("[npk] ioapic: Remote-IRR stuck on id {} pin {}", a.id, pin);
    }
}

/// Parse the MADT's I/O APICs and overrides, map them, and mask every pin
/// the firmware did not reserve. Changes nothing a driver relies on: no
/// interrupt reaches us through an I/O APIC before this.
pub fn init() {
    let Some(madt) = crate::acpi::find_table(b"APIC") else {
        kprintln!("[npk] ioapic: no MADT");
        return;
    };
    // SAFETY: the MADT header is identity-mapped; length checked below.
    let len = unsafe { *((madt + 4) as *const u32) } as usize;
    if !(44..=0x10000).contains(&len) {
        return;
    }
    crate::acpi::ensure_mapped_pub(madt, len);

    let mut apics = Vec::new();
    let mut overrides = Vec::new();
    let mut off = 44;
    while off + 2 <= len {
        // SAFETY: inside the MADT, bounds checked by the loop and entry length.
        let ty = unsafe { *((madt + off) as *const u8) };
        let elen = unsafe { *((madt + off + 1) as *const u8) } as usize;
        if elen < 2 || off + elen > len {
            break;
        }
        match ty {
            // Type 1: I/O APIC — id, reserved, address (u32), GSI base (u32).
            1 if elen >= 12 => {
                // SAFETY: elen >= 12 covers the fields read.
                let (id, addr, gsi_base) = unsafe {
                    (*((madt + off + 2) as *const u8),
                     core::ptr::read_unaligned((madt + off + 4) as *const u32),
                     core::ptr::read_unaligned((madt + off + 8) as *const u32))
                };
                apics.push(IoApic { id, base: addr as u64, gsi_base, pins: 0, version: 0 });
            }
            // Type 2: Interrupt Source Override — bus, source, GSI, flags.
            2 if elen >= 10 => {
                // SAFETY: elen >= 10 covers the fields read.
                let (bus_irq, gsi, flags) = unsafe {
                    (*((madt + off + 3) as *const u8),
                     core::ptr::read_unaligned((madt + off + 4) as *const u32),
                     core::ptr::read_unaligned((madt + off + 8) as *const u16))
                };
                if bus_irq < 16 {
                    overrides.push(Override { bus_irq, gsi, flags });
                }
            }
            _ => {}
        }
        off += elen;
    }

    for a in apics.iter_mut() {
        let _ = crate::paging::map_page(a.base & !0xFFF, a.base & !0xFFF,
            crate::paging::PageFlags::PRESENT
                | crate::paging::PageFlags::WRITABLE
                | crate::paging::PageFlags::NO_CACHE);
        let r1 = read(a, 1); // IOAPIC version register
        a.version = (r1 & 0xFF) as u8;
        a.pins = ((r1 >> 16) & 0xFF) + 1;
        let mut kept = 0;
        for pin in 0..a.pins {
            let mode = (read_entry(a, pin).0 >> RTE_DELIVERY_SHIFT) & 7;
            if mode == DELIVERY_SMI || mode == DELIVERY_EXTINT {
                kept += 1;
            }
            clear_pin(a, pin);
        }
        kprintln!("[npk] ioapic: id {} @ {:#x}, GSI {}-{}, version {:#x}{}",
            a.id, a.base, a.gsi_base, a.gsi_base + a.pins - 1, a.version,
            if kept > 0 { alloc::format!(", {} SMI/ExtINT pin(s) left to firmware", kept) }
            else { alloc::string::String::new() });
    }
    for o in &overrides {
        kprintln!("[npk] ioapic: ISA IRQ {} -> GSI {} (flags {:#x})", o.bus_irq, o.gsi, o.flags);
    }
    crate::interrupts::without_interrupts(|| {
        let mut st = LOCK.lock();
        st.apics = apics;
        st.overrides = overrides;
    });
}

/// Where ISA IRQ `irq` arrives, and how: (GSI, level, active_low).
/// `mp_override_legacy_irq`: an override wins; ISA conforms to edge/high;
/// a level override on IRQ 0 is a known firmware bug and taken as edge.
pub fn isa_irq(irq: u8) -> (u32, bool, bool) {
    let o = crate::interrupts::without_interrupts(|| {
        LOCK.lock().overrides.iter().find(|o| o.bus_irq == irq).copied()
    });
    match o {
        None => (irq as u32, false, false),
        Some(o) => {
            let polarity = o.flags & 3;
            let mut trigger = (o.flags >> 2) & 3;
            if irq == 0 && trigger == 3 {
                trigger = 1;
            }
            (o.gsi, trigger == 3, polarity == 3)
        }
    }
}

fn with_pin<R>(gsi: u32, f: impl FnOnce(&IoApic, u32) -> R) -> Option<R> {
    crate::interrupts::without_interrupts(|| {
        let st = LOCK.lock();
        let a = st.apics.iter().find(|a| gsi >= a.gsi_base && gsi < a.gsi_base + a.pins)?;
        Some(f(a, gsi - a.gsi_base))
    })
}

/// Route `gsi` to `vector` on the LAPIC `dest_apic` (fixed delivery,
/// physical destination), left MASKED — `unmask` when the driver is ready.
/// False if no I/O APIC serves this GSI.
pub fn route(gsi: u32, vector: u8, dest_apic: u32, level: bool, active_low: bool) -> bool {
    crate::interrupts::without_interrupts(|| {
        let mut st = LOCK.lock();
        if st.routed.contains(&gsi) {
            return false; // one owner per line
        }
        let Some(a) = st.apics.iter().find(|a| gsi >= a.gsi_base && gsi < a.gsi_base + a.pins).copied()
        else {
            return false;
        };
        let mut lo = vector as u32 | RTE_MASKED;
        if level { lo |= RTE_LEVEL; }
        if active_low { lo |= RTE_ACTIVE_LOW; }
        write_entry(&a, gsi - a.gsi_base, lo, (dest_apic & 0xFF) << 24);
        st.routed.push(gsi);
        true
    })
}

/// Is `gsi` served by an I/O APIC and still without an owner?
pub fn is_free(gsi: u32) -> bool {
    crate::interrupts::without_interrupts(|| {
        let st = LOCK.lock();
        !st.routed.contains(&gsi)
            && st.apics.iter().any(|a| gsi >= a.gsi_base && gsi < a.gsi_base + a.pins)
    })
}

pub fn mask(gsi: u32) {
    with_pin(gsi, |a, pin| {
        let (lo, hi) = read_entry(a, pin);
        write_entry(a, pin, lo | RTE_MASKED, hi);
    });
}

pub fn unmask(gsi: u32) {
    with_pin(gsi, |a, pin| {
        let (lo, hi) = read_entry(a, pin);
        write_entry(a, pin, lo & !RTE_MASKED, hi);
    });
}

/// Re-point `gsi` at LAPIC `dest_apic` (the core that waits on it).
pub fn set_dest(gsi: u32, dest_apic: u32) {
    with_pin(gsi, |a, pin| {
        let (lo, _) = read_entry(a, pin);
        write_entry(a, pin, lo, (dest_apic & 0xFF) << 24);
    });
}
