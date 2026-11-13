//! The port against Linux: `testdata/linux_vc4_d0.log` is run G of the Pi 5
//! oracle, the unmodified vc4, i2c-brcmstb, irq-brcmstb-l2 and
//! clk-raspberrypi drivers bound on the D0 board's device tree against a
//! register model started from the state the firmware leaves (measured on
//! the board). One line per access, `{ fn` / `} fn` around the driver
//! functions, `FW` lines at the firmware property interface. The port
//! must repeat it access for access.

use crate::clk::FwClocks;
use drm_core::drm_edid::drm_do_probe_ddc_edid;
use crate::hw::{Block, ClockOp, HdmiReg, Hw, Spi};
use crate::i2c_brcmstb::{Bsc, Ddc};
use crate::irq_brcmstb_l2::{L2Intc, L2_2711_LVL_INTC_INIT, L2_LVL_INTC_INIT};
use crate::vc4_drv::Vc4;
use crate::vc4_hdmi::Vc4Hdmi;
use crate::vc4_hvs::{Gen, Hvs};
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
            ClockOp::Disable => (SET_CLOCK_STATE, 0),
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
/// them, in Linux's order: aon_intr, disp_intr, bsc_irq.
fn probe_l2(evs: &[Ev]) -> (L2Intc, L2Intc, Bsc) {
    let mut hw = Strict::new(segment(evs, "brcmstb_l2_driver_init", 0));
    let aon = L2Intc::probe(&mut hw, Block::AonIntr, &L2_2711_LVL_INTC_INIT);
    let disp = L2Intc::probe(&mut hw, Block::DispIntr, &L2_2711_LVL_INTC_INIT);
    let bsc_irq = L2Intc::probe(&mut hw, Block::BscIrq, &L2_LVL_INTC_INIT);
    hw.done();
    let mut bsc = Bsc::new(bsc_irq);
    let mut hw = Strict::new(segment(evs, "brcmstb_i2c_driver_init", 0));
    bsc.probe(&mut hw, 0, 1, 97500);
    bsc.probe(&mut hw, 1, 2, 97500);
    hw.done();
    (aon, disp, bsc)
}

fn probe_bsc(evs: &[Ev]) -> Bsc {
    probe_l2(evs).2
}

const GET_CLOCKS: u32 = 0x0001_0007;
const V3D_CLK: u32 = 5;

/// `raspberrypi_clk_probe` for the display's clocks: the module neither
/// asks `GET_CLOCKS` (no module may) nor registers V3D's clock.
fn register_clocks(evs: &[Ev]) -> FwClocks {
    let seg: Vec<Ev> = segment(evs, "raspberrypi_clk_driver_init", 0).into_iter().filter(|e| match e {
        Ev::Fw { tag, req, .. } => *tag != GET_CLOCKS && req.first() != Some(&V3D_CLK),
        _ => true,
    }).collect();
    let mut hw = Strict::new(seg);
    let clocks = FwClocks::register(&mut hw).expect("register");
    hw.done();
    clocks
}

#[test]
fn clock_registration_is_linux() {
    register_clocks(&parse(LOG));
}

#[test]
fn hvs_bind_is_linux() {
    let evs = parse(LOG);
    let (_aon, mut disp, _bsc) = probe_l2(&evs);
    let mut clocks = register_clocks(&evs);
    let mut hw = Strict::new(segment(&evs, "vc4_hvs_bind", 0));
    let hvs = Hvs::bind(&mut hw, &mut disp, &mut clocks).expect("bind");
    hw.done();
    assert_eq!(hvs.generation, Gen::Gen6D);
    assert_eq!(hvs.dlist_mem_size, 0x1000);
    assert_eq!(hvs.max_core_rate, 0x363d7f80);
    assert!(hvs.vc5_hdmi_enable_hdmi_20 && hvs.vc5_hdmi_enable_4096by2160);
    assert_eq!((hvs.mitchell_netravali_filter.start, hvs.nearest_neighbour_filter.start), (32, 43));
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
        let mut ddc = Ddc { bsc: &mut bsc, n: 0 };
        assert_eq!(drm_do_probe_ddc_edid(&mut |m| ddc.i2c_transfer(&mut hw, m), &mut buf, block), 0, "read");
        hw.done();
        got.extend(buf);
    }
    assert_eq!(got.len(), 257);
    assert_eq!(got, model_reads(&evs));
}

#[test]
fn hdmi_bind_is_linux() {
    let evs = parse(LOG);
    let (mut aon, mut disp, _bsc) = probe_l2(&evs);
    let mut clocks = register_clocks(&evs);
    let mut hw = Strict::new(segment(&evs, "vc4_hvs_bind", 0));
    Hvs::bind(&mut hw, &mut disp, &mut clocks).expect("hvs");
    for port in 0..2 {
        let mut hw = Strict::new(segment(&evs, "vc4_hdmi_bind", port as usize));
        let h = Vc4Hdmi::bind(&mut hw, port, &mut aon, &mut clocks).expect("hdmi");
        hw.done();
        assert!(h.scdc_enabled);
    }
}

