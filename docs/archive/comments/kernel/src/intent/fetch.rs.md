# `kernel/src/intent/fetch.rs` @ 5e0102684

## L1-24 · `use alloc::string::String;`

```
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
```

## L33-36 · `const MAX_JOBS: usize = 16;`

```
/// In-flight + finished-but-uncollected jobs, all callers together. A browser
/// holds three at once (the document, its stylesheets, one sub-resource
/// batch), so this is room for several of them side by side without letting
/// one app fill the table.
```

## L39-41 · `const MAX_JOBS_PER_OWNER: usize = 4;`

```
/// …and the per-caller share of it, so one buggy module cannot take the table
/// away from every other. Three is what a page load needs; the fourth is
/// headroom for the turn where a navigation overlaps the batch it cancels.
```

## L44 · `const MAX_URLS: usize = 64;`

```
/// Same bound `npk_http_request_many` applies to one batch.
```

## L47-52 · `const MAX_RESERVED_BYTES: usize = 64 * 1024 * 1024;`

```
/// Bytes all pending answers may reserve at once. Reserved at `begin` from
/// what the caller asked for (not what arrives — that is unknown until it
/// does), released at `take` or `cancel`. A browser reserves ~17 MiB for one
/// page load (3 MiB document + 8 MiB stylesheets + 6 MiB sub-resources), so
/// this is three of those at the same time and then a refusal instead of a
/// kernel heap the size of the caller's ambition.
```

## L55-68 · `const WORKER_COUNT: usize = 1;`

```
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
```

## L71-74 · `const WORKER_STACK_BYTES: usize = 512 * 1024;`

```
/// TLS handshake + gzip inflate + the h2 frame loop is a deep chain, and a
/// fiber stack has no guard page — an overflow is a silent memory smash, not
/// a fault. The 9p persist worker took 1 MiB for the same reason; this chain
/// is shallower (no B-tree COW), so half of that.
```

## L77-80 · `const IDLE_TICKS: u64 = 200;`

```
/// Idle ticks (100 Hz) a worker stays alive after its last job before it
/// ends. Kept alive over the gap between a document and its stylesheets, so
/// an ordinary page load never pays a re-spawn; gone long before the machine
/// is idle, so nothing wakes a core to look at an empty queue.
```

## L83-84 · `enum Work {`

```
/// What a job asked for. Two shapes because the client has two entry points,
/// and the answers have different shapes too.
```

## L86 · `One {`

```
/// One request, with everything `npk_http_send` allows.
```

## L94-95 · `tls: bool,`

```
/// Ueber TLS? `false` heisst Klartext — `parse_url` laesst das nur
/// unter der Politik in `plain_http_allowed` zu.
```

## L97-98 · `from_reach: Option<http::Reach>,`

```
/// Die Reichweite des Dokuments, das die Anfrage ausloest.
/// Siehe `http::Reach` und `docs/plan/BROWSER_FETCH_ORIGIN.md` §3.1.
```

## L101 · `Many { urls: Vec<String>, cookies: Vec<String>, cap: usize, from_reach: Option<http::Reach> },`

```
/// A batch, multiplexed per host exactly as `npk_http_request_many` does.
```

## L105 · `pub(crate) struct Reply {`

```
/// What came back. `error` empty means it worked.
```

## L108 · `pub lens: Vec<i32>,`

```
/// `Many` only: bytes per URL in request order, -1 for one that failed.
```

## L114 · `pub error: String,`

```
/// `kind\tmessage`, the same pair `npk_http_last_error` hands back.
```

## L126-128 · `owner: u32,`

```
/// The pid of the module that started it. A handle is only ever answered
/// to its owner — otherwise one sandboxed app could read another app's
/// document by guessing a small integer.
```

## L136-137 · `next_id: i32,`

```
/// Handles never repeat within a boot, so a stale handle from a job that
/// was already taken cannot name a fresh one in the same slot.
```

## L150-151 · `static CANCEL: [AtomicBool; MAX_JOBS] = [const { AtomicBool::new(false) }; MAX_JOBS];`

```
/// Per-slot cancel flag, outside the lock so the transfer sink can read it
/// per chunk without taking one.
```

## L154 · `pub(crate) fn begin_one(`

```
// ── Starting ───────────────────────────────────────────────────────────────
```

## L192-197 · `if owner == 0 {`

