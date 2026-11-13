//! drm_core against Linux: the EDID of the pi5 oracle's run G
//! (tools/wasm/gpu_bcm2712/core/testdata), parsed and turned into modes,
//! must give the `# kms:` lines Linux printed for HDMI-A-1.

use crate::drm_connector::drm_connector;
use crate::drm_edid::{drm_edid_alloc, drm_edid_connector_add_modes, drm_edid_connector_update};
use crate::drm_probe_helper::{drm_helper_probe_single_connector_modes, drm_probe_funcs};
use crate::kms_dump::connector_lines;
use crate::regs::DRM_MODE_CONNECTOR_HDMIA;
use alloc::string::String;
use alloc::vec::Vec;

const LOG: &str = include_str!("../../gpu_bcm2712/core/testdata/linux_vc4_d0.log");

/// The EDID the model answered on DDC 0: the reads at 0x50 after the
/// one-byte probe.
fn run_g_edid() -> Vec<u8> {
    let bytes: Vec<u8> = LOG
        .lines()
        .filter(|l| l.starts_with("# model: ddc0 read 0x50:"))
        .flat_map(|l| {
            l.split(": ").nth(2).unwrap().split(" [").next().unwrap()
                .split_whitespace().map(|b| u8::from_str_radix(b, 16).unwrap()).collect::<Vec<_>>()
        })
        .collect();
    bytes[1..].to_vec()
}

/// Linux's lines for one connector, up to the next connector or modeset.
fn linux_lines(name: &str) -> Vec<String> {
    let head = alloc::format!("# kms: connector {} ", name);
    let mut out = Vec::new();
    let mut on = false;
    for l in LOG.lines() {
        if l.starts_with("# kms: connector ") || l.starts_with("# kms: modeset ") {
            on = l.starts_with(&head);
        }
        if on {
            out.push(String::from(l));
        }
    }
    out
}

#[test]
fn run_g_edid_gives_linux_modes() {
    let edid = run_g_edid();
    assert_eq!(edid.len(), 256);
    let mut c = drm_connector::new(String::from("HDMI-A-1"), DRM_MODE_CONNECTOR_HDMIA);
    c.rpi_downstream = true;

    // The probe with a driver that reads this EDID and takes every mode,
    // as vc4 did for these.
    let n = drm_helper_probe_single_connector_modes(&mut c, 7680, 7680, &mut Sink(edid));

    let linux = linux_lines("HDMI-A-1");
    let port = connector_lines(&c);
    for (a, b) in linux.iter().zip(port.iter()) {
        assert_eq!(a, b);
    }
    assert_eq!(linux.len(), port.len(), "{} modes added", n);
}

#[test]
fn run_g_unplugged_port_is_linux() {
    // HDMI1 has nothing attached: detect says disconnected, the probe
    // resets the display info (Raspberry Pi's tree: RGB444 stays).
    struct Unplugged;
    impl drm_probe_funcs for Unplugged {
        fn detect(&mut self, _c: &mut drm_connector, _force: bool) -> crate::drm_connector::drm_connector_status {
            crate::drm_connector::drm_connector_status::connector_status_disconnected
        }
        fn get_modes(&mut self, _c: &mut drm_connector) -> i32 {
            unreachable!("no modes on a disconnected connector")
        }
        fn possible_encoders(&self, _c: &drm_connector) -> Vec<(u32, Vec<u32>)> {
            Vec::new()
        }
    }
    let mut c = drm_connector::new(String::from("HDMI-A-2"), DRM_MODE_CONNECTOR_HDMIA);
    c.rpi_downstream = true;
    assert_eq!(drm_helper_probe_single_connector_modes(&mut c, 7680, 7680, &mut Unplugged), 0);
    assert_eq!(connector_lines(&c), linux_lines("HDMI-A-2"));
}

/// Run G's device as the oracle dumped it: the CRTC count and limits,
/// each connector's possible encoders and their CRTC masks.
struct RunG {
    edid: Vec<u8>,
    num_crtc: u32,
    max: (u32, u32),
    encoders: Vec<(String, u32)>,
    possible: Vec<(String, String)>,
}

impl RunG {
    fn from_log() -> Self {
        let mut g = RunG { edid: run_g_edid(), num_crtc: 0, max: (0, 0), encoders: Vec::new(), possible: Vec::new() };
        for l in LOG.lines() {
            let w: Vec<&str> = l.split_whitespace().collect();
            match w.get(2) {
                Some(&"config") => {
                    g.num_crtc = w[4].parse().unwrap();
                    let (x, y) = w[6].split_once('x').unwrap();
                    g.max = (x.parse().unwrap(), y.parse().unwrap());
                }
                Some(&"encoder") => {
                    g.encoders.push((String::from(w[3]), u32::from_str_radix(&w[5][2..], 16).unwrap()))
                }
                Some(&"possible") => g.possible.push((String::from(w[3]), String::from(w[4]))),
                _ => {}
            }
        }
        g
    }
}

