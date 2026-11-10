//! The Raspberry Pi 5's way to RP1: the device tree read into what
//! `pi5_core` needs, its register blocks mapped, bring-up run and reported.

use super::dt::{Fdt, Node};
use crate::hw::Mmio;
use crate::kprintln;
use pi5_core::hw::{Block, Hw};
use pi5_core::{mip::Mip, pcie};

const MAX_WINDOWS: usize = 8;

/// The register blocks, mapped.
struct Board {
    pcie: Mmio,
    rescal: Mmio,
    reset: Mmio,
    mip: Mmio,
    bar0: Mmio,
    bar1: Mmio,
}

impl Board {
    fn block(&self, b: Block) -> &Mmio {
        match b {
            Block::Pcie => &self.pcie,
            Block::Rescal => &self.rescal,
            Block::Reset => &self.reset,
            Block::Mip => &self.mip,
            Block::Rp1Bar0 => &self.bar0,
            Block::Rp1Bar1 => &self.bar1,
        }
    }
}

impl Hw for Board {
    fn read(&mut self, b: Block, off: u64, width: u8) -> u32 {
        let m = self.block(b);
        match width {
            1 => m.r8(off) as u32,
            2 => m.r16(off) as u32,
            _ => m.r32(off),
        }
    }
    fn write(&mut self, b: Block, off: u64, width: u8, v: u32) {
        let m = self.block(b);
        match width {
            1 => m.w8(off, v as u8),
            2 => m.w16(off, v as u16),
            _ => m.w32(off, v),
        }
    }
    fn delay_us(&mut self, us: u64) {
        crate::time::delay_us(us);
    }
}

/// Map `len` bytes of a node's register window, or an empty window.
fn map(phys: u64, len: u64) -> Option<Mmio> {
    // SAFETY: every caller passes a `reg` window of a device tree node of a
    // device this module drives; `map` refuses RAM.
    unsafe { Mmio::map(phys, len) }.ok()
}

/// The controller whose child is RP1.
fn rp1_controller(fdt: &Fdt) -> Option<Node> {
    let mut found = None;
    fdt.walk(|n| {
        if fdt.prop_has_str(n, b"compatible", b"brcm,bcm2712-pcie") && fdt.is_enabled(n) {
            let mut has_rp1 = false;
            fdt.children(n, |c| {
                has_rp1 |= fdt.name_is(c, b"rp1");
                !has_rp1
            });
            if has_rp1 {
                found = Some(*n);
                return false;
            }
        }
        true
    });
    found
}

/// The node and argument cells of entry `name` of `resets`/`reset-names`.
fn reset_by_name(fdt: &Fdt, n: &Node, name: &[u8]) -> Option<(Node, Option<u32>)> {
    let idx = fdt.prop_str_index(n, b"reset-names", name)?;
    // Walk `resets`, each entry a phandle and that controller's
    // #reset-cells.
    let mut cell = 0;
    let mut i = 0;
    loop {
        let ph = fdt.prop_cell(n, b"resets", cell)?;
        let ctrl = fdt.find_phandle(ph)?;
        let cells = fdt.prop_u32(&ctrl, b"#reset-cells").unwrap_or(0) as usize;
        if i == idx {
            let arg = if cells > 0 { fdt.prop_cell(n, b"resets", cell + 1) } else { None };
            return Some((ctrl, arg));
        }
        cell += 1 + cells;
        i += 1;
    }
}

/// `mip_parse_dt`.
fn mip_from_dt(fdt: &Fdt, node: &Node) -> Option<Mip> {
    let msi_offset = fdt.prop_u32(node, b"brcm,msi-offset").unwrap_or(0);
    let gic = fdt.find_phandle(fdt.prop_cell(node, b"msi-ranges", 0)?)?;
    let ic = fdt.prop_u32(&gic, b"#interrupt-cells")? as usize;
    let msi_base = fdt.prop_cell(node, b"msi-ranges", 2)?;
    let num_msis = fdt.prop_cell(node, b"msi-ranges", ic + 1)?;
    let mut regs = [(0u64, 0u64); 2];
    let mut k = 0;
    fdt.reg_raw(node, |a, s| {
        if k < 2 { regs[k] = (a, s); }
        k += 1;
    });
    if k < 2 { return None; }
    Some(Mip { msi_offset, msi_base, num_msis, msg_addr: regs[1].0 })
}

