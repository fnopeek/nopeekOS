//! drivers/gpu/drm/scheduler (Linux 6.18.55): sched_main.c, sched_entity.c
//! and sched_fence.c, with the amdgpu backend (`amdgpu_sched_ops`).
//!
//! A ring's scheduler is `adev.rings[r].sched`; entities live in
//! `adev.sched_entities` and jobs in `adev.jobs`, both named by their key.
//! The scheduler's work items run from `workqueue::run_pending`. The policy
//! is the default, FIFO.

use crate::amdgpu_device::amdgpu_device;
use crate::dma_fence::{self, dma_fence_func, dma_fence_ops, Fence};
use crate::linux::Line;
use crate::workqueue::{self, work};
use alloc::collections::VecDeque;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::{Cell, RefCell};

pub const DRM_SCHED_PRIORITY_KERNEL: u32 = 0;
pub const DRM_SCHED_PRIORITY_HIGH: u32 = 1;
pub const DRM_SCHED_PRIORITY_NORMAL: u32 = 2;
pub const DRM_SCHED_PRIORITY_LOW: u32 = 3;
pub const DRM_SCHED_PRIORITY_COUNT: u32 = 4;

/// `MAX_SCHEDULE_TIMEOUT`, in microseconds like every timeout here.
pub const MAX_SCHEDULE_TIMEOUT: i64 = i64::MAX;
/// `MAX_WAIT_SCHED_ENTITY_Q_EMPTY`: `msecs_to_jiffies(1000)`.
const MAX_WAIT_SCHED_ENTITY_Q_EMPTY: i64 = 1_000_000;

pub const DRM_SCHED_FENCE_DONT_PIPELINE: u32 = dma_fence::DMA_FENCE_FLAG_USER_BITS;

/// `enum drm_gpu_sched_stat`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum drm_gpu_sched_stat {
    RESET,
    ENODEV,
    NO_HANG,
}

/// `struct drm_sched_fence` as the finished fence carries it.
pub struct drm_sched_fence {
    pub scheduled: Fence,
    pub parent: RefCell<Option<Fence>>,
    pub sched: usize,
    pub owner: u64,
}

/// What `to_drm_sched_fence` reaches from either of the two fences.
pub struct drm_sched_fence_ref {
    pub scheduled: Fence,
    /// `None` once the finished fence is released (`dma_fence_get_rcu`
    /// fails then).
    pub finished: Option<Fence>,
    pub sched: usize,
    pub owner: u64,
}

/// `to_drm_sched_fence`.
pub fn to_drm_sched_fence(f: &Fence) -> Option<drm_sched_fence_ref> {
    match &f.ops {
        dma_fence_ops::sched_finished(s) => Some(drm_sched_fence_ref {
            scheduled: s.scheduled.clone(),
            finished: Some(f.clone()),
            sched: s.sched,
            owner: s.owner,
        }),
        dma_fence_ops::sched_scheduled { finished, sched, owner } => Some(drm_sched_fence_ref {
            scheduled: f.clone(),
            finished: finished.upgrade(),
            sched: *sched,
            owner: *owner,
        }),
        _ => None,
    }
}

/// The `drm_sched_fence` of a finished fence.
fn s_fence_of(finished: &Fence) -> &drm_sched_fence {
    match &finished.ops {
        dma_fence_ops::sched_finished(s) => s,
        _ => unreachable!(),
    }
}

/// `struct drm_sched_job`.
#[derive(Default)]
pub struct drm_sched_job {
    pub entity: Option<usize>,
    /// The ring whose scheduler runs the job.
    pub sched: Option<usize>,
    pub credits: u32,
    /// `s_fence`, by its finished fence once armed; the owner is what
    /// `drm_sched_fence_alloc` records before.
    pub s_fence: Option<Fence>,
    pub s_fence_owner: u64,
    s_fence_alloc: bool,
    /// The `dependencies` xarray: an erased index is `None`.
    pub dependencies: Vec<Option<Fence>>,
    last_dependency: usize,
    pub submit_ts: u64,
    pub s_priority: u32,
    /// The callback that ends the job (`cb`).
    cb: Option<u64>,
}

/// `struct drm_sched_rq`.
#[derive(Default)]
pub struct drm_sched_rq {
    entities: Vec<usize>,
    /// `rb_tree_root`, ordered by `oldest_job_waiting`.
    rb_tree: Vec<(u64, usize)>,
    current_entity: Option<usize>,
}

