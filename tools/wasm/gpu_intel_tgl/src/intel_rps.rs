//! gt/intel_rps.c (Linux 6.18.55): render P-states (frequency control)
//! on Tiger Lake, without GuC SLPC.
//!
//! On TGL the frequency is steered by the RPS timer from engine busyness
//! (`has_busy_stats`), and raised at once for a waiter (`intel_rps_boost`).
//! The module has no timers or workqueues: `intel_rps_run_pending` runs
//! the due timer and the queued work, and the caller sleeps no longer
//! than `intel_rps_timer_due`.

use crate::i915_drv::drm_i915_private;
use crate::intel_gt::intel_gt;
use crate::intel_gt_pm_irq::{gen6_gt_pm_disable_irq, gen6_gt_pm_enable_irq};
use crate::linux::{self, Line};
use crate::regs::*;

/// `struct intel_rps`, as far as the hardware side needs it. Frequencies
/// are in hardware units (16.67 MHz on gen9+).
#[derive(Clone, Copy, Default)]
pub struct intel_rps {
    pub enabled: bool,
    pub active: bool,
    pub interrupts: bool,
    pub timer: bool,

    pub pm_intrmsk_mbz: u32,
    pub pm_events: u32,
    pub pm_iir: u32,

    pub min_freq_softlimit: u8,
    pub max_freq_softlimit: u8,
    pub cur_freq: u8,
    pub last_freq: u8,
    pub min_freq: u8,
    pub boost_freq: u8,
    pub idle_freq: u8,
    pub efficient_freq: u8,
    pub rp1_freq: u8,
    pub rp0_freq: u8,
    pub max_freq: u8,
    pub last_adj: i32,

    /// `power.mode`: LOW_POWER, BETWEEN or HIGH_POWER; -1 forces a write.
    pub power_mode: i32,
    pub up_threshold: u32,
    pub down_threshold: u32,
    pub interactive: bool,

    /// `rps->num_waiters`, `rps->boosts`.
    pub num_waiters: u32,
    pub boosts: u32,
    /// `rps->pm_timestamp` (ns), `rps->pm_interval` (ms).
    pub pm_timestamp: u64,
    pub pm_interval: u32,
    /// `rps->timer` armed for this `now_us`; `rps->work` queued.
    pub timer_due: Option<u64>,
    pub work_queued: bool,
}

/// `BUSY_MAX_EI`, ms.
const BUSY_MAX_EI: u32 = 20;

/// `struct intel_rps_freq_caps`.
struct intel_rps_freq_caps {
    rp0_freq: u8,
    rp1_freq: u8,
    min_freq: u8,
}

/// `rps_pm_sanitize_mask`.
fn rps_pm_sanitize_mask(rps: &intel_rps, mask: u32) -> u32 {
    mask & !rps.pm_intrmsk_mbz
}

/// `rps_pm_mask`.
fn rps_pm_mask(rps: &intel_rps, val: u8) -> u32 {
    let mut mask = 0;
    if val > rps.min_freq_softlimit {
        mask |= GEN6_PM_RP_UP_EI_EXPIRED | GEN6_PM_RP_DOWN_THRESHOLD | GEN6_PM_RP_DOWN_TIMEOUT;
    }
    if val < rps.max_freq_softlimit {
        mask |= GEN6_PM_RP_UP_EI_EXPIRED | GEN6_PM_RP_UP_THRESHOLD;
    }
    mask &= rps.pm_events;
    rps_pm_sanitize_mask(rps, !mask)
}

/// `rps_enable_interrupts`. `rps_reset_ei` clears the software EI sample.
fn rps_enable_interrupts(i915: &mut drm_i915_private, gt: &mut intel_gt) {
    let events = gt.rps.pm_events;
    gen6_gt_pm_enable_irq(i915, gt, events);
    i915.uncore.intel_uncore_write(GEN6_PMINTRMSK, rps_pm_mask(&gt.rps, gt.rps.last_freq));
}

