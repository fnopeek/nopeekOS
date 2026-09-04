//! Fetching that does not stand still.
//!
//! `npk_http_send` is synchronous: the calling module sits INSIDE the host
//! call for the whole exchange — DNS, TCP, TLS, the wait for the first byte —
//! and a fiber in a host call cannot paint, cannot read a key and cannot let
//! its peers run (`feedback_wasm_host_call_freezes_peer_fibers`). For a
//! browser that is the whole complaint: a server that goes quiet freezes the
//! window, and no timeout is short enough to make freezing acceptable.
//!
//! So the wait moves off the caller's stack. A module hands in a request and
//! gets a HANDLE; a worker fiber on ANOTHER core runs exactly the same
//! synchronous client (`https_request_streaming` / `https_get_many` —
//! unchanged, and there is deliberately no second HTTP implementation here);
//! the module asks `poll` between two frames and collects the answer with
//! `take`.
//!
//! Two things this does NOT do, on purpose:
//!
//! - It does not make the HTTP client asynchronous. The blocking recv loops
//!   are still blocking; they now block a fiber nobody is waiting on.
//! - It does not interrupt a running exchange. `cancel` takes effect at the
//!   next chunk boundary and otherwise just discards the answer — which is
//!   all the caller can observe, since it stopped waiting the moment it got
//!   its handle.

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

use super::http;

/// In-flight + finished-but-uncollected jobs, all callers together. A browser
/// holds three at once (the document, its stylesheets, one sub-resource
/// batch), so this is room for several of them side by side without letting
/// one app fill the table.
const MAX_JOBS: usize = 16;

/// …and the per-caller share of it, so one buggy module cannot take the table
/// away from every other. Three is what a page load needs; the fourth is
/// headroom for the turn where a navigation overlaps the batch it cancels.
const MAX_JOBS_PER_OWNER: usize = 4;

/// Same bound `npk_http_request_many` applies to one batch.
const MAX_URLS: usize = 64;

/// Bytes all pending answers may reserve at once. Reserved at `begin` from
/// what the caller asked for (not what arrives — that is unknown until it
/// does), released at `take` or `cancel`. A browser reserves ~17 MiB for one
/// page load (3 MiB document + 8 MiB stylesheets + 6 MiB sub-resources), so
/// this is three of those at the same time and then a refusal instead of a
/// kernel heap the size of the caller's ambition.
const MAX_RESERVED_BYTES: usize = 64 * 1024 * 1024;

/// How many fetch workers may run at once.
///
/// ONE, deliberately. Two would let a click start its document while the
/// picture batch it replaces is still on the wire — but it would also make
/// PARALLEL use of `intent::http` the normal case, and that client has never
/// run that way: the connection pools are spin-locked, and `pool_take` closes
/// a stale session while holding the lock. Whether that is safe under two
/// callers is a question to answer by reading it, not by assuming it.
///
/// What one worker costs is bounded and visible: a navigation started while a
/// sub-resource batch is running waits out that batch (one round trip, order
/// 100-300 ms) before its own request goes out. The window stays alive the
/// whole time — which was the entire complaint. Raising this is one constant,
/// once the parallel-safety question above has actually been looked at.
const WORKER_COUNT: usize = 1;

/// TLS handshake + gzip inflate + the h2 frame loop is a deep chain, and a
/// fiber stack has no guard page — an overflow is a silent memory smash, not
/// a fault. The 9p persist worker took 1 MiB for the same reason; this chain
/// is shallower (no B-tree COW), so half of that.
const WORKER_STACK_BYTES: usize = 512 * 1024;

/// Idle ticks (100 Hz) a worker stays alive after its last job before it
/// ends. Kept alive over the gap between a document and its stylesheets, so
/// an ordinary page load never pays a re-spawn; gone long before the machine
/// is idle, so nothing wakes a core to look at an empty queue.
const IDLE_TICKS: u64 = 200;

/// What a job asked for. Two shapes because the client has two entry points,
/// and the answers have different shapes too.
enum Work {
    /// One request, with everything `npk_http_send` allows.
    One {
        method: String,
        host: String,
        path: String,
        headers: Vec<String>,
        body: Vec<u8>,
        cap: usize,
        /// Ueber TLS? `false` heisst Klartext — `parse_url` laesst das nur
        /// unter der Politik in `plain_http_allowed` zu.
        tls: bool,
    },
    /// A batch, multiplexed per host exactly as `npk_http_request_many` does.
    Many { urls: Vec<String>, cap: usize },
}

