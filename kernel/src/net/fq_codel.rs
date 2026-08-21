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

use crate::interrupts::{rdtsc, tsc_freq};

pub const MTU: usize = 1514;

const FLOWS: usize = 16; // per-flow sub-queues (power of two)
const FLOW_MASK: u32 = FLOWS as u32 - 1;
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
const CAP: usize = 256;
const EMPTY: u16 = u16::MAX;
const QUANTUM: i32 = MTU as i32; // DRR quantum (bytes)

// CoDel reciprocal-sqrt fixed point (codel_impl.h): rec_inv_sqrt is u16,
// shifted by (32 - 16) = 16 when used as a 0.32 fraction.
const REC_INV_SQRT_SHIFT: u32 = 16;
const REC_INV_SQRT_MAX: u16 = u16::MAX; // 1.0 in the 0.16 fixed point

// codel_Newton_step: one Newton-Raphson iteration toward 1/sqrt(count).
fn newton_step(count: u32, rec_inv_sqrt: u16) -> u16 {
    let invsqrt = (rec_inv_sqrt as u32) << REC_INV_SQRT_SHIFT;
    let invsqrt2 = (((invsqrt as u64) * invsqrt as u64) >> 32) as u32;
    let val = (3u64 << 32).wrapping_sub((count as u64) * invsqrt2 as u64);
    let val = val >> 2;
    let val = (val * invsqrt as u64) >> (32 - 2 + 1);
    (val >> REC_INV_SQRT_SHIFT) as u16
}

// codel_control_law: next drop time = t + interval / sqrt(count).
fn control_law(t: u64, interval: u64, rec_inv_sqrt: u16) -> u64 {
    let ep_ro = (rec_inv_sqrt as u64) << REC_INV_SQRT_SHIFT; // 0.32 fraction
    t + ((interval * ep_ro) >> 32)
}

struct Codel {
    count: u32,
    lastcount: u32,
    dropping: bool,
    rec_inv_sqrt: u16,
    first_above_time: u64, // 0 = sojourn not yet above target
    drop_next: u64,
}

impl Codel {
    const fn new() -> Self {
        Codel {
            count: 0,
            lastcount: 0,
            dropping: false,
            rec_inv_sqrt: 0,
            first_above_time: 0,
            drop_next: 0,
        }
    }
}

struct Flow {
    head: u16, // pool index of oldest packet, EMPTY if none
    tail: u16,
    deficit: i32,
    in_list: u8, // 0 = none, 1 = new, 2 = old
    codel: Codel,
}

impl Flow {
    /// All-zero, for the const initialiser ONLY. `head`/`tail` are not valid
    /// yet — `lazy_init` sets them to EMPTY before anything reads them.
    const fn zeroed() -> Self {
        Flow { head: 0, tail: 0, deficit: 0, in_list: 0, codel: Codel::new() }
    }
    const fn new() -> Self {
        Flow { head: EMPTY, tail: EMPTY, deficit: 0, in_list: 0, codel: Codel::new() }
    }
}

// A tiny fixed-capacity FIFO of flow indices (the new_flows / old_flows lists).
struct FlowQ {
    buf: [u8; FLOWS + 1],
    head: usize,
    tail: usize,
}

impl FlowQ {
    const fn new() -> Self {
        FlowQ { buf: [0; FLOWS + 1], head: 0, tail: 0 }
    }
    fn is_empty(&self) -> bool {
        self.head == self.tail
    }
    fn front(&self) -> Option<usize> {
        if self.is_empty() { None } else { Some(self.buf[self.head] as usize) }
    }
    fn push(&mut self, f: usize) {
        self.buf[self.tail] = f as u8;
        self.tail = (self.tail + 1) % (FLOWS + 1);
    }
    fn pop(&mut self) -> Option<usize> {
        if self.is_empty() {
            return None;
        }
        let f = self.buf[self.head] as usize;
        self.head = (self.head + 1) % (FLOWS + 1);
        Some(f)
    }
}

pub struct FqCodel {
    bufs: [[u8; MTU]; CAP],
    lens: [u16; CAP],
    ts: [u64; CAP],   // enqueue TSC of each packet
    link: [u16; CAP], // next packet in the owning flow's FIFO, or next free slot
    free: u16,        // head of the free-slot list
    flows: [Flow; FLOWS],
    new_q: FlowQ,
    old_q: FlowQ,
    backlog: usize, // total queued bytes (for CoDel's "don't drop if tiny" rule)
    target: u64,    // ticks (5 ms)
    interval: u64,  // ticks (100 ms)
    // Drop bookkeeping, split by cause: AQM drops mean CoDel is doing its job
    // (queue standing too long), pool-full drops mean the driver is not keeping
    // up at all. They call for opposite fixes, so never sum them into one number.
    drops_aqm: u64,
    drops_full: u64,
    drops_oversize: u64,
}

