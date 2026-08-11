# Phase 12.4 — Shade ↔ MicroVM Display Bridge (design spike)

> **Archiv (2026-08-11).** Der Spike ist gebaut: A1-A4 sind drin, die
> MicroVM erscheint als gekacheltes Fenster (nie fullscreen). Cross-Domain
> blieb wie vorgesehen de-scoped.

Status: **DESIGN, not implemented.** Spike before code, per decision
2026-05-15. Refines the "12.4 virtio-gpu cross-domain + Shade-Bridge"
roadmap line in `docs/archive/PHASE12_MICROVM.md` into a concrete architecture and
**de-scopes cross-domain** (it's a later performance axis, not the
architecture-defining piece).

## The invariant (non-negotiable)

nopeekOS is a tiling-WM OS. **Shade owns compositing; apps never own
the screen.** The Phase-10 Widget ABI + per-window scene pipeline
exist precisely for this. Therefore:

> A Linux app in the microvm is a **tiled window** in the dwindle
> layout, composited by Shade next to native windows — never
> fullscreen, never GPU-monopolising.

This invariant kills two options outright:
- **GPU passthrough** (single-GPU ⇒ VM owns the whole display ⇒
  fullscreen) — contradicts tiling. Out.
- **cross-domain/virgl now** — would force nopeekOS to grow a host
  GL stack (unbounded, against `feedback_linux_strict`). It is a
  *performance transport*, layered on later; it does not change the
  invariant. Deferred.

What the invariant *requires*: guest renders → a buffer → **Shade
composites that buffer as a window**. Rendering inside the guest is
software-Wayland first (no Mesa/GL, no host 3D). Acceleration is a
separate, later axis that never changes "Shade composites it."

## The central problem: concurrency

Grounded in code (2026-05-15):

- `intent::microvm_linux()` (`kernel/src/intent/mod.rs:1592`) is a
  **blocking, synchronous** call. `microvm::run_linux` →
  `run_linux_loop` is a tight VMRESUME loop that occupies its core
  for the guest's entire lifetime and only returns on guest
  exit (HLT/poweroff/panic).
- Shade + compositor + framebuffer are **Core-0-only, not
  thread-safe** (`intent/mod.rs:578` comment; render path
  `shade/mod.rs:149`).
- Today the VM runs on Core 0 → **Shade is frozen for the guest's
  whole life**. A terminal can't render while the VM runs. That is
  fundamentally incompatible with "browser is one tile among many."

**This is the architecture-defining problem.** Pixel formats,
damage rects, etc. are details; this is the structural blocker.

## Architecture: three decoupled domains

```
┌─ Core 0 ───────────────┐   ┌─ Dedicated VM core ────────┐
│ Shade compositor       │   │ microvm run-loop (VMRESUME) │
│ - dwindle retile       │   │ - virtio-blk/net/gpu/input  │
│ - render_window() loop │   │ - guest Linux + Wayland     │
│ - input routing        │   │   (software render)         │
│ READS surface buffers  │   │ WRITES surface buffer on    │
│ WRITES input queues    │   │   virtio-gpu FLUSH          │
└──────────┬─────────────┘   │ READS input queue           │
           │                 └──────────┬──────────────────┘
           │   Mutex-guarded handoff    │
           ▼   (no direct calls)        ▼
   ┌─────────────────────────────────────────────┐
   │ per-window GuestSurface registry             │
   │  SURFACES: Mutex<BTreeMap<WindowId, …>>      │
   │   double-buffer (A/B) + AtomicPtr front      │
   │  + per-VM virtio-input event queue           │
   └─────────────────────────────────────────────┘
```

**Rule: the VM core NEVER calls into Shade.** It only writes its
surface double-buffer and reads its input queue, both Mutex-guarded.
Shade on Core 0 only reads the surface during composite and writes
the input queue. This mirrors the existing framebuffer A/B +
`AtomicPtr` lock-free pattern (`drivers/framebuffer.rs:46`) and the
widget `Mutex<BTreeMap>` scene/event pattern
(`shade/widgets/mod.rs:134,161`) — proven primitives, nothing new.

## Component decisions

### D1' — Launch model: a windowed app, not a terminal intent

Today `microvm linux` is an intent typed into a loop (loop = the
shell); guest output spews as `[guest]` text into *that* terminal.
That is the dev/debug path only. **Productised: launching the
browser creates its own `WindowKind::Surface` window and binds the
`VmContext` to that window**, exactly like drun/loft are their own
windows via the app-spawn machinery (`npk_spawn_module`, launcher
binding `sys/config/launcher`). The launching loop stays free. The
launcher (drun entry / a `browser` verb / config) is **orthogonal**
to the execution model — per invariant #2 the consumer side never
depends on how or from where it was launched. The old terminal-intent
path may remain as a debug affordance.

