//! The Ethernet port against Linux: `testdata/linux_eth_*.log` are the RP1
//! bring-up of `linux_rp1_probe_*` followed by the unmodified rp1-clk,
//! pinctrl-rp1, macb, phylib/phylink and Broadcom PHY drivers against the
//! register model, and a scenario: open, link up, transmit, receive, the
//! interrupt races of steps d.1 and d.2, close. The two steppings differ
//! only in the bring-up.
//!
//! The driver's own sequences replay strictly: the clock ops, `macb_probe`
//! as a whole (clocks, GPIO reset line, interrupt request, MDIO, PHY
//! probe), `macb_open` with `ndo_set_rx_mode`, the phylib/phylink work
//! until the link is up, every `ndo_start_xmit`, every interrupt from the
//! GIC's view down (RP1 mask, `macb_interrupt`, unmask, IACK), every NAPI
//! poll and `macb_close`. Descriptor memory plays the model's part: the
//! model's `# model: gem ...` lines are checked against what the port
//! wrote and applied where the GEM writes back. The clock tree, whose
//! framework the port reduces, is checked by its final state.
//!
//! Left out, being Linux framework without hardware effect or not on this
//! path: the clock framework's registration reads of clocks outside the
//! Ethernet share and its writes to them; pinctrl-rp1's probe (the GPIO
//! bank interrupts, gpiolib's direction reads of every line).

use crate::clk_rp1;
use crate::eth;
use crate::gem::{self, Dma, Ring, Rings};
use crate::hw::{Block, Hw};
use crate::pci;
use crate::phy;
use crate::rp1;
use crate::tests::{parse, segment, segment_nth, Ev, Strict};
use std::collections::BTreeMap;

fn runs() -> [(&'static str, Vec<Ev>); 2] {
    [
        ("d0", parse(include_str!("../testdata/linux_eth_d0.log"))),
        ("c1", parse(include_str!("../testdata/linux_eth_c1.log"))),
    ]
}

fn note(e: &Ev) -> Option<&str> {
    if let Ev::Note(n) = e { Some(n) } else { None }
}

/// The events after the line `== run f: <part>`.
fn part<'a>(evs: &'a [Ev], name: &str) -> &'a [Ev] {
    let tag = format!("== run f: {name}");
    let at = evs.iter().position(|e| note(e) == Some(&tag)).expect(name);
    &evs[at + 1..]
}

/// RP1 as `rp1_probe` left it: the MSI-X table at BAR0 + 0.
fn rp1() -> rp1::Rp1 {
    rp1::Rp1 { chip_id: rp1::RP1_C0_CHIP_ID, fpga: false, msix: pci::MsixTable { block: Block::Rp1Bar0, base: 0 } }
}

/// `eth_hw_addr_random` with the run's fixed random sequence.
const RANDOM_MAC: [u8; 6] = [0x96, 0xd0, 0x0b, 0x46, 0x81, 0xbc];

/// Every top-level call of `name` whose accesses touch BAR1 offsets in
/// `regs`.
fn calls_on(evs: &[Ev], name: &str, regs: std::ops::Range<u64>) -> Vec<Vec<Ev>> {
    let mut depth = 0;
    let mut calls = 0;
    for e in evs {
        match e {
            Ev::Enter(n) if n == name => { if depth == 0 { calls += 1; } depth += 1; }
            Ev::Exit(n) if n == name => depth -= 1,
            _ => {}
        }
    }
    (0..calls).map(|n| segment_nth(evs, name, n, "")).filter(|seg| seg.iter().any(|e| matches!(e,
        Ev::Read { b: Block::Rp1Bar1, off, .. } | Ev::Write { b: Block::Rp1Bar1, off, .. } if regs.contains(off))))
        .collect()
}

fn bar1(reg: u32) -> u64 { clk_rp1::CLOCKS_BASE + reg as u64 }
fn regs_of(first: u32, last: u32) -> std::ops::Range<u64> { bar1(first)..bar1(last) + 4 }

