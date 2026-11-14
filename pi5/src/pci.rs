//! What the Linux PCI core does for the one topology behind the BCM2712
//! root complex, a root port with one endpoint on bus 1: bus numbers,
//! BAR sizing and assignment from the first outbound window, the bridge
//! window, decode and bus mastering, MSI-X (`drivers/pci/probe.c`,
//! `setup-bus.c`, `setup-res.c`, `msi/msi.c`).
//!
//! Not implemented: ASPM (the link stays in L0), I/O and prefetchable
//! windows (disabled), more than one device or function.

use crate::hw::{Block, Hw};
use crate::pcie::{cfg_read, cfg_write};

pub const PCI_COMMAND: u16 = 0x04;
pub const PCI_COMMAND_MEMORY: u16 = 0x2;
pub const PCI_COMMAND_MASTER: u16 = 0x4;
pub const PCI_COMMAND_INTX_DISABLE: u16 = 0x400;
const PCI_STATUS: u16 = 0x06;
const PCI_STATUS_CAP_LIST: u16 = 0x10;
const PCI_BASE_ADDRESS_0: u16 = 0x10;
const PCI_PRIMARY_BUS: u16 = 0x18;
const PCI_SUBORDINATE_BUS: u16 = 0x1a;
const PCI_IO_BASE: u16 = 0x1c;
const PCI_SEC_STATUS: u16 = 0x1e;
const PCI_MEMORY_BASE: u16 = 0x20;
const PCI_PREF_MEMORY_BASE: u16 = 0x24;
const PCI_PREF_BASE_UPPER32: u16 = 0x28;
const PCI_PREF_LIMIT_UPPER32: u16 = 0x2c;
const PCI_IO_BASE_UPPER16: u16 = 0x30;
const PCI_CAPABILITY_LIST: u16 = 0x34;
const PCI_BRIDGE_CONTROL: u16 = 0x3e;
const PCI_BRIDGE_CTL_SERR: u16 = 0x02;

const PCI_CAP_ID_PM: u8 = 0x01;
const PCI_PM_CTRL: u16 = 4;
const PCI_PM_CTRL_PME_STATUS: u16 = 0x8000;
const PCI_CAP_ID_EXP: u8 = 0x10;
const PCI_EXP_RTCTL: u16 = 0x1c;
const PCI_EXP_RTCTL_CRSSVE: u16 = 0x10;
const PCI_EXP_RTCAP: u16 = 0x1e;
const PCI_EXP_RTCAP_CRSVIS: u16 = 0x1;
pub const PCI_CAP_ID_MSIX: u8 = 0x11;
const PCI_MSIX_FLAGS: u16 = 2;
const PCI_MSIX_FLAGS_QSIZE: u16 = 0x07ff;
const PCI_MSIX_FLAGS_MASKALL: u16 = 0x4000;
const PCI_MSIX_FLAGS_ENABLE: u16 = 0x8000;
const PCI_MSIX_TABLE: u16 = 4;
const PCI_MSIX_TABLE_BIR: u32 = 0x7;
const PCI_MSIX_ENTRY_SIZE: u64 = 16;
const PCI_MSIX_ENTRY_LOWER_ADDR: u64 = 0;
const PCI_MSIX_ENTRY_UPPER_ADDR: u64 = 4;
const PCI_MSIX_ENTRY_DATA: u64 = 8;
const PCI_MSIX_ENTRY_VECTOR_CTRL: u64 = 12;
const PCI_MSIX_ENTRY_CTRL_MASKBIT: u32 = 0x1;

const RP: (u8, u8) = (0, 0);
const DEV_BUS: u8 = 1;

fn r8(hw: &mut impl Hw, (b, d): (u8, u8), w: u16) -> u8 { cfg_read(hw, b, d, w, 1) as u8 }
fn r16(hw: &mut impl Hw, (b, d): (u8, u8), w: u16) -> u16 { cfg_read(hw, b, d, w, 2) as u16 }
fn r32(hw: &mut impl Hw, (b, d): (u8, u8), w: u16) -> u32 { cfg_read(hw, b, d, w, 4) }
fn w8(hw: &mut impl Hw, (b, d): (u8, u8), w: u16, v: u8) { cfg_write(hw, b, d, w, 1, v as u32) }
fn w16(hw: &mut impl Hw, (b, d): (u8, u8), w: u16, v: u16) { cfg_write(hw, b, d, w, 2, v as u32) }
fn w32(hw: &mut impl Hw, (b, d): (u8, u8), w: u16, v: u32) { cfg_write(hw, b, d, w, 4, v) }