/// The accesses from the start up to the first call of `end`, without
/// those inside calls of `skip`.
fn upto(evs: &[Ev], end: &str, skip: &[&str]) -> Vec<Ev> {
    let mut out = Vec::new();
    let mut depth = 0;
    for e in evs {
        match e {
            Ev::Enter(n) if n == end && depth == 0 => break,
            Ev::Enter(n) if skip.contains(&n.as_str()) => depth += 1,
            Ev::Exit(n) if skip.contains(&n.as_str()) => depth -= 1,
            Ev::Read { .. } | Ev::Write { .. } if depth == 0 => out.push(e.clone()),
            Ev::Fw { tag, req, .. } if depth == 0 && *tag != GET_CLOCKS && req.first() != Some(&V3D_CLK) => {
                out.push(e.clone())
            }
            _ => {}
        }
    }
    out
}

/// The whole bind as one stream: the L2 controllers, the clocks, the DDC
/// controllers, NOTIFY_DISPLAY_DONE, the HVS, both HDMI controllers, both
/// pixel valves and the takeover of both running outputs, without the
/// writeback engines.
#[test]
fn bind_and_takeover_are_linux() {
    let evs = parse(LOG);
    let stream = upto(&evs, "drm_fb_helper_initial_config", &["vc4_txp_bind"]);
    assert!(stream.len() > 350, "{} events", stream.len());
    let mut hw = Strict::new(stream);
    let vc4 = Vc4::bind(&mut hw).expect("bind");
    hw.done();
    assert!(!vc4.hdmi[0].scdc_enabled && !vc4.hdmi[1].scdc_enabled);
}

/// Linux's `# kms:` lines for one connector, up to the next connector or
/// modeset.
fn kms_connector_lines(name: &str) -> Vec<String> {
    let head = format!("# kms: connector {} ", name);
    let mut out = Vec::new();
    let mut on = false;
    for l in LOG.lines() {
        if l.starts_with("# kms: connector ") || l.starts_with("# kms: modeset ") {
            on = l.starts_with(&head);
        }
        if on {
            out.push(l.to_string());
        }
    }
    out
}

/// The fbdev client's probe after the bind: both connectors detected
/// (hotplug, EDID over DDC 0, runtime PM around it), the modes validated
/// by vc4, and Linux's choice. The port's CRTC `n` is pixel valve `n`,
/// Linux's CRTC `n + 2` (its writeback CRTCs come first).
#[test]
fn fbdev_probe_is_linux() {
    use drm_core::kms_dump::{connector_lines, modeset_lines};

    let evs = parse(LOG);
    let mut hw = Strict::new(upto(&evs, "drm_fb_helper_initial_config", &["vc4_txp_bind"]));
    let mut vc4 = Vc4::bind(&mut hw).expect("bind");
    hw.done();

    let mut hw = Strict::new(segment(&evs, "drm_client_modeset_probe", 0));
    let client = vc4.fbdev_probe(&mut hw);
    hw.done();

    for (i, c) in vc4.connectors.iter().enumerate() {
        assert_eq!(connector_lines(c), kms_connector_lines(&c.name), "connector {i}");
    }
    let linux: Vec<String> = LOG.lines()
        .filter(|l| l.starts_with("# kms: modeset crtc 2 ") || l.starts_with("# kms: modeset crtc 3 ")
            || l.starts_with("# kms: chosen "))
        .map(String::from).collect();
    let port: Vec<String> = modeset_lines(&client, &vc4.connectors).into_iter()
        .map(|l| match l.strip_prefix("# kms: modeset crtc ") {
            Some(rest) => {
                let (n, tail) = rest.split_once(' ').unwrap();
                format!("# kms: modeset crtc {} {}", n.parse::<u32>().unwrap() + 2, tail)
            }
            None => l,
        })
        .collect();
    assert_eq!(port, linux);
}

