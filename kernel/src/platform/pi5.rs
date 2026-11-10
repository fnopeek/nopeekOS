//! The Raspberry Pi 5's way to RP1: the device tree read into what
//! `pi5_core` needs, its register blocks mapped, bring-up run and reported.

use super::dt::{Fdt, Node};
use crate::hw::Mmio;
use crate::kprintln;
use pi5_core::hw::{Block, Hw};
use pi5_core::{dwc3, mip::Mip, pcie};

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

/// RP1's BAR1 alone, for the DWC3 work after bring-up.
struct Bar1(Mmio);

impl Hw for Bar1 {
    fn read(&mut self, _: Block, off: u64, _: u8) -> u32 { self.0.r32(off) }
    fn write(&mut self, _: Block, off: u64, _: u8, v: u32) { self.0.w32(off, v) }
    fn delay_us(&mut self, us: u64) { crate::time::delay_us(us) }
}

/// RP1's interrupt lines as a controller in front of the GIC
/// (`rp1_irq_chip`): line `i` is vector `i` of RP1's MSI-X table, which the
/// MIP turns into an edge-triggered SPI.
struct Rp1Irqs {
    rp1: spin::Once<(pi5_core::rp1::Rp1, Mmio, Mmio)>,
    /// `rp1_level_triggered_irq`.
    level: [core::sync::atomic::AtomicBool; pi5_core::rp1::RP1_IRQS as usize],
}

static RP1_IRQS: Rp1Irqs = Rp1Irqs {
    rp1: spin::Once::new(),
    level: [const { core::sync::atomic::AtomicBool::new(false) }; pi5_core::rp1::RP1_IRQS as usize],
};
static RP1_CHIP: &'static dyn crate::irq::Chained = &RP1_IRQS;
/// The MIP that RP1's vectors write to, for each line's SPI.
static MIP: spin::Once<Mip> = spin::Once::new();

/// RP1's BARs alone, for the interrupt chip.
struct Bars(Mmio, Mmio);

impl Hw for Bars {
    fn read(&mut self, b: Block, off: u64, _: u8) -> u32 {
        if b == Block::Rp1Bar0 { self.0.r32(off) } else { self.1.r32(off) }
    }
    fn write(&mut self, b: Block, off: u64, _: u8, v: u32) {
        if b == Block::Rp1Bar0 { self.0.w32(off, v) } else { self.1.w32(off, v) }
    }
    fn delay_us(&mut self, us: u64) { crate::time::delay_us(us) }
}

impl crate::irq::Chained for Rp1Irqs {
    fn mask(&self, line: u32) {
        if let Some((rp1, b0, b1)) = self.rp1.get() {
            rp1.irq_mask(&mut Bars(*b0, *b1), line);
        }
    }
    fn unmask(&self, line: u32) {
        if let Some((rp1, b0, b1)) = self.rp1.get() {
            rp1.irq_unmask(&mut Bars(*b0, *b1), line);
        }
    }
    fn handled(&self, line: u32) {
        let level = self.level.get(line as usize).is_some_and(|l| l.load(core::sync::atomic::Ordering::Relaxed));
        if let (true, Some((rp1, b0, b1))) = (level, self.rp1.get()) {
            rp1.irq_ack(&mut Bars(*b0, *b1), line);
        }
    }
}

/// `IRQ_TYPE_EDGE_RISING` and `IRQ_TYPE_LEVEL_HIGH` of a device tree
/// interrupt specifier.
const IRQ_TYPE_EDGE_RISING: u32 = 1;
const IRQ_TYPE_LEVEL_HIGH: u32 = 4;

/// The interrupt of RP1 child `node` (`interrupts` = line, type), as
/// `request_irq` would make it: `rp1_irq_xlate`, the line's SPI through the
/// MIP, `rp1_irq_set_type`, `rp1_irq_activate`. None without RP1, for an
/// unknown type, or if the GIC refuses the SPI.
fn rp1_irq(fdt: &Fdt, node: &Node) -> Option<(crate::irq::Irq, u32, u32)> {
    let (rp1, b0, b1) = RP1_IRQS.rp1.get()?;
    let line = fdt.prop_cell(node, b"interrupts", 0)?;
    let trigger = match fdt.prop_cell(node, b"interrupts", 1)? {
        IRQ_TYPE_EDGE_RISING => pi5_core::rp1::Trigger::Edge,
        IRQ_TYPE_LEVEL_HIGH => pi5_core::rp1::Trigger::Level,
        _ => return None,
    };
    let intid = crate::arch::aarch64::gic::SPI_BASE + MIP.get()?.msi(line)?.spi;
    let irq = crate::irq::register_chained(&RP1_CHIP, line, intid)?;
    RP1_IRQS.level[line as usize].store(trigger == pi5_core::rp1::Trigger::Level, core::sync::atomic::Ordering::Relaxed);
    rp1.irq_enable(&mut Bars(*b0, *b1), line, trigger);
    Some((irq, line, intid))
}

