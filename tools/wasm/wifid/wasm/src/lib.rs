//! wifid.wasm — the WiFi connection manager.
//!
//! Vendor-independent: it drives any vendor WiFi driver over the
//! kernel-mediated WiFi-class control channel (docs/spec/WIFI_CLASS_ABI.md).
//! It knows the saved networks (vault, system tier, read through the kernel),
//! has the driver scan, picks a network, connects, and runs the EAPOL 4-way
//! and group-key handshakes with the PMK of that network. Users ask through
//! the kernel (`wlan` intent, deck); wifid publishes its state back.
//!
//! No heap: networks, scan list and buffers are fixed arrays.

#![no_std]

use wifid_core::eapol::{Step, Supplicant};
use wifid_core::wpa2_pmk;

// Capabilities: NETCTL (0x80, the control channel and the known networks) +
// READ (0x01, sys/config/wifi) + WRITE (0x02, the log and sys/config/wifid).
#[unsafe(link_section = ".npk.caps")]
#[used]
static NPK_CAPS: [u8; 1] = [0x83];

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    log("[wifid] panic");
    core::arch::wasm32::unreachable()
}

use npk_sys::cell::Single;

const LOG_CAP: usize = 8192;

struct Log {
    buf: [u8; LOG_CAP],
    len: usize,
    /// Bytes already persisted. `log` only appends to the buffer;
    /// `log_flush` writes it out, and the main loop calls that once per
    /// poll round.
    flushed: usize,
}

static LOG: Single<Log> = Single::new(Log { buf: [0; LOG_CAP], len: 0, flushed: 0 });

// A decimal number into the log, without allocating.
fn log_num(mut v: u32) {
    let mut b = [0u8; 10];
    let mut i = 10;
    if v == 0 {
        i -= 1;
        b[i] = b'0';
    }
    while v > 0 {
        i -= 1;
        b[i] = b'0' + (v % 10) as u8;
        v /= 10;
    }
    log(core::str::from_utf8(&b[i..]).unwrap_or("?"));
}

// To the boot log and the buffer for npkFS `sys/log/wifid`. Persisting happens
// in `log_flush`, once per poll round: one store per line would be one full
// npkFS commit each. Network names are never logged: the log is a file any
// module with READ can fetch.
fn log(s: &str) {
    npk_sys::print(s.as_bytes());
    LOG.with(|g| {
        for &c in s.as_bytes() {
            if g.len < LOG_CAP {
                g.buf[g.len] = c;
                g.len += 1;
            }
        }
    });
}

fn log_flush() {
    LOG.with(|g| {
        if g.len == g.flushed {
            return;
        }
        g.flushed = g.len;
        npk_sys::store(b"sys/log/wifid", &g.buf[..g.len]);
    });
}

// ── control-channel wire format (WIFI_CLASS_ABI §4, WLAN_MANAGER §3) ──
// downlink (manager → driver)
const CMD_SCAN: u8 = 0x01;
const CMD_CONNECT: u8 = 0x02;
const CMD_DISCONNECT: u8 = 0x03;
const CMD_SET_KEY: u8 = 0x04;
const CMD_TX_EAPOL: u8 = 0x05;
const CMD_AUTHORIZED: u8 = 0x08;
// uplink (driver → manager)
const EV_READY: u8 = 0x83;
const EV_EAPOL_RX: u8 = 0x84;
const EV_LINK_UP: u8 = 0x85;
const EV_LINK_DOWN: u8 = 0x86;
const EV_CONNECT_FAILED: u8 = 0x87;
const EV_SCAN_RESULT: u8 = 0x89;
/// The driver takes commands now (sent again after a driver restart).
const EV_DRIVER_UP: u8 = 0x8a;
/// The smoothed level of our cell: [op][dBm i8].
const EV_SIGNAL: u8 = 0x8b;

// The panel's item: icons by `IconId`, the
// `link` tool opens on a click.
const ICON_WIFI_HIGH: i32 = 55;
const ICON_WIFI_MEDIUM: i32 = 56;
const ICON_WIFI_LOW: i32 = 57;
const ICON_WIFI_NONE: i32 = 58;
const ICON_WIFI_SLASH: i32 = 59;
const ITEM_NORMAL: i32 = 0;
const ITEM_OFF: i32 = 2;
// user requests (kernel → manager), WLAN_MANAGER §4
const REQ_SCAN: u8 = 0x01;
const REQ_CONNECT: u8 = 0x02;
const REQ_DISCONNECT: u8 = 0x03;
const REQ_FORGET: u8 = 0x04;
const REQ_KNOWN_CHANGED: u8 = 0x05;

const LINK_DOWN_REQUESTED: u8 = 0;

// `security` of a scanned AP.
const SEC_WPA2_PSK: u8 = 2;
const SEC_WPA2_WPA3: u8 = 4;

