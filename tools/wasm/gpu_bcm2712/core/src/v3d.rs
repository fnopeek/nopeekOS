//! V3D 7.1's jobs as Linux runs them, the module's share of
//! `drivers/gpu/drm/v3d` (rpi-6.18): the interrupts (`v3d_irq`,
//! `v3d_hub_irq`, enable, disable, reset), the caches around a job
//! (`v3d_invalidate_caches` before; `v3d_clean_caches`, the cache-clean
//! job), the job runs of every queue (`v3d_bin_job_run`,
//! `v3d_render_job_run`, `v3d_tfu_job_run`, `v3d_csd_job_run`), the
//! binner's overflow memory, what the timeout handlers compare, and the
//! performance counters (`v3d_perfmon_start/stop`). Power, the SMS, the
//! reset controller and the MMU are the kernel's (`pi5_v3d`).

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
const V3D_CLE_CT0CA: u32 = 0x00110;
const V3D_CLE_CT0RA: u32 = 0x00118;
const V3D_CLE_CT0QTS: u32 = 0x0015c;
const V3D_CLE_CT0QTS_ENABLE: u32 = 1 << 1;
const V3D_CLE_CT0QBA: u32 = 0x00160;
const V3D_CLE_CT1QBA: u32 = 0x00164;
const V3D_CLE_CT0QEA: u32 = 0x00168;
const V3D_CLE_CT1QEA: u32 = 0x0016c;
const V3D_CLE_CT0QMA: u32 = 0x00170;
const V3D_CLE_CT0QMS: u32 = 0x00174;
const V3D_PTB_BPOA: u32 = 0x00308;
const V3D_PTB_BPOS: u32 = 0x0030c;
const V3D_V4_PCTR_0_EN: u32 = 0x00650;
const V3D_V4_PCTR_0_CLR: u32 = 0x00654;
const V3D_PCTR_0_OVERFLOW: u32 = 0x00658;
const V3D_V4_PCTR_0_SRC_0_3: u32 = 0x00660;
const V3D_PCTR_0_PCTR0: u32 = 0x00680;
const V3D_TFU_ICFG: u32 = 0x00708;
const V3D_TFU_ICFG_IOC: u32 = 1 << 0;
const V3D_TFU_IIA: u32 = 0x0070c;
const V3D_TFU_ICA: u32 = 0x00710;
const V3D_TFU_IIS: u32 = 0x00714;
const V3D_TFU_IUA: u32 = 0x00718;
const V3D_V7_TFU_IOC: u32 = 0x0071c;
const V3D_TFU_IOA: u32 = 0x00720;
const V3D_TFU_IOS: u32 = 0x00724;
const V3D_TFU_COEF0: u32 = 0x00728;
const V3D_CSD_CURRENT_CFG4: u32 = 0x00968;
const V3D_ERR_STAT: u32 = 0x00f20;
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

/// `v3d_irq_disable`: everything masked, then what was pending cleared.
/// The caller has no interrupt handler in flight (its fiber is the
/// handler) and drops any overflow work it has queued.
pub fn irq_disable(hw: &mut impl Hw) {
    hw.write(Block::V3dCore, V3D_CTL_INT_MSK_SET, !0);
    hw.write(Block::V3dHub, V3D_HUB_INT_MSK_SET, !0);
    hw.write(Block::V3dCore, V3D_CTL_INT_CLR, CORE_IRQS);
    hw.write(Block::V3dHub, V3D_HUB_INT_CLR, HUB_IRQS);
}

/// `v3d_irq_reset`: after a GPU reset.
pub fn irq_reset(hw: &mut impl Hw) {
    irq_enable(hw);
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

/// What the core interrupt said (`v3d_irq`): which queue's job finished,
/// and whether the binner ran out of memory (edge signalled: it comes
/// again only once memory was added; FLDONE waits until it is cleared).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CoreIrq {
    pub out_of_memory: bool,
    pub bin_done: bool,
    pub render_done: bool,
    pub csd_done: bool,
}

