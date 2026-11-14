//! V3D's job runs, interrupts and counters against the register writes of
//! Linux's v3d_sched.c, v3d_irq.c and v3d_perfmon.c (rpi-6.18, V3D 7.1).

use crate::hw::{Block, ClockOp, Hw, Spi};
use crate::v3d::*;
use std::collections::BTreeMap;

#[derive(Default)]
struct Rec {
    vals: BTreeMap<(u8, u32), u32>,
    writes: Vec<(Block, u32, u32)>,
}

fn key(b: Block) -> u8 {
    match b {
        Block::V3dHub => 1,
        Block::V3dCore => 2,
        _ => 0,
    }
}

impl Hw for Rec {
    fn read(&mut self, b: Block, off: u32) -> u32 {
        *self.vals.get(&(key(b), off)).unwrap_or(&0)
    }
    fn write(&mut self, b: Block, off: u32, v: u32) {
        self.writes.push((b, off, v));
    }
    fn now_us(&mut self) -> u64 { 0 }
    fn delay_us(&mut self, _: u64) {}
    fn irq_arm(&mut self, _: Spi) -> u64 { 0 }
    fn irq_wait(&mut self, _: Spi, _: u64, _: u32) -> bool { true }
    fn fw_clock(&mut self, _: u32, _: ClockOp) -> Option<u32> { None }
    fn fw_display_done(&mut self) -> bool { true }
}

fn offs(r: &Rec) -> Vec<(u32, u32)> {
    r.writes.iter().map(|w| (w.1, w.2)).collect()
}

// v3d_invalidate_caches on 7.1: L2TCACTL = L2TFLS | FLM_FLUSH, SLCACTL.
const INVALIDATE: [(u32, u32); 2] = [(0x30, 1), (0x24, 0x0f0f_0f0f)];

#[test]
fn a_bin_job_starts_as_v3d_bin_job_run_starts_it() {
    let mut r = Rec::default();
    bin_job_run(&mut r, &BinJob { start: 0x10_0000, end: 0x10_0400, qma: 0x20_0000, qms: 0x8000, qts: 0x30_0000 });
    let mut want = vec![(0x30c, 0)];
    want.extend(INVALIDATE);
    want.extend([(0x170, 0x20_0000), (0x174, 0x8000), (0x15c, 0x30_0000 | 2), (0x160, 0x10_0000), (0x168, 0x10_0400)]);
    assert_eq!(offs(&r), want);
    assert!(r.writes.iter().all(|w| w.0 == Block::V3dCore));
    // Without tile alloc and state: only the list.
    let mut r = Rec::default();
    bin_job_run(&mut r, &BinJob { start: 4, end: 8, ..Default::default() });
    assert_eq!(&offs(&r)[3..], &[(0x160, 4), (0x168, 8)]);
}

#[test]
fn a_render_job_and_overflow_memory_as_linux_writes_them() {
    let mut r = Rec::default();
    render_job_run(&mut r, 0x40_0000, 0x40_0100);
    let mut want = INVALIDATE.to_vec();
    want.extend([(0x164, 0x40_0000), (0x16c, 0x40_0100)]);
    assert_eq!(offs(&r), want);
    let mut r = Rec::default();
    overflow_mem(&mut r, 0x80_0000, 256 * 1024);
    assert_eq!(offs(&r), vec![(0x308, 0x80_0000), (0x30c, 256 * 1024)]);
}

#[test]
fn a_tfu_job_writes_every_71_register_then_icfg() {
    let mut r = Rec::default();
    let job = TfuJob { icfg: 0x1230, iia: 1, iis: 2, ica: 3, iua: 4, ioa: 5, ios: 6, coef: [7, 8, 9, 10], ioc: 11 };
    tfu_job_run(&mut r, &job);
    assert_eq!(offs(&r), vec![(0x70c, 1), (0x714, 2), (0x710, 3), (0x718, 4), (0x720, 5), (0x71c, 11),
        (0x724, 6), (0x728, 7), (0x72c, 8), (0x730, 9), (0x734, 10), (0x708, 0x1231)]);
    assert!(r.writes.iter().all(|w| w.0 == Block::V3dHub));
}