/// `pci_find_capability`.
pub fn find_cap(hw: &mut impl Hw, dev: (u8, u8), id: u8) -> Option<u16> {
    if r16(hw, dev, PCI_STATUS) & PCI_STATUS_CAP_LIST == 0 { return None; }
    let mut pos = r8(hw, dev, PCI_CAPABILITY_LIST) as u16;
    for _ in 0..48 {
        if pos < 0x40 { return None; }
        pos &= !3;
        let ent = r16(hw, dev, pos);
        if ent & 0xff == 0xff { return None; }
        if ent as u8 == id { return Some(pos); }
        pos = ent >> 8;
    }
    None
}

/// The root port before the link comes up: SERR forwarding, PME status
/// clear, bus numbers reset, status clear, CRS software visibility.
pub fn root_port_init(hw: &mut impl Hw) {
    let ctl = r16(hw, RP, PCI_BRIDGE_CONTROL);
    w16(hw, RP, PCI_BRIDGE_CONTROL, ctl | PCI_BRIDGE_CTL_SERR);
    if let Some(pm) = find_cap(hw, RP, PCI_CAP_ID_PM) {
        let c = r16(hw, RP, pm + PCI_PM_CTRL);
        w16(hw, RP, pm + PCI_PM_CTRL, c | PCI_PM_CTRL_PME_STATUS);
    }
    // pci_scan_bridge_extend: "bridge configuration invalid", start over.
    w32(hw, RP, PCI_PRIMARY_BUS, 0);
    w16(hw, RP, PCI_SEC_STATUS, 0xffff);
    w16(hw, RP, PCI_STATUS, 0xffff);
    if let Some(exp) = find_cap(hw, RP, PCI_CAP_ID_EXP)
        && r16(hw, RP, exp + PCI_EXP_RTCAP) & PCI_EXP_RTCAP_CRSVIS != 0
    {
        let c = r16(hw, RP, exp + PCI_EXP_RTCTL);
        w16(hw, RP, exp + PCI_EXP_RTCTL, c | PCI_EXP_RTCTL_CRSSVE);
    }
    // Primary 0, secondary 1, subordinate 0xff while bus 1 is scanned.
    w32(hw, RP, PCI_PRIMARY_BUS, 0x00ff_0100);
}

/// A memory BAR as sized and placed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Bar {
    pub size: u64,
    /// PCI bus address.
    pub bus: u64,
    pub is_64: bool,
}

/// The endpoint on bus 1.
#[derive(Clone, Copy, Debug)]
pub struct Device {
    pub vendor: u16,
    pub device: u16,
    pub class: u32,
    pub bars: [Bar; 6],
}

impl Device {
    pub const ADDR: (u8, u8) = (DEV_BUS, 0);
}

/// `__pci_read_base` for the six BARs of `dev`; I/O BARs are skipped.
fn size_bars(hw: &mut impl Hw, dev: (u8, u8)) -> [Bar; 6] {
    let mut bars = [Bar::default(); 6];
    let mut i = 0;
    while i < 6 {
        let reg = PCI_BASE_ADDRESS_0 + 4 * i as u16;
        let orig = r32(hw, dev, reg);
        w32(hw, dev, reg, !0);
        let sz = r32(hw, dev, reg);
        w32(hw, dev, reg, orig);
        if orig & 1 != 0 {
            i += 1;
            continue;
        }
        let is_64 = orig & 0x6 == 0x4;
        let mut mask = (sz & !0xf) as u64;
        if is_64 && i < 5 {
            let reg_hi = reg + 4;
            let orig_hi = r32(hw, dev, reg_hi);
            w32(hw, dev, reg_hi, !0);
            let sz_hi = r32(hw, dev, reg_hi);
            w32(hw, dev, reg_hi, orig_hi);
            mask |= (sz_hi as u64) << 32;
        } else if mask != 0 {
            mask |= 0xffff_ffff_0000_0000;
        }
        if mask != 0 {
            bars[i] = Bar { size: (!mask).wrapping_add(1), bus: 0, is_64 };
        }
        i += if is_64 { 2 } else { 1 };
    }
    bars
}