/// The HDMI encoder's part of the fbdev client's first commit, each
/// function against its run G segment: the connector state as
/// `drm_atomic_helper_connector_hdmi_check` computes it (RGB, 8 bpc,
/// 594 MHz, limited range, AVI, SPD and HDMI vendor infoframes), the
/// encoder's mode fixup, then `pre_crtc_configure` (clocks, PHY,
/// timings), `pre_crtc_enable` (CSC) and `post_crtc_enable` (infoframes,
/// FIFO, scrambling over SCDC).
#[test]
fn hdmi_encoder_enable_is_linux() {
    use drm_core::drm_connector::{__drm_atomic_helper_connector_hdmi_reset, drm_atomic_connector_check,
        drm_connector_state};
    use crate::vc4_hdmi::vc4_hdmi_connector_atomic_check;

    let evs = parse(LOG);
    let mut hw = Strict::new(upto(&evs, "drm_fb_helper_initial_config", &["vc4_txp_bind"]));
    let mut vc4 = Vc4::bind(&mut hw).expect("bind");
    let mut hw = Strict::new(segment(&evs, "drm_client_modeset_probe", 0));
    let client = vc4.fbdev_probe(&mut hw);
    let mode = client.modesets[0].mode.clone().expect("mode on pixel valve 0");

    let connector = &vc4.connectors[0];
    let mut old = drm_connector_state::default();
    __drm_atomic_helper_connector_hdmi_reset(connector, &mut old);
    let mut new = old;
    new.crtc = Some(0);
    new.best_encoder = Some(0);
    drm_atomic_connector_check(connector, &mut new);
    assert_eq!(new.max_bpc, 8);
    assert!(vc4_hdmi_connector_atomic_check(connector, &old, &mut new, &mode, &vc4.hvs).expect("check"));
    assert_eq!((new.hdmi.output_format, new.hdmi.output_bpc, new.hdmi.tmds_char_rate), (0, 8, 594_000_000));
    assert!(new.hdmi.is_limited_range);
    assert!(new.hdmi.infoframes.avi.set && new.hdmi.infoframes.spd.set && new.hdmi.infoframes.hdmi.set);

    let mut adjusted = mode.clone();
    assert_eq!(vc4.hdmi[0].encoder_atomic_check(&mut adjusted), 0);
    vc4.hdmi[0].encoder_atomic_mode_set(&adjusted, &new);

    let mut hw = Strict::new(segment(&evs, "vc4_hdmi_encoder_pre_crtc_configure", 0));
    vc4.hdmi[0].encoder_pre_crtc_configure(&mut hw, &mut vc4.clocks, &new);
    hw.done();

    let mut hw = Strict::new(segment(&evs, "vc4_hdmi_encoder_pre_crtc_enable", 0));
    vc4.hdmi[0].encoder_pre_crtc_enable(&mut hw, &new);
    hw.done();

    let mut hw = Strict::new(segment(&evs, "vc4_hdmi_encoder_post_crtc_enable", 0));
    let connector = &vc4.connectors[0];
    vc4.hdmi[0].encoder_post_crtc_enable(&mut hw, &mut vc4.bsc, connector, &old, &new, &vc4.hvs);
    hw.done();
    assert!(vc4.hdmi[0].scdc_enabled && vc4.hdmi[0].scrambling_work.is_some());
}

/// The fbdev framebuffer of run G: 3840x2160 RGB565, pitch 7680, at the
/// bus address `vc5_dumb_create` got (`dma alloc ... at 0x40008000`).
fn run_g_fb() -> drm_core::drm_plane::drm_framebuffer {
    use drm_core::drm_fourcc::drm_format_info;
    drm_core::drm_plane::drm_framebuffer {
        format: drm_format_info(drm_core::regs::DRM_FORMAT_RGB565).expect("RGB565"),
        width: 3840,
        height: 2160,
        pitches: [7680, 0, 0, 0],
        offsets: [0; 4],
        modifier: drm_core::regs::DRM_FORMAT_MOD_LINEAR as u64,
        dma_addr: [0x4000_8000, 0, 0, 0],
    }
}

/// The fbdev client's first commit (`drm_fb_helper_set_par`) after bind
/// and probe, as one stream: the plane's dlist at 54, the background,
/// the pixel valve, the HVS channel, the HDMI encoder, then the flip on
/// the first vblank interrupt.
#[test]
fn fbdev_commit_is_linux() {
    let evs = parse(LOG);
    let mut hw = Strict::new(upto(&evs, "drm_fb_helper_initial_config", &["vc4_txp_bind"]));
    let mut vc4 = Vc4::bind(&mut hw).expect("bind");
    let mut hw = Strict::new(segment(&evs, "drm_client_modeset_probe", 0));
    let client = vc4.fbdev_probe(&mut hw);
    hw.done();

    let mut hw = Strict::new(segment(&evs, "drm_fb_helper_set_par", 0));
    assert_eq!(vc4.fbdev_commit(&mut hw, &client, &run_g_fb()), 0);
    hw.done();

    let c = &vc4.kms.crtcs[0];
    assert_eq!((c.assigned_channel, c.mm.map(|m| m.node.start)), (Some(0), Some(54)));
    assert!(!vc4.crtc[0].event);
    assert_eq!(vc4.kms.vblank[0].refcount, 0);
}