/// `v3d_irq`: the status read and acknowledged.
pub fn core_irq(hw: &mut impl Hw) -> CoreIrq {
    let sts = hw.read(Block::V3dCore, V3D_CTL_INT_STS);
    hw.write(Block::V3dCore, V3D_CTL_INT_CLR, sts);
    CoreIrq {
        out_of_memory: sts & V3D_INT_OUTOMEM != 0,
        bin_done: sts & V3D_INT_FLDONE != 0,
        render_done: sts & V3D_INT_FRDONE != 0,
        csd_done: sts & V3D_INT_CSDDONE != 0,
    }
}

/// What the hub interrupt said (`v3d_hub_irq`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HubIrq {
    pub tfu_done: bool,
    pub write_violation: bool,
    pub pte_invalid: bool,
    pub cap_exceeded: bool,
    pub gmp_violation: bool,
}

impl HubIrq {
    /// An MMU interrupt: the kernel reads the address and clears the error.
    pub fn mmu_fault(&self) -> bool {
        self.write_violation || self.pte_invalid || self.cap_exceeded
    }
}

/// `v3d_hub_irq`: the status read and acknowledged.
pub fn hub_irq(hw: &mut impl Hw) -> HubIrq {
    let sts = hw.read(Block::V3dHub, V3D_HUB_INT_STS);
    hw.write(Block::V3dHub, V3D_HUB_INT_CLR, sts);
    HubIrq {
        tfu_done: sts & V3D_HUB_INT_TFUC != 0,
        write_violation: sts & V3D_HUB_INT_MMU_WRV != 0,
        pte_invalid: sts & V3D_HUB_INT_MMU_PTI != 0,
        cap_exceeded: sts & V3D_HUB_INT_MMU_CAP != 0,
        gmp_violation: sts & V3D_V7_HUB_INT_GMPV != 0,
    }
}

/// `v3d71_axi_ids`: the client behind an MMU fault's AXI ID.
pub fn axi_client(axi_id: u32) -> &'static str {
    const IDS: [(u32, u32, &str); 9] = [
        (0x00, 0x30, "L2T"),
        (0x30, 0x38, "CLE"),
        (0x38, 0x39, "PTB"),
        (0x39, 0x3A, "PSE"),
        (0x3A, 0x3B, "CSD"),
        (0x40, 0x60, "TLB"),
        (0x60, 0x70, "MMU"),
        (0x7C, 0x7E, "TFU"),
        (0x7F, 0x80, "GMP"),
    ];
    let id = axi_id & 0x7f;
    IDS.iter().find(|&&(b, e, _)| id >= b && id < e).map_or("?", |&(_, _, c)| c)
}

/// A bin job (`drm_v3d_submit_cl`'s binner half): the control list from
/// `start` to `end`, the tile-alloc memory and size (`qma`, `qms`) and the
/// tile state (`qts`), 0 when the list sets them itself.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BinJob {
    pub start: u32,
    pub end: u32,
    pub qma: u32,
    pub qms: u32,
    pub qts: u32,
}

/// `v3d_bin_job_run`: last job's overflow memory dropped, the caches
/// invalidated, the list started by writing its end address.
pub fn bin_job_run(hw: &mut impl Hw, job: &BinJob) {
    hw.write(Block::V3dCore, V3D_PTB_BPOS, 0);
    invalidate_caches(hw);
    if job.qma != 0 {
        hw.write(Block::V3dCore, V3D_CLE_CT0QMA, job.qma);
        hw.write(Block::V3dCore, V3D_CLE_CT0QMS, job.qms);
    }
    if job.qts != 0 {
        hw.write(Block::V3dCore, V3D_CLE_CT0QTS, V3D_CLE_CT0QTS_ENABLE | job.qts);
    }
    hw.write(Block::V3dCore, V3D_CLE_CT0QBA, job.start);
    hw.write(Block::V3dCore, V3D_CLE_CT0QEA, job.end);
}

/// `v3d_render_job_run`: the caches invalidated, the render list from
/// `start` to `end` started by its end address.
pub fn render_job_run(hw: &mut impl Hw, start: u32, end: u32) {
    invalidate_caches(hw);
    hw.write(Block::V3dCore, V3D_CLE_CT1QBA, start);
    hw.write(Block::V3dCore, V3D_CLE_CT1QEA, end);
}