/// `gen11_rps_reset_interrupts`.
fn gen11_rps_reset_interrupts(i915: &mut drm_i915_private) {
    while crate::intel_gt_irq::gen11_gt_reset_one_iir(i915, 0, GEN11_GTPM) {}
}

/// `rps_reset_interrupts`.
fn rps_reset_interrupts(i915: &mut drm_i915_private, gt: &mut intel_gt) {
    gen11_rps_reset_interrupts(i915);
    gt.rps.pm_iir = 0;
}

/// `rps_disable_interrupts`. `intel_synchronize_irq` and the work cancel
/// have nothing to wait for in a single-threaded module.
fn rps_disable_interrupts(i915: &mut drm_i915_private, gt: &mut intel_gt) {
    i915.uncore.intel_uncore_write(GEN6_PMINTRMSK, rps_pm_sanitize_mask(&gt.rps, !0));
    gen6_gt_pm_disable_irq(i915, gt, GEN6_PM_RPS_EVENTS);
    rps_reset_interrupts(i915, gt);
}

/// `intel_gt_ns_to_pm_interval` (gt/intel_gt_clock_utils.c).
fn intel_gt_ns_to_pm_interval(gt: &intel_gt, ns: u64) -> u32 {
    let clocks = ns * gt.clock_frequency as u64 / 1_000_000_000;
    clocks.div_ceil(16) as u32
}

/// `rps_set_power`: evaluation intervals and thresholds per power mode.
fn rps_set_power(i915: &mut drm_i915_private, gt: &mut intel_gt, new_power: i32) {
    if new_power == gt.rps.power_mode {
        return;
    }

    // Units of 1280 ns.
    let (ei_up, ei_down): (u64, u64) = match new_power as u32 {
        LOW_POWER => (16000, 32000),
        BETWEEN => (13000, 32000),
        HIGH_POWER => (10000, 32000),
        _ => (0, 0),
    };

    let up = gt.rps.up_threshold as u64;
    let down = gt.rps.down_threshold as u64;
    let up_ei = intel_gt_ns_to_pm_interval(gt, ei_up * 1000);
    let up_th = intel_gt_ns_to_pm_interval(gt, ei_up * up * 10);
    let down_ei = intel_gt_ns_to_pm_interval(gt, ei_down * 1000);
    let down_th = intel_gt_ns_to_pm_interval(gt, ei_down * down * 10);

    let u = &i915.uncore;
    u.intel_uncore_write_fw(GEN6_RP_UP_EI, up_ei);
    u.intel_uncore_write_fw(GEN6_RP_UP_THRESHOLD, up_th);
    u.intel_uncore_write_fw(GEN6_RP_DOWN_EI, down_ei);
    u.intel_uncore_write_fw(GEN6_RP_DOWN_THRESHOLD, down_th);

    // GRAPHICS_VER > 9: no GEN6_RP_MEDIA_TURBO.
    u.intel_uncore_write_fw(GEN6_RP_CONTROL,
                            GEN6_RP_MEDIA_HW_NORMAL_MODE | GEN6_RP_MEDIA_IS_GFX | GEN6_RP_ENABLE
                                | GEN6_RP_UP_BUSY_AVG | GEN6_RP_DOWN_IDLE_AVG);

    gt.rps.power_mode = new_power;
}

