//! The IOMMU behind one confined device, whichever the firmware describes:
//! VT-d (DMAR, `vtd`) or AMD-Vi (IVRS, `amd_vi`) for a PCI function. Both
//! keep the same contract; every call is a no-op for a device that is not
//! protected. A device-tree device sits behind its bcm2712-iommu
//! (`bcm2712_iommu`).

use alloc::vec::Vec;
use crate::drivers::DevId;
#[cfg(target_arch = "x86_64")]
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
pub fn protect(dev: DevId) -> Result<(), &'static str> {
    match dev {
        #[cfg(target_arch = "x86_64")]
        DevId::Pci(dev) => {
            if crate::drivers::acpi::find_table(b"DMAR").is_some() {
                vtd::protect(dev)
            } else if amd_vi::present() {
                amd_vi::protect(dev)
            } else {
                Err("the firmware describes no IOMMU")
            }
        }
        #[cfg(target_arch = "aarch64")]
        DevId::Dt(base) => crate::drivers::bcm2712_iommu::protect(base),
    }
}

pub fn protected(dev: DevId) -> bool {
    match dev {
        #[cfg(target_arch = "x86_64")]
        DevId::Pci(dev) => vtd::protected(dev) || amd_vi::protected(dev),
        #[cfg(target_arch = "aarch64")]
        DevId::Dt(base) => crate::drivers::bcm2712_iommu::protected(base),
    }
}

pub fn grant(dev: DevId, phys: u64, pages: usize) {
    match dev {
        #[cfg(target_arch = "x86_64")]
        DevId::Pci(dev) => {
            vtd::grant(dev, phys, pages);
            amd_vi::grant(dev, phys, pages);
        }
        #[cfg(target_arch = "aarch64")]
        DevId::Dt(base) => {
            crate::drivers::bcm2712_iommu::grant(base, phys, pages);
            note_v3d(base, &[phys], pages, true);
        }
    }
}

pub fn grant_list(dev: DevId, list: &[u64]) {
    match dev {
        #[cfg(target_arch = "x86_64")]
        DevId::Pci(dev) => {
            vtd::grant_list(dev, list);
            amd_vi::grant_list(dev, list);
        }
        #[cfg(target_arch = "aarch64")]
        DevId::Dt(base) => {
            crate::drivers::bcm2712_iommu::grant_list(base, list);
            note_v3d(base, list, 1, true);
        }
    }
}

/// When it returns, the device no longer reaches the pages whose last
/// grant this was.
pub fn revoke(dev: DevId, phys: u64, pages: usize) {
    match dev {
        #[cfg(target_arch = "x86_64")]
        DevId::Pci(dev) => {
            vtd::revoke(dev, phys, pages);
            amd_vi::revoke(dev, phys, pages);
        }
        #[cfg(target_arch = "aarch64")]
        DevId::Dt(base) => {
            note_v3d(base, &[phys], pages, false);
            crate::drivers::bcm2712_iommu::revoke(base, phys, pages);
        }
    }
}

pub fn revoke_list(dev: DevId, list: &[u64]) {
    match dev {
        #[cfg(target_arch = "x86_64")]
        DevId::Pci(dev) => {
            vtd::revoke_list(dev, list);
            amd_vi::revoke_list(dev, list);
        }
        #[cfg(target_arch = "aarch64")]
        DevId::Dt(base) => {
            note_v3d(base, list, 1, false);
            crate::drivers::bcm2712_iommu::revoke_list(base, list);
        }
    }
}

/// The display device's grants also bound what its V3D may be given
/// (`pi5_v3d`), whether or not the HVS's IOMMU translates: a revoke takes
/// V3D's mappings of the pages away first.
#[cfg(target_arch = "aarch64")]
fn note_v3d(base: u64, starts: &[u64], pages: usize, grant: bool) {
    if !crate::platform::pi5_display_set::is_hvs(base) {
        return;
    }
    for &p in starts {
        crate::platform::pi5_v3d::note(p, pages, grant);
    }
}

pub fn poll(dev: DevId) {
    match dev {
        #[cfg(target_arch = "x86_64")]
        DevId::Pci(dev) => {
            vtd::poll(dev);
            if amd_vi::protected(dev) {
                amd_vi::poll();
            }
        }
        #[cfg(target_arch = "aarch64")]
        DevId::Dt(_) => {}
    }
}

pub fn take_faults(dev: DevId) -> Vec<Fault> {
    match dev {
        #[cfg(target_arch = "x86_64")]
        DevId::Pci(dev) => {
            let mut f = vtd::take_faults(dev);
            f.extend(amd_vi::take_faults(dev));
            f
        }
        #[cfg(target_arch = "aarch64")]
        DevId::Dt(_) => Vec::new(),
    }
}

/// What `gpu iommu` shows about the protected device.
pub struct Status {
    pub kind: &'static str,
    pub dev: DevId,
    pub pages: usize,
    pub faults_total: u64,
}

#[cfg(target_arch = "x86_64")]
pub fn status() -> Option<Status> {
    if let Some(s) = vtd::status() {
        return Some(Status { kind: "VT-d", dev: DevId::Pci(s.dev), pages: s.pages, faults_total: s.faults_total });
    }
    let s = amd_vi::status()?;
    let (dev, pages) = s.domain?;
    Some(Status { kind: "AMD-Vi", dev: DevId::Pci(dev), pages, faults_total: amd_vi::faults_total() })
}