/// `v3d_overflow_mem_work` after the buffer is mapped: `size` bytes at V3D
/// address `va` handed to the binner.
pub fn overflow_mem(hw: &mut impl Hw, va: u32, size: u32) {
    hw.write(Block::V3dCore, V3D_PTB_BPOA, va);
    hw.write(Block::V3dCore, V3D_PTB_BPOS, size);
}

/// `drm_v3d_submit_tfu` on V3D 7.1.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TfuJob {
    pub icfg: u32,
    pub iia: u32,
    pub iis: u32,
    pub ica: u32,
    pub iua: u32,
    pub ioa: u32,
    pub ios: u32,
    pub coef: [u32; 4],
    pub ioc: u32,
}

/// `v3d_tfu_job_run`: the input and output registers, all four
/// coefficient words on 7.1, then ICFG with IOC, which starts it. No cache
/// invalidation: the TFU reads and writes memory itself.
pub fn tfu_job_run(hw: &mut impl Hw, job: &TfuJob) {
    hw.write(Block::V3dHub, V3D_TFU_IIA, job.iia);
    hw.write(Block::V3dHub, V3D_TFU_IIS, job.iis);
    hw.write(Block::V3dHub, V3D_TFU_ICA, job.ica);
    hw.write(Block::V3dHub, V3D_TFU_IUA, job.iua);
    hw.write(Block::V3dHub, V3D_TFU_IOA, job.ioa);
    hw.write(Block::V3dHub, V3D_V7_TFU_IOC, job.ioc);
    hw.write(Block::V3dHub, V3D_TFU_IOS, job.ios);
    for (i, &c) in job.coef.iter().enumerate() {
        hw.write(Block::V3dHub, V3D_TFU_COEF0 + 4 * i as u32, c);
    }
    hw.write(Block::V3dHub, V3D_TFU_ICFG, job.icfg | V3D_TFU_ICFG_IOC);
}

/// Where a control list thread is (`CTnCA`, `CTnRA`): `v3d_cl_job_timedout`
/// resets only when neither moved since the last timeout. `q` 0 bin, 1
/// render.
pub fn cl_progress(hw: &mut impl Hw, q: u32) -> (u32, u32) {
    (hw.read(Block::V3dCore, V3D_CLE_CT0CA + 4 * q), hw.read(Block::V3dCore, V3D_CLE_CT0RA + 4 * q))
}

/// The batches left of the running dispatch (`V3D_CSD_CURRENT_CFG4`):
/// `v3d_csd_job_timedout` resets only when it did not change.
pub fn csd_progress(hw: &mut impl Hw) -> u32 {
    hw.read(Block::V3dCore, V3D_CSD_CURRENT_CFG4)
}

/// `V3D_ERR_STAT`, which `v3d_reset` reports.
pub fn err_stat(hw: &mut impl Hw) -> u32 {
    hw.read(Block::V3dCore, V3D_ERR_STAT)
}