impl FqCodel {
    /// EVERY field here must be zero. A single non-zero byte — `EMPTY` is
    /// 0xffff — drags the whole 388 KB struct out of .bss and into the kernel
    /// image as literal bytes. It did: `WASM_NIC` sat in .data at 98 KB because
    /// `link` and `Flow::head/tail` were initialised to EMPTY, and both are
    /// rebuilt by `lazy_init` before anything reads them anyway.
    pub const fn new() -> Self {
        const F: Flow = Flow::zeroed();
        FqCodel {
            bufs: [[0; MTU]; CAP],
            lens: [0; CAP],
            ts: [0; CAP],
            link: [0; CAP],
            free: 0,
            flows: [F; FLOWS],
            new_q: FlowQ::new(),
            old_q: FlowQ::new(),
            backlog: 0,
            target: 0,
            interval: 0,
            drops_aqm: 0,
            drops_full: 0,
            drops_oversize: 0,
        }
    }

    /// Frames discarded so far: (CoDel/AQM, pool-full).
    pub fn drops_split(&self) -> (u64, u64) { (self.drops_aqm, self.drops_full) }
    pub fn drops_oversize(&self) -> u64 { self.drops_oversize }
    /// Bytes currently queued for the driver.
    pub fn backlog(&self) -> usize { self.backlog }
    pub fn reset_drops(&mut self) {
        self.drops_aqm = 0;
        self.drops_full = 0;
        self.drops_oversize = 0;
    }

    fn lazy_init(&mut self) {
        if self.interval == 0 {
            let hz = tsc_freq().max(1);
            self.target = hz / 200; // 5 ms
            self.interval = hz / 10; // 100 ms
            // Build the free-slot list 0 -> 1 -> ... -> CAP-1 -> EMPTY.
            for i in 0..CAP {
                self.link[i] = if i + 1 < CAP { (i + 1) as u16 } else { EMPTY };
            }
            self.free = 0;
            // …and give the flows their sentinel. The const initialiser could
            // not: a non-zero byte there costs 388 KB of kernel image.
            for f in self.flows.iter_mut() {
                *f = Flow::new();
            }
        }
    }

    /// Reset to empty (driver re-register).
    pub fn clear(&mut self) {
        for i in 0..CAP {
            self.link[i] = if i + 1 < CAP { (i + 1) as u16 } else { EMPTY };
        }
        self.free = 0;
        for f in self.flows.iter_mut() {
            *f = Flow::new();
        }
        self.new_q = FlowQ::new();
        self.old_q = FlowQ::new();
        self.backlog = 0;
    }

    fn alloc_slot(&mut self) -> Option<u16> {
        if self.free == EMPTY {
            return None;
        }
        let s = self.free;
        self.free = self.link[s as usize];
        self.link[s as usize] = EMPTY;
        Some(s)
    }

    fn free_slot(&mut self, s: u16) {
        self.link[s as usize] = self.free;
        self.free = s;
    }

    // Pop the head packet slot of a flow's FIFO (no CoDel), or None.
    fn flow_pop(&mut self, fi: usize) -> Option<u16> {
        let s = self.flows[fi].head;
        if s == EMPTY {
            return None;
        }
        let next = self.link[s as usize];
        self.flows[fi].head = next;
        if next == EMPTY {
            self.flows[fi].tail = EMPTY;
        }
        Some(s)
    }

