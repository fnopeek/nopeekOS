//! Which device a driver module or a kernel service speaks of: a PCI
//! function, or a node of the firmware's device tree, named by the
//! physical address of its first register window.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DevId {
    #[cfg(target_arch = "x86_64")]
    Pci(crate::drivers::pci::PciAddr),
    /// A device-tree node: the base of its first `reg` window.
    #[cfg(target_arch = "aarch64")]
    Dt(u64),
}

impl DevId {
    /// The PCI function, when the device is one.
    #[cfg(target_arch = "x86_64")]
    pub fn pci(self) -> Option<crate::drivers::pci::PciAddr> {
        match self {
            DevId::Pci(a) => Some(a),
        }
    }
}

impl core::fmt::Display for DevId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            #[cfg(target_arch = "x86_64")]
            DevId::Pci(a) => write!(f, "{:02x}:{:02x}.{}", a.bus, a.device, a.function),
            #[cfg(target_arch = "aarch64")]
            DevId::Dt(base) => write!(f, "@{:x}", base),
        }
    }
}