/// What came back. `error` empty means it worked.
pub(crate) struct Reply {
    pub body: Vec<u8>,
    /// `Many` only: bytes per URL in request order, -1 for one that failed.
    pub lens: Vec<i32>,
    pub status: u16,
    pub headers: String,
    pub final_url: String,
    pub content_type: String,
    /// `kind\tmessage`, the same pair `npk_http_last_error` hands back.
    pub error: String,
}

enum Phase {
    Queued(Work),
    Running,
    Ready(Reply),
}

struct Job {
    id: i32,
    /// The pid of the module that started it. A handle is only ever answered
    /// to its owner — otherwise one sandboxed app could read another app's
    /// document by guessing a small integer.
    owner: u32,
    reserved: usize,
    phase: Phase,
}

struct Queue {
    slots: [Option<Job>; MAX_JOBS],
    /// Handles never repeat within a boot, so a stale handle from a job that
    /// was already taken cannot name a fresh one in the same slot.
    next_id: i32,
    workers: usize,
    reserved: usize,
}

static Q: Mutex<Queue> = Mutex::new(Queue {
    slots: [const { None }; MAX_JOBS],
    next_id: 1,
    workers: 0,
    reserved: 0,
});

/// Per-slot cancel flag, outside the lock so the transfer sink can read it
/// per chunk without taking one.
static CANCEL: [AtomicBool; MAX_JOBS] = [const { AtomicBool::new(false) }; MAX_JOBS];

// ── Starting ───────────────────────────────────────────────────────────────

pub(crate) fn begin_one(
    owner: u32,
    caller_core: usize,
    method: String,
    host: String,
    path: String,
    headers: Vec<String>,
    body: Vec<u8>,
    cap: usize,
    tls: bool,
) -> Result<i32, &'static str> {
    submit(owner, caller_core, cap, Work::One { method, host, path, headers, body, cap, tls })
}

pub(crate) fn begin_many(
    owner: u32,
    caller_core: usize,
    urls: Vec<String>,
    cap: usize,
) -> Result<i32, &'static str> {
    if urls.is_empty() || urls.len() > MAX_URLS {
        return Err("bad url list");
    }
    submit(owner, caller_core, cap, Work::Many { urls, cap })
}

fn submit(
    owner: u32,
    caller_core: usize,
    cap: usize,
    work: Work,
) -> Result<i32, &'static str> {
    // A handle is only ever answered to its owner, and pid 0 is not an owner:
    // it is what the inline execution paths (`execute_inner`, `execute_wasi`
    // and their forge twins) leave in the state, so every one of them would
    // share one identity and could collect another's answer. Those paths run a
    // module to completion with nothing else to do meanwhile — the synchronous
    // `npk_http_send` is exactly right for them.
    if owner == 0 {
        return Err("async fetch needs a process");
    }
    let (id, spawn) = {
        let mut q = Q.lock();
        if q.slots.iter().flatten().filter(|j| j.owner == owner).count() >= MAX_JOBS_PER_OWNER {
            return Err("too many fetches in flight");
        }
        if q.reserved + cap > MAX_RESERVED_BYTES {
            return Err("fetch buffer budget exhausted");
        }
        let Some(slot) = q.slots.iter().position(|s| s.is_none()) else {
            return Err("fetch queue full");
        };
        let id = q.next_id;
        // Never 0 (a caller may read it as "no handle") and never negative
        // (every host fn reserves those for errors).
        q.next_id = if q.next_id == i32::MAX { 1 } else { q.next_id + 1 };
        q.reserved += cap;
        CANCEL[slot].store(false, Ordering::Release);
        q.slots[slot] = Some(Job { id, owner, reserved: cap, phase: Phase::Queued(work) });

        // One more worker only while there is more work than workers — a
        // second document does not deserve a second core if the first one is
        // still queued behind nothing.
        let queued = q.slots.iter().flatten()
            .filter(|j| matches!(j.phase, Phase::Queued(_))).count();
        let spawn = if q.workers < WORKER_COUNT && q.workers < queued {
            q.workers += 1;
            Some(q.workers - 1)
        } else {
            None
        };
        (id, spawn)
    };
    // Outside the lock: `admit_with_stack` allocates the stack.
    if let Some(k) = spawn {
        let core = worker_core(caller_core, k);
        crate::smp::fiber::admit_with_stack(core, worker_entry, k as u64, WORKER_STACK_BYTES);
    }
    Ok(id)
}

