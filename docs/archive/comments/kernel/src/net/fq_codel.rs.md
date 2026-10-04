# `kernel/src/net/fq_codel.rs` @ 5e0102684

## L1-18 · `use crate::interrupts::{rdtsc, tsc_freq};`

```
//! fq_codel TX queue — a faithful port of Linux's "Make WiFi Fast" design
//! (net/sched fq_codel = fq_impl.h flow scheduler + codel_impl.h AQM), used here
//! for the software TX queue in front of a slow NIC (the WASM WiFi driver).
//!
//! Two cooperating parts, exactly as Linux:
//!  - **fq** — packets are hashed into per-flow sub-queues, scheduled by deficit
//!    round-robin with a "new flows first" rule. A sparse flow (a ping, a DNS
//!    lookup) is serviced ahead of a bulk flow, so it never waits behind the
//!    bulk backlog.
//!  - **CoDel** — per flow, the sojourn time of the head packet is tracked; once
//!    it stays above TARGET for longer than INTERVAL a controlled drop schedule
//!    starts (drop rate ∝ √count via the same Newton reciprocal-sqrt control
//!    law as Linux). Latency is held near TARGET instead of building bufferbloat.
//!
//! Time is measured in TSC ticks (the kernel's only fine clock); TARGET/INTERVAL
//! are derived from tsc_freq() on first use. Combined with the driver's AQL-style
//! in-flight cap (which keeps the hardware queue shallow), this is the full Linux
//! latency-under-load architecture.
```

## L24 · `const FLOWS: usize = 16; // per-flow sub-queues (power of two)`

```
// per-flow sub-queues (power of two)
```

## L26-43 · `const CAP: usize = 256;`

```
// Total packet slots shared across all flows.
//
// 64 was far below anything CoDel can work with — Linux's fq_codel defaults to
// 10240 packets. A tail-drop queue that shallow defeats the AQM it sits under:
// CoDel decides by how long a packet SAT in the queue, and it needs room to
// observe that. Below it, the queue just overflows.
//
// It showed the moment this system sent bulk data for the first time.
// `tcp::send` bursts a whole 64 KiB chunk — 45 segments — in one call, and
// MAX_UNACKED lets ~180 segments go out before any ACK. Against 64 slots the
// overflow is arithmetic, not bad luck: measured `drops full 76` on a link with
// 4 % air retries and every block-ack acknowledged. The air was fine; the queue
// was three times too small for what TCP is allowed to have in flight.
//
// 256 slots = 388 KB, and it holds a full MAX_UNACKED window (181 segments)
// with room for CoDel to do its job. It costs nothing in the kernel image: the
// struct is all-zero-initialised and lives in .bss — see `new()` below, which
// is deliberately NOT allowed to write a sentinel.
```

## L46 · `const QUANTUM: i32 = MTU as i32; // DRR quantum (bytes)`

```
// DRR quantum (bytes)
```

## L48-49 · `const REC_INV_SQRT_SHIFT: u32 = 16;`

```
// CoDel reciprocal-sqrt fixed point (codel_impl.h): rec_inv_sqrt is u16,
// shifted by (32 - 16) = 16 when used as a 0.32 fraction.
```

## L51 · `const REC_INV_SQRT_MAX: u16 = u16::MAX; // 1.0 in the 0.16 fixed point`

```
// 1.0 in the 0.16 fixed point
```

## L53 · `fn newton_step(count: u32, rec_inv_sqrt: u16) -> u16 {`

```
// codel_Newton_step: one Newton-Raphson iteration toward 1/sqrt(count).
```

## L63 · `fn control_law(t: u64, interval: u64, rec_inv_sqrt: u16) -> u64 {`

```
// codel_control_law: next drop time = t + interval / sqrt(count).
```

## L65 · `let ep_ro = (rec_inv_sqrt as u64) << REC_INV_SQRT_SHIFT; // 0.32 fraction`

```
// 0.32 fraction
```

## L74 · `first_above_time: u64, // 0 = sojourn not yet above target`

```
// 0 = sojourn not yet above target
```

## L92 · `head: u16, // pool index of oldest packet, EMPTY if none`

```
// pool index of oldest packet, EMPTY if none
```

## L95 · `in_list: u8, // 0 = none, 1 = new, 2 = old`

```
// 0 = none, 1 = new, 2 = old
```

## L100-101 · `const fn zeroed() -> Self {`

```
/// All-zero, for the const initialiser ONLY. `head`/`tail` are not valid
/// yet — `lazy_init` sets them to EMPTY before anything reads them.
```

## L110 · `struct FlowQ {`

```
// A tiny fixed-capacity FIFO of flow indices (the new_flows / old_flows lists).
```

## L144 · `ts: [u64; CAP],   // enqueue TSC of each packet`

```
// enqueue TSC of each packet
```

## L145 · `link: [u16; CAP], // next packet in the owning flow's FIFO, or next free slot`

```
// next packet in the owning flow's FIFO, or next free slot
```

## L146 · `free: u16,        // head of the free-slot list`

```
// head of the free-slot list
```

## L150 · `backlog: usize, // total queued bytes (for CoDel's "don't drop if tiny" rule)`

```
// total queued bytes (for CoDel's "don't drop if tiny" rule)
```

## L151 · `target: u64,    // ticks (5 ms)`

```
// ticks (5 ms)
```

