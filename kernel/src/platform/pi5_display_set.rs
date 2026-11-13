//! The BCM2712 display as one device for a driver module: the HVS and the
//! blocks vc4 binds with it (`vc4_drv.c` component match), found from the
//! boot device tree through the same phandles Linux follows: the HVS's
//! `interrupt-parent`, each HDMI's `ddc`, `resets` and `interrupt-parent`,
//! the DDC's `interrupt-parent`.
//!
//! A module gets the register windows and GIC SPIs of these nodes only,
//! each window bounded to its `reg` entry in bytes: several of the blocks
//! share a 4 KiB page with other devices (GPIO, pin control). The writeback
//! engines (`mop`, `moplet`) are not part of the set: they write memory and
//! are not behind the HVS's IOMMU. The firmware mailbox stays kernel-owned
//! (`rpi_fw`); the module asks for clock rates through `fw_clock`.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use super::dt::{Fdt, Node};
use pi5_core::rpi_fw as fw;

/// The parts, by the index `npk_mmio_map_dt` and `npk_irq_register_dt`
/// take: 0 hvs, 1 pixelvalve0, 2 pixelvalve1, 3 disp_intr, 4 dvp, 5 hdmi0,
/// 6 hdmi1, 7 ddc0, 8 ddc1, 9 bsc_irq, 10 aon_intr.
const PARTS: usize = 11;

struct Block {
    /// The node's `reg` entries, CPU physical, in the binding's order.
    regs: Vec<(u64, u64)>,
    /// Its GIC interrupt ID, for a node wired straight to the GIC.
    spi: Option<u32>,
}

pub struct Set {
    /// The HVS's first window: the set's `DevId::Dt`, and the device its
    /// IOMMU confines.
    pub hvs: u64,
    blocks: [Block; PARTS],
}

static SET: spin::Once<Option<Set>> = spin::Once::new();
/// The module holding the set, 0 for none.
static OWNER: AtomicU32 = AtomicU32::new(0);
static DISPLAY_DONE: AtomicBool = AtomicBool::new(false);

/// The display set of the boot tree, if it describes a BCM2712 display.
pub fn get() -> Option<&'static Set> {
    SET.call_once(|| {
        let fdt = super::dt::boot()?;
        let s = resolve(&fdt);
        if s.is_none() {
            crate::kprintln!("[npk] display: the device tree has no complete BCM2712 display set");
        }
        s
    }).as_ref()
}

fn compatible(fdt: &Fdt, compat: &[u8]) -> Option<Node> {
    fdt.find_compatible(compat).filter(|n| fdt.is_enabled(n))
}

/// The node `node`'s property `name` points to by phandle at cell `i`.
fn by_phandle(fdt: &Fdt, node: &Node, name: &[u8], i: usize) -> Option<Node> {
    fdt.find_phandle(fdt.prop_cell(node, name, i)?)
}

fn same(a: &Node, b: &Node) -> bool {
    a.offset == b.offset
}

fn block(fdt: &Fdt, node: &Node) -> Option<Block> {
    let mut regs = Vec::new();
    fdt.reg(node, |a, s| regs.push((a, s)));
    if regs.is_empty() { return None; }
    // A GIC SPI: no `interrupt-parent` of its own (the consumers of an L2
    // controller have one), cells type 0, line, level-high 4.
    let spi = match (fdt.prop(node, b"interrupt-parent"), fdt.prop_cell(node, b"interrupts", 0)) {
        (None, Some(0)) => {
            let line = fdt.prop_cell(node, b"interrupts", 1)?;
            if fdt.prop_cell(node, b"interrupts", 2) != Some(4) { return None; }
            Some(line + 32)
        }
        _ => None,
    };
    Some(Block { regs, spi })
}