/// Scan bus 1 behind the root port and place the endpoint's BARs from bus
/// address `window_bus` on, largest first, each aligned to its size; then
/// open the bridge's memory window over them and enable both functions.
/// Returns None when nothing answers on bus 1.
pub fn scan(hw: &mut impl Hw, window_bus: u64) -> Option<Device> {
    let id = r32(hw, Device::ADDR, 0);
    if id == !0 || id == 0 { return None; }
    let class = r32(hw, Device::ADDR, 0x08) >> 8;
    let mut bars = size_bars(hw, Device::ADDR);

    // Subordinate bus: only bus 1 sits behind the root port.
    w8(hw, RP, PCI_SUBORDINATE_BUS, DEV_BUS);
    w16(hw, RP, PCI_SEC_STATUS, 0xffff);

    // pbus_size_mem + __assign_resources_sorted: descending size.
    let mut order = [0usize, 1, 2, 3, 4, 5];
    for i in 1..order.len() {
        let mut j = i;
        while j > 0 && bars[order[j - 1]].size < bars[order[j]].size {
            order.swap(j - 1, j);
            j -= 1;
        }
    }
    let mut at = window_bus;
    for &i in &order {
        let b = &mut bars[i];
        if b.size == 0 { continue; }
        at = at.next_multiple_of(b.size);
        b.bus = at;
        at += b.size;
        let reg = PCI_BASE_ADDRESS_0 + 4 * i as u16;
        w32(hw, Device::ADDR, reg, b.bus as u32);
        if b.is_64 {
            w32(hw, Device::ADDR, reg + 4, (b.bus >> 32) as u32);
        }
    }
    let end = at.next_multiple_of(1 << 20);

    // pci_setup_bridge: I/O off, memory window, prefetchable window off.
    w32(hw, RP, PCI_IO_BASE_UPPER16, 0x0000_ffff);
    w16(hw, RP, PCI_IO_BASE, 0x00f0);
    w32(hw, RP, PCI_IO_BASE_UPPER16, 0);
    let mem = ((window_bus >> 16) as u32 & 0xfff0) | (((end - 1) as u32) & 0xfff0_0000);
    w32(hw, RP, PCI_MEMORY_BASE, mem);
    w32(hw, RP, PCI_PREF_LIMIT_UPPER32, 0);
    w32(hw, RP, PCI_PREF_MEMORY_BASE, 0x0000_fff0);
    w32(hw, RP, PCI_PREF_BASE_UPPER32, 0);
    w32(hw, RP, PCI_PREF_LIMIT_UPPER32, 0);
    let ctl = r16(hw, RP, PCI_BRIDGE_CONTROL);
    w16(hw, RP, PCI_BRIDGE_CONTROL, ctl);

    // pci_enable_device on the bridge, then on the endpoint; bus mastering.
    for dev in [RP, Device::ADDR] {
        let c = r16(hw, dev, PCI_COMMAND);
        w16(hw, dev, PCI_COMMAND, c | PCI_COMMAND_MEMORY);
        let c = r16(hw, dev, PCI_COMMAND);
        w16(hw, dev, PCI_COMMAND, c | PCI_COMMAND_MASTER);
    }
    Some(Device { vendor: id as u16, device: (id >> 16) as u16, class, bars })
}

/// One MSI-X vector's message.
#[derive(Clone, Copy, Debug)]
pub struct MsixMsg {
    pub addr: u64,
    pub data: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MsixError {
    NoCapability,
    TooFew(u32),
    /// The table sits in a BAR without a register block.
    TableBar(u8),
}

/// Where an enabled device's MSI-X table lies.
#[derive(Clone, Copy, Debug)]
pub struct MsixTable {
    pub block: Block,
    pub base: u64,
}

impl MsixTable {
    fn ctrl(&self, i: u32) -> u64 {
        self.base + i as u64 * PCI_MSIX_ENTRY_SIZE + PCI_MSIX_ENTRY_VECTOR_CTRL
    }