/// `struct drm_gpu_scheduler`.
#[derive(Default)]
pub struct drm_gpu_scheduler {
    pub ready: bool,
    pub name: String,
    pub credit_limit: u32,
    credit_count: u32,
    pub timeout: i64,
    hang_limit: u32,
    pub num_rqs: u32,
    sched_rq: Vec<drm_sched_rq>,
    /// Jobs by key, oldest first.
    pending_list: VecDeque<u64>,
    pub score: u32,
    pause_submit: bool,
    free_guilty: bool,
}

/// `struct drm_sched_entity`.
#[derive(Default)]
pub struct drm_sched_entity {
    /// `list` is on its run queue's `entities`.
    in_list: bool,
    /// `rq`: the ring of its scheduler and the priority.
    pub rq: Option<(usize, u32)>,
    /// `sched_list` while there is more than one to choose from.
    sched_list: Option<Vec<usize>>,
    num_sched_list: u32,
    pub priority: u32,
    pub guilty: Option<Rc<Cell<i32>>>,
    /// `job_queue`, by job key.
    job_queue: VecDeque<u64>,
    pub fence_context: u64,
    fence_seq: u32,
    dependency: Option<Fence>,
    /// The callback on `dependency` (`cb`).
    cb: Option<u64>,
    pub last_scheduled: Option<Fence>,
    stopped: bool,
    oldest_job_waiting: u64,
    in_rb_tree: bool,
}

fn entity(adev: &mut amdgpu_device, e: usize) -> &mut drm_sched_entity {
    adev.sched_entities[e].as_mut().unwrap()
}

fn job(adev: &mut amdgpu_device, j: u64) -> &mut drm_sched_job {
    &mut adev.jobs.get_mut(&j).unwrap().base
}

fn rq(adev: &mut amdgpu_device, at: (usize, u32)) -> &mut drm_sched_rq {
    &mut adev.rings[at.0].sched.sched_rq[at.1 as usize]
}

// sched_fence.c

/// `drm_sched_fence_alloc`.
fn drm_sched_fence_alloc(j: &mut drm_sched_job, owner: u64) {
    j.s_fence_owner = owner;
    j.s_fence_alloc = true;
}

/// `drm_sched_fence_init`: the scheduled fence on the entity's first
/// context, the finished one on its second.
fn drm_sched_fence_init(adev: &mut amdgpu_device, j: u64, e: usize) {
    let ent = entity(adev, e);
    let sched = ent.rq.unwrap().0;
    ent.fence_seq = ent.fence_seq.wrapping_add(1);
    let (seq, ctx) = (ent.fence_seq as u64, ent.fence_context);
    let owner = job(adev, j).s_fence_owner;
    let finished = Rc::new_cyclic(|w| {
        let scheduled = dma_fence::dma_fence_init(
            dma_fence_ops::sched_scheduled { finished: w.clone(), sched, owner }, ctx, seq);
        dma_fence::dma_fence_new(
            dma_fence_ops::sched_finished(drm_sched_fence {
                scheduled,
                parent: RefCell::new(None),
                sched,
                owner,
            }),
            ctx + 1,
            seq,
        )
    });
    job(adev, j).s_fence = Some(finished);
}

/// `drm_sched_fence_scheduled`.
fn drm_sched_fence_scheduled(adev: &mut amdgpu_device, finished: &Fence, parent: Option<&Fence>) {
    let s = s_fence_of(finished);
    if let Some(p) = parent {
        // drm_sched_fence_set_parent
        *s.parent.borrow_mut() = Some(p.clone());
    }
    let scheduled = s.scheduled.clone();
    dma_fence::dma_fence_signal(adev, &scheduled);
}

/// `drm_sched_fence_finished`.
fn drm_sched_fence_finished(adev: &mut amdgpu_device, finished: &Fence, result: i32) {
    if result != 0 {
        dma_fence::dma_fence_set_error(finished, result);
    }
    dma_fence::dma_fence_signal(adev, finished);
}

// sched_main.c

/// `drm_sched_available_credits`.
fn drm_sched_available_credits(adev: &amdgpu_device, r: usize) -> u32 {
    let s = &adev.rings[r].sched;
    match s.credit_limit.checked_sub(s.credit_count) {
        Some(c) => c,
        None => {
            Line::new().s("WARNING: drm_sched credit count above the limit").say();
            0
        }
    }
}

/// `drm_sched_can_queue`.
fn drm_sched_can_queue(adev: &mut amdgpu_device, r: usize, e: usize) -> bool {
    let Some(&j) = entity(adev, e).job_queue.front() else { return false };
    let limit = adev.rings[r].sched.credit_limit;
    if job(adev, j).credits > limit {
        Line::new().s("WARNING: Jobs may not exceed the credit limit, truncate.").say();
        job(adev, j).credits = limit;
    }
    drm_sched_available_credits(adev, r) >= job(adev, j).credits
}