/// PCI address space code of a `ranges` flags cell: 2 = 32-bit memory,
/// 3 = 64-bit memory.
fn is_mem(flags: u32) -> bool {
    matches!((flags >> 24) & 3, 2 | 3)
}

/// Bring up the PCIe link to RP1 and report what answered.
pub fn init(fdt: &Fdt) {
    let Some(node) = rp1_controller(fdt) else {
        kprintln!("[npk] PCIe: no controller with RP1 in the device tree");
        return;
    };
    let Some((pcie_base, pcie_len)) = fdt.reg0(&node) else { return };

    let mut out = [pcie::Window { cpu: 0, pci: 0, size: 0 }; MAX_WINDOWS];
    let mut n_out = 0;
    fdt.ranges(&node, b"ranges", |flags, pci, cpu, size| {
        if is_mem(flags) && n_out < MAX_WINDOWS {
            out[n_out] = pcie::Window { cpu, pci, size };
            n_out += 1;
        }
    });
    let mut inb = [pcie::Window { cpu: 0, pci: 0, size: 0 }; MAX_WINDOWS];
    let mut n_in = 0;
    fdt.ranges(&node, b"dma-ranges", |_, pci, cpu, size| {
        if n_in < MAX_WINDOWS {
            inb[n_in] = pcie::Window { cpu, pci, size };
            n_in += 1;
        }
    });
    if n_out == 0 {
        kprintln!("[npk] PCIe: no memory window");
        return;
    }

    let mut vdm = [0u8; 8];
    let mut fifo = [0u8; 4];
    let mut mode = [0u8; 16];
    let clkreq_mode = match fdt.prop_str(&node, b"brcm,clkreq-mode", &mut mode) {
        None => pcie::ClkreqMode::Default,
        Some(l) => match &mode[..l] {
            b"default" => pcie::ClkreqMode::Default,
            b"no-l1ss" => pcie::ClkreqMode::NoL1ss,
            _ => pcie::ClkreqMode::Safe,
        },
    };
    let bridge = reset_by_name(fdt, &node, b"bridge");
    let swinit = reset_by_name(fdt, &node, b"swinit");
    let rescal = reset_by_name(fdt, &node, b"rescal");
    let cfg = pcie::Config {
        outbound: &out[..n_out],
        inbound: &inb[..n_in],
        max_link_speed: fdt.prop_u32(&node, b"max-link-speed").unwrap_or(0),
        num_lanes: fdt.prop_u32(&node, b"num-lanes"),
        aspm_no_l0s: fdt.prop(&node, b"aspm-no-l0s").is_some(),
        fifo_qos_map: fdt.prop_bytes(&node, b"brcm,fifo-qos-map", &mut fifo).map(|_| fifo),
        vdm_qos_map: fdt.prop_bytes(&node, b"brcm,vdm-qos-map", &mut vdm).map(|_| vdm),
        clkreq_mode,
        tperst_clk_ms: fdt.prop_u32(&node, b"brcm,tperst-clk-ms").unwrap_or(0),
        bridge_reset: bridge.and_then(|(_, a)| a),
        swinit_reset: swinit.and_then(|(_, a)| a),
        rescal: rescal.is_some(),
    };

    // The reset controllers: bridge and swinit share the brcmstb block.
    let reset_node = bridge.or(swinit).map(|(c, _)| c);
    let reg_of = |n: Option<Node>| n.and_then(|n| fdt.reg0(&n));
    let Some(mip_node) = fdt.prop_u32(&node, b"msi-parent").and_then(|ph| fdt.find_phandle(ph)) else {
        kprintln!("[npk] PCIe: no msi-parent");
        return;
    };
    let Some(mip) = mip_from_dt(fdt, &mip_node) else {
        kprintln!("[npk] PCIe: MIP node unreadable");
        return;
    };
    let empty = Mmio::empty();
    let win = out[0];
    // RP1's BARs land at the start of the first window (`pci::scan`):
    // BAR1 (4 MiB) at bus 0, BAR0 (16 KiB) after BAR2; mapped generously,
    // checked against what the scan assigns before use.
    let mut board = Board {
        pcie: map(pcie_base, pcie_len).unwrap_or(empty),
        rescal: reg_of(rescal.map(|(c, _)| c)).and_then(|(a, s)| map(a, s)).unwrap_or(empty),
        reset: reg_of(reset_node).and_then(|(a, s)| map(a, s)).unwrap_or(empty),
        mip: reg_of(Some(mip_node)).and_then(|(a, s)| map(a, s)).unwrap_or(empty),
        bar0: empty,
        bar1: empty,
    };

    kprintln!("[npk] PCIe: controller @ {:#x}, window {:#x} -> bus {:#x}, MIP SPI {}..{}",
        pcie_base, win.cpu, win.pci, mip.msi_base, mip.msi_base + mip.num_msis);

    // Bring-up step by step, so the screen shows where a hang happened.
    mip.init(&mut board);
    let mut p = match pcie::probe(&mut board, &cfg) {
        Ok(p) => p,
        Err(e) => { kprintln!("[npk] PCIe: probe failed: {:?}", e); return; }
    };
    kprintln!("[npk] PCIe: setup done, hardware revision {:#x}; starting the link", p.hw_rev);
    pi5_core::pci::root_port_init(&mut board);
    let link = match p.start_link(&mut board) {
        Ok(l) => l,
        Err(e) => { kprintln!("[npk] PCIe: {:?}", e); return; }
    };
    let gts = match link.speed { 1 => "2.5", 2 => "5.0", 3 => "8.0", _ => "?" };
    kprintln!("[npk] PCIe: link up, {} GT/s x{}", gts, link.width);
    let Some(dev) = pi5_core::pci::scan(&mut board, win.pci) else {
        kprintln!("[npk] PCIe: nothing on bus 1");
        return;
    };
    kprintln!("[npk] PCIe: 01:00.0 [{:04x}:{:04x}] class {:06x}", dev.vendor, dev.device, dev.class);
    for (i, b) in dev.bars.iter().enumerate().filter(|(_, b)| b.size != 0) {
        kprintln!("[npk]   BAR{} bus {:#x}, {} KiB", i, b.bus, b.size >> 10);
    }
    let cpu_of = |bus: u64| win.cpu + (bus - win.pci);
    board.bar0 = map(cpu_of(dev.bars[0].bus), dev.bars[0].size).unwrap_or(empty);
    board.bar1 = map(cpu_of(dev.bars[1].bus), dev.bars[1].size).unwrap_or(empty);
    if (dev.vendor, dev.device) != (pi5_core::rp1::PCI_VENDOR_ID_RPI, pi5_core::rp1::PCI_DEVICE_ID_RP1_C0) {
        kprintln!("[npk] PCIe: not RP1");
        return;
    }
    let rp1 = match pi5_core::rp1::probe(&mut board, &dev, &mip) {
        Ok(r) => r,
        Err(e) => { kprintln!("[npk] RP1: {:?}", e); return; }
    };
    kprintln!("[npk] RP1: chip {:#x}{}, {} MSI-X vectors -> GIC SPI {}+",
        rp1.chip_id, if rp1.fpga { " (FPGA)" } else { "" }, pi5_core::rp1::RP1_IRQS, mip.msi_base);

    // The two USB controllers' identity, read before any driver touches
    // them: GSNPSID and GHWPARAMS0..8 of each DWC3.
    for (name, off) in [("usb@200000", 0x20_0000u64), ("usb@300000", 0x30_0000)] {
        kprintln!("[npk] RP1 {}: reading DWC3 ID registers...", name);
        let id = board.r32(Block::Rp1Bar1, off + 0xc120);
        let mut hp = [0u32; 9];
        for (i, v) in hp.iter_mut().enumerate().take(8) {
            *v = board.r32(Block::Rp1Bar1, off + 0xc140 + 4 * i as u64);
        }
        hp[8] = board.r32(Block::Rp1Bar1, off + 0xc600);
        kprintln!("[npk]   GSNPSID {:#010x}", id);
        kprintln!("[npk]   GHWPARAMS {:08x} {:08x} {:08x} {:08x} {:08x}", hp[0], hp[1], hp[2], hp[3], hp[4]);
        kprintln!("[npk]             {:08x} {:08x} {:08x} {:08x}", hp[5], hp[6], hp[7], hp[8]);
    }
}
