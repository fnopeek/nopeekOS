//! wifid.wasm — the WiFi connection manager (WPA2 supplicant).
//!
//! Vendor-independent: it drives any vendor WiFi driver (e.g. wifi_ax200) over
//! the kernel-mediated WiFi-class control channel (see docs/spec/WIFI_CLASS_ABI.md). It
//! owns the credentials and the WPA2 4-way handshake; the vendor driver only
//! transports frames and installs the keys the supplicant computes.
//!
//! This first slice establishes the foundation: declare the NETCTL capability,
//! load the PSK from npkFS, derive the PMK (the std-tested [`wifid_core`]
//! crypto), and exercise the control channel. The EAPOL 4-way state machine
//! follows once the driver's EAPOL transport is wired.

#![no_std]

use wifid_core::eapol::{Step, Supplicant};
use wifid_core::wpa2_pmk;

// Capabilities: NETCTL (0x80, the control channel) + READ (0x01, fetch the
// credential) + WRITE (0x02, write the debug log). The kernel grants EXACTLY
// these from the caps byte — so NETCTL alone (0x80) would have no READ and the
// autostart instance couldn't even load the PSK.
#[unsafe(link_section = ".npk.caps")]
#[used]
static NPK_CAPS: [u8; 1] = [0x83];

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    log("[wifid] panic");
    loop {}
}

unsafe extern "C" {
    fn npk_fetch(name_ptr: i32, name_len: i32, buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_wifi_send_cmd(buf_ptr: i32, len: i32) -> i32;
    fn npk_wifi_poll_event(buf_ptr: i32, max: i32) -> i32;
    fn npk_sleep(ms: i32) -> i32;
    // Terminal/framebuffer output (like the driver) — visible on serial-less HW;
    // npk_log_serial is invisible on machines without a COM port (the HP).
    fn npk_print(ptr: i32, len: i32);
    fn npk_store(name_ptr: i32, name_len: i32, data_ptr: i32, data_len: i32) -> i32;
}

const LOG_CAP: usize = 8192;
static mut LOG_BUF: [u8; LOG_CAP] = [0; LOG_CAP];
static mut LOG_LEN: usize = 0;

// Log to the terminal AND persist to npkFS `sys/log/wifid` — wifid runs in an
// invisible autostart window, so its log is read back with `fetch /sys/log/wifid`.
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
        let name = b"sys/log/wifid";
        npk_store(name.as_ptr() as i32, name.len() as i32, buf as i32, l as i32);
    }
}

fn log_hex(prefix: &str, bytes: &[u8]) {
    log(prefix);
    let mut buf = [0u8; 2];
    for &b in bytes {
        let hi = b >> 4;
        let lo = b & 0xf;
        buf[0] = if hi < 10 { b'0' + hi } else { b'a' + hi - 10 };
        buf[1] = if lo < 10 { b'0' + lo } else { b'a' + lo - 10 };
        log(unsafe { core::str::from_utf8_unchecked(&buf) });
    }
    log("\n");
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

// Our RSN element (WPA2-PSK-CCMP) — MUST match the one the driver put in the
// assoc request, since it is echoed in 4-way msg2's key_data.
const RSN_IE: [u8; 22] = [
    0x30, 0x14, 0x01, 0x00, 0x00, 0x0f, 0xac, 0x04, 0x01, 0x00, 0x00, 0x0f, 0xac, 0x04, 0x01, 0x00,
    0x00, 0x0f, 0xac, 0x02, 0x00, 0x00,
];

static mut SSID_BUF: [u8; 64] = [0; 64];
static mut PSK_BUF: [u8; 128] = [0; 128];
static mut EVENT_BUF: [u8; 2048] = [0; 2048];

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    log("[wifid] WiFi manager start (WPA2 supplicant)\n");

    // ── Load the credential from npkFS — two objects so SSID and passphrase
    // can both contain spaces (set from the loop, e.g.
    //   store /sys/config/wifi_ssid My Network
    //   store /sys/config/wifi_psk  my secret pass
    // This plaintext-in-an-(at-rest-encrypted)-object is a bring-up provisional;
    // a capability-gated keystore replaces it later (see project_keystore).
    let ssid = match read_cfg(b"sys/config/wifi_ssid", core::ptr::addr_of_mut!(SSID_BUF) as *mut u8, 64) {
        Some(s) if !s.is_empty() => s,
        _ => {
            log("[wifid] no sys/config/wifi_ssid — set it: store /sys/config/wifi_ssid <name>. Idle.\n");
            return;
        }
    };
    let pass = match read_cfg(b"sys/config/wifi_psk", core::ptr::addr_of_mut!(PSK_BUF) as *mut u8, 128) {
        Some(p) if p.len() >= 8 => p,
        _ => {
            log("[wifid] no/short sys/config/wifi_psk — set it: store /sys/config/wifi_psk <pass>. Idle.\n");
            return;
        }
    };
    log("[wifid] credential loaded for SSID '");
    log(unsafe { core::str::from_utf8_unchecked(ssid) });
    log("'\n");

    // ── Derive the PMK (PBKDF2-HMAC-SHA1, 4096 iters) — the std-tested core.
    let pmk = wpa2_pmk(pass, ssid);
    log_hex("[wifid] PMK = ", &pmk);
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
        unsafe { npk_sleep(50) };
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
            // SNonce: a fixed bring-up value (functional; real entropy is a TODO —
            // see project_keystore / no npk_random host-fn yet).
            let mut snonce = [0u8; 32];
            for (i, b) in snonce.iter_mut().enumerate() {
                *b = 0x5a ^ sa[i % 6] ^ (i as u8);
            }
            *sup = Some(Supplicant::new(*pmk, aa, sa, snonce, &RSN_IE));
            log("[wifid] READY — supplicant armed for 4-way\n");
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
                        send_set_key(true, id, gtk, &[0u8; 6]);
                    }
                    send_cmd(&[CMD_AUTHORIZED]);
                    log("[wifid] *** 4-way complete — AUTHORIZED ***\n");
                }
                Step::Fail => log("[wifid] 4-way FAILED (bad MIC / unwrap)\n"),
                Step::Ignore => {}
            }
        }
        Some(EV_LINK_UP) => log("[wifid] link up — connected\n"),
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