#[test]
fn interrupts_decode_and_clear_as_v3d_irq_does() {
    let mut r = Rec::default();
    r.vals.insert((2, 0x50), (1 << 6) | (1 << 2) | 1);
    let c = core_irq(&mut r);
    assert_eq!(c, CoreIrq { out_of_memory: true, bin_done: false, render_done: true, csd_done: true });
    assert_eq!(offs(&r), vec![(0x58, (1 << 6) | (1 << 2) | 1)]);
    let mut r = Rec::default();
    r.vals.insert((1, 0x50), (1 << 1) | (1 << 4));
    let h = hub_irq(&mut r);
    assert!(h.tfu_done && h.pte_invalid && h.mmu_fault() && !h.write_violation);
    assert_eq!(axi_client(0x3a), "CSD");
    assert_eq!(axi_client(0x80 | 0x7c), "TFU");
    assert_eq!(axi_client(0x70), "?");
    // v3d_irq_disable: masked, then cleared.
    let mut r = Rec::default();
    irq_disable(&mut r);
    assert_eq!(offs(&r), vec![(0x60, !0), (0x60, !0), (0x58, 0x47), (0x58, 0x7a)]);
}

#[test]
fn counters_are_programmed_four_to_a_source_word() {
    let mut r = Rec::default();
    perfmon_start(&mut r, &[0, 1, 2, 3, 92]);
    assert_eq!(offs(&r), vec![(0x660, 0x0302_0100), (0x664, 92), (0x650, 0x1f), (0x654, 0x1f), (0x658, 0x1f)]);
    let mut r = Rec::default();
    r.vals.insert((2, 0x680), 10);
    r.vals.insert((2, 0x684), 20);
    let mut v = [1u64, 2];
    perfmon_stop(&mut r, &mut v, true);
    assert_eq!(v, [11, 22]);
    assert_eq!(offs(&r), vec![(0x650, 0)]);
    assert_eq!(PERF_COUNTERS_71[0], "cycle-count");
}

mod sched {
    use super::*;
    use crate::v3d_sched::*;

    fn csd(n: u32) -> [u32; 7] { [n << 16, 1 << 16, 1 << 16, 0, 1, 0x1000, 0x2000] }

    fn done(ev: &[Event]) -> Vec<(Fence, bool)> {
        ev.iter().filter_map(|e| match e { Event::Done { fence, ok, .. } => Some((*fence, *ok)), _ => None }).collect()
    }

    #[test]
    fn a_dispatch_runs_then_its_clean_job_after_the_interrupt() {
        let (mut s, mut r, mut ev) = (Sched::new(), Rec::default(), Vec::new());
        let f = s.submit_csd(csd(4), &[]);
        s.pump(&mut r, 0, &mut ev);
        // The CSD started (CFG0 written last), the clean job waits for it.
        assert_eq!(r.writes.last().unwrap().1, 0x930);
        assert!(!s.signaled(f) && ev.is_empty());
        s.irq(CoreIrq { csd_done: true, ..Default::default() }, HubIrq::default(), 10, &mut ev);
        s.pump(&mut r, 10, &mut ev);
        assert_eq!(done(&ev), vec![(f - 1, true), (f, true)]);
        assert!(s.signaled(f) && s.idle());
        assert_eq!(s.jobs, [0, 0, 0, 1, 1]);
        assert_eq!(s.busy_us[3], 10);
    }

    #[test]
    fn a_queue_runs_one_job_at_a_time_and_dependencies_hold_others_back() {
        let (mut s, mut r, mut ev) = (Sched::new(), Rec::default(), Vec::new());
        let a = s.submit(Work::Csd(csd(1)), &[]);
        let b = s.submit(Work::Csd(csd(2)), &[]);
        let t = s.submit(Work::Tfu(TfuJob::default()), &[b]);
        s.pump(&mut r, 0, &mut ev);
        let starts = |r: &Rec| r.writes.iter().filter(|w| w.1 == 0x930 || w.1 == 0x708).count();
        assert_eq!(starts(&r), 1);
        s.irq(CoreIrq { csd_done: true, ..Default::default() }, HubIrq::default(), 1, &mut ev);
        s.pump(&mut r, 1, &mut ev);
        assert_eq!(starts(&r), 2);
        assert!(s.signaled(a) && !s.signaled(t));
        s.irq(CoreIrq { csd_done: true, ..Default::default() }, HubIrq::default(), 2, &mut ev);
        s.pump(&mut r, 2, &mut ev);
        assert_eq!(starts(&r), 3);
        s.irq(CoreIrq::default(), HubIrq { tfu_done: true, ..Default::default() }, 3, &mut ev);
        assert!(s.signaled(t) && s.idle());
    }

