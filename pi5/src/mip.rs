//! The MSI-X interrupt peripheral, after Linux
//! `drivers/irqchip/irq-bcm2712-mip.c`: a PCIe write of `n` to its message
//! address raises GIC SPI `msi_base + msi_offset + n`, edge-triggered.

use crate::hw::{Block, Hw};

const MIP_INT_CFGL_HOST: u64 = 0x20;
const MIP_INT_CFGH_HOST: u64 = 0x30;
const MIP_INT_MASKL_HOST: u64 = 0x40;
const MIP_INT_MASKH_HOST: u64 = 0x50;
const MIP_INT_MASKL_VPU: u64 = 0x60;
const MIP_INT_MASKH_VPU: u64 = 0x70;

/// What `mip_parse_dt` reads.
#[derive(Clone, Copy, Debug)]
pub struct Mip {
    /// `brcm,msi-offset`.
    pub msi_offset: u32,
    /// The first GIC SPI of `msi-ranges`.
    pub msi_base: u32,
    /// The vector count of `msi-ranges`.
    pub num_msis: u32,
    /// The second `reg` entry, untranslated: the PCI bus address devices
    /// write their messages to.
    pub msg_addr: u64,
}

/// One allocated vector: the message a device writes, the GIC SPI it raises.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Msi {
    pub addr: u64,
    pub data: u32,
    pub spi: u32,
}

impl Mip {
    /// `mip_init_domains`: every vector unmasked for the host, masked for
    /// the VPU, edge-triggered.
    pub fn init(&self, hw: &mut impl Hw) {
        hw.w32(Block::Mip, MIP_INT_MASKL_HOST, 0);
        hw.w32(Block::Mip, MIP_INT_MASKH_HOST, 0);
        hw.w32(Block::Mip, MIP_INT_MASKL_VPU, !0);
        hw.w32(Block::Mip, MIP_INT_MASKH_VPU, !0);
        hw.w32(Block::Mip, MIP_INT_CFGL_HOST, !0);
        hw.w32(Block::Mip, MIP_INT_CFGH_HOST, !0);
    }

    /// `mip_middle_domain_alloc` + `mip_compose_msi_msg` for vector `irq`
    /// of the bitmap (Linux hands them out lowest first).
    pub fn msi(&self, irq: u32) -> Option<Msi> {
        if irq >= self.num_msis { return None; }
        let hwirq = irq + self.msi_offset;
        Some(Msi { addr: self.msg_addr, data: hwirq, spi: hwirq + self.msi_base })
    }
}