/// The clock ops against every call Linux made of them on the Ethernet
/// share, with the rates the clock core passed (the propagation calls
/// included: a child's new rate is its recalc with the parent's new rate).
#[test]
fn clock_ops_match_linux() {
    use clk_rp1::*;
    for (name, evs) in runs() {
        let evs = part(&evs, "rp1-clk");
        let core = regs_of(0x8000, 0x800c);
        let check = |segs: Vec<Vec<Ev>>, n: usize, run: &mut dyn FnMut(usize, &mut Strict)| {
            assert_eq!(segs.len(), n, "{name}: calls");
            for (i, seg) in segs.into_iter().enumerate() {
                let mut hw = Strict::new(seg);
                run(i, &mut hw);
                hw.done();
            }
        };
        check(calls_on(evs, "rp1_pll_core_on", core.clone()), 1, &mut |_, hw| PLL_SYS_CORE.on(hw).unwrap());
        check(calls_on(evs, "rp1_pll_core_set_rate", core.clone()), 1,
            &mut |_, hw| PLL_SYS_CORE.set_rate(hw, 1_000_000_000, XOSC_HZ).unwrap());
        let prim = [(0, 1_000_000_000), (200_000_000, 1_000_000_000)];
        check(calls_on(evs, "rp1_pll_set_rate", regs_of(0x8010, 0x8010)), 2,
            &mut |i, hw| PLL_SYS.set_rate(hw, prim[i].0, prim[i].1));
        let sec = [(1_000_000_000u64.div_ceil(19), 1_000_000_000), (125_000_000, 1_000_000_000)];
        check(calls_on(evs, "rp1_pll_divider_set_rate", regs_of(0x8014, 0x8014)), 2,
            &mut |i, hw| PLL_SYS_SEC.set_rate(hw, sec[i].0, sec[i].1));
        let eth = [(1_000_000_000u64.div_ceil(19) >> 16, 1_000_000_000u64.div_ceil(19)),
            (125_000_000 / 255, 125_000_000), (125_000_000, 125_000_000)];
        check(calls_on(evs, "rp1_clock_set_rate", regs_of(0x64, 0x70)), 3,
            &mut |i, hw| CLK_ETH.set_rate_and_parent(hw, eth[i].0, eth[i].1, None).unwrap());
        check(calls_on(evs, "rp1_clock_set_rate", regs_of(0x134, 0x140)), 1,
            &mut |_, hw| CLK_ETH_TSU.set_rate_and_parent(hw, 50_000_000, XOSC_HZ, None).unwrap());
        check(calls_on(evs, "rp1_clock_set_rate_and_parent", regs_of(0x14, 0x20)), 1,
            &mut |_, hw| CLK_SYS.set_rate_and_parent(hw, 200_000_000, 200_000_000, Some(2)).unwrap());
        check(calls_on(evs, "rp1_clock_on", regs_of(0x14, 0x20)), 1, &mut |_, hw| CLK_SYS.on(hw));

        let probe = part(evs, "macb");
        check(calls_on(probe, "rp1_pll_divider_on", regs_of(0x8014, 0x8014)), 1, &mut |_, hw| { PLL_SYS_SEC.on(hw); });
        check(calls_on(probe, "rp1_clock_on", regs_of(0x64, 0x70)), 1, &mut |_, hw| CLK_ETH.on(hw));
        check(calls_on(probe, "rp1_clock_on", regs_of(0x134, 0x140)), 1, &mut |_, hw| CLK_ETH_TSU.on(hw));
    }
}

/// RP1's clock registers as the run's model has them: reset 0, a PLL's
/// CS.LOCK reads 1 while PWR.PD and PWR.VCOPD are clear and FBDIV_INT is
/// not 0, the secondary outputs' IMPL bit reads 1. SEL is never written
/// and reads 0.
struct ClockRegs {
    regs: BTreeMap<u64, u32>,
    writes: Vec<(u64, u32)>,
}

impl Hw for ClockRegs {
    fn read(&mut self, b: Block, off: u64, _w: u8) -> u32 {
        assert_eq!(b, Block::Rp1Bar1);
        let get = |o: u64| *self.regs.get(&o).unwrap_or(&0);
        let rel = off - clk_rp1::CLOCKS_BASE;
        let v = get(off);
        match rel {
            0x8000 | 0xc000 | 0x10000 => {
                let locked = get(off + 4) & 0x21 == 0 && get(off + 8) != 0;
                if locked { v | 1 << 31 } else { v & !(1 << 31) }
            }
            0x8014 | 0xc014 | 0xc018 | 0x10014 => v | 1 << 31,
            _ => v,
        }
    }
    fn write(&mut self, b: Block, off: u64, _w: u8, v: u32) {
        assert_eq!(b, Block::Rp1Bar1);
        self.regs.insert(off, v);
        self.writes.push((off, v));
    }
    fn delay_us(&mut self, _us: u64) {}
}