#[test]
fn fbdev_commit_segment_is_the_whole_commit() {
    let evs = parse(LOG);
    let seg = segment(&evs, "drm_fb_helper_set_par", 0);
    assert!(seg.len() > 250, "{} events", seg.len());
}

/// The takeover keeps the firmware's size when the monitor has a mode of
/// it (run G's monitor has 1920x1080 at 148.5 MHz), and falls back to
/// the fbdev client's choice otherwise.
#[test]
fn takeover_picks_the_framebuffer_size() {
    let evs = parse(LOG);
    let mut hw = Strict::new(upto(&evs, "drm_fb_helper_initial_config", &["vc4_txp_bind"]));
    let mut vc4 = Vc4::bind(&mut hw).expect("bind");
    let mut hw = Strict::new(segment(&evs, "drm_client_modeset_probe", 0));
    let client = vc4.fbdev_probe(&mut hw);

    let m = vc4.takeover_modesets(&client, 1920, 1080);
    let mode = m.modesets[0].mode.as_ref().expect("pixel valve 0");
    assert_eq!((mode.hdisplay, mode.vdisplay, mode.clock), (1920, 1080, 148500));
    assert_eq!(m.modesets[0].connectors, vec![0]);
    assert!(m.modesets[1].mode.is_none());

    let m = vc4.takeover_modesets(&client, 1366, 768);
    assert_eq!(m.modesets[0].mode.as_ref().map(|m| (m.hdisplay, m.vdisplay)), Some((3840, 2160)));
}

/// Registers as the last write left them, every write kept.
#[derive(Default)]
struct Regs {
    vals: std::collections::BTreeMap<(String, u32), u32>,
    writes: Vec<(Block, u32, u32)>,
}

impl Hw for Regs {
    /// A display that runs: every vblank arrives, and the HVS reads the
    /// dlist its list pointer names.
    fn read(&mut self, b: Block, off: u32) -> u32 {
        use crate::regs::{PV_INTSTAT, SCALER6_DISPX_DL, SCALER6_DISPX_LPTRS};
        if matches!(b, Block::Pv(_)) && off == PV_INTSTAT {
            return u32::MAX;
        }
        for g in 0..2 {
            if let Some(c) = SCALER6_DISPX_DL[g].iter().position(|&r| r == off) && b == Block::Hvs {
                return self.read(b, SCALER6_DISPX_LPTRS[g][c]) & 0xfff;
            }
        }
        *self.vals.get(&(format!("{b:?}"), off)).unwrap_or(&0)
    }
    fn write(&mut self, b: Block, off: u32, v: u32) {
        self.vals.insert((format!("{b:?}"), off), v);
        self.writes.push((b, off, v));
    }
    fn now_us(&mut self) -> u64 { 0 }
    fn delay_us(&mut self, _: u64) {}
    fn irq_arm(&mut self, _: Spi) -> u64 { 0 }
    fn irq_wait(&mut self, _: Spi, _: u64, _: u32) -> bool { true }
    fn fw_clock(&mut self, _: u32, _: ClockOp) -> Option<u32> { Some(0) }
    fn fw_display_done(&mut self) -> bool { true }
}

/// A page flip after run G's fbdev commit touches only the HVS: a new
/// dlist with the other buffer's address, no pixel valve, no HDMI.
#[test]
fn page_flip_rewrites_only_the_dlist() {
    let evs = parse(LOG);
    let mut hw = Strict::new(upto(&evs, "drm_fb_helper_initial_config", &["vc4_txp_bind"]));
    let mut vc4 = Vc4::bind(&mut hw).expect("bind");
    let mut hw = Strict::new(segment(&evs, "drm_client_modeset_probe", 0));
    let client = vc4.fbdev_probe(&mut hw);
    let mut hw = Strict::new(segment(&evs, "drm_fb_helper_set_par", 0));
    assert_eq!(vc4.fbdev_commit(&mut hw, &client, &run_g_fb()), 0);

    let mut other = run_g_fb();
    other.dma_addr[0] = 0x5000_0000;
    let mut regs = Regs::default();
    assert_eq!(vc4.page_flip(&mut regs, &other), 0);
    // The pixel valve only acknowledges its vblank interrupt.
    let foreign = |&&(b, off, _): &&(Block, u32, u32)| match b {
        Block::Hvs | Block::DispIntr => false,
        Block::Pv(_) => off != crate::regs::PV_INTSTAT,
        _ => true,
    };
    assert!(!regs.writes.iter().any(|w| foreign(&w)), "{:?}", regs.writes.iter().find(|w| foreign(w)));
    assert!(regs.writes.iter().any(|&(b, _, v)| b == Block::Hvs && v == 0x5000_0000),
        "no dlist word holds the new address");
    assert_eq!(vc4.kms.planes[0].base.fb.map(|f| f.dma_addr[0]), Some(0x5000_0000));
}
