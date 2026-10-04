# `tools/wasm/wifid/wasm/src/lib.rs` @ 5e0102684

## L1-11 · `#![no_std]`

```
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
```

## L18-21 · `#[unsafe(link_section = ".npk.caps")]`

```
// Capabilities: NETCTL (0x80, the control channel) + READ (0x01, fetch the
// credential) + WRITE (0x02, write the debug log). The kernel grants EXACTLY
// these from the caps byte — so NETCTL alone (0x80) would have no READ and the
// autostart instance couldn't even load the PSK.
```

## L32-34 · `#[link(wasm_import_module = "env")]`

```
// Host functions are WASM imports from the `env` module, resolved by the
// kernel at instantiation. Naming the module explicitly is what makes them
// imports rather than ordinary undefined C symbols, which rust-lld rejects.
```

## L42-43 · `fn npk_print(ptr: i32, len: i32);`

```
// Terminal/framebuffer output (like the driver) — visible on serial-less HW;
// npk_log_serial is invisible on machines without a COM port (the HP).
```

## L46-47 · `fn npk_random_bytes(buf_ptr: i32, len: i32) -> i32;`

```
/// Kernel 0.329.0, `security::csprng`. Braucht KEINE Kapabilitaet —
/// wie `npk_unix_time`. Gibt die Zahl der geschriebenen Bytes oder -1.
```

## L55-56 · `static mut LOG_FLUSHED: usize = 0;`

```
/// Bytes already persisted. `log` only APPENDS to the buffer; `log_flush`
/// writes it out, and the main loop calls that once per poll round.
```

## L59-71 · `fn log_num(mut v: u32) {`

```
// Log to the terminal AND persist to npkFS `sys/log/wifid` — wifid runs in an
// invisible autostart window, so its log is read back with `fetch /sys/log/wifid`.
//
// The store used to happen on EVERY line, and `log_hex` calls this once per
// BYTE PAIR — a 32-byte key dump was 32 stores. Each store is an `fs::write`,
// which is N puts plus a full four-phase `commit_root`. Over WiFi, where this
// module talks constantly (rekeys, link changes, reconnects), those commits
// land in the middle of an OTA streaming download — and npkFS's `put`
// deliberately DEFERS its commit on the documented assumption of "N puts then
// exactly one commit_root". Over a cable this module is silent and the
// assumption holds; over WiFi it does not. Suspected in the repeated npkFS
// damage after OTA over WiFi, not proven.
/// Eine Dezimalzahl in den Log — ohne alloc, wie alles hier.
```

## L103-104 · `fn log_flush() {`

```
/// Persist the log if it grew. One store per poll round instead of one per
/// line — and none at all while nothing is happening.
```

## L132-133 · `const CMD_SET_KEY: u8 = 0x04;`

```
// ── control-channel wire format (docs/spec/WIFI_CLASS_ABI.md) ──────────────────────
// downlink (manager → driver)
```

## L137 · `const EV_READY: u8 = 0x83;`

```
// uplink (driver → manager)
```

## L143-150 · `const RSN_IE: [u8; 22] = [`

```
// Our RSN element (WPA2-PSK-CCMP) — MUST match the one the driver put in the
// assoc request, since it is echoed in 4-way msg2's key_data and the AP
// compares the two.
//
// **This is a second definition of the same thing**: the driver carries it as
// `RSN_IE_WPA2_CCMP_PSK` (wifi_rtl8822ce/src/lib.rs). Two places for one
// value drift, so `framecheck.py` holds them against each other byte for
// byte. Do not edit one without the other.
```

## L164-175 · `let (ssid, pass) = loop {`

```
// ── Load the credential from npkFS. The network settings live in one
// `key: value` file shared with the driver; the passphrase keeps its own
// object, because nothing but this module has business holding it:
//   store /sys/config/wifi     ssid: My Network
//   store /sys/config/wifi_psk my secret pass
// This plaintext-in-an-(at-rest-encrypted)-object is a bring-up provisional;
// a capability-gated keystore replaces it later (see project_keystore).
// Wait for the credential rather than exiting without one. On autostart this
// races the rest of boot, and a single failed read used to end the process
// for good — after which the driver associates, sends READY into the void,
// the AP gets no answer to msg1 and deauthenticates us. That presents as
// "connected but no DHCP lease", pointing at the wrong layer entirely.
```