/// `gen6_rps_set_thresholds`.
fn gen6_rps_set_thresholds(i915: &mut drm_i915_private, gt: &mut intel_gt, val: u8) {
    let rps = &gt.rps;
    let mut new_power = rps.power_mode;
    match rps.power_mode {
        m if m == LOW_POWER as i32 => {
            if val as u32 > rps.efficient_freq as u32 + 1 && val > rps.cur_freq {
                new_power = BETWEEN as i32;
            }
        }
        m if m == BETWEEN as i32 => {
            if val <= rps.efficient_freq && val < rps.cur_freq {
                new_power = LOW_POWER as i32;
            } else if val >= rps.rp0_freq && val > rps.cur_freq {
                new_power = HIGH_POWER as i32;
            }
        }
        m if m == HIGH_POWER as i32 => {
            if (val as u32) < (rps.rp1_freq as u32 + rps.rp0_freq as u32) >> 1 && val < rps.cur_freq {
                new_power = BETWEEN as i32;
            }
        }
        _ => {}
    }
    // Max/min bins are special.
    if val <= rps.min_freq_softlimit {
        new_power = LOW_POWER as i32;
    }
    if val >= rps.max_freq_softlimit {
        new_power = HIGH_POWER as i32;
    }
    if rps.interactive {
        new_power = HIGH_POWER as i32;
    }
    rps_set_power(i915, gt, new_power);
}

/// `gen6_rps_set`, GRAPHICS_VER >= 9.
fn gen6_rps_set(i915: &mut drm_i915_private, val: u8) {
    i915.uncore.intel_uncore_write_fw(GEN6_RPNSWREQ, GEN9_FREQUENCY(val as u32));
}

/// `rps_set`.
fn rps_set(i915: &mut drm_i915_private, gt: &mut intel_gt, val: u8, update: bool) {
    if val == gt.rps.last_freq {
        return;
    }
    gen6_rps_set(i915, val);
    if update {
        gen6_rps_set_thresholds(i915, gt, val);
    }
    gt.rps.last_freq = val;
}

/// `rps_limits`, GRAPHICS_VER >= 9.
fn rps_limits(rps: &intel_rps, val: u8) -> u32 {
    let mut limits = (rps.max_freq_softlimit as u32) << 23;
    if val <= rps.min_freq_softlimit {
        limits |= (rps.min_freq_softlimit as u32) << 14;
    }
    limits
}

/// `intel_rps_set`.
fn intel_rps_set(i915: &mut drm_i915_private, gt: &mut intel_gt, val: u8) {
    if gt.rps.active {
        rps_set(i915, gt, val, true);
        // Keep the interrupts coming until min or max is reached.
        if gt.rps.interrupts {
            let u = &i915.uncore;
            u.intel_uncore_write_fw(GEN6_RP_INTERRUPT_LIMITS, rps_limits(&gt.rps, val));
            u.intel_uncore_write_fw(GEN6_PMINTRMSK, rps_pm_mask(&gt.rps, val));
        }
    }
    gt.rps.cur_freq = val;
}

/// `intel_rps_unpark`: start from the clamped current frequency.
pub fn intel_rps_unpark(i915: &mut drm_i915_private, gt: &mut intel_gt) {
    if !gt.rps.enabled {
        return;
    }

    gt.rps.active = true;
    let val = gt.rps.cur_freq.clamp(gt.rps.min_freq_softlimit, gt.rps.max_freq_softlimit);
    intel_rps_set(i915, gt, val);

    gt.rps.pm_iir = 0;
    if gt.rps.interrupts {
        rps_enable_interrupts(i915, gt);
    }
    if gt.rps.timer {
        rps_start_timer(gt);
    }
}

/// `intel_rps_park`: drop to the idle frequency, waking the media well so
/// the punit commits the voltage change now.
pub fn intel_rps_park(i915: &mut drm_i915_private, gt: &mut intel_gt) {
    if !gt.rps.enabled {
        return;
    }
    if !core::mem::replace(&mut gt.rps.active, false) {
        return;
    }

    if gt.rps.timer {
        rps_stop_timer(gt);
    }
    if gt.rps.interrupts {
        rps_disable_interrupts(i915, gt);
    }

    if gt.rps.last_freq <= gt.rps.idle_freq {
        return;
    }

    i915.uncore.intel_uncore_forcewake_get(FORCEWAKE_MEDIA);
    let idle = gt.rps.idle_freq;
    rps_set(i915, gt, idle, false);
    i915.uncore.intel_uncore_forcewake_put(FORCEWAKE_MEDIA);

    // Restart lower than before on the next unpark, as a down-clock would.
    let rps = &mut gt.rps;
    let mut adj = rps.last_adj;
    if adj < 0 {
        adj *= 2;
    } else {
        adj = -2;
    }
    rps.last_adj = adj;
    rps.cur_freq = (rps.cur_freq as i32 + adj).max(rps.min_freq as i32) as u8;
    if rps.cur_freq < rps.efficient_freq {
        rps.cur_freq = rps.efficient_freq;
        rps.last_adj = 0;
    }
}

