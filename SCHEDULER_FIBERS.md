# SCHEDULER_FIBERS.md — Fiber / Green-Thread Scheduler for WASM Apps

**Status:** Design / not started. Implementation kicks off in a fresh session.
**Stopgap shipped:** kernel v0.183.0 (idle HLT + interim 16 spawn slots).

---

## The problem

WASM apps don't scale past roughly the number of worker cores. Real
operating systems run thousands of processes on a handful of cores
because:

1. **Idle processes cost ~0 CPU** — they *block* (park), they don't spin.
2. **The scheduler multiplexes** many runnable tasks over few cores.
3. The practical limit is **CPU throughput when actually computing**, not
   a process counter.

nopeekOS has neither today.

### Why it breaks

- **Each resident app is an infinite loop that owns a core.** An app's
  `_start` is `loop { poll_event(); npk_sleep(16); }`. Its
  `wasm_worker_task` (`kernel/src/wasm.rs`) runs `_start` *to completion* —
  which never happens — so it holds its worker core for the app's entire
  lifetime.

- **Idle was 100% CPU (fixed v0.183.0).** `npk_sleep` busy-spun
  (`core::hint::spin_loop()`) when it had no helper work → a running app
  pinned its core at 100% even while doing nothing (confirmed: host `htop`
  showed every QEMU vCPU maxed; heavy power draw). v0.183.0 changed the
  idle path to **HLT until the next timer IRQ** (IF-preserving) → idle apps
  drop to ~0%. *This is a stopgap — the app still owns its core, just
  halted instead of spinning.*

- **`npk_sleep`'s inline-helper trick nests fatally.** While waiting,
  `npk_sleep` pulls the next scheduler task and runs it inline. Fine for a
  task that **returns** (an intent, a one-shot screenshot). Fatal for
  another **resident app**: its `_start` never returns → the helper core
  nests into it forever and the host app freezes. So with more apps than
  cores you get nesting + starvation. Core 0 = kernel / IRQ / input; the
  test NUC has 3 worker cores → `dock + bar + loft + iris` = 4 infinite
  loops on 3 cores → thrash.

- **`MAX_WASM_JOBS` is a fixed array and the wrong model.** Spawns go
  through a `[Option<WasmJob>; N]` slot array; a worker `.take()`s its slot
  the instant it starts, so `N` caps *pending* (not-yet-started) spawns.
  `N=4` overflowed (`[npk] No free WASM job slots`) once 4 apps were
  resident and a one-shot was queued. Bumped to 16 in v0.183.0 as **interim
  scaffolding only** — a fixed slot count just moves the wall.

---

### v0.183.0 idle-HLT was necessary but INSUFFICIENT (observed)

After v0.183.0 (npk_sleep HLTs when it has no helper work), the host still
showed **2–4 QEMU vCPUs pegged at ~100% while nopeekOS sat idle** at the
shell with the panels up. So the spin is NOT just npk_sleep's old
busy-wait. Static reading didn't isolate the culprit: no continuous
`scheduler::spawn` in hot paths, Core 0's intent `run_loop` HLTs when idle
(no VM). Open hypotheses to confirm with the built-in `top` (CORE
USAGE/QUEUE/ROLE) + per-core instrumentation during the rework:
- The npk_sleep HLT branch may never be reached because `next_task` keeps
  returning work (nested resident-app tasks pulled inline → an app's
  `_start` loop spins in a sub-loop), OR
- worker cores may lack a **per-core wake source** (the per-core APIC timer
  is a known TODO), so a worker can't HLT-and-wake cooperatively and ends
  up spinning, OR
- the per_core idle MWAIT/HLT path isn't engaging under load.
**Conclusion: point-patches won't fix idle-100%. The system must become
event-driven end to end (block on input/timer, never poll), which is
exactly the fiber rework below — plus likely a per-core timer as a
prerequisite.**