fn resolve(fdt: &Fdt) -> Option<Set> {
    let hvs = compatible(fdt, b"brcm,bcm2712-hvs")?;
    let pv0 = compatible(fdt, b"brcm,bcm2712-pixelvalve0")?;
    let pv1 = compatible(fdt, b"brcm,bcm2712-pixelvalve1")?;
    let hdmi0 = compatible(fdt, b"brcm,bcm2712-hdmi0")?;
    let hdmi1 = compatible(fdt, b"brcm,bcm2712-hdmi1")?;
    let disp_intr = by_phandle(fdt, &hvs, b"interrupt-parent", 0)?;
    let aon_intr = by_phandle(fdt, &hdmi0, b"interrupt-parent", 0)?;
    let ddc0 = by_phandle(fdt, &hdmi0, b"ddc", 0)?;
    let ddc1 = by_phandle(fdt, &hdmi1, b"ddc", 0)?;
    let bsc_irq = by_phandle(fdt, &ddc0, b"interrupt-parent", 0)?;
    let dvp = by_phandle(fdt, &hdmi0, b"resets", 0)?;
    let ok = fdt.prop_has_str(&disp_intr, b"compatible", b"brcm,l2-intc")
        && fdt.prop_has_str(&aon_intr, b"compatible", b"brcm,l2-intc")
        && fdt.prop_has_str(&bsc_irq, b"compatible", b"brcm,bcm7271-l2-intc")
        && fdt.prop_has_str(&ddc0, b"compatible", b"brcm,brcmstb-i2c")
        && fdt.prop_has_str(&ddc1, b"compatible", b"brcm,brcmstb-i2c")
        && fdt.prop_has_str(&dvp, b"compatible", b"brcm,brcm2711-dvp")
        && by_phandle(fdt, &hdmi1, b"interrupt-parent", 0).is_some_and(|n| same(&n, &aon_intr))
        && by_phandle(fdt, &ddc1, b"interrupt-parent", 0).is_some_and(|n| same(&n, &bsc_irq))
        && by_phandle(fdt, &hdmi1, b"resets", 0).is_some_and(|n| same(&n, &dvp));
    if !ok { return None; }
    let blocks: [Block; PARTS] = [
        block(fdt, &hvs)?,
        block(fdt, &pv0)?,
        block(fdt, &pv1)?,
        block(fdt, &disp_intr)?,
        block(fdt, &dvp)?,
        block(fdt, &hdmi0)?,
        block(fdt, &hdmi1)?,
        block(fdt, &ddc0)?,
        block(fdt, &ddc1)?,
        block(fdt, &bsc_irq)?,
        block(fdt, &aon_intr)?,
    ];
    // Every window is a device window of the identity map, never RAM.
    if blocks.iter().flat_map(|b| b.regs.iter())
        .any(|&(a, s)| crate::arch::aarch64::mmu::device_window(a, s).is_err())
    {
        return None;
    }
    // The pixel valves and the interrupt controllers raise SPIs of their own.
    if [1, 2, 3, 9, 10].iter().any(|&p: &usize| blocks[p].spi.is_none()) { return None; }
    Some(Set { hvs: blocks[0].regs[0].0, blocks })
}

impl Set {
    /// Window `reg` of part `part`: (address, length in bytes).
    pub fn window(&self, part: usize, reg: usize) -> Option<(u64, u64)> {
        self.blocks.get(part)?.regs.get(reg).copied()
    }

    /// The GIC interrupt ID of part `part`'s interrupt `index`.
    pub fn spi(&self, part: usize, index: usize) -> Option<u32> {
        if index != 0 { return None; }
        self.blocks.get(part)?.spi
    }
}

/// Is `base` the HVS of the display set?
pub fn is_hvs(base: u64) -> bool {
    get().is_some_and(|s| s.hvs == base)
}

/// The set for module `pid`: free, or its own already. pid 0 is the
/// shared identity of the inline paths and never owns a device.
pub fn take(pid: u32) -> bool {
    pid != 0 && match OWNER.compare_exchange(0, pid, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => true,
        Err(cur) => cur == pid,
    }
}

/// Its module lets go of the set.
pub fn release() {
    OWNER.store(0, Ordering::Release);
}

/// The firmware clocks of the set's `clocks` properties: the HVS's core
/// and disp, the HDMI's hdmi (M2MC) and bvb (PIXEL_BVB).
const CLOCKS: [u32; 4] = [
    fw::RPI_FIRMWARE_CORE_CLK_ID,
    fw::RPI_FIRMWARE_DISP_CLK_ID,
    fw::RPI_FIRMWARE_M2MC_CLK_ID,
    fw::RPI_FIRMWARE_PIXEL_BVB_CLK_ID,
];

/// What `npk_fw_clock` may do to a clock.
#[derive(Clone, Copy)]
pub enum ClockOp {
    Rate,
    MaxRate,
    MinRate,
    /// `raspberrypi_fw_set_rate`, clamped to the firmware's range.
    SetRate(u32),
    /// `raspberrypi_fw_prepare`: on, at the rate it has.
    Enable,
}

/// `op` on display clock `id`. None for a clock outside the display's, or
/// when the firmware refuses.
pub fn fw_clock(id: u32, op: ClockOp) -> Option<u32> {
    if !CLOCKS.contains(&id) { return None; }
    let rf = super::rpi_fw::property::<3>;
    match op {
        ClockOp::Rate => super::rpi_fw::clock_rate(id),
        ClockOp::MaxRate => super::rpi_fw::clock_max_rate(id),
        ClockOp::MinRate => super::rpi_fw::clock_min_rate(id),
        ClockOp::SetRate(rate) => {
            let lo = super::rpi_fw::clock_min_rate(id)?;
            let hi = super::rpi_fw::clock_max_rate(id)?;
            rf(fw::RPI_FIRMWARE_SET_CLOCK_RATE, [id, rate.clamp(lo, hi), 0]).map(|a| a[1])
        }
        ClockOp::Enable => rf(fw::RPI_FIRMWARE_SET_CLOCK_STATE, [id, fw::RPI_FIRMWARE_STATE_ENABLE_BIT, 0])
            .map(|a| a[1]),
    }
}

/// `RPI_FIRMWARE_NOTIFY_DISPLAY_DONE`: the firmware stops driving the
/// display. Once per boot; false if already sent or refused.
pub fn display_done() -> bool {
    if DISPLAY_DONE.swap(true, Ordering::AcqRel) { return false; }
    super::rpi_fw::property::<0>(fw::RPI_FIRMWARE_NOTIFY_DISPLAY_DONE, []).is_some()
}