/// `drm_sched_rq_remove_fifo_locked`.
fn drm_sched_rq_remove_fifo_locked(adev: &mut amdgpu_device, e: usize, at: (usize, u32)) {
    if entity(adev, e).in_rb_tree {
        rq(adev, at).rb_tree.retain(|x| x.1 != e);
        entity(adev, e).in_rb_tree = false;
    }
}

/// `drm_sched_rq_update_fifo_locked`: equal times go after the ones there.
fn drm_sched_rq_update_fifo_locked(adev: &mut amdgpu_device, e: usize, at: (usize, u32), ts: u64) {
    drm_sched_rq_remove_fifo_locked(adev, e, at);
    entity(adev, e).oldest_job_waiting = ts;
    let t = rq(adev, at);
    let pos = t.rb_tree.partition_point(|x| x.0 <= ts);
    t.rb_tree.insert(pos, (ts, e));
    entity(adev, e).in_rb_tree = true;
}

/// `drm_sched_rq_add_entity`.
fn drm_sched_rq_add_entity(adev: &mut amdgpu_device, at: (usize, u32), e: usize) {
    if entity(adev, e).in_list {
        return;
    }
    adev.rings[at.0].sched.score += 1;
    rq(adev, at).entities.push(e);
    entity(adev, e).in_list = true;
}

/// `drm_sched_rq_remove_entity`.
fn drm_sched_rq_remove_entity(adev: &mut amdgpu_device, at: (usize, u32), e: usize) {
    if !entity(adev, e).in_list {
        return;
    }
    adev.rings[at.0].sched.score -= 1;
    let t = rq(adev, at);
    t.entities.retain(|&x| x != e);
    if t.current_entity == Some(e) {
        t.current_entity = None;
    }
    entity(adev, e).in_list = false;
    drm_sched_rq_remove_fifo_locked(adev, e, at);
}

/// `drm_sched_entity_is_ready`.
fn drm_sched_entity_is_ready(adev: &mut amdgpu_device, e: usize) -> bool {
    let ent = entity(adev, e);
    !ent.job_queue.is_empty() && ent.dependency.is_none()
}

/// What `drm_sched_rq_select_entity_fifo` returns.
enum selected {
    none,
    entity(usize),
    /// `ERR_PTR(-ENOSPC)`.
    nospc,
}

/// `drm_sched_rq_select_entity_fifo`.
fn drm_sched_rq_select_entity_fifo(adev: &mut amdgpu_device, r: usize, prio: u32) -> selected {
    let order: Vec<usize> = rq(adev, (r, prio)).rb_tree.iter().map(|x| x.1).collect();
    for e in order {
        if drm_sched_entity_is_ready(adev, e) {
            if !drm_sched_can_queue(adev, r, e) {
                return selected::nospc;
            }
            // reinit_completion(&entity->entity_idle): nothing waits on it
            // while a single thread runs the job.
            return selected::entity(e);
        }
    }
    selected::none
}

/// `drm_sched_run_job_queue`.
fn drm_sched_run_job_queue(adev: &mut amdgpu_device, r: usize) {
    if !adev.rings[r].sched.pause_submit {
        workqueue::queue_work(adev, work::sched_run_job { ring: r });
    }
}

/// `drm_sched_run_free_queue`.
fn drm_sched_run_free_queue(adev: &mut amdgpu_device, r: usize) {
    if !adev.rings[r].sched.pause_submit {
        workqueue::queue_work(adev, work::sched_free_job { ring: r });
    }
}

/// `drm_sched_job_done`.
fn drm_sched_job_done(adev: &mut amdgpu_device, j: u64, result: i32) {
    let finished = job(adev, j).s_fence.clone().unwrap();
    let r = s_fence_of(&finished).sched;
    let credits = job(adev, j).credits;
    let s = &mut adev.rings[r].sched;
    s.credit_count -= credits;
    s.score -= 1;
    drm_sched_fence_finished(adev, &finished, result);
    drm_sched_run_free_queue(adev, r);
}

/// `drm_sched_job_done_cb`.
pub fn drm_sched_job_done_cb(adev: &mut amdgpu_device, f: &Fence, _ring: usize, j: u64) {
    job(adev, j).cb = None;
    drm_sched_job_done(adev, j, f.error());
}

/// `drm_sched_start_timeout`.
fn drm_sched_start_timeout(adev: &mut amdgpu_device, r: usize) {
    let s = &adev.rings[r].sched;
    if s.timeout != MAX_SCHEDULE_TIMEOUT && !s.pending_list.is_empty() {
        let t = s.timeout as u64;
        workqueue::mod_delayed_work(adev, work::sched_timedout { ring: r }, t);
    }
}