    /// Enqueue one Ethernet frame. Drops (tail / fattest-flow) if the pool is
    /// full — never blocks.
    /// Returns false when the frame was NOT taken. It used to return nothing,
    /// and the caller counted every call as enqueued — so a frame refused here
    /// was indistinguishable from one that went out. That is how 186 of 237
    /// TX frames vanished with `drops 0` and `backlog 0`.
    pub fn enqueue(&mut self, frame: &[u8]) -> bool {
        if frame.is_empty() || frame.len() > MTU {
            // An over-MTU frame is a BUG upstream, not congestion: nothing here
            // can make it fit, and dropping it silently makes the sender look
            // like a dead peer. Counted apart from congestion drops so the two
            // can never be confused again.
            if !frame.is_empty() { self.drops_oversize += 1; }
            return false;
        }
        self.lazy_init();
        let now = rdtsc();

        let slot = match self.alloc_slot() {
            Some(s) => s,
            None => {
                // Pool full: drop the head of the fattest flow to make room, so a
                // new sparse flow's packet still gets in (fq_codel_drop).
                if !self.drop_fattest() {
                    self.drops_full += 1;
                    return false;
                }
                match self.alloc_slot() {
                    Some(s) => s,
                    None => { self.drops_full += 1; return false; }
                }
            }
        };

        let len = frame.len();
        self.bufs[slot as usize][..len].copy_from_slice(frame);
        self.lens[slot as usize] = len as u16;
        self.ts[slot as usize] = now;
        self.link[slot as usize] = EMPTY;

        let fi = (flow_hash(frame) & FLOW_MASK) as usize;
        // Append to the flow FIFO.
        if self.flows[fi].tail == EMPTY {
            self.flows[fi].head = slot;
        } else {
            self.link[self.flows[fi].tail as usize] = slot;
        }
        self.flows[fi].tail = slot;
        self.backlog += len;

        // A newly-active flow joins new_flows with a fresh quantum (sparse-flow
        // priority) — fq_impl.h.
        if self.flows[fi].in_list == 0 {
            self.flows[fi].deficit = QUANTUM;
            self.flows[fi].in_list = 1;
            self.flows[fi].codel.dropping = false;
            self.new_q.push(fi);
        }
        true
    }

    // Drop the head packet of whichever flow has the most queued bytes.
    fn drop_fattest(&mut self) -> bool {
        let mut best = usize::MAX;
        let mut best_bytes = 0usize;
        for fi in 0..FLOWS {
            let mut s = self.flows[fi].head;
            let mut bytes = 0usize;
            while s != EMPTY {
                bytes += self.lens[s as usize] as usize;
                s = self.link[s as usize];
            }
            if bytes > best_bytes {
                best_bytes = bytes;
                best = fi;
            }
        }
        if best == usize::MAX {
            return false;
        }
        if let Some(s) = self.flow_pop(best) {
            self.backlog -= self.lens[s as usize] as usize;
            self.free_slot(s);
            self.drops_full += 1;
            return true;
        }
        false
    }

    // codel_should_drop: is the head packet's sojourn over target long enough?
    fn should_drop(&mut self, fi: usize, slot: u16, now: u64) -> bool {
        let sojourn = now.wrapping_sub(self.ts[slot as usize]);
        let c = &mut self.flows[fi].codel;
        if sojourn < self.target || self.backlog <= MTU {
            c.first_above_time = 0;
            return false;
        }
        if c.first_above_time == 0 {
            c.first_above_time = now + self.interval;
            false
        } else {
            now >= c.first_above_time
        }
    }

    // CoDel dequeue for one flow: returns a slot to deliver, dropping stale head
    // packets per the control law. None means the flow drained.
    fn codel_dequeue(&mut self, fi: usize, now: u64) -> Option<u16> {
        let mut slot = match self.flow_pop(fi) {
            Some(s) => s,
            None => {
                self.flows[fi].codel.dropping = false;
                return None;
            }
        };
        self.backlog -= self.lens[slot as usize] as usize;
        let mut drop = self.should_drop(fi, slot, now);

        if self.flows[fi].codel.dropping {
            if !drop {
                self.flows[fi].codel.dropping = false;
            } else {
                while self.flows[fi].codel.dropping && now >= self.flows[fi].codel.drop_next {
                    self.flows[fi].codel.count += 1;
                    let c = self.flows[fi].codel.count;
                    self.flows[fi].codel.rec_inv_sqrt =
                        newton_step(c, self.flows[fi].codel.rec_inv_sqrt);
                    self.free_slot(slot); // drop
                    self.drops_aqm += 1;
                    slot = match self.flow_pop(fi) {
                        Some(s) => s,
                        None => {
                            self.flows[fi].codel.dropping = false;
                            return None;
                        }
                    };
                    self.backlog -= self.lens[slot as usize] as usize;
                    drop = self.should_drop(fi, slot, now);
                    if !drop {
                        self.flows[fi].codel.dropping = false;
                    } else {
                        let dn = self.flows[fi].codel.drop_next;
                        let ris = self.flows[fi].codel.rec_inv_sqrt;
                        self.flows[fi].codel.drop_next = control_law(dn, self.interval, ris);
                    }
                }
            }
        } else if drop {
            // First drop: discard this one, take the next, enter dropping state.
            self.free_slot(slot);
            self.drops_aqm += 1;
            slot = match self.flow_pop(fi) {
                Some(s) => s,
                None => {
                    self.flows[fi].codel.dropping = false;
                    return None;
                }
            };
            self.backlog -= self.lens[slot as usize] as usize;
            let _ = self.should_drop(fi, slot, now);
            self.flows[fi].codel.dropping = true;
            // Seed count: if we dropped again recently, ramp from the prior
            // count, else restart (codel_dequeue).
            let c = &mut self.flows[fi].codel;
            let delta = c.count.wrapping_sub(c.lastcount);
            if delta > 1 && now.wrapping_sub(c.drop_next) < 16 * self.interval {
                c.count = delta;
                c.rec_inv_sqrt = newton_step(c.count, c.rec_inv_sqrt);
            } else {
                c.count = 1;
                c.rec_inv_sqrt = REC_INV_SQRT_MAX;
            }
            c.lastcount = c.count;
            let ris = c.rec_inv_sqrt;
            c.drop_next = control_law(now, self.interval, ris);
        }
        Some(slot)
    }

