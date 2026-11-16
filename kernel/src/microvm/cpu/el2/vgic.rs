//! The guest's GICv2, after Linux `arch/arm64/kvm/vgic/`: the distributor
//! is emulated (`vgic-mmio.c`, `vgic-mmio-v2.c`), the CPU interface is the
//! hardware's virtual one (GICV, mapped into the guest), fed through the
//! list registers of the hypervisor interface GICH (`vgic-v2.c`).
//!
//! Before every entry `flush` puts the interrupts a vCPU should see into
//! its list registers, highest priority first (`vgic_flush_lr_state`);
//! after every exit `fold` takes back what the guest did with them
//! (`vgic_v2_fold_lr_state`). A level interrupt asks for an EOI
//! maintenance interrupt so its line is sampled again once the guest is
//! done with it; more interrupts than list registers ask for an underflow
//! one. Interrupt groups are not emulated (everything is group 0, as for a
//! Linux guest on KVM's GICv2).

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use spin::Mutex;

pub const NR_SPIS: usize = 64;
const NR_PRIVATE: usize = 32;
const NR_IRQS: usize = NR_PRIVATE + NR_SPIS;
pub const MAX_VCPUS: usize = 8;
const MAX_LRS: usize = 64;

// GICD registers (`include/linux/irqchip/arm-gic.h`).
const GIC_DIST_CTRL: u32 = 0x000;
const GIC_DIST_CTR: u32 = 0x004;
const GIC_DIST_IIDR: u32 = 0x008;
const GIC_DIST_IGROUP: u32 = 0x080;
const GIC_DIST_ENABLE_SET: u32 = 0x100;
const GIC_DIST_ENABLE_CLEAR: u32 = 0x180;
const GIC_DIST_PENDING_SET: u32 = 0x200;
const GIC_DIST_PENDING_CLEAR: u32 = 0x280;
const GIC_DIST_ACTIVE_SET: u32 = 0x300;
const GIC_DIST_ACTIVE_CLEAR: u32 = 0x380;
const GIC_DIST_PRI: u32 = 0x400;
const GIC_DIST_TARGET: u32 = 0x800;
const GIC_DIST_CONFIG: u32 = 0xc00;
const GIC_DIST_SOFTINT: u32 = 0xf00;
const GIC_DIST_SGI_PENDING_CLEAR: u32 = 0xf10;
const GIC_DIST_SGI_PENDING_SET: u32 = 0xf20;
/// `IMPLEMENTER_ARM`, KVM's IIDR implementer.
const IIDR: u32 = 0x43b;
/// ICPIDR2: architecture version 2.
const ICPIDR2: u32 = 0xfe8;

// GICH registers.
const GICH_HCR: u64 = 0x0;
const GICH_VTR: u64 = 0x4;
const GICH_VMCR: u64 = 0x8;
const GICH_ELRSR0: u64 = 0x30;
const GICH_ELRSR1: u64 = 0x34;
const GICH_APR: u64 = 0xf0;
const GICH_LR0: u64 = 0x100;
const GICH_HCR_EN: u32 = 1 << 0;
const GICH_HCR_UIE: u32 = 1 << 1;
const GICH_HCR_NPIE: u32 = 1 << 3;
const GICH_LR_VIRTUALID: u32 = 0x3ff;
const GICH_LR_PHYSID_CPUID_SHIFT: u32 = 10;
const GICH_LR_PHYSID_CPUID: u32 = 7 << GICH_LR_PHYSID_CPUID_SHIFT;
const GICH_LR_EOI: u32 = 1 << 19;
const GICH_LR_PRIORITY_SHIFT: u32 = 23;
const GICH_LR_PENDING_BIT: u32 = 1 << 28;
const GICH_LR_ACTIVE_BIT: u32 = 1 << 29;
const GICH_LR_STATE: u32 = 3 << 28;
/// GICH_VMCR.PMR, the guest's priority mask (`GICH_VMCR_PRIMASK_*`).
const GICH_VMCR_PRIMASK_SHIFT: u32 = 27;

#[derive(Clone, Copy)]
struct Irq {
    enabled: bool,
    pending_latch: bool,
    line_level: bool,
    active: bool,
    edge: bool,
    priority: u8,
    /// ITARGETSR as written (SPIs); the vCPU is its lowest set bit.
    targets: u8,
    /// SGIs: the vCPUs that sent it and have not been delivered yet.
    sources: u8,
    active_source: u8,
    /// The vCPU whose list register holds it, if any.
    in_lr: Option<u8>,
}

