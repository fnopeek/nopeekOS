//! wifid.wasm — the WiFi connection manager (WPA2 supplicant).
//!
//! Vendor-independent: it drives any vendor WiFi driver (e.g. wifi_ax200) over
//! the kernel-mediated WiFi-class control channel (see docs/spec/WIFI_CLASS_ABI.md). It
//! owns the credentials and the WPA2 4-way handshake; the vendor driver only
//! transports frames and installs the keys the supplicant computes.
//!
//! It declares the NETCTL capability, loads the PSK from npkFS, derives the
//! PMK (the std-tested [`wifid_core`] crypto), and runs the EAPOL 4-way and
//! group-key handshakes over the control channel.

#![no_std]

use wifid_core::eapol::{Step, Supplicant};
use wifid_core::wpa2_pmk;

// Capabilities: NETCTL (0x80, the control channel) + READ (0x01, fetch the
// credential) + WRITE (0x02, write the debug log). The kernel grants exactly
// these from the caps byte — NETCTL alone would leave no READ to load the PSK.
#[unsafe(link_section = ".npk.caps")]
#[used]
static NPK_CAPS: [u8; 1] = [0x83];

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    log("[wifid] panic");
    core::arch::wasm32::unreachable()
}

// Host functions are WASM imports from the `env` module, resolved by the
// kernel at instantiation. Naming the module explicitly is what makes them
// imports rather than ordinary undefined C symbols, which rust-lld rejects.
#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn npk_fetch(name_ptr: i32, name_len: i32, buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_wifi_send_cmd(buf_ptr: i32, len: i32) -> i32;
    fn npk_wifi_poll_event(buf_ptr: i32, max: i32) -> i32;
    fn npk_sleep(ms: i32) -> i32;
    fn npk_wait(mask: i32, timeout_ms: i32) -> i32;
    // Terminal/framebuffer output (like the driver) — visible on machines
    // without a serial port, where npk_log_serial shows nothing.
    fn npk_print(ptr: i32, len: i32);
    fn npk_store(name_ptr: i32, name_len: i32, data_ptr: i32, data_len: i32) -> i32;
    /// `security::csprng`. Needs no capability, like `npk_unix_time`. Returns
    /// the number of bytes written or -1.
    fn npk_random_bytes(buf_ptr: i32, len: i32) -> i32;
}

const LOG_CAP: usize = 8192;
static mut LOG_BUF: [u8; LOG_CAP] = [0; LOG_CAP];
static mut LOG_LEN: usize = 0;

/// Bytes already persisted. `log` only appends to the buffer; `log_flush`
/// writes it out, and the main loop calls that once per poll round.
static mut LOG_FLUSHED: usize = 0;

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
    log(unsafe { core::str::from_utf8_unchecked(&b[i..]) });
}

// Log to the terminal and buffer for npkFS `sys/log/wifid` — wifid runs in an
// invisible autostart window, so its log is read back with `fetch /sys/log/wifid`.
// Persisting happens in `log_flush`, once per poll round: one store per line
// would be one full npkFS commit each,
// breaking npkFS's "N puts then one commit_root" assumption mid-download.
fn log(s: &str) {
    unsafe { npk_print(s.as_ptr() as i32, s.len() as i32) };
    unsafe {
        let buf = core::ptr::addr_of_mut!(LOG_BUF) as *mut u8;
        let len_ptr = core::ptr::addr_of_mut!(LOG_LEN);
        let mut l = len_ptr.read();
        for &c in s.as_bytes() {
            if l < LOG_CAP {
                buf.add(l).write(c);
                l += 1;
            }
        }
        len_ptr.write(l);
    }
}

/// Persist the log if it grew. One store per poll round instead of one per
/// line — and none at all while nothing is happening.
fn log_flush() {
    unsafe {
        let l = core::ptr::addr_of_mut!(LOG_LEN).read();
        let flushed_ptr = core::ptr::addr_of_mut!(LOG_FLUSHED);
        if l == flushed_ptr.read() {
            return;
        }
        flushed_ptr.write(l);
        let buf = core::ptr::addr_of_mut!(LOG_BUF) as *mut u8;
        let name = b"sys/log/wifid";
        npk_store(name.as_ptr() as i32, name.len() as i32, buf as i32, l as i32);
    }
}

// ── control-channel wire format (docs/spec/WIFI_CLASS_ABI.md) ──────────────────────
// downlink (manager → driver)
const CMD_SET_KEY: u8 = 0x04;
const CMD_TX_EAPOL: u8 = 0x05;
const CMD_AUTHORIZED: u8 = 0x08;
// uplink (driver → manager)
const EV_READY: u8 = 0x83;
const EV_EAPOL_RX: u8 = 0x84;
const EV_LINK_UP: u8 = 0x85;
const EV_LINK_DOWN: u8 = 0x86;

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