/// The Ethernet share: PLL_SYS core, primary and secondary outputs, CLK_SYS,
/// CLK_ETH, CLK_ETH_TSU.
fn in_share(off: u64) -> bool {
    let rel = off - clk_rp1::CLOCKS_BASE;
    (0x8000..0x8018).contains(&rel) || (0x14..0x24).contains(&rel) || (0x64..0x74).contains(&rel)
        || (0x134..0x144).contains(&rel)
}

/// The rp1-clk probe and `macb_clk_init` leave the share where Linux left
/// it, and write nothing outside it.
#[test]
fn eth_clocks_leave_linux_state() {
    for (name, evs) in runs() {
        let mut linux = BTreeMap::new();
        for e in part(&evs, "rp1-clk").iter().take_while(|e| note(e) != Some("== run f: scenario")) {
            if let Ev::Write { b: Block::Rp1Bar1, off, v, .. } = *e
                && (clk_rp1::CLOCKS_BASE..clk_rp1::CLOCKS_BASE + 0x10038).contains(&off) && in_share(off) {
                linux.insert(off, v);
            }
        }
        let mut hw = ClockRegs { regs: BTreeMap::new(), writes: Vec::new() };
        clk_rp1::eth_clocks(&mut hw).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        assert_eq!(clk_rp1::macb_clk_init(&mut hw), clk_rp1::ETH_RATES);
        let ours: BTreeMap<_, _> = hw.writes.iter().copied().collect();
        assert!(hw.writes.iter().all(|(off, _)| in_share(*off)), "{name}: write outside the share");
        assert_eq!(ours, linux, "{name}");
    }
}

/// On clocks the firmware already set as the DT asks, nothing changes:
/// the locked PLL is left running, every write repeats the value there.
#[test]
fn eth_clocks_keep_running_clocks() {
    let evs = &runs()[0].1;
    let mut regs = BTreeMap::new();
    for e in part(evs, "rp1-clk").iter().take_while(|e| note(e) != Some("== run f: scenario")) {
        if let Ev::Write { b: Block::Rp1Bar1, off, v, .. } = *e && in_share(off) {
            regs.insert(off, v);
        }
    }
    let mut hw = ClockRegs { regs: regs.clone(), writes: Vec::new() };
    clk_rp1::eth_clocks(&mut hw).unwrap();
    clk_rp1::macb_clk_init(&mut hw);
    for (off, v) in &hw.writes {
        assert_eq!(regs.get(off), Some(v), "{off:#x} changed");
    }
    assert_eq!(hw.writes.len(), 5, "{:x?}", hw.writes);
}

/// The model's RX buffers: Linux's sk_buff data areas follow each other at
/// 0x780 bytes from 0x10_4000_6080.
const RX_BUF0: u64 = 0x10_4000_6080;
const RX_BUF_STEP: u64 = 0x780;
const RINGS: Rings = Rings { tx: 0x10_4000_0000, rx: 0x10_4000_3000, tieoff: 0x10_4000_6000 };

/// Descriptor memory with the model's buffer addresses.
struct Mem {
    tx: Vec<[u32; gem::DESC_WORDS]>,
    rx: Vec<[u32; gem::DESC_WORDS]>,
    tieoff: [u32; gem::DESC_WORDS],
    next_rx: u64,
    delivered: Vec<(usize, u64, usize)>,
    tx_done: Vec<(u64, usize)>,
    rx_freed: usize,
}

impl Mem {
    fn new() -> Mem {
        Mem { tx: vec![[0; 6]; gem::RING_SIZE], rx: vec![[0; 6]; gem::RING_SIZE], tieoff: [0; 6],
            next_rx: RX_BUF0, delivered: Vec::new(), tx_done: Vec::new(), rx_freed: 0 }
    }
    fn desc(&mut self, ring: Ring, i: usize) -> &mut [u32; gem::DESC_WORDS] {
        match ring {
            Ring::Tx => &mut self.tx[i],
            Ring::Rx => &mut self.rx[i],
            Ring::TieOff => { assert_eq!(i, 0); &mut self.tieoff }
        }
    }
}