### D1 — VM lifecycle: cooperative slice on Core 0 (see R1)

The microvm stops being a blocking foreground intent. Launching the
browser **spawns a long-lived VM task bound to a Shade window**, on a
**dedicated core** (not Core 0; not a general work-stealing worker —
the VMRESUME loop is non-yielding and would starve the scheduler).
VMX/SVM root mode is **per-logical-CPU**: VMXON/VMCS setup must run
**on that core**. This is the biggest unknown — see Risk R1.

Lifecycle, both directions:
- Window close → signal VM to power off (graceful), then reclaim core.
- Guest poweroff/panic → substrate catches it (trust boundary already
  holds — validated v0.128/v0.165), window enters an "exited" state
  (dimmed + "process ended"), not a kernel issue.

### D2 — Window model: a new `WindowKind::Surface`

`WindowKind` today is `Terminal | Widget`. Add **`Surface`** — a
raw-bitmap window (no widget tree, no terminal). Conceptually this is
the generalised "Canvas escape hatch" (`docs/archive/PHASE10_WIDGETS.md`) and will
also serve future non-VM raw-pixel apps. Kept distinct from `Widget`
to avoid conflating a declarative tree with an opaque framebuffer.

Storage parallels `SCENES`: a new
`SURFACES: Mutex<BTreeMap<WindowId, GuestSurface>>`, reusing the
exact proven Mutex/double-buffer machinery. `create`/`close`/`focus`
/`retile` already dispatch on `WindowKind` — add the `Surface` arm.

```rust
struct GuestSurface {
    buf_a: Vec<u8>, buf_b: Vec<u8>,   // BGRX, window-content sized
    front: AtomicUsize,               // 0=a 1=b, VM writes back, swaps
    dirty: AtomicBool,                // VM sets, Shade clears on blit
    w: u32, h: u32,                   // current negotiated size
    vm: VmHandle,                     // for input routing + lifecycle
}
```

### D3 — Pixel handoff

virtio-gpu `RESOURCE_FLUSH` (currently a no-op log,
`virtio_gpu_pci.rs`) resolves its scanout → the bound `GuestSurface`
→ memcpy `host_pixels` into the **back** buffer, swap `front`, set
`dirty`. Shade's `render_window()` `Surface` arm memcpys the front
buffer into the shadow at the tile rect (same shape as the Widget
pixel-blit at `compositor.rs:602`). One copy each side; the two loops
never block on each other beyond the short Mutex.

### D4 — Geometry: window size drives the guest, no scaling

The tile rect is dwindle-decided and changes on retile/resize. The
**correct** answer (and how real virtio-gpu + a compositor behaves):
virtio-gpu `GET_DISPLAY_INFO` advertises the **window content rect**,
not a hardcoded 1280×720. On Shade resize: update display info →
raise a virtio-gpu **config-change interrupt** → guest re-queries →
guest Wayland compositor reconfigures to the new size. No host-side
scaling, ever. The guest becomes a well-behaved citizen reacting to
"display resize" = window resize. (First slice may pin one size to
defer the resize round-trip — see phasing.)

### D5 — Input seam (impl later, seam defined now)

Shade already hit-tests/focuses windows and routes key/pointer.
For a focused `Surface` window: events go to
`surface.vm` → that VM's **virtio-input eventq** (the empty
stub from 12.4c). Defining the seam now (`WindowId → VmHandle →
virtio-input queue`) keeps D1/D2 honest; the eventq fill is a
separate task gated on this design.

## R1 — RESOLVED by research (2026-05-15): cooperative time-slice, NOT a dedicated core

The original spike proposed a dedicated non-Core-0 core. Deep code
research **disproved that as the right first step**: `mm/paging.rs`
`map_page` mutates a shared PML4 with no lock and `flush_tlb_all()`
only reloads the calling core's CR3 → running the VM on core N
concurrently with Core 0 is a TLB-incoherence / data-race **hard
blocker**. The clean multi-core fix (global map_page lock + IPI TLB
shootdown + per-core TSS + scheduler core-reservation) is real but
large new SMP-correctness surface.

**The clean fix is cooperative time-slicing on Core 0**, which the
substrate already enables: `vmcs::run_guest_once` re-establishes
`HOST_RSP`/`HOST_RIP` to its *own* stack frame just-in-time per call
(`enable.rs:195`, `vmcs.rs:830`). So the run loop can return to its
caller between any two `run_guest_once` calls and re-enter from a
different frame transparently. The **only** invariant: VMX-root
(VMXON) + the loaded VMCS + heap state must persist across slices —
i.e. VMXOFF must not happen between slices. Normal kernel/Shade code
running in VMX-root between slices is exactly what the hypervisor
already does between VMRESUMEs, just longer.

