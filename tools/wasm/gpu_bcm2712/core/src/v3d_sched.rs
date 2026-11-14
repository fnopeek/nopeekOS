//! V3D's job queues as Linux runs them: `drm_sched` with the v3d backend
//! (`v3d_sched.c`, `v3d_submit.c`, `v3d_irq.c`, rpi-6.18). One queue per
//! hardware unit, each runs its jobs in order and one at a time; a job
//! waits for the fences it depends on; its fence signals from the unit's
//! interrupt; a job that runs past its timeout without progress resets
//! the GPU, after which the guilty job fails and the others run again.
//!
//! The caller owns memory, power and the kernel's share of a reset: it
//! submits jobs, feeds the interrupts in (`irq`), asks for work to start
//! (`pump`) and for timeouts (`check_timeouts`), and acts on `Event`s.

use crate::hw::Hw;
use crate::v3d::{self, BinJob, CoreIrq, HubIrq, TfuJob};
use alloc::collections::VecDeque;
use alloc::vec::Vec;

/// `enum v3d_queue` without the CPU queue (its jobs are CPU writes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Queue {
    Bin = 0,
    Render = 1,
    Tfu = 2,
    Csd = 3,
    CacheClean = 4,
}

const QUEUES: usize = 5;
/// Failed fences remembered for `failed`.
const FAILED_KEPT: usize = 64;
const ALL: [Queue; QUEUES] = [Queue::Bin, Queue::Render, Queue::Tfu, Queue::Csd, Queue::CacheClean];

/// `.timeout = msecs_to_jiffies(500)` of every v3d scheduler.
pub const JOB_TIMEOUT_US: u64 = 500_000;

/// A fence: the job's number, signalled once it finished (or failed).
pub type Fence = u64;

/// What a job runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Work {
    Bin(BinJob),
    Render { start: u32, end: u32 },
    Tfu(TfuJob),
    /// The CSD's seven QUEUED_CFG words.
    Csd([u32; 7]),
    /// `v3d_cache_clean_job_run`.
    CacheClean,
}

impl Work {
    fn queue(&self) -> Queue {
        match self {
            Work::Bin(_) => Queue::Bin,
            Work::Render { .. } => Queue::Render,
            Work::Tfu(_) => Queue::Tfu,
            Work::Csd(_) => Queue::Csd,
            Work::CacheClean => Queue::CacheClean,
        }
    }
}

struct Job {
    fence: Fence,
    work: Work,
    deps: Vec<Fence>,
    /// Overflow buffers the binner was given for this job's scene, freed
    /// with its render job (`unref_list`); the caller's handles.
    unref: Vec<u32>,
}

struct Active {
    job: Job,
    started_us: u64,
    deadline_us: u64,
    /// What the timeout handler saw last (`timedout_ctca/ctra`,
    /// `timedout_batches`).
    progress: (u32, u32),
}

/// What the caller has to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// A fence signalled; `ok` false when its job failed (a reset found it
    /// guilty, or it could not run). `unref` are overflow buffers to free.
    Done { fence: Fence, ok: bool, us: u64, unref: Vec<u32> },
    /// The binner ran out of memory: allocate 256 KiB, map it, call
    /// `overflow_mem` (`v3d_overflow_mem_work`).
    OutOfMemory,
    /// An MMU fault (the kernel reads and clears it) or a GMP violation.
    MmuFault { write_violation: bool, pte_invalid: bool, cap_exceeded: bool },
    GmpViolation,
    /// A job hung: `v3d_irq_disable`, the kernel's reset, `v3d_irq_reset`,
    /// then `reset_done`.
    Reset { queue: Queue, err_stat: u32 },
}

/// The queues, their running jobs, and every fence's state.
pub struct Sched {
    pending: [VecDeque<Job>; QUEUES],
    active: [Option<Active>; QUEUES],
    next: Fence,
    /// Fences submitted and not yet signalled.
    open: Vec<Fence>,
    /// The last fences that signalled with an error: a job that depends on
    /// one fails without running, as drm_sched passes a dependency's error
    /// on to the job's finished fence.
    failed: VecDeque<Fence>,
    /// `drm_sched_stop` until the reset is done.
    stopped: bool,
    /// Per queue: jobs finished and time on the unit (`v3d_stats`).
    pub jobs: [u64; QUEUES],
    pub busy_us: [u64; QUEUES],
}

