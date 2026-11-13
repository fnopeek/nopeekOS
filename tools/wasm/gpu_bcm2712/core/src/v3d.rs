//! V3D 7.1's compute jobs as Linux runs them, the module's share of
//! `drivers/gpu/drm/v3d` (rpi-6.18): `v3d_irq_enable`, the caches around a
//! job (`v3d_invalidate_caches` before, `v3d_clean_caches` after, as the
//! cache-clean job a CSD submission with `DRM_V3D_SUBMIT_CL_FLUSH_CACHE`
//! queues), `v3d_csd_job_run` and the core and hub interrupts. Power, the
//! SMS and the MMU are the kernel's (`pi5_v3d`).

use crate::hw::{Block, Hw};

// v3d_regs.h, V3D 7.1.
const V3D_HUB_INT_STS: u32 = 0x00050;
const V3D_HUB_INT_CLR: u32 = 0x00058;
const V3D_HUB_INT_MSK_SET: u32 = 0x00060;
const V3D_HUB_INT_MSK_CLR: u32 = 0x00064;
const V3D_HUB_INT_MMU_WRV: u32 = 1 << 5;
const V3D_HUB_INT_MMU_PTI: u32 = 1 << 4;
const V3D_HUB_INT_MMU_CAP: u32 = 1 << 3;
const V3D_HUB_INT_TFUC: u32 = 1 << 1;
const V3D_V7_HUB_INT_GMPV: u32 = 1 << 6;
const V3D_CTL_INT_STS: u32 = 0x00050;
const V3D_CTL_INT_CLR: u32 = 0x00058;
const V3D_CTL_INT_MSK_SET: u32 = 0x00060;
const V3D_CTL_INT_MSK_CLR: u32 = 0x00064;
const V3D_INT_CSDDONE: u32 = 1 << 6;
const V3D_INT_OUTOMEM: u32 = 1 << 2;
const V3D_INT_FLDONE: u32 = 1 << 1;
const V3D_INT_FRDONE: u32 = 1 << 0;
const V3D_CTL_SLCACTL: u32 = 0x00024;
const V3D_CTL_L2TCACTL: u32 = 0x00030;
const V3D_L2TCACTL_TMUWCF: u32 = 1 << 8;
const V3D_L2TCACTL_FLM_FLUSH: u32 = 0;
const V3D_L2TCACTL_FLM_CLEAN: u32 = 2;
const V3D_L2TCACTL_FLM_SHIFT: u32 = 1;
const V3D_L2TCACTL_L2TFLS: u32 = 1 << 0;
const V3D_CSD_QUEUED_CFG0: u32 = 0x00930;
const V3D_V7_CSD_QUEUED_CFG7: u32 = 0x0094c;
const V3D_CSD_QUEUED_CFG0_NUM_WGS_MASK: u32 = 0xffff << 16;

/// `V3D_CORE_IRQS(71)`.
const CORE_IRQS: u32 = V3D_INT_OUTOMEM | V3D_INT_FLDONE | V3D_INT_FRDONE | V3D_INT_CSDDONE;
/// `V3D_HUB_IRQS(71)`.
const HUB_IRQS: u32 = V3D_HUB_INT_MMU_WRV | V3D_HUB_INT_MMU_PTI | V3D_HUB_INT_MMU_CAP | V3D_HUB_INT_TFUC
    | V3D_V7_HUB_INT_GMPV;

/// `wait_for(cond, 100)`: poll every 10 us for up to 100 ms.
fn wait_for(hw: &mut impl Hw, b: Block, off: u32, done: impl Fn(u32) -> bool) -> bool {
    let end = hw.now_us() + 100_000;
    loop {
        if done(hw.read(b, off)) {
            return true;
        }
        if hw.now_us() >= end {
            return done(hw.read(b, off));
        }
        hw.delay_us(10);
    }
}