impl Dma for Mem {
    fn read(&mut self, ring: Ring, index: usize, word: usize) -> u32 { self.desc(ring, index)[word] }
    fn write(&mut self, ring: Ring, index: usize, word: usize, v: u32) { self.desc(ring, index)[word] = v; }
    fn barrier(&mut self) {}
    fn rx_alloc(&mut self, _slot: usize) -> Option<u64> {
        let b = self.next_rx;
        self.next_rx += RX_BUF_STEP;
        Some(b)
    }
    fn rx_deliver(&mut self, slot: usize, bus: u64, len: usize, _csum_ok: bool) { self.delivered.push((slot, bus, len)); }
    fn rx_free(&mut self, _slot: usize, _bus: u64) { self.rx_freed += 1; }
    fn tx_done(&mut self, bus: u64, len: usize) { self.tx_done.push((bus, len)); }
}

fn hexes(s: &str) -> Vec<u64> {
    s.split_whitespace().filter_map(|t| t.strip_prefix("0x"))
        .filter_map(|t| u64::from_str_radix(t.trim_end_matches(|c: char| !c.is_ascii_hexdigit()), 16).ok()).collect()
}

/// A `# model: gem ...` line: check what the port left in descriptor
/// memory, or play the GEM's write-back.
fn model_dma(line: &str, mem: &mut Mem) {
    let Some(m) = line.strip_prefix("# model: gem ") else { return };
    let n = hexes(m);
    if m.starts_with("tx desc") && m.contains(" addr ") {
        let d = mem.tx[n[0] as usize];
        assert_eq!(((d[2] as u64) << 32 | d[0] as u64, d[1]), (n[1], n[2] as u32), "{line}");
    } else if m.starts_with("tx desc") && m.contains("used bit set") {
        mem.tx[n[0] as usize][1] |= 1 << 31;
    } else if m.starts_with("rx desc") {
        let d = &mut mem.rx[n[0] as usize];
        assert_eq!(((d[2] as u64) << 32 | (d[0] & !3) as u64, d[0] & 1), (n[1], 0), "{line}");
        d[0] |= 1;
        d[1] = n[2] as u32;
    } else if m.starts_with("tx ring base") {
        assert_eq!(n[0], RINGS.tx);
        for (i, d) in mem.tx.iter().enumerate() {
            assert!(d[1] & 1 << 31 != 0, "tx desc {i} not used");
            assert_eq!(d[1] & 1 << 30 != 0, i == gem::RING_SIZE - 1, "tx desc {i} wrap");
        }
    } else if m.starts_with("rx ring base") {
        assert_eq!(n[0], RINGS.rx);
        for (i, d) in mem.rx.iter().enumerate() {
            let want = if i < gem::RING_SIZE - 1 { RX_BUF0 + i as u64 * RX_BUF_STEP } else { 0 };
            assert_eq!((d[2] as u64) << 32 | d[0] as u64, want, "rx desc {i}");
        }
    }
}

/// The LPI timer request Linux's work lines in `evs` amount to (the last).
fn lpi_request(evs: &[Ev]) -> Option<gem::LpiTimer> {
    let jiffies = |n: &str| n.split("due in ").nth(1).and_then(|t| t.split(' ').next()).map(|t| t.parse::<u64>().unwrap());
    evs.iter().filter_map(note).filter(|n| n.contains("macb_tx_lpi_work_fn")).map(|n| {
        if n.contains("cancelled") { gem::LpiTimer::Cancel } else { gem::LpiTimer::Arm(jiffies(n).unwrap() * 1000) }
    }).last()
}

/// The accesses from `evs[at]` (an `Enter`) to the matching exit of
/// `last`, and the index after it.
fn take_call(evs: &[Ev], at: usize, last: &str) -> (Vec<Ev>, usize) {
    let mut out = Vec::new();
    let mut i = at;
    let Ev::Enter(first) = &evs[at] else { unreachable!() };
    let mut name = first.clone();
    let mut depth = 0;
    loop {
        match &evs[i] {
            Ev::Enter(n) if *n == name => depth += 1,
            Ev::Exit(n) if *n == name => {
                depth -= 1;
                if depth == 0 {
                    if name == last { return (out, i + 1); }
                    // continue into the next call, which must be `last`
                    let next = evs[i + 1..].iter().position(|e| matches!(e, Ev::Enter(_))).unwrap() + i + 1;
                    let Ev::Enter(n2) = &evs[next] else { unreachable!() };
                    assert_eq!(n2, last);
                    assert!(evs[i + 1..next].iter().all(|e| !matches!(e, Ev::Read { .. } | Ev::Write { .. })));
                    name = n2.clone();
                    i = next;
                    continue;
                }
            }
            e @ (Ev::Read { .. } | Ev::Write { .. }) => out.push(e.clone()),
            _ => {}
        }
        i += 1;
    }
}