```
// A handle is only ever answered to its owner, and pid 0 is not an owner:
// it is what the inline execution paths (`execute_inner`, `execute_wasi`
// and their forge twins) leave in the state, so every one of them would
// share one identity and could collect another's answer. Those paths run a
// module to completion with nothing else to do meanwhile — the synchronous
// `npk_http_send` is exactly right for them.
```

## L213-214 · `q.next_id = if q.next_id == i32::MAX { 1 } else { q.next_id + 1 };`

```
// Never 0 (a caller may read it as "no handle") and never negative
// (every host fn reserves those for errors).
```

## L220-222 · `let queued = q.slots.iter().flatten()`

```
// One more worker only while there is more work than workers — a
// second document does not deserve a second core if the first one is
// still queued behind nothing.
```

## L233 · `if let Some(k) = spawn {`

```
// Outside the lock: `admit_with_stack` allocates the stack.
```

## L241-250 · `fn worker_core(caller_core: usize, k: usize) -> usize {`

```
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
```

## L272 · `pub(crate) fn poll(owner: u32, id: i32) -> i32 {`

```
// ── Asking, collecting, dropping ───────────────────────────────────────────
```

## L274-275 · `pub(crate) fn poll(owner: u32, id: i32) -> i32 {`

```
/// 1 = an answer is waiting, 0 = still running, -1 = it failed (the reason
/// comes with `take`), -2 = no such handle for this owner.
```

## L288-290 · `pub(crate) fn result_count(owner: u32, id: i32) -> Option<usize> {`

```
/// How many entries a finished batch has, without collecting it. `None`
/// while it is still running or if the handle is unknown — a caller needs
/// this to size its length table before `take` destroys the job.
```

## L300-301 · `Got(Reply),`

```
/// The job is finished and gone; the answer is here (`error` says whether
/// it worked).
```

## L303 · `NotReady,`

```
/// Still running — the job is kept, ask again later.
```

## L305 · `Unknown,`

```
/// Never existed under this owner, or was already taken.
```

## L324-325 · `other => {`

```
// Cannot happen — the phase was checked under this same lock — but a
// kernel panics on `expect`, and this costs one arm.
```

## L335-339 · `pub(crate) fn cancel(owner: u32, id: i32) {`

```
/// Give up on a handle. A queued job is dropped, a finished one's answer is
/// thrown away, and a running one is marked — its worker stops at the next
/// chunk and discards what it has. Idempotent, and silent about a handle that
/// is already gone: a browser cancels on every navigation and must not have
/// to know which of the three states it caught.
```

## L349 · `return; // the worker frees it in finish`

```
// the worker frees it in `finish`
```

## L356-358 · `pub(crate) fn release_owner(owner: u32) {`

```
/// Drop everything a module left behind when its instance ends. Without this
/// a browser that is closed mid-load leaks its slots (and its megabytes of
/// reservation) until the next boot.
```

## L376 · `fn worker_entry(_k: u64) {`

```
// ── The worker ─────────────────────────────────────────────────────────────
```

## L392-393 · `other => {`

```
// Cannot happen (matched under this lock); put it
// back rather than panic on the impossible.
```

## L403-405 · `if crate::interrupts::ticks().wrapping_sub(idle_since) > IDLE_TICKS {`

```
// The queue check and the worker count leave together, so
// a `begin` that lands between them either sees a worker
// that is still counted (and queues) or spawns a new one.
```

## L418 · `let reply = run(slot, work);`

```
// The exchange itself, with NO lock held: it waits for a network.
```

## L425 · `continue; // released under us (owner died) — nothing to publish`

```
// released under us (owner died) — nothing to publish
```

## L448-449 · `accept_gzip: tls,`

```
// Both as `npk_http_send` sets them: the kernel unpacks gzip,
// and the browser is the caller h2 was turned on for.
```

## L458-459 · `if CANCEL[slot].load(Ordering::Acquire) {`

```
// The only place a running exchange can be stopped. One
// atomic per chunk, not per byte.
```

## L482-484 · `Err(e) => Reply {`

```
// Everything else stays empty on failure, for the reason
// `npk_http_send` clears it: a caller must never read one
// request's answer and attribute it to the next.
```

## L498-501 · `let mut blob: Vec<u8> = Vec::new();`

```
// Packed here rather than at `take`, so the guest side is a plain
// copy: bodies back to back, one length each, and one that would
// overrun the budget is DROPPED rather than truncated — half an
// image decodes to garbage, a missing one draws a placeholder.
```

