//! The port against Linux: `testdata/linux_rp1_probe_*.log` are runs of
//! the unmodified Linux drivers (pcie-brcmstb, reset-brcmstb(-rescal),
//! irq-bcm2712-mip, rp1 and the PCI core) against a register model, one
//! line per access, with `{ fn` / `} fn` around the driver functions.
//!
//! The driver functions must repeat Linux access for access; the whole
//! bring-up, which replaces the PCI core with the share this topology
//! needs, must leave every register Linux wrote with Linux's final value.
//! The model behind these runs answers RP1's xHCI capability registers as
//! the RP1 datasheet (chapter 5) and the xHCI 1.2 layout give them.

use super::*;
use crate::hw::{Block, Hw};
use std::collections::{BTreeMap, VecDeque};

#[derive(Clone, Debug, PartialEq)]
enum Ev {
    Read { b: Block, off: u64, w: u8, v: u32 },
    Write { b: Block, off: u64, w: u8, v: u32 },
    Enter(String),
    Exit(String),
}

fn block(name: &str) -> Option<Block> {
    Some(match name {
        "pcie2" => Block::Pcie,
        "rescal" => Block::Rescal,
        "bcm_reset" => Block::Reset,
        "mip0" => Block::Mip,
        "rp1.bar0" => Block::Rp1Bar0,
        "rp1.bar1" => Block::Rp1Bar1,
        _ => return None,
    })
}

fn num(s: &str) -> u64 {
    u64::from_str_radix(s.trim_start_matches("0x"), 16).expect(s)
}

fn parse(log: &str) -> Vec<Ev> {
    let mut out = Vec::new();
    for line in log.lines() {
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
        let (Some(op), Some(at)) = (it.next(), it.next()) else { continue };
        let kind = op.as_bytes()[0];
        if !(kind == b'R' || kind == b'W') || op.len() < 2 { continue; }
        let Ok(bits) = op[1..].parse::<u8>() else { continue };
        let (name, off) = at.split_once('+').expect(at);
        let Some(b) = block(name) else { continue };
        let (off, w) = (num(off), bits / 8);
        let v = num(it.nth(1).expect(line)) as u32;
        out.push(if kind == b'R' { Ev::Read { b, off, w, v } } else { Ev::Write { b, off, w, v } });
    }
    out
}

/// The accesses inside the first top-level call of `name`.
fn segment(evs: &[Ev], name: &str) -> Vec<Ev> {
    let start = evs.iter().position(|e| *e == Ev::Enter(name.into())).expect(name);
    let mut depth = 0;
    let mut out = Vec::new();
    for e in &evs[start..] {
        match e {
            Ev::Enter(n) if n == name => depth += 1,
            Ev::Exit(n) if n == name => {
                depth -= 1;
                if depth == 0 { break; }
            }
            Ev::Read { .. } | Ev::Write { .. } => out.push(e.clone()),
            _ => {}
        }
    }
    out
}

/// Plays a recorded sequence back and fails on the first difference.
struct Strict {
    evs: VecDeque<Ev>,
}

impl Hw for Strict {
    fn read(&mut self, b: Block, off: u64, w: u8) -> u32 {
        match self.evs.pop_front() {
            Some(Ev::Read { b: eb, off: eo, w: ew, v }) if (eb, eo, ew) == (b, off, w) => v,
            other => panic!("read {b:?}+{off:#x}/{w}, Linux: {other:?}"),
        }
    }
    fn write(&mut self, b: Block, off: u64, w: u8, v: u32) {
        match self.evs.pop_front() {
            Some(Ev::Write { b: eb, off: eo, w: ew, v: ev }) if (eb, eo, ew, ev) == (b, off, w, v) => {}
            other => panic!("write {b:?}+{off:#x}/{w} <- {v:#x}, Linux: {other:?}"),
        }
    }
    fn delay_us(&mut self, _us: u64) {}
}