    /// `pci_msix_mask`: set the entry's mask bit, then a read of the table
    /// flushes the write to the device.
    pub fn mask(&self, hw: &mut impl Hw, i: u32) {
        hw.w32(self.block, self.ctrl(i), PCI_MSIX_ENTRY_CTRL_MASKBIT);
        hw.r32(self.block, self.base);
    }

    /// `pci_msix_unmask`.
    pub fn unmask(&self, hw: &mut impl Hw, i: u32) {
        hw.w32(self.block, self.ctrl(i), 0);
    }

    /// Vector `i`'s control word; bit 0 masks it.
    pub fn vector_ctrl(&self, hw: &mut impl Hw, i: u32) -> u32 {
        hw.r32(self.block, self.ctrl(i))
    }
}

/// `msix_capability_init`: enable with every vector masked, write `n`
/// messages into the table, INTx off, mask every entry, lift the function
/// mask; then unmask the `n` vectors, as RP1's chained handlers do when
/// they are installed. `block` names the register block of a BAR.
pub fn msix_enable(hw: &mut impl Hw, n: u32, msg: impl Fn(u32) -> MsixMsg,
    block: impl Fn(u8) -> Option<Block>) -> Result<MsixTable, MsixError>
{
    let dev = Device::ADDR;
    let cap = find_cap(hw, dev, PCI_CAP_ID_MSIX).ok_or(MsixError::NoCapability)?;
    let f = r16(hw, dev, cap + PCI_MSIX_FLAGS);
    w16(hw, dev, cap + PCI_MSIX_FLAGS, f | PCI_MSIX_FLAGS_MASKALL | PCI_MSIX_FLAGS_ENABLE);
    let flags = r16(hw, dev, cap + PCI_MSIX_FLAGS);
    let tsize = (flags & PCI_MSIX_FLAGS_QSIZE) as u32 + 1;
    if n > tsize { return Err(MsixError::TooFew(tsize)); }
    let t = r32(hw, dev, cap + PCI_MSIX_TABLE);
    let bir = (t & PCI_MSIX_TABLE_BIR) as u8;
    let blk = block(bir).ok_or(MsixError::TableBar(bir))?;
    let base = (t & !PCI_MSIX_TABLE_BIR) as u64;
    let entry = |i: u32| base + i as u64 * PCI_MSIX_ENTRY_SIZE;
    for i in 0..n {
        let m = msg(i);
        let e = entry(i);
        // __pci_write_msi_msg: masked while the message changes.
        hw.w32(blk, e + PCI_MSIX_ENTRY_VECTOR_CTRL, PCI_MSIX_ENTRY_CTRL_MASKBIT);
        hw.w32(blk, e + PCI_MSIX_ENTRY_LOWER_ADDR, m.addr as u32);
        hw.w32(blk, e + PCI_MSIX_ENTRY_UPPER_ADDR, (m.addr >> 32) as u32);
        hw.w32(blk, e + PCI_MSIX_ENTRY_DATA, m.data);
        hw.w32(blk, e + PCI_MSIX_ENTRY_VECTOR_CTRL, 0);
    }
    // pci_intx_for_msi.
    let c = r16(hw, dev, PCI_COMMAND);
    w16(hw, dev, PCI_COMMAND, c | PCI_COMMAND_INTX_DISABLE);
    // msix_mask_all: no stale entry may fire.
    for i in 0..tsize {
        hw.w32(blk, entry(i) + PCI_MSIX_ENTRY_VECTOR_CTRL, PCI_MSIX_ENTRY_CTRL_MASKBIT);
    }
    let f = r16(hw, dev, cap + PCI_MSIX_FLAGS);
    w16(hw, dev, cap + PCI_MSIX_FLAGS, f & !PCI_MSIX_FLAGS_MASKALL);
    for i in 0..n {
        hw.w32(blk, entry(i) + PCI_MSIX_ENTRY_VECTOR_CTRL, 0);
    }
    Ok(MsixTable { block: blk, base })
}
