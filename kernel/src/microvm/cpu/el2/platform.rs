//! The guest's machine: QEMU's `virt` board (`hw/arm/virt.c`) in what a
//! Linux guest sees of it, its device tree built the way `create_fdt` and
//! its helpers build it. RAM is laid out as for the x86 guest
//! (`guest_mem::RamLayout`, from 0 with a hole at [3 GiB, 4 GiB)); the
//! devices sit in that hole, the virtio BARs at the addresses the x86
//! guest has them.
//!
//!   GICv2 distributor (emulated)            DIST_BASE
//!   GICv2 CPU interface (the GICV frame)    CPU_BASE
//!   16550 UART (polled)                     UART_BASE
//!   PCI ECAM, bus 0                         ECAM_BASE
//!   PCI memory: the virtio BARs              PCI_MMIO_BASE
//!   PCI 64-bit prefetchable: hostmem        `hostmem::gpa()`

use crate::microvm::devices::guest_mem::RamLayout;
use crate::microvm::linux::fdt_writer::FdtWriter;
use alloc::vec::Vec;

pub const DIST_BASE: u64 = 0xc000_0000;
pub const DIST_SIZE: u64 = 0x1_0000;
pub const CPU_BASE: u64 = 0xc001_0000;
/// The GICV frame: GICC registers and, in its second page, GICC_DIR.
pub const CPU_SIZE: u64 = 0x2000;
pub const UART_BASE: u64 = 0xc002_0000;
pub const UART_SIZE: u64 = 0x1000;
pub const ECAM_BASE: u64 = 0xc100_0000;
/// One bus: 32 devices x 8 functions x 4 KiB.
pub const ECAM_SIZE: u64 = 0x10_0000;
pub const PCI_MMIO_BASE: u64 = 0xfe00_0000;
pub const PCI_MMIO_SIZE: u64 = 0x0100_0000;

/// Device-tree GIC interrupt types (`dt-bindings/interrupt-controller/arm-gic.h`).
const GIC_SPI: u32 = 0;
const GIC_PPI: u32 = 1;
const IRQ_TYPE_EDGE_RISING: u32 = 1;
const IRQ_TYPE_LEVEL_HIGH: u32 = 4;

/// The architected timer's PPIs as QEMU's virt gives them (`ARCH_TIMER_*_IRQ`).
pub const PPI_SEC_PHYS: u32 = 13;
pub const PPI_PHYS: u32 = 14;
pub const PPI_VIRT: u32 = 11;
pub const PPI_HYP: u32 = 10;
/// The virtual timer's INTID.
pub const VTIMER_INTID: u32 = 16 + PPI_VIRT;

const PHANDLE_GIC: u32 = 1;

/// The guest's MPIDR for vCPU `i`: Aff0 = i, as KVM's
/// `reset_mpidr` for up to 16 vCPUs.
pub fn mpidr(i: u8) -> u64 {
    (1 << 31) | i as u64
}

/// Two cells of a 64-bit value.
fn hi_lo(v: u64) -> [u32; 2] {
    [(v >> 32) as u32, v as u32]
}