/// `v3d_irq_enable` for one core.
pub fn irq_enable(hw: &mut impl Hw) {
    hw.write(Block::V3dCore, V3D_CTL_INT_CLR, CORE_IRQS);
    hw.write(Block::V3dHub, V3D_HUB_INT_CLR, HUB_IRQS);
    hw.write(Block::V3dCore, V3D_CTL_INT_MSK_SET, !CORE_IRQS);
    hw.write(Block::V3dCore, V3D_CTL_INT_MSK_CLR, CORE_IRQS);
    hw.write(Block::V3dHub, V3D_HUB_INT_MSK_SET, !HUB_IRQS);
    hw.write(Block::V3dHub, V3D_HUB_INT_MSK_CLR, HUB_IRQS);
}

/// `v3d_irq_disable`.
pub fn irq_disable(hw: &mut impl Hw) {
    hw.write(Block::V3dCore, V3D_CTL_INT_MSK_SET, !0);
    hw.write(Block::V3dHub, V3D_HUB_INT_MSK_SET, !0);
}

/// `v3d_invalidate_caches` on V3D 7.1: no L3 and no L2C; the texture L2
/// flushed (L2T accesses stall until it is done), the slice caches
/// invalidated.
pub fn invalidate_caches(hw: &mut impl Hw) {
    hw.write(Block::V3dCore, V3D_CTL_L2TCACTL,
        V3D_L2TCACTL_L2TFLS | (V3D_L2TCACTL_FLM_FLUSH << V3D_L2TCACTL_FLM_SHIFT));
    hw.write(Block::V3dCore, V3D_CTL_SLCACTL, (0xf << 24) | (0xf << 16) | (0xf << 8) | 0xf);
}

/// `v3d_clean_caches` on V3D 7.1: the TMU write combiner flushed, then the
/// texture L2 cleaned, both waited for. False on a timeout.
pub fn clean_caches(hw: &mut impl Hw) -> bool {
    hw.write(Block::V3dCore, V3D_CTL_L2TCACTL, V3D_L2TCACTL_TMUWCF);
    let wcf = wait_for(hw, Block::V3dCore, V3D_CTL_L2TCACTL, |v| v & V3D_L2TCACTL_TMUWCF == 0);
    hw.write(Block::V3dCore, V3D_CTL_L2TCACTL,
        V3D_L2TCACTL_L2TFLS | (V3D_L2TCACTL_FLM_CLEAN << V3D_L2TCACTL_FLM_SHIFT));
    let clean = wait_for(hw, Block::V3dCore, V3D_CTL_L2TCACTL, |v| v & V3D_L2TCACTL_L2TFLS == 0);
    wcf && clean
}

/// `v3d_csd_job_run`: the caches invalidated, CFG1..CFG6, CFG7 left 0,
/// then CFG0, which starts the job. False (nothing run) when an axis has
/// no workgroups.
pub fn csd_job_run(hw: &mut impl Hw, cfg: &[u32; 7]) -> bool {
    if cfg[..3].iter().any(|c| c & V3D_CSD_QUEUED_CFG0_NUM_WGS_MASK == 0) {
        return false;
    }
    invalidate_caches(hw);
    for (i, &c) in cfg.iter().enumerate().skip(1) {
        hw.write(Block::V3dCore, V3D_CSD_QUEUED_CFG0 + 4 * i as u32, c);
    }
    hw.write(Block::V3dCore, V3D_V7_CSD_QUEUED_CFG7, 0);
    hw.write(Block::V3dCore, V3D_CSD_QUEUED_CFG0, cfg[0]);
    true
}

/// What the core interrupt said (`v3d_irq`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CoreIrq {
    pub csd_done: bool,
    /// The binner ran out of memory: not a compute job's.
    pub out_of_memory: bool,
}

/// `v3d_irq`: the status read and cleared.
pub fn core_irq(hw: &mut impl Hw) -> CoreIrq {
    let sts = hw.read(Block::V3dCore, V3D_CTL_INT_STS);
    hw.write(Block::V3dCore, V3D_CTL_INT_CLR, sts);
    CoreIrq { csd_done: sts & V3D_INT_CSDDONE != 0, out_of_memory: sts & V3D_INT_OUTOMEM != 0 }
}