/// `macb_probe` as a whole.
fn probe(name: &str, evs: &[Ev]) -> eth::Eth {
    let mut hw = Strict::new(segment(part(evs, "macb"), "macb_probe"));
    let e = eth::probe(&mut hw, &rp1(), &eth::PI5, RANDOM_MAC).unwrap_or_else(|e| panic!("{name}: {e:?}"));
    hw.done();
    e
}

#[test]
fn macb_probe_matches_linux() {
    for (name, evs) in runs() {
        let e = probe(name, &evs);
        assert_eq!(e.gem.mac, RANDOM_MAC);
        let c = e.gem.caps;
        assert_eq!((c.mid, c.native_io, c.num_queues), (0x0007_0109, true, 1), "{name}");
        assert_eq!((c.isr_clear_on_write, c.dma64, c.ptp, c.csum, c.lso, c.fifo_mode), (false, true, true, true, false, false));
        assert_eq!(e.phy.state, phy::State::Ready);
        assert_eq!(e.phy.supported_eee, phy::modes::T1000_FULL);
    }
}

/// Run F's scenario, step by step, each driver entry point against its
/// own Linux segment.
#[test]
fn scenario_matches_linux() {
    let rp1 = rp1();
    for (name, evs) in runs() {
        let mut e = probe(name, &evs);
        let mut mem = Mem::new();
        let sc = part(&evs, "scenario");
        let (mut irqs, mut xmits) = (0, 0);
        let tx_bufs = [0x10_400f_5900u64, 0x10_400f_69c0, 0x10_400f_7300];
        let mut i = 0;
        let mut ran = Vec::new();
        while i < sc.len() {
            match &sc[i] {
                Ev::Note(n) if n.starts_with("# step a.1:") => {
                    let end = sc[i..].iter().position(|e| note(e).is_some_and(|n| n.starts_with("# step a.2"))).unwrap() + i;
                    let region = &sc[i..end];
                    let polls = region.iter().filter(|e| note(e) == Some("# work run: phy_state_machine")).count();
                    let mut hw = Strict::new(region.iter().filter(|e| matches!(e, Ev::Read { .. } | Ev::Write { .. })).cloned().collect());
                    let mut last = None;
                    for _ in 0..polls {
                        last = e.poll_link(&mut hw, &mut mem).unwrap_or_else(|x| panic!("{name}: {x:?}")).or(last);
                    }
                    hw.done();
                    assert_eq!(e.gem.take_lpi_timer(), lpi_request(region), "{name}: LPI timer");
                    assert_eq!(polls, 4);
                    let Some(phy::Event::Up(l)) = last else { panic!("{name}: no link") };
                    assert_eq!((l.speed, l.full_duplex, l.rx_pause, l.tx_lpi), (1000, true, false, true));
                    for ev in region.iter().filter_map(note) { model_dma(ev, &mut mem); }
                    ran.push("link".to_string());
                    i = end;
                    continue;
                }
                Ev::Note(n) => model_dma(n, &mut mem),
                Ev::Enter(f) => {
                    let f = f.clone();
                    let last = if f == "macb_open" { "macb_set_rx_mode" } else { f.as_str() };
                    let (seg, next) = take_call(sc, i, last);
                    let mut hw = Strict::new(seg);
                    match f.as_str() {
                        "macb_open" => e.open(&mut hw, &mut mem, RINGS, 0, 0x064b_0825).unwrap(),
                        "macb_start_xmit" => {
                            assert!(!e.gem.xmit(&mut hw, &mut mem, tx_bufs[xmits], 0x40).unwrap());
                            xmits += 1;
                        }
                        "generic_handle_domain_irq" => {
                            let irq = e.interrupt(&mut hw, &rp1);
                            let want = [(false, true), (true, false), (true, true), (false, true), (true, false)][irqs];
                            assert_eq!((irq.handled, irq.rx, irq.tx), (true, want.0, want.1), "{name}: irq {irqs}");
                            irqs += 1;
                        }
                        "macb_tx_poll" => assert_eq!(e.gem.tx_poll(&mut hw, &mut mem, 64), (1, false)),
                        "macb_rx_poll" => {
                            let before = mem.delivered.len();
                            let (work, again) = e.gem.rx_poll(&mut hw, &mut mem, 64);
                            assert_eq!((work, again), (mem.delivered.len() - before, false));
                        }
                        "macb_close" => e.close(&mut hw, &mut mem).unwrap(),
                        other => panic!("{name}: {other} not driven"),
                    }
                    hw.done();
                    assert_eq!(e.gem.take_lpi_timer(), lpi_request(&sc[i..next]), "{name}: {f}: LPI timer");
                    ran.push(f);
                    // Model lines inside the call (the GEM's view during it).
                    for ev in sc[i..next].iter().filter_map(note) { model_dma(ev, &mut mem); }
                    i = next;
                    continue;
                }
                Ev::Read { .. } | Ev::Write { .. } => panic!("{name}: access outside a driver call: {:?}", sc[i]),
                Ev::Exit(_) => {}
            }
            i += 1;
        }
        assert_eq!(ran, ["macb_open", "link", "macb_start_xmit", "generic_handle_domain_irq", "macb_tx_poll",
            "generic_handle_domain_irq", "macb_rx_poll", "macb_start_xmit", "generic_handle_domain_irq",
            "macb_rx_poll", "macb_tx_poll", "macb_start_xmit", "generic_handle_domain_irq",
            "generic_handle_domain_irq", "macb_tx_poll", "macb_rx_poll", "macb_close"], "{name}");
        let rx: Vec<_> = mem.delivered.iter().map(|d| (d.0, d.2)).collect();
        assert_eq!(rx, [(0, 0x3c), (1, 0x3c), (2, 0x3c), (3, 0x3c)], "{name}");
        assert_eq!(mem.tx_done, tx_bufs.iter().map(|&b| (b, 0x40)).collect::<Vec<_>>(), "{name}");
        assert_eq!(mem.rx_freed, gem::RING_SIZE - 1, "{name}");
        assert!(e.link.is_none());
        assert_eq!(e.phy.state, phy::State::Halted);
    }
}

