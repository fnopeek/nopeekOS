//! Dual 8259A PIC — ported from Linux `arch/x86/kvm/i8259.c`.
//!
//! IRR / ISR / IMR per chip, fixed + rotating priority, specific and
//! non-specific EOI, auto-EOI, edge/level via ELCR, the cascade on master
//! line 2, OCW3 register select and poll mode. The CPU side is
//! `read_irq` (`kvm_pic_read_irq`: acknowledge the highest request, move it
//! to ISR, return its vector) and `output` (the INTR pin, `pic_irq_request`).
//!
//! The stub this replaces tracked only the mask and the vector base; device
//! lines were injected straight into the guest past a mask or a running
//! handler, and the guest's EOI went nowhere.

pub const PIC_MASTER_CMD: u16 = 0x20;
pub const PIC_MASTER_IMR: u16 = 0x21;
pub const PIC_SLAVE_CMD: u16 = 0xA0;
pub const PIC_SLAVE_IMR: u16 = 0xA1;
pub const PIC_ELCR_MASTER: u16 = 0x4D0;
pub const PIC_ELCR_SLAVE: u16 = 0x4D1;

/// `struct kvm_kpic_state`.
#[derive(Clone, Copy, Default)]
struct Kpic {
    last_irr: u8,
    irr: u8,
    imr: u8,
    isr: u8,
    priority_add: u8,
    irq_base: u8,
    read_reg_select: bool,
    poll: bool,
    special_mask: bool,
    init_state: u8,
    auto_eoi: bool,
    rotate_on_auto_eoi: bool,
    special_fully_nested_mode: bool,
    init4: bool,
    elcr: u8,
    elcr_mask: u8,
}

impl Kpic {
    /// `pic_set_irq1`: edge sets IRR on a rising edge, level follows the line.
    fn set_irq1(&mut self, irq: u8, level: bool) {
        let mask = 1u8 << irq;
        if self.elcr & mask != 0 {
            if level {
                self.irr |= mask;
                self.last_irr |= mask;
            } else {
                self.irr &= !mask;
                self.last_irr &= !mask;
            }
        } else if level {
            if self.last_irr & mask == 0 {
                self.irr |= mask;
            }
            self.last_irr |= mask;
        } else {
            self.last_irr &= !mask;
        }
    }

    /// `get_priority`: highest priority in `mask` (0 = highest), 8 if none.
    fn get_priority(&self, mask: u8) -> u8 {
        if mask == 0 { return 8; }
        let mut p = 0u8;
        while mask & (1 << ((p + self.priority_add) & 7)) == 0 { p += 1; }
        p
    }

    /// `pic_get_irq`: the line this chip wants to raise, if any.
    fn get_irq(&self, is_master: bool) -> Option<u8> {
        let priority = self.get_priority(self.irr & !self.imr);
        if priority == 8 { return None; }
        let mut isr = self.isr;
        if self.special_fully_nested_mode && is_master { isr &= !(1 << 2); }
        if priority < self.get_priority(isr) {
            Some((priority + self.priority_add) & 7)
        } else {
            None
        }
    }

    /// `pic_intack`.
    fn intack(&mut self, irq: u8) {
        self.isr |= 1 << irq;
        // A level-sensitive request is not cleared here.
        if self.elcr & (1 << irq) == 0 { self.irr &= !(1 << irq); }
        if self.auto_eoi {
            if self.rotate_on_auto_eoi { self.priority_add = (irq + 1) & 7; }
            self.isr &= !(1 << irq);
        }
    }

    /// `kvm_pic_reset` (ICW1).
    fn reset(&mut self) {
        let edge_irr = self.irr & !self.elcr;
        self.last_irr = 0;
        self.irr &= self.elcr;
        self.imr = 0;
        self.priority_add = 0;
        self.special_mask = false;
        self.read_reg_select = false;
        if !self.init4 {
            self.special_fully_nested_mode = false;
            self.auto_eoi = false;
        }
        self.init_state = 1;
        // The PIC feeds the LAPIC in virtual-wire mode, so pending edges are
        // dropped from ISR as KVM does when a vCPU accepts PIC interrupts.
        self.isr &= !edge_irr;
    }
}