**Confirmed via `top` vs host htop (2026-05-30):** with only dock (core1) +
bar (core2) + the shell loop (core0) running, nopeekOS `top` reported
core0=100% (kernel/irq), core1=core2=0%, core3=idle 0% — but the host showed
cores pegged at ~100% INCLUDING the core `top` calls "idle" — and on the
bare-metal HW host **all 4 cores sit constantly at 100%** while nopeekOS
is idle. So (a) `top`'s perf counters are broken (USAGE + MHz both wrong;
`MHz=0`, `done=0` despite `spawned=5`, `steals=0` despite apps on worker
cores), and (b) **the MWAIT/HLT idle path does not actually quiesce the
vCPU** — even a genuinely idle worker core spins at the host level (MWAIT
likely wakes immediately and busy re-checks instead of staying asleep).
So the idle-100% is in the idle/wake path itself, not (only) the apps.
The real idle must MONITOR/MWAIT (or HLT) and STAY asleep until a genuine
wake — no spurious re-arming.

**Diagnosis step 0 (new session): the built-in `top` is buggy and needs a
full redesign — do NOT trust it.** Before touching the scheduler, add
fresh, trustworthy **per-core instrumentation**: for each core, what is it
doing right now — HLT/MWAIT-idle vs. running which task/app, plus run-queue
depth and a real busy% (busy-cycles / total-cycles since last sample).
That gives the data to pinpoint the idle-100% spinner directly. (Redesign
of the `top` app itself is a separate follow-up.)