/// The interrupt request of `macb_init` alone: line 6 as level, IACK_EN
/// then ENABLE, the vector unmasked.
#[test]
fn rp1_eth_irq_request_matches_linux() {
    for (name, evs) in runs() {
        let mac = part(&evs, "macb");
        let mut want = segment(mac, "rp1_irq_set_type");
        want.extend(segment(mac, "rp1_irq_activate"));
        want.extend(segment(mac, "rp1_unmask_irq"));
        let mut hw = Strict::new(want);
        let r = rp1();
        r.irq_enable(&mut hw, 6, rp1::Trigger::Level);
        r.irq_unmask(&mut hw, 6);
        hw.done();
        let _ = name;
    }
}

/// `macb_pad_and_fcs`: zero padding to 60 bytes and the FCS, little
/// endian; the CRC is IEEE 802.3's (check value 0xcbf43926).
#[test]
fn pad_and_fcs() {
    let crc = |d: &[u8]| {
        let mut c = !0u32;
        for &x in d { c ^= x as u32; for _ in 0..8 { c = (c >> 1) ^ (0xedb8_8320 & (c & 1).wrapping_neg()); } }
        c
    };
    assert_eq!(!crc(b"123456789"), 0xcbf4_3926);

    let mut b = [0xffu8; 64];
    b[..9].copy_from_slice(b"123456789");
    assert_eq!(gem::pad_and_fcs(&mut b[..63], 9), None);
    assert_eq!(gem::pad_and_fcs(&mut b, 9), Some(64));
    assert!(b[9..60].iter().all(|&x| x == 0));
    // A frame followed by its FCS leaves the CRC residue.
    assert_eq!(crc(&b), 0xdebb_20e3);

    let mut long = [0u8; 104];
    for (i, x) in long[..100].iter_mut().enumerate() { *x = i as u8; }
    assert_eq!(gem::pad_and_fcs(&mut long, 100), Some(104));
    assert_eq!(crc(&long), 0xdebb_20e3);
}
