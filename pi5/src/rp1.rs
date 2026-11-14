//! RP1, the I/O controller behind the PCIe link, after Linux
//! `drivers/mfd/rp1.c` (`rp1_probe`): BAR1 holds its peripherals, BAR0 the
//! MSI-X table; one vector per RP1 interrupt, each enabled per peripheral
//! in the `MSIX_CFG` registers when a driver requests it.

use crate::hw::{Block, Hw};
use crate::mip::Mip;
use crate::pci::{self, Device, MsixMsg, MsixTable};

pub const PCI_VENDOR_ID_RPI: u16 = 0x1de4;
pub const PCI_DEVICE_ID_RP1_C0: u16 = 0x0001;
pub const RP1_C0_CHIP_ID: u32 = 0x2000_1927;
const RP1_PLATFORM_FPGA: u32 = 1 << 0;
/// `RP1_INT_END`.
pub const RP1_IRQS: u32 = 61;

const RP1_SYSINFO_BASE: u64 = 0x00_0000;
const SYSINFO_CHIP_ID_OFFSET: u64 = 0x0;
const SYSINFO_PLATFORM_OFFSET: u64 = 0x4;
/// The MSIX_CFG registers, `RP1_PCIE_APBS_BASE`.
pub const RP1_PCIE_APBS_BASE: u64 = 0x10_8000;
const REG_SET: u64 = 0x800;
const REG_CLR: u64 = 0xc00;
fn msix_cfg(hwirq: u32) -> u64 { 0x8 + 4 * hwirq as u64 }
const MSIX_CFG_IACK_EN: u32 = 1 << 3;
const MSIX_CFG_IACK: u32 = 1 << 2;
const MSIX_CFG_ENABLE: u32 = 1 << 0;
/// Raw interrupt status of the 61 lines, `INTSTATL` and `INTSTATH`.
const INTSTATL: u64 = 0x108;
const INTSTATH: u64 = 0x10c;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// BAR1 too small: RP1's firmware did not run.
    NotInitialised(u64),
    Msix(pci::MsixError),
    WrongChip(u32),
}

/// A probed RP1.
#[derive(Clone, Copy, Debug)]
pub struct Rp1 {
    pub chip_id: u32,
    pub fpga: bool,
    /// One vector per RP1 interrupt line.
    pub msix: MsixTable,
}

/// `rp1_probe` after the PCI core enabled `dev`: MSI-X vectors through the
/// MIP, chip ID check.
pub fn probe(hw: &mut impl Hw, dev: &Device, mip: &Mip) -> Result<Rp1, Error> {
    if dev.bars[1].size <= 0x10000 {
        return Err(Error::NotInitialised(dev.bars[1].size));
    }
    let msix = pci::msix_enable(hw, RP1_IRQS, |i| {
        let m = mip.msi(i).unwrap_or(crate::mip::Msi { addr: 0, data: 0, spi: 0 });
        MsixMsg { addr: m.addr, data: m.data }
    }, |bir| match bir {
        0 => Some(Block::Rp1Bar0),
        1 => Some(Block::Rp1Bar1),
        _ => None,
    }).map_err(Error::Msix)?;
    let chip_id = hw.r32(Block::Rp1Bar1, RP1_SYSINFO_BASE + SYSINFO_CHIP_ID_OFFSET);
    let platform = hw.r32(Block::Rp1Bar1, RP1_SYSINFO_BASE + SYSINFO_PLATFORM_OFFSET);
    if chip_id != RP1_C0_CHIP_ID {
        return Err(Error::WrongChip(chip_id));
    }
    Ok(Rp1 { chip_id, fpga: platform & RP1_PLATFORM_FPGA != 0, msix })
}

/// Interrupt trigger of an RP1 line, `rp1_irq_set_type`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    Level,
    Edge,
}

impl Rp1 {
    /// `rp1_irq_set_type` then `rp1_irq_activate`: let RP1 interrupt
    /// `hwirq` send its MSI-X message.
    pub fn irq_enable(&self, hw: &mut impl Hw, hwirq: u32, trigger: Trigger) {
        let base = RP1_PCIE_APBS_BASE;
        match trigger {
            Trigger::Level => hw.w32(Block::Rp1Bar1, base + REG_SET + msix_cfg(hwirq), MSIX_CFG_IACK_EN),
            Trigger::Edge => hw.w32(Block::Rp1Bar1, base + REG_CLR + msix_cfg(hwirq), MSIX_CFG_IACK_EN),
        }
        hw.w32(Block::Rp1Bar1, base + REG_SET + msix_cfg(hwirq), MSIX_CFG_ENABLE);
    }

    /// `rp1_chained_handle_irq` after the handler: a level interrupt is
    /// acknowledged so RP1 can send it again.
    pub fn irq_ack(&self, hw: &mut impl Hw, hwirq: u32) {
        hw.w32(Block::Rp1Bar1, RP1_PCIE_APBS_BASE + REG_SET + msix_cfg(hwirq), MSIX_CFG_IACK);
    }

    /// Line `hwirq`'s raw status, its `MSIX_CFG` word and its MSI-X
    /// vector control word, for a report.
    pub fn irq_state(&self, hw: &mut impl Hw, hwirq: u32) -> (bool, u32, u32) {
        let reg = if hwirq < 32 { INTSTATL } else { INTSTATH };
        let raw = hw.r32(Block::Rp1Bar1, RP1_PCIE_APBS_BASE + reg) >> (hwirq % 32) & 1 != 0;
        let cfg = hw.r32(Block::Rp1Bar1, RP1_PCIE_APBS_BASE + msix_cfg(hwirq));
        (raw, cfg, self.msix.vector_ctrl(hw, hwirq))
    }

    /// `rp1_mask_irq`: the line's MSI-X vector masked (`pci_msi_mask_irq`).
    pub fn irq_mask(&self, hw: &mut impl Hw, hwirq: u32) {
        self.msix.mask(hw, hwirq);
    }

    /// `rp1_unmask_irq`.
    pub fn irq_unmask(&self, hw: &mut impl Hw, hwirq: u32) {
        self.msix.unmask(hw, hwirq);
    }
}