// Our RSN element (WPA2-PSK-CCMP) — must match the one the driver put in the
// assoc request, since it is echoed in 4-way msg2's key_data and the AP
// compares the two.
//
// The driver carries the same value as `RSN_IE_WPA2_CCMP_PSK`
// (wifi_rtl8822ce/src/lib.rs); `framecheck.py` checks the two byte for
// byte. Do not edit one without the other.
const RSN_IE: [u8; 22] = [
    0x30, 0x14, 0x01, 0x00, 0x00, 0x0f, 0xac, 0x04, 0x01, 0x00, 0x00, 0x0f, 0xac, 0x04, 0x01, 0x00,
    0x00, 0x0f, 0xac, 0x02, 0x00, 0x00,
];

const WAIT_WIFI_EVENT: i32 = 16;
/// A connect that neither links nor fails within this is treated as failed.
const CONNECT_TIMEOUT_MS: i64 = 25_000;
/// Handshakes that end in a drop before the link came up, in a row, before
/// the password is taken to be wrong. One can be bad luck; three are not.
const MAX_AUTH_FAILURES: u32 = 3;
/// `band: auto` takes 5 GHz from this level on (the driver's rule).
const PREFER_5G_DBM: i8 = -70;

const SSID_MAX: usize = 32;
const PSK_MAX: usize = 63;
const MAX_KNOWN: usize = 32;
const MAX_APS: usize = 48;

#[derive(Clone, Copy)]
struct Ssid {
    b: [u8; SSID_MAX],
    n: u8,
}

impl Ssid {
    const EMPTY: Ssid = Ssid { b: [0; SSID_MAX], n: 0 };
    fn from(s: &[u8]) -> Option<Ssid> {
        if s.is_empty() || s.len() > SSID_MAX {
            return None;
        }
        let mut x = Ssid::EMPTY;
        x.b[..s.len()].copy_from_slice(s);
        x.n = s.len() as u8;
        Some(x)
    }
    fn get(&self) -> &[u8] {
        &self.b[..self.n as usize]
    }
}

impl PartialEq for Ssid {
    fn eq(&self, o: &Ssid) -> bool {
        self.get() == o.get()
    }
}

#[derive(Clone, Copy)]
struct Known {
    ssid: Ssid,
    auto: bool,
    prio: i8,
    /// 0 = default from sys/config/wifi, 1 = 2.4 GHz only, 2 = 5 GHz only.
    band: u8,
    psk: [u8; PSK_MAX],
    psk_len: u8,
}

#[derive(Clone, Copy)]
struct Ap {
    bssid: [u8; 6],
    rssi: i8,
    channel: u8,
    security: u8,
    ssid: Ssid,
}

#[derive(Clone, Copy, PartialEq)]
enum State {
    Idle,
    Scanning,
    Connecting,
    Connected,
    Reconnecting,
    /// The user disconnected: no automatic connect until they ask again.
    Disconnected,
}