/// The DWC3 whose xHCI is being attached, for `plat_start`.
static STARTING: spin::Mutex<Option<(Mmio, dwc3::Dwc3)>> = spin::Mutex::new(None);

/// `dwc3_xhci_plat_start`: the PHYs may suspend again once xHCI runs.
fn dwc3_plat_start() {
    if let Some((bar1, d)) = *STARTING.lock() {
        d.enable_susphy(&mut Bar1(bar1), true);
    }
}

/// A `snps,dwc3` node's quirks (`dwc3_get_properties`).
fn dwc3_config(fdt: &Fdt, n: &Node) -> dwc3::Config {
    let b = |name: &[u8]| fdt.prop(n, name).is_some();
    let u8p = |name: &[u8]| {
        let mut v = [0u8; 1];
        fdt.prop_bytes(n, name, &mut v).map(|_| v[0]).unwrap_or(0)
    };
    dwc3::Config {
        dis_rxdet_inp3: b(b"snps,dis_rxdet_inp3_quirk"),
        enh_nak_fs: b(b"snps,enhanced-nak-fs-quirk"),
        enh_nak_hs: b(b"snps,enhanced-nak-hs-quirk"),
        parkmode_disable_ss: b(b"snps,parkmode-disable-ss-quirk"),
        parkmode_disable_hs: b(b"snps,parkmode-disable-hs-quirk"),
        parkmode_disable_fsls: b(b"snps,parkmode-disable-fsls-quirk"),
        dis_tx_ipgap_linecheck: b(b"snps,dis-tx-ipgap-linecheck-quirk"),
        dis_enblslpm: b(b"snps,dis_enblslpm_quirk"),
        dis_u2_freeclk_exists: b(b"snps,dis-u2-freeclk-exists-quirk"),
        dis_u2_susphy: b(b"snps,dis_u2_susphy_quirk"),
        dis_u3_susphy: b(b"snps,dis_u3_susphy_quirk"),
        axi_pipe_limit: u8p(b"snps,axi-pipe-limit"),
        rx_thr_num_pkt: u8p(b"snps,rx-thr-num-pkt"),
        rx_max_burst: u8p(b"snps,rx-max-burst"),
        tx_thr_num_pkt: u8p(b"snps,tx-thr-num-pkt"),
        tx_max_burst: u8p(b"snps,tx-max-burst"),
        tx_thr_num_pkt_prd: u8p(b"snps,tx-thr-num-pkt-prd"),
        tx_max_burst_prd: u8p(b"snps,tx-max-burst-prd"),
    }
}

/// `DWC3_XHCI_REGS_END + 1`: the xHCI part of a DWC3's window.
const DWC3_XHCI_LEN: u64 = 0x8000;

/// The DWC3 hosts under the RP1 node: each core into host mode, then its
/// xHCI to the USB driver. True when a keyboard came up.
fn usb(fdt: &Fdt, pcie_node: &Node, board: &mut Board, bar1_cpu: u64, dma_off: u64) -> bool {
    let mut rp1 = None;
    fdt.children(pcie_node, |c| {
        if fdt.name_is(c, b"rp1") { rp1 = Some(*c); }
        rp1.is_none()
    });
    let Some(rp1) = rp1 else { return false };
    let mut hosts = [None; 4];
    let mut n = 0;
    fdt.children(&rp1, |c| {
        if fdt.prop_has_str(c, b"compatible", b"snps,dwc3") && fdt.is_enabled(c) && n < hosts.len() {
            hosts[n] = Some(*c);
            n += 1;
        }
        true
    });
    let mut keyboard = false;
    for node in hosts.iter().flatten() {
        let Some((cpu, _)) = fdt.reg0(node) else { continue };
        let mut name = [0u8; 32];
        let l = fdt.name_into(node, &mut name);
        let name = core::str::from_utf8(&name[..l]).unwrap_or("?");
        let d = match dwc3::probe(board, cpu - bar1_cpu, dwc3_config(fdt, node), 0) {
            Ok(d) => d,
            Err(e) => { kprintln!("[npk] RP1 {}: {:?}", name, e); continue; }
        };
        kprintln!("[npk] RP1 {}: DWC3 {:x}, {} USB 2.0 + {} USB 3.0 ports, host mode; xHCI next",
            name, d.revision, d.num_usb2_ports, d.num_usb3_ports);
        *STARTING.lock() = Some((board.bar1, d));
        let quirks = crate::xhci::Quirks { write_64_hi_lo: true, plat_start: Some(dwc3_plat_start) };
        let irq = match rp1_irq(fdt, node) {
            Some((irq, line, intid)) => {
                kprintln!("[npk] RP1 {}: interrupt line {} -> INTID {} (irq {})", name, line, intid, irq);
                Some(irq)
            }
            None => {
                kprintln!("[npk] RP1 {}: no interrupt, events are polled", name);
                None
            }
        };
        keyboard |= crate::xhci::attach_platform(cpu, DWC3_XHCI_LEN, dma_off, quirks, irq);
        *STARTING.lock() = None;
    }
    keyboard
}

