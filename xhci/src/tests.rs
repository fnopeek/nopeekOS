//! The port against Linux: `testdata/linux_xhci_irq_*.log` are runs of
//! the unmodified Linux xHCI core (xhci-plat, xhci.c, xhci-ring.c) and its
//! interrupt path against a register model of RP1's DWC3 xHCI, one line
//! per register access, `{ fn` / `} fn` around the Linux functions, and a
//! `# model: event ring ...` line for every event the model writes.
//!
//! `run_finished`, `irq` and what they call must repeat Linux access for
//! access, reads answered with what Linux read. What Linux does with an
//! event inside `xhci_handle_event_trb` is the driver's own business and is
//! left out; how many events each interrupt takes is not.

use super::*;
use std::collections::{BTreeMap, VecDeque};

/// Where the DWC3's xHCI window sits in RP1's BAR1.
const XHCI_IN_BAR1: u64 = 0x20_0000;
const XHCI_LEN: u64 = 0x8000;

#[derive(Clone, Debug, PartialEq)]
enum Ev {
    Read { off: u32, v: u32 },
    Write { off: u32, v: u32 },
    /// The model wrote event TRB `index` of the ring.
    Event { index: usize, trb: [u32; 4] },
    Enter(String),
    Exit(String),
}

fn num(s: &str) -> u64 {
    u64::from_str_radix(s.trim_start_matches("0x"), 16).expect(s)
}

fn parse(log: &str) -> (Vec<Ev>, Vec<(u64, usize)>) {
    let mut out = Vec::new();
    let mut erst = Vec::new();
    for line in log.lines() {
        if let Some(m) = line.strip_prefix("# model: ") {
            let w: Vec<&str> = m.split_whitespace().collect();
            match w.as_slice() {
                ["event", "ring", i, "seg", _, "trb", a, b, c, d, "cycle", _] => out.push(Ev::Event {
                    index: num(i) as usize,
                    trb: [num(a) as u32, num(b) as u32, num(c) as u32, num(d) as u32],
                }),
                ["erst", _, "base", base, "size", size] => erst.push((num(base), num(size) as usize)),
                _ => {}
            }
            continue;
        }
        let line = line.split(" ;").next().unwrap_or("");
        if let Some(n) = line.strip_prefix("{ ") {
            out.push(Ev::Enter(n.trim().to_string()));
            continue;
        }
        if let Some(n) = line.strip_prefix("} ") {
            out.push(Ev::Exit(n.trim().to_string()));
            continue;
        }
        let mut it = line.split_whitespace();
        let (Some(op), Some(at)) = (it.next(), it.next()) else { continue };
        let kind = op.as_bytes()[0];
        if !(kind == b'R' || kind == b'W') || op[1..] != *"32" { continue; }
        let Some(("rp1.bar1", off)) = at.split_once('+') else { continue };
        let off = num(off);
        if !(XHCI_IN_BAR1..XHCI_IN_BAR1 + XHCI_LEN).contains(&off) { continue; }
        let off = (off - XHCI_IN_BAR1) as u32;
        let v = num(it.nth(1).expect(line)) as u32;
        out.push(if kind == b'R' { Ev::Read { off, v } } else { Ev::Write { off, v } });
    }
    (out, erst)
}

/// The register and event lines inside the `n`th top-level call of `name`,
/// without those inside calls of `skip` (their events stay in).
fn segment_nth(evs: &[Ev], name: &str, n: usize, skip: &str) -> Option<Vec<Ev>> {
    let mut seen = 0;
    let mut i = 0;
    while i < evs.len() {
        if evs[i] == Ev::Enter(name.into()) {
            if seen == n { break; }
            seen += 1;
        }
        i += 1;
    }
    if i == evs.len() { return None; }
    let (mut depth, mut skipping) = (0, 0);
    let mut out = Vec::new();
    for e in &evs[i..] {
        match e {
            Ev::Enter(x) if x == name => depth += 1,
            Ev::Exit(x) if x == name => {
                depth -= 1;
                if depth == 0 { break; }
            }
            Ev::Enter(x) if x == skip => skipping += 1,
            Ev::Exit(x) if x == skip => skipping -= 1,
            Ev::Event { .. } => out.push(e.clone()),
            Ev::Read { .. } | Ev::Write { .. } if skipping == 0 => out.push(e.clone()),
            _ => {}
        }
    }
    Some(out)
}