/// Where a fetch worker runs.
///
/// Never Core 0 (the shell and the compositor are the machine's keep-alive
/// minimum), never the microvm's dedicated core (it never enters the fiber
/// scheduler, so a worker admitted there would never run at all), and — the
/// point of the whole exercise — never the caller's, because the recv loops
/// spin rather than yield and would freeze the very app they are fetching for.
///
/// If nothing is left, share the caller's core: that is exactly today's
/// behaviour (the app stood still inside the host call) and no worse.
fn worker_core(caller_core: usize, k: usize) -> usize {
    let n = crate::smp::per_core::core_count();
    if n <= 1 {
        return 0;
    }
    let vm = crate::smp::per_core::dedicated_vm_core();
    let mut ok = [0usize; 64];
    let mut m = 0;
    for c in 1..n.min(64) {
        if c == caller_core || Some(c) == vm || m == ok.len() {
            continue;
        }
        ok[m] = c;
        m += 1;
    }
    if m == 0 {
        return if caller_core == 0 { n - 1 } else { caller_core };
    }
    ok[k % m]
}

// ── Asking, collecting, dropping ───────────────────────────────────────────

/// 1 = an answer is waiting, 0 = still running, -1 = it failed (the reason
/// comes with `take`), -2 = no such handle for this owner.
pub(crate) fn poll(owner: u32, id: i32) -> i32 {
    let q = Q.lock();
    match q.slots.iter().flatten().find(|j| j.id == id && j.owner == owner) {
        Some(j) => match &j.phase {
            Phase::Ready(r) if r.error.is_empty() => 1,
            Phase::Ready(_) => -1,
            _ => 0,
        },
        None => -2,
    }
}

/// How many entries a finished batch has, without collecting it. `None`
/// while it is still running or if the handle is unknown — a caller needs
/// this to size its length table before `take` destroys the job.
pub(crate) fn result_count(owner: u32, id: i32) -> Option<usize> {
    let q = Q.lock();
    match q.slots.iter().flatten().find(|j| j.id == id && j.owner == owner) {
        Some(Job { phase: Phase::Ready(r), .. }) => Some(r.lens.len()),
        _ => None,
    }
}

pub(crate) enum Take {
    /// The job is finished and gone; the answer is here (`error` says whether
    /// it worked).
    Got(Reply),
    /// Still running — the job is kept, ask again later.
    NotReady,
    /// Never existed under this owner, or was already taken.
    Unknown,
}

pub(crate) fn take(owner: u32, id: i32) -> Take {
    let mut q = Q.lock();
    let Some(slot) = q.slots.iter().position(|s| {
        matches!(s, Some(j) if j.id == id && j.owner == owner)
    }) else {
        return Take::Unknown;
    };
    if !matches!(q.slots[slot].as_ref().map(|j| &j.phase), Some(Phase::Ready(_))) {
        return Take::NotReady;
    }
    match q.slots[slot].take() {
        Some(Job { reserved, phase: Phase::Ready(r), .. }) => {
            q.reserved = q.reserved.saturating_sub(reserved);
            Take::Got(r)
        }
        // Cannot happen — the phase was checked under this same lock — but a
        // kernel panics on `expect`, and this costs one arm.
        other => {
            if let Some(j) = other {
                q.reserved = q.reserved.saturating_sub(j.reserved);
            }
            Take::Unknown
        }
    }
}

/// Give up on a handle. A queued job is dropped, a finished one's answer is
/// thrown away, and a running one is marked — its worker stops at the next
/// chunk and discards what it has. Idempotent, and silent about a handle that
/// is already gone: a browser cancels on every navigation and must not have
/// to know which of the three states it caught.
pub(crate) fn cancel(owner: u32, id: i32) {
    let mut q = Q.lock();
    let Some(slot) = q.slots.iter().position(|s| {
        matches!(s, Some(j) if j.id == id && j.owner == owner)
    }) else {
        return;
    };
    if matches!(q.slots[slot].as_ref().map(|j| &j.phase), Some(Phase::Running)) {
        CANCEL[slot].store(true, Ordering::Release);
        return; // the worker frees it in `finish`
    }
    if let Some(job) = q.slots[slot].take() {
        q.reserved = q.reserved.saturating_sub(job.reserved);
    }
}

/// Drop everything a module left behind when its instance ends. Without this
/// a browser that is closed mid-load leaks its slots (and its megabytes of
/// reservation) until the next boot.
pub(crate) fn release_owner(owner: u32) {
    let mut q = Q.lock();
    for slot in 0..MAX_JOBS {
        let is_owner = matches!(&q.slots[slot], Some(j) if j.owner == owner);
        if !is_owner {
            continue;
        }
        if matches!(q.slots[slot].as_ref().map(|j| &j.phase), Some(Phase::Running)) {
            CANCEL[slot].store(true, Ordering::Release);
            continue;
        }
        if let Some(job) = q.slots[slot].take() {
            q.reserved = q.reserved.saturating_sub(job.reserved);
        }
    }
}

