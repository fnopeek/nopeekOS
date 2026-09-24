//! ACPI SCI: the fixed and general-purpose event registers of the FADT.
//!
//! The kernel owns the registers; the AML driver (`aml`) owns the meaning.
//! It asks for its EC's GPE with `npk_sci_arm(gpe)`, gets the SCI as an
//! ordinary level interrupt (oneshot: masked on fire, unmasked by the next
//! `npk_wait`), and on every wake calls `npk_sci_service` — which acks what
//! fired — before it drains the EC and runs the `_Qxx` methods.
//!
//! Order as ACPICA for an EDGE GPE (Linux `ec.c` installs its handler edge-
//! triggered): clear the status bit first, then handle, so an event that
//! arrives during the handling sets the bit again and fires again.
//!
//! Before this the EC was drained every 10 s, and on the IdeaPad the EC had
//! dropped a hotkey event by then: `QR_EC` answered 0 while the same query
//! within 5 ms of SCI_EVT answered 0x1C/0x1D (brightness up/down).

use crate::serial::{inb, inw, outb, outw};
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

static ARMED: AtomicBool = AtomicBool::new(false);
/// Vector the SCI was registered on (for `report`).
pub static VECTOR: AtomicU32 = AtomicU32::new(0);
/// `service` calls, and how many found the EC GPE / a PM1 event / nothing.
static CALLS: AtomicU32 = AtomicU32::new(0);
static EC_HITS: AtomicU32 = AtomicU32::new(0);
static PM1_HITS: AtomicU32 = AtomicU32::new(0);
static EMPTY: AtomicU32 = AtomicU32::new(0);
/// GPE number the EC signals on.
static EC_GPE: AtomicU32 = AtomicU32::new(0);

#[derive(Clone, Copy)]
struct Blocks {
    sci_int: u16,
    pm1a_evt: u16,
    pm1_half: u16,
    gpe0: u16,
    gpe0_half: u16,
}

fn blocks() -> Option<Blocks> {
    let fadt = crate::acpi::find_table(b"FACP")?;
    crate::acpi::ensure_mapped_pub(fadt, 256);
    // SAFETY: FADT mapped; fields at their ACPI 6.5 §5.2.9 offsets.
    let (sci, pm1a, pm1_len, gpe0, gpe0_len) = unsafe {
        let r32 = |o: usize| core::ptr::read_unaligned((fadt + o) as *const u32);
        let r16 = |o: usize| core::ptr::read_unaligned((fadt + o) as *const u16);
        let r8 = |o: usize| core::ptr::read_volatile((fadt + o) as *const u8);
        (r16(46), r32(56), r8(88), r32(80), r8(92))
    };
    if pm1a == 0 || pm1a > 0xFFFF || gpe0 == 0 || gpe0 > 0xFFFF || gpe0_len < 2 {
        return None;
    }
    Some(Blocks {
        sci_int: sci,
        pm1a_evt: pm1a as u16,
        pm1_half: (pm1_len / 2) as u16,
        gpe0: gpe0 as u16,
        gpe0_half: (gpe0_len / 2) as u16,
    })
}

/// Take the SCI for the EC's `gpe`: every other GPE disabled (ACPICA
/// disables all at init and enables per handler), every status cleared,
/// `gpe` enabled. Returns the SCI's (GSI, level, active-low). Once only.
pub fn arm_ec(gpe: u32) -> Option<(u32, bool, bool)> {
    let b = blocks()?;
    if gpe >= b.gpe0_half as u32 * 8 { return None; }
    if ARMED.swap(true, Ordering::AcqRel) { return None; }
    EC_GPE.store(gpe, Ordering::Relaxed);
    // SAFETY: GPE0 and PM1a are the FADT's I/O blocks; status bits are
    // write-1-to-clear, enable bits plain.
    unsafe {
        for i in 0..b.gpe0_half {
            outb(b.gpe0 + b.gpe0_half + i, 0);
            outb(b.gpe0 + i, 0xFF);
        }
        let sts = inw(b.pm1a_evt);
        outw(b.pm1a_evt, sts);
        let en_port = b.gpe0 + b.gpe0_half + (gpe / 8) as u16;
        outb(en_port, 1 << (gpe % 8));
    }
    let (gsi, level, low) = crate::ioapic::isa_irq(b.sci_int as u8);
    crate::kprintln!("[npk] sci: IRQ {} -> GSI {} ({}, {}), EC on GPE {}",
        b.sci_int, gsi, if level { "level" } else { "edge" },
        if low { "active-low" } else { "active-high" }, gpe);
    Some((gsi, level, low))
}