/// `drm_sched_fault`.
pub fn drm_sched_fault(adev: &mut amdgpu_device, r: usize) {
    workqueue::mod_delayed_work(adev, work::sched_timedout { ring: r }, 0);
}

/// `drm_sched_suspend_timeout`: what was left of it.
pub fn drm_sched_suspend_timeout(adev: &mut amdgpu_device, r: usize) -> i64 {
    let w = work::sched_timedout { ring: r };
    let now = crate::linux::now_us();
    let expires = workqueue::delayed_work_expires(adev, w);
    workqueue::mod_delayed_work(adev, w, MAX_SCHEDULE_TIMEOUT as u64);
    match expires {
        Some(t) if t > now => (t - now) as i64,
        _ => adev.rings[r].sched.timeout,
    }
}

/// `drm_sched_resume_timeout`.
pub fn drm_sched_resume_timeout(adev: &mut amdgpu_device, r: usize, remaining: i64) {
    let w = work::sched_timedout { ring: r };
    if adev.rings[r].sched.pending_list.is_empty() {
        workqueue::cancel_delayed_work(adev, w);
    } else {
        workqueue::mod_delayed_work(adev, w, remaining as u64);
    }
}

/// `drm_sched_job_begin`.
fn drm_sched_job_begin(adev: &mut amdgpu_device, j: u64) {
    let r = job(adev, j).sched.unwrap();
    adev.rings[r].sched.pending_list.push_back(j);
    drm_sched_start_timeout(adev, r);
}

/// `drm_sched_job_timedout`.
pub fn drm_sched_job_timedout(adev: &mut amdgpu_device, r: usize) {
    let mut status = drm_gpu_sched_stat::RESET;
    if let Some(j) = adev.rings[r].sched.pending_list.pop_front() {
        status = crate::amdgpu_job::amdgpu_job_timedout(adev, j);
        if adev.rings[r].sched.free_guilty {
            crate::amdgpu_job::amdgpu_job_free_cb(adev, j);
            adev.rings[r].sched.free_guilty = false;
        }
        if status == drm_gpu_sched_stat::NO_HANG {
            // drm_sched_job_reinsert_on_false_timeout
            adev.rings[r].sched.pending_list.push_front(j);
            drm_sched_run_free_queue(adev, r);
        }
    }
    if status != drm_gpu_sched_stat::ENODEV {
        drm_sched_start_timeout(adev, r);
    }
}

/// `drm_sched_job_init`.
pub fn drm_sched_job_init(adev: &mut amdgpu_device, j: u64, e: usize, credits: u32, owner: u64) -> i32 {
    if entity(adev, e).rq.is_none() {
        Line::new().s("drm_sched_job_init: entity has no rq!").say();
        return -2;
    }
    if credits == 0 {
        Line::new().s("*ERROR* drm_sched_job_init: credits cannot be 0!").say();
        return -22;
    }
    let jb = job(adev, j);
    *jb = drm_sched_job::default();
    jb.entity = Some(e);
    jb.credits = credits;
    drm_sched_fence_alloc(jb, owner);
    0
}

/// `drm_sched_job_arm`.
pub fn drm_sched_job_arm(adev: &mut amdgpu_device, j: u64) {
    let e = job(adev, j).entity.expect("drm_sched_job_arm without an entity");
    drm_sched_entity_select_rq(adev, e);
    let (r, _) = entity(adev, e).rq.unwrap();
    let prio = entity(adev, e).priority;
    let jb = job(adev, j);
    jb.sched = Some(r);
    jb.s_priority = prio;
    drm_sched_fence_init(adev, j, e);
}

/// `drm_sched_job_add_dependency`: takes the reference it is given.
pub fn drm_sched_job_add_dependency(adev: &mut amdgpu_device, j: u64, fence: Option<Fence>) -> i32 {
    let Some(fence) = fence else { return 0 };
    let deps = &mut job(adev, j).dependencies;
    // Deduplicate if we already depend on a fence from the same context.
    for entry in deps.iter_mut().flatten() {
        if entry.context != fence.context {
            continue;
        }
        if dma_fence::dma_fence_is_later(&fence, entry) {
            *entry = fence;
        }
        return 0;
    }
    // xa_alloc: the lowest free index.
    match deps.iter().position(|d| d.is_none()) {
        Some(i) => deps[i] = Some(fence),
        None => deps.push(Some(fence)),
    }
    0
}

/// `drm_sched_job_has_dependency`.
pub fn drm_sched_job_has_dependency(adev: &mut amdgpu_device, j: u64, fence: &Fence) -> bool {
    job(adev, j).dependencies.iter().flatten().any(|f| Rc::ptr_eq(f, fence))
}