    #[test]
    fn a_cl_submission_bins_then_renders_and_overflow_goes_with_the_render() {
        let (mut s, mut r, mut ev) = (Sched::new(), Rec::default(), Vec::new());
        let f = s.submit_cl(Some(BinJob { start: 1, end: 2, ..Default::default() }), (3, 4), true, &[]);
        s.pump(&mut r, 0, &mut ev);
        assert_eq!(r.writes.last().unwrap().1, 0x168);
        s.irq(CoreIrq { out_of_memory: true, ..Default::default() }, HubIrq::default(), 1, &mut ev);
        assert_eq!(ev, vec![Event::OutOfMemory]);
        assert!(s.attach_overflow(7));
        s.irq(CoreIrq { bin_done: true, ..Default::default() }, HubIrq::default(), 2, &mut ev);
        s.pump(&mut r, 2, &mut ev);
        assert_eq!(r.writes.last().unwrap().1, 0x16c);
        assert!(!s.attach_overflow(8));
        ev.clear();
        s.irq(CoreIrq { render_done: true, ..Default::default() }, HubIrq::default(), 3, &mut ev);
        s.pump(&mut r, 3, &mut ev);
        assert!(matches!(&ev[0], Event::Done { unref, .. } if unref == &vec![7]));
        assert!(s.signaled(f) && s.idle());
    }

    #[test]
    fn a_job_that_makes_progress_gets_more_time_one_that_does_not_resets() {
        let (mut s, mut r, mut ev) = (Sched::new(), Rec::default(), Vec::new());
        let a = s.submit(Work::Csd(csd(1)), &[]);
        let after = s.submit(Work::Tfu(TfuJob::default()), &[a]);
        s.pump(&mut r, 0, &mut ev);
        // CURRENT_CFG4 moved since the start: another timeout.
        r.vals.insert((2, 0x968), 5);
        s.check_timeouts(&mut r, JOB_TIMEOUT_US, &mut ev);
        assert!(ev.is_empty());
        assert_eq!(s.next_deadline(), Some(2 * JOB_TIMEOUT_US));
        // No progress since: every queue stops, a reset is asked for.
        r.vals.insert((2, 0xf20), 0x40);
        s.check_timeouts(&mut r, 2 * JOB_TIMEOUT_US, &mut ev);
        assert_eq!(ev, vec![Event::Reset { queue: Queue::Csd, err_stat: 0x40 }]);
        let n = r.writes.len();
        s.pump(&mut r, 2 * JOB_TIMEOUT_US, &mut ev);
        assert_eq!(r.writes.len(), n);
        ev.clear();
        // The hung job fails; what depends on it fails without running.
        s.reset_done(Queue::Csd, 2 * JOB_TIMEOUT_US, &mut ev);
        s.pump(&mut r, 2 * JOB_TIMEOUT_US, &mut ev);
        assert_eq!(done(&ev), vec![(a, false), (after, false)]);
        assert_eq!(r.writes.len(), n);
        assert!(s.idle());
    }

    #[test]
    fn after_a_reset_the_innocent_jobs_run_again_from_the_start() {
        let (mut s, mut r, mut ev) = (Sched::new(), Rec::default(), Vec::new());
        let t = s.submit(Work::Tfu(TfuJob::default()), &[]);
        let c = s.submit(Work::Csd(csd(1)), &[]);
        s.pump(&mut r, 0, &mut ev);
        // The TFU has nothing to compare: past its timeout it resets.
        s.check_timeouts(&mut r, JOB_TIMEOUT_US, &mut ev);
        assert!(matches!(ev[0], Event::Reset { queue: Queue::Tfu, .. }));
        ev.clear();
        s.reset_done(Queue::Tfu, JOB_TIMEOUT_US, &mut ev);
        assert_eq!(done(&ev), vec![(t, false)]);
        let starts = r.writes.iter().filter(|w| w.1 == 0x930).count();
        s.pump(&mut r, JOB_TIMEOUT_US, &mut ev);
        assert_eq!(r.writes.iter().filter(|w| w.1 == 0x930).count(), starts + 1);
        assert!(!s.signaled(c));
    }
}