/// The device tree of a guest with `ram` and `vcpus`, booted with
/// `cmdline` and an initramfs at `[initrd, initrd + initrd_len)`.
pub fn build_dtb(ram: RamLayout, vcpus: u8, cmdline: &str, initrd: Option<(u64, u64)>) -> Vec<u8> {
    let mut f = FdtWriter::new();
    f.begin("");
    f.prop_u32("interrupt-parent", PHANDLE_GIC);
    f.prop_u32("#size-cells", 2);
    f.prop_u32("#address-cells", 2);
    f.prop_str("model", "nopeekOS microVM");
    f.prop_str("compatible", "linux,dummy-virt");

    f.begin("psci");
    f.prop_strs("compatible", &["arm,psci-1.0", "arm,psci-0.2", "arm,psci"]);
    f.prop_str("method", "hvc");
    f.end();

    // `create_fdt`: memory nodes, one per region.
    let mut mem = |f: &mut FdtWriter, base: u64, size: u64| {
        if size == 0 { return; }
        f.begin(&alloc::format!("memory@{:x}", base));
        f.prop_str("device_type", "memory");
        let [bh, bl] = hi_lo(base);
        let [sh, sl] = hi_lo(size);
        f.prop_cells("reg", &[bh, bl, sh, sl]);
        f.end();
    };
    mem(&mut f, 0, ram.low);
    mem(&mut f, crate::microvm::devices::guest_mem::HIGH_RAM_BASE, ram.high());

    // `fdt_add_cpu_nodes`.
    f.begin("cpus");
    f.prop_u32("#size-cells", 0);
    f.prop_u32("#address-cells", 1);
    for i in 0..vcpus {
        f.begin(&alloc::format!("cpu@{}", i));
        f.prop_str("device_type", "cpu");
        f.prop_str("compatible", "arm,arm-v8");
        f.prop_u32("reg", (mpidr(i) & 0xff_ffff) as u32);
        f.prop_str("enable-method", "psci");
        f.end();
    }
    f.end();

    // `fdt_add_timer_nodes`: PPIs to every vCPU, level high.
    f.begin("timer");
    f.prop_strs("compatible", &["arm,armv8-timer", "arm,armv7-timer"]);
    let mask = ((1u32 << vcpus) - 1) << 8;
    let t = IRQ_TYPE_LEVEL_HIGH | mask;
    f.prop_cells("interrupts", &[
        GIC_PPI, PPI_SEC_PHYS, t,
        GIC_PPI, PPI_PHYS, t,
        GIC_PPI, PPI_VIRT, t,
        GIC_PPI, PPI_HYP, t,
    ]);
    f.prop_empty("always-on");
    f.end();

    // `fdt_add_gic_node`: GICv2, distributor and CPU interface.
    f.begin(&alloc::format!("intc@{:x}", DIST_BASE));
    f.prop_str("compatible", "arm,cortex-a15-gic");
    f.prop_u32("#interrupt-cells", 3);
    f.prop_empty("interrupt-controller");
    f.prop_u32("#address-cells", 0);
    let [dh, dl] = hi_lo(DIST_BASE);
    let [ch, cl] = hi_lo(CPU_BASE);
    f.prop_cells("reg", &[dh, dl, 0, 0x1000, ch, cl, 0, CPU_SIZE as u32]);
    f.prop_u32("phandle", PHANDLE_GIC);
    f.end();

    // A polled 16550: no interrupt, the 8250 driver's timer reads it.
    f.begin(&alloc::format!("serial@{:x}", UART_BASE));
    f.prop_str("compatible", "ns16550a");
    let [uh, ul] = hi_lo(UART_BASE);
    f.prop_cells("reg", &[uh, ul, 0, UART_SIZE as u32]);
    f.prop_u32("clock-frequency", 1_843_200);
    f.end();

    // `create_pcie`: a generic ECAM host bridge, bus 0, BARs as assigned.
    f.begin(&alloc::format!("pcie@{:x}", ECAM_BASE));
    f.prop_str("compatible", "pci-host-ecam-generic");
    f.prop_str("device_type", "pci");
    f.prop_u32("#address-cells", 3);
    f.prop_u32("#size-cells", 2);
    f.prop_u32("linux,pci-domain", 0);
    f.prop_cells("bus-range", &[0, 0]);
    f.prop_empty("dma-coherent");
    let [eh, el] = hi_lo(ECAM_BASE);
    f.prop_cells("reg", &[eh, el, 0, ECAM_SIZE as u32]);
    let [mh, ml] = hi_lo(PCI_MMIO_BASE);
    let [wh, wl] = hi_lo(crate::microvm::devices::hostmem::gpa());
    let [zh, zl] = hi_lo(crate::microvm::devices::hostmem::size());
    f.prop_cells("ranges", &[
        0x0200_0000, mh, ml, mh, ml, 0, PCI_MMIO_SIZE as u32,
        0x4300_0000, wh, wl, wh, wl, zh, zl,
    ]);
    // INTA of each slot to its SPI. QEMU's map (`create_pcie_irq_map`)
    // swizzles four lines over the slots and masks two device bits; here
    // each slot has its own line, so the mask keeps all five.
    f.prop_u32("#interrupt-cells", 1);
    f.prop_cells("interrupt-map-mask", &[0xf800, 0, 0, 7]);
    let mut map = Vec::new();
    for &(slot, line) in crate::microvm::devices::pci_bus::INTX_LINES.iter() {
        map.extend_from_slice(&[(slot as u32) << 11, 0, 0, 1, PHANDLE_GIC, GIC_SPI, line as u32,
            IRQ_TYPE_EDGE_RISING]);
    }
    f.prop_cells("interrupt-map", &map);
    f.end();

    f.begin("chosen");
    f.prop_str("bootargs", cmdline);
    f.prop_str("stdout-path", &alloc::format!("/serial@{:x}", UART_BASE));
    if let Some((start, end)) = initrd {
        f.prop_u64s("linux,initrd-start", &[start]);
        f.prop_u64s("linux,initrd-end", &[end]);
    }
    f.prop_u32("linux,pci-probe-only", 1);
    let mut seed = [0u8; 32];
    crate::csprng::fill(&mut seed);
    f.prop("rng-seed", &seed);
    f.prop_u64s("kaslr-seed", &[crate::csprng::random_u64()]);
    f.end();

    f.end();
    f.finish()
}