impl State {
    fn name(self) -> &'static str {
        match self {
            State::Idle => "idle",
            State::Scanning => "scanning",
            State::Connecting => "connecting",
            State::Connected => "connected",
            State::Reconnecting => "reconnecting",
            State::Disconnected => "disconnected",
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum BandPref {
    Auto,
    Only24,
    Only5,
}

struct Mgr {
    known: [Known; MAX_KNOWN],
    n_known: usize,
    aps: [Ap; MAX_APS],
    n_aps: usize,
    have_scan: bool,
    state: State,
    /// The network being joined or joined.
    target: Ssid,
    /// The network the link last came up on; its DHCP lease is the
    /// kernel's current one.
    linked: Ssid,
    bssid: [u8; 6],
    channel: u8,
    /// The level of the joined cell in dBm (scan, then the driver's
    /// `SIGNAL`); 0 = unknown.
    rssi: i8,
    pmk: Option<[u8; 32]>,
    sup: Option<Supplicant>,
    /// The connect came from `sys/config/wifid` without a scan; a failure
    /// falls back to scanning.
    fast: bool,
    /// After the next scan, join this network (a user request).
    pending: Option<Ssid>,
    /// A scan is out (state may stay Connected while it runs).
    scanning: bool,
    deadline: i64,
    auth_failures: u32,
    error: &'static str,
    /// The driver said it takes commands.
    driver_up: bool,
    /// The panel item last published: (icon, flags, bars).
    item: core::cell::Cell<(i32, i32, i32)>,
    default_band: BandPref,
    out: [u8; 256],
}

fn now_ms() -> i64 {
    npk_sys::ticks()
}

fn send_cmd(msg: &[u8]) {
    if npk_sys::wifi_send_cmd(msg) != 0 {
        log("[wifid] command not taken by the channel (full?)\n");
    }
}

// TX_EAPOL: [op][len u16][frame].
fn send_tx_eapol(frame: &[u8]) {
    let mut buf = [0u8; 259];
    buf[0] = CMD_TX_EAPOL;
    buf[1] = (frame.len() & 0xff) as u8;
    buf[2] = (frame.len() >> 8) as u8;
    buf[3..3 + frame.len()].copy_from_slice(frame);
    send_cmd(&buf[..3 + frame.len()]);
}

// SET_KEY: [op][key_type][key_idx][cipher=4 CCMP][key_len][key..][rsc 6].
fn send_set_key(group: bool, key_idx: u8, key: &[u8], rsc: &[u8; 6]) {
    let mut buf = [0u8; 64];
    buf[0] = CMD_SET_KEY;
    buf[1] = if group { 1 } else { 0 };
    buf[2] = key_idx;
    buf[3] = 4; // CCMP
    buf[4] = key.len() as u8;
    buf[5..5 + key.len()].copy_from_slice(key);
    buf[5 + key.len()..5 + key.len() + 6].copy_from_slice(rsc);
    send_cmd(&buf[..5 + key.len() + 6]);
}

impl Mgr {
    fn new() -> Mgr {
        Mgr {
            known: [Known { ssid: Ssid::EMPTY, auto: false, prio: 0, band: 0, psk: [0; PSK_MAX], psk_len: 0 }; MAX_KNOWN],
            n_known: 0,
            aps: [Ap { bssid: [0; 6], rssi: -128, channel: 0, security: 0, ssid: Ssid::EMPTY }; MAX_APS],
            n_aps: 0,
            have_scan: false,
            state: State::Idle,
            target: Ssid::EMPTY,
            linked: Ssid::EMPTY,
            bssid: [0; 6],
            channel: 0,
            rssi: 0,
            pmk: None,
            sup: None,
            fast: false,
            pending: None,
            scanning: false,
            deadline: 0,
            auth_failures: 0,
            error: "",
            driver_up: false,
            item: core::cell::Cell::new((0, 0, i32::MIN)),
            default_band: BandPref::Auto,
            out: [0; 256],
        }
    }

    /// Re-read the known networks: `[ssid_len][ssid][rec_len][record]`.
    fn load_known(&mut self) {
        let mut buf = [0u8; 4096];
        let n = npk_sys::wifi_known(&mut buf);
        // Wipe the old passwords first; the list may have shrunk.
        for k in self.known.iter_mut() {
            k.psk = [0; PSK_MAX];
        }
        self.n_known = 0;
        if n < 0 {
            log("[wifid] the known networks could not be read\n");
            return;
        }
        let b = &buf[..n as usize];
        let mut i = 0;
        while i < b.len() && self.n_known < MAX_KNOWN {
            let sl = b[i] as usize;
            let Some(ssid) = b.get(i + 1..i + 1 + sl).and_then(Ssid::from) else { break };
            i += 1 + sl;
            let Some(&rl) = b.get(i) else { break };
            let Some(rec) = b.get(i + 1..i + 1 + rl as usize) else { break };
            i += 1 + rl as usize;
            // v1: [0x01][flags][prio][band][psk_len][psk]
            if rec.len() < 5 || rec[0] != 0x01 {
                continue;
            }
            let pl = rec[4] as usize;
            let Some(psk) = rec.get(5..5 + pl) else { continue };
            if pl > PSK_MAX {
                continue;
            }
            let k = &mut self.known[self.n_known];
            k.ssid = ssid;
            k.auto = rec[1] & 1 != 0;
            k.prio = rec[2] as i8;
            k.band = rec[3];
            k.psk[..pl].copy_from_slice(psk);
            k.psk_len = pl as u8;
            self.n_known += 1;
        }
        buf.fill(0);
        log("[wifid] known networks: ");
        log_num(self.n_known as u32);
        log("\n");
    }

    fn known(&self, s: &Ssid) -> Option<&Known> {
        self.known[..self.n_known].iter().find(|k| k.ssid == *s)
    }

    fn read_default_band(&mut self) {
        let mut buf = [0u8; 512];
        self.default_band = match read_cfg(b"sys/config/wifi", &mut buf).and_then(|c| cfg_get(c, b"band")) {
            Some(b"5") => BandPref::Only5,
            Some(b"2.4") | Some(b"2,4") => BandPref::Only24,
            _ => BandPref::Auto,
        };
    }

    /// The cell of `ssid` to join. A network's own band (`band` 1 or 2) is
    /// a filter; the default from sys/config/wifi only a preference — a
    /// network on the other band alone is still joined (a phone hotspot on
    /// 2.4 GHz with `band: 5`).
    fn pick_cell(&self, ssid: &Ssid, band: u8) -> Option<Ap> {
        let (pref, strict) = match band {
            1 => (BandPref::Only24, true),
            2 => (BandPref::Only5, true),
            _ => (self.default_band, false),
        };
        let mut best_all: Option<Ap> = None;
        let mut best_5g: Option<Ap> = None;
        let mut best_24: Option<Ap> = None;
        for a in self.aps[..self.n_aps].iter().filter(|a| a.ssid == *ssid) {
            if best_all.is_none_or(|b| a.rssi > b.rssi) {
                best_all = Some(*a);
            }
            let slot = if a.channel > 14 { &mut best_5g } else { &mut best_24 };
            if slot.is_none_or(|b| a.rssi > b.rssi) {
                *slot = Some(*a);
            }
        }
        match pref {
            BandPref::Only5 if strict => best_5g,
            BandPref::Only24 if strict => best_24,
            BandPref::Only5 => best_5g.or(best_all),
            BandPref::Only24 => best_24.or(best_all),
            BandPref::Auto => match best_5g {
                Some(f) if f.rssi >= PREFER_5G_DBM => Some(f),
                _ => best_all,
            },
        }
    }

    /// The best known network in range that may join on its own: higher
    /// priority first, then the stronger signal.
    fn pick_auto(&self) -> Option<Ssid> {
        let mut best: Option<(i8, i8, Ssid)> = None;
        for k in self.known[..self.n_known].iter().filter(|k| k.auto) {
            let Some(c) = self.pick_cell(&k.ssid, k.band) else { continue };
            if !matches!(c.security, SEC_WPA2_PSK | SEC_WPA2_WPA3) {
                continue;
            }
            if best.is_none_or(|(p, r, _)| (k.prio, c.rssi) > (p, r)) {
                best = Some((k.prio, c.rssi, k.ssid));
            }
        }
        best.map(|(_, _, s)| s)
    }

    fn start_scan(&mut self) {
        self.scanning = true;
        if !matches!(self.state, State::Connected | State::Connecting | State::Reconnecting) {
            self.state = State::Scanning;
        }
        send_cmd(&[CMD_SCAN]);
    }

    /// Join `ssid` on cell `cell`.
    fn connect(&mut self, ssid: Ssid, bssid: [u8; 6], channel: u8, fast: bool) -> bool {
        let Some(k) = self.known(&ssid).copied() else {
            self.error = "unknown network";
            return false;
        };
        // PBKDF2 is the slow part (4096 rounds); a reconnect to the same
        // network keeps it.
        if self.target != ssid || self.pmk.is_none() {
            self.pmk = Some(wpa2_pmk(&k.psk[..k.psk_len as usize], ssid.get()));
        }
        self.target = ssid;
        self.bssid = bssid;
        self.channel = channel;
        self.rssi = 0;
        self.fast = fast;
        self.sup = None;
        self.error = "";
        self.state = State::Connecting;
        self.deadline = now_ms() + CONNECT_TIMEOUT_MS;
        let mut m = [0u8; 8];
        m[0] = CMD_CONNECT;
        m[1..7].copy_from_slice(&bssid);
        m[7] = channel;
        send_cmd(&m);
        log(if fast { "[wifid] connecting to the last network (no scan)\n" } else { "[wifid] connecting\n" });
        true
    }

    fn connect_ssid(&mut self, ssid: Ssid) {
        // `wlan set band` takes effect at the next choice, not the next boot.
        self.read_default_band();
        let band = self.known(&ssid).map_or(0, |k| k.band);
        match self.pick_cell(&ssid, band) {
            Some(c) if matches!(c.security, SEC_WPA2_PSK | SEC_WPA2_WPA3) => {
                self.auth_failures = 0;
                self.connect(ssid, c.bssid, c.channel, false);
                self.rssi = c.rssi;
            }
            Some(_) => {
                self.error = "this network's security is not supported (WPA2-PSK only)";
                self.idle();
            }
            None if self.aps[..self.n_aps].iter().any(|a| a.ssid == ssid) => {
                self.error = "this network is not on the band set for it";
                self.idle();
            }
            None => {
                self.error = "network not in range";
                self.idle();
            }
        }
    }

    fn idle(&mut self) {
        if self.state != State::Disconnected {
            self.state = State::Idle;
        }
    }

    /// Join the best known network if nothing holds us back.
    fn auto_connect(&mut self) {
        if !matches!(self.state, State::Idle | State::Scanning) {
            return;
        }
        self.read_default_band();
        match self.pick_auto() {
            Some(s) => self.connect_ssid(s),
            None => self.idle(),
        }
    }

    /// The fast path: the last network, straight to its cell.
    fn try_last(&mut self) -> bool {
        let mut buf = [0u8; 256];
        let Some(c) = read_cfg(b"sys/config/wifid", &mut buf) else { return false };
        let (Some(b), Some(ch), Some(s)) = (cfg_get(c, b"bssid"), cfg_get(c, b"channel"), cfg_get(c, b"ssid")) else {
            return false;
        };
        let mut bssid = [0u8; 6];
        let mut raw = [0u8; SSID_MAX];
        if hex_decode(b, &mut bssid) != Some(6) {
            return false;
        }
        let Some(n) = hex_decode(s, &mut raw) else { return false };
        let Some(ssid) = Ssid::from(&raw[..n]) else { return false };
        let Some(channel) = parse_u8(ch) else { return false };
        match self.known(&ssid) {
            Some(k) if k.auto => self.connect(ssid, bssid, channel, true),
            _ => false,
        }
    }

    fn remember_last(&self) {
        let mut buf = [0u8; 160];
        let mut n = 0;
        let mut put = |s: &[u8]| {
            let k = s.len().min(buf.len() - n);
            buf[n..n + k].copy_from_slice(&s[..k]);
            n += k;
        };
        let mut hex = [0u8; 2 * SSID_MAX];
        put(b"bssid: ");
        put(hex_encode(&self.bssid, &mut hex));
        put(b"\nchannel: ");
        let mut d = [0u8; 3];
        put(dec_u8(self.channel, &mut d));
        put(b"\nssid: ");
        put(hex_encode(self.target.get(), &mut hex));
        put(b"\n");
        npk_sys::store(b"sys/config/wifid", &buf[..n]);
    }

    fn on_event(&mut self, ev: &[u8]) {
        match ev.first().copied() {
            Some(EV_DRIVER_UP) => {
                log("[wifid] driver up\n");
                self.driver_up = true;
                self.sup = None;
                self.scanning = false;
                if self.state != State::Disconnected {
                    self.state = State::Idle;
                    self.read_default_band();
                    self.load_known();
                    // Straight to the last network, else look around.
                    if !self.try_last() {
                        self.start_scan();
                    }
                }
            }
            Some(EV_SCAN_RESULT) => self.on_scan(ev),
            Some(EV_SIGNAL) => {
                if let Some(&d) = ev.get(1) {
                    self.rssi = d as i8;
                }
            }
            // READY: [op][ap_mac 6][our_mac 6] → a supplicant for this BSS.
            Some(EV_READY) if ev.len() >= 13 => {
                let Some(pmk) = self.pmk else {
                    log("[wifid] READY without a network chosen — ignored\n");
                    return;
                };
                let mut aa = [0u8; 6];
                let mut sa = [0u8; 6];
                aa.copy_from_slice(&ev[1..7]);
                sa.copy_from_slice(&ev[7..13]);
                // SNonce: 32 real random bytes (802.11i §12.7.6.2). A
                // constant one makes the PTK depend on the ANonce alone.
                let mut snonce = [0u8; 32];
                if npk_sys::random_bytes(&mut snonce) != 32 {
                    // A predictable nonce is worse than no handshake.
                    log("[wifid] no entropy for the SNonce — refusing the handshake\n");
                    self.sup = None;
                    return;
                }
                self.bssid = aa;
                self.sup = Some(Supplicant::new(pmk, aa, sa, snonce, &RSN_IE));
                if self.state != State::Connected {
                    self.deadline = now_ms() + CONNECT_TIMEOUT_MS;
                }
                log("[wifid] READY — supplicant armed for 4-way\n");
            }
            Some(EV_EAPOL_RX) if ev.len() >= 3 => self.on_eapol(ev),
            // LINK_UP: [op][bssid 6][channel u8]
            Some(EV_LINK_UP) => {
                if let Some(b) = ev.get(1..7) {
                    self.bssid.copy_from_slice(b);
                }
                if let Some(&ch) = ev.get(7) {
                    self.channel = ch;
                }
                self.state = State::Connected;
                self.auth_failures = 0;
                self.fast = false;
                self.error = "";
                // Another network than the lease came from: the kernel saw
                // only a short down/up and would keep the old address.
                if self.linked.n > 0 && self.linked != self.target {
                    log("[wifid] another network — asking for a new address\n");
                    npk_sys::wifi_new_network();
                }
                self.linked = self.target;
                self.remember_last();
                log("[wifid] link up — connected\n");
            }
            // LINK_DOWN: [op][reason]. The supplicant holds a PTK for a
            // session that is gone and a used SNonce: drop it; a fresh READY
            // builds a new one.
            Some(EV_LINK_DOWN) => {
                self.sup = None;
                let reason = ev.get(1).copied().unwrap_or(2);
                log("[wifid] link down (");
                log(match reason {
                    0 => "requested",
                    1 => "deauth",
                    _ => "lost",
                });
                log(")\n");
                if reason == LINK_DOWN_REQUESTED {
                    // Connecting: we are on the way to another network.
                    // Disconnected: the user wants it so.
                    if !matches!(self.state, State::Connecting | State::Disconnected) {
                        self.state = State::Idle;
                        self.auto_connect();
                    }
                    return;
                }
                if self.state == State::Connecting || (self.state == State::Reconnecting && self.auth_failures > 0) {
                    // Dropped before the link came up: the handshake failed.
                    self.auth_failures += 1;
                    if self.auth_failures >= MAX_AUTH_FAILURES {
                        log("[wifid] the handshake keeps failing — wrong password?\n");
                        self.error = "the network keeps refusing us - wrong password?";
                        send_cmd(&[CMD_DISCONNECT]);
                        self.state = State::Disconnected;
                        return;
                    }
                }
                // The driver reconnects on its own (same network).
                self.state = State::Reconnecting;
                self.deadline = now_ms() + CONNECT_TIMEOUT_MS;
            }
            // CONNECT_FAILED: [op][reason]
            Some(EV_CONNECT_FAILED) => {
                log("[wifid] connect failed\n");
                self.sup = None;
                if self.fast {
                    self.fast = false;
                    self.state = State::Idle;
                    self.start_scan();
                } else {
                    self.error = match ev.get(1) {
                        Some(1) => "the network did not answer",
                        _ => "the network refused the association",
                    };
                    self.state = State::Idle;
                }
            }
            _ => {}
        }
    }

    /// SCAN_RESULT: [op][count] then per AP bssid[6] rssi ch sec ssid_len ssid.
    fn on_scan(&mut self, ev: &[u8]) {
        self.scanning = false;
        self.have_scan = true;
        self.n_aps = 0;
        let count = ev.get(1).copied().unwrap_or(0) as usize;
        let mut i = 2;
        for _ in 0..count {
            let Some(h) = ev.get(i..i + 10) else { break };
            let sl = h[9] as usize;
            let Some(s) = ev.get(i + 10..i + 10 + sl) else { break };
            i += 10 + sl;
            let Some(ssid) = Ssid::from(s) else { continue }; // hidden
            if self.n_aps == MAX_APS {
                break;
            }
            let a = &mut self.aps[self.n_aps];
            a.bssid.copy_from_slice(&h[0..6]);
            a.rssi = h[6] as i8;
            a.channel = h[7];
            a.security = h[8];
            a.ssid = ssid;
            self.n_aps += 1;
        }
        log("[wifid] scan: ");
        log_num(self.n_aps as u32);
        log(" cells\n");
        if let Some(s) = self.pending.take() {
            self.state = if self.state == State::Disconnected { State::Idle } else { self.state };
            if self.state == State::Connected && self.target == s {
                return;
            }
            self.connect_ssid(s);
            return;
        }
        if self.state == State::Scanning {
            self.state = State::Idle;
        }
        self.auto_connect();
    }

    fn on_eapol(&mut self, ev: &[u8]) {
        let n = ((ev[2] as usize) << 8) | ev[1] as usize;
        if ev.len() < 3 + n {
            return;
        }
        let frame = &ev[3..3 + n];
        let Some(s) = self.sup.as_mut() else {
            log("[wifid] EAPOL_RX but no supplicant (missed READY)\n");
            return;
        };
        let out = &mut self.out;
        match s.on_eapol(frame, out) {
            Step::Reply(m) => {
                log("[wifid] 4-way: sending msg2\n");
                send_tx_eapol(&out[..m]);
            }
            Step::Done(m) => {
                log("[wifid] 4-way: msg3 OK — sending msg4 + installing keys\n");
                send_tx_eapol(&out[..m]);
                if let Some(ptk) = s.ptk() {
                    send_set_key(false, 0, &ptk.tk, &[0u8; 6]);
                }
                if let Some((gtk, id)) = s.gtk() {
                    send_set_key(true, id, gtk, &s.gtk_rsc());
                }
                send_cmd(&[CMD_AUTHORIZED]);
                log("[wifid] *** 4-way complete — AUTHORIZED ***\n");
            }
            Step::Rekey(m) => {
                // Answer first, install second: the AP starts using the new
                // group key as soon as it has our acknowledgement.
                log("[wifid] group rekey — new GTK, acknowledging\n");
                send_tx_eapol(&out[..m]);
                if let Some((gtk, id)) = s.gtk() {
                    send_set_key(true, id, gtk, &s.gtk_rsc());
                }
            }
            Step::ReplyOnly(m) => {
                log("[wifid] repeated handshake message — answered, keys kept\n");
                send_tx_eapol(&out[..m]);
            }
            Step::Fail => log("[wifid] 4-way FAILED (bad MIC / unwrap)\n"),
            // Three hardening rules (replay, key version, length) end here;
            // each would otherwise look like "nothing happened".
            Step::Ignore => {
                if s.replays_dropped > 0 || s.bad_key_version > 0 || s.too_long > 0 {
                    log("[wifid] EAPOL dropped: replay ");
                    log_num(s.replays_dropped);
                    log(", key version ");
                    log_num(s.bad_key_version);
                    log(", too long ");
                    log_num(s.too_long);
                    log(" (repeats ");
                    log_num(s.replays_repeated);
                    log(")\n");
                }
            }
        }
    }

    fn on_request(&mut self, r: &[u8]) {
        let ssid = || r.get(1).and_then(|&n| r.get(2..2 + n as usize)).and_then(Ssid::from);
        match r.first().copied() {
            Some(REQ_SCAN) if self.driver_up => self.start_scan(),
            Some(REQ_CONNECT) => {
                let Some(s) = ssid() else { return };
                if self.state == State::Connected && self.target == s {
                    return;
                }
                // The password may be new.
                self.load_known();
                self.pmk = None;
                self.error = "";
                self.auth_failures = 0;
                if self.state == State::Disconnected {
                    self.state = State::Idle;
                }
                if self.have_scan && self.pick_cell(&s, self.known(&s).map_or(0, |k| k.band)).is_some() {
                    self.connect_ssid(s);
                } else {
                    self.pending = Some(s);
                    self.start_scan();
                }
            }
            Some(REQ_DISCONNECT) => {
                self.pending = None;
                self.state = State::Disconnected;
                self.sup = None;
                send_cmd(&[CMD_DISCONNECT]);
            }
            Some(REQ_FORGET) => {
                let gone = ssid();
                self.load_known();
                if gone.is_some_and(|s| s == self.target)
                    && matches!(self.state, State::Connected | State::Connecting | State::Reconnecting)
                {
                    self.pmk = None;
                    send_cmd(&[CMD_DISCONNECT]);
                    self.state = State::Idle;
                }
            }
            Some(REQ_KNOWN_CHANGED) => {
                self.load_known();
                self.auto_connect();
            }
            _ => {}
        }
    }

    fn on_timeout(&mut self) {
        if !matches!(self.state, State::Connecting | State::Reconnecting) || now_ms() < self.deadline {
            return;
        }
        log("[wifid] connect timed out\n");
        self.sup = None;
        if self.fast {
            self.fast = false;
            self.state = State::Idle;
            self.start_scan();
        } else if self.state == State::Connecting {
            self.error = "the network did not answer in time";
            send_cmd(&[CMD_DISCONNECT]);
            self.state = State::Idle;
        } else {
            // Reconnecting: the driver keeps trying; just stop counting.
            self.deadline = now_ms() + CONNECT_TIMEOUT_MS;
        }
    }

    /// The state text (`wlan` and the deck read it).
    fn publish(&self) {
        let mut buf = [0u8; 4096];
        let mut w = Writer { b: &mut buf, n: 0 };
        w.s(b"state ");
        w.s(if self.driver_up { self.state.name().as_bytes() } else { b"no-driver" });
        if self.scanning {
            w.s(b" (scanning)");
        }
        w.s(b"\n");
        if self.target.n > 0 && self.state != State::Idle {
            w.s(b"ssid ");
            w.esc(self.target.get());
            w.s(b"\nchannel ");
            let mut d = [0u8; 3];
            w.s(dec_u8(self.channel, &mut d));
            w.s(b"\n");
            if self.rssi != 0 {
                w.s(b"signal ");
                w.i8(self.rssi);
                w.s(b"\n");
            }
            let sec = self.aps[..self.n_aps].iter().find(|a| a.ssid == self.target).map(|a| a.security);
            if let Some(sec) = sec {
                w.s(b"security ");
                w.s(dec_u8(sec, &mut d));
                w.s(b"\n");
            }
        }
        if !self.error.is_empty() {
            w.s(b"error ");
            w.s(self.error.as_bytes());
            w.s(b"\n");
        }
        // Networks in range, one line each (best cell), strongest first.
        let mut shown = [false; MAX_APS];
        loop {
            let mut best: Option<usize> = None;
            for i in 0..self.n_aps {
                if shown[i] {
                    continue;
                }
                if best.is_none_or(|b| self.aps[i].rssi > self.aps[b].rssi) {
                    best = Some(i);
                }
            }
            let Some(i) = best else { break };
            let a = self.aps[i];
            for j in 0..self.n_aps {
                if self.aps[j].ssid == a.ssid {
                    shown[j] = true;
                }
            }
            w.s(b"ap ");
            w.i8(a.rssi);
            w.s(b" ");
            let mut d = [0u8; 3];
            w.s(dec_u8(a.channel, &mut d));
            w.s(b" ");
            w.s(dec_u8(a.security, &mut d));
            w.s(if self.known(&a.ssid).is_some() { b" 1 " } else { b" 0 " });
            w.esc(a.ssid.get());
            w.s(b"\n");
        }
        for k in &self.known[..self.n_known] {
            w.s(b"known ");
            w.s(if k.auto { b"1 " } else { b"0 " });
            w.i8(k.prio);
            w.s(b" ");
            w.esc(k.ssid.get());
            w.s(b"\n");
        }
        let n = w.n;
        npk_sys::wifi_publish(&buf[..n]);
        self.publish_item();
    }

    /// The panel's WLAN icon: bars by signal while connected, dimmed while
    /// on the way, struck through without a network. Published only on a
    /// change, so the panel is not woken for nothing.
    fn publish_item(&self) {
        let (icon, flags, bars) = match self.state {
            State::Connected => {
                let (icon, bars) = match self.rssi {
                    0 => (ICON_WIFI_HIGH, -1),
                    r if r >= -60 => (ICON_WIFI_HIGH, 3),
                    r if r >= -70 => (ICON_WIFI_MEDIUM, 2),
                    r if r >= -80 => (ICON_WIFI_LOW, 1),
                    _ => (ICON_WIFI_NONE, 0),
                };
                (icon, ITEM_NORMAL, bars)
            }
            State::Connecting | State::Reconnecting => (ICON_WIFI_NONE, ITEM_OFF, -1),
            _ => (ICON_WIFI_SLASH, ITEM_OFF, -1),
        };
        if self.item.get() == (icon, flags, bars) {
            return;
        }
        self.item.set((icon, flags, bars));
        // Icon only, as panels do: the network's name is in the `link` card.
        npk_sys::status_set(b"wlan", icon, bars, flags, b"", b"link");
    }
}

struct Writer<'a> {
    b: &'a mut [u8],
    n: usize,
}

impl Writer<'_> {
    fn s(&mut self, s: &[u8]) {
        let k = s.len().min(self.b.len() - self.n);
        self.b[self.n..self.n + k].copy_from_slice(&s[..k]);
        self.n += k;
    }
    fn i8(&mut self, v: i8) {
        if v < 0 {
            self.s(b"-");
        }
        let mut d = [0u8; 3];
        self.s(dec_u8(v.unsigned_abs(), &mut d));
    }
    /// An SSID is raw bytes: printable ASCII stays, anything else and `\`
    /// become `\xNN`, so one line holds one name.
    fn esc(&mut self, ssid: &[u8]) {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        for &c in ssid {
            if (0x20..0x7f).contains(&c) && c != b'\\' {
                self.s(&[c]);
            } else {
                self.s(&[b'\\', b'x', HEX[(c >> 4) as usize], HEX[(c & 15) as usize]]);
            }
        }
    }
}