impl Strict {
    fn new(evs: Vec<Ev>) -> Self { Strict { evs: evs.into() } }
    fn done(&self) {
        assert!(self.evs.is_empty(), "Linux went on: {:?}", self.evs.front());
    }
}

/// A register file seeded from the log: a config space register the port
/// wrote reads back what it wrote, except after an all-ones BAR sizing write, which
/// answers like the others with the next value Linux read there (the last
/// one repeating). Every other register answers with Linux's reads.
struct Model {
    reads: BTreeMap<(u8, u64), VecDeque<u32>>,
    last_read: BTreeMap<(u8, u64), u32>,
    written: BTreeMap<(u8, u64), u32>,
}

fn key(b: Block, off: u64) -> (u8, u64) { (b as u8, off) }

impl Model {
    fn new(evs: &[Ev]) -> Self {
        let mut reads: BTreeMap<_, VecDeque<u32>> = BTreeMap::new();
        for e in evs {
            if let Ev::Read { b, off, v, .. } = *e {
                reads.entry(key(b, off)).or_default().push_back(v);
            }
        }
        Model { reads, last_read: BTreeMap::new(), written: BTreeMap::new() }
    }
}

impl Hw for Model {
    fn read(&mut self, b: Block, off: u64, _w: u8) -> u32 {
        let k = key(b, off);
        let config = b == Block::Pcie && (off < 0x1000 || (0x8000..0x9000).contains(&off));
        if config
            && let Some(&v) = self.written.get(&k)
            && v != 0xffff_ffff && v != 0xffff_f800
        {
            return v;
        }
        let v = match self.reads.get_mut(&k).and_then(|q| q.pop_front()) {
            Some(v) => v,
            None => *self.last_read.get(&k).unwrap_or_else(|| panic!("read {b:?}+{off:#x}: Linux never did")),
        };
        self.last_read.insert(k, v);
        v
    }
    fn write(&mut self, b: Block, off: u64, _w: u8, v: u32) {
        self.written.insert(key(b, off), v);
    }
    fn delay_us(&mut self, _us: u64) {}
}

fn final_writes(evs: &[Ev]) -> BTreeMap<(u8, u64), u32> {
    let mut m = BTreeMap::new();
    for e in evs {
        if let Ev::Write { b, off, v, .. } = *e {
            m.insert(key(b, off), v);
        }
    }
    m
}

const OUTBOUND: [pcie::Window; 2] = [
    pcie::Window { cpu: 0x1f_0000_0000, pci: 0, size: 0xffff_fffc },
    pcie::Window { cpu: 0x1c_0000_0000, pci: 0x4_0000_0000, size: 0x3_0000_0000 },
];
const INBOUND: [pcie::Window; 3] = [
    pcie::Window { cpu: 0x1f_0000_0000, pci: 0, size: 0x40_0000 },
    pcie::Window { cpu: 0, pci: 0x10_0000_0000, size: 0x10_0000_0000 },
    pcie::Window { cpu: 0x10_0013_0000, pci: 0xff_ffff_f000, size: 0x1000 },
];

/// The node `pcie@1000120000` of bcm2712-rpi-5-b.dtb.
fn cfg() -> pcie::Config<'static> {
    pcie::Config {
        outbound: &OUTBOUND,
        inbound: &INBOUND,
        max_link_speed: 2,
        num_lanes: Some(4),
        aspm_no_l0s: true,
        fifo_qos_map: None,
        vdm_qos_map: Some([0x08, 0x08, 0x08, 0x09, 0x0a, 0x0a, 0x0b, 0x0b]),
        clkreq_mode: pcie::ClkreqMode::Default,
        tperst_clk_ms: 0,
        bridge_reset: Some(0x2c),
        swinit_reset: None,
        rescal: true,
    }
}

/// `msi-controller@1000130000`.
const MIP: mip::Mip = mip::Mip { msi_offset: 0, msi_base: 128, num_msis: 64, msg_addr: 0xff_ffff_f000 };