impl Irq {
    const fn new(edge: bool) -> Self {
        Irq { enabled: false, pending_latch: false, line_level: false, active: false, edge,
            priority: 0, targets: 0, sources: 0, active_source: 0, in_lr: None }
    }

    /// `irq_is_pending`.
    fn pending(&self) -> bool {
        if self.edge { self.pending_latch } else { self.pending_latch || self.line_level }
    }
}

struct Dist {
    enabled: bool,
    vcpus: u8,
    private: [[Irq; NR_PRIVATE]; MAX_VCPUS],
    spis: [Irq; NR_SPIS],
}

impl Dist {
    const fn new() -> Self {
        // SGIs are edge, PPIs level (KVM `vgic_init`: PPIs level unless
        // the guest reconfigures), SPIs level until configured.
        let mut p = [Irq::new(false); NR_PRIVATE];
        let mut i = 0;
        while i < 16 {
            p[i] = Irq::new(true);
            i += 1;
        }
        Dist { enabled: false, vcpus: 1, private: [p; MAX_VCPUS], spis: [Irq::new(false); NR_SPIS] }
    }

    fn irq(&mut self, vcpu: u8, intid: u32) -> Option<&mut Irq> {
        let i = intid as usize;
        if i < NR_PRIVATE {
            self.private.get_mut(vcpu as usize).map(|p| &mut p[i])
        } else {
            self.spis.get_mut(i - NR_PRIVATE)
        }
    }

    /// The vCPU an interrupt goes to (`vgic_target_oracle` for GICv2).
    fn target(&self, owner: u8, intid: u32, irq: &Irq) -> Option<u8> {
        if (intid as usize) < NR_PRIVATE {
            return Some(owner);
        }
        let t = irq.targets & ((1u16 << self.vcpus) - 1) as u8;
        (t != 0).then(|| t.trailing_zeros() as u8)
    }
}

static DIST: Mutex<Dist> = Mutex::new(Dist::new());
/// Host GICH and GICV, and the list registers the hardware has.
static GICH: AtomicU64 = AtomicU64::new(0);
static GICV: AtomicU64 = AtomicU64::new(0);
static NR_LR: AtomicUsize = AtomicUsize::new(0);
/// The maintenance interrupt's INTID.
static MAINT_IRQ: AtomicU64 = AtomicU64::new(0);

/// The host's GICH/GICV from its device tree (`vgic_v2_probe`): the third
/// and fourth `reg` of the GIC node and the maintenance PPI. False without
/// virtualization extensions.
pub fn probe(fdt: &crate::platform::dt::Fdt) -> bool {
    let node = fdt.find_compatible(b"arm,gic-400").or_else(|| fdt.find_compatible(b"arm,cortex-a15-gic"));
    let Some(node) = node else { return false };
    let mut regs = [(0u64, 0u64); 4];
    let mut n = 0;
    fdt.reg(&node, |a, s| {
        if n < 4 { regs[n] = (a, s); }
        n += 1;
    });
    if n < 4 || regs[3].1 < 0x2000 {
        return false;
    }
    let Some((o, len)) = fdt.prop(&node, b"interrupts") else { return false };
    if len < 12 || fdt.cell_u32(o) != 1 {
        return false;
    }
    let maint = 16 + fdt.cell_u32(o + 4);
    GICH.store(regs[2].0, Ordering::Relaxed);
    GICV.store(regs[3].0, Ordering::Relaxed);
    MAINT_IRQ.store(maint as u64, Ordering::Relaxed);
    let vtr = gich_rd(GICH_VTR);
    NR_LR.store(((vtr & 0x3f) as usize + 1).min(MAX_LRS), Ordering::Relaxed);
    true
}

pub fn gicv_base() -> u64 {
    GICV.load(Ordering::Relaxed)
}

pub fn nr_lr() -> usize {
    NR_LR.load(Ordering::Relaxed)
}

pub fn maintenance_irq() -> u32 {
    MAINT_IRQ.load(Ordering::Relaxed) as u32
}