## L193 · `let pmk = wpa2_pmk(pass, ssid);`

```
// ── Derive the PMK (PBKDF2-HMAC-SHA1, 4096 iters) — the std-tested core.
```

## L198-199 · `let ev_ptr = core::ptr::addr_of_mut!(EVENT_BUF) as *mut u8;`

```
// ── Resident supplicant loop. Runs on a worker core via autostart; drains
// control-channel events and drives the 4-way handshake to completion. ──
```

## L212 · `log_flush();`

```
// One store per round, not one per line.
```

## L214-221 · `const WAIT_WIFI_EVENT: i32 = 16;`

```
// **Wait for the driver's next event instead of polling for it.**
// Until 0.13.0 this slept 4 ms while a handshake was in flight and
// 50 ms otherwise — 250 wakes a second on a connected link, where
// nothing happens for minutes. The kernel now wakes us when the
// driver queues an event (`WAIT_WIFI_EVENT`), so a 4-way message is
// answered at once, not one poll interval later. The supplicant has
// no timers of its own: no deadline.
// docs/plan/CORES_AND_EVENTS.md, Stufe 2d.
```

## L229 · `Some(EV_READY) if ev.len() >= 13 => {`

```
// READY: [op][ap_mac 6][our_mac 6] → build the supplicant for this BSS.
```

## L235-244 · `let mut snonce = [0u8; 32];`

```
// SNonce: 32 REAL random bytes (802.11i §12.7.6.2).
//
// This used to be a fixed value derived from our own MAC, with
// the note "no npk_random host-fn yet". That reason expired in
// kernel 0.329.0: `npk_random_bytes` sits on `security::csprng`
// and needs no capability. A constant SNonce makes the PTK
// depend on the ANonce alone — an AP that repeats an ANonce
// (some do after a reboot) hands back the SAME PTK while our
// packet number restarts at 1, and it then drops our frames as
// replays. A standing link that carries nothing.
```

## L248-250 · `log("[wifid] no entropy for the SNonce — refusing the handshake\n");`

```
// **Kein Rueckfall auf einen erfundenen Wert.** Ein
// vorhersagbarer Nonce ist schlechter als kein Handschlag:
// er sieht aus wie einer.
```

## L258 · `Some(EV_EAPOL_RX) if ev.len() >= 3 => {`

```
// EAPOL_RX: [op][len u16][frame] → feed the 4-way state machine.
```

## L290-291 · `log("[wifid] group rekey — new GTK, acknowledging\n");`

```
// Answer first, install second: the AP starts using the new
// group key as soon as it has our acknowledgement.
```

## L299-303 · `Step::Ignore => {`

```
// **Ein `Ignore` ist seit 0.12.0 nicht mehr immer
// harmlos.** Drei Haerteregeln enden hier, und jede
// einzelne wuerde sonst als „nichts passiert" aussehen —
// genau die Form, die uns schon zweimal einen Lauf
// gekostet hat.
```

## L322-328 · `Some(EV_LINK_DOWN) => {`

```
// LINK_DOWN: [op][reason] — the cell dropped us.
//
// **Throw the supplicant away.** It holds a PTK for a session
// that no longer exists, and its SNonce has been used. The
// driver reconnects and sends a fresh READY, which builds a new
// one with new entropy — keeping the old one around would mean
// answering the next msg1 with a stale nonce.
```

## L348 · `fn send_tx_eapol(frame: &[u8]) {`

```
// TX_EAPOL: [op][len u16][frame].
```

## L358 · `fn send_set_key(group: bool, key_idx: u8, key: &[u8], rsc: &[u8; 6]) {`

```
// SET_KEY: [op][key_type][key_idx][cipher=4 CCMP][key_len][key..][rsc 6].
```

## L364 · `buf[3] = 4; // CCMP`

```
// CCMP
```

## L371-372 · `fn cfg_trim(v: &[u8]) -> &[u8] {`

```
/// Fetch a config object into `buf` and return its value with trailing
/// whitespace (newline a text editor may append) trimmed. None on miss.
```

## L380-381 · `fn cfg_get<'a>(text: &'a [u8], key: &[u8]) -> Option<&'a [u8]> {`

```
/// Value of `key` in a `key: value` config, or None. `#` starts a comment;
/// only the first colon splits, so a passphrase-like value keeps its colons.
```