/// How often `skip` was entered inside the `n`th top-level call of `name`.
fn count_inside(evs: &[Ev], name: &str, n: usize, skip: &str) -> usize {
    let mut seen = 0;
    let (mut inside, mut depth, mut count) = (false, 0, 0);
    for e in evs {
        match e {
            Ev::Enter(x) if x == name => {
                if depth == 0 {
                    inside = seen == n;
                    seen += 1;
                }
                depth += 1;
            }
            Ev::Exit(x) if x == name => {
                depth -= 1;
                if depth == 0 && inside { return count; }
            }
            Ev::Enter(x) if inside && x == skip => count += 1,
            _ => {}
        }
    }
    count
}

/// The event ring's memory as the model wrote it.
#[derive(Default)]
struct Mem(BTreeMap<usize, u32>);

impl Mem {
    fn put(&mut self, index: usize, trb: [u32; 4]) {
        for (k, w) in trb.iter().enumerate() {
            self.0.insert(index * 16 + k * 4, *w);
        }
    }
}

/// Plays a recorded sequence back and fails on the first difference;
/// event lines are written into the ring memory when they come up.
struct Strict<'m> {
    evs: VecDeque<Ev>,
    mem: &'m std::cell::RefCell<Mem>,
}

impl Strict<'_> {
    fn events_due(&mut self) {
        while let Some(Ev::Event { index, trb }) = self.evs.front().cloned() {
            self.mem.borrow_mut().put(index, trb);
            self.evs.pop_front();
        }
    }
    fn done(&mut self) {
        self.events_due();
        assert!(self.evs.is_empty(), "Linux did more: {:x?}", self.evs.iter().take(6).collect::<Vec<_>>());
    }
}

impl Regs for Strict<'_> {
    fn r32(&mut self, off: u32) -> u32 {
        self.events_due();
        match self.evs.pop_front() {
            Some(Ev::Read { off: o, v }) if o == off => v,
            other => panic!("port reads {:#x}, Linux: {:x?}", off, other),
        }
    }
    fn w32(&mut self, off: u32, v: u32) {
        self.events_due();
        match self.evs.pop_front() {
            Some(Ev::Write { off: o, v: lv }) if o == off && lv == v => {}
            other => panic!("port writes {:#x} <- {:#x}, Linux: {:x?}", off, v, other),
        }
    }
    fn delay_us(&mut self, _: u64) {}
}

struct MemRef<'m>(&'m std::cell::RefCell<Mem>);

impl EventMem for MemRef<'_> {
    fn r32(&self, off: usize) -> u32 {
        *self.0.borrow().0.get(&off).unwrap_or(&0)
    }
}

/// CAPLENGTH and RTSOFF as Linux read them.
fn layout(evs: &[Ev]) -> Layout {
    let read = |off| evs.iter().find_map(|e| match e { Ev::Read { off: o, v } if *o == off => Some(*v), _ => None })
        .expect("capability read");
    Layout { op: read(0x00) & 0xff, ir0: (read(0x18) & !0x1f) + 0x20 }
}

