//! Register access as the drivers see it: a block and an offset, so the
//! same code runs on the board and against a recorded Linux run.

/// The register blocks the drivers touch.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Block {
    /// The PCIe root complex (`brcm,bcm2712-pcie`), config space included.
    Pcie,
    /// The SATA/PCIe resistor calibration (`brcm,bcm7216-pcie-sata-rescal`).
    Rescal,
    /// The software-init reset lines (`brcm,brcmstb-reset`).
    Reset,
    /// The MSI-X interrupt peripheral (`brcm,bcm2712-mip`).
    Mip,
    /// RP1's BAR0: the MSI-X table.
    Rp1Bar0,
    /// RP1's BAR1: its peripherals.
    Rp1Bar1,
    /// The VideoCore mailbox (`brcm,bcm2835-mbox`).
    Mbox,
}

/// MMIO and delays. Widths are 1, 2 or 4 bytes.
pub trait Hw {
    fn read(&mut self, b: Block, off: u64, width: u8) -> u32;
    fn write(&mut self, b: Block, off: u64, width: u8, v: u32);
    /// Sleep or spin for at least `us` microseconds.
    fn delay_us(&mut self, us: u64);

    fn r32(&mut self, b: Block, off: u64) -> u32 { self.read(b, off, 4) }
    fn w32(&mut self, b: Block, off: u64, v: u32) { self.write(b, off, 4, v) }
    fn r16(&mut self, b: Block, off: u64) -> u16 { self.read(b, off, 2) as u16 }
    fn w16(&mut self, b: Block, off: u64, v: u16) { self.write(b, off, 2, v as u32) }
    fn r8(&mut self, b: Block, off: u64) -> u8 { self.read(b, off, 1) as u8 }
    fn w8(&mut self, b: Block, off: u64, v: u8) { self.write(b, off, 1, v as u32) }
}

/// `u32p_replace_bits`: put `val` into the field `mask` of `*reg`.
pub fn replace_bits(reg: &mut u32, val: u32, mask: u32) {
    *reg = (*reg & !mask) | ((val << mask.trailing_zeros()) & mask);
}

/// `FIELD_GET`.
pub fn field_get(reg: u32, mask: u32) -> u32 {
    (reg & mask) >> mask.trailing_zeros()
}

/// `read_poll_timeout`: read until `done` or until `timeout_us` passed,
/// waiting `step_us` between reads; true when `done` held.
pub fn poll(hw: &mut impl Hw, b: Block, off: u64, step_us: u64, timeout_us: u64, done: impl Fn(u32) -> bool) -> (bool, u32) {
    let mut waited = 0;
    loop {
        let v = hw.r32(b, off);
        if done(v) { return (true, v); }
        if waited >= timeout_us {
            let v = hw.r32(b, off);
            return (done(v), v);
        }
        hw.delay_us(step_us);
        waited += step_us;
    }
}