fn dec_u8(v: u8, out: &mut [u8; 3]) -> &[u8] {
    let mut i = 3;
    let mut v = v;
    loop {
        i -= 1;
        out[i] = b'0' + v % 10;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    &out[i..]
}

fn parse_u8(s: &[u8]) -> Option<u8> {
    let mut v: u32 = 0;
    if s.is_empty() {
        return None;
    }
    for &c in s {
        if !c.is_ascii_digit() {
            return None;
        }
        v = v * 10 + (c - b'0') as u32;
        if v > 255 {
            return None;
        }
    }
    Some(v as u8)
}

fn hex_encode<'a>(b: &[u8], out: &'a mut [u8]) -> &'a [u8] {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let n = b.len().min(out.len() / 2);
    for (i, &x) in b[..n].iter().enumerate() {
        out[2 * i] = HEX[(x >> 4) as usize];
        out[2 * i + 1] = HEX[(x & 15) as usize];
    }
    &out[..2 * n]
}

fn hex_decode(s: &[u8], out: &mut [u8]) -> Option<usize> {
    if s.len() % 2 != 0 || s.len() / 2 > out.len() {
        return None;
    }
    let nib = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    };
    for i in 0..s.len() / 2 {
        out[i] = nib(s[2 * i])? << 4 | nib(s[2 * i + 1])?;
    }
    Some(s.len() / 2)
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    log("[wifid] WiFi manager start\n");
    let mut m = Mgr::new();
    m.load_known();
    m.publish();
    log_flush();

    let mut buf = [0u8; 2048];
    loop {
        let mut changed = false;
        loop {
            let n = npk_sys::wifi_poll_request(&mut buf);
            if n <= 0 {
                break;
            }
            let r = buf;
            m.on_request(&r[..n as usize]);
            changed = true;
        }
        loop {
            let n = npk_sys::wifi_poll_event(&mut buf);
            if n <= 0 {
                break;
            }
            let ev = buf;
            m.on_event(&ev[..n as usize]);
            changed = true;
        }
        let before = m.state;
        m.on_timeout();
        if changed || m.state != before {
            m.publish();
        }
        log_flush();
        // Woken by a driver event or a user request; a running connect also
        // needs its deadline.
        let timeout = if matches!(m.state, State::Connecting | State::Reconnecting) {
            (m.deadline - now_ms()).clamp(10, CONNECT_TIMEOUT_MS) as i32
        } else {
            -1
        };
        npk_sys::wait(WAIT_WIFI_EVENT, timeout);
    }
}

fn cfg_trim(v: &[u8]) -> &[u8] {
    let (mut a, mut b) = (0, v.len());
    while a < b && matches!(v[a], b' ' | b'\t' | b'\r') {
        a += 1;
    }
    while b > a && matches!(v[b - 1], b' ' | b'\t' | b'\r') {
        b -= 1;
    }
    &v[a..b]
}

/// Value of `key` in a `key: value` config, or None. `#` starts a comment;
/// only the first colon splits.
fn cfg_get<'a>(text: &'a [u8], key: &[u8]) -> Option<&'a [u8]> {
    for line in text.split(|&b| b == b'\n') {
        let line = cfg_trim(line);
        if line.is_empty() || line[0] == b'#' {
            continue;
        }
        let Some(c) = line.iter().position(|&b| b == b':') else { continue };
        if cfg_trim(&line[..c]) == key {
            return Some(cfg_trim(&line[c + 1..]));
        }
    }
    None
}

fn read_cfg<'a>(name: &[u8], buf: &'a mut [u8]) -> Option<&'a [u8]> {
    let n = npk_sys::fetch(name, buf);
    if n <= 0 {
        return None;
    }
    Some(&buf[..n as usize])
}