fn runs() -> [(&'static str, Vec<Ev>, Vec<(u64, usize)>); 2] {
    let (c1, e1) = parse(include_str!("../testdata/linux_xhci_irq_c1.log"));
    let (d0, e0) = parse(include_str!("../testdata/linux_xhci_irq_d0.log"));
    [("C1", c1, e1), ("D0", d0, e0)]
}

#[test]
fn run_finished_matches_linux() {
    for (name, evs, _) in runs() {
        let mem = std::cell::RefCell::new(Mem::default());
        let seg = segment_nth(&evs, "xhci_run_finished", 0, "").expect(name);
        let mut hw = Strict { evs: seg.into(), mem: &mem };
        let mut st = Intr::default();
        run_finished(&mut hw, &layout(&evs), &mut st, true).expect(name);
        hw.done();
    }
}

#[test]
fn every_interrupt_matches_linux() {
    for (name, evs, erst) in runs() {
        assert!(!erst.is_empty(), "{}: no ERST in the log", name);
        // One block of segments, back to back, as this port lays them out.
        for (i, (base, size)) in erst.iter().enumerate() {
            assert_eq!(*size, TRBS_PER_SEGMENT, "{}: segment {} size", name, i);
            assert_eq!(*base, erst[0].0 + (i * TRBS_PER_SEGMENT * 16) as u64, "{}: segment {} not contiguous", name, i);
        }
        let l = layout(&evs);
        let mem = std::cell::RefCell::new(Mem::default());
        // Events before the first interrupt.
        let first = evs.iter().position(|e| *e == Ev::Enter("xhci_irq".into())).expect(name);
        for e in &evs[..first] {
            if let Ev::Event { index, trb } = e { mem.borrow_mut().put(*index, *trb); }
        }
        let mut ring = EventRing::new(erst[0].0, erst.len());
        let mut st = Intr::default();
        let mut n = 0;
        while let Some(seg) = segment_nth(&evs, "xhci_irq", n, "xhci_handle_event_trb") {
            let linux_events = count_inside(&evs, "xhci_irq", n, "xhci_handle_event_trb");
            let mut hw = Strict { evs: seg.into(), mem: &mem };
            let mut taken = 0;
            irq(&mut hw, &l, &mut st, &mut ring, &MemRef(&mem), |_| taken += 1);
            hw.done();
            assert_eq!(taken, linux_events, "{}: interrupt {} took a different number of events", name, n);
            // Events the model wrote between this interrupt and the next.
            let end = evs.iter().enumerate().filter(|(_, e)| **e == Ev::Exit("xhci_irq".into())).nth(n).map(|(i, _)| i).unwrap();
            let next = evs[end..].iter().position(|e| *e == Ev::Enter("xhci_irq".into())).map(|p| end + p).unwrap_or(evs.len());
            for e in &evs[end..next] {
                if let Ev::Event { index, trb } = e { mem.borrow_mut().put(*index, *trb); }
            }
            n += 1;
        }
        assert!(n > 0, "{}: no interrupt in the log", name);
    }
}

/// Linux's value of the `n`th write to `off` in `seg`.
fn written(seg: &[Ev], off: u32, n: usize) -> u32 {
    seg.iter().filter_map(|e| match e { Ev::Write { off: o, v } if *o == off => Some(*v), _ => None })
        .nth(n).expect("write")
}

fn replay(seg: Vec<Ev>, f: impl FnOnce(&mut Strict)) {
    let mem = std::cell::RefCell::new(Mem::default());
    let mut hw = Strict { evs: seg.into(), mem: &mem };
    f(&mut hw);
    hw.done();
}

#[test]
fn halt_and_reset_match_linux() {
    for (name, evs, _) in runs() {
        let l = layout(&evs);
        replay(segment_nth(&evs, "xhci_halt", 0, "").expect(name), |hw| {
            let mut st = Intr::default();
            halt(hw, &l, &mut st).expect(name);
            assert!(st.halted);
        });
        replay(segment_nth(&evs, "xhci_reset", 0, "").expect(name), |hw| {
            reset(hw, &l, XHCI_RESET_LONG_USEC).expect(name);
        });
    }
}

/// The register part of `xhci_init`, with the driver's structures where
/// Linux had its own: their addresses are what Linux wrote.
#[test]
fn init_matches_linux() {
    for (name, evs, erst) in runs() {
        let l = layout(&evs);
        let seg = segment_nth(&evs, "xhci_init", 0, "xhci_mem_init").expect(name);
        let crcr = written(&seg, l.op + CRCR, 0) as u64 | (written(&seg, l.op + CRCR + 4, 0) as u64) << 32;
        let dcbaa = written(&seg, l.op + DCBAAP, 0) as u64 | (written(&seg, l.op + DCBAAP + 4, 0) as u64) << 32;
        let erstba = written(&seg, l.ir0 + ERSTBA, 0) as u64 | (written(&seg, l.ir0 + ERSTBA + 4, 0) as u64) << 32;
        let hcs1 = evs.iter().find_map(|e| match e { Ev::Read { off: 0x04, v } => Some(*v), _ => None }).unwrap();
        let i = Init {
            max_slots: hcs1 & 0xff,
            cmd_ring: crcr & !0x3f,
            cmd_cycle: (crcr & 1) as u32,
            dcbaa,
            erst: erstba,
            erst_entries: erst.len() as u32,
            event_ring: EventRing::new(erst[0].0, erst.len()),
            write_64_hi_lo: true,
        };
        replay(seg, |hw| { init(hw, &l, &i); });
    }
}

#[test]
fn run_matches_linux() {
    for (name, evs, _) in runs() {
        let l = layout(&evs);
        let seg = segment_nth(&evs, "xhci_run", 0, "xhci_run_finished").expect(name);
        replay(seg, |hw| run(hw, &l));
    }
}

/// The ring geometry `event_ring_segments` derives from HCSPARAMS2.
#[test]
fn segments_follow_erst_max() {
    assert_eq!(event_ring_segments(0), 1);
    assert_eq!(event_ring_segments(2 << 4), 4);
    assert_eq!(event_ring_segments(15 << 4), ERST_DEFAULT_SEGS);
}