**Step 0 SHIPPED — kernel v0.184.0 (`cores` / `cpu` command).** The fix is
to measure idle *directly* instead of trusting self-reported busy-TSC (which
can't tell a halted core from a spinning one — a spinner never accounts
itself idle, so it looks free while pegging the vCPU). New per-core counters
in `smp/per_core.rs` — `CORE_HALT_TSC` + `CORE_HALT_COUNT` — are incremented
via `record_halt(core, cycles)` at *every* genuine halt site, each bracketed
with `rdtsc`: worker `MWAIT` + `HLT` fallback + dedicated-VM `HLT`
(`per_core.rs`), `npk_sleep`'s `HLT` (`wasm.rs`), and Core 0's two shell-idle
`HLT`s (`intent/mod.rs`). The `cores` command (`intent/system.rs::intent_cores`,
serial/kprintln — bypasses the broken WASM `top`) double-samples those
counters over a 500 ms window and prints, per core: **BUSY% = 100 − halted%**,
**HALTS/s**, **avg C-state residency (µs)**, run-queue depth, and a measured
ROLE. Reading the table — this is what discriminates the open hypotheses:
- **BUSY ~100%, HALTS/s ≈ 0** → core is **SPINNING**, never reaches a halt
  (the prime suspect: `npk_input_wait` in `wasm.rs` is a pure
  `core::hint::spin_loop()` — confirmed by reading; resident dock/bar block
  there, not in `npk_sleep`'s HLT).
- **HALTS/s very high, residency tiny** → MWAIT/HLT wakes immediately and
  busy-re-checks (spurious-wake spin / no per-core wake source).
- **low BUSY, few HALTS/s, long residency** → genuinely asleep (healthy).
Validate `cores` in QEMU then on the NUC before Stage 1.

## The fix: fibers (stackful green threads)

wasmi cannot be paused mid-`_start` (no arbitrary preemption point; fuel
metering only *stops* execution, it can't *resume* mid-function). Stackful
coroutines sidestep that entirely:

- Each WASM app runs on its **own stack**; wasmi executes synchronously on
  it, exactly as today.
- A blocking host call (`npk_sleep`, and a new `npk_event_wait(timeout)`)
  performs a **context switch**: save the fiber's stack pointer +
  callee-saved registers, restore the scheduler's context. **The core is
  now free** to run another fiber or to HLT.
- A parked app holds **no core and burns 0 CPU**. It's just a saved stack
  sitting in a queue.
- The scheduler keeps a **run-queue of runnable fibers** and multiplexes
  them over the worker cores — cooperatively at yield points, optionally
  preemptively via the 100 Hz timer for compute-heavy fibers.
- A parked app becomes **runnable** when its wakeup fires: an input event
  routed to it, or its sleep deadline elapses. `npk_event_wait` registers
  that wakeup before yielding.

**Result:** thousands of mostly-idle apps cost ~0; only runnable fibers
draw cores; CPU is the only real limit. `MAX_WASM_JOBS` and the worker-core
count stop being binding constraints.

---

## Staged implementation plan

Each stage builds + boots + is validated in QEMU before the next (the
scheduler and boot path are stability-critical — see the demo-first /
test-on-HW rules in memory). AMD/QEMU first, NUC after.

1. **Context-switch primitive.** An asm routine that swaps `rsp` + the
   callee-saved registers (rbx, rbp, r12–r15) between a *current* and a
   *target* `Context { rsp: u64 }`. Save current, load target, `ret` into
   the target's saved return address. A per-fiber guard stack (e.g. 64 KB,
   guard page if practical). Unit-test by switching between two trivial
   contexts and back.

2. **Fiber per app.** Allocate a stack per `wasm_worker_task`; start
   `_start` on it via the switch primitive. Keep a **per-core scheduler
   context** to switch back to. On `_start` return, free the fiber + stack
   and switch back to the scheduler.

3. **Yield points.** Rework `npk_sleep` (and add `npk_event_wait`) to
   **switch back to the scheduler** instead of spinning / HLT-in-place. The
   fiber is re-queued: `sleep` with a wake-deadline (TSC), `event_wait`
   parked until an event (or timeout). Remove the inline-helper nesting
   hack — it's obsolete once fibers yield properly.

4. **Scheduler run-queue of fibers.** Ready-queue + a sleeping/parked set
   (keyed by wake-deadline / wait-reason). Each worker core's loop: pick a
   ready fiber, switch into it, run until it yields or returns; on yield
   re-queue, on return free it. Wake sleeping fibers whose deadline passed.
   Keep the existing Chase-Lev deque for *native* run-to-completion tasks
   (intents) — those don't need fibers; they just run on a core to
   completion as today.

5. **Wakeup registry + convert the apps.** Route input + timer to mark the
   right fibers runnable. Convert the resident apps (`dock`, `bar`, `loft`,
   `iris`, `spell`) from `poll + npk_sleep` to `npk_event_wait` so they
   park when idle and wake on input/timer. (SDK: add `npk_event_wait`.)

6. **Later — preemption + parallelism.** Timer-driven preemptive switch for
   compute-heavy fibers (fairness), and a way for one app to **fan a heavy
   task across cores** (Florian: "rechenintensive Sachen sollten multicore
   ziehen") — e.g. parallel PNG encode/decode / crypto.

---

## Notes / constraints

- **Core 0 stays the kernel / IRQ / input core** (fixed) — fibers run on
  the worker cores only.
- Native intent tasks (HTTP, OTA, install) keep running to completion on a
  worker core via the existing scheduler — they already return; no fiber
  needed.
- Validate boot stability at every stage; a broken context switch = triple
  fault. Keep a fallback path until stage 4 is solid.
- Once landed: drop the `MAX_WASM_JOBS` scaffolding back down / replace the
  fixed array with a dynamic queue, and delete the `npk_sleep` HLT-in-place
  stopgap (superseded by real yielding).

## Related

- `kernel/src/wasm.rs` — `wasm_worker_task`, `npk_sleep`, `WASM_JOBS`.
- `kernel/src/smp/scheduler.rs` — Chase-Lev deque, `next_task`, `spawn`,
  `WORK_AVAILABLE`.
- `kernel/src/smp/per_core.rs` — worker idle loop, MONITOR/MWAIT, HLT.
- Memory: `project_fiber_scheduler.md`, `project_resident_app_core_pinning.md`.