/// `__gen6_rps_get_freq_caps`: RP0 > RP1 > RPn from RP_STATE_CAP, RP1 from
/// FREQ_INFO_REC on GRAPHICS_VER >= 10, converted from 50 MHz units.
fn gen6_rps_get_freq_caps(i915: &mut drm_i915_private) -> intel_rps_freq_caps {
    let rp_state_cap = i915.uncore.intel_uncore_read(GEN6_RP_STATE_CAP);
    let rp0 = rp_state_cap & 0xff;
    let rp1 = crate::intel_gt::reg_field_get(RPE_MASK, i915.uncore.intel_uncore_read(GEN10_FREQ_INFO_REC));
    let min = (rp_state_cap >> 16) & 0xff;
    intel_rps_freq_caps {
        rp0_freq: (rp0 * GEN9_FREQ_SCALER) as u8,
        rp1_freq: (rp1 * GEN9_FREQ_SCALER) as u8,
        min_freq: (min * GEN9_FREQ_SCALER) as u8,
    }
}

/// `gen6_rps_init`: RPe from the PCU's dynamic duty cycle control.
fn gen6_rps_init(i915: &mut drm_i915_private, rps: &mut intel_rps) {
    let caps = gen6_rps_get_freq_caps(i915);
    rps.rp0_freq = caps.rp0_freq;
    rps.rp1_freq = caps.rp1_freq;
    rps.min_freq = caps.min_freq;

    // hw_max = RP0 until we check for overclocking
    rps.max_freq = rps.rp0_freq;

    rps.efficient_freq = rps.rp1_freq;
    let mut ddcc_status = 0u32;
    if crate::intel_pcode::snb_pcode_read(i915, HSW_PCODE_DYNAMIC_DUTY_CYCLE_CONTROL, &mut ddcc_status, None).is_ok() {
        rps.efficient_freq = ((((ddcc_status >> 8) & 0xff) * GEN9_FREQ_SCALER) as u8)
            .clamp(rps.min_freq, rps.max_freq);
    }
}

/// `intel_rps_init`. The overclock query is SNB/IVB/HSW's; the redirect
/// and ARAT bits of `pm_intrmsk_mbz` are for other generations and GuC.
pub fn intel_rps_init(i915: &mut drm_i915_private, gt: &mut intel_gt) {
    let mut rps = gt.rps;
    gen6_rps_init(i915, &mut rps);

    // Derive initial user preferences/limits from the hardware limits.
    rps.max_freq_softlimit = rps.max_freq;
    rps.min_freq_softlimit = rps.min_freq;

    rps.up_threshold = 95;
    rps.down_threshold = 85;

    rps.boost_freq = rps.max_freq;
    rps.idle_freq = rps.min_freq;

    // Start in the middle, from here we will autotune based on workload.
    rps.cur_freq = rps.efficient_freq;

    rps.pm_intrmsk_mbz = 0;
    gt.rps = rps;
}

/// `intel_rps_sanitize`.
pub fn intel_rps_sanitize(i915: &mut drm_i915_private, gt: &mut intel_gt) {
    rps_disable_interrupts(i915, gt);
}