impl Default for Sched {
    fn default() -> Self {
        Self::new()
    }
}

impl Sched {
    pub fn new() -> Sched {
        Sched {
            pending: [const { VecDeque::new() }; QUEUES],
            active: [const { None }; QUEUES],
            next: 1,
            open: Vec::new(),
            failed: VecDeque::new(),
            stopped: false,
            jobs: [0; QUEUES],
            busy_us: [0; QUEUES],
        }
    }

    /// `v3d_push_job`: queued behind its queue's jobs, run once `deps`
    /// signalled. Its fence.
    pub fn submit(&mut self, work: Work, deps: &[Fence]) -> Fence {
        let fence = self.next;
        self.next += 1;
        self.open.push(fence);
        self.pending[work.queue() as usize].push_back(Job { fence, work, deps: deps.to_vec(), unref: Vec::new() });
        fence
    }

    /// `v3d_setup_csd_jobs_and_bos` with `v3d_submit_csd_ioctl`: the
    /// dispatch, and the cache-clean job that waits for it. The clean
    /// job's fence, which is the submission's.
    pub fn submit_csd(&mut self, cfg: [u32; 7], deps: &[Fence]) -> Fence {
        let csd = self.submit(Work::Csd(cfg), deps);
        self.submit(Work::CacheClean, &[csd])
    }

    /// `v3d_submit_cl_ioctl`: the bin job, the render job that waits for
    /// it, and with `flush` a cache-clean job after the render. The last
    /// job's fence.
    pub fn submit_cl(&mut self, bin: Option<BinJob>, render: (u32, u32), flush: bool, deps: &[Fence]) -> Fence {
        let mut rdeps = deps.to_vec();
        if let Some(b) = bin {
            rdeps = alloc::vec![self.submit(Work::Bin(b), deps)];
        }
        let r = self.submit(Work::Render { start: render.0, end: render.1 }, &rdeps);
        if flush { self.submit(Work::CacheClean, &[r]) } else { r }
    }

    /// Has `f` signalled?
    pub fn signaled(&self, f: Fence) -> bool {
        f < self.next && !self.open.contains(&f)
    }

    /// Did `f` signal with an error (among the last failures)?
    pub fn failed(&self, f: Fence) -> bool {
        self.failed.contains(&f)
    }

    /// No job queued or running.
    pub fn idle(&self) -> bool {
        self.open.is_empty()
    }

    /// The earliest timeout of a running job.
    pub fn next_deadline(&self) -> Option<u64> {
        self.active.iter().flatten().map(|a| a.deadline_us).min()
    }

    fn signal(&mut self, a: Active, ok: bool, now_us: u64, out: &mut Vec<Event>) {
        let q = a.job.work.queue() as usize;
        let us = now_us.saturating_sub(a.started_us);
        self.jobs[q] += 1;
        self.busy_us[q] += us;
        self.open.retain(|&f| f != a.job.fence);
        if !ok {
            if self.failed.len() == FAILED_KEPT {
                self.failed.pop_front();
            }
            self.failed.push_back(a.job.fence);
        }
        out.push(Event::Done { fence: a.job.fence, ok, us, unref: a.job.unref });
    }

    /// `drm_sched_main`: every queue whose unit is free and whose next job
    /// has its fences starts that job (`run_job`).
    pub fn pump(&mut self, hw: &mut impl Hw, now_us: u64, out: &mut Vec<Event>) {
        if self.stopped {
            return;
        }
        loop {
            let mut started = false;
            for q in ALL {
                let i = q as usize;
                if self.active[i].is_some() {
                    continue;
                }
                let ready = self.pending[i].front().is_some_and(|j| j.deps.iter().all(|&d| self.signaled(d)));
                if !ready {
                    continue;
                }
                // A dependency that signalled with an error does not cancel
                // the job: drm_sched cancels only an entity marked guilty,
                // and v3d's entities have no guilty flag.
                let job = self.pending[i].pop_front().expect("a front job");
                let runs = match &job.work {
                    Work::Bin(b) => { v3d::bin_job_run(hw, b); true }
                    Work::Render { start, end } => { v3d::render_job_run(hw, *start, *end); true }
                    Work::Tfu(t) => { v3d::tfu_job_run(hw, t); true }
                    Work::Csd(cfg) => v3d::csd_job_run(hw, cfg),
                    Work::CacheClean => {
                        // Synchronous: no fence to wait on.
                        let ok = v3d::clean_caches(hw);
                        let a = Active { job, started_us: now_us, deadline_us: now_us, progress: (0, 0) };
                        self.signal(a, ok, now_us, out);
                        started = true;
                        continue;
                    }
                };
                let a = Active { job, started_us: now_us, deadline_us: now_us + JOB_TIMEOUT_US, progress: (0, 0) };
                if runs {
                    self.active[i] = Some(a);
                } else {
                    // v3d_csd_job_run with no workgroups: nothing ran, the
                    // fence signals at once.
                    self.signal(a, true, now_us, out);
                }
                started = true;
            }
            if !started {
                return;
            }
        }
    }

