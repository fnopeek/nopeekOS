//! pi5_core: the Raspberry Pi 5's way to its I/O controller, after Linux.
//!
//! The BCM2712 reaches RP1 (USB, Ethernet, GPIO) over PCIe: the root
//! complex (`pcie`) with its resets (`reset`), the MSI-X interrupt
//! peripheral that turns RP1's messages into GIC interrupts (`mip`), the
//! PCI core's share for one root port and one device (`pci`), and RP1
//! itself (`rp1`). Behind RP1: its USB controllers (`dwc3`) and its
//! Ethernet (`eth`: clocks `clk_rp1`, the PHY reset line `gpio_rp1`, the MAC
//! `gem`, the PHY `phy`). Every register access goes through [`hw::Hw`], so the
//! same code runs on the board and against a recorded Linux run.

#![cfg_attr(not(test), no_std)]

pub mod clk_rp1;
pub mod dwc3;
pub mod eth;
pub mod gem;
pub mod gpio_rp1;
pub mod hw;
pub mod mip;
pub mod pci;
pub mod pcie;
pub mod phy;
pub mod reset;
pub mod rp1;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_eth;

/// How far bring-up came.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Pcie(pcie::Error),
    /// Link up, but nothing answers on bus 1.
    NoDevice,
    /// Something other than RP1 on bus 1.
    NotRp1(u16, u16),
    Rp1(rp1::Error),
}

/// Everything bring-up found.
pub struct Rp1Link {
    pub pcie: pcie::Pcie,
    pub dev: pci::Device,
    pub rp1: rp1::Rp1,
}

/// The order Linux probes in: MIP, then the root complex, whose bus scan
/// starts the link, then RP1. `window_bus` is the PCI address of the first
/// outbound window, where the device's BARs go.
pub fn bring_up(hw: &mut impl hw::Hw, cfg: &pcie::Config, mip: &mip::Mip, window_bus: u64) -> Result<Rp1Link, Error> {
    mip.init(hw);
    let mut pcie = pcie::probe(hw, cfg).map_err(Error::Pcie)?;
    pci::root_port_init(hw);
    pcie.start_link(hw).map_err(Error::Pcie)?;
    let dev = pci::scan(hw, window_bus).ok_or(Error::NoDevice)?;
    if (dev.vendor, dev.device) != (rp1::PCI_VENDOR_ID_RPI, rp1::PCI_DEVICE_ID_RP1_C0) {
        return Err(Error::NotRp1(dev.vendor, dev.device));
    }
    let rp1 = rp1::probe(hw, &dev, mip).map_err(Error::Rp1)?;
    Ok(Rp1Link { pcie, dev, rp1 })
}