/// `drm_sched_job_cleanup`.
pub fn drm_sched_job_cleanup(adev: &mut amdgpu_device, j: u64) {
    let jb = job(adev, j);
    // drm_sched_job_arm() has been called, or not: either way the fence
    // goes with this reference.
    jb.s_fence = None;
    jb.s_fence_alloc = false;
    jb.dependencies.clear();
}

/// `drm_sched_wakeup`.
pub fn drm_sched_wakeup(adev: &mut amdgpu_device, r: usize) {
    drm_sched_run_job_queue(adev, r);
}

/// `drm_sched_select_entity`.
fn drm_sched_select_entity(adev: &mut amdgpu_device, r: usize) -> Option<usize> {
    for prio in DRM_SCHED_PRIORITY_KERNEL..adev.rings[r].sched.num_rqs {
        match drm_sched_rq_select_entity_fifo(adev, r, prio) {
            selected::none => continue,
            selected::entity(e) => return Some(e),
            selected::nospc => return None,
        }
    }
    None
}

/// `drm_sched_get_finished_job`: the job and whether the next is done too.
fn drm_sched_get_finished_job(adev: &mut amdgpu_device, r: usize) -> Option<(u64, bool)> {
    let &j = adev.rings[r].sched.pending_list.front()?;
    let finished = job(adev, j).s_fence.clone()?;
    if !dma_fence::dma_fence_is_signaled(adev, &finished) {
        return None;
    }
    adev.rings[r].sched.pending_list.pop_front();
    // cancel this job's TO timer
    workqueue::cancel_delayed_work(adev, work::sched_timedout { ring: r });
    let mut have_more = false;
    if let Some(&next) = adev.rings[r].sched.pending_list.front() {
        let nf = job(adev, next).s_fence.clone().unwrap();
        let scheduled = s_fence_of(&nf).scheduled.clone();
        if scheduled.test_flag(dma_fence::DMA_FENCE_FLAG_TIMESTAMP_BIT) {
            // make the scheduled timestamp more accurate
            scheduled.set_timestamp(finished.timestamp());
        }
        have_more = dma_fence::dma_fence_is_signaled(adev, &nf);
        // start TO timer for next job
        drm_sched_start_timeout(adev, r);
    }
    Some((j, have_more))
}

/// `drm_sched_pick_best`: the ready scheduler with the lowest score.
fn drm_sched_pick_best(adev: &amdgpu_device, list: &[usize]) -> Option<usize> {
    let mut picked = None;
    let mut min_score = u32::MAX;
    for &r in list {
        let s = &adev.rings[r].sched;
        if !s.ready {
            Line::new().s("scheduler ").s(&s.name).s(" is not ready, skipping").say();
            continue;
        }
        if s.score < min_score {
            min_score = s.score;
            picked = Some(r);
        }
    }
    picked
}

/// `drm_sched_free_job_work`.
pub fn drm_sched_free_job_work(adev: &mut amdgpu_device, r: usize) {
    if let Some((j, have_more)) = drm_sched_get_finished_job(adev, r) {
        crate::amdgpu_job::amdgpu_job_free_cb(adev, j);
        if have_more {
            drm_sched_run_free_queue(adev, r);
        }
    }
    drm_sched_run_job_queue(adev, r);
}

/// `drm_sched_run_job_work`.
pub fn drm_sched_run_job_work(adev: &mut amdgpu_device, r: usize) {
    // Find entity to schedule from
    let Some(e) = drm_sched_select_entity(adev, r) else { return };
    let Some(j) = drm_sched_entity_pop_job(adev, e) else {
        // complete_all(&entity->entity_idle)
        drm_sched_run_job_queue(adev, r);
        return;
    };
    let finished = job(adev, j).s_fence.clone().unwrap();
    let credits = job(adev, j).credits;
    adev.rings[r].sched.credit_count += credits;
    drm_sched_job_begin(adev, j);
    let fence = crate::amdgpu_job::amdgpu_job_run(adev, j);
    drm_sched_fence_scheduled(adev, &finished, fence.as_ref().ok().and_then(|f| f.as_ref()));
    match fence {
        Ok(Some(f)) => match dma_fence::dma_fence_add_callback(adev, &f, dma_fence_func::sched_job_done { ring: r, job: j }) {
            Ok(id) => job(adev, j).cb = Some(id),
            Err(-2) => drm_sched_job_done(adev, j, f.error()),
            Err(e) => Line::new().s("fence add callback failed (").dec((-e) as u64).s(")").say(),
        },
        Ok(None) => drm_sched_job_done(adev, j, 0),
        Err(e) => drm_sched_job_done(adev, j, e),
    }
    // wake_up(&sched->job_scheduled): its waiters check in `wait_until`.
    drm_sched_run_job_queue(adev, r);
}