fn gich_rd(off: u64) -> u32 {
    // SAFETY: GICH from the device tree, Device memory in the identity map;
    // banked per core, so each core reaches its own.
    unsafe { core::ptr::read_volatile((GICH.load(Ordering::Relaxed) + off) as *const u32) }
}

fn gich_wr(off: u64, v: u32) {
    // SAFETY: as in `gich_rd`.
    unsafe { core::ptr::write_volatile((GICH.load(Ordering::Relaxed) + off) as *mut u32, v) }
}

/// A fresh distributor for a VM of `vcpus` (`kvm_vgic_dist_init`).
pub fn reset(vcpus: u8) {
    let mut d = DIST.lock();
    *d = Dist::new();
    d.vcpus = vcpus.clamp(1, MAX_VCPUS as u8);
    for v in 0..MAX_VCPUS {
        for i in 0..NR_PRIVATE {
            d.private[v][i].targets = 1 << v;
        }
    }
}

/// One vCPU's CPU interface across exits (`vgic_v2_cpu_if`).
pub struct CpuIf {
    vmcr: u32,
    apr: u32,
    lr: [u32; MAX_LRS],
    used: usize,
    /// A pending interrupt is left behind: ask for an exit when the list
    /// registers drain.
    underflow: bool,
}

impl CpuIf {
    pub const fn new() -> Self {
        CpuIf { vmcr: 0, apr: 0, lr: [0; MAX_LRS], used: 0, underflow: false }
    }

    /// The guest's priority mask, from its saved GICV_PMR.
    fn pmr(&self) -> u8 {
        (((self.vmcr >> GICH_VMCR_PRIMASK_SHIFT) & 0x1f) << 3) as u8
    }

    pub fn pmr_value(&self) -> u8 {
        self.pmr()
    }

    /// A stand-in holding only a priority mask, for `pending_for`.
    pub fn set_pmr(&mut self, pmr: u8) {
        self.vmcr = ((pmr as u32 >> 3) & 0x1f) << GICH_VMCR_PRIMASK_SHIFT;
    }
}

/// `vgic_v2_populate_lr`.
fn populate_lr(irq: &mut Irq, intid: u32) -> u32 {
    let mut val = intid & GICH_LR_VIRTUALID;
    if irq.pending() {
        val |= GICH_LR_PENDING_BIT;
        if irq.edge {
            irq.pending_latch = false;
        }
        if intid < 16 {
            let src = irq.sources.trailing_zeros().min(7);
            val |= src << GICH_LR_PHYSID_CPUID_SHIFT;
            irq.sources &= !(1 << src);
            if irq.sources != 0 {
                irq.pending_latch = true;
            }
        }
    }
    if irq.active {
        val |= GICH_LR_ACTIVE_BIT;
        if intid < 16 {
            val |= (irq.active_source as u32) << GICH_LR_PHYSID_CPUID_SHIFT;
        }
    }
    if !irq.edge {
        val |= GICH_LR_EOI;
    }
    val | ((irq.priority as u32 >> 3) << GICH_LR_PRIORITY_SHIFT)
}

/// Before entry, with host interrupts masked on the vCPU's core: fill the
/// list registers and load the CPU interface (`kvm_vgic_flush_hwstate`,
/// `vgic_v2_restore_state`).
pub fn flush(vcpu: u8, cif: &mut CpuIf) {
    let nr_lr = nr_lr();
    let mut d = DIST.lock();
    let dist_on = d.enabled;
    // Candidates: (active first, priority, intid).
    let mut cand: [(u8, u8, u16); NR_IRQS] = [(0, 0, 0); NR_IRQS];
    let mut n = 0;
    for intid in 0..NR_IRQS as u32 {
        let Some(irq) = d.irq(vcpu, intid).copied() else { continue };
        if irq.in_lr.is_some() { continue; }
        let deliverable = irq.active || (dist_on && irq.enabled && irq.pending());
        if !deliverable { continue; }
        if d.target(vcpu, intid, &irq) != Some(vcpu) { continue; }
        cand[n] = (if irq.active { 0 } else { 1 }, irq.priority, intid as u16);
        n += 1;
    }
    cand[..n].sort_unstable();
    let take = n.min(nr_lr);
    for (i, &(_, _, intid)) in cand[..take].iter().enumerate() {
        let intid = intid as u32;
        if let Some(irq) = d.irq(vcpu, intid) {
            cif.lr[i] = populate_lr(irq, intid);
            irq.in_lr = Some(vcpu);
        }
    }
    cif.used = take;
    cif.underflow = n > take;
    drop(d);
    let mut hcr = GICH_HCR_EN;
    if cif.underflow {
        hcr |= GICH_HCR_UIE | GICH_HCR_NPIE;
    }
    gich_wr(GICH_HCR, hcr);
    gich_wr(GICH_VMCR, cif.vmcr);
    gich_wr(GICH_APR, cif.apr);
    for i in 0..cif.used {
        gich_wr(GICH_LR0 + i as u64 * 4, cif.lr[i]);
    }
}