/// What the hub interrupt said (`v3d_hub_irq`): an MMU fault aborted the
/// job (the address is in the kernel's registers), or a GMP violation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HubIrq {
    pub mmu_fault: bool,
    pub gmp_violation: bool,
}

pub fn hub_irq(hw: &mut impl Hw) -> HubIrq {
    let sts = hw.read(Block::V3dHub, V3D_HUB_INT_STS);
    hw.write(Block::V3dHub, V3D_HUB_INT_CLR, sts);
    HubIrq {
        mmu_fault: sts & (V3D_HUB_INT_MMU_WRV | V3D_HUB_INT_MMU_PTI | V3D_HUB_INT_MMU_CAP) != 0,
        gmp_violation: sts & V3D_V7_HUB_INT_GMPV != 0,
    }
}

/// `v3d_csd_choose_workgroups_per_supergroup` (Mesa `v3d_util.c`).
pub fn workgroups_per_supergroup(can_use_supergroups: bool, has_tsy_barrier: bool, threads: u32, qpu_count: u32,
                                 num_wgs: u32, wg_size: u32) -> u32 {
    if !can_use_supergroups || wg_size % 16 == 0 {
        return 1;
    }
    let mut max_batches_per_sg = wg_size;
    let mut max_wgs_per_sg = 16;
    if has_tsy_barrier {
        let max_qpu_threads = qpu_count * threads;
        max_batches_per_sg = max_batches_per_sg.min(max_qpu_threads / 2);
        max_wgs_per_sg = max_batches_per_sg * 16 / wg_size;
    }
    let mut best_wgs_per_sg = 1;
    let mut best_unused_lanes = 16;
    for wgs_per_sg in 1..=max_wgs_per_sg {
        if wgs_per_sg > num_wgs {
            return best_wgs_per_sg;
        }
        let batch_size = wgs_per_sg * wg_size;
        if batch_size > max_batches_per_sg * 16 {
            return best_wgs_per_sg;
        }
        let unused_lanes = (16 - batch_size % 16) & 15;
        if unused_lanes < best_unused_lanes {
            best_wgs_per_sg = wgs_per_sg;
            best_unused_lanes = unused_lanes;
        }
        if unused_lanes == 0 {
            return best_wgs_per_sg;
        }
    }
    best_wgs_per_sg
}

/// The program a dispatch runs, as `cmd_buffer_create_csd_job` needs it.
#[derive(Clone, Copy, Debug)]
pub struct CsdProgram {
    pub threads: u32,
    pub single_seg: bool,
    pub local_size: [u32; 3],
    pub can_use_supergroups: bool,
    pub has_control_barrier: bool,
}

/// `cmd_buffer_create_csd_job`'s CFG words for V3D 7.1 rev `rev`: `grid`
/// workgroups, the program at V3D address `code`, its uniform stream at
/// `uniforms`.
pub fn csd_cfg(p: &CsdProgram, rev: u32, qpu_count: u32, grid: [u32; 3], code: u32, uniforms: u32) -> [u32; 7] {
    let wg_size = p.local_size[0] * p.local_size[1] * p.local_size[2];
    let num_wgs = grid[0] * grid[1] * grid[2];
    let wgs_per_sg = workgroups_per_supergroup(p.can_use_supergroups, p.has_control_barrier, p.threads,
        qpu_count, num_wgs, wg_size);
    let batches_per_sg = (wgs_per_sg * wg_size).div_ceil(16);
    let whole_sgs = num_wgs / wgs_per_sg;
    let rem_wgs = num_wgs - whole_sgs * wgs_per_sg;
    let num_batches = batches_per_sg * whole_sgs + (rem_wgs * wg_size).div_ceil(16);
    [
        grid[0] << 16,
        grid[1] << 16,
        grid[2] << 16,
        ((wgs_per_sg & 0xf) << 8) | ((batches_per_sg - 1) << 12) | (wg_size & 0xff),
        // V3D 7.1.6 and later take the number of batches, not minus one.
        if rev >= 6 { num_batches } else { num_batches - 1 },
        code | if p.single_seg { 1 << 1 } else { 0 } | if p.threads == 4 { 1 } else { 0 },
        uniforms,
    ]
}