pub struct Pic8259 {
    pics: [Kpic; 2],
    /// The I/O APIC on the same lines (KVM routes GSI 0-15 to both chips).
    pub ioapic: super::ioapic::Ioapic,
}

impl Default for Pic8259 {
    fn default() -> Self { Self::new() }
}

impl Pic8259 {
    pub const fn new() -> Self {
        const K: Kpic = Kpic {
            last_irr: 0, irr: 0, imr: 0xFF, isr: 0, priority_add: 0, irq_base: 0,
            read_reg_select: false, poll: false, special_mask: false, init_state: 0,
            auto_eoi: false, rotate_on_auto_eoi: false, special_fully_nested_mode: false,
            init4: false, elcr: 0, elcr_mask: 0,
        };
        let mut pics = [K, K];
        // `kvm_pic_init`: ELCR bits that may be set on each chip. Vector
        // bases default to the BIOS layout until the guest writes ICW2.
        pics[0].elcr_mask = 0xF8;
        pics[1].elcr_mask = 0xDE;
        pics[0].irq_base = 0x08;
        pics[1].irq_base = 0x70;
        Pic8259 { pics, ioapic: super::ioapic::Ioapic::new() }
    }

    /// `pic_update_irq`: propagate the slave's request through the cascade.
    fn update(&mut self) {
        if self.pics[1].get_irq(false).is_some() {
            self.pics[0].set_irq1(2, true);
            self.pics[0].set_irq1(2, false);
        }
    }

    /// `kvm_pic_set_irq`: drive line `irq` (0..15) to `level`.
    pub fn set_irq(&mut self, irq: u8, level: bool) {
        if irq >= 16 { return; }
        self.pics[(irq >> 3) as usize].set_irq1(irq & 7, level);
        self.update();
    }

    /// A device event on `irq`: a rising and a falling edge. Our devices
    /// signal completions, not held levels; on an edge-triggered line (the
    /// guest's setting — no PIRQ router, no ELCR writes) that is exact, and a
    /// pulse while the previous one is still in service stays latched in IRR.
    pub fn pulse(&mut self, irq: u8) {
        self.set_irq(irq, true);
        self.set_irq(irq, false);
        self.ioapic.pulse(
            super::ioapic::isa_pin(irq),
            crate::microvm::cpu::svm::lapic::vcpu_on_this_core(),
        );
    }

    /// The guest has masked `irq` at the 8259 — with an I/O APIC it masks all
    /// sixteen, and the line is then served there.
    pub fn masked(&self, irq: u8) -> bool {
        self.pics[(irq >> 3) as usize].imr & (1 << (irq & 7)) != 0
    }

    /// The INTR pin (`s->output`): does the master want to interrupt the CPU?
    pub fn output(&self) -> bool {
        self.pics[0].get_irq(true).is_some()
    }

    /// `kvm_pic_read_irq`: acknowledge (INTA) and return the vector.
    pub fn read_irq(&mut self) -> u8 {
        let intno = match self.pics[0].get_irq(true) {
            Some(irq) => {
                self.pics[0].intack(irq);
                if irq == 2 {
                    let irq2 = match self.pics[1].get_irq(false) {
                        Some(i) => { self.pics[1].intack(i); i }
                        None => 7, // spurious IRQ on the slave
                    };
                    self.pics[1].irq_base.wrapping_add(irq2)
                } else {
                    self.pics[0].irq_base.wrapping_add(irq)
                }
            }
            None => self.pics[0].irq_base.wrapping_add(7), // spurious on master
        };
        self.update();
        intno
    }

    /// Nothing requested or in service on `irq` — the previous edge was taken
    /// and acknowledged.
    pub fn line_idle(&self, irq: u8) -> bool {
        let s = &self.pics[(irq >> 3) as usize];
        let m = 1u8 << (irq & 7);
        s.irr & m == 0 && s.isr & m == 0
    }