/// `rps_reset`: force the power mode and frequency to be written.
fn rps_reset(i915: &mut drm_i915_private, gt: &mut intel_gt) -> bool {
    gt.rps.power_mode = -1;
    gt.rps.last_freq = u8::MAX;

    let min = gt.rps.min_freq;
    rps_set(i915, gt, min, true);

    gt.rps.cur_freq = gt.rps.min_freq;
    true
}

/// `gen9_rps_enable`, GRAPHICS_VER 12.
fn gen9_rps_enable(i915: &mut drm_i915_private, gt: &mut intel_gt) -> bool {
    i915.uncore.intel_uncore_write_fw(GEN6_RP_IDLE_HYSTERSIS, 0xa);
    gt.rps.pm_events = GEN6_PM_RP_UP_THRESHOLD | GEN6_PM_RP_DOWN_THRESHOLD;
    rps_reset(i915, gt)
}

/// `has_busy_stats`: every engine reports busyness (execlists sets
/// I915_ENGINE_SUPPORTS_STATS).
fn has_busy_stats(gt: &intel_gt) -> bool {
    gt.engine.iter().flatten().all(|e| e.flags & I915_ENGINE_SUPPORTS_STATS != 0)
}

/// `intel_gpu_freq`, GRAPHICS_VER >= 9: hardware units to MHz.
pub fn intel_gpu_freq(val: u32) -> u32 {
    (val * GT_FREQUENCY_MULTIPLIER + GEN9_FREQ_SCALER / 2) / GEN9_FREQ_SCALER
}

/// `intel_rps_enable`. `intel_gt_check_clock_frequency` is a debug check.
pub fn intel_rps_enable(i915: &mut drm_i915_private, gt: &mut intel_gt) {
    if TGL_HAS_RPS == 0 {
        return;
    }

    i915.uncore.intel_uncore_forcewake_get(FORCEWAKE_ALL);
    let enabled = if gt.rps.max_freq <= gt.rps.min_freq {
        // no room for dynamic reclocking
        false
    } else {
        gen9_rps_enable(i915, gt)
    };
    i915.uncore.intel_uncore_forcewake_put(FORCEWAKE_ALL);
    if !enabled {
        return;
    }

    let r = &gt.rps;
    Line::new().s("RPS: min ").dec(intel_gpu_freq(r.min_freq as u32) as u64).s(" MHz, RPe ")
        .dec(intel_gpu_freq(r.efficient_freq as u32) as u64).s(" MHz, RP1 ")
        .dec(intel_gpu_freq(r.rp1_freq as u32) as u64).s(" MHz, RP0 ")
        .dec(intel_gpu_freq(r.rp0_freq as u32) as u64).s(" MHz, thresholds ")
        .dec(r.up_threshold as u64).s("/").dec(r.down_threshold as u64).s("%").say();

    // GRAPHICS_VER 12: the timer when engines report busyness; interrupts
    // are only used from 6 to 11.
    if has_busy_stats(gt) {
        gt.rps.timer = true;
    }

    gt.rps.enabled = true;
}

/// `gen6_rps_disable`.
fn gen6_rps_disable(i915: &mut drm_i915_private) {
    i915.uncore.intel_uncore_write_fw(GEN6_RP_CONTROL, 0);
}

/// `intel_rps_disable`.
pub fn intel_rps_disable(i915: &mut drm_i915_private, gt: &mut intel_gt) {
    if !gt.rps.enabled {
        return;
    }
    gt.rps.enabled = false;
    gt.rps.interrupts = false;
    gt.rps.timer = false;
    gen6_rps_disable(i915);
}

/// `intel_rps_get_min_raw_freq`: 50 MHz units.
pub fn intel_rps_get_min_raw_freq(gt: &intel_gt) -> u32 {
    gt.rps.min_freq as u32 / GEN9_FREQ_SCALER
}

/// `intel_rps_get_max_raw_freq`: 50 MHz units.
pub fn intel_rps_get_max_raw_freq(gt: &intel_gt) -> u32 {
    gt.rps.max_freq as u32 / GEN9_FREQ_SCALER
}