fn runs() -> [(&'static str, Vec<Ev>); 2] {
    [
        ("d0", parse(include_str!("../testdata/linux_rp1_probe_d0.log"))),
        ("c1", parse(include_str!("../testdata/linux_rp1_probe_c1.log"))),
    ]
}

/// Probe up to the PCI core's first config access.
fn probe_segment(evs: &[Ev]) -> Vec<Ev> {
    let start = evs.iter().position(|e| *e == Ev::Enter("brcm_pcie_probe".into())).unwrap();
    let end = evs[start..].iter().position(|e| *e == Ev::Enter("brcm_pcie_map_bus".into())).unwrap() + start;
    evs[start..end].iter().filter(|e| matches!(e, Ev::Read { .. } | Ev::Write { .. })).cloned().collect()
}

#[test]
fn probe_matches_linux() {
    for (name, evs) in runs() {
        let mut hw = Strict::new(probe_segment(&evs));
        let p = pcie::probe(&mut hw, &cfg()).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        hw.done();
        assert_eq!(p.hw_rev, 0x304, "{name}");
    }
}

#[test]
fn start_link_matches_linux() {
    for (name, evs) in runs() {
        let mut setup = Model::new(&probe_segment(&evs));
        let mut p = pcie::probe(&mut setup, &cfg()).unwrap();
        let mut hw = Strict::new(segment(&evs, "brcm_pcie_start_link"));
        let link = p.start_link(&mut hw).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        hw.done();
        assert_eq!(link, pcie::Link { speed: 2, width: 4 }, "{name}");
    }
}

#[test]
fn mip_matches_linux() {
    for (_, evs) in runs() {
        let mut hw = Strict::new(segment(&evs, "mip_init_domains"));
        MIP.init(&mut hw);
        hw.done();
    }
}

/// Registers whose final value is Linux policy the port does not follow:
/// ASPM (common clock, retrain, L1) and the interrupt line byte.
fn policy_only(b: Block, off: u64) -> bool {
    b == Block::Pcie && matches!(off, 0x00bc | 0x00be | 0x8080 | 0x803c)
}

/// The USB controllers' windows in BAR1, which `dwc3_matches_linux` covers.
fn in_dwc3(b: Block, off: u64) -> bool {
    b == Block::Rp1Bar1 && (0x20_0000..0x40_0000).contains(&off)
}

#[test]
fn bring_up_leaves_linux_state() {
    for (name, evs) in runs() {
        let mut hw = Model::new(&evs);
        let link = bring_up(&mut hw, &cfg(), &MIP, 0).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        assert_eq!(link.rp1.chip_id, rp1::RP1_C0_CHIP_ID);
        let bars: Vec<_> = link.dev.bars[..3].iter().map(|b| (b.bus, b.size)).collect();
        assert_eq!(bars, [(0x41_0000, 0x4000), (0, 0x40_0000), (0x40_0000, 0x1_0000)], "{name}");

        let linux = final_writes(&evs);
        let mut diff = Vec::new();
        for (&(b, off), &v) in &linux {
            let blk = [Block::Pcie, Block::Rescal, Block::Reset, Block::Mip, Block::Rp1Bar0, Block::Rp1Bar1][b as usize];
            if policy_only(blk, off) || in_dwc3(blk, off) { continue; }
            let first_read = evs.iter().find_map(|e| match *e {
                Ev::Read { b: eb, off: eo, v, .. } if key(eb, eo) == (b, off) => Some(v),
                _ => None,
            });
            match hw.written.get(&(b, off)) {
                Some(&ours) if ours == v => {}
                // Written back to the value it had (BAR sizing of the
                // root port, ROM sizing): no change to make.
                None if first_read == Some(v) => {}
                ours => diff.push(format!("{blk:?}+{off:#x}: Linux {v:#x}, port {ours:x?}")),
            }
        }
        for (&(b, off), &v) in &hw.written {
            if !linux.contains_key(&(b, off)) {
                diff.push(format!("block {b}+{off:#x}: port wrote {v:#x}, Linux never"));
            }
        }
        assert!(diff.is_empty(), "{name}:\n{}", diff.join("\n"));
    }
}

/// The accesses inside the `n`th top-level call of `name`, leaving out
/// those inside calls of `skip`.
fn segment_nth(evs: &[Ev], name: &str, n: usize, skip: &str) -> Vec<Ev> {
    let mut seen = 0;
    let (mut depth, mut skipping) = (0, 0);
    let mut out = Vec::new();
    for e in evs {
        match e {
            Ev::Enter(f) if f == name => {
                if depth == 0 { seen += 1; }
                depth += 1;
            }
            Ev::Exit(f) if f == name => {
                depth -= 1;
                if depth == 0 && seen == n + 1 { break; }
            }
            Ev::Enter(f) if f == skip && depth > 0 => skipping += 1,
            Ev::Exit(f) if f == skip && depth > 0 => skipping -= 1,
            Ev::Read { .. } | Ev::Write { .. } if depth > 0 && seen == n + 1 && skipping == 0 => out.push(e.clone()),
            _ => {}
        }
    }
    out
}

/// Strict, except the xHCI capability reads of `dwc3_get_num_ports`,
/// which the port makes in its own order; they answer from `caps`.
struct StrictCaps {
    strict: Strict,
    caps: BTreeMap<u64, u32>,
}

impl Hw for StrictCaps {
    fn read(&mut self, b: Block, off: u64, w: u8) -> u32 {
        let rel = off & 0xf_ffff;
        if b == Block::Rp1Bar1 && (rel == 0x10 || (0x8000..0x8100).contains(&rel)) && self.caps.contains_key(&off) {
            return self.caps[&off];
        }
        self.strict.read(b, off, w)
    }
    fn write(&mut self, b: Block, off: u64, w: u8, v: u32) { self.strict.write(b, off, w, v) }
    fn delay_us(&mut self, _us: u64) {}
}

/// `usb@200000` of bcm2712-rpi-5-b.dtb.
const RP1_USB: dwc3::Config = dwc3::Config {
    dis_rxdet_inp3: true,
    enh_nak_fs: true,
    enh_nak_hs: false,
    parkmode_disable_ss: true,
    parkmode_disable_hs: false,
    parkmode_disable_fsls: true,
    dis_tx_ipgap_linecheck: false,
    dis_enblslpm: false,
    dis_u2_freeclk_exists: false,
    dis_u2_susphy: false,
    dis_u3_susphy: false,
    axi_pipe_limit: 8,
    rx_thr_num_pkt: 0,
    rx_max_burst: 0,
    tx_thr_num_pkt: 2,
    tx_max_burst: 8,
    tx_thr_num_pkt_prd: 0,
    tx_max_burst_prd: 0,
};

/// LINUX_VERSION_CODE of the tree the logs come from.
const LINUX_GUID: u32 = 0x0006_123e;

#[test]
fn dwc3_matches_linux() {
    for (name, evs) in runs() {
        for (i, base) in [0x20_0000u64, 0x30_0000].into_iter().enumerate() {
            let mut caps = BTreeMap::new();
            for e in segment_nth(&evs, "dwc3_get_num_ports", i, "") {
                if let Ev::Read { off, v, .. } = e { caps.insert(off, v); }
            }
            let probe = segment_nth(&evs, "dwc3_probe", i, "dwc3_get_num_ports");
            let mut hw = StrictCaps { strict: Strict::new(probe), caps };
            let d = dwc3::probe(&mut hw, base, RP1_USB, LINUX_GUID).unwrap_or_else(|e| panic!("{name} {i}: {e:?}"));
            hw.strict.done();
            assert_eq!((d.revision, d.num_usb2_ports, d.num_usb3_ports, d.is_fpga), (0x330b, 1, 1, false), "{name} {i}");
        }
    }
}
