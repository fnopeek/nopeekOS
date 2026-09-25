//! I/O APIC, ported from Linux `arch/x86/kvm/ioapic.c` (version 0x11, 24
//! pins, MMIO at 0xFEC00000: IOREGSEL at +0x00, IOWIN at +0x10).
//!
//! The guest's device lines reach it and the 8259 alike (KVM's default GSI
//! routing); which one delivers is the guest's choice — with an I/O APIC in
//! the MP table Linux masks the 8259. A serviced pin becomes a fixed-vector
//! message to its destination LAPIC(s) through the posted-interrupt path
//! (`lapic::deliver_ipi`), so a vCPU on another core is kicked like for an IPI.
//!
//! Every pin our devices drive is declared edge-triggered in the MP table
//! (they signal completions as pulses). A pin the guest nonetheless programs
//! level-triggered is served like an edge and never sets Remote IRR: without
//! the LAPIC's EOI broadcast wired back here, a set Remote IRR would block the
//! pin for good.

use crate::microvm::cpu::svm::lapic;

pub const IOAPIC_BASE: u64 = 0xFEC0_0000;
pub const IOAPIC_SIZE: u64 = 0x1000;

const NUM_PINS: usize = 24;
/// `IOAPIC_VERSION_ID`, max redirection entry 23 in bits 23:16.
const VERSION: u32 = 0x11 | ((NUM_PINS as u32 - 1) << 16);

const IOREGSEL: u32 = 0x00;
const IOWIN: u32 = 0x10;

const REG_ID: u32 = 0x00;
const REG_VER: u32 = 0x01;
const REG_ARB: u32 = 0x02;
const REG_REDIR_FIRST: u32 = 0x10;

// Redirection entry fields (`union kvm_ioapic_redirect_entry`).
const RTE_VECTOR: u64 = 0xFF;
const RTE_DELIVERY_MODE_SHIFT: u32 = 8;
const RTE_DEST_LOGICAL: u64 = 1 << 11;
const RTE_DELIVERY_STATUS: u64 = 1 << 12;
const RTE_REMOTE_IRR: u64 = 1 << 14;
const RTE_MASK: u64 = 1 << 16;
const RTE_DEST_SHIFT: u32 = 56;

/// ISA IRQ → pin, as the MP table declares it: IRQ 0 (the PIT) on pin 2,
/// every other line on its own number.
pub fn isa_pin(irq: u8) -> usize {
    if irq == 0 { 2 } else { irq as usize }
}

pub struct Ioapic {
    id: u32,
    ioregsel: u32,
    /// Line state per pin (edge detection).
    irr: u32,
    redirtbl: [u64; NUM_PINS],
}

impl Ioapic {
    /// `kvm_ioapic_reset`: every pin masked.
    pub const fn new() -> Self {
        Ioapic { id: 0, ioregsel: 0, irr: 0, redirtbl: [RTE_MASK; NUM_PINS] }
    }

    /// The APIC ID the MP table gives the I/O APIC (after the vCPUs').
    pub fn set_id(&mut self, id: u8) { self.id = id as u32; }

    /// `ioapic_set_irq` for an edge pin: deliver on the rising edge.
    pub fn set_irq(&mut self, pin: usize, level: bool, from: u8) {
        if pin >= NUM_PINS { return; }
        let mask = 1u32 << pin;
        if !level {
            self.irr &= !mask;
            return;
        }
        let old = self.irr;
        self.irr |= mask;
        if old != self.irr {
            self.service(pin, from);
        }
    }

    /// A device event: a rising and a falling edge.
    pub fn pulse(&mut self, pin: usize, from: u8) {
        self.set_irq(pin, true, from);
        self.set_irq(pin, false, from);
    }

    /// `ioapic_service` + `kvm_irq_delivery_to_apic`.
    fn service(&mut self, pin: usize, from: u8) {
        let e = self.redirtbl[pin];
        if e & RTE_MASK != 0 { return; }
        lapic::deliver_msg(
            (e >> RTE_DEST_SHIFT) as u8,
            e & RTE_DEST_LOGICAL != 0,
            ((e >> RTE_DELIVERY_MODE_SHIFT) & 0x7) as u8,
            (e & RTE_VECTOR) as u8,
            from,
        );
    }

    /// `ioapic_read_indirect`.
    fn read_indirect(&self) -> u32 {
        match self.ioregsel {
            REG_ID | REG_ARB => (self.id & 0xF) << 24,
            REG_VER => VERSION,
            r if r >= REG_REDIR_FIRST => {
                let idx = ((r - REG_REDIR_FIRST) >> 1) as usize;
                let Some(&e) = self.redirtbl.get(idx) else { return u32::MAX };
                if r & 1 != 0 { (e >> 32) as u32 } else { e as u32 }
            }
            _ => 0,
        }
    }

    /// `ioapic_write_indirect`: Remote IRR and delivery status are read-only.
    fn write_indirect(&mut self, val: u32) {
        match self.ioregsel {
            REG_ID => self.id = (val >> 24) & 0xF,
            REG_VER | REG_ARB => {}
            r if r >= REG_REDIR_FIRST => {
                let idx = ((r - REG_REDIR_FIRST) >> 1) as usize;
                let Some(e) = self.redirtbl.get_mut(idx) else { return };
                let keep = *e & (RTE_REMOTE_IRR | RTE_DELIVERY_STATUS);
                *e = if r & 1 != 0 {
                    (*e & 0xFFFF_FFFF) | (val as u64) << 32
                } else {
                    (*e & !0xFFFF_FFFF) | val as u64
                };
                *e = (*e & !(RTE_REMOTE_IRR | RTE_DELIVERY_STATUS)) | keep;
                // Edge pins never hold Remote IRR (KVM clears it the same way).
                *e &= !RTE_REMOTE_IRR;
            }
            _ => {}
        }
    }

    /// MMIO at `off` into the I/O APIC page. `Some(value)` for a read.
    pub fn mmio(&mut self, off: u32, write: Option<u32>) -> Option<u32> {
        match (off & 0xF0, write) {
            (IOREGSEL, Some(v)) => { self.ioregsel = v & 0xFF; None }
            (IOREGSEL, None) => Some(self.ioregsel),
            (IOWIN, Some(v)) => { self.write_indirect(v); None }
            (IOWIN, None) => Some(self.read_indirect()),
            (_, None) => Some(0),
            (_, Some(_)) => None,
        }
    }
}
