//! The IOMMU behind one confined device, whichever the firmware describes:
//! VT-d (DMAR, `vtd`) or AMD-Vi (IVRS, `amd_vi`). Both keep the same
//! contract; every call is a no-op for a device that is not protected.

use alloc::vec::Vec;
use crate::drivers::pci::PciAddr;
use crate::drivers::{amd_vi, vtd};

/// A recorded DMA remapping fault.
#[derive(Clone, Copy)]
pub struct Fault {
    pub addr: u64,
    /// The requester ID.
    pub source: u16,
    /// VT-d: the fault reason; AMD-Vi: the event's flags.
    pub reason: u16,
    pub read: bool,
}

/// Put `dev` behind the IOMMU the firmware describes. Afterwards it
/// reaches the firmware's reserved ranges for it and what `grant` adds.
pub fn protect(dev: PciAddr) -> Result<(), &'static str> {
    if crate::drivers::acpi::find_table(b"DMAR").is_some() {
        vtd::protect(dev)
    } else if amd_vi::present() {
        amd_vi::protect(dev)
    } else {
        Err("the firmware describes no IOMMU")
    }
}

pub fn protected(dev: PciAddr) -> bool {
    vtd::protected(dev) || amd_vi::protected(dev)
}

pub fn grant(dev: PciAddr, phys: u64, pages: usize) {
    vtd::grant(dev, phys, pages);
    amd_vi::grant(dev, phys, pages);
}

pub fn grant_list(dev: PciAddr, list: &[u64]) {
    vtd::grant_list(dev, list);
    amd_vi::grant_list(dev, list);
}

/// When it returns, the device no longer reaches the pages whose last
/// grant this was.
pub fn revoke(dev: PciAddr, phys: u64, pages: usize) {
    vtd::revoke(dev, phys, pages);
    amd_vi::revoke(dev, phys, pages);
}

pub fn revoke_list(dev: PciAddr, list: &[u64]) {
    vtd::revoke_list(dev, list);
    amd_vi::revoke_list(dev, list);
}

pub fn poll(dev: PciAddr) {
    vtd::poll(dev);
    if amd_vi::protected(dev) {
        amd_vi::poll();
    }
}

pub fn take_faults(dev: PciAddr) -> Vec<Fault> {
    let mut f = vtd::take_faults(dev);
    f.extend(amd_vi::take_faults(dev));
    f
}

/// What `gpu iommu` shows about the protected device.
pub struct Status {
    pub kind: &'static str,
    pub dev: PciAddr,
    pub pages: usize,
    pub faults_total: u64,
}

pub fn status() -> Option<Status> {
    if let Some(s) = vtd::status() {
        return Some(Status { kind: "VT-d", dev: s.dev, pages: s.pages, faults_total: s.faults_total });
    }
    let s = amd_vi::status()?;
    let (dev, pages) = s.domain?;
    Some(Status { kind: "AMD-Vi", dev, pages, faults_total: amd_vi::faults_total() })
}