/// `drm_sched_init` with amdgpu's arguments: all priorities, its own
/// ordered submit queue.
pub fn drm_sched_init(adev: &mut amdgpu_device, r: usize, credit_limit: u32, timeout: i64, name: String) -> i32 {
    let s = &mut adev.rings[r].sched;
    if !s.sched_rq.is_empty() {
        Line::new().s("drm_sched_init: scheduler already initialized!").say();
        return 0;
    }
    s.credit_limit = credit_limit;
    s.name = name;
    s.timeout = timeout;
    s.hang_limit = 0;
    s.num_rqs = DRM_SCHED_PRIORITY_COUNT;
    s.sched_rq = (0..DRM_SCHED_PRIORITY_COUNT).map(|_| drm_sched_rq::default()).collect();
    s.pending_list.clear();
    s.credit_count = 0;
    s.score = 0;
    s.pause_submit = false;
    s.ready = true;
    0
}

/// `drm_sched_fini`.
pub fn drm_sched_fini(adev: &mut amdgpu_device, r: usize) {
    // drm_sched_wqueue_stop
    adev.rings[r].sched.pause_submit = true;
    workqueue::cancel_work(adev, work::sched_run_job { ring: r });
    workqueue::cancel_work(adev, work::sched_free_job { ring: r });
    for prio in DRM_SCHED_PRIORITY_KERNEL..adev.rings[r].sched.num_rqs {
        let ents = rq(adev, (r, prio)).entities.clone();
        for e in ents {
            // Prevents reinsertion and marks job_queue as idle, it will
            // be removed from the rq in drm_sched_entity_fini()
            // eventually.
            entity(adev, e).stopped = true;
        }
        rq(adev, (r, prio)).entities.clear();
    }
    workqueue::cancel_delayed_work(adev, work::sched_timedout { ring: r });
    // drm_sched_cancel_remaining_jobs: amdgpu has no cancel_job.
    if !adev.rings[r].sched.pending_list.is_empty() {
        Line::new().s("WARNING: ").s(&adev.rings[r].sched.name.clone())
            .s(": jobs still pending at drm_sched_fini").say();
    }
    let s = &mut adev.rings[r].sched;
    s.ready = false;
    s.sched_rq.clear();
    s.num_rqs = 0;
}

// sched_entity.c

/// `drm_sched_entity_init`: the entity's key.
pub fn drm_sched_entity_init(adev: &mut amdgpu_device, priority: u32, sched_list: &[usize],
                             guilty: Option<Rc<Cell<i32>>>) -> Result<usize, i32> {
    let mut ent = drm_sched_entity {
        guilty,
        num_sched_list: sched_list.len() as u32,
        priority,
        sched_list: (sched_list.len() > 1).then(|| sched_list.to_vec()),
        ..Default::default()
    };
    if let Some(&s0) = sched_list.first() {
        let num_rqs = adev.rings[s0].sched.num_rqs;
        if adev.rings[s0].sched.sched_rq.is_empty() {
            Line::new().s("drm_sched_entity_init: called with uninitialized scheduler").say();
        } else {
            if ent.priority >= num_rqs {
                Line::new().s("entity has out-of-bounds priority: ").dec(ent.priority as u64).s(". num_rqs: ")
                    .dec(num_rqs as u64).say();
                ent.priority = (num_rqs as i32 - 1).max(DRM_SCHED_PRIORITY_KERNEL as i32) as u32;
            }
            ent.rq = Some((s0, ent.priority));
        }
    }
    ent.fence_context = dma_fence::dma_fence_context_alloc(2);
    let slot = match adev.sched_entities.iter().position(|e| e.is_none()) {
        Some(i) => i,
        None => {
            adev.sched_entities.push(None);
            adev.sched_entities.len() - 1
        }
    };
    adev.sched_entities[slot] = Some(ent);
    Ok(slot)
}

/// `drm_sched_entity_is_idle`.
fn drm_sched_entity_is_idle(adev: &mut amdgpu_device, e: usize) -> bool {
    let ent = entity(adev, e);
    !ent.in_list || ent.job_queue.is_empty() || ent.stopped
}

/// `entity->stopped`.
pub fn drm_sched_entity_stopped(adev: &mut amdgpu_device, e: usize) -> bool {
    entity(adev, e).stopped
}

/// `drm_sched_entity_error`.
pub fn drm_sched_entity_error(adev: &mut amdgpu_device, e: usize) -> i32 {
    entity(adev, e).last_scheduled.as_ref().map_or(0, |f| f.error())
}

