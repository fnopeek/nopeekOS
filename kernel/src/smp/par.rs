//! Fork-join over `0..n`: the caller and idle worker cores take indices
//! from one counter until none are left, as a padata job spreads work over
//! CPUs. The caller always works too, so a job finishes even when no worker
//! is free; a helper that starts after the caller is done finds the job
//! closed and leaves without touching it.

use alloc::sync::Arc;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst};

struct Job {
    next: AtomicUsize,
    n: usize,
    /// Helpers between their check of `closed` and their return.
    active: AtomicUsize,
    closed: AtomicBool,
    /// The caller's closure; called only while `closed` is false.
    work: *const (dyn Fn(usize) + Sync),
}

// SAFETY: `work` points at a `Sync` closure, and `for_each` keeps it alive
// for every call made through it (see there).
unsafe impl Send for Job {}
// SAFETY: as for `Send`; every other field is atomic.
unsafe impl Sync for Job {}

impl Job {
    fn drain(&self) {
        loop {
            let i = self.next.fetch_add(1, SeqCst);
            if i >= self.n { return; }
            // SAFETY: `work` is alive while a participant drains (see
            // `for_each`).
            unsafe { (*self.work)(i) };
        }
    }
}

/// Call `f(i)` for every `i` in `0..n`, on this core and on up to `n - 1`
/// idle worker cores. Returns when every call has returned. The caller must
/// hold no lock that `f` takes.
pub fn for_each(n: usize, f: &(dyn Fn(usize) + Sync)) {
    let helpers = n.saturating_sub(1).min(super::scheduler::worker_count());
    if helpers == 0 {
        for i in 0..n { f(i); }
        return;
    }
    // SAFETY: only the lifetime is erased. A helper calls `work` only after
    // counting itself in `active` and seeing `closed` false; this function
    // sets `closed` and then waits until `active` is zero before it returns,
    // and with sequentially consistent ordering a helper either sees `closed`
    // or is seen in `active`. So `f` outlives every call.
    let work: *const (dyn Fn(usize) + Sync + 'static) =
        unsafe { core::mem::transmute(f as *const (dyn Fn(usize) + Sync + '_)) };
    let job = Arc::new(Job {
        next: AtomicUsize::new(0),
        n,
        active: AtomicUsize::new(0),
        closed: AtomicBool::new(false),
        work,
    });
    for _ in 0..helpers {
        super::scheduler::spawn("par", helper, Arc::into_raw(job.clone()) as u64);
    }
    job.drain();
    job.closed.store(true, SeqCst);
    while job.active.load(SeqCst) != 0 {
        core::hint::spin_loop();
    }
}

fn helper(arg: u64) {
    // SAFETY: `arg` is the `Arc<Job>` that `for_each` leaked for this task.
    let job = unsafe { Arc::from_raw(arg as *const Job) };
    job.active.fetch_add(1, SeqCst);
    if !job.closed.load(SeqCst) {
        job.drain();
    }
    job.active.fetch_sub(1, SeqCst);
}
