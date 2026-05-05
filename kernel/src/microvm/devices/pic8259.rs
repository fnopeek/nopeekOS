//! Minimal 8259 PIC stub.
//!
//! Linux probes the 8259 by writing a value to the master IMR (port
//! 0x21) and reading it back. If the readback differs, it falls back
//! to `null_legacy_pic` — leaving IRQs 0..15 with `dummy_irq_chip`
//! handlers. `request_irq` on those then fails with -EINVAL, which
//! breaks `virtio-pci`'s INTx fallback path.
//!
//! We don't emulate the 8259's interrupt delivery logic (IRR/ISR,
//! cascade). For Phase 12.2 IRQs land in the guest via direct
//! VMCS/VMCB event-injection; the 8259 stub only exists so Linux's
//! probe + irq-chip-init succeeds and `request_irq` returns 0.
//!
//! Surface:
//!   * 0x20  (master command) — write: accept ICW1/OCW2/OCW3 silently.
//!                              read:  return 0 (no IRQs pending).
//!   * 0x21  (master IMR/ICW)  — write: store. read: return stored.
//!   * 0xA0  (slave  command) — write: accept silently. read: 0.
//!   * 0xA1  (slave  IMR/ICW)  — write: store. read: return stored.

#![allow(dead_code)]

pub const PIC_MASTER_CMD: u16 = 0x20;
pub const PIC_MASTER_IMR: u16 = 0x21;
pub const PIC_SLAVE_CMD:  u16 = 0xA0;
pub const PIC_SLAVE_IMR:  u16 = 0xA1;

pub struct Pic8259 {
    /// Master IMR (last value written to port 0x21 in non-ICW state).
    /// On reset Linux writes 0xFF (mask everything), then ICW1..ICW4
    /// during init, then OCW1 (final IMR). Tracking the latest write
    /// gives correct readback for both the boot-time probe and the
    /// per-IRQ unmask path.
    master_imr: u8,
    slave_imr: u8,
}

impl Pic8259 {
    pub const fn new() -> Self {
        Self {
            master_imr: 0xFF,
            slave_imr: 0xFF,
        }
    }
}

/// Dispatch an 8259-port PIO access. Returns `Some(value)` for IN
/// reads, `None` for OUT writes. Caller already restricted the port
/// to 0x20 / 0x21 / 0xA0 / 0xA1.
pub fn handle_pic_io(pic: &mut Pic8259, port: u16, dir_in: bool, val_out: u8) -> Option<u64> {
    match (port, dir_in) {
        (PIC_MASTER_IMR, false) => { pic.master_imr = val_out; None }
        (PIC_SLAVE_IMR,  false) => { pic.slave_imr  = val_out; None }
        (PIC_MASTER_IMR, true)  => Some(pic.master_imr as u64),
        (PIC_SLAVE_IMR,  true)  => Some(pic.slave_imr  as u64),
        // Command-port writes (ICW1, OCW2 EOI, OCW3) — accept silently.
        (PIC_MASTER_CMD, false) | (PIC_SLAVE_CMD, false) => None,
        // Command-port reads (default selector in OCW3 returns IRR).
        // Return zero — no IRQs pending in our pretend PIC.
        (PIC_MASTER_CMD, true) | (PIC_SLAVE_CMD, true) => Some(0),
        _ => None,
    }
}