impl drm_probe_funcs for RunG {
    fn detect(&mut self, c: &mut drm_connector, _force: bool) -> crate::drm_connector::drm_connector_status {
        use crate::drm_connector::drm_connector_status::*;
        if c.name == "HDMI-A-1" { connector_status_connected } else { connector_status_disconnected }
    }
    fn get_modes(&mut self, c: &mut drm_connector) -> i32 {
        let drm_edid = drm_edid_alloc(&self.edid).expect("edid");
        drm_edid_connector_update(c, Some(&drm_edid));
        drm_edid_connector_add_modes(c)
    }
    fn possible_encoders(&self, c: &drm_connector) -> Vec<(u32, Vec<u32>)> {
        self.possible
            .iter()
            .filter(|(conn, _)| *conn == c.name)
            .map(|(_, enc)| {
                let e = self.encoders.iter().position(|(n, _)| n == enc).unwrap();
                let mask = self.encoders[e].1;
                (e as u32, (0..32).filter(|b| mask & (1 << b) != 0).collect())
            })
            .collect()
    }
}

impl crate::drm_client_modeset::drm_client_funcs for RunG {
    fn num_crtc(&self) -> u32 {
        self.num_crtc
    }
    fn max_width(&self) -> u32 {
        self.max.0
    }
    fn max_height(&self) -> u32 {
        self.max.1
    }
    fn connector_state(&self, _c: &drm_connector) -> (Option<u32>, Option<u32>) {
        // vc4 has taken the firmware's outputs down at bind.
        (None, None)
    }
    fn crtc_state_mode(&self, _crtc: u32) -> crate::drm_modes::drm_display_mode {
        Default::default()
    }
}

#[test]
fn run_g_fbdev_choice_is_linux() {
    use crate::drm_client_modeset::{drm_client_dev, drm_client_modeset_probe};
    use crate::regs::DRM_MODE_CONNECTOR_WRITEBACK;

    let mut g = RunG::from_log();
    assert_eq!(g.num_crtc, 4);
    let mut connectors = alloc::vec![
        drm_connector::new(String::from("HDMI-A-1"), DRM_MODE_CONNECTOR_HDMIA),
        drm_connector::new(String::from("HDMI-A-2"), DRM_MODE_CONNECTOR_HDMIA),
        drm_connector::new(String::from("Writeback-1"), DRM_MODE_CONNECTOR_WRITEBACK),
        drm_connector::new(String::from("Writeback-2"), DRM_MODE_CONNECTOR_WRITEBACK),
    ];
    for c in connectors.iter_mut().take(2) {
        c.rpi_downstream = true;
    }
    let mut client = drm_client_dev::new(g.num_crtc);
    assert_eq!(drm_client_modeset_probe(&mut client, &mut connectors, &mut g, 0, 0), 0);

    let linux: Vec<String> = LOG.lines().filter(|l| l.starts_with("# kms: modeset ") || l.starts_with("# kms: chosen "))
        .map(String::from).collect();
    assert_eq!(crate::kms_dump::modeset_lines(&client, &connectors), linux);
    for name in ["HDMI-A-1", "HDMI-A-2"] {
        let c = connectors.iter().find(|c| c.name == name).unwrap();
        assert_eq!(connector_lines(c), linux_lines(name));
    }
}

#[test]
fn mutated_edids_never_panic() {
    // A monitor's EDID is outside input: run G's EDID with random bytes
    // changed (xorshift, fixed seed), through update, modes and probe.
    let base = run_g_edid();
    let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut next = || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    for _ in 0..4000 {
        let mut e = base.clone();
        let flips = 1 + next() % 24;
        for _ in 0..flips {
            let i = (next() % e.len() as u64) as usize;
            e[i] = next() as u8;
        }
        // Mostly the extension, where the data blocks are.
        if next() % 2 == 0 {
            let i = 128 + (next() % 128) as usize;
            e[i] = next() as u8;
        }
        let mut c = drm_connector::new(String::from("HDMI-A-1"), DRM_MODE_CONNECTOR_HDMIA);
        drm_helper_probe_single_connector_modes(&mut c, 8192, 8192, &mut Sink(e));
    }
}

/// A connected sink with this EDID, every mode valid.
struct Sink(Vec<u8>);

impl drm_probe_funcs for Sink {
    fn get_modes(&mut self, c: &mut drm_connector) -> i32 {
        let Some(drm_edid) = drm_edid_alloc(&self.0) else { return 0 };
        drm_edid_connector_update(c, Some(&drm_edid));
        drm_edid_connector_add_modes(c)
    }
    fn possible_encoders(&self, _c: &drm_connector) -> Vec<(u32, Vec<u32>)> {
        alloc::vec![(0, alloc::vec![0])]
    }
}