/// Ack what raised the SCI. Bit 0: the EC's GPE was set. Bit 1: the EC has
/// an EVENT to query (status SCI_EVT) — the EC also raises its GPE when a
/// transaction's output is ready, so bit 0 alone is mostly our own reads
/// (Linux `ec.c` queries only on SCI_EVT; measured on the IdeaPad: ~60 GPEs
/// a second, driven by aml's own EC accesses, until this bit was checked).
/// Bits 16..31: the PM1 fixed events that were set AND enabled (bit 8 =
/// power button).
pub fn service() -> u32 {
    if !ARMED.load(Ordering::Acquire) { return 0; }
    let Some(b) = blocks() else { return 0 };
    let gpe = EC_GPE.load(Ordering::Relaxed);
    let mut out = 0u32;
    CALLS.fetch_add(1, Ordering::Relaxed);
    // SAFETY: as in `arm_ec`.
    unsafe {
        let sts_port = b.gpe0 + (gpe / 8) as u16;
        let bit = 1u8 << (gpe % 8);
        if inb(sts_port) & bit != 0 {
            outb(sts_port, bit);
            out |= 1;
        }
        if b.pm1_half >= 2 {
            let sts = inw(b.pm1a_evt);
            let en = inw(b.pm1a_evt + b.pm1_half);
            let fired = sts & en;
            if fired != 0 {
                outw(b.pm1a_evt, fired);
                out |= (fired as u32) << 16;
            }
        }
    }
    // SAFETY: EC status port, read only.
    if unsafe { inb(0x66) } & 0x20 != 0 {
        out |= 2;
    }
    if out & 1 != 0 { EC_HITS.fetch_add(1, Ordering::Relaxed); }
    if out >> 16 != 0 { PM1_HITS.fetch_add(1, Ordering::Relaxed); }
    if out == 0 { EMPTY.fetch_add(1, Ordering::Relaxed); }
    out
}

/// Counters and the raw registers, for `ec watch`: a line that keeps
/// firing shows as `fired` far above `EC`, with the culprit's status bit
/// standing in the dump.
pub fn report() {
    let Some(b) = blocks() else { return };
    if !ARMED.load(Ordering::Acquire) {
        crate::kprintln!("  SCI: nicht genommen (aml < 0.4.0?)");
        return;
    }
    let v = VECTOR.load(Ordering::Relaxed) as u8;
    crate::kprintln!("  SCI: vector {:#x} fired {} · service {} (EC {}, PM1 {}, leer {})",
        v, crate::irq::fired_count(v), CALLS.load(Ordering::Relaxed),
        EC_HITS.load(Ordering::Relaxed), PM1_HITS.load(Ordering::Relaxed),
        EMPTY.load(Ordering::Relaxed));
    let mut sts = alloc::string::String::new();
    let mut en = alloc::string::String::new();
    // SAFETY: GPE0 / PM1a are the FADT's I/O blocks; reads only.
    unsafe {
        for i in 0..b.gpe0_half {
            sts.push_str(&alloc::format!("{:02x} ", inb(b.gpe0 + i)));
            en.push_str(&alloc::format!("{:02x} ", inb(b.gpe0 + b.gpe0_half + i)));
        }
        crate::kprintln!("  GPE0 STS {}· EN {}· PM1 STS {:04x} EN {:04x} · EC Status {:02x}",
            sts, en, inw(b.pm1a_evt), inw(b.pm1a_evt + b.pm1_half), inb(0x66));
    }
}