    /// `v3d_irq` and `v3d_hub_irq` after the caller read and acknowledged
    /// them: the finished queues' fences signal.
    pub fn irq(&mut self, core: CoreIrq, hub: HubIrq, now_us: u64, out: &mut Vec<Event>) {
        if core.out_of_memory {
            out.push(Event::OutOfMemory);
        }
        for (done, q) in [(core.bin_done, Queue::Bin), (core.render_done, Queue::Render),
                          (core.csd_done, Queue::Csd), (hub.tfu_done, Queue::Tfu)] {
            if done {
                if let Some(a) = self.active[q as usize].take() {
                    self.signal(a, true, now_us, out);
                }
            }
        }
        if hub.mmu_fault() {
            out.push(Event::MmuFault {
                write_violation: hub.write_violation,
                pte_invalid: hub.pte_invalid,
                cap_exceeded: hub.cap_exceeded,
            });
        }
        if hub.gmp_violation {
            out.push(Event::GmpViolation);
        }
    }

    /// `v3d_overflow_mem_work` found a bin job running: the buffer `handle`
    /// goes with its render job. False when no bin job runs (the caller
    /// frees it; Linux drops it).
    pub fn attach_overflow(&mut self, handle: u32) -> bool {
        let Some(bin) = self.active[Queue::Bin as usize].as_ref() else { return false };
        let bin_fence = bin.job.fence;
        let render = self.pending[Queue::Render as usize].iter_mut()
            .chain(self.active[Queue::Render as usize].as_mut().map(|a| &mut a.job))
            .find(|j| j.deps.contains(&bin_fence));
        match render {
            Some(r) => { r.unref.push(handle); true }
            None => false,
        }
    }

    /// The timeout handlers (`v3d_cl_job_timedout`, `v3d_tfu_job_timedout`,
    /// `v3d_csd_job_timedout`): a job past its deadline that made progress
    /// gets another timeout; one that did not stops every queue and asks
    /// for a reset.
    pub fn check_timeouts(&mut self, hw: &mut impl Hw, now_us: u64, out: &mut Vec<Event>) {
        if self.stopped {
            return;
        }
        for q in ALL {
            let i = q as usize;
            let Some(a) = self.active[i].as_mut() else { continue };
            if now_us < a.deadline_us {
                continue;
            }
            let progress = match q {
                Queue::Bin | Queue::Render => Some(v3d::cl_progress(hw, i as u32)),
                Queue::Csd => Some((v3d::csd_progress(hw), 0)),
                Queue::Tfu | Queue::CacheClean => None,
            };
            if let Some(p) = progress {
                if p != a.progress {
                    a.progress = p;
                    a.deadline_us = now_us + JOB_TIMEOUT_US;
                    continue;
                }
            }
            // v3d_gpu_reset_for_timeout: drm_sched_stop on every queue.
            self.stopped = true;
            out.push(Event::Reset { queue: q, err_stat: v3d::err_stat(hw) });
            return;
        }
    }

    /// After the reset: `drm_sched_increase_karma` failed the hung job
    /// (`guilty`), `drm_sched_resubmit_jobs` runs the others again from
    /// the start, `drm_sched_start` unblocks the queues.
    pub fn reset_done(&mut self, guilty: Queue, now_us: u64, out: &mut Vec<Event>) {
        for q in ALL {
            let i = q as usize;
            let Some(a) = self.active[i].take() else { continue };
            if q == guilty {
                self.signal(a, false, now_us, out);
            } else {
                self.pending[i].push_front(a.job);
            }
        }
        self.stopped = false;
    }
}
