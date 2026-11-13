//! The port against Linux: `testdata/linux_vc4_d0.log` is run G of the Pi 5
//! oracle, the unmodified vc4, i2c-brcmstb, irq-brcmstb-l2 and
//! clk-raspberrypi drivers bound on the D0 board's device tree against a
//! register model started from the state the firmware leaves (measured on
//! the board). One line per access, `{ fn` / `} fn` around the driver
//! functions, `FW` lines at the firmware property interface. The port
//! must repeat it access for access.

use crate::drm_edid::drm_do_probe_ddc_edid;
use crate::hw::{Block, ClockOp, HdmiReg, Hw, Spi};
use crate::i2c_brcmstb::{Bsc, Ddc};
use crate::irq_brcmstb_l2::{L2Intc, L2_2711_LVL_INTC_INIT, L2_LVL_INTC_INIT};
use std::collections::VecDeque;

const LOG: &str = include_str!("../testdata/linux_vc4_d0.log");

#[derive(Clone, Debug, PartialEq)]
enum Ev {
    Read { b: Block, off: u32, v: u32 },
    Write { b: Block, off: u32, v: u32 },
    /// A firmware property call: tag, request words, answer words.
    Fw { tag: u32, req: Vec<u32>, ans: Vec<u32> },
    Enter(String),
    Exit(String),
    Note(String),
}

fn block(name: &str) -> Option<Block> {
    let hdmi = |n: u8, sub: &str| {
        Some(Block::Hdmi(n, match sub {
            "" => HdmiReg::Hdmi,
            "dvp" => HdmiReg::Dvp,
            "phy" => HdmiReg::Phy,
            "rm" => HdmiReg::Rm,
            "packet" => HdmiReg::Packet,
            "metadata" => HdmiReg::Metadata,
            "csc" => HdmiReg::Csc,
            "cec" => HdmiReg::Cec,
            _ => return None,
        }))
    };
    let (base, sub) = name.split_once('.').unwrap_or((name, ""));
    match base {
        "hvs" => Some(Block::Hvs),
        "pv0" => Some(Block::Pv(0)),
        "pv1" => Some(Block::Pv(1)),
        "disp_intr" => Some(Block::DispIntr),
        "dvp" => Some(Block::Dvp),
        "hdmi0" => hdmi(0, sub),
        "hdmi1" => hdmi(1, sub),
        "hd" => Some(Block::Hdmi(0, HdmiReg::Hd)),
        "ddc0" => Some(Block::Ddc(0)),
        "ddc1" => Some(Block::Ddc(1)),
        "bsc_irq" => Some(Block::BscIrq),
        "aon_intr" => Some(Block::AonIntr),
        _ => None,
    }
}

/// `hd` is one window both controllers map.
fn norm(b: Block) -> Block {
    match b {
        Block::Hdmi(_, HdmiReg::Hd) => Block::Hdmi(0, HdmiReg::Hd),
        b => b,
    }
}

fn num(s: &str) -> u32 {
    u32::from_str_radix(s.trim_start_matches("0x"), 16).expect(s)
}

fn words(s: &str) -> Vec<u32> {
    s.trim().trim_start_matches('[').trim_end_matches(']').split_whitespace().map(num).collect()
}

fn parse(log: &str) -> Vec<Ev> {
    let mut out = Vec::new();
    for line in log.lines() {
        if let Some(rest) = line.strip_prefix("FW ") {
            let tag = num(rest.split('(').nth(1).unwrap().split(')').next().unwrap());
            let (_, io) = rest.rsplit_once(": ").unwrap();
            let (req, ans) = io.split_once(" -> ").unwrap();
            out.push(Ev::Fw { tag, req: words(req), ans: words(ans) });
            continue;
        }
        let line = line.split(" ;").next().unwrap_or("");
        if let Some(n) = line.strip_prefix("{ ") {
            out.push(Ev::Enter(n.to_string()));
            continue;
        }
        if let Some(n) = line.strip_prefix("} ") {
            out.push(Ev::Exit(n.to_string()));
            continue;
        }
        let mut it = line.split_whitespace();
        let (Some(op), Some(at)) = (it.next(), it.next()) else {
            out.push(Ev::Note(line.to_string()));
            continue;
        };
        if op != "R32" && op != "W32" {
            out.push(Ev::Note(line.to_string()));
            continue;
        }
        let (name, off) = at.split_once('+').expect(at);
        let Some(b) = block(name) else { continue };
        let v = num(it.nth(1).expect(line));
        out.push(if op == "R32" { Ev::Read { b, off: num(off), v } } else { Ev::Write { b, off: num(off), v } });
    }
    out
}

/// The accesses inside the `nth` (from 0) top-level call of `name`.
fn segment(evs: &[Ev], name: &str, nth: usize) -> Vec<Ev> {
    let starts: Vec<usize> = evs.iter().enumerate()
        .filter(|(_, e)| **e == Ev::Enter(name.into())).map(|(i, _)| i).collect();
    let mut depth = 0;
    let mut out = Vec::new();
    let mut k = 0;
    let mut i = starts[0];
    while i < evs.len() {
        match &evs[i] {
            Ev::Enter(n) if n == name => depth += 1,
            Ev::Exit(n) if n == name => {
                depth -= 1;
                if depth == 0 {
                    if k == nth { break; }
                    k += 1;
                    i = starts[starts.iter().position(|&s| s > i).expect(name)];
                    continue;
                }
            }
            e @ (Ev::Read { .. } | Ev::Write { .. } | Ev::Fw { .. }) if k == nth => out.push(e.clone()),
            _ => {}
        }
        i += 1;
    }
    out
}