/// Bring up the PCIe link to RP1 and report what answered; then USB.
/// True when a USB keyboard came up.
pub fn init(fdt: &Fdt) -> bool {
    let Some(node) = rp1_controller(fdt) else {
        kprintln!("[npk] PCIe: no controller with RP1 in the device tree");
        return false;
    };
    let Some((pcie_base, pcie_len)) = fdt.reg0(&node) else { return false };

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
        return false;
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
        return false;
    };
    let Some(mip) = mip_from_dt(fdt, &mip_node) else {
        kprintln!("[npk] PCIe: MIP node unreadable");
        return false;
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
        Err(e) => { kprintln!("[npk] PCIe: probe failed: {:?}", e); return false; }
    };
    kprintln!("[npk] PCIe: setup done, hardware revision {:#x}; starting the link", p.hw_rev);
    pi5_core::pci::root_port_init(&mut board);
    let link = match p.start_link(&mut board) {
        Ok(l) => l,
        Err(e) => { kprintln!("[npk] PCIe: {:?}", e); return false; }
    };
    let gts = match link.speed { 1 => "2.5", 2 => "5.0", 3 => "8.0", _ => "?" };
    kprintln!("[npk] PCIe: link up, {} GT/s x{}", gts, link.width);
    let Some(dev) = pi5_core::pci::scan(&mut board, win.pci) else {
        kprintln!("[npk] PCIe: nothing on bus 1");
        return false;
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
        return false;
    }
    let rp1 = match pi5_core::rp1::probe(&mut board, &dev, &mip) {
        Ok(r) => r,
        Err(e) => { kprintln!("[npk] RP1: {:?}", e); return false; }
    };
    kprintln!("[npk] RP1: chip {:#x}{}, {} MSI-X vectors -> GIC SPI {}+",
        rp1.chip_id, if rp1.fpga { " (FPGA)" } else { "" }, pi5_core::rp1::RP1_IRQS, mip.msi_base);
    RP1_IRQS.rp1.call_once(|| (rp1, board.bar0, board.bar1));
    MIP.call_once(|| mip);

    let dma_off = inb[..n_in].iter().find(|w| w.cpu == 0).map(|w| w.pci).unwrap_or(0);
    usb(fdt, &node, &mut board, cpu_of(dev.bars[1].bus), dma_off)
}

/// What there is to do once bring-up ends: show what the USB keyboard
/// types (its controller's interrupt thread fills the key buffer; a
/// controller without an interrupt is polled here) and blink the activity
/// LED at 1 Hz.
pub fn echo_keys(led: bool) -> ! {
    kprintln!("[npk] USB keyboard ready: type, the keys appear below.");
    let mut led_on = false;
    let mut led_at = crate::time::ticks();
    loop {
        if crate::xhci::needs_poll() {
            crate::xhci::poll_events();
            crate::xhci::hotplug_pass();
        }
        while let Some(b) = crate::xhci::poll_keyboard() {
            match b {
                b'\r' | b'\n' => crate::framebuffer::write_byte(b'\n'),
                0x08 | 0x7f => crate::framebuffer::write_byte(0x08),
                0x20..=0x7e => crate::framebuffer::write_byte(b),
                // Escape sequences (arrows, F keys) and control bytes.
                0x00..=0x1f => {}
                // The console font is ASCII: one mark per character, at
                // its first byte.
                0xc0..=0xff => crate::framebuffer::write_byte(b'?'),
                _ => {}
            }
        }
        if led && crate::time::ticks() >= led_at + 50 {
            led_on = !led_on;
            crate::platform::led::set(led_on);
            led_at = crate::time::ticks();
        }
        // SAFETY: waits for the next interrupt (the 100 Hz tick at least);
        // no memory is touched.
        unsafe { core::arch::asm!("wfi", options(nomem, nostack)) };
    }
}