## L152 · `interval: u64,  // ticks (100 ms)`

```
// ticks (100 ms)
```

## L153-155 · `drops_aqm: u64,`

```
// Drop bookkeeping, split by cause: AQM drops mean CoDel is doing its job
// (queue standing too long), pool-full drops mean the driver is not keeping
// up at all. They call for opposite fixes, so never sum them into one number.
```

## L162-166 · `pub const fn new() -> Self {`

```
/// EVERY field here must be zero. A single non-zero byte — `EMPTY` is
/// 0xffff — drags the whole 388 KB struct out of .bss and into the kernel
/// image as literal bytes. It did: `WASM_NIC` sat in .data at 98 KB because
/// `link` and `Flow::head/tail` were initialised to EMPTY, and both are
/// rebuilt by `lazy_init` before anything reads them anyway.
```

## L187 · `pub fn drops_split(&self) -> (u64, u64) { (self.drops_aqm, self.drops_full) }`

```
/// Frames discarded so far: (CoDel/AQM, pool-full).
```

## L190 · `pub fn backlog(&self) -> usize { self.backlog }`

```
/// Bytes currently queued for the driver.
```

## L201 · `self.target = hz / 200; // 5 ms`

```
// 5 ms
```

## L202 · `self.interval = hz / 10; // 100 ms`

```
// 100 ms
```

## L203 · `for i in 0..CAP {`

```
// Build the free-slot list 0 -> 1 -> ... -> CAP-1 -> EMPTY.
```

## L208-209 · `for f in self.flows.iter_mut() {`

```
// …and give the flows their sentinel. The const initialiser could
// not: a non-zero byte there costs 388 KB of kernel image.
```

## L216 · `pub fn clear(&mut self) {`

```
/// Reset to empty (driver re-register).
```

## L245 · `fn flow_pop(&mut self, fi: usize) -> Option<u16> {`

```
// Pop the head packet slot of a flow's FIFO (no CoDel), or None.
```

## L259-264 · `pub fn enqueue(&mut self, frame: &[u8]) -> bool {`

```
/// Enqueue one Ethernet frame. Drops (tail / fattest-flow) if the pool is
/// full — never blocks.
/// Returns false when the frame was NOT taken. It used to return nothing,
/// and the caller counted every call as enqueued — so a frame refused here
/// was indistinguishable from one that went out. That is how 186 of 237
/// TX frames vanished with `drops 0` and `backlog 0`.
```

## L267-270 · `if !frame.is_empty() { self.drops_oversize += 1; }`

```
// An over-MTU frame is a BUG upstream, not congestion: nothing here
// can make it fit, and dropping it silently makes the sender look
// like a dead peer. Counted apart from congestion drops so the two
// can never be confused again.
```

## L280-281 · `if !self.drop_fattest() {`

```
// Pool full: drop the head of the fattest flow to make room, so a
// new sparse flow's packet still gets in (fq_codel_drop).
```

## L300 · `if self.flows[fi].tail == EMPTY {`

```
// Append to the flow FIFO.
```

## L309-310 · `if self.flows[fi].in_list == 0 {`

```
// A newly-active flow joins new_flows with a fresh quantum (sparse-flow
// priority) — fq_impl.h.
```

## L320 · `fn drop_fattest(&mut self) -> bool {`

```
// Drop the head packet of whichever flow has the most queued bytes.
```

## L348 · `fn should_drop(&mut self, fi: usize, slot: u16, now: u64) -> bool {`

```
// codel_should_drop: is the head packet's sojourn over target long enough?
```

## L364-365 · `fn codel_dequeue(&mut self, fi: usize, now: u64) -> Option<u16> {`

```
// CoDel dequeue for one flow: returns a slot to deliver, dropping stale head
// packets per the control law. None means the flow drained.
```

## L386 · `self.free_slot(slot); // drop`

```
// drop
```

## L407 · `self.free_slot(slot);`

```
// First drop: discard this one, take the next, enter dropping state.
```

## L420-421 · `let c = &mut self.flows[fi].codel;`

```
// Seed count: if we dropped again recently, ramp from the prior
// count, else restart (codel_dequeue).
```

## L438-439 · `pub fn dequeue(&mut self, out: &mut [u8; MTU]) -> Option<usize> {`

```
/// Dequeue the next frame to transmit into `out`; None if empty. Implements
/// fq_codel_dequeue: new flows first, deficit round-robin, CoDel per flow.
```

## L444 · `let (from_new, fi) = match self.new_q.front() {`

```
// Pick the head flow: new flows take priority over old.
```

## L455 · `if from_new {`

```
// Move to the tail of old_flows for its next turn.
```

## L475-477 · `if from_new {`

```
// Flow drained. A new flow that empties is given one more
// chance on old_flows (prevents starvation); an old flow that
// empties leaves the rotation.
```

## L497-498 · `fn flow_hash(frame: &[u8]) -> u32 {`

```
// Hash a frame to a flow: IPv4 5-tuple (src/dst/proto/ports) so each TCP/UDP
// flow and ICMP stream lands in its own sub-queue; else the dst MAC. FNV-1a.
```

## L508 · `for &b in &frame[26..34] {`

```
// src + dst IPv4 (bytes 26..34), protocol.
```

## L513 · `let l4 = 14 + ihl;`

```
// L4 ports for TCP(6)/UDP(17), if present.
```