/// `drm_sched_entity_kill_jobs_work`.
fn drm_sched_entity_kill_jobs_work(adev: &mut amdgpu_device, j: u64) {
    let n = job(adev, j).dependencies.len();
    for index in 0..n {
        let Some(mut f) = job(adev, j).dependencies[index].clone() else { continue };
        let mut f_opt = Some(f.clone());
        if let Some(s) = to_drm_sched_fence(&f) {
            if Rc::ptr_eq(&f, &s.scheduled) {
                f_opt = s.finished;
            }
        }
        job(adev, j).dependencies[index] = None;
        if let Some(g) = f_opt {
            f = g;
            if dma_fence::dma_fence_add_callback(adev, &f, dma_fence_func::sched_entity_kill_jobs { job: j }).is_ok() {
                return;
            }
        }
    }
    let finished = job(adev, j).s_fence.clone().unwrap();
    drm_sched_fence_scheduled(adev, &finished, None);
    drm_sched_fence_finished(adev, &finished, -3);
    if s_fence_of(&finished).parent.borrow().is_some() {
        Line::new().s("WARNING: killed job has a parent fence").say();
    }
    crate::amdgpu_job::amdgpu_job_free_cb(adev, j);
}

/// `drm_sched_entity_kill_jobs_cb`: `schedule_work` runs it at once here,
/// with no lock held to wait out.
pub fn drm_sched_entity_kill_jobs_cb(adev: &mut amdgpu_device, j: u64) {
    drm_sched_entity_kill_jobs_work(adev, j);
}

/// `drm_sched_entity_kill`.
fn drm_sched_entity_kill(adev: &mut amdgpu_device, e: usize) {
    let Some(at) = entity(adev, e).rq else { return };
    entity(adev, e).stopped = true;
    drm_sched_rq_remove_entity(adev, at, e);
    // wait_for_completion(&entity->entity_idle): the job runs on this
    // thread, so the entity is idle here.
    let mut prev = entity(adev, e).last_scheduled.clone();
    while let Some(j) = entity(adev, e).job_queue.pop_front() {
        let finished = job(adev, j).s_fence.clone().unwrap();
        let added = prev.as_ref().is_some_and(|p| {
            dma_fence::dma_fence_add_callback(adev, p, dma_fence_func::sched_entity_kill_jobs { job: j }).is_ok()
        });
        if !added {
            drm_sched_entity_kill_jobs_cb(adev, j);
        }
        prev = Some(finished);
    }
}

/// `drm_sched_entity_flush`: the caller is not exiting, so it waits for
/// the entity to empty and kills nothing.
pub fn drm_sched_entity_flush(adev: &mut amdgpu_device, e: usize, timeout: i64) -> i64 {
    if entity(adev, e).rq.is_none() {
        return 0;
    }
    workqueue::wait_until(adev, MAX_SCHEDULE_TIMEOUT, |adev| drm_sched_entity_is_idle(adev, e));
    timeout
}

/// `drm_sched_entity_fini`.
pub fn drm_sched_entity_fini(adev: &mut amdgpu_device, e: usize) {
    // If consumption of existing IBs wasn't completed. Forcefully remove
    // them here. Also makes sure that the scheduler won't touch this
    // entity any more.
    drm_sched_entity_kill(adev, e);
    if let Some(dep) = entity(adev, e).dependency.take() {
        if let Some(id) = entity(adev, e).cb.take() {
            dma_fence::dma_fence_remove_callback(&dep, id);
        }
    }
    entity(adev, e).last_scheduled = None;
}

/// `drm_sched_entity_destroy`: flushed and finished, and its key freed.
pub fn drm_sched_entity_destroy(adev: &mut amdgpu_device, e: usize) {
    drm_sched_entity_flush(adev, e, MAX_WAIT_SCHED_ENTITY_Q_EMPTY);
    drm_sched_entity_fini(adev, e);
    adev.sched_entities[e] = None;
}

/// `drm_sched_entity_wakeup`.
pub fn drm_sched_entity_wakeup(adev: &mut amdgpu_device, e: usize) {
    let ent = entity(adev, e);
    ent.dependency = None;
    ent.cb = None;
    let r = ent.rq.unwrap().0;
    drm_sched_wakeup(adev, r);
}