/// After the exit, host interrupts still masked: save the CPU interface,
/// clear the list registers and fold their state back
/// (`vgic_v2_save_state`, `vgic_v2_fold_lr_state`).
pub fn fold(vcpu: u8, cif: &mut CpuIf) {
    if cif.used > 0 {
        let elrsr = gich_rd(GICH_ELRSR0) as u64 | (gich_rd(GICH_ELRSR1) as u64) << 32;
        for i in 0..cif.used {
            if elrsr & (1 << i) != 0 {
                cif.lr[i] &= !GICH_LR_STATE;
            } else {
                cif.lr[i] = gich_rd(GICH_LR0 + i as u64 * 4);
            }
            gich_wr(GICH_LR0 + i as u64 * 4, 0);
        }
    }
    cif.vmcr = gich_rd(GICH_VMCR);
    cif.apr = gich_rd(GICH_APR);
    gich_wr(GICH_HCR, 0);
    if cif.used == 0 {
        return;
    }
    let mut d = DIST.lock();
    for i in 0..cif.used {
        let val = cif.lr[i];
        let intid = val & GICH_LR_VIRTUALID;
        let cpuid = ((val & GICH_LR_PHYSID_CPUID) >> GICH_LR_PHYSID_CPUID_SHIFT) as u8;
        let Some(irq) = d.irq(vcpu, intid) else { continue };
        irq.in_lr = None;
        irq.active = val & GICH_LR_ACTIVE_BIT != 0;
        if irq.active && intid < 16 {
            irq.active_source = cpuid;
        }
        if irq.edge && val & GICH_LR_PENDING_BIT != 0 {
            irq.pending_latch = true;
            if intid < 16 {
                irq.sources |= 1 << cpuid;
            }
        }
        if !irq.edge && val & GICH_LR_STATE == 0 {
            irq.pending_latch = false;
        }
    }
    cif.used = 0;
}

/// `kvm_vgic_vcpu_pending_irq`: something the vCPU would take, unmasked
/// by its priority mask, is waiting (in a list register or not).
pub fn pending_for(vcpu: u8, cif: &CpuIf) -> bool {
    let pmr = cif.pmr();
    let mut d = DIST.lock();
    if !d.enabled {
        return false;
    }
    for intid in 0..NR_IRQS as u32 {
        let Some(irq) = d.irq(vcpu, intid).copied() else { continue };
        if irq.enabled && irq.pending() && !irq.active && irq.priority < pmr
            && d.target(vcpu, intid, &irq) == Some(vcpu)
        {
            return true;
        }
    }
    false
}

/// An edge on SPI `spi` (device line `spi`): latch it pending
/// (`kvm_vgic_inject_irq` with level 1 on an edge interrupt). Returns the
/// vCPU to kick.
pub fn pulse_spi(spi: u32) -> Option<u8> {
    let intid = NR_PRIVATE as u32 + spi;
    let mut d = DIST.lock();
    let irq = d.irq(0, intid)?;
    if irq.edge {
        irq.pending_latch = true;
    } else {
        // A level SPI the guest made level: the device's edge is a
        // momentary assertion, latched as KVM's userspace irqchip would.
        irq.pending_latch = true;
    }
    let irq = *irq;
    if !(irq.enabled && d.enabled) {
        return None;
    }
    d.target(0, intid, &irq)
}

/// Set the line of PPI `intid` on `vcpu` (`kvm_vgic_inject_irq` for a
/// level interrupt). True if it changed.
pub fn set_ppi_level(vcpu: u8, intid: u32, level: bool) -> bool {
    let mut d = DIST.lock();
    let Some(irq) = d.irq(vcpu, intid) else { return false };
    let changed = irq.line_level != level;
    irq.line_level = level;
    changed
}