/// `v3d_v71_performance_counters`: the name of each counter source.
pub const PERF_COUNTERS_71: [&str; 93] = [
    "cycle-count",
    "core-active",
    "CLE-bin-thread-active-cycles",
    "CLE-render-thread-active-cycles",
    "compute-active-cycles",
    "FEP-valid-primitives-no-rendered-pixels",
    "FEP-valid-primitives-rendered-pixels",
    "FEP-clipped-quads",
    "FEP-valid-quads",
    "TLB-quads-not-passing-stencil-test",
    "TLB-quads-not-passing-z-and-stencil-test",
    "TLB-quads-passing-z-and-stencil-test",
    "TLB-quads-written-to-color-buffer",
    "TLB-partial-quads-written-to-color-buffer",
    "PTB-primitives-need-clipping",
    "PTB-primitives-discarded-outside-viewport",
    "PTB-primitives-binned",
    "PTB-primitives-discarded-reversed",
    "QPU-total-instr-cache-hit",
    "QPU-total-instr-cache-miss",
    "QPU-total-uniform-cache-hit",
    "QPU-total-uniform-cache-miss",
    "TMU-active-cycles",
    "TMU-stalled-cycles",
    "TMU-total-text-quads-access",
    "TMU-cache-x4-active-cycles",
    "TMU-cache-x4-stalled-cycles",
    "TMU-total-text-quads-x4-access",
    "L2T-total-cache-hit",
    "L2T-total-cache-miss",
    "L2T-local",
    "L2T-writeback",
    "L2T-zero",
    "L2T-merge",
    "L2T-fill",
    "L2T-stalls-no-wid",
    "L2T-stalls-no-rid",
    "L2T-stalls-queue-full",
    "L2T-stalls-wrightback",
    "L2T-stalls-mem",
    "L2T-stalls-fill",
    "L2T-hitq",
    "L2T-hitq-full",
    "L2T-stalls-read-data",
    "L2T-TMU-read-hits",
    "L2T-TMU-read-miss",
    "L2T-VCD-read-hits",
    "L2T-VCD-read-miss",
    "L2T-SLC-read-hits",
    "L2T-SLC-read-miss",
    "AXI-writes-seen-watch-0",
    "AXI-reads-seen-watch-0",
    "AXI-writes-stalled-seen-watch-0",
    "AXI-reads-stalled-seen-watch-0",
    "AXI-write-bytes-seen-watch-0",
    "AXI-read-bytes-seen-watch-0",
    "AXI-writes-seen-watch-1",
    "AXI-reads-seen-watch-1",
    "AXI-writes-stalled-seen-watch-1",
    "AXI-reads-stalled-seen-watch-1",
    "AXI-write-bytes-seen-watch-1",
    "AXI-read-bytes-seen-watch-1",
    "core-memory-writes",
    "L2T-memory-writes",
    "PTB-memory-writes",
    "TLB-memory-writes",
    "core-memory-reads",
    "L2T-memory-reads",
    "PTB-memory-reads",
    "PSE-memory-reads",
    "TLB-memory-reads",
    "PTB-memory-words-writes",
    "TLB-memory-words-writes",
    "PSE-memory-words-reads",
    "TLB-memory-words-reads",
    "AXI-read-trans",
    "AXI-write-trans",
    "AXI-read-wait-cycles",
    "AXI-write-wait-cycles",
    "AXI-max-outstanding-reads",
    "AXI-max-outstanding-writes",
    "QPU-wait-bubble",
    "QPU-ic-miss-bubble",
    "QPU-active",
    "QPU-total-active-clk-cycles-fragment-shading",
    "QPU-stalls",
    "QPU-total-clk-cycles-waiting-fragment-shading",
    "QPU-stalls-TMU",
    "QPU-stalls-TLB",
    "QPU-stalls-VPM",
    "QPU-stalls-uniforms",
    "QPU-stalls-SFU",
    "QPU-stalls-other",
];

/// `v3d_perfmon_start`: up to 32 counters, the sources of four in each
/// `PCTR_0_SRC` word, then enabled, cleared, overflow cleared.
pub fn perfmon_start(hw: &mut impl Hw, counters: &[u8]) {
    let n = counters.len().min(32);
    for src in 0..n.div_ceil(4) {
        let mut channel = 0;
        for k in 0..4 {
            let i = src * 4 + k;
            let c = if i < n { counters[i] as u32 } else { 0 };
            channel |= (c & 0xff) << (8 * k);
        }
        hw.write(Block::V3dCore, V3D_V4_PCTR_0_SRC_0_3 + 4 * src as u32, channel);
    }
    let mask = if n == 32 { !0 } else { (1u32 << n) - 1 };
    hw.write(Block::V3dCore, V3D_V4_PCTR_0_EN, mask);
    hw.write(Block::V3dCore, V3D_V4_PCTR_0_CLR, mask);
    hw.write(Block::V3dCore, V3D_PCTR_0_OVERFLOW, mask);
}

/// `v3d_perfmon_stop`: the `values.len()` counters added to `values` when
/// `capture`, then the counters disabled.
pub fn perfmon_stop(hw: &mut impl Hw, values: &mut [u64], capture: bool) {
    if capture {
        for (i, v) in values.iter_mut().enumerate().take(32) {
            *v += hw.read(Block::V3dCore, V3D_PCTR_0_PCTR0 + 4 * i as u32) as u64;
        }
    }
    hw.write(Block::V3dCore, V3D_V4_PCTR_0_EN, 0);
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