/// `drm_sched_entity_add_dependency_cb`: true when the entity waits.
fn drm_sched_entity_add_dependency_cb(adev: &mut amdgpu_device, e: usize) -> bool {
    let (sched, _) = entity(adev, e).rq.unwrap();
    let fence = entity(adev, e).dependency.clone().unwrap();
    let ctx = entity(adev, e).fence_context;
    if fence.context == ctx || fence.context == ctx + 1 {
        // Fence is a scheduled/finished fence from a job which belongs to
        // the same entity, we can ignore fences from ourself
        entity(adev, e).dependency = None;
        return false;
    }
    if let Some(s) = to_drm_sched_fence(&fence) {
        if fence.error() == 0 && s.sched == sched && !fence.test_flag(DRM_SCHED_FENCE_DONT_PIPELINE) {
            // Fence is from the same scheduler, only need to wait for it
            // to be scheduled
            entity(adev, e).dependency = Some(s.scheduled);
        }
    }
    let dep = entity(adev, e).dependency.clone().unwrap();
    match dma_fence::dma_fence_add_callback(adev, &dep, dma_fence_func::sched_entity_wakeup { entity: e }) {
        Ok(id) => {
            entity(adev, e).cb = Some(id);
            true
        }
        Err(_) => {
            entity(adev, e).dependency = None;
            false
        }
    }
}

/// `drm_sched_job_dependency`.
fn drm_sched_job_dependency(adev: &mut amdgpu_device, j: u64, e: usize) -> Option<Fence> {
    let jb = job(adev, j);
    if let Some(Some(f)) = jb.dependencies.get(jb.last_dependency).cloned() {
        jb.last_dependency += 1;
        return Some(f);
    }
    crate::amdgpu_job::amdgpu_job_prepare_job(adev, j, e)
}

/// `drm_sched_entity_pop_job`.
fn drm_sched_entity_pop_job(adev: &mut amdgpu_device, e: usize) -> Option<u64> {
    let j = *entity(adev, e).job_queue.front()?;
    loop {
        let dep = drm_sched_job_dependency(adev, j, e);
        entity(adev, e).dependency = dep;
        if entity(adev, e).dependency.is_none() {
            break;
        }
        if drm_sched_entity_add_dependency_cb(adev, e) {
            return None;
        }
    }
    let finished = job(adev, j).s_fence.clone().unwrap();
    // skip jobs from entity that marked guilty
    if entity(adev, e).guilty.as_ref().is_some_and(|g| g.get() != 0) {
        dma_fence::dma_fence_set_error(&finished, -125);
    }
    entity(adev, e).last_scheduled = Some(finished);
    entity(adev, e).job_queue.pop_front();
    // Update the entity's location in the min heap according to the
    // timestamp of the next job, if any.
    if let Some(&next) = entity(adev, e).job_queue.front() {
        let at = entity(adev, e).rq.unwrap();
        let ts = job(adev, next).submit_ts;
        drm_sched_rq_update_fifo_locked(adev, e, at, ts);
    }
    // Jobs and entities might have different lifecycles. Since we're
    // removing the job from the entities queue, set the jobs entity
    // pointer to NULL to prevent any future access of the entity through
    // this job.
    job(adev, j).entity = None;
    Some(j)
}

/// `drm_sched_entity_select_rq`.
fn drm_sched_entity_select_rq(adev: &mut amdgpu_device, e: usize) {
    let Some(list) = entity(adev, e).sched_list.clone() else { return };
    if !entity(adev, e).job_queue.is_empty() {
        return;
    }
    if let Some(f) = entity(adev, e).last_scheduled.clone() {
        if !dma_fence::dma_fence_is_signaled(adev, &f) {
            return;
        }
    }
    let prio = entity(adev, e).priority;
    let rq_new = drm_sched_pick_best(adev, &list).map(|s| (s, prio));
    let old = entity(adev, e).rq;
    if rq_new != old {
        if let Some(at) = old {
            drm_sched_rq_remove_entity(adev, at, e);
        }
        entity(adev, e).rq = rq_new;
    }
    if entity(adev, e).num_sched_list == 1 {
        entity(adev, e).sched_list = None;
    }
}

/// `drm_sched_entity_push_job`.
pub fn drm_sched_entity_push_job(adev: &mut amdgpu_device, j: u64) {
    let e = job(adev, j).entity.unwrap();
    let at = entity(adev, e).rq.unwrap();
    adev.rings[at.0].sched.score += 1;
    let submit_ts = crate::linux::now_us();
    job(adev, j).submit_ts = submit_ts;
    let first = entity(adev, e).job_queue.is_empty();
    entity(adev, e).job_queue.push_back(j);
    // first job wakes up scheduler
    if first {
        if entity(adev, e).stopped {
            Line::new().s("Trying to push to a killed entity").say();
            return;
        }
        let at = entity(adev, e).rq.unwrap();
        drm_sched_rq_add_entity(adev, at, e);
        drm_sched_rq_update_fifo_locked(adev, e, at, submit_ts);
        drm_sched_wakeup(adev, at.0);
    }
}