This eliminates ALL four SMP blockers (per-core TSS, paging race, TLB
shootdown, scheduler reservation). The dedicated-core + SMP-correct
paging is now a **later pure-performance optimisation**, not a
correctness prerequisite.

Design: a persistent `VmContext` (heap, single global `Option`) owns
the VMXON region, VMCS, EPT, `host_base`, `PciBus`, `SerialState`,
`GuestRegs`, `launched`, exit history. `vmx_open()` does VMXON + VMCS
setup once; `run_slice(ctx, budget) -> {StillRunning | Exited}` runs
the existing loop body bounded to N iters / a TSC deadline;
`vmx_close()` does VMXOFF + free at teardown. The Core-0 event loop
interleaves: render Shade → `run_slice` → render Shade → … The
`is_core0_intent` gate stays (VM still on Core 0) — it just no longer
*blocks* Core 0.

Remaining risks (smaller):
- **R2 — resize round-trip latency.** Dragging a tile split would
  spam config-change IRQs → guest mode reconfigures. Needs debounce;
  acceptable to pin size in the first slice.
- **R3 — multiple VMs later.** Registry is keyed by `WindowId`; each
  `Surface`→its own VM/core. Design doesn't preclude N, but core
  budget is finite — out of scope until one works.

## Phasing — minimal architecture-proving slice

Goal: prove R1 + the handoff + tiling integration with **no Mesa, no
big bundle**.

1. **R1 (resolved approach)**: split the VMXON bracket into
   `vmx_open`/`vmx_close`, hoist run-loop locals into a persistent
   heap `VmContext`, add `run_slice(budget)`. Step 1a:
   behaviour-preserving (open → slice-loop-to-exit → close, identical
   to today). Step 1b: bound the slice + interleave with Shade render
   on Core 0 so a terminal keeps redrawing while the guest runs.
   Mirror vmx → svm. This is the gate.
2. `WindowKind::Surface` + `SURFACES` registry + double-buffer (D2).
3. virtio-gpu `RESOURCE_FLUSH` → back buffer + dirty (D3); Shade
   `render_window()` Surface arm composites the tile.
4. Tiny **software-Wayland test client** (~10 MB gzip-sqfs, raw
   lane, fast OTA): a compositor + `weston-simple-shm` drawing a
   moving rectangle → appears as a **resizable tile next to a
   terminal**. That single result proves concurrency + handoff +
   tiling — the architecture-defining risks — end to end.
5. Then: input injection (D5), resize round-trip (D4), and only
   afterwards the real Mesa/cage/LibreWolf bundle.

## Forward-compatibility contract (binding — guards against a future rewrite)

The cooperative-slice choice is a **strict prefix** of the eventual
multi-core design, not a throwaway. To keep it that way, these
invariants are binding for ALL code written from here on. Violating
one is the only thing that turns "later optimisation" into a rewrite:

1. **`VmContext` owns all VM state**; the hypervisor hot path
   (`run_guest_once`, exit handlers) stays core-agnostic — it already
   self-establishes HOST_RSP/RIP per call. The time-slice→dedicated
   -core migration must be: call the *same* `run_slice` from a
   different scheduler context + add the *additive* SMP-paging fixes
   (per-core TSS, global `map_page` lock, IPI TLB shootdown). Nothing
   in `VmContext`/`run_slice`/exit-handlers may change for that move.
2. **The consumer side never knows which core the VM runs on.** Shade,
   the Window/`GuestSurface` model, input routing communicate with
   the VM ONLY through the handle-keyed, Mutex-guarded surface
   double-buffer + input queues. No code may assume "one VM", "VM is
   inline on Core 0", or call into the VM synchronously. Registry is
   `WindowId → VmContext handle`, N-VM-shaped from day one even though
   only one exists now.
3. **The `is_core0_intent` gate is a *scheduling* fact, not an API.**
   Nothing outside the substrate may branch on "VM is on Core 0".

If these hold, multi-core is additive + localized. If any is broken,
single-core assumptions leak into the consumer side and the cost
becomes unbounded. This section is the explicit answer to "are we
saving LoC now and paying with a massive rewrite later" — no, *iff*
these three hold.

## Explicitly de-scoped here

cross-domain/virgl, Mesa, GPU passthrough, the 500 MB bundle,
multi-VM. None are on the path to proving the architecture; all are
downstream of a working single software-rendered Surface tile.