/// Plays a recorded sequence back and fails on the first difference.
struct Strict {
    evs: VecDeque<Ev>,
}

impl Strict {
    fn new(evs: Vec<Ev>) -> Self {
        Strict { evs: evs.into() }
    }

    fn done(&self) {
        assert!(self.evs.is_empty(), "Linux went on: {:?}", self.evs.front());
    }
}

const GET_CLOCK_RATE: u32 = 0x0003_0002;
const GET_MAX_CLOCK_RATE: u32 = 0x0003_0004;
const GET_MIN_CLOCK_RATE: u32 = 0x0003_0007;
const SET_CLOCK_STATE: u32 = 0x0003_8001;
const SET_CLOCK_RATE: u32 = 0x0003_8002;
const NOTIFY_DISPLAY_DONE: u32 = 0x0003_0066;

impl Hw for Strict {
    fn read(&mut self, b: Block, off: u32) -> u32 {
        match self.evs.pop_front() {
            Some(Ev::Read { b: eb, off: eo, v }) if (norm(eb), eo) == (norm(b), off) => v,
            other => panic!("read {b:?}+{off:#x}, Linux: {other:?}"),
        }
    }
    fn write(&mut self, b: Block, off: u32, v: u32) {
        match self.evs.pop_front() {
            Some(Ev::Write { b: eb, off: eo, v: ev }) if (norm(eb), eo, ev) == (norm(b), off, v) => {}
            other => panic!("write {b:?}+{off:#x} <- {v:#x}, Linux: {other:?}"),
        }
    }
    fn now_us(&mut self) -> u64 { 0 }
    fn delay_us(&mut self, _: u64) {}
    fn irq_arm(&mut self, _: Spi) -> u64 { 0 }
    fn irq_wait(&mut self, _: Spi, _: u64, _: u32) -> bool { true }
    fn fw_clock(&mut self, id: u32, op: ClockOp) -> Option<u32> {
        let (tag, req1) = match op {
            ClockOp::Rate => (GET_CLOCK_RATE, 0),
            ClockOp::MaxRate => (GET_MAX_CLOCK_RATE, 0),
            ClockOp::MinRate => (GET_MIN_CLOCK_RATE, 0),
            ClockOp::SetRate(r) => (SET_CLOCK_RATE, r),
            ClockOp::Enable => (SET_CLOCK_STATE, 1),
        };
        match self.evs.pop_front() {
            Some(Ev::Fw { tag: t, req, ans }) if t == tag && req[0] == id && req[1] == req1 => Some(ans[1]),
            other => panic!("firmware {op:?} clock {id}, Linux: {other:?}"),
        }
    }
    fn fw_display_done(&mut self) -> bool {
        match self.evs.pop_front() {
            Some(Ev::Fw { tag: NOTIFY_DISPLAY_DONE, .. }) => true,
            other => panic!("NOTIFY_DISPLAY_DONE, Linux: {other:?}"),
        }
    }
}

/// The three L2 controllers and both DDC controllers as Linux probes
/// them, in Linux's order.
fn probe_bsc(evs: &[Ev]) -> Bsc {
    let mut hw = Strict::new(segment(evs, "brcmstb_l2_driver_init", 0));
    let _aon = L2Intc::probe(&mut hw, Block::AonIntr, &L2_2711_LVL_INTC_INIT);
    let _disp = L2Intc::probe(&mut hw, Block::DispIntr, &L2_2711_LVL_INTC_INIT);
    let bsc_irq = L2Intc::probe(&mut hw, Block::BscIrq, &L2_LVL_INTC_INIT);
    hw.done();
    let mut bsc = Bsc::new(bsc_irq);
    let mut hw = Strict::new(segment(evs, "brcmstb_i2c_driver_init", 0));
    bsc.probe(&mut hw, 0, 1, 97500);
    bsc.probe(&mut hw, 1, 2, 97500);
    hw.done();
    bsc
}

#[test]
fn l2_and_i2c_probe_are_linux() {
    probe_bsc(&parse(LOG));
}

/// The model's bytes on a `# model: ddc0 read 0x50: ...` line.
fn model_reads(evs: &[Ev]) -> Vec<u8> {
    evs.iter().filter_map(|e| match e {
        Ev::Note(n) if n.starts_with("# model: ddc0 read 0x50:") => Some(n),
        _ => None,
    }).flat_map(|n| {
        n.split(": ").nth(2).unwrap().split(" [").next().unwrap()
            .split_whitespace().map(|b| u8::from_str_radix(b, 16).unwrap()).collect::<Vec<_>>()
    }).collect()
}

#[test]
fn edid_read_is_linux() {
    let evs = parse(LOG);
    let mut bsc = probe_bsc(&evs);
    let mut got = Vec::new();
    // `drm_probe_ddc`, then `_drm_do_get_edid` for the base block and the
    // one extension the base block announces.
    for (nth, len, block) in [(0, 1, 0), (1, 128, 0), (2, 128, 1)] {
        let mut hw = Strict::new(segment(&evs, "drm_do_probe_ddc_edid", nth));
        let mut buf = vec![0u8; len];
        drm_do_probe_ddc_edid(&mut hw, &mut Ddc { bsc: &mut bsc, n: 0 }, &mut buf, block).expect("read");
        hw.done();
        got.extend(buf);
    }
    assert_eq!(got.len(), 257);
    assert_eq!(got, model_reads(&evs));
}
