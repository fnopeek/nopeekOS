//! Minimal 8259 PIC stub.
//!
//! Linux probes the 8259 by writing to the master IMR (port 0x21) and
//! reading it back. With a NULL PIC, IRQs 0..15 get `dummy_irq_chip`
//! handlers, breaking `request_irq` / virtio-pci's INTx fallback.
//!
//! We don't emulate interrupt delivery — IRQs land in the guest via
//! direct VMCS/VMCB event-injection. The stub exists so probe + irq-
//! chip-init pass and `request_irq` returns 0.
//!
//! Plus we track ICW2 (vector base) for both master and slave so
//! callers can map an IRQ line to the actual interrupt vector Linux
//! programmed.

#![allow(dead_code)]

pub const PIC_MASTER_CMD: u16 = 0x20;
pub const PIC_MASTER_IMR: u16 = 0x21;
pub const PIC_SLAVE_CMD:  u16 = 0xA0;
pub const PIC_SLAVE_IMR:  u16 = 0xA1;

const ICW1_INIT: u8 = 0x10;

#[derive(Default)]
pub struct Pic8259 {
    master_imr: u8,
    slave_imr: u8,
    master_icw2: u8, // vector base for master IRQs 0..7
    slave_icw2: u8,  // vector base for slave  IRQs 8..15
    master_init_step: u8,
    slave_init_step: u8,
}

impl Pic8259 {
    pub const fn new() -> Self {
        Self {
            master_imr: 0xFF,
            slave_imr: 0xFF,
            // Reasonable BIOS-style defaults until ICW2 is observed.
            // Linux will overwrite with 0x20 / 0x28 during 8259 init.
            master_icw2: 0x20,
            slave_icw2: 0x28,
            master_init_step: 0,
            slave_init_step: 0,
        }
    }

    /// Return the interrupt vector that an IRQ line maps to.
    /// IRQ 0..7 = master, IRQ 8..15 = slave.
    pub fn vector_for_irq(&self, irq: u8) -> u8 {
        if irq < 8 {
            self.master_icw2 + irq
        } else {
            self.slave_icw2 + (irq - 8)
        }
    }
}

/// Dispatch an 8259-port PIO access. Returns `Some(value)` for IN
/// reads, `None` for OUT writes.
pub fn handle_pic_io(pic: &mut Pic8259, port: u16, dir_in: bool, val_out: u8) -> Option<u64> {
    match (port, dir_in) {
        // Command-port writes — ICW1 starts a 3- or 4-write init sequence.
        (PIC_MASTER_CMD, false) => {
            if val_out & ICW1_INIT != 0 { pic.master_init_step = 1; }
            None
        }
        (PIC_SLAVE_CMD, false) => {
            if val_out & ICW1_INIT != 0 { pic.slave_init_step = 1; }
            None
        }
        // Data-port writes during init = ICW2/3/4; otherwise OCW1 (mask).
        (PIC_MASTER_IMR, false) => {
            match pic.master_init_step {
                1 => { pic.master_icw2 = val_out & 0xF8; pic.master_init_step = 2; }
                2 => { pic.master_init_step = 3; }            // ICW3
                3 => { pic.master_init_step = 0; }            // ICW4
                _ => { pic.master_imr = val_out; }            // OCW1
            }
            None
        }
        (PIC_SLAVE_IMR, false) => {
            match pic.slave_init_step {
                1 => { pic.slave_icw2 = val_out & 0xF8; pic.slave_init_step = 2; }
                2 => { pic.slave_init_step = 3; }
                3 => { pic.slave_init_step = 0; }
                _ => { pic.slave_imr = val_out; }
            }
            None
        }
        // IMR readback — Linux's probe + each per-IRQ unmask reads back.
        (PIC_MASTER_IMR, true) => Some(pic.master_imr as u64),
        (PIC_SLAVE_IMR,  true) => Some(pic.slave_imr  as u64),
        // Command-port reads (default OCW3 selector) → IRR. We have no
        // pending IRQs in the PIC (we deliver via direct event-inject).
        (PIC_MASTER_CMD, true) | (PIC_SLAVE_CMD, true) => Some(0),
        _ => None,
    }
}
