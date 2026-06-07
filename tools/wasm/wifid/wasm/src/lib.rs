//! wifid.wasm — the WiFi connection manager (WPA2 supplicant).
//!
//! Vendor-independent: it drives any vendor WiFi driver (e.g. wifi_ax200) over
//! the kernel-mediated WiFi-class control channel (see WIFI_CLASS_ABI.md). It
//! owns the credentials and the WPA2 4-way handshake; the vendor driver only
//! transports frames and installs the keys the supplicant computes.
//!
//! This first slice establishes the foundation: declare the NETCTL capability,
//! load the PSK from npkFS, derive the PMK (the std-tested [`wifid_core`]
//! crypto), and exercise the control channel. The EAPOL 4-way state machine
//! follows once the driver's EAPOL transport is wired.

#![no_std]

use wifid_core::wpa2_pmk;

// Manager side of the WiFi-class channel → declare NETCTL (bit 0x80). The
// kernel grants exactly that (gates npk_wifi_send_cmd / npk_wifi_poll_event).
#[unsafe(link_section = ".npk.caps")]
#[used]
static NPK_CAPS: [u8; 1] = [0x80];

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
}

fn log(s: &str) {
    unsafe { npk_print(s.as_ptr() as i32, s.len() as i32) };
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

// ── control-channel wire format (WIFI_CLASS_ABI.md) ──────────────────────
const CMD_SCAN: u8 = 0x01;
const EV_SCAN_AP: u8 = 0x81;
const EV_SCAN_DONE: u8 = 0x82;

// No heap needed yet; provide a tiny bump allocator only so `alloc`-free core
// links cleanly (wifid_core itself is allocation-free).

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

    // ── Exercise the control channel: ask the driver to scan.
    let scan = [CMD_SCAN];
    let r = unsafe { npk_wifi_send_cmd(scan.as_ptr() as i32, scan.len() as i32) };
    if r < 0 {
        log("[wifid] control channel send failed (driver not bound?)\n");
    } else {
        log("[wifid] SCAN command sent to driver\n");
    }

    // ── Resident loop: drain events from the driver and log them. The 4-way
    // handshake response to EAPOL_RX events lands in the next slice.
    let ev_ptr = core::ptr::addr_of_mut!(EVENT_BUF) as *mut u8;
    loop {
        loop {
            let len = unsafe { npk_wifi_poll_event(ev_ptr as i32, 2048) };
            if len <= 0 {
                break;
            }
            let ev = unsafe { core::slice::from_raw_parts(ev_ptr as *const u8, len as usize) };
            handle_event(ev);
        }
        unsafe { npk_sleep(200) };
    }
}

fn handle_event(ev: &[u8]) {
    match ev.first().copied() {
        Some(EV_SCAN_AP) => log("[wifid] event: SCAN_AP\n"),
        Some(EV_SCAN_DONE) => log("[wifid] event: SCAN_DONE\n"),
        Some(op) => {
            log_hex("[wifid] event op=", &[op]);
        }
        None => {}
    }
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