static mut SSID_BUF: [u8; 512] = [0; 512];
static mut PSK_BUF: [u8; 128] = [0; 128];
static mut EVENT_BUF: [u8; 2048] = [0; 2048];

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    log("[wifid] WiFi manager start (WPA2 supplicant)\n");

    // ── Load the credential from npkFS. The network settings live in one
    // `key: value` file shared with the driver; the passphrase keeps its own
    // object, because nothing but this module has business holding it:
    //   store /sys/config/wifi     ssid: My Network
    //   store /sys/config/wifi_psk my secret pass
    // Plaintext in an at-rest-encrypted object; a capability-gated keystore
    // is meant to replace it.
    // Wait for the credential rather than exiting without one: on autostart
    // this races the rest of boot. Without a supplicant the driver associates,
    // sends READY into the void, and the AP deauthenticates us after an
    // unanswered msg1 — which looks like "connected but no DHCP lease".
    let (ssid, pass) = loop {
        let ssid = read_cfg(b"sys/config/wifi", core::ptr::addr_of_mut!(SSID_BUF) as *mut u8, 512)
            .and_then(|c| cfg_get(c, b"ssid"))
            .filter(|s| !s.is_empty());
        let pass = read_cfg(b"sys/config/wifi_psk", core::ptr::addr_of_mut!(PSK_BUF) as *mut u8, 128)
            .filter(|p| p.len() >= 8);
        match (ssid, pass) {
            (Some(s), Some(p)) => break (s, p),
            (None, _) => log("[wifid] waiting for an `ssid:` line in sys/config/wifi\n"),
            (_, None) => log("[wifid] waiting for sys/config/wifi_psk (store /sys/config/wifi_psk <pass>)\n"),
        }
        unsafe { npk_sleep(2000) };
    };
    // Neither the network name nor anything derived from the passphrase is
    // logged: the log is a file any module with READ can fetch.
    log("[wifid] credential loaded\n");

    // ── Derive the PMK (PBKDF2-HMAC-SHA1, 4096 iters) — the std-tested core.
    let pmk = wpa2_pmk(pass, ssid);
    log("[wifid] supplicant resident — waiting for the driver to associate\n");

    // ── Resident supplicant loop. Runs on a worker core via autostart; drains
    // control-channel events and drives the 4-way handshake to completion. ──
    let ev_ptr = core::ptr::addr_of_mut!(EVENT_BUF) as *mut u8;
    let mut sup: Option<Supplicant> = None;
    let mut out = [0u8; 256];
    loop {
        loop {
            let len = unsafe { npk_wifi_poll_event(ev_ptr as i32, 2048) };
            if len <= 0 {
                break;
            }
            let ev = unsafe { core::slice::from_raw_parts(ev_ptr as *const u8, len as usize) };
            handle_event(ev, &pmk, &mut sup, &mut out);
        }
        // One store per round, not one per line.
        log_flush();
        // Wait for the driver's next event instead of polling for it. The
        // kernel wakes us when the driver queues an event
        // (`WAIT_WIFI_EVENT`), so a 4-way message is answered at once. The
        // supplicant has no timers of its own: no deadline.
        // docs/plan/CORES_AND_EVENTS.md.
        const WAIT_WIFI_EVENT: i32 = 16;
        unsafe { npk_wait(WAIT_WIFI_EVENT, -1) };
    }
}