/// The requested and the actual frequency, for the log.
pub fn report(i915: &mut drm_i915_private, gt: &intel_gt, when: &str) {
    let u = &mut i915.uncore;
    let swreq = u.intel_uncore_read(GEN6_RPNSWREQ);
    let rpstat = u.intel_uncore_read(GEN6_RPSTAT1);
    let ctl = u.intel_uncore_read(GEN6_RP_CONTROL);
    let cagf = crate::intel_gt::reg_field_get(GEN12_CAGF_MASK, rpstat);
    Line::new().s("RPS ").s(when).s(": enabled ").dec(gt.rps.enabled as u64).s(", requested ")
        .dec(intel_gpu_freq(swreq >> 23) as u64).s(" MHz, actual ").dec(intel_gpu_freq(cagf) as u64)
        .s(" MHz, RP_CONTROL ").hex(ctl).say();
}

/// `ktime_get`, in ns.
fn ktime_get() -> u64 {
    linux::now_us() * 1000
}

/// `jiffies` at CONFIG_HZ=1000, in the `now_us` clock.
fn jiffies_us() -> u64 {
    linux::now_us()
}

/// `rps_start_timer`.
fn rps_start_timer(gt: &mut intel_gt) {
    gt.rps.pm_timestamp = ktime_get().wrapping_sub(gt.rps.pm_timestamp);
    gt.rps.pm_interval = 1;
    gt.rps.timer_due = Some(jiffies_us() + 1000);
}

/// `rps_stop_timer`: `timer_delete_sync`, then `cancel_work_sync`.
fn rps_stop_timer(gt: &mut intel_gt) {
    gt.rps.timer_due = None;
    gt.rps.pm_timestamp = ktime_get().wrapping_sub(gt.rps.pm_timestamp);
    gt.rps.work_queued = false;
}

/// `rps_timer`: the three busiest engines over the last interval decide
/// whether to clock up or down; the interval doubles while nothing changes.
fn rps_timer(gt: &mut intel_gt) {
    let mut max_busy = [0u64; 3];
    let mut timestamp = 0u64;
    for e in 0..gt.engine.len() {
        if gt.engine[e].is_none() {
            continue;
        }
        let dt = crate::intel_execlists_submission::intel_engine_get_busy_time(gt, e, &mut timestamp);
        let Some(x) = gt.engine[e].as_mut() else { continue };
        let last = x.stats_rps;
        x.stats_rps = dt;

        let mut busy = dt.wrapping_sub(last);
        for m in max_busy.iter_mut() {
            if busy > *m {
                core::mem::swap(&mut busy, m);
            }
        }
    }
    let last = gt.rps.pm_timestamp;
    gt.rps.pm_timestamp = timestamp;

    if gt.rps.active {
        let dt = timestamp.wrapping_sub(last);

        let mut busy = max_busy[0];
        for (i, m) in max_busy.iter().enumerate().skip(1) {
            if *m == 0 {
                break;
            }
            busy += m / (1 << i);
        }

        let rps = &mut gt.rps;
        if 100 * busy > rps.up_threshold as u64 * dt && rps.cur_freq < rps.max_freq_softlimit {
            rps.pm_iir |= GEN6_PM_RP_UP_THRESHOLD;
            rps.pm_interval = 1;
            rps.work_queued = true;
        } else if 100 * busy < rps.down_threshold as u64 * dt && rps.cur_freq > rps.min_freq_softlimit {
            rps.pm_iir |= GEN6_PM_RP_DOWN_THRESHOLD;
            rps.pm_interval = 1;
            rps.work_queued = true;
        } else {
            rps.last_adj = 0;
        }

        rps.timer_due = Some(jiffies_us() + rps.pm_interval as u64 * 1000);
        rps.pm_interval = (rps.pm_interval * 2).min(BUSY_MAX_EI);
    }
}