    /// Dequeue the next frame to transmit into `out`; None if empty. Implements
    /// fq_codel_dequeue: new flows first, deficit round-robin, CoDel per flow.
    pub fn dequeue(&mut self, out: &mut [u8; MTU]) -> Option<usize> {
        self.lazy_init();
        let now = rdtsc();
        loop {
            // Pick the head flow: new flows take priority over old.
            let (from_new, fi) = match self.new_q.front() {
                Some(f) => (true, f),
                None => match self.old_q.front() {
                    Some(f) => (false, f),
                    None => return None,
                },
            };

            if self.flows[fi].deficit <= 0 {
                self.flows[fi].deficit += QUANTUM;
                // Move to the tail of old_flows for its next turn.
                if from_new {
                    self.new_q.pop();
                } else {
                    self.old_q.pop();
                }
                self.flows[fi].in_list = 2;
                self.old_q.push(fi);
                continue;
            }

            match self.codel_dequeue(fi, now) {
                Some(slot) => {
                    let len = self.lens[slot as usize] as usize;
                    self.flows[fi].deficit -= len as i32;
                    out[..len].copy_from_slice(&self.bufs[slot as usize][..len]);
                    self.free_slot(slot);
                    return Some(len);
                }
                None => {
                    // Flow drained. A new flow that empties is given one more
                    // chance on old_flows (prevents starvation); an old flow that
                    // empties leaves the rotation.
                    if from_new {
                        self.new_q.pop();
                        if !self.old_q.is_empty() {
                            self.flows[fi].in_list = 2;
                            self.old_q.push(fi);
                        } else {
                            self.flows[fi].in_list = 0;
                        }
                    } else {
                        self.old_q.pop();
                        self.flows[fi].in_list = 0;
                    }
                    continue;
                }
            }
        }
    }
}

// Hash a frame to a flow: IPv4 5-tuple (src/dst/proto/ports) so each TCP/UDP
// flow and ICMP stream lands in its own sub-queue; else the dst MAC. FNV-1a.
fn flow_hash(frame: &[u8]) -> u32 {
    let mut h: u32 = 0x811c9dc5;
    let mix = |b: u8, h: &mut u32| {
        *h ^= b as u32;
        *h = h.wrapping_mul(0x0100_0193);
    };
    if frame.len() >= 34 && frame[12] == 0x08 && frame[13] == 0x00 {
        let ihl = ((frame[14] & 0x0f) as usize) * 4;
        let proto = frame[23];
        // src + dst IPv4 (bytes 26..34), protocol.
        for &b in &frame[26..34] {
            mix(b, &mut h);
        }
        mix(proto, &mut h);
        // L4 ports for TCP(6)/UDP(17), if present.
        let l4 = 14 + ihl;
        if (proto == 6 || proto == 17) && frame.len() >= l4 + 4 {
            for &b in &frame[l4..l4 + 4] {
                mix(b, &mut h);
            }
        }
    } else {
        for &b in &frame[0..6.min(frame.len())] {
            mix(b, &mut h);
        }
    }
    h
}