/// GICD_SGIR (`vgic_mmio_write_sgir`): the vCPUs that now have an SGI.
fn write_sgir(d: &mut Dist, source: u8, val: u32) -> u8 {
    let sgi = val & 0xf;
    let filter = (val >> 24) & 3;
    let all = ((1u16 << d.vcpus) - 1) as u8;
    let targets = match filter {
        0 => ((val >> 16) & 0xff) as u8 & all,
        1 => all & !(1 << source),
        2 => 1 << source,
        _ => 0,
    };
    for t in 0..d.vcpus {
        if targets & (1 << t) == 0 { continue; }
        let irq = &mut d.private[t as usize][sgi as usize];
        irq.pending_latch = true;
        irq.sources |= 1 << source;
    }
    targets
}

/// Bit `bit` of `intid`'s register bank, as `f` reads it.
fn bits_read(d: &mut Dist, vcpu: u8, base_intid: u32, f: impl Fn(&Irq) -> bool) -> u32 {
    let mut v = 0;
    for b in 0..32 {
        if let Some(irq) = d.irq(vcpu, base_intid + b) {
            if f(irq) { v |= 1 << b; }
        }
    }
    v
}

fn bits_write(d: &mut Dist, vcpu: u8, base_intid: u32, val: u32, f: impl Fn(&mut Irq)) {
    for b in 0..32 {
        if val & (1 << b) == 0 { continue; }
        if let Some(irq) = d.irq(vcpu, base_intid + b) {
            f(irq);
        }
    }
}

/// A distributor read by `vcpu` of `len` bytes at offset `off`.
pub fn dist_read(vcpu: u8, off: u32, len: u8) -> u64 {
    let mut d = DIST.lock();
    let word = off & !3;
    let v: u32 = match word {
        GIC_DIST_CTRL => d.enabled as u32,
        GIC_DIST_CTR => ((d.vcpus as u32 - 1) << 5) | ((NR_IRQS as u32 / 32) - 1),
        GIC_DIST_IIDR => IIDR,
        GIC_DIST_IGROUP..=0x0fc => 0,
        GIC_DIST_ENABLE_SET..=0x1fc | GIC_DIST_ENABLE_CLEAR..=0x1fc => {
            let base = ((word & 0x7f) / 4) * 32;
            bits_read(&mut d, vcpu, base, |i| i.enabled)
        }
        GIC_DIST_PENDING_SET..=0x2fc => {
            let base = ((word & 0x7f) / 4) * 32;
            bits_read(&mut d, vcpu, base, |i| i.pending())
        }
        GIC_DIST_ACTIVE_SET..=0x3fc => {
            let base = ((word & 0x7f) / 4) * 32;
            bits_read(&mut d, vcpu, base, |i| i.active)
        }
        GIC_DIST_PRI..=0x7fc => {
            let mut v = 0;
            for b in 0..4 {
                if let Some(i) = d.irq(vcpu, word - GIC_DIST_PRI + b) { v |= (i.priority as u32) << (8 * b); }
            }
            v
        }
        GIC_DIST_TARGET..=0xbfc => {
            let mut v = 0;
            for b in 0..4 {
                if let Some(i) = d.irq(vcpu, word - GIC_DIST_TARGET + b) { v |= (i.targets as u32) << (8 * b); }
            }
            v
        }
        GIC_DIST_CONFIG..=0xcfc => {
            let base = (word - GIC_DIST_CONFIG) * 4;
            let mut v = 0;
            for b in 0..16 {
                if let Some(i) = d.irq(vcpu, base + b) {
                    if i.edge { v |= 2 << (2 * b); }
                }
            }
            v
        }
        GIC_DIST_SGI_PENDING_CLEAR..=0xf2c => {
            let first = (word & 0xf) as u32;
            let mut v = 0;
            for b in 0..4 {
                v |= (d.private[vcpu as usize % MAX_VCPUS][(first + b) as usize].sources as u32) << (8 * b);
            }
            v
        }
        ICPIDR2 => 0x2b,
        _ => 0,
    };
    let shift = (off & 3) * 8;
    let mask = if len >= 4 { 0xffff_ffff } else { (1u64 << (len as u32 * 8)) - 1 };
    ((v >> shift) as u64) & mask
}