/// `rps_work`: apply a waiter's boost or a threshold event to the frequency.
fn rps_work(i915: &mut drm_i915_private, gt: &mut intel_gt) {
    let pm_iir = core::mem::take(&mut gt.rps.pm_iir) & gt.rps.pm_events;
    let client_boost = gt.rps.num_waiters != 0;

    if pm_iir != 0 || client_boost {
        if !gt.rps.active {
            return;
        }

        let rps = &gt.rps;
        let mut adj = rps.last_adj;
        let mut new_freq = rps.cur_freq as i32;
        let min = rps.min_freq_softlimit as i32;
        let mut max = rps.max_freq_softlimit as i32;
        if client_boost {
            max = rps.max_freq as i32;
        }

        if client_boost && new_freq < rps.boost_freq as i32 {
            new_freq = rps.boost_freq as i32;
            adj = 0;
        } else if pm_iir & GEN6_PM_RP_UP_THRESHOLD != 0 {
            adj = if adj > 0 { adj * 2 } else { 1 };
            if new_freq >= rps.max_freq_softlimit as i32 {
                adj = 0;
            }
        } else if client_boost {
            adj = 0;
        } else if pm_iir & GEN6_PM_RP_DOWN_TIMEOUT != 0 {
            if rps.cur_freq > rps.efficient_freq {
                new_freq = rps.efficient_freq as i32;
            } else if rps.cur_freq > rps.min_freq_softlimit {
                new_freq = rps.min_freq_softlimit as i32;
            }
            adj = 0;
        } else if pm_iir & GEN6_PM_RP_DOWN_THRESHOLD != 0 {
            adj = if adj < 0 { adj * 2 } else { -1 };
            if new_freq <= rps.min_freq_softlimit as i32 {
                adj = 0;
            }
        } else {
            adj = 0;
        }

        new_freq = (new_freq + adj).clamp(min, max);
        intel_rps_set(i915, gt, new_freq as u8);
        gt.rps.last_adj = adj;
    }

    let events = gt.rps.pm_events;
    crate::intel_gt_pm_irq::gen6_gt_pm_unmask_irq(i915, gt, events);
}

/// `intel_rps_boost`, for a waiter on a request that has not started
/// (`i915_gem_object_boost`): clock straight up to `boost_freq`.
pub fn intel_rps_boost(gt: &mut intel_gt, rq: usize) {
    if gt.requests[rq].signaled || gt.requests[rq].boost {
        return;
    }
    gt.requests[rq].boost = true;
    let rps = &mut gt.rps;
    rps.num_waiters += 1;
    if rps.num_waiters > 1 {
        return;
    }
    if !rps.active {
        return;
    }
    if rps.cur_freq < rps.boost_freq {
        rps.work_queued = true;
    }
    rps.boosts += 1;
}

/// `intel_rps_dec_waiters`.
pub fn intel_rps_dec_waiters(gt: &mut intel_gt) {
    gt.rps.num_waiters = gt.rps.num_waiters.saturating_sub(1);
}

/// When (`now_us`) the RPS timer or the queued work wants to run next.
pub fn intel_rps_timer_due(gt: &intel_gt) -> Option<u64> {
    if gt.rps.work_queued {
        return Some(0);
    }
    gt.rps.timer_due
}

/// Runs the RPS timer if it is due and then the queued work, as the
/// kernel's timer and workqueue would. A frequency change is logged at
/// most once a second.
pub fn intel_rps_run_pending(i915: &mut drm_i915_private, gt: &mut intel_gt) {
    if gt.rps.timer_due.is_some_and(|t| linux::now_us() >= t) {
        gt.rps.timer_due = None;
        rps_timer(gt);
    }
    if core::mem::take(&mut gt.rps.work_queued) {
        rps_work(i915, gt);
    }
}

/// The requested frequency in MHz, for statistics.
pub fn intel_rps_requested_mhz(gt: &intel_gt) -> u32 {
    intel_gpu_freq(gt.rps.cur_freq as u32)
}