    /// `pic_ioport_write`.
    fn write_cmd_or_data(&mut self, chip: usize, addr: u16, val: u8) {
        let is_master = chip == 0;
        if addr & 1 == 0 {
            let s = &mut self.pics[chip];
            if val & 0x10 != 0 {
                s.init4 = val & 1 != 0;
                s.reset();
            } else if val & 0x08 != 0 {
                if val & 0x04 != 0 { s.poll = true; }
                if val & 0x02 != 0 { s.read_reg_select = val & 1 != 0; }
                if val & 0x40 != 0 { s.special_mask = (val >> 5) & 1 != 0; }
            } else {
                let cmd = val >> 5;
                match cmd {
                    0 | 4 => s.rotate_on_auto_eoi = cmd >> 2 != 0,
                    1 | 5 => {
                        let priority = s.get_priority(s.isr);
                        if priority != 8 {
                            let irq = (priority + s.priority_add) & 7;
                            if cmd == 5 { s.priority_add = (irq + 1) & 7; }
                            s.isr &= !(1 << irq);
                            self.update();
                        }
                    }
                    3 => {
                        s.isr &= !(1 << (val & 7));
                        self.update();
                    }
                    6 => {
                        s.priority_add = (val + 1) & 7;
                        self.update();
                    }
                    7 => {
                        let irq = val & 7;
                        s.priority_add = (irq + 1) & 7;
                        s.isr &= !(1 << irq);
                        self.update();
                    }
                    _ => {}
                }
            }
        } else {
            let s = &mut self.pics[chip];
            match s.init_state {
                0 => {
                    s.imr = val;
                    self.update();
                }
                1 => {
                    s.irq_base = val & 0xF8;
                    s.init_state = 2;
                }
                2 => s.init_state = if s.init4 { 3 } else { 0 },
                3 => {
                    s.special_fully_nested_mode = (val >> 4) & 1 != 0;
                    s.auto_eoi = (val >> 1) & 1 != 0;
                    s.init_state = 0;
                }
                _ => {}
            }
        }
        let _ = is_master;
    }

    /// `pic_poll_read`.
    fn poll_read(&mut self, chip: usize, port: u16) -> u8 {
        match self.pics[chip].get_irq(chip == 0) {
            Some(irq) => {
                if port >> 7 != 0 {
                    self.pics[0].isr &= !(1 << 2);
                    self.pics[0].irr &= !(1 << 2);
                }
                let s = &mut self.pics[chip];
                s.irr &= !(1 << irq);
                s.isr &= !(1 << irq);
                if port >> 7 != 0 || irq != 2 { self.update(); }
                irq | 0x80
            }
            None => {
                self.update();
                0x07
            }
        }
    }

    /// `pic_ioport_read`.
    fn read_cmd_or_data(&mut self, chip: usize, port: u16) -> u8 {
        if self.pics[chip].poll {
            let r = self.poll_read(chip, port);
            self.pics[chip].poll = false;
            r
        } else if port & 1 == 0 {
            let s = &self.pics[chip];
            if s.read_reg_select { s.isr } else { s.irr }
        } else {
            self.pics[chip].imr
        }
    }

    /// Port I/O on 0x20/0x21, 0xA0/0xA1 and the ELCR pair 0x4D0/0x4D1.
    /// `Some(value)` for reads, `None` for writes or foreign ports.
    pub fn ioport(&mut self, port: u16, dir_in: bool, val: u8) -> Option<u64> {
        let chip = match port {
            PIC_MASTER_CMD | PIC_MASTER_IMR => 0,
            PIC_SLAVE_CMD | PIC_SLAVE_IMR => 1,
            PIC_ELCR_MASTER | PIC_ELCR_SLAVE => {
                let s = &mut self.pics[(port & 1) as usize];
                if dir_in { return Some(s.elcr as u64); }
                s.elcr = val & s.elcr_mask;
                return None;
            }
            _ => return None,
        };
        if dir_in {
            Some(self.read_cmd_or_data(chip, port) as u64)
        } else {
            self.write_cmd_or_data(chip, port, val);
            None
        }
    }
}