/// A distributor write. Returns the vCPUs to kick (a bit each).
pub fn dist_write(vcpu: u8, off: u32, len: u8, val: u64) -> u8 {
    let mut d = DIST.lock();
    let word = off & !3;
    let val = val as u32;
    match word {
        GIC_DIST_CTRL => {
            d.enabled = val & 1 != 0;
            return all_vcpus(&d);
        }
        GIC_DIST_ENABLE_SET..=0x17c => {
            let base = ((word & 0x7f) / 4) * 32;
            bits_write(&mut d, vcpu, base, val, |i| i.enabled = true);
            return all_vcpus(&d);
        }
        GIC_DIST_ENABLE_CLEAR..=0x1fc => {
            let base = ((word & 0x7f) / 4) * 32;
            bits_write(&mut d, vcpu, base, val, |i| i.enabled = false);
        }
        GIC_DIST_PENDING_SET..=0x27c => {
            let base = ((word & 0x7f) / 4) * 32;
            bits_write(&mut d, vcpu, base, if base == 0 { val & 0xffff_0000 } else { val },
                |i| i.pending_latch = true);
            return all_vcpus(&d);
        }
        GIC_DIST_PENDING_CLEAR..=0x2fc => {
            let base = ((word & 0x7f) / 4) * 32;
            bits_write(&mut d, vcpu, base, if base == 0 { val & 0xffff_0000 } else { val },
                |i| i.pending_latch = false);
        }
        GIC_DIST_ACTIVE_SET..=0x37c => {
            let base = ((word & 0x7f) / 4) * 32;
            bits_write(&mut d, vcpu, base, val, |i| i.active = true);
        }
        GIC_DIST_ACTIVE_CLEAR..=0x3fc => {
            let base = ((word & 0x7f) / 4) * 32;
            bits_write(&mut d, vcpu, base, val, |i| i.active = false);
            return all_vcpus(&d);
        }
        GIC_DIST_PRI..=0x7fc => {
            for b in 0..len.min(4) as u32 {
                let intid = off - GIC_DIST_PRI + b;
                // Five priority bits, as the GIC-400 (`VGIC_PRI_BITS`).
                if let Some(i) = d.irq(vcpu, intid) { i.priority = (val >> (8 * b)) as u8 & 0xf8; }
            }
        }
        GIC_DIST_TARGET..=0xbfc => {
            for b in 0..len.min(4) as u32 {
                let intid = off - GIC_DIST_TARGET + b;
                if intid < NR_PRIVATE as u32 { continue; }
                let all = all_vcpus(&d);
                if let Some(i) = d.irq(vcpu, intid) { i.targets = (val >> (8 * b)) as u8 & all; }
            }
            return all_vcpus(&d);
        }
        GIC_DIST_CONFIG..=0xcfc => {
            let base = (word - GIC_DIST_CONFIG) * 4;
            for b in 0..16 {
                let intid = base + b;
                // SGIs are edge, PPIs as they are (read-only on the GIC-400).
                if intid < NR_PRIVATE as u32 { continue; }
                if let Some(i) = d.irq(vcpu, intid) { i.edge = val & (2 << (2 * b)) != 0; }
            }
        }
        GIC_DIST_SOFTINT => return write_sgir(&mut d, vcpu, val),
        GIC_DIST_SGI_PENDING_CLEAR..=0xf1c => {
            let first = word & 0xf;
            for b in 0..4 {
                let sgi = (first + b) as usize;
                let irq = &mut d.private[vcpu as usize % MAX_VCPUS][sgi];
                irq.sources &= !((val >> (8 * b)) as u8);
                if irq.sources == 0 { irq.pending_latch = false; }
            }
        }
        GIC_DIST_SGI_PENDING_SET..=0xf2c => {
            let first = word & 0xf;
            for b in 0..4 {
                let sgi = (first + b) as usize;
                let irq = &mut d.private[vcpu as usize % MAX_VCPUS][sgi];
                irq.sources |= (val >> (8 * b)) as u8;
                if irq.sources != 0 { irq.pending_latch = true; }
            }
            return 1 << vcpu;
        }
        _ => {}
    }
    0
}

fn all_vcpus(d: &Dist) -> u8 {
    ((1u16 << d.vcpus) - 1) as u8
}