fn handle_event(ev: &[u8], pmk: &[u8; 32], sup: &mut Option<Supplicant>, out: &mut [u8]) {
    match ev.first().copied() {
        // READY: [op][ap_mac 6][our_mac 6] → build the supplicant for this BSS.
        Some(EV_READY) if ev.len() >= 13 => {
            let mut aa = [0u8; 6];
            let mut sa = [0u8; 6];
            aa.copy_from_slice(&ev[1..7]);
            sa.copy_from_slice(&ev[7..13]);
            // SNonce: 32 real random bytes (802.11i §12.7.6.2).
            //
            // A constant SNonce makes the PTK depend on the ANonce alone —
            // an AP that repeats an ANonce (some do after a reboot) gets the
            // same PTK while our packet number restarts at 1, and then drops
            // our frames as replays.
            let mut snonce = [0u8; 32];
            let got = unsafe { npk_random_bytes(snonce.as_mut_ptr() as i32, 32) };
            if got != 32 {
                // No fallback to a made-up value. A predictable nonce is worse
                // than no handshake: it looks like one.
                log("[wifid] no entropy for the SNonce — refusing the handshake\n");
                *sup = None;
                return;
            }
            *sup = Some(Supplicant::new(*pmk, aa, sa, snonce, &RSN_IE));
            log("[wifid] READY — supplicant armed for 4-way (random SNonce)\n");
        }
        // EAPOL_RX: [op][len u16][frame] → feed the 4-way state machine.
        Some(EV_EAPOL_RX) if ev.len() >= 3 => {
            let n = ((ev[2] as usize) << 8) | ev[1] as usize;
            if ev.len() < 3 + n {
                return;
            }
            let frame = &ev[3..3 + n];
            let s = match sup {
                Some(s) => s,
                None => {
                    log("[wifid] EAPOL_RX but no supplicant (missed READY)\n");
                    return;
                }
            };
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
                    // A repeat of a message whose key is installed: answer it,
                    // reinstall nothing.
                    log("[wifid] repeated handshake message — answered, keys kept\n");
                    send_tx_eapol(&out[..m]);
                }
                Step::Fail => log("[wifid] 4-way FAILED (bad MIC / unwrap)\n"),
                // `Ignore` is not always harmless: three hardening rules
                // (replay, key version, length) end here, and each would
                // otherwise look like "nothing happened". Log the counters.
                Step::Ignore => {
                    if s.replays_dropped > 0 || s.bad_key_version > 0
                        || s.too_long > 0
                    {
                        log("[wifid] EAPOL verworfen: wiedereinspielung ");
                        log_num(s.replays_dropped);
                        log(", key-version ");
                        log_num(s.bad_key_version);
                        log(", zu lang ");
                        log_num(s.too_long);
                        log(" (wiederholungen ");
                        log_num(s.replays_repeated);
                        log(")\n");
                    }
                }
            }
        }
        Some(EV_LINK_UP) => log("[wifid] link up — connected\n"),
        // LINK_DOWN: [op][reason] — the cell dropped us.
        //
        // Throw the supplicant away. It holds a PTK for a session
        // that no longer exists, and its SNonce has been used. The
        // driver reconnects and sends a fresh READY, which builds a new
        // one with new entropy — keeping the old one around would mean
        // answering the next msg1 with a stale nonce.
        Some(EV_LINK_DOWN) => {
            *sup = None;
            log("[wifid] link down (reason ");
            log(match ev.get(1) {
                Some(0) => "requested",
                Some(1) => "deauth",
                Some(2) => "lost",
                _ => "?",
            });
            log(") — supplicant dropped, waiting for a fresh READY\n");
        }
        _ => {}
    }
}

fn send_cmd(msg: &[u8]) {
    unsafe { npk_wifi_send_cmd(msg.as_ptr() as i32, msg.len() as i32) };
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

/// Fetch a config object into `buf` and return its value with trailing
/// whitespace (newline a text editor may append) trimmed. None on miss.
fn cfg_trim(v: &[u8]) -> &[u8] {
    let (mut a, mut b) = (0, v.len());
    while a < b && matches!(v[a], b' ' | b'\t' | b'\r') { a += 1; }
    while b > a && matches!(v[b - 1], b' ' | b'\t' | b'\r') { b -= 1; }
    &v[a..b]
}

/// Value of `key` in a `key: value` config, or None. `#` starts a comment;
/// only the first colon splits, so a passphrase-like value keeps its colons.
fn cfg_get<'a>(text: &'a [u8], key: &[u8]) -> Option<&'a [u8]> {
    for line in text.split(|&b| b == b'\n') {
        let line = cfg_trim(line);
        if line.is_empty() || line[0] == b'#' { continue; }
        let Some(c) = line.iter().position(|&b| b == b':') else { continue };
        if cfg_trim(&line[..c]) == key {
            return Some(cfg_trim(&line[c + 1..]));
        }
    }
    None
}

fn read_cfg(name: &[u8], buf: *mut u8, max: i32) -> Option<&'static [u8]> {
    let n = unsafe { npk_fetch(name.as_ptr() as i32, name.len() as i32, buf as i32, max) };
    if n <= 0 {
        return None;
    }
    let mut v = unsafe { core::slice::from_raw_parts(buf as *const u8, n as usize) };
    while let Some(&last) = v.last() {
        if matches!(last, b'\n' | b'\r' | b' ' | b'\t') {
            v = &v[..v.len() - 1];
        } else {
            break;
        }
    }
    Some(v)
}
