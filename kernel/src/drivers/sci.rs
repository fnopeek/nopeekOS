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

use crate::hw::{Port, PortRange};
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
    /// PM1a_EVT status and enable registers.
    pm1a_sts: Port,
    pm1a_en: Port,
    pm1_half: u16,
    /// GPE0 block: status bytes, then as many enable bytes.
    gpe0: PortRange,
    gpe0_half: u16,
}

fn blocks() -> Option<Blocks> {
    let fadt = crate::acpi::table(b"FACP")?;
    // Fields at their ACPI 6.5 §5.2.9 offsets.
    let sci = fadt.u16(46)?;
    let pm1a = fadt.io_port(56)?;
    let pm1_len = fadt.u8(88)?;
    let gpe0 = fadt.io_port(80)?;
    let gpe0_len = fadt.u8(92)?;
    if gpe0_len < 2 {
        return None;
    }
    let pm1_half = (pm1_len / 2) as u16;
    // SAFETY: the FADT's PM1a event and GPE0 blocks, the ACPI hardware this
    // module drives.
    let (pm1a_sts, pm1a_en, gpe0) = unsafe {
        (Port::new(pm1a), Port::new(pm1a.wrapping_add(pm1_half)), PortRange::new(gpe0, gpe0_len as u16))
    };
    Some(Blocks {
        sci_int: sci,
        pm1a_sts,
        pm1a_en,
        pm1_half,
        gpe0,
        gpe0_half: (gpe0_len / 2) as u16,
    })
}

/// Take the SCI for the EC's `gpe`: every other GPE disabled (ACPICA
/// disables all at init and enables per handler), every status cleared,
/// `gpe` enabled. Returns the SCI's (GSI, level, active-low). Once only.
pub fn arm_ec(gpe: u32) -> Option<(u32, bool, bool)> {
    // Whoever takes the SCI switches to ACPI mode; `acpi.legacy = 1` keeps
    // the firmware in legacy mode.
    if crate::config::get("acpi.legacy").as_deref() == Some("1") {
        crate::kprintln!("[npk] sci: acpi.legacy = 1 — firmware keeps its events, no SCI");
        return None;
    }
    let b = blocks()?;
    crate::acpi::enable_acpi_mode();
    if gpe >= b.gpe0_half as u32 * 8 { return None; }
    if ARMED.swap(true, Ordering::AcqRel) { return None; }
    EC_GPE.store(gpe, Ordering::Relaxed);
    // Status bits are write-1-to-clear, enable bits plain.
    for i in 0..b.gpe0_half {
        b.gpe0.outb(b.gpe0_half + i, 0);
        b.gpe0.outb(i, 0xFF);
    }
    let sts = b.pm1a_sts.inw();
    b.pm1a_sts.outw(sts);
    b.gpe0.outb(b.gpe0_half + (gpe / 8) as u16, 1 << (gpe % 8));
    let (gsi, level, low) = crate::ioapic::isa_irq(b.sci_int as u8);
    crate::kprintln!("[npk] sci: IRQ {} -> GSI {} ({}, {}), EC on GPE {}",
        b.sci_int, gsi, if level { "level" } else { "edge" },
        if low { "active-low" } else { "active-high" }, gpe);
    Some((gsi, level, low))
}

/// Ack what raised the SCI. Bit 0: the EC's GPE was set. Bit 1: the EC has
/// an EVENT to query (status SCI_EVT) — the EC also raises its GPE when a
/// transaction's output is ready, so bit 0 alone is mostly our own reads
/// (Linux `ec.c` queries only on SCI_EVT).
/// Bits 16..31: the PM1 fixed events that were set AND enabled (bit 8 =
/// power button).
pub fn service() -> u32 {
    if !ARMED.load(Ordering::Acquire) { return 0; }
    let Some(b) = blocks() else { return 0 };
    let gpe = EC_GPE.load(Ordering::Relaxed);
    let mut out = 0u32;
    CALLS.fetch_add(1, Ordering::Relaxed);
    let sts_off = (gpe / 8) as u16;
    let bit = 1u8 << (gpe % 8);
    if b.gpe0.inb(sts_off) & bit != 0 {
        b.gpe0.outb(sts_off, bit);
        out |= 1;
    }
    if b.pm1_half >= 2 {
        let sts = b.pm1a_sts.inw();
        let en = b.pm1a_en.inw();
        let fired = sts & en;
        if fired != 0 {
            b.pm1a_sts.outw(fired);
            out |= (fired as u32) << 16;
        }
    }
    if crate::drivers::ec::status() & 0x20 != 0 {
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
    for i in 0..b.gpe0_half {
        sts.push_str(&alloc::format!("{:02x} ", b.gpe0.inb(i)));
        en.push_str(&alloc::format!("{:02x} ", b.gpe0.inb(b.gpe0_half + i)));
    }
    crate::kprintln!("  GPE0 STS {}· EN {}· PM1 STS {:04x} EN {:04x} · EC Status {:02x}",
        sts, en, b.pm1a_sts.inw(), b.pm1a_en.inw(), crate::drivers::ec::status());
}

/// `ec gpe off|on` (diagnostic): take the EC's GPE out of the SCI or put it
/// back. While off, events are only drained on aml's periodic battery round.
pub fn set_ec_gpe(on: bool) -> bool {
    let Some(b) = blocks() else { return false };
    if !ARMED.load(Ordering::Acquire) { return false; }
    let gpe = EC_GPE.load(Ordering::Relaxed);
    let off = b.gpe0_half + (gpe / 8) as u16;
    let v = b.gpe0.inb(off);
    let bit = 1u8 << (gpe % 8);
    b.gpe0.outb(off, if on { v | bit } else { v & !bit });
    true
}

/// `ec mode legacy|acpi` (diagnostic): hand the events back to the firmware (SMM)
/// with FADT.ACPI_DISABLE, or take them again with ACPI_ENABLE. Returns
/// SCI_EN afterwards.
pub fn set_acpi_mode(acpi: bool) -> Option<bool> {
    let fadt = crate::acpi::table(b"FACP")?;
    // SMI_CMD 48, ACPI_ENABLE 52, ACPI_DISABLE 53, PM1a_CNT 64.
    let smi = fadt.io_port(48)?;
    let en = fadt.u8(52)?;
    let dis = fadt.u8(53)?;
    let cnt = fadt.io_port(64)?;
    // SAFETY: the FADT's SMI command port and PM1a control port.
    let (smi, cnt) = unsafe { (Port::new(smi), Port::new(cnt)) };
    smi.outb(if acpi { en } else { dis });
    let tsc_ms = (crate::interrupts::tsc_freq() / 1000).max(1);
    let t0 = crate::interrupts::rdtsc();
    loop {
        let on = cnt.inw() & 1 != 0;
        if on == acpi || crate::interrupts::rdtsc() - t0 > 3000 * tsc_ms {
            return Some(on);
        }
        core::hint::spin_loop();
    }
}