// ── The worker ─────────────────────────────────────────────────────────────

fn worker_entry(_k: u64) {
    let mut idle_since = crate::interrupts::ticks();
    loop {
        let picked = {
            let mut q = Q.lock();
            let next = q.slots.iter().position(|s| {
                matches!(s, Some(j) if matches!(j.phase, Phase::Queued(_)))
            });
            match next {
                Some(slot) => match q.slots[slot].as_mut() {
                    Some(job) => {
                        let id = job.id;
                        match core::mem::replace(&mut job.phase, Phase::Running) {
                            Phase::Queued(work) => Some((slot, id, work)),
                            // Cannot happen (matched under this lock); put it
                            // back rather than panic on the impossible.
                            other => {
                                job.phase = other;
                                None
                            }
                        }
                    }
                    None => None,
                },
                None => {
                    // The queue check and the worker count leave together, so
                    // a `begin` that lands between them either sees a worker
                    // that is still counted (and queues) or spawns a new one.
                    if crate::interrupts::ticks().wrapping_sub(idle_since) > IDLE_TICKS {
                        q.workers -= 1;
                        return;
                    }
                    None
                }
            }
        };
        let Some((slot, id, work)) = picked else {
            crate::smp::fiber::yield_sleep(4);
            continue;
        };
        // The exchange itself, with NO lock held: it waits for a network.
        let reply = run(slot, work);
        idle_since = crate::interrupts::ticks();

        let mut q = Q.lock();
        let still_ours = matches!(&q.slots[slot], Some(j) if j.id == id);
        if !still_ours {
            continue; // released under us (owner died) — nothing to publish
        }
        if CANCEL[slot].load(Ordering::Acquire) {
            if let Some(job) = q.slots[slot].take() {
                q.reserved = q.reserved.saturating_sub(job.reserved);
            }
            continue;
        }
        if let Some(job) = q.slots[slot].as_mut() {
            job.phase = Phase::Ready(reply);
        }
    }
}

fn run(slot: usize, work: Work) -> Reply {
    match work {
        Work::One { method, host, path, headers, body, cap, tls } => {
            let mut out: Vec<u8> = Vec::new();
            let mut info = http::FetchInfo::default();
            let req = http::HttpRequest {
                method: &method,
                headers: &headers,
                body: &body,
                // Both as `npk_http_send` sets them: the kernel unpacks gzip,
                // and the browser is the caller h2 was turned on for.
                accept_gzip: tls,
                try_h2: tls,
                plain: !tls,
            };
            let res = http::https_request_streaming(
                &host, &path, &req, cap,
                &mut |chunk: &[u8]| -> Result<(), &'static str> {
                    // The only place a running exchange can be stopped. One
                    // atomic per chunk, not per byte.
                    if CANCEL[slot].load(Ordering::Acquire) {
                        return Err("cancelled");
                    }
                    if out.len() < cap {
                        let take = chunk.len().min(cap - out.len());
                        out.extend_from_slice(&chunk[..take]);
                    }
                    Ok(())
                },
                Some(&mut info),
                true,
            );
            match res {
                Ok(_) => Reply {
                    body: out,
                    lens: Vec::new(),
                    status: info.status,
                    headers: info.headers,
                    final_url: info.final_url,
                    content_type: info.content_type,
                    error: String::new(),
                },
                // Everything else stays empty on failure, for the reason
                // `npk_http_send` clears it: a caller must never read one
                // request's answer and attribute it to the next.
                Err(e) => Reply {
                    body: Vec::new(),
                    lens: Vec::new(),
                    status: 0,
                    headers: String::new(),
                    final_url: String::new(),
                    content_type: String::new(),
                    error: alloc::format!("{}\t{}", http::error_kind(e), e),
                },
            }
        }
        Work::Many { urls, cap } => {
            let bodies = http::https_get_many(&urls, cap);
            // Packed here rather than at `take`, so the guest side is a plain
            // copy: bodies back to back, one length each, and one that would
            // overrun the budget is DROPPED rather than truncated — half an
            // image decodes to garbage, a missing one draws a placeholder.
            let mut blob: Vec<u8> = Vec::new();
            let mut lens: Vec<i32> = Vec::new();
            for body in bodies {
                let n: i32 = match body {
                    Some(b) if blob.len() + b.len() <= cap => {
                        blob.extend_from_slice(&b);
                        b.len() as i32
                    }
                    _ => -1,
                };
                lens.push(n);
            }
            Reply {
                body: blob,
                lens,
                status: 0,
                headers: String::new(),
                final_url: String::new(),
                content_type: String::new(),
                error: String::new(),
            }
        }
    }
}
